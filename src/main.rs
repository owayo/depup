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
use depup::cargo_rollback::report::{lock_age_report_lines, lock_mismatch_lines};
use depup::cli::CliArgs;
use depup::config::DepupConfig;
use depup::domain::Language;
use depup::global_config::{GlobalConfig, resolve_max_change, resolve_osv};
use depup::install::{InstallPlan, RustAuditPlan};
use depup::node_lock::audit_node_lock;
use depup::orchestrator::Orchestrator;
use depup::output::{OutputConfig, create_formatter};
use depup::package_manager::SystemPackageManager;
use depup::progress::Progress;
use std::collections::HashSet;
use std::io::{self, Write};
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
        let plan = InstallPlan::new(&orchestrator, &result, &monorepo_dirs, &args.path);
        // 他の言語の install が失敗しても、Rust / Node lock の監査は行う。
        let install_result = run_package_installs(&args, &plan);
        let audit_plan = plan.rust_audit_plan(&orchestrator, &result);
        age_audit_failed = enforce_rust_lock_age(&args, &orchestrator, &audit_plan).await;
        age_audit_failed |= enforce_node_lock_age(&args, &plan).await;

        // 画面に出した更新先と、実際に Cargo.lock へ入った版の食い違いを知らせる。
        // 結果の一覧は install 前に出しているので、ここで別に注記する
        report_rust_lock_mismatches(&audit_plan);
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

