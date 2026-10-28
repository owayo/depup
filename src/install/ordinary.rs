//! 既定の `--install`。パッケージマネージャーは、`--age` 監査の完了まで開いたままの
//! 復元用トランザクション内で動く。この実行で追加した版が lockfile に残って
//! `--age` を満たさない場合、または lockfile を検証できなかった場合は、install 単位を
//! 実行前の状態へ戻す。実行前から lock にあった版の違反は報告するが、
//! 復元の理由にはしない。
use super::age::{named, print_rust_audit, rust_restore_reason};
use super::transaction::InstallTransactions;
use super::*;
use crate::cargo_rollback::audit::{AuditReceipt, LockAgeAuditResult};
use crate::cargo_rollback::report::lock_mismatch_lines;
use crate::cli::CliArgs;
use crate::domain::UpdateSummary;
use crate::node_lock::{NodeBaseline, NodeLockAuditResult, UnverifiedKind};
use crate::orchestrator::osv_report::RollbackOsvChecks;
use crate::package_manager::{InstallResult, PreparedInstall, SystemPackageManager};
use crate::progress::Progress;
use async_trait::async_trait;
use colored::Colorize;
use indicatif::ProgressBar;

/// 既定の install の監査。テストではレジストリと Cargo へのアクセスを差し替える。
#[async_trait(?Send)]
pub(crate) trait Checks {
    async fn rust_lock(
        &self,
        lock: &RustLockPlan,
        bar: Option<&ProgressBar>,
    ) -> (LockAgeAuditResult, Option<AuditReceipt>);
    async fn node_baseline(&self, directory: &Path, manager: &str) -> NodeBaseline;
    async fn node_lock(
        &self,
        directory: &Path,
        manager: &str,
        policy: &AgePolicy,
        bar: Option<&ProgressBar>,
    ) -> NodeLockAuditResult;
    async fn pnpm_targets(&self, directory: &Path, summary: &mut UpdateSummary) -> Vec<String>;
}

struct SystemChecks<'a>(&'a Orchestrator);

#[async_trait(?Send)]
impl Checks for SystemChecks<'_> {
    async fn rust_lock(
        &self,
        lock: &RustLockPlan,
        bar: Option<&ProgressBar>,
    ) -> (LockAgeAuditResult, Option<AuditReceipt>) {
        self.0
            .enforce_lock_age_rust_tracked(
                &lock.directory,
                &lock.policy,
                &lock.baseline,
                &lock.preferred,
                bar,
            )
            .await
    }
    async fn node_baseline(&self, directory: &Path, manager: &str) -> NodeBaseline {
        crate::node_lock::read_baseline(directory, manager).await
    }
    async fn node_lock(
        &self,
        directory: &Path,
        manager: &str,
        policy: &AgePolicy,
        bar: Option<&ProgressBar>,
    ) -> NodeLockAuditResult {
        crate::node_lock::audit_node_lock(directory, manager, policy, bar).await
    }
    async fn pnpm_targets(&self, directory: &Path, summary: &mut UpdateSummary) -> Vec<String> {
        crate::node_lock::verify_pnpm_update_targets(directory, summary).await
    }
}

/// 予定した install を実行して lockfile を監査し、検証に失敗した単位を復元する。
pub async fn run(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    plan: &InstallPlan,
    result: &mut OrchestratorResult,
) -> InstallOutcome {
    let pm = SystemPackageManager::new();
    run_with(
        args,
        orchestrator,
        plan,
        result,
        &SystemChecks(orchestrator),
        |command| pm.run_prepared_install(command),
    )
    .await
}

