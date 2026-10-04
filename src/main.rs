//! depup - 多言語対応の依存関係アップデーター CLI ツール
//!
//! 複数のプログラミング言語の依存関係を更新するツール:
//! - Node.js（package.json）対応
//! - Python（pyproject.toml）対応
//! - Rust（Cargo.toml）対応
//! - Go（go.mod）対応
//! - Ruby（Gemfile）対応
//! - PHP（composer.json）対応
//! - Java（build.gradle / build.gradle.kts）対応

use clap::Parser;
use colored::Colorize;
use depup::cargo_rollback::report::{DisplayedUpdate, LockedVersionMismatch, lock_mismatches};
use depup::cli::CliArgs;
use depup::config::DepupConfig;
use depup::domain::{Language, UpdateResult};
use depup::global_config::{GlobalConfig, resolve_max_change, resolve_osv};
use depup::manifest::RegistryLockEntries;
use depup::orchestrator::{
    LOCK_AGE_AUDIT_BUDGET, LockAgeAdjustment, LockAgeAuditResult, LockAgeStatus, Orchestrator,
    OrchestratorResult, PreferredVersions,
};
use depup::output::{OutputConfig, create_formatter};
use depup::package_manager::SystemPackageManager;
use depup::progress::Progress;
use depup::update::AgePolicy;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = CliArgs::parse();

    // バージョンフラグの処理
    if args.print_version {
        println!("depup {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // --cd が指定されている場合はディレクトリを変更
    if let Some(ref dir) = args.directory
        && let Err(e) = std::env::set_current_dir(dir)
    {
        eprintln!(
            "Error: cannot change to directory '{}': {}",
            dir.display(),
            e
        );
        return ExitCode::FAILURE;
    }

    // メインロジックを実行してエラーを処理
    match run(args).await {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// アプリケーションのメインロジック
async fn run(args: CliArgs) -> anyhow::Result<ExitCode> {
    let mut args = args;

    // グローバル設定 (~/.config/depup/config.toml) を読み込み、
    // CLI > config > 組み込みデフォルトの優先順位で age / osv を確定する。
    let global_config = GlobalConfig::load();
    // age はプロジェクト設定 (pnpm/bun の minimumReleaseAge) と統合判定するため
    // orchestrator 側の build_filter で最終解決する。main では生の CLI 値を保持。
    args.osv = resolve_osv(args.osv, args.no_osv, global_config.as_ref());
    args.max_change = resolve_max_change(args.max_change, global_config.as_ref());

    // verbose モードではバージョン情報を表示
    if args.verbose {
        eprintln!("depup v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Target: {}", args.path.display());
        if args.dry_run {
            eprintln!("Mode: dry-run");
        }
        match args.age {
            Some(age) => eprintln!("Age filter (CLI): {}s", age.as_secs()),
            None if args.no_age => eprintln!(
                "Age filter: --no-age (still overridden by project minimumReleaseAge if present)"
            ),
            None => {
                eprintln!("Age filter: (resolved by orchestrator from project / config / default)")
            }
        }
        eprintln!(
            "OSV vulnerability check: {}",
            if args.osv { "enabled" } else { "disabled" }
        );
    }

    // .depup モノレポ設定を確認
    let monorepo_config = DepupConfig::from_dir(&args.path);

    // オーケストレーターを作成 (global_config を渡して age 解決に利用)
    let orchestrator = Orchestrator::new(args.clone())?.with_global_config(global_config);

    let (result, monorepo_dirs) = if let Some(config) = monorepo_config {
        let dirs = config.directories_with_root(&args.path);
        if args.verbose {
            eprintln!("Monorepo mode: {} directories", dirs.len());
            for dir in &dirs {
                eprintln!("  - {}", dir.display());
            }
        }
        let r = orchestrator.run_directories(&dirs).await;
        (r, Some(dirs))
    } else {
        let r = orchestrator.run().await;
        (r, None)
    };

    // CLI オプションに基づいて出力フォーマッターを作成
    let output_config =
        OutputConfig::from_cli(args.json, args.diff, args.verbose, args.quiet, args.dry_run);
    let formatter = create_formatter(output_config);

    // 結果を出力
    let mut stdout = io::stdout().lock();
    formatter.format(&result, &mut stdout)?;
    stdout.flush()?;

    // verbose モードではエラーを表示
    if args.verbose && !result.errors.is_empty() {
        eprintln!();
        eprintln!("Errors encountered:");
        for error in &result.errors {
            eprintln!("  - {}", error);
        }
    }

    let mut age_audit_failed = false;
    // dry-run でない場合、要求があればパッケージマネージャの install を実行
    if args.install && !args.dry_run {
        let lock_baselines =
            collect_rust_lock_baselines(&orchestrator.rust_lock_boundary(), &result);
        // 他の言語の install が失敗しても、変更された Rust lock の監査は行う。
        let install_result = run_package_installs(&args, &result, &monorepo_dirs, &orchestrator);
        age_audit_failed =
            enforce_rust_lock_age(&args, &orchestrator, &result, &lock_baselines).await;

        // 画面に出した更新先と、実際に Cargo.lock へ入った版の食い違いを知らせる。
        // 結果の一覧は install 前に出しているので、ここで別に注記する
        report_rust_lock_mismatches(&orchestrator.rust_lock_boundary(), &result);
        install_result?;
    }

    // 適切な終了コードを返す。
    // OSV 警告は「脆弱な候補を検出して安全な版へフォールバックした」という
    // 設計どおりの正常動作の通知なので、エラー扱い (exit code 2) にしない。
    let has_errors = result
        .errors
        .iter()
        .any(|e| !matches!(e, depup::orchestrator::OrchestratorError::OsvWarning { .. }));

    if has_errors || age_audit_failed {
        // 部分的な成功 - 一部エラーが発生
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// パッケージマネージャの install を実行する (単一ディレクトリとモノレポの両方に対応)
fn run_package_installs(
    args: &CliArgs,
    result: &OrchestratorResult,
    monorepo_dirs: &Option<Vec<PathBuf>>,
    orchestrator: &Orchestrator,
) -> anyhow::Result<()> {
    // ディレクトリ -> install が必要な言語のマップを構築
    let install_map = build_install_map(result, monorepo_dirs, &args.path);

    if install_map.is_empty() {
        return Ok(());
    }

    let pm_runner = SystemPackageManager::new();

    if args.verbose {
        eprintln!();
        eprintln!("Running package manager install...");
        if install_map
            .iter()
            .any(|(dir, _)| orchestrator.resolved_age_policy_for(dir).min_age.is_some())
        {
            // age が有効な場合、transitive 依存へネイティブ対応しない PM を通知する。
            //
            // 判定は言語単位ではなく **実際に選ばれる PM 単位**で行う。Node の
            // transitive age は pnpm、Python は uv だけの機能なので、言語で判定すると
            // npm / yarn / bun / pip / poetry / rye / pipenv のプロジェクトで通知が
            // 出ず、「transitive にも cooldown が効いた」と誤解させてしまう。
            // どの PM が対応済みかは Language 側が単一の情報源。
            let mut unsupported: Vec<String> = install_map
                .iter()
                .flat_map(|(dir, langs)| langs.iter().map(move |lang| (dir, *lang)))
                .filter_map(|(dir, lang)| {
                    let (_, pm) = pm_runner.resolve_package_manager(lang, dir)?;
                    (!lang.pm_has_native_transitive_age_support(pm)).then(|| pm.to_string())
                })
                .collect();
            unsupported.sort();
            unsupported.dedup();
            if !unsupported.is_empty() {
                eprintln!(
                    "  Note: --age applies to direct deps only for: {} (no native transitive-age support)",
                    unsupported.join(", ")
                );
            }
        }
    }

    let mut any_install_failed = false;

    // install コマンドは出力をキャプチャするため完了まで何も表示されない。
    // `cargo update` / `pnpm install` は分単位でかかることがあり、無表示だと
    // フリーズと区別がつかないので、実行中の言語をスピナーで示す。
    let mut progress = Progress::new(!args.quiet);

    for (dir, languages) in &install_map {
        for language in languages {
            progress.spinner(&format!("Running {} install...", language.display_name()));
            let policy = install_age_policy(orchestrator, result, dir, *language)
                .map_err(anyhow::Error::msg)?;
            let install_results = [pm_runner.run_install_with_age_policy(*language, dir, &policy)];
            progress.finish_and_clear();

            for install_result in &install_results {
                if install_result.command.is_empty() {
                    continue;
                }

                if install_result.success {
                    if args.verbose {
                        eprintln!(
                            "  {} install completed: {} ({})",
                            install_result.language.display_name(),
                            install_result.command,
                            dir.display()
                        );
                    }
                } else {
                    eprintln!(
                        "  {} install failed: {} ({})",
                        install_result.language.display_name(),
                        install_result.command,
                        dir.display()
                    );
                    if !install_result.stderr.is_empty() {
                        eprintln!("    {}", install_result.stderr);
                    }
                    any_install_failed = true;
                }
            }
        }
    }

    if any_install_failed {
        anyhow::bail!("Some package manager installs failed");
    }

    Ok(())
}

/// install が解決する配下のマニフェストの age を統合する。
fn install_age_policy(
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
    dir: &Path,
    language: Language,
) -> Result<AgePolicy, String> {
    let mut policy = orchestrator.resolved_age_policy_for_language(dir, language)?;
    for manifest in &result.summary.manifests {
        if manifest.language == language
            && manifest.path.starts_with(dir)
            && let Some(parent) = manifest.path.parent()
        {
            policy.merge(&orchestrator.resolved_age_policy_for_language(parent, language)?);
        }
    }
    Ok(policy)
}

/// 更新対象の Rust プロジェクトについて、install 前の Cargo.lock の内容を控える。
///
/// post-install の age 監査は「install によって新しく入った / 版が変わった」依存だけを
/// 対象にする。その差分を取るための基準値。install 前に Cargo.lock がまだ無い
/// ディレクトリは記録しない (install で生成された lock は全エントリが新規となり、
/// 監査側で空のベースラインとして扱われる)。
fn collect_rust_lock_baselines(
    boundary: &Path,
    result: &OrchestratorResult,
) -> HashMap<PathBuf, RegistryLockEntries> {
    let mut baselines: HashMap<PathBuf, RegistryLockEntries> = HashMap::new();
    for manifest in &result.summary.manifests {
        if manifest.language != Language::Rust || !manifest.has_updates() {
            continue;
        }
        let Some(parent) = manifest.path.parent() else {
            continue;
        };
        let Some(lock_path) = depup::manifest::find_cargo_lock_upward(parent, boundary) else {
            continue;
        };
        let lock_dir = lock_path.parent().unwrap_or(parent).to_path_buf();
        baselines
            .entry(lock_dir)
            .or_insert_with(|| depup::manifest::read_registry_entries(&lock_path));
    }
    baselines
}

/// 起動場所や更新対象に含まれないメンバーも、共有 lock の制約へ統合する。
fn collect_rust_lock_policies(
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
) -> HashMap<PathBuf, AgePolicy> {
    let boundary = orchestrator.rust_lock_boundary();
    let mut paths: Vec<PathBuf> = result
        .summary
        .manifests
        .iter()
        .filter(|manifest| manifest.language == Language::Rust)
        .map(|manifest| manifest.path.clone())
        .collect();
    let mut roots = Vec::new();
    for path in &paths {
        if let Some(lock) = path
            .parent()
            .and_then(|dir| depup::manifest::find_cargo_lock_upward(dir, &boundary))
            && let Some(root) = lock.parent()
            && !roots.iter().any(|dir| dir == root)
        {
            roots.push(root.to_path_buf());
        }
    }
    for root in roots {
        paths.extend(
            depup::manifest::detect_manifests(&root)
                .into_iter()
                .filter(|manifest| manifest.language == Language::Rust)
                .map(|manifest| manifest.path),
        );
    }
    let mut policies: HashMap<PathBuf, AgePolicy> = HashMap::new();
    for path in paths {
        let Some(parent) = path.parent() else {
            continue;
        };
        let Some(lock) = depup::manifest::find_cargo_lock_upward(parent, &boundary) else {
            continue;
        };
        let dir = lock.parent().unwrap_or(parent).to_path_buf();
        let policy = orchestrator.resolved_age_policy_for(parent);
        policies
            .entry(dir)
            .and_modify(|existing| existing.merge(&policy))
            .or_insert(policy);
    }
    policies
}

/// Rust プロジェクト (Cargo.toml を含む) ディレクトリに対し、
/// `--age` を install で変わった crate (直接依存・推移依存とも) にも適用する。
/// install 済み Cargo.lock を走査し、age 違反の crate を古いバージョンへ差し戻す。
async fn enforce_rust_lock_age(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
    baselines: &HashMap<PathBuf, RegistryLockEntries>,
) -> bool {
    // 対象となる Rust プロジェクトディレクトリを収集。
    // workspace メンバーや Tauri (src-tauri) の Cargo.lock はマニフェストと別の
    // 階層にあることがあるため、マニフェストのディレクトリから上方向に lock を
    // 探し、lock が実在するディレクトリを監査対象にする。
    let mut rust_dirs: Vec<PathBuf> = Vec::new();
    // judge がこの実行で選んだ版 (画面に出した更新先) を lock ごとにまとめる。
    // 差し戻し先の第一候補になるので、差し戻せれば表示と lock の版が揃う
    let mut preferred: HashMap<PathBuf, PreferredVersions> = HashMap::new();
    let policies = collect_rust_lock_policies(orchestrator, result);
    let mut missing_lock = false;
    for manifest in &result.summary.manifests {
        // 更新がなかった Rust manifest は cargo update も走らないため audit 不要
        if manifest.language != Language::Rust || !manifest.has_updates() {
            continue;
        }
        let Some(parent) = manifest.path.parent() else {
            continue;
        };
        let Some(lock_path) =
            depup::manifest::find_cargo_lock_upward(parent, &orchestrator.rust_lock_boundary())
        else {
            if orchestrator
                .resolved_age_policy_for(parent)
                .min_age
                .is_some()
            {
                eprintln!(
                    "  {} — cannot audit --age: Cargo.lock not found",
                    parent.display()
                );
                missing_lock = true;
            }
            continue;
        };
        let lock_dir = lock_path.parent().unwrap_or(parent).to_path_buf();
        let versions = preferred.entry(lock_dir.clone()).or_default();
        for update in manifest.updates() {
            if let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = update
                && dependency.git_source.is_none()
            {
                versions
                    .entry(dependency.name.clone())
                    .or_default()
                    .push(new_version.clone());
            }
        }
        if !rust_dirs.contains(&lock_dir) {
            rust_dirs.push(lock_dir);
        }
    }

    if rust_dirs.is_empty() {
        return missing_lock;
    }

    if args.verbose {
        eprintln!();
        eprintln!("Enforcing --age on crates changed in Cargo.lock...");
    }

    // 監査は crates.io の 1 リクエスト/秒 制限に律速される。何件目を照会中かを
    // 出さないと、対象が多いときに無言のフリーズと区別がつかない。
    let mut progress = Progress::new(!args.quiet);
    progress.start(0, "Auditing Cargo.lock against --age");
    let bar = progress.bar();

    let mut has_unresolved = missing_lock;
    for dir in &rust_dirs {
        let Some(policy) = policies.get(dir).filter(|policy| policy.min_age.is_some()) else {
            continue;
        };
        let baseline = baselines.get(dir).cloned().unwrap_or_default();
        let preferred = preferred.get(dir).cloned().unwrap_or_default();
        let audit = orchestrator
            .enforce_lock_age_rust_with_policy(dir, policy, &baseline, &preferred, bar.as_ref())
            .await;
        has_unresolved |= audit.has_unresolved();
        let lines = lock_age_report_lines(dir, &audit, args.verbose);
        if !lines.is_empty() {
            progress.suspend(|| {
                for line in &lines {
                    if line.warning {
                        eprintln!("{}", line.text.yellow());
                    } else {
                        eprintln!("{}", line.text);
                    }
                }
            });
        }
    }

    progress.finish_and_clear();
    has_unresolved
}

/// stderr に出す 1 行
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportLine {
    text: String,
    /// `--age` を満たせなかった・確かめられなかったことを伝える行 (黄色で出す)
    warning: bool,
}

impl ReportLine {
    fn info(text: String) -> Self {
        Self {
            text,
            warning: false,
        }
    }

    fn warning(text: String) -> Self {
        Self {
            text,
            warning: true,
        }
    }
}

/// 1 つの Cargo.lock の監査結果を、stderr に出す行へ変換する。
///
/// 差し戻せなかった crate の件数は `--verbose` が無くても必ず出す。`--age` は供給網
/// 対策として使われるので、満たせなかったことを黙って捨てると、差し戻せた分だけが
/// 表示されて `--age` が効いたように見えてしまう。
fn lock_age_report_lines(dir: &Path, audit: &LockAgeAuditResult, verbose: bool) -> Vec<ReportLine> {
    let mut lines = Vec::new();
    let dir = dir.display();

    // 元の Cargo.lock を戻せなかった等、差し戻せなかったことより深刻な問題は必ず出す
    for problem in &audit.problems {
        lines.push(ReportLine::warning(format!("  {dir} — {problem}")));
    }

    if audit.unchecked > 0 {
        lines.push(ReportLine::warning(format!(
            "  {dir} — age audit stopped after {}s; {} crate(s) left unchecked",
            LOCK_AGE_AUDIT_BUDGET.as_secs(),
            audit.unchecked
        )));
    }

    if audit.adjustments.is_empty() {
        // 予算切れで未検証が残っている場合は「全て age 内」とは言い切れない
        // (直前に未検証件数を警告済み)
        if verbose && audit.unchecked == 0 && audit.problems.is_empty() {
            lines.push(ReportLine::info(format!(
                "  {dir} — all crates changed by install are within --age"
            )));
        }
        return lines;
    }

    let rolled_back: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| a.status.is_resolved())
        .collect();
    let unverified: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| a.status.is_unverified())
        .collect();
    let restored: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| a.status.is_restored())
        .collect();
    let remaining: Vec<&LockAgeAdjustment> = audit
        .adjustments
        .iter()
        .filter(|a| !a.status.is_resolved() && !a.status.is_restored() && !a.status.is_unverified())
        .collect();

    if !rolled_back.is_empty() {
        lines.push(ReportLine::info(format!(
            "  {dir} — {} crate(s) rolled back to satisfy --age:",
            rolled_back.len()
        )));
        for adj in &rolled_back {
            let to = match adj.status {
                LockAgeStatus::Removed => "removed",
                _ => adj.to.as_deref().unwrap_or("?"),
            };
            lines.push(ReportLine::info(format!(
                "    {} {} → {}",
                adj.name, adj.from, to
            )));
        }
    }

    // install 前の版へ戻しただけで、その版も期間を満たさないもの。「satisfy --age」と
    // 一緒に並べると、期間を満たしたと読めてしまう
    if !restored.is_empty() {
        lines.push(ReportLine::warning(format!(
            "  {dir} — {} crate(s) returned to the version locked before the install, which is also newer than --age:",
            restored.len()
        )));
        for adj in &restored {
            lines.push(ReportLine::warning(format!(
                "    {} {} → {}",
                adj.name,
                adj.from,
                adj.to.as_deref().unwrap_or("?")
            )));
        }
    }

    if !remaining.is_empty() {
        if verbose {
            lines.push(ReportLine::warning(format!(
                "  {dir} — {} crate(s) could not be rolled back to satisfy --age:",
                remaining.len()
            )));
            for adj in &remaining {
                lines.push(ReportLine::warning(format!(
                    "    {} ({}): {}",
                    adj.name,
                    adj.from,
                    lock_age_status_detail(&adj.status)
                )));
            }
        } else {
            lines.push(ReportLine::warning(format!(
                "  {dir} — {} crate(s) could not be rolled back to satisfy --age (use --verbose for details)",
                remaining.len()
            )));
        }
    }

    if !unverified.is_empty() {
        if verbose {
            lines.push(ReportLine::warning(format!(
                "  {dir} — {} crate(s) could not be checked against --age:",
                unverified.len()
            )));
            for adj in &unverified {
                lines.push(ReportLine::warning(format!(
                    "    {} ({}): {}",
                    adj.name,
                    adj.from,
                    lock_age_status_detail(&adj.status)
                )));
            }
        } else {
            lines.push(ReportLine::warning(format!(
                "  {dir} — {} crate(s) could not be checked against --age: release date unavailable (use --verbose for details)",
                unverified.len()
            )));
        }
    }

    lines
}

/// 差し戻せなかった理由の説明
fn lock_age_status_detail(status: &LockAgeStatus) -> String {
    match status {
        LockAgeStatus::Downgraded => "rolled back".to_string(),
        LockAgeStatus::Removed => "removed from Cargo.lock".to_string(),
        LockAgeStatus::Restored => {
            "returned to the version locked before the install (also newer than --age)".to_string()
        }
        LockAgeStatus::NoOlderCandidate => "no older version satisfies --age".to_string(),
        LockAgeStatus::UpdateCommandFailed(msg) => format!("cargo update failed: {msg}"),
        LockAgeStatus::BlockedByManifest(requirement) => format!(
            "Cargo.toml requires `{requirement}`, which excludes every version that satisfies --age"
        ),
        LockAgeStatus::NotAttempted(reason) => format!("not attempted: {reason}"),
        LockAgeStatus::ReleaseDateUnavailable => "release date unavailable".to_string(),
    }
}

/// `--install` の後、画面に出した更新先と Cargo.lock に入った版が違う Rust の更新を注記する。
///
/// 表示する更新先は Cargo.toml に書いた版だが、`cargo update` はその版要求 (`^0.2.128`)
/// を満たす最新版 (`0.2.129`) を lock に入れる。差し戻しで揃えられなかった場合や、
/// `--max-change` / OSV で古い版を選んだ場合は、表示とビルドに使われる版が食い違う。
fn report_rust_lock_mismatches(boundary: &Path, result: &OrchestratorResult) {
    for manifest in &result.summary.manifests {
        if manifest.language != Language::Rust || !manifest.has_updates() {
            continue;
        }
        let Some(parent) = manifest.path.parent() else {
            continue;
        };
        let Some(lock_path) = depup::manifest::find_cargo_lock_upward(parent, boundary) else {
            continue;
        };
        let entries = depup::manifest::read_registry_entries(&lock_path);
        let updates: Vec<DisplayedUpdate> = manifest
            .updates()
            .filter_map(|update| match update {
                UpdateResult::Update {
                    dependency,
                    new_version,
                    ..
                } if dependency.git_source.is_none() => Some(DisplayedUpdate {
                    name: dependency.name.clone(),
                    from: dependency.version_spec.display_version(),
                    to: new_version.clone(),
                }),
                _ => None,
            })
            .collect();
        let mismatches = lock_mismatches(&updates, &entries);
        for line in lock_mismatch_lines(&manifest.path, &mismatches) {
            eprintln!("{}", line.yellow());
        }
    }
}

/// 表示と lock の食い違いを stderr に出す行へ変換する
fn lock_mismatch_lines(manifest_path: &Path, mismatches: &[LockedVersionMismatch]) -> Vec<String> {
    if mismatches.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "  {} — {} update(s) locked at a different version than shown:",
        manifest_path.display(),
        mismatches.len()
    )];
    for mismatch in mismatches {
        lines.push(format!(
            "    {} {} → {} (locked: {})",
            mismatch.name, mismatch.from, mismatch.to, mismatch.locked
        ));
    }
    lines
}