/// 計画されたパッケージマネージャの install を実行する。
fn run_package_installs(args: &CliArgs, plan: &InstallPlan) -> anyhow::Result<()> {
    if plan.jobs.is_empty() && plan.wrapper_updates.is_empty() {
        return Ok(());
    }
    let pm_runner = SystemPackageManager::new();
    if args.verbose {
        eprintln!();
        eprintln!("Running package manager install...");
        if plan.jobs.iter().any(|job| {
            job.policy
                .as_ref()
                .is_ok_and(|policy| policy.min_age.is_some())
        }) {
            let mut unsupported: Vec<String> = plan
                .jobs
                .iter()
                .filter_map(|job| {
                    let (_, pm) =
                        pm_runner.resolve_package_manager(job.language, &job.directory)?;
                    (job.language != depup::domain::Language::Node
                        && !job.language.pm_has_native_transitive_age_support(pm))
                    .then(|| pm.to_string())
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
    let mut failed_wrapper_roots = HashSet::new();
    for (directory, version) in &plan.wrapper_updates {
        if args.verbose {
            eprintln!(
                "Updating Gradle Wrapper to {version} ({})...",
                directory.display()
            );
        }
        if let Err(error) = pm_runner.update_gradle_wrapper(directory, version) {
            eprintln!(
                "  Gradle Wrapper update failed ({}): {error}",
                directory.display()
            );
            failed_wrapper_roots.insert(directory);
            any_install_failed = true;
        }
    }
    // install の出力をキャプチャしている間も、実行中の言語をスピナーで示す。
    let mut progress = Progress::new(!args.quiet);
    for job in &plan.jobs {
        if job.language == Language::Java
            && failed_wrapper_roots
                .iter()
                .any(|root| job.directory.starts_with(root) || root.starts_with(&job.directory))
        {
            continue;
        }
        progress.spinner(&format!(
            "Running {} install...",
            job.language.display_name()
        ));
        let policy = job
            .policy
            .as_ref()
            .map_err(|error| anyhow::Error::msg(error.clone()))?;
        let result = pm_runner.run_install_with_age_policy(job.language, &job.directory, policy);
        progress.finish_and_clear();
        if result.command.is_empty() {
            continue;
        }
        if result.success {
            if args.verbose {
                eprintln!(
                    "  {} install completed: {} ({})",
                    result.language.display_name(),
                    result.command,
                    job.directory.display()
                );
            }
        } else {
            eprintln!(
                "  {} install failed: {} ({})",
                result.language.display_name(),
                result.command,
                job.directory.display()
            );
            if !result.stderr.is_empty() {
                eprintln!("    {}", result.stderr);
            }
            any_install_failed = true;
        }
    }
    if any_install_failed {
        anyhow::bail!("Some package manager installs failed");
    }
    Ok(())
}

/// 計画された共有 lock ごとに、既存版も含め crate の age を監査する。
async fn enforce_rust_lock_age(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    plan: &RustAuditPlan,
) -> bool {
    for directory in &plan.missing_lock_dirs {
        eprintln!(
            "  {} — cannot audit --age: Cargo.lock not found",
            directory.display()
        );
    }
    let mut has_unresolved = !plan.missing_lock_dirs.is_empty();
    if plan.locks.is_empty() {
        return has_unresolved;
    }
    if args.verbose {
        eprintln!();
        eprintln!("Enforcing --age on registry crates in Cargo.lock...");
    }
    let mut progress = Progress::new(!args.quiet);
    progress.start(0, "Auditing Cargo.lock against --age");
    let bar = progress.bar();
    for lock in &plan.locks {
        if lock.policy.min_age.is_none() {
            continue;
        }
        if let Some(problem) = &lock.baseline_problem {
            has_unresolved = true;
            progress.suspend(|| {
                eprintln!(
                    "  {} — cannot safely roll back --age violations: pre-install {problem}",
                    lock.directory.display()
                )
            });
            continue;
        }
        let audit = orchestrator
            .enforce_lock_age_rust_with_policy(
                &lock.directory,
                &lock.policy,
                &lock.baseline,
                &lock.preferred,
                bar.as_ref(),
            )
            .await;
        has_unresolved |= audit.has_unresolved();
        let lines = lock_age_report_lines(&lock.directory, &audit, args.verbose);
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

/// Node は lock の実際の解決版を検査し、違反・未確認を必ず報告する。
async fn enforce_node_lock_age(args: &CliArgs, plan: &InstallPlan) -> bool {
    let pm_runner = SystemPackageManager::new();
    let mut failed = false;
    let mut progress = Progress::new(!args.quiet);
    for job in &plan.node_audits {
        let has_lock = [
            "pnpm-lock.yaml",
            "package-lock.json",
            "npm-shrinkwrap.json",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
        ]
        .iter()
        .any(|filename| job.directory.join(filename).exists());
        if !has_lock
            && !plan.jobs.iter().any(|install| {
                install.language == Language::Node && install.directory == job.directory
            })
        {
            continue;
        }
        let policy = match &job.policy {
            Ok(policy) if policy.min_age.is_some() => policy,
            Ok(_) => continue,
            Err(error) => {
                eprintln!(
                    "  {} — cannot audit --age: {error}",
                    job.directory.display()
                );
                failed = true;
                continue;
            }
        };
        let Some((directory, pm)) = pm_runner.resolve_package_manager(job.language, &job.directory)
        else {
            eprintln!(
                "  {} — cannot audit --age: Node package manager could not be determined",
                job.directory.display()
            );
            failed = true;
            continue;
        };
        progress.start(0, "Auditing Node lockfile against --age");
        let bar = progress.bar();
        let audit = audit_node_lock(&directory, pm, policy, bar.as_ref()).await;
        failed |= audit.has_failures();
        progress.suspend(|| {
            for message in audit.failure_messages() {
                eprintln!(
                    "{}",
                    format!("  {} — {message}", directory.display()).yellow()
                );
            }
            if args.verbose && !audit.has_failures() {
                eprintln!(
                    "  {} — {} public npm package version(s) verified against --age",
                    directory.display(),
                    audit.checked
                );
            }
        });
        progress.finish_and_clear();
    }
    failed
}

/// 画面に出した更新先と、監査後の lock に入った版の相違を知らせる。
fn report_rust_lock_mismatches(plan: &RustAuditPlan) {
    for (manifest, mismatches) in plan.mismatch_reports() {
        for line in lock_mismatch_lines(&manifest, &mismatches) {
            eprintln!("{}", line.yellow());
        }
    }
}
