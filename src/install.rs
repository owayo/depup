//! install の実行単位と、それに伴う Cargo.lock 監査の計画。
//!
//! install 前の lock を保存し、install 後に生成された lock も含めて監査を計画する。

use crate::cargo_rollback::audit::{PreferredVersions, read_audit_lock};
use crate::cargo_rollback::report::{DisplayedUpdate, LockedVersionMismatch, lock_mismatches};
use crate::domain::{Language, UpdateResult};
use crate::manifest::{
    RegistryLockEntries, detect_manifests, find_cargo_lock_upward, read_registry_entries,
};
use crate::orchestrator::{Orchestrator, OrchestratorResult};
use crate::update::AgePolicy;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// 1 つのディレクトリ・言語の install。
pub struct InstallJob {
    pub directory: PathBuf,
    pub language: Language,
    /// 解釈できない native 設定は実行時にエラーとして扱う。
    pub policy: Result<AgePolicy, String>,
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
        let jobs = build_install_map(result, monorepo_dirs, default_path)
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
                })
            })
            .collect();
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
                        job.language == Language::Rust && directory.starts_with(&job.directory)
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
        let mut paths: BTreeSet<PathBuf> = result
            .summary
            .manifests
            .iter()
            .filter(|manifest| manifest.language == Language::Rust)
            .map(|manifest| manifest.path.clone())
            .collect();
        let roots: BTreeSet<PathBuf> = paths
            .iter()
            .filter_map(|path| lock_directory(path, &boundary))
            .collect();
        // 起動場所や更新対象に含まれないメンバーの制約も、同じ lock へ統合する。
        for root in roots {
            paths.extend(
                detect_manifests(&root)
                    .into_iter()
                    .filter(|manifest| manifest.language == Language::Rust)
                    .map(|manifest| manifest.path),
            );
        }
        let mut locks: BTreeMap<PathBuf, RustLockGroup> = BTreeMap::new();
        for path in paths {
            let Some(directory) = lock_directory(&path, &boundary) else {
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
            if manifest.language != Language::Rust {
                continue;
            }
            let Some(parent) = manifest.path.parent() else {
                continue;
            };
            let Some(directory) = lock_directory(&manifest.path, &boundary) else {
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

/// pnpm の共有 lock は、実際に検出される workspace member だけを結び付ける。
fn node_resolution_directory(manifest: &Path) -> PathBuf {
    let parent = manifest.parent().unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    let manifest = parent.join("package.json");
    for directory in parent.ancestors() {
        if directory == parent {
            if [
                "pnpm-lock.yaml",
                "package-lock.json",
                "npm-shrinkwrap.json",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            ]
            .iter()
            .any(|filename| directory.join(filename).exists())
            {
                return directory.to_path_buf();
            }
        } else if directory.join("pnpm-workspace.yaml").is_file()
            && detect_manifests(directory).iter().any(|candidate| {
                candidate.language == Language::Node
                    && std::fs::canonicalize(&candidate.path).is_ok_and(|path| path == manifest)
            })
        {
            return directory.to_path_buf();
        }
    }
    parent
}

fn node_audit_jobs(orchestrator: &Orchestrator, result: &OrchestratorResult) -> Vec<InstallJob> {
    let mut groups: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    for manifest in &result.summary.manifests {
        if manifest.language == Language::Node {
            groups
                .entry(node_resolution_directory(&manifest.path))
                .or_default()
                .insert(manifest.path.clone());
        }
    }
    groups
        .into_iter()
        .map(|(directory, mut manifests)| {
            manifests.extend(
                detect_manifests(&directory)
                    .into_iter()
                    .filter(|manifest| manifest.language == Language::Node)
                    .map(|manifest| manifest.path),
            );
            let policy = (|| {
                let mut policy =
                    orchestrator.resolved_age_policy_for_language(&directory, Language::Node)?;
                for manifest in manifests {
                    if let Some(parent) = manifest.parent() {
                        policy.merge(
                            &orchestrator
                                .resolved_age_policy_for_language(parent, Language::Node)?,
                        );
                    }
                }
                Ok(policy)
            })();
            InstallJob {
                directory,
                language: Language::Node,
                policy,
            }
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
