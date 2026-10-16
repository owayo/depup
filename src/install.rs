//! install の実行単位と、それに伴う Cargo.lock 監査の計画。
//!
//! install 前の lock を保存し、install 後に生成された lock も含めて監査を計画する。

use crate::cargo_rollback::audit::{PreferredVersions, read_audit_lock};
use crate::cargo_rollback::report::{DisplayedUpdate, LockedVersionMismatch, lock_mismatches};
use crate::domain::{Language, UpdateResult};
use crate::manifest::{
    RegistryLockEntries, detect_manifests, find_cargo_lock_upward, read_registry_entries,
};
use crate::node_scope::{node_resolution_directory, node_scopes};
use crate::orchestrator::{Orchestrator, OrchestratorResult};
use crate::package_manager::InstallScope;
use crate::update::AgePolicy;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// 1 つのディレクトリ・言語の install。
pub struct InstallJob {
    pub directory: PathBuf,
    pub language: Language,
    /// 解釈できない native 設定は実行時にエラーとして扱う。
    pub policy: Result<AgePolicy, String>,
    /// Scope preparation errors must never fall back to a full update.
    pub scope: Result<InstallScope, String>,
    /// The planned manifests, including lock-only Git updates.
    pub manifests: Vec<PathBuf>,
}

/// install 前に確定する実行単位と、変更を判定するための lock の保存。
pub struct InstallPlan {
    pub jobs: Vec<InstallJob>,
    /// Wrapper distributions whose properties file was actually written.
    pub wrapper_updates: Vec<(PathBuf, String)>,
    /// Node lock の監査は、マニフェストの更新がない実行でも計画する。
    pub node_audits: Vec<InstallJob>,
    baselines: BTreeMap<PathBuf, Result<RegistryLockEntries, String>>,
}

impl InstallPlan {
    pub fn new(
        orchestrator: &Orchestrator,
        result: &OrchestratorResult,
        monorepo_dirs: &Option<Vec<PathBuf>>,
        default_path: &Path,
    ) -> Self {
        Self::new_scoped(orchestrator, result, monorepo_dirs, default_path, false)
    }

    pub fn new_scoped(
        orchestrator: &Orchestrator,
        result: &OrchestratorResult,
        monorepo_dirs: &Option<Vec<PathBuf>>,
        default_path: &Path,
        only_mode: bool,
    ) -> Self {
        let node_audits = node_audit_jobs(orchestrator, result);
        let wrapper_updates = result
            .summary
            .manifests
            .iter()
            .filter(|manifest| {
                manifest
                    .path
                    .ends_with("gradle/wrapper/gradle-wrapper.properties")
                    && result.write_results.iter().any(|write| {
                        write.path == manifest.path && write.file_modified && !write.has_errors()
                    })
            })
            .filter_map(|manifest| {
                let version = manifest.results.iter().find_map(|item| match item {
                    UpdateResult::Update {
                        dependency,
                        new_version,
                        ..
                    } if dependency.name == crate::manifest::GRADLE_WRAPPER_PACKAGE => {
                        Some(new_version.clone())
                    }
                    _ => None,
                })?;
                let project = manifest.path.parent()?.parent()?.parent()?.to_path_buf();
                Some((project, version))
            })
            .collect();
        let shared_node_policies = &node_audits;
        let mut jobs: Vec<_> = build_install_map(result, monorepo_dirs, default_path)
            .into_iter()
            .flat_map(|(directory, languages)| {
                languages.into_iter().map(move |language| InstallJob {
                    policy: if language == Language::Node {
                        shared_node_policies
                            .iter()
                            .find(|job| job.directory == directory)
                            .expect("every Node install has an audit scope")
                            .policy
                            .clone()
                    } else {
                        install_age_policy(orchestrator, result, &directory, language)
                    },
                    directory: directory.clone(),
                    language,
                    scope: Ok(InstallScope::Full),
                    manifests: result
                        .summary
                        .manifests
                        .iter()
                        .filter(|manifest| {
                            manifest.language == language
                                && if language == Language::Node {
                                    node_resolution_directory(&manifest.path) == directory
                                } else {
                                    monorepo_dirs.as_ref().map_or_else(
                                        || default_path.to_path_buf(),
                                        |dirs| {
                                            nearest_monorepo_dir(&manifest.path, dirs, default_path)
                                        },
                                    ) == directory
                                }
                        })
                        .map(|manifest| manifest.path.clone())
                        .collect(),
                })
            })
            .collect();
        if orchestrator.refresh_lock_requested() {
            refresh::plan_jobs(&mut jobs, orchestrator, result, &node_audits);
        } else if only_mode {
            scoped::replace_scoped_jobs(&mut jobs, orchestrator, result);
        } else if result.errors.iter().any(|error| {
            matches!(
                error,
                crate::orchestrator::OrchestratorError::NodePreflightError { .. }
            )
        }) {
            scoped::replace_full_rust_jobs(&mut jobs, orchestrator, result);
        }
        let groups = RustLockGroups::collect(orchestrator, result);
        let baselines = groups
            .locks
            .into_keys()
            .map(|directory| {
                let entries =
                    read_audit_lock(&directory.join("Cargo.lock")).map(|(_, entries)| entries);
                (directory, entries)
            })
            .collect();
        Self {
            jobs,
            wrapper_updates,
            node_audits,
            baselines,
        }
    }

