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
use depup::cli::CliArgs;
use depup::config::DepupConfig;
use depup::domain::Language;
use depup::global_config::{GlobalConfig, resolve_max_change, resolve_osv};
use depup::manifest::RegistryLockEntries;
use depup::orchestrator::{LOCK_AGE_AUDIT_BUDGET, Orchestrator, OrchestratorResult};
use depup::output::{OutputConfig, create_formatter};
use depup::package_manager::{SystemPackageManager, run_installs};
use depup::progress::Progress;
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

    // dry-run でない場合、要求があればパッケージマネージャの install を実行
    if args.install && !args.dry_run {
        // install フェーズも judge と同じ解決済み age を使う。
        // これにより direct deps と install 後の transitive 依存で age ポリシーが揃う
        // (CLI --age 未指定でもプロジェクト minimumReleaseAge / グローバル設定 / デフォルト 1w が反映される)。
        let install_min_age = orchestrator.resolved_min_age();

        // install 前の Cargo.lock を控えておく。post-install の age 監査は
        // 「install で新しく入った / 版が変わった」依存だけを対象にする
        // (crates.io は 1 リクエスト/秒 のため lock 全体を舐めると数分かかる)。
        let lock_baselines = if install_min_age.is_some() {
            collect_rust_lock_baselines(&args, &result)
        } else {
            HashMap::new()
        };

        run_package_installs(&args, &result, &monorepo_dirs, install_min_age)?;

        // Rust の transitive 依存も age 制約を満たすよう Cargo.lock を整える。
        if let Some(age) = install_min_age {
            enforce_rust_lock_age(&args, &orchestrator, &result, &lock_baselines, age).await;
        }
    }

    // 適切な終了コードを返す。
    // OSV 警告は「脆弱な候補を検出して安全な版へフォールバックした」という
    // 設計どおりの正常動作の通知なので、エラー扱い (exit code 2) にしない。
    let has_errors = result
        .errors
        .iter()
        .any(|e| !matches!(e, depup::orchestrator::OrchestratorError::OsvWarning { .. }));

    if has_errors {
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
    min_age: Option<std::time::Duration>,
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
        if min_age.is_some() {
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
            let install_results =
                run_installs(&pm_runner, std::slice::from_ref(language), dir, min_age);
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

/// 更新対象の Rust プロジェクトについて、install 前の Cargo.lock の内容を控える。
///
/// post-install の age 監査は「install によって新しく入った / 版が変わった」依存だけを
/// 対象にする。その差分を取るための基準値。install 前に Cargo.lock がまだ無い
/// ディレクトリは記録しない (install で生成された lock は全エントリが新規となり、
/// 監査側で空のベースラインとして扱われる)。
fn collect_rust_lock_baselines(
    args: &CliArgs,
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
        let Some(lock_path) = depup::manifest::find_cargo_lock_upward(parent, &args.path) else {
            continue;
        };
        let lock_dir = lock_path.parent().unwrap_or(parent).to_path_buf();
        baselines
            .entry(lock_dir)
            .or_insert_with(|| depup::manifest::read_registry_entries(&lock_path));
    }
    baselines
}

/// Rust プロジェクト (Cargo.toml を含む) ディレクトリに対し、
/// `--age` を transitive 依存にも適用する。install 済み Cargo.lock を走査し、
/// age 違反の依存を `cargo update -p --precise` で古いバージョンへ差し戻す。
async fn enforce_rust_lock_age(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
    baselines: &HashMap<PathBuf, RegistryLockEntries>,
    age: std::time::Duration,
) {
    use depup::orchestrator::LockAgeStatus;

    // 対象となる Rust プロジェクトディレクトリを収集。
    // workspace メンバーや Tauri (src-tauri) の Cargo.lock はマニフェストと別の
    // 階層にあることがあるため、マニフェストのディレクトリから上方向に lock を
    // 探し、lock が実在するディレクトリを監査対象にする。
    let mut rust_dirs: Vec<PathBuf> = Vec::new();
    for manifest in &result.summary.manifests {
        // 更新がなかった Rust manifest は cargo update も走らないため audit 不要
        if manifest.language != Language::Rust || !manifest.has_updates() {
            continue;
        }
        let Some(parent) = manifest.path.parent() else {
            continue;
        };
        let Some(lock_path) = depup::manifest::find_cargo_lock_upward(parent, &args.path) else {
            if args.verbose {
                eprintln!(
                    "  {} — Cargo.lock not found; skipping transitive age audit",
                    parent.display()
                );
            }
            continue;
        };
        let lock_dir = lock_path.parent().unwrap_or(parent).to_path_buf();
        if !rust_dirs.contains(&lock_dir) {
            rust_dirs.push(lock_dir);
        }
    }

    if rust_dirs.is_empty() {
        return;
    }

    if args.verbose {
        eprintln!();
        eprintln!("Enforcing --age on transitive Rust dependencies...");
    }

    // 監査は crates.io の 1 リクエスト/秒 制限に律速される。何件目を照会中かを
    // 出さないと、対象が多いときに無言のフリーズと区別がつかない。
    let mut progress = Progress::new(!args.quiet);
    progress.start(0, "Auditing transitive Rust dependencies");
    let bar = progress.bar();

    for dir in &rust_dirs {
        let baseline = baselines.get(dir).cloned().unwrap_or_default();
        let audit = orchestrator
            .enforce_lock_age_rust(dir, age, &baseline, bar.as_ref())
            .await;
        let adjustments = audit.adjustments;

        if audit.unchecked > 0 {
            progress.suspend(|| {
                eprintln!(
                    "  {} — transitive age audit stopped after {}s; {} crate(s) left unchecked",
                    dir.display(),
                    LOCK_AGE_AUDIT_BUDGET.as_secs(),
                    audit.unchecked
                );
            });
        }

        if adjustments.is_empty() {
            // 予算切れで未検証が残っている場合は「全て age 内」とは言い切れない
            // (直前に未検証件数を警告済み)
            if args.verbose && audit.unchecked == 0 {
                progress.suspend(|| {
                    eprintln!("  {} — all transitive deps within --age", dir.display());
                });
            }
            continue;
        }

        let downgraded: Vec<_> = adjustments
            .iter()
            .filter(|a| matches!(a.status, LockAgeStatus::Downgraded))
            .collect();
        let failures: Vec<_> = adjustments
            .iter()
            .filter(|a| !matches!(a.status, LockAgeStatus::Downgraded))
            .collect();

        if !downgraded.is_empty() {
            progress.suspend(|| {
                eprintln!(
                    "  {} — {} transitive dep(s) rolled back to satisfy --age:",
                    dir.display(),
                    downgraded.len()
                );
                for adj in &downgraded {
                    eprintln!(
                        "    {} {} → {}",
                        adj.name,
                        adj.from,
                        adj.to.as_deref().unwrap_or("?")
                    );
                }
            });
        }

        if !failures.is_empty() && args.verbose {
            progress.suspend(|| {
                eprintln!(
                    "  {} — {} transitive dep(s) could not be rolled back:",
                    dir.display(),
                    failures.len()
                );
                for adj in &failures {
                    let detail = match &adj.status {
                        LockAgeStatus::NoOlderCandidate => "no older candidate".to_string(),
                        LockAgeStatus::ReleaseDateUnavailable => {
                            "release date unavailable".to_string()
                        }
                        LockAgeStatus::UpdateCommandFailed(msg) => {
                            format!("cargo update failed: {msg}")
                        }
                        LockAgeStatus::Downgraded => unreachable!(),
                    };
                    eprintln!("    {} ({}): {}", adj.name, adj.from, detail);
                }
            });
        }
    }

    progress.finish_and_clear();
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
