//! Opt-in lock refresh. Resolution is provisional until all final-lock checks pass.
use super::transaction::InstallTransactions;
use super::*;
use crate::cargo_rollback::audit::{AuditReceipt, LockAgeAuditResult};
use crate::cli::CliArgs;
use crate::domain::LockRefreshResult;
use crate::file_snapshot::{FileGuard, FileState};
use crate::manifest::{is_crates_io_source, parse_lock_packages};
use crate::package_manager::{InstallResult, PreparedInstall, SystemPackageManager};
use crate::progress::Progress;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use std::time::Duration;

const VERIFICATION_BUDGET: Duration = Duration::from_secs(180);

pub struct RefreshOutcome {
    pub install_failed: bool,
    pub audit_failed: bool,
}

pub(super) fn plan_jobs(
    jobs: &mut Vec<InstallJob>,
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
    node: &[InstallJob],
) {
    jobs.retain(|job| !matches!(job.language, Language::Node | Language::Rust));
    jobs.extend(node.iter().map(|job| {
        InstallJob {
            directory: job.directory.clone(),
            language: Language::Node,
            policy: job.policy.clone(),
            manifests: job.manifests.clone(),
            scope: blocked_reason(result, &job.directory, Language::Node)
                .map_or(Ok(InstallScope::Refresh), Err),
        }
    }));
    let mut roots: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let boundary = orchestrator.rust_lock_boundary();
    let stopped = stopped_cargo_roots(result);
    let stopped_paths = stopped_cargo_paths(result);
    for manifest in result.summary.by_language(Language::Rust) {
        if stopped_paths.contains(
            &crate::file_snapshot::resolved_path(&manifest.path)
                .unwrap_or_else(|_| manifest.path.clone()),
        ) {
            continue;
        }
        match crate::cargo_scope::rust_resolution_directory(&manifest.path, &boundary) {
            Ok(root) if !stopped.contains(&root) => {
                roots.entry(root).or_default().push(manifest.path.clone())
            }
            Ok(_) => {}
            Err(error) => jobs.push(InstallJob {
                directory: manifest.path.parent().unwrap_or(Path::new(".")).into(),
                language: Language::Rust,
                policy: Ok(orchestrator.resolved_age_policy_for(&boundary)),
                manifests: vec![manifest.path.clone()],
                scope: Err(error),
            }),
        }
    }
    let shared = RustLockGroups::collect(orchestrator, result).locks;
    for (directory, manifests) in roots {
        jobs.push(InstallJob {
            policy: shared
                .iter()
                .find(|(root, _)| {
                    crate::file_snapshot::resolved_path(root).is_ok_and(|root| root == directory)
                })
                .map_or_else(
                    || install_age_policy(orchestrator, result, &directory, Language::Rust),
                    |(_, group)| Ok(group.policy.clone()),
                ),
            scope: blocked_reason(result, &directory, Language::Rust)
                .map_or(Ok(InstallScope::Refresh), Err),
            directory,
            language: Language::Rust,
            manifests,
        });
    }
    jobs.sort_by(|a, b| {
        a.directory
            .cmp(&b.directory)
            .then_with(|| a.language.to_string().cmp(&b.language.to_string()))
    });
}

fn blocked_reason(
    result: &OrchestratorResult,
    directory: &Path,
    language: Language,
) -> Option<String> {
    if result
        .summary
        .by_language(language)
        .any(|manifest| in_scope(&manifest.path, directory) && manifest.failed_count() > 0)
    {
        return Some("lock refresh was stopped because a manifest update failed".into());
    }
    result.errors.iter().find_map(|error| match error {
        crate::orchestrator::OrchestratorError::ManifestParseError { path, .. }
        | crate::orchestrator::OrchestratorError::WriteError { path, .. }
            if in_scope(Path::new(path), directory) =>
        {
            Some(
                "lock refresh was stopped because a manifest could not be parsed or written".into(),
            )
        }
        _ => None,
    })
}

fn in_scope(path: &Path, directory: &Path) -> bool {
    // Unreadable identities cannot establish that a failed declaration is independent.
    crate::file_snapshot::resolved_path(path)
        .and_then(|path| {
            crate::file_snapshot::resolved_path(directory)
                .map(|directory| path.starts_with(directory))
        })
        .unwrap_or(true)
}