    /// install が新規作成した lock を再検出し、install 前の版を対応付ける。
    pub fn rust_audit_plan(
        &self,
        orchestrator: &Orchestrator,
        result: &OrchestratorResult,
    ) -> RustAuditPlan {
        let groups = RustLockGroups::collect(orchestrator, result);
        let locks = groups
            .locks
            .into_iter()
            .map(|(directory, group)| RustLockPlan {
                baseline: self
                    .baselines
                    .get(&directory)
                    .and_then(|result| result.as_ref().ok())
                    .cloned()
                    .unwrap_or_default(),
                baseline_problem: self
                    .baselines
                    .get(&directory)
                    .and_then(|result| result.as_ref().err())
                    .cloned(),
                directory,
                policy: group.policy,
                preferred: group.preferred,
                manifests: group.manifests,
            })
            .collect();
        RustAuditPlan {
            locks,
            missing_lock_dirs: groups
                .missing_lock_dirs
                .into_iter()
                .filter(|directory| {
                    self.jobs.iter().any(|job| {
                        if job.language != Language::Rust {
                            return false;
                        }
                        match &job.scope {
                            Ok(InstallScope::Full) => directory.starts_with(&job.directory),
                            Ok(_) => crate::cargo_scope::rust_resolution_directory(
                                &directory.join("Cargo.toml"),
                                &orchestrator.rust_lock_boundary(),
                            )
                            .is_ok_and(|root| root == job.directory),
                            Err(_) => false,
                        }
                    })
                })
                .collect(),
        }
    }
}

/// 共有 Cargo.lock ごとの制約・install 前の版・表示した更新先。
pub struct RustLockPlan {
    pub directory: PathBuf,
    pub policy: AgePolicy,
    pub baseline: RegistryLockEntries,
    /// 既存 lock を読めなかった場合、空の下限で自動差し戻ししない。
    pub baseline_problem: Option<String>,
    pub preferred: PreferredVersions,
    manifests: Vec<ManifestLockUpdates>,
}

/// install 後の監査対象。lock ごとの実行順序はパス順で固定する。
pub struct RustAuditPlan {
    pub locks: Vec<RustLockPlan>,
    pub missing_lock_dirs: Vec<PathBuf>,
}

impl RustAuditPlan {
    /// lock は共有単位ごとに一度読み、表示した更新先との相違をマニフェスト順に返す。
    pub fn mismatch_reports(&self) -> Vec<(PathBuf, Vec<LockedVersionMismatch>)> {
        let mut reports = Vec::new();
        for lock in &self.locks {
            let entries = read_registry_entries(&lock.directory.join("Cargo.lock"));
            for manifest in &lock.manifests {
                reports.push((
                    manifest.index,
                    manifest.path.clone(),
                    lock_mismatches(&manifest.updates, &entries),
                ));
            }
        }
        reports.sort_by_key(|(index, _, _)| *index);
        reports
            .into_iter()
            .map(|(_, path, mismatches)| (path, mismatches))
            .collect()
    }
}

struct ManifestLockUpdates {
    index: usize,
    path: PathBuf,
    updates: Vec<DisplayedUpdate>,
}

struct RustLockGroup {
    policy: AgePolicy,
    preferred: PreferredVersions,
    manifests: Vec<ManifestLockUpdates>,
}

struct RustLockGroups {
    locks: BTreeMap<PathBuf, RustLockGroup>,
    missing_lock_dirs: Vec<PathBuf>,
}

