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
use depup::install::transaction::{InstallTransactions, RollbackReport};
use depup::install::{InstallPlan, RustAuditPlan};
use depup::node_lock::audit_node_lock;
use depup::orchestrator::Orchestrator;
use depup::output::{OutputConfig, create_formatter};
use depup::package_manager::SystemPackageManager;
use depup::progress::Progress;
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
    args.validate_refresh_lock().map_err(anyhow::Error::msg)?;

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

    let (mut result, monorepo_dirs) = if let Some(config) = monorepo_config {
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
    let defer_output = args.install && !args.dry_run;
    if !defer_output {
        formatter.format(&result, &mut stdout)?;
        stdout.flush()?;
    }

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
        let plan = InstallPlan::new_scoped(
            &orchestrator,
            &result,
            &monorepo_dirs,
            &args.path,
            !args.only.is_empty(),
        );
        // 他の言語の install が失敗しても、Rust / Node lock の監査は行う。
        age_audit_failed = finish_install(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            formatter.as_ref(),
            &mut stdout,
            defer_output,
        )
        .await?;
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

/// Recovery and verification finish before the final output is flushed.
async fn finish_install(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    plan: &InstallPlan,
    result: &mut depup::orchestrator::OrchestratorResult,
    formatter: &dyn depup::output::OutputFormatter,
    stdout: &mut impl Write,
    defer_output: bool,
) -> anyhow::Result<bool> {
    if args.refresh_lock {
        let outcome = depup::install::refresh::run(args, orchestrator, plan, result).await;
        if defer_output {
            formatter.format(result, stdout)?;
            stdout.flush()?;
        }
        if outcome.install_failed {
            anyhow::bail!("Some package manager installs failed");
        }
        return Ok(outcome.audit_failed);
    }
    let recovery = run_package_installs(args, plan, result, &orchestrator.rust_lock_boundary());
    recovery.print(&mut io::stderr())?;
    recovery.mark_summary(&mut result.summary);
    let mut audit_plan = plan.rust_audit_plan(orchestrator, result);
    recovery.exclude_rust_audits(&mut audit_plan);
    let mut age_audit_failed = enforce_rust_lock_age(args, orchestrator, &audit_plan).await;
    age_audit_failed |= enforce_node_lock_age(args, plan, &recovery).await;
    let pm = SystemPackageManager::new();
    let mut verification_progress = Progress::new(!args.quiet);
    for job in plan
        .jobs
        .iter()
        .filter(|job| job.language == Language::Node && !recovery.covers_directory(&job.directory))
    {
        let manager = recovery
            .node_manager(&job.directory)
            .or_else(|| pm.resolve_package_manager(job.language, &job.directory));
        if let Some((directory, "pnpm")) = manager {
            verification_progress.spinner("Verifying pnpm update targets in the final lockfile");
            for message in
                depup::node_lock::verify_pnpm_update_targets(&directory, &mut result.summary).await
            {
                verification_progress
                    .suspend(|| eprintln!("  {} — {message}", directory.display()));
                result.errors.push(
                    depup::orchestrator::OrchestratorError::InstallResolutionError {
                        directory: directory.clone(),
                        message,
                    },
                );
                age_audit_failed = true;
            }
        }
    }
    verification_progress.finish_and_clear();
    report_rust_lock_mismatches(&audit_plan);
    if defer_output {
        formatter.format(result, stdout)?;
        stdout.flush()?;
    }
    if recovery.has_failures() {
        anyhow::bail!("Some package manager installs failed");
    }
    Ok(age_audit_failed)
}

/// 計画されたパッケージマネージャの install を実行する。
fn run_package_installs(
    args: &CliArgs,
    plan: &InstallPlan,
    result: &depup::orchestrator::OrchestratorResult,
    boundary: &std::path::Path,
) -> RollbackReport {
    let pm_runner = SystemPackageManager::new();
    let mut transactions = InstallTransactions::begin(plan, result, boundary);
    if args.verbose
        && plan.jobs.iter().any(|job| {
            job.policy
                .as_ref()
                .is_ok_and(|policy| policy.min_age.is_some())
        })
    {
        let mut unsupported: Vec<_> = transactions
            .commands()
            .filter(|command| {
                command.language != Language::Node
                    && !command
                        .language
                        .pm_has_native_transitive_age_support(command.package_manager)
            })
            .map(|command| command.package_manager)
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
    let mut progress = Progress::new(!args.quiet);
    transactions.execute(|command| {
        progress.spinner(&format!(
            "Running {} install...",
            command.language.display_name()
        ));
        let result = pm_runner.run_prepared_install(command);
        progress.finish_and_clear();
        if !result.success || args.verbose {
            eprintln!(
                "  {} install {}: {} ({})",
                result.language.display_name(),
                if result.success {
                    "completed"
                } else {
                    "failed"
                },
                result.command,
                command.directory.display()
            );
            if !result.success && !result.stderr.is_empty() {
                eprintln!("    {}", result.stderr);
            }
        }
        result
    });
    transactions.rollback()
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
async fn enforce_node_lock_age(
    args: &CliArgs,
    plan: &InstallPlan,
    recovery: &RollbackReport,
) -> bool {
    let pm_runner = SystemPackageManager::new();
    let mut failed = false;
    let mut progress = Progress::new(!args.quiet);
    for job in &plan.node_audits {
        if recovery.covers_directory(&job.directory) {
            continue;
        }
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
        let Some((directory, pm)) = recovery
            .node_manager(&job.directory)
            .or_else(|| pm_runner.resolve_package_manager(job.language, &job.directory))
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

#[cfg(all(test, unix))]
mod install_tests {
    use super::*;
    use depup::domain::{
        Dependency, ManifestUpdateResult, UpdateResult, UpdateSummary, VersionSpec, VersionSpecKind,
    };
    use depup::manifest::{GradleParser, ManifestWriter};
    use depup::orchestrator::OrchestratorResult;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn failed_install_restores_the_written_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("build.gradle");
        let before = "dependencies { implementation 'org.example:library:1.0.0' }\n";
        std::fs::write(&path, before).unwrap();
        let script = temp.path().join("gradlew");
        std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut manifest = ManifestUpdateResult::new(&path, Language::Java);
        manifest.add_result(UpdateResult::update(
            Dependency::new(
                "org.example:library",
                VersionSpec::new(VersionSpecKind::Exact, "1.0.0", "1.0.0"),
                false,
                Language::Java,
            ),
            "2.0.0",
        ));
        let write = ManifestWriter::new(false)
            .apply_updates(&manifest, &GradleParser)
            .unwrap();
        assert!(write.file_modified);
        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(manifest);
        let result = OrchestratorResult {
            summary,
            write_results: vec![write],
            errors: vec![],
        };
        let mut args = CliArgs::parse_from(["depup", "--install", "--no-age", "--quiet"]);
        args.path = temp.path().into();
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
        let recovery = run_package_installs(&args, &plan, &result, temp.path());
        assert!(recovery.has_failures());
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
    }
}

#[cfg(test)]
mod final_output_tests {
    use super::*;
    use depup::domain::{
        Dependency, ManifestUpdateResult, UpdateResult, UpdateSummary, VersionSpec, VersionSpecKind,
    };
    use depup::manifest::{ManifestWriter, PackageJsonParser};
    use depup::orchestrator::OrchestratorResult;
    #[derive(Default)]
    struct Output {
        bytes: Vec<u8>,
        flushes: usize,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }
    #[tokio::test]
    async fn failed_scope_recovers_excludes_missing_lock_audit_and_flushes_one_final_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("package.json");
        let before = "{\"dependencies\":{\"example\":\"1.0.0\"}}";
        std::fs::write(&path, before).unwrap();
        let mut manifest = ManifestUpdateResult::new(&path, Language::Node);
        manifest.add_result(UpdateResult::update(
            Dependency::new(
                "example",
                VersionSpec::new(VersionSpecKind::Exact, "1.0.0", "1.0.0"),
                false,
                Language::Node,
            ),
            "2.0.0",
        ));
        let write = ManifestWriter::new(false)
            .apply_updates(&manifest, &PackageJsonParser)
            .unwrap();
        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(manifest);
        let mut result = OrchestratorResult {
            summary,
            write_results: vec![write],
            errors: vec![],
        };
        let mut args =
            CliArgs::parse_from(["depup", "--install", "--json", "--age", "2w", "--quiet"]);
        args.path = temp.path().into();
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let mut plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
        plan.jobs[0].scope = Err("simulated scope preparation failure".into());
        let formatter = create_formatter(OutputConfig::from_cli(true, false, false, true, false));
        let mut output = Output::default();
        let error = finish_install(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            formatter.as_ref(),
            &mut output,
            true,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("installs failed"));
        assert_eq!(output.flushes, 1);
        let values = serde_json::Deserializer::from_slice(&output.bytes)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["summary"]["updates"], 0);
        assert_eq!(values[0]["summary"]["failed"], 1);
        assert!(
            values[0]["manifests"][0]["failed"][0]["error"]
                .as_str()
                .unwrap()
                .contains("canceled")
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
    }
    #[tokio::test]
    async fn age_only_failure_keeps_the_resolved_files_and_does_not_become_install_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("package.json");
        std::fs::write(&path, "{}").unwrap();
        let lock = temp.path().join("bun.lock");
        std::fs::write(&lock, "existing unsupported lock").unwrap();
        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(ManifestUpdateResult::new(path, Language::Node));
        let mut result = OrchestratorResult {
            summary,
            write_results: vec![],
            errors: vec![],
        };
        let mut args =
            CliArgs::parse_from(["depup", "--install", "--json", "--age", "2w", "--quiet"]);
        args.path = temp.path().into();
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
        assert!(plan.jobs.is_empty());
        let formatter = create_formatter(OutputConfig::from_cli(true, false, false, true, false));
        let mut output = Output::default();
        assert!(
            finish_install(
                &args,
                &orchestrator,
                &plan,
                &mut result,
                formatter.as_ref(),
                &mut output,
                true
            )
            .await
            .unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(lock).unwrap(),
            "existing unsupported lock"
        );
        assert_eq!(output.flushes, 1);
    }
}