#[derive(Default)]
struct Snapshot {
    bytes: Option<Vec<u8>>,
    packages: BTreeSet<(String, String)>,
    protected: BTreeSet<String>,
    node_direct: BTreeMap<PathBuf, BTreeMap<String, String>>,
    node_direct_edges: crate::node_lock::DirectEdges,
}

fn lock_path(job: &InstallJob) -> PathBuf {
    job.directory.join(if job.language == Language::Rust {
        "Cargo.lock"
    } else {
        "pnpm-lock.yaml"
    })
}

#[async_trait(?Send)]
trait Verification: Sync {
    async fn snapshot(&self, job: &InstallJob) -> Result<Snapshot, String>;
    async fn cargo_age(
        &self,
        job: &InstallJob,
        baseline: &RegistryLockEntries,
        preferred: &PreferredVersions,
        progress: &Progress,
    ) -> (LockAgeAuditResult, Option<AuditReceipt>);
    async fn node_age(&self, job: &InstallJob, progress: &Progress) -> Result<(), String>;
    async fn osv(&self, language: Language, name: &str, version: &str) -> Result<(), String>;
}

struct SystemVerification<'a>(&'a Orchestrator);

#[async_trait(?Send)]
impl Verification for SystemVerification<'_> {
    async fn snapshot(&self, job: &InstallJob) -> Result<Snapshot, String> {
        match std::fs::symlink_metadata(lock_path(job)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Snapshot::default());
            }
            Err(error) => return Err(error.to_string()),
            Ok(_) => {}
        }
        if job.language == Language::Node {
            let (bytes, node) = crate::node_lock::refresh_snapshot(&job.directory).await?;
            return Ok(Snapshot {
                bytes: Some(bytes),
                packages: node.packages,
                protected: node.protected,
                node_direct: node.direct,
                node_direct_edges: node.direct_edges,
            });
        }
        let (content, entries) = read_audit_lock(&lock_path(job))?;
        let protected = parse_lock_packages(&content)
            .into_iter()
            .filter(|package| {
                package
                    .source
                    .as_deref()
                    .is_some_and(|source| !is_crates_io_source(source))
            })
            .map(|package| {
                serde_json::json!([package.name, package.version, package.source]).to_string()
            })
            .collect();
        Ok(Snapshot {
            bytes: Some(content.into_bytes()),
            packages: entries
                .into_iter()
                .flat_map(|(name, versions)| {
                    versions
                        .into_iter()
                        .map(move |version| (name.clone(), version))
                })
                .collect(),
            protected,
            ..Default::default()
        })
    }
    async fn cargo_age(
        &self,
        job: &InstallJob,
        baseline: &RegistryLockEntries,
        preferred: &PreferredVersions,
        progress: &Progress,
    ) -> (LockAgeAuditResult, Option<AuditReceipt>) {
        self.0
            .enforce_lock_age_rust_tracked(
                &job.directory,
                job.policy.as_ref().expect("prepared policy"),
                baseline,
                preferred,
                progress.bar().as_ref(),
            )
            .await
    }
    async fn node_age(&self, job: &InstallJob, progress: &Progress) -> Result<(), String> {
        let audit = crate::node_lock::audit_node_lock(
            &job.directory,
            "pnpm",
            job.policy.as_ref().expect("prepared policy"),
            progress.bar().as_ref(),
        )
        .await;
        if audit.has_failures() {
            Err(audit.failure_messages().join("; "))
        } else {
            Ok(())
        }
    }
    async fn osv(&self, language: Language, name: &str, version: &str) -> Result<(), String> {
        self.0
            .check_locked_version(
                if language == Language::Rust {
                    "crates.io"
                } else {
                    "npm"
                },
                name,
                version,
            )
            .await
    }
}

/// Execute the opt-in path while keeping the recovery snapshots alive through verification.
pub async fn run(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    plan: &InstallPlan,
    result: &mut OrchestratorResult,
) -> RefreshOutcome {
    let pm = SystemPackageManager::new();
    run_with(
        args,
        orchestrator,
        plan,
        result,
        &SystemVerification(orchestrator),
        |command| pm.run_prepared_install(command),
    )
    .await
}