impl RustLockGroups {
    fn collect(orchestrator: &Orchestrator, result: &OrchestratorResult) -> Self {
        let boundary = orchestrator.rust_lock_boundary();
        let stopped_paths = stopped_cargo_paths(result);
        let stopped_roots = stopped_cargo_roots(result);
        let stopped = |path: &Path| {
            stopped_paths.contains(&std::fs::canonicalize(path).unwrap_or_else(|_| path.into()))
        };
        let directory_for = |path: &Path| {
            if stopped(path) {
                return None;
            }
            lock_directory(path, &boundary).filter(|directory| {
                !stopped_roots.contains(
                    &std::fs::canonicalize(directory).unwrap_or_else(|_| directory.clone()),
                )
            })
        };
        let mut paths: BTreeSet<PathBuf> = result
            .summary
            .manifests
            .iter()
            .filter(|manifest| manifest.language == Language::Rust && !stopped(&manifest.path))
            .map(|manifest| manifest.path.clone())
            .collect();
        let roots: BTreeSet<PathBuf> = paths
            .iter()
            .filter_map(|path| directory_for(path))
            .collect();
        // 起動場所や更新対象に含まれないメンバーの制約も、同じ lock へ統合する。
        for root in roots {
            paths.extend(
                detect_manifests(&root)
                    .into_iter()
                    .filter(|manifest| {
                        manifest.language == Language::Rust && !stopped(&manifest.path)
                    })
                    .map(|manifest| manifest.path),
            );
        }
        let mut locks: BTreeMap<PathBuf, RustLockGroup> = BTreeMap::new();
        for path in paths {
            let Some(directory) = directory_for(&path) else {
                continue;
            };
            let policy = orchestrator.resolved_age_policy_for(path.parent().unwrap());
            locks
                .entry(directory)
                .and_modify(|group| group.policy.merge(&policy))
                .or_insert_with(|| RustLockGroup {
                    policy,
                    preferred: PreferredVersions::new(),
                    manifests: Vec::new(),
                });
        }
        let mut missing_lock_dirs = BTreeSet::new();
        for (index, manifest) in result.summary.manifests.iter().enumerate() {
            if manifest.language != Language::Rust || stopped(&manifest.path) {
                continue;
            }
            let Some(parent) = manifest.path.parent() else {
                continue;
            };
            let Some(directory) = directory_for(&manifest.path) else {
                if lock_directory(&manifest.path, &boundary).is_some() {
                    continue;
                }
                if orchestrator
                    .resolved_age_policy_for(parent)
                    .min_age
                    .is_some()
                {
                    missing_lock_dirs.insert(parent.to_path_buf());
                }
                continue;
            };
            if !manifest.has_updates() {
                continue;
            }
            let group = locks
                .get_mut(&directory)
                .expect("every detected lock has a policy");
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
            for update in &updates {
                group
                    .preferred
                    .entry(update.name.clone())
                    .or_default()
                    .push(update.to.clone());
            }
            group.manifests.push(ManifestLockUpdates {
                index,
                path: manifest.path.clone(),
                updates,
            });
        }
        Self {
            locks,
            missing_lock_dirs: missing_lock_dirs.into_iter().collect(),
        }
    }
}

fn stopped_cargo_paths(result: &OrchestratorResult) -> BTreeSet<PathBuf> {
    result
        .errors
        .iter()
        .filter_map(|error| match error {
            crate::orchestrator::OrchestratorError::CargoPreflightError { manifest, .. } => {
                Some(manifest.clone())
            }
            _ => None,
        })
        .collect()
}

fn stopped_cargo_roots(result: &OrchestratorResult) -> BTreeSet<PathBuf> {
    result
        .errors
        .iter()
        .filter_map(|error| match error {
            crate::orchestrator::OrchestratorError::CargoPreflightError {
                root: Some(root),
                ..
            } => Some(root.clone()),
            _ => None,
        })
        .collect()
}

/// Cargo.lock の探索と共有単位への変換はここだけで行う。
fn lock_directory(manifest: &Path, boundary: &Path) -> Option<PathBuf> {
    let parent = manifest.parent()?;
    let lock = find_cargo_lock_upward(parent, boundary)?;
    Some(lock.parent().unwrap_or(parent).to_path_buf())
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

fn node_audit_jobs(orchestrator: &Orchestrator, result: &OrchestratorResult) -> Vec<InstallJob> {
    node_scopes(&result.summary.manifests, |directory| {
        orchestrator.resolved_age_policy_for_language(directory, Language::Node)
    })
    .into_iter()
    .filter(|scope| !result.errors.iter().any(|error| {
        matches!(error, crate::orchestrator::OrchestratorError::NodePreflightError { directory, .. } if directory == &scope.directory)
    }))
    .map(|scope| InstallJob {
        directory: scope.directory.clone(),
        language: Language::Node,
        policy: scope.policy,
        scope: Ok(InstallScope::Full),
        manifests: result.summary.manifests.iter().filter(|manifest| manifest.language == Language::Node && node_resolution_directory(&manifest.path) == scope.directory).map(|manifest| manifest.path.clone()).collect(),
    })
    .collect()
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
        let directory = if manifest.language == Language::Node {
            node_resolution_directory(&manifest.path)
        } else {
            match monorepo_dirs {
                Some(dirs) => nearest_monorepo_dir(&manifest.path, dirs, default_path),
                None => default_path.to_path_buf(),
            }
        };
        let languages = dir_langs.entry(directory).or_default();
        if !languages.contains(&manifest.language) {
            languages.push(manifest.language);
        }
    }
    let mut map: Vec<_> = dir_langs.into_iter().collect();
    map.sort_by(|(left, _), (right, _)| left.cmp(right));
    map
}

#[cfg(test)]
mod tests;

pub mod refresh;
mod scoped;

pub mod transaction;