pub(crate) async fn run_with(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    plan: &InstallPlan,
    result: &mut OrchestratorResult,
    checks: &impl Checks,
    mut runner: impl FnMut(&PreparedInstall) -> InstallResult,
) -> InstallOutcome {
    let mut tx = InstallTransactions::begin(plan, result, &orchestrator.rust_lock_boundary());
    note_direct_only_age(args, plan, &tx);
    // パッケージマネージャーが lockfile を書き換える前に読む。
    let baselines = node_baselines(plan, &tx, checks).await;
    let mut progress = Progress::new(!args.quiet);
    tx.execute(|command| {
        progress.spinner(&format!(
            "Running {} install...",
            command.language.display_name()
        ));
        let outcome = runner(command);
        progress.finish_and_clear();
        if !outcome.success || args.verbose {
            eprintln!(
                "  {} install {}: {} ({})",
                outcome.language.display_name(),
                if outcome.success {
                    "completed"
                } else {
                    "failed"
                },
                outcome.command,
                command.directory.display()
            );
            if !outcome.success && !outcome.stderr.is_empty() {
                eprintln!("    {}", outcome.stderr);
            }
        }
        outcome
    });
    let mut audit_plan = plan.rust_audit_plan(orchestrator, result);
    tx.pending_report().exclude_rust_audits(&mut audit_plan);
    let mut rollback_osv = RollbackOsvChecks::default();
    let mut audit_failed =
        audit_rust_locks(args, &audit_plan, &mut tx, checks, &mut rollback_osv).await;
    audit_failed |= audit_node_locks(args, plan, &baselines, &mut tx, checks).await;
    let recovery = tx.rollback();
    let report_error = recovery.print(&mut std::io::stderr()).err();
    recovery.mark_summary(&mut result.summary);
    rollback_osv.warn(orchestrator, result, args.quiet).await;
    // pnpm の更新先の不一致は調査用にファイルを残す（age 違反ではない）。
    let mut verification = Progress::new(!args.quiet);
    for job in plan
        .jobs
        .iter()
        .filter(|job| job.language == Language::Node && !recovery.covers_directory(&job.directory))
    {
        let manager = recovery.node_manager(&job.directory).or_else(|| {
            SystemPackageManager::new().resolve_package_manager(job.language, &job.directory)
        });
        if let Some((directory, "pnpm")) = manager {
            verification.spinner("Verifying pnpm update targets in the final lockfile");
            for message in checks.pnpm_targets(&directory, &mut result.summary).await {
                verification.suspend(|| eprintln!("  {} — {message}", directory.display()));
                result.errors.push(
                    crate::orchestrator::OrchestratorError::InstallResolutionError {
                        directory: directory.clone(),
                        message,
                    },
                );
                audit_failed = true;
            }
        }
    }
    verification.finish_and_clear();
    // 復元した単位の lockfile は、表示した更新内容を表さなくなる。
    recovery.exclude_rust_audits(&mut audit_plan);
    for (manifest, mismatches) in audit_plan.mismatch_reports() {
        for line in lock_mismatch_lines(&manifest, &mismatches) {
            eprintln!("{}", line.yellow());
        }
    }
    InstallOutcome {
        install_failed: recovery.has_install_failures(),
        audit_failed: audit_failed || recovery.has_failures(),
        report_error,
    }
}

