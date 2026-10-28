use super::*;
use crate::install::security::{Notices, check_changes, rust_packages};

#[derive(Default)]
pub(super) struct SecurityResult {
    pub(super) failed: bool,
    pub(super) notices: Vec<(PathBuf, Vec<u8>, Notices)>,
}

pub(super) async fn audit_osv(
    args: &CliArgs,
    plan: &InstallPlan,
    rust: &RustAuditPlan,
    node: &BTreeMap<PathBuf, NodeBaseline>,
    tx: &mut InstallTransactions,
    checks: &impl Checks,
) -> SecurityResult {
    let mut report = SecurityResult::default();
    if !checks.osv_enabled() {
        return report;
    }
    let mut progress = Progress::new(!args.quiet);
    progress.spinner("Verifying final lockfiles against OSV");
    for lock in &rust.locks {
        let path = lock.directory.join("Cargo.lock");
        let checked = async {
            let (content, entries) = crate::cargo_rollback::audit::read_audit_lock(&path)?;
            let before = if lock.baseline_problem.is_some() {
                BTreeSet::new()
            } else {
                rust_packages(&lock.baseline)
            };
            let notices = check_changes(
                &before,
                &rust_packages(&entries),
                &progress,
                |name, before, after| async move {
                    checks
                        .osv_change(Language::Rust, &name, &before, &after)
                        .await
                },
            )
            .await?;
            Ok::<_, String>((content.into_bytes(), notices))
        }
        .await;
        progress.suspend(|| finish_scope(path, checked, tx, &mut report));
    }
    let pm = SystemPackageManager::new();
    for job in &plan.node_audits {
        if !plan
            .jobs
            .iter()
            .any(|install| install.directory == job.directory && install.language == Language::Node)
        {
            continue;
        }
        let pending = tx.pending_report();
        let Some((directory, manager)) = pending
            .node_manager(&job.directory)
            .or_else(|| pm.resolve_package_manager(Language::Node, &job.directory))
        else {
            continue;
        };
        let Some(path) = crate::node_lock::audited_lockfile(&directory, manager) else {
            continue;
        };
        let checked = async {
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            let NodeBaseline::Known(after) =
                checks.node_osv_snapshot(&directory, manager, false).await
            else {
                return Err("final Node lock could not be verified for OSV".into());
            };
            let before = match node.get(&job.directory) {
                Some(NodeBaseline::Known(packages)) => packages.clone(),
                _ => BTreeSet::new(),
            };
            let notices = check_changes(
                &before,
                &after,
                &progress,
                |name, before, after| async move {
                    checks
                        .osv_change(Language::Node, &name, &before, &after)
                        .await
                },
            )
            .await?;
            Ok((bytes, notices))
        }
        .await;
        progress.suspend(|| finish_scope(path, checked, tx, &mut report));
    }
    progress.finish_and_clear();
    report
}

fn finish_scope(
    path: PathBuf,
    checked: Result<(Vec<u8>, Notices), String>,
    tx: &mut InstallTransactions,
    result: &mut SecurityResult,
) {
    let checked = checked.and_then(|(bytes, notices)| {
        if tx.manages(&path) {
            tx.check_lock(&path, Some(&bytes))?;
        }
        if std::fs::read(&path).map_err(|error| error.to_string())? != bytes {
            return Err("lockfile changed during OSV verification".into());
        }
        Ok((bytes, notices))
    });
    match checked {
        Ok((bytes, notices)) => result.notices.push((path, bytes, notices)),
        Err(error) => {
            result.failed = true;
            let reason = format!("final lock OSV verification failed: {error}");
            eprintln!("  {} — {reason}", path.display());
            tx.fail_holder(&path, reason);
        }
    }
}