/// 結果からディレクトリ -> install が必要な言語のマップを構築する
fn nearest_monorepo_dir(
    manifest_path: &Path,
    monorepo_dirs: &[PathBuf],
    fallback: &Path,
) -> PathBuf {
    monorepo_dirs
        .iter()
        .filter(|dir| manifest_path.starts_with(dir))
        .max_by_key(|dir| dir.components().count())
        .cloned()
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn build_install_map(
    result: &OrchestratorResult,
    monorepo_dirs: &Option<Vec<PathBuf>>,
    default_path: &Path,
) -> Vec<(PathBuf, Vec<Language>)> {
    let mut dir_langs: HashMap<PathBuf, Vec<Language>> = HashMap::new();

    for manifest in &result.summary.manifests {
        if !manifest.has_updates() {
            continue;
        }

        // このマニフェストが属するディレクトリを特定
        let working_dir = if let Some(dirs) = monorepo_dirs {
            let manifest_path = &manifest.path;
            nearest_monorepo_dir(manifest_path, dirs, default_path)
        } else {
            default_path.to_path_buf()
        };

        let entry = dir_langs.entry(working_dir).or_default();
        if !entry.contains(&manifest.language) {
            entry.push(manifest.language);
        }
    }

    // `HashMap` の `RandomState` はインスタンスごとにシードが変わるため、そのまま
    // collect するとモノレポで install を走らせるディレクトリの順序が実行のたびに
    // 入れ替わる。verbose 出力や失敗時の stderr の行順が変わると CI のログ比較で
    // 偽の差分になるので、パス順に固定する。
    let mut install_map: Vec<(PathBuf, Vec<Language>)> = dir_langs.into_iter().collect();
    install_map.sort_by(|a, b| a.0.cmp(&b.0));
    install_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use depup::domain::{
        Dependency, ManifestUpdateResult, UpdateResult, UpdateSummary, VersionSpec, VersionSpecKind,
    };

    #[test]
    fn shared_install_includes_a_member_with_no_updates_and_does_not_mix_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let member = root.join("member");
        let sibling = temp.path().join("other");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(member.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
        std::fs::write(sibling.join(".npmrc"), "minimum-release-age=90d\n").unwrap();
        let mut args = CliArgs::parse_from(["depup", "--no-age"]);
        args.path = temp.path().to_path_buf();
        let orchestrator = Orchestrator::new(args).unwrap();
        let mut result = result_with_update(root.join("package.json"), Language::Node);
        for dir in [&member, &sibling] {
            result.summary.add_manifest(ManifestUpdateResult::new(
                dir.join("package.json"),
                Language::Node,
            ));
        }
        let policy = install_age_policy(&orchestrator, &result, &root, Language::Node).unwrap();
        assert_eq!(
            policy.min_age,
            Some(std::time::Duration::from_secs(30 * 86400))
        );
        assert!(policy.exemptions.is_empty());
    }

    #[tokio::test]
    async fn a_missing_lock_is_an_unverified_age_audit() {
        let temp = tempfile::tempdir().unwrap();
        let mut args = CliArgs::parse_from(["depup", "--age", "2w", "--quiet"]);
        args.path = temp.path().to_path_buf();
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let result = result_with_update(temp.path().join("Cargo.toml"), Language::Rust);
        assert!(enforce_rust_lock_age(&args, &orchestrator, &result, &HashMap::new()).await);
    }

    #[test]
    fn lock_policy_includes_siblings_when_started_inside_a_member() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let member = root.join("member");
        let other = root.join("other");
        for dir in [&member, &other] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                "[package]\nname = 'example'\nversion = '1.0.0'\n",
            )
            .unwrap();
        }
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = ['member', 'other']\n",
        )
        .unwrap();
        std::fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
        std::fs::write(other.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
        let mut args = CliArgs::parse_from(["depup", "--no-age"]);
        args.path = member.clone();
        let orchestrator = Orchestrator::new(args).unwrap();
        let result = result_with_update(member.join("Cargo.toml"), Language::Rust);
        let policies = collect_rust_lock_policies(&orchestrator, &result);
        assert_eq!(policies.len(), 1);
        let policy = policies.values().next().unwrap();
        assert_eq!(
            policy.min_age,
            Some(std::time::Duration::from_secs(30 * 86400))
        );
        assert!(policy.exemptions.is_empty());
    }

    fn result_with_update(path: PathBuf, language: Language) -> OrchestratorResult {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "1.0.0", "1.0.0");
        let dep = Dependency::production("pkg", spec, language);
        let mut manifest = ManifestUpdateResult::new(path, language);
        manifest.add_result(UpdateResult::update(dep, "2.0.0"));

        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_nearest_monorepo_dir_uses_deepest_match() {
        let root = PathBuf::from("/repo");
        let app = PathBuf::from("/repo/apps/web");
        let manifest = app.join("package.json");
        let dirs = vec![root.clone(), app.clone()];

        assert_eq!(nearest_monorepo_dir(&manifest, &dirs, &root), app);
    }

    fn adjustment(
        name: &str,
        from: &str,
        to: Option<&str>,
        status: LockAgeStatus,
    ) -> LockAgeAdjustment {
        LockAgeAdjustment {
            name: name.to_string(),
            from: from.to_string(),
            to: to.map(str::to_string),
            status,
        }
    }

    fn texts(lines: &[ReportLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    /// 差し戻せなかった crate の件数は `--verbose` が無くても 1 行出す。
    /// 見出しは直接依存も含むので `transitive dep(s)` ではなく `crate(s)`
    #[test]
    fn test_lock_age_report_lines_counts_failures_without_verbose() {
        let audit = LockAgeAuditResult {
            adjustments: vec![
                adjustment("cc", "1.5.1", Some("1.4.6"), LockAgeStatus::Downgraded),
                adjustment(
                    "wasm-bindgen",
                    "0.2.129",
                    None,
                    LockAgeStatus::UpdateCommandFailed("conflict".to_string()),
                ),
                adjustment("web-sys", "0.3.106", None, LockAgeStatus::NoOlderCandidate),
            ],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("./wasm"), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  ./wasm — 1 crate(s) rolled back to satisfy --age:",
                "    cc 1.5.1 → 1.4.6",
                "  ./wasm — 2 crate(s) could not be rolled back to satisfy --age (use --verbose for details)",
            ]
        );
        assert!(!lines[0].warning);
        assert!(lines[2].warning);
    }

    /// `--verbose` では差し戻せなかった理由を 1 件ずつ出す
    #[test]
    fn test_lock_age_report_lines_lists_reasons_with_verbose() {
        let audit = LockAgeAuditResult {
            adjustments: vec![
                adjustment(
                    "foo",
                    "2.1.3",
                    None,
                    LockAgeStatus::BlockedByManifest("^2.1.3".to_string()),
                ),
                adjustment(
                    "bar",
                    "1.0.2",
                    None,
                    LockAgeStatus::NotAttempted("audit time budget ran out".to_string()),
                ),
            ],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, true);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — 2 crate(s) could not be rolled back to satisfy --age:",
                "    foo (2.1.3): Cargo.toml requires `^2.1.3`, which excludes every version that satisfies --age",
                "    bar (1.0.2): not attempted: audit time budget ran out",
            ]
        );
    }

    /// 解き直しで lock から外れた crate も違反が解消したものとして数える
    #[test]
    fn test_lock_age_report_lines_shows_removed_crates_as_rolled_back() {
        let audit = LockAgeAuditResult {
            adjustments: vec![adjustment("tokio", "1.53.1", None, LockAgeStatus::Removed)],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — 1 crate(s) rolled back to satisfy --age:",
                "    tokio 1.53.1 → removed",
            ]
        );
    }

    /// 公開日を取れなかった crate は「差し戻せなかった」とは分けて数える
    #[test]
    fn test_lock_age_report_lines_separates_unverified_crates() {
        let audit = LockAgeAuditResult {
            adjustments: vec![adjustment(
                "private-crate",
                "0.1.0",
                None,
                LockAgeStatus::ReleaseDateUnavailable,
            )],
            unchecked: 3,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — age audit stopped after 180s; 3 crate(s) left unchecked",
                "  . — 1 crate(s) could not be checked against --age: release date unavailable (use --verbose for details)",
            ]
        );
        assert!(lines.iter().all(|line| line.warning));
    }

    /// 何も変えずに全部 age 内なら、`--verbose` のときだけその旨を出す
    #[test]
    fn test_lock_age_report_lines_quiet_when_everything_is_within_age() {
        let audit = LockAgeAuditResult::default();
        assert!(lock_age_report_lines(Path::new("."), &audit, false).is_empty());
        assert_eq!(
            texts(&lock_age_report_lines(Path::new("."), &audit, true)),
            vec!["  . — all crates changed by install are within --age"]
        );
    }

    /// install 前の版へ戻しただけで、その版も期間内のものは「satisfy --age」と分けて出す
    #[test]
    fn test_lock_age_report_lines_separates_restored_crates() {
        let audit = LockAgeAuditResult {
            adjustments: vec![
                adjustment("cc", "1.5.1", Some("1.4.6"), LockAgeStatus::Downgraded),
                adjustment("foo", "1.50.1", Some("1.50.0"), LockAgeStatus::Restored),
            ],
            unchecked: 0,
            problems: Vec::new(),
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec![
                "  . — 1 crate(s) rolled back to satisfy --age:",
                "    cc 1.5.1 → 1.4.6",
                "  . — 1 crate(s) returned to the version locked before the install, which is also newer than --age:",
                "    foo 1.50.1 → 1.50.0",
            ]
        );
        assert!(lines[2].warning);
    }

    /// 元の Cargo.lock を戻せなかった等の問題は `--verbose` が無くても必ず出す
    #[test]
    fn test_lock_age_report_lines_always_shows_problems() {
        let audit = LockAgeAuditResult {
            adjustments: Vec::new(),
            unchecked: 0,
            problems: vec!["restoring the previous Cargo.lock also failed".to_string()],
        };

        let lines = lock_age_report_lines(Path::new("."), &audit, false);

        assert_eq!(
            texts(&lines),
            vec!["  . — restoring the previous Cargo.lock also failed"]
        );
        assert!(lines[0].warning);
    }

    /// 表示した更新先と lock の版が違うときの注記
    #[test]
    fn test_lock_mismatch_lines_formats_locked_version() {
        let mismatches = vec![LockedVersionMismatch {
            name: "wasm-bindgen".to_string(),
            from: "0.2.127".to_string(),
            to: "0.2.128".to_string(),
            locked: "0.2.129".to_string(),
        }];

        let lines = lock_mismatch_lines(Path::new("./Cargo.toml"), &mismatches);

        assert_eq!(
            lines,
            vec![
                "  ./Cargo.toml — 1 update(s) locked at a different version than shown:",
                "    wasm-bindgen 0.2.127 → 0.2.128 (locked: 0.2.129)",
            ]
        );
        assert!(lock_mismatch_lines(Path::new("./Cargo.toml"), &[]).is_empty());
    }

    #[test]
    fn test_build_install_map_uses_nested_monorepo_dir() {
        let root = PathBuf::from("/repo");
        let app = PathBuf::from("/repo/apps/web");
        let result = result_with_update(app.join("package.json"), Language::Node);
        let dirs = Some(vec![root.clone(), app.clone()]);

        let install_map = build_install_map(&result, &dirs, &root);

        assert_eq!(install_map.len(), 1);
        assert_eq!(install_map[0].0, app);
        assert_eq!(install_map[0].1, vec![Language::Node]);
    }
}