async fn run_with(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    plan: &InstallPlan,
    result: &mut OrchestratorResult,
    verifier: &impl Verification,
    mut runner: impl FnMut(&PreparedInstall) -> InstallResult,
) -> RefreshOutcome {
    if args.dry_run {
        return RefreshOutcome {
            install_failed: false,
            audit_failed: false,
        };
    }
    let jobs: Vec<_> = plan
        .jobs
        .iter()
        .filter(|job| matches!(job.language, Language::Node | Language::Rust))
        .collect();
    let mut tx = InstallTransactions::begin(plan, result, &orchestrator.rust_lock_boundary());
    let mut progress = Progress::new(!args.quiet);
    let mut baselines = BTreeMap::new();
    let mut admitted = BTreeMap::new();
    let mut errors = BTreeMap::new();
    let manifests = tx.declaration_snapshots();
    for job in &jobs {
        if tx.pending_report().covers_directory(&job.directory) {
            continue;
        }
        progress.spinner(&format!("Reading {} lock refresh baseline", job.language));
        match verifier.snapshot(job).await {
            Ok(snapshot) => {
                let checked = tx.check_lock(&lock_path(job), snapshot.bytes.as_deref());
                if let Err(error) = checked {
                    reject(&mut tx, &mut errors, job, error);
                } else {
                    baselines.insert((job.directory.clone(), job.language.to_string()), snapshot);
                }
            }
            Err(error) => reject(&mut tx, &mut errors, job, error),
        }
    }
    tx.execute(|command| {
        progress.spinner(&format!("Resolving {} lock refresh", command.language));
        runner(command)
    });
    let rust_plan = plan.rust_audit_plan(orchestrator, result);
    for job in &jobs {
        if tx.pending_report().covers_directory(&job.directory) {
            continue;
        }
        let key = (job.directory.clone(), job.language.to_string());
        let Some(before) = baselines.get(&key) else {
            continue;
        };
        progress.spinner(&format!("Verifying {} lock refresh", job.language));
        let age = if job.language == Language::Rust {
            let baseline = before.packages.iter().fold(
                RegistryLockEntries::new(),
                |mut out, (name, version)| {
                    out.entry(name.clone()).or_default().push(version.clone());
                    out
                },
            );
            let empty = PreferredVersions::new();
            let preferred = rust_plan
                .locks
                .iter()
                .find(|lock| lock.directory == job.directory)
                .map_or(&empty, |lock| &lock.preferred);
            let (audit, receipt) = verifier
                .cargo_age(job, &baseline, preferred, &progress)
                .await;
            let ownership = receipt.map_or(Ok(()), |receipt| {
                tx.accept_audit_lock(&lock_path(job), &receipt.before, &receipt.after)
            });
            ownership.and_then(|_| if audit.has_unresolved() { Err(format!("Cargo.lock did not pass --age: {} unresolved adjustment(s), {} unchecked; {}", audit.adjustments.iter().filter(|item| !item.status.is_resolved()).count(), audit.unchecked, audit.problems.join("; "))) } else { Ok(()) })
        } else {
            verifier.node_age(job, &progress).await
        };
        if let Err(error) = age {
            reject(&mut tx, &mut errors, job, error);
            continue;
        }
        let checked = async {
            let snapshot = verifier.snapshot(job).await?;
            tx.check_lock(&lock_path(job), snapshot.bytes.as_deref())?;
            if job.language == Language::Node {
                let failures = crate::node_lock::target_failures(&result.summary, &job.directory, &snapshot.node_direct_edges);
                if !failures.is_empty() { return Err(failures.into_iter().map(|failure| failure.message).collect::<Vec<_>>().join("; ")); }
            }
            if snapshot.protected != before.protected { return Err("lock refresh changed an external dependency whose registry policy cannot be verified".into()); }
            check_osv(verifier, job.language, &before.packages, &snapshot.packages, &progress).await?;
            // Recheck the admitted bytes after network operations.
            tx.check_lock(&lock_path(job), snapshot.bytes.as_deref())?;
            Ok::<_,String>(snapshot)
        }.await;
        match checked {
            Ok(snapshot) => {
                admitted.insert(key, snapshot);
            }
            Err(error) => reject(&mut tx, &mut errors, job, error),
        }
    }
    // Match the app's direct importer and Cargo root edges, never every copy in a shared lock.
    for job in &jobs {
        if tx.pending_report().covers_directory(&job.directory) {
            continue;
        }
        if let Err(error) =
            check_tauri(job, &admitted, verifier, &orchestrator.rust_lock_boundary()).await
        {
            reject(&mut tx, &mut errors, job, error);
        }
    }
    verify_manifests(&manifests, &mut tx, &mut errors);
    tx.materialize_nodes(|command| {
        progress.spinner("Installing verified pnpm lockfile");
        runner(command)
    });
    // Lifecycle scripts must not substitute a different resolution after verification.
    for job in &jobs {
        if tx.pending_report().covers_directory(&job.directory) {
            continue;
        }
        if let Some(snapshot) = admitted.get(&(job.directory.clone(), job.language.to_string()))
            && let Err(error) = tx.check_lock(&lock_path(job), snapshot.bytes.as_deref())
        {
            reject(&mut tx, &mut errors, job, error);
        }
    }
    verify_manifests(&manifests, &mut tx, &mut errors);
    progress.finish_and_clear();
    let report = tx.rollback();
    let _ = report.print(&mut std::io::stderr());
    report.mark_summary(&mut result.summary);
    if jobs.is_empty() {
        result.summary.lock_refreshes.push(LockRefreshResult {
            directory: args.path.clone(),
            language: None,
            status: "unsupported".into(),
            changed: false,
            error: Some("no eligible pnpm or Cargo lock refresh scope was found".into()),
        });
    }
    for job in jobs {
        let key = (job.directory.clone(), job.language.to_string());
        let failed = report.covers_directory(&job.directory);
        result.summary.lock_refreshes.push(LockRefreshResult {
            directory: job.directory.clone(),
            language: Some(job.language),
            status: if failed { "failed" } else { "completed" }.into(),
            changed: !failed
                && baselines
                    .get(&key)
                    .zip(admitted.get(&key))
                    .is_some_and(|(before, after)| before.bytes != after.bytes),
            error: failed.then(|| {
                errors.get(&job.directory).cloned().unwrap_or_else(|| {
                    "this shared install unit failed; see recovery report".into()
                })
            }),
        });
    }
    RefreshOutcome {
        install_failed: report.has_install_failures(),
        audit_failed: !errors.is_empty()
            || result
                .summary
                .lock_refreshes
                .iter()
                .any(|item| item.status != "completed"),
    }
}