fn note_direct_only_age(args: &CliArgs, plan: &InstallPlan, tx: &InstallTransactions) {
    if !args.verbose
        || !plan.jobs.iter().any(|job| {
            job.policy
                .as_ref()
                .is_ok_and(|policy| policy.min_age.is_some())
        })
    {
        return;
    }
    let mut unsupported: Vec<_> = tx
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

/// lockfile を監査する Node install について、実行前から lock された公開 npm の版。
async fn node_baselines(
    plan: &InstallPlan,
    tx: &InstallTransactions,
    checks: &impl Checks,
) -> BTreeMap<PathBuf, NodeBaseline> {
    let pending = tx.pending_report();
    let pm = SystemPackageManager::new();
    let mut baselines = BTreeMap::new();
    for job in &plan.node_audits {
        let installs = plan.jobs.iter().any(|install| {
            install.language == Language::Node && install.directory == job.directory
        });
        if !installs
            || pending.covers_directory(&job.directory)
            || !job
                .policy
                .as_ref()
                .is_ok_and(|policy| policy.min_age.is_some())
        {
            continue;
        }
        let Some((directory, manager)) = pending
            .node_manager(&job.directory)
            .or_else(|| pm.resolve_package_manager(job.language, &job.directory))
        else {
            continue;
        };
        if matches!(manager, "pnpm" | "npm") {
            baselines.insert(
                job.directory.clone(),
                checks.node_baseline(&directory, manager).await,
            );
        }
    }
    baselines
}

/// 共有 Cargo.lock ごとに監査し、監査自身の差し戻しを採用して、復元する単位を失敗にする。
async fn audit_rust_locks(
    args: &CliArgs,
    plan: &RustAuditPlan,
    tx: &mut InstallTransactions,
    checks: &impl Checks,
    rollback_osv: &mut RollbackOsvChecks,
) -> bool {
    for directory in &plan.missing_lock_dirs {
        eprintln!(
            "  {} — cannot audit --age: Cargo.lock not found",
            directory.display()
        );
        // このマニフェストを install した単位を選び、同じディレクトリの別の単位は選ばない。
        tx.fail_holder(
            &directory.join("Cargo.toml"),
            "Cargo.lock was not found after the install, so --age could not be checked".into(),
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
            tx.fail_holder(
                &lock.directory.join("Cargo.lock"),
                format!(
                    "the Cargo.lock before this run could not be read ({problem}), so the versions this run added could not be checked against --age"
                ),
            );
            continue;
        }
        let (audit, receipt) = checks.rust_lock(lock, bar.as_ref()).await;
        rollback_osv.record(&lock.directory, &audit);
        has_unresolved |= audit.has_unresolved();
        print_rust_audit(&lock.directory, &audit, args.verbose, &progress);
        let mut reason = rust_restore_reason(&audit, &lock.baseline);
        let path = lock.directory.join("Cargo.lock");
        // 監査が保証したバイト列だけを採用する。どの単位にも含まれない lockfile は監査だけの対象。
        if let Some(receipt) = receipt
            && tx.manages(&path)
            && let Err(error) = tx.accept_audit_lock(&path, &receipt.before, &receipt.after)
        {
            has_unresolved = true;
            reason.get_or_insert(format!(
                "Cargo.lock changed outside the --age audit ({error})"
            ));
        }
        if let Some(reason) = reason {
            tx.fail_holder(&path, reason);
        }
    }
    progress.finish_and_clear();
    has_unresolved
}

/// Node の lockfile を読み取り専用で監査し、復元する単位を失敗にする。
async fn audit_node_locks(
    args: &CliArgs,
    plan: &InstallPlan,
    baselines: &BTreeMap<PathBuf, NodeBaseline>,
    tx: &mut InstallTransactions,
    checks: &impl Checks,
) -> bool {
    let pm_runner = SystemPackageManager::new();
    let mut failed = false;
    let mut progress = Progress::new(!args.quiet);
    for job in &plan.node_audits {
        // install 自体や Tauri の Cargo.lock ですでに失敗した単位も、どのみち復元される。
        let pending = tx.pending_report();
        if pending.covers_directory(&job.directory) {
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
        let Some((directory, manager)) = pending
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
        let audit = checks
            .node_lock(&directory, manager, policy, bar.as_ref())
            .await;
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
        if let Some(baseline) = baselines.get(&job.directory)
            && let Some(reason) = node_restore_reason(manager, &audit, baseline)
            && let Some(lockfile) = crate::node_lock::audited_lockfile(&directory, manager)
        {
            tx.fail_holder(&lockfile, reason);
        }
    }
    failed
}

/// Node の単位を実行前へ戻す理由。公開 npm レジストリ以外の取得元は監査対象外であり、
/// 未対応のパッケージマネージャーには baseline がない。
fn node_restore_reason(
    manager: &str,
    audit: &NodeLockAuditResult,
    baseline: &NodeBaseline,
) -> Option<String> {
    if !audit.errors.is_empty() {
        return Some(format!(
            "the {manager} lockfile could not be checked against --age: {}",
            audit.errors.join("; ")
        ));
    }
    let added = |name: &str, version: &str| match baseline {
        NodeBaseline::Known(packages) => {
            !packages.contains(&(name.to_string(), version.to_string()))
        }
        NodeBaseline::Unknown(_) => true,
    };
    let failing: Vec<String> = audit
        .violations
        .iter()
        .filter(|violation| added(&violation.name, &violation.version))
        .map(|violation| {
            format!(
                "{} {} (newer than --age)",
                violation.name, violation.version
            )
        })
        .chain(
            audit
                .unverified
                .iter()
                .filter(|item| {
                    item.kind == UnverifiedKind::Unconfirmed && added(&item.name, &item.version)
                })
                .map(|item| format!("{} {} ({})", item.name, item.version, item.reason)),
        )
        .collect();
    if failing.is_empty() {
        return None;
    }
    Some(match baseline {
        NodeBaseline::Known(_) => format!(
            "the {manager} lockfile keeps versions this run added that do not pass --age: {}",
            named(&failing)
        ),
        NodeBaseline::Unknown(reason) => format!(
            "the {manager} lockfile has versions that do not pass --age, and the lockfile before this run could not be read ({reason}), so they cannot be told apart from existing ones: {}",
            named(&failing)
        ),
    })
}

#[cfg(test)]
mod tests;