fn reject(
    tx: &mut InstallTransactions,
    errors: &mut BTreeMap<PathBuf, String>,
    job: &InstallJob,
    error: String,
) {
    tx.fail_verification(&job.directory, error.clone());
    errors.insert(job.directory.clone(), error);
}

fn verify_manifests(
    manifests: &[(PathBuf, (FileGuard, FileState))],
    tx: &mut InstallTransactions,
    errors: &mut BTreeMap<PathBuf, String>,
) {
    for (directory, (guard, before)) in manifests {
        if guard
            .read()
            .map_or(true, |current| !before.matches(&current))
        {
            let error =
                "a manifest changed during lock refresh; declarations must remain unchanged"
                    .to_string();
            tx.fail_verification(directory, error.clone());
            errors.insert(directory.clone(), error);
        }
    }
}

async fn check_osv(
    verifier: &impl Verification,
    language: Language,
    before: &BTreeSet<(String, String)>,
    after: &BTreeSet<(String, String)>,
    progress: &Progress,
) -> Result<(), String> {
    let targets: Vec<_> = after.difference(before).collect();
    let bar = progress.bar();
    if let Some(bar) = &bar {
        bar.set_position(0);
        bar.set_length(targets.len() as u64);
    }
    let bar = bar.as_ref();
    let requests = stream::iter(targets)
        .map(|(name, version)| async move {
            if let Some(bar) = &bar {
                bar.set_message(format!("OSV: {name}@{version}"));
            }
            let result = verifier.osv(language, name, version).await;
            if let Some(bar) = &bar {
                bar.inc(1);
            }
            result
        })
        .buffer_unordered(8);
    tokio::time::timeout(VERIFICATION_BUDGET, async {
        tokio::pin!(requests);
        while let Some(result) = requests.next().await {
            result?;
        }
        Ok(())
    })
    .await
    .map_err(|_| "lock refresh OSV verification timed out".to_string())?
}

async fn check_tauri(
    job: &InstallJob,
    snapshots: &BTreeMap<(PathBuf, String), Snapshot>,
    verifier: &impl Verification,
    boundary: &Path,
) -> Result<(), String> {
    let mut apps = BTreeSet::new();
    for path in &job.manifests {
        if path.ends_with("src-tauri/Cargo.toml")
            && let Some(app) = path.parent().and_then(Path::parent)
        {
            apps.insert(app.to_path_buf());
        }
        if path.file_name().is_some_and(|name| name == "package.json")
            && path
                .parent()
                .is_some_and(|root| root.join("src-tauri/Cargo.toml").is_file())
        {
            apps.insert(path.parent().unwrap().to_path_buf());
        }
    }
    for app in apps {
        let manifest = app.join("src-tauri/Cargo.toml");
        let node_dir = node_resolution_directory(&app.join("package.json"));
        let owned;
        let node = if let Some(snapshot) =
            snapshots.get(&(node_dir.clone(), Language::Node.to_string()))
        {
            snapshot
        } else {
            let peer = InstallJob {
                directory: node_dir,
                language: Language::Node,
                policy: job.policy.clone(),
                scope: Ok(InstallScope::Refresh),
                manifests: vec![app.join("package.json")],
            };
            owned = verifier.snapshot(&peer).await?;
            &owned
        };
        let app_identity = app.canonicalize().map_err(|error| error.to_string())?;
        let Some(direct) = node.node_direct.get(&app_identity) else {
            return Err("cannot verify the Tauri app's direct pnpm importer".into());
        };
        let npm: Vec<_> = crate::tauri_sync::TAURI_NPM_PACKAGES
            .iter()
            .filter_map(|name| direct.get(*name))
            .collect();
        if npm.is_empty() {
            continue;
        }
        let root = crate::cargo_scope::rust_resolution_directory(&manifest, boundary)
            .map_err(|error| error.to_string())?;
        let (lock, _) = read_audit_lock(&root.join("Cargo.lock"))?;
        let rust = locked_tauri_version(&manifest, &lock)?;
        if npm
            .iter()
            .any(|version| !crate::tauri_sync::versions_match(version, &rust))
        {
            return Err(
                "the final Tauri npm/Cargo lock versions have different major.minor series".into(),
            );
        }
    }
    Ok(())
}

fn locked_tauri_version(manifest: &Path, lock: &str) -> Result<String, String> {
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let name = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or("cannot identify the Tauri app's Cargo package name")?;
    let packages = parse_lock_packages(lock);
    let roots: Vec<_> = packages
        .iter()
        .filter(|package| package.source.is_none() && package.name == name)
        .collect();
    let [root] = roots.as_slice() else {
        return Err(
            "cannot uniquely identify the Tauri app's Cargo root package in the lock".into(),
        );
    };
    let candidates: Vec<_> = packages
        .iter()
        .filter(|package| {
            package.name == crate::tauri_sync::TAURI_CRATE
                && root.dependencies.iter().any(|edge| {
                    edge.name == package.name
                        && edge
                            .version
                            .as_ref()
                            .is_none_or(|version| version == &package.version)
                        && edge.source.as_ref().is_none_or(|source| {
                            package.source.as_ref().is_some_and(|candidate| {
                                candidate.split('#').next() == Some(source.as_str())
                            })
                        })
                })
        })
        .collect();
    let [tauri] = candidates.as_slice() else {
        return Err("cannot uniquely identify the Tauri app's direct Cargo lock version".into());
    };
    Ok(tauri.version.clone())
}

#[cfg(test)]
mod tests;
