//! 書き込み前に既存 pnpm lock の age を確認する。

use super::{OrchestratorError, tauri_project_directory};
use crate::cargo_scope::{CargoMembership, CargoRoot};
use crate::domain::{Language, ManifestUpdateResult, UpdateSummary};
use crate::manifest::ManifestInfo;
use crate::node_lock::NodeLockAuditResult;
use crate::node_scope::{node_resolution_directory, node_scopes};
use crate::package_manager::{SystemPackageManager, pnpm_age_minutes};
use crate::update::AgePolicy;
use async_trait::async_trait;
use indicatif::ProgressBar;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) struct ScopeContext<'a> {
    pub manifests: &'a [ManifestInfo],
    pub cargo: &'a CargoMembership<'a>,
}

#[async_trait]
pub(super) trait ExistingLockAuditor: Sync {
    async fn audit(
        &self,
        directory: &Path,
        policy: &AgePolicy,
        bar: Option<&ProgressBar>,
    ) -> NodeLockAuditResult;
}

pub(super) struct PnpmLockAuditor;

#[async_trait]
impl ExistingLockAuditor for PnpmLockAuditor {
    async fn audit(
        &self,
        directory: &Path,
        policy: &AgePolicy,
        bar: Option<&ProgressBar>,
    ) -> NodeLockAuditResult {
        crate::node_lock::audit_node_lock(directory, "pnpm", policy, bar).await
    }
}

pub(super) struct BlockedScope {
    directory: PathBuf,
    manifests: Vec<PathBuf>,
    policy: Option<AgePolicy>,
    failure: Failure,
}

enum Failure {
    Audit(NodeLockAuditResult),
    Policy(String),
    Lock(String),
}

impl Failure {
    fn reason(&self) -> &'static str {
        match self {
            Self::Audit(audit) if !audit.violations.is_empty() => {
                "the existing pnpm-lock.yaml violates --age"
            }
            Self::Audit(_) => "the existing pnpm-lock.yaml could not be verified against --age",
            Self::Policy(_) => "the shared pnpm age policy could not be read or interpreted",
            Self::Lock(_) => "the existing pnpm-lock.yaml could not be read or inspected",
        }
    }
}

/// Lock の判定失敗を「lock なし」や npm へのフォールバックへ落とさない。
fn existing_pnpm_lock(directory: &Path) -> Result<bool, String> {
    let path = directory.join("pnpm-lock.yaml");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("could not inspect pnpm-lock.yaml: {error}")),
        Ok(_) => {}
    }
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("could not read pnpm-lock.yaml metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("pnpm-lock.yaml is not a regular file".into());
    }
    match SystemPackageManager::new().resolve_package_manager(Language::Node, directory) {
        Some((_, "pnpm")) => Ok(true),
        _ => Err("could not confirm the package manager for the existing pnpm-lock.yaml".into()),
    }
}

pub(super) async fn preflight(
    summary: &UpdateSummary,
    scopes: &ScopeContext<'_>,
    errors: &[OrchestratorError],
    policy_for: impl Fn(&Path) -> Result<AgePolicy, String>,
    auditor: &impl ExistingLockAuditor,
    bar: Option<&ProgressBar>,
) -> Vec<BlockedScope> {
    let mut inventory = summary.manifests.clone();
    for manifest in scopes
        .manifests
        .iter()
        .filter(|manifest| manifest.language == Language::Node)
    {
        // Parse/check failures omit their result. Keep selected failed Node projects
        // in the scope without reintroducing languages excluded by CLI flags.
        if !inventory.iter().any(|result| result.path == manifest.path)
            && errors.iter().any(|error| {
                matches!(error,
                OrchestratorError::ManifestParseError { path, .. }
                if path == &manifest.path.display().to_string())
            })
        {
            inventory.push(ManifestUpdateResult::new(&manifest.path, Language::Node));
        }
    }
    let mut blocked = Vec::new();
    for scope in node_scopes(&inventory, policy_for) {
        if matches!(&scope.policy, Ok(policy) if policy.min_age.is_none())
            || (scope.updated.is_empty() && !has_related_updates(summary, scopes, &scope.directory))
        {
            continue;
        }
        let failure = match existing_pnpm_lock(&scope.directory) {
            Ok(false) => continue,
            Err(error) => Failure::Lock(error),
            Ok(true) => match &scope.policy {
                Err(error) => Failure::Policy(error.clone()),
                Ok(policy) => {
                    let audit = auditor.audit(&scope.directory, policy, bar).await;
                    if !audit.has_failures() {
                        continue;
                    }
                    Failure::Audit(audit)
                }
            },
        };
        blocked.push(BlockedScope {
            directory: scope.directory,
            manifests: scope.updated,
            policy: scope.policy.ok(),
            failure,
        });
    }
    blocked
}

/// 判定失敗や更新なしの Node も、到達できる install に実更新があれば確認する。
fn has_related_updates(
    summary: &UpdateSummary,
    scopes: &ScopeContext<'_>,
    node_directory: &Path,
) -> bool {
    if !summary
        .manifests
        .iter()
        .any(|manifest| manifest.has_updates())
    {
        return false;
    }
    let linked = linked_scopes(summary, scopes, node_directory);
    summary
        .manifests
        .iter()
        .any(|manifest| manifest.has_updates() && linked.paths.contains(&manifest.path))
}

/// writer は judged_updates を使うため、失敗記録と明示除外の両方が必要。
pub(super) fn block(
    summary: &mut UpdateSummary,
    errors: &mut Vec<OrchestratorError>,
    scopes: &ScopeContext<'_>,
    blocked: &[BlockedScope],
) -> BTreeSet<PathBuf> {
    use colored::Colorize as _;
    let mut skip = BTreeSet::new();
    for scope in blocked {
        for line in report_lines(scope) {
            eprintln!("{}", line.yellow());
        }
        let mut linked = linked_scopes(summary, scopes, &scope.directory);
        linked.paths.extend(scope.manifests.iter().cloned());
        for (directory, unverified) in &linked.node_directories {
            if errors.iter().any(|error| matches!(error,
                OrchestratorError::NodePreflightError { directory: stopped, .. } if stopped == directory)) {
                continue;
            }
            let message = if directory == &scope.directory {
                scope.failure.reason().to_string()
            } else if let Some(reason) = unverified {
                reason.clone()
            } else {
                format!(
                    "shares a synchronized Tauri install with the stopped pnpm scope in {}",
                    scope.directory.display()
                )
            };
            errors.push(OrchestratorError::NodePreflightError {
                directory: directory.clone(),
                message,
            });
        }
        if scopes.manifests.iter().any(|manifest| {
            manifest.language == Language::Rust
                && linked.paths.contains(scopes.cargo.identity(&manifest.path))
        }) {
            for (manifest, root) in scopes.cargo.roots() {
                if !linked.paths.contains(manifest) || errors.iter().any(|error| matches!(error,
                    OrchestratorError::CargoPreflightError { manifest: stopped, .. } if stopped == manifest)) {
                    continue;
                }
                let root = match root {
                    CargoRoot::Known(root) => Some(root.clone()),
                    CargoRoot::Unknown(_) => None,
                };
                let message = linked
                    .unverified
                    .get(manifest)
                    .cloned()
                    .unwrap_or_else(|| scope.failure.reason().into());
                errors.push(OrchestratorError::CargoPreflightError {
                    manifest: manifest.clone(),
                    root,
                    message,
                });
            }
        }
        for manifest in &mut summary.manifests {
            if !linked.paths.contains(&manifest.path) || skip.contains(&manifest.path) {
                continue;
            }
            let indices: Vec<_> = manifest
                .results
                .iter()
                .enumerate()
                .filter(|(_, result)| result.is_update())
                .map(|(index, _)| index)
                .collect();
            let reason = linked
                .unverified
                .get(&manifest.path)
                .map(String::as_str)
                .unwrap_or_else(|| scope.failure.reason());
            for index in indices {
                manifest.record_write_error(index, format!("not written: {reason}"));
            }
            if manifest.has_judged_updates() {
                errors.push(OrchestratorError::WriteError {
                    path: manifest.path.display().to_string(),
                    message: format!(
                        "{} in {}; the manifest was not changed and its install was skipped",
                        reason,
                        scope.directory.display()
                    ),
                });
            }
        }
        skip.extend(linked.paths);
    }
    skip
}

struct LinkedScopes {
    paths: BTreeSet<PathBuf>,
    /// None は確認済みの接続。Some は独立と確認できなかった原因。
    node_directories: BTreeMap<PathBuf, Option<String>>,
    unverified: BTreeMap<PathBuf, String>,
}

fn unverified_cargo_reason(error: &str) -> String {
    format!(
        "Cargo install scope could not be confirmed as independent from the stopped Tauri install: {error}"
    )
}

/// 共有を確認できた経路が後から見つかれば、不明経路より優先する。
fn insert_link<K: Ord>(
    links: &mut BTreeMap<K, Option<String>>,
    directory: K,
    reason: Option<String>,
) -> bool {
    match links.entry(directory) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(reason);
            true
        }
        std::collections::btree_map::Entry::Occupied(mut entry)
            if entry.get().is_some() && reason.is_none() =>
        {
            entry.insert(None);
            true
        }
        _ => false,
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ScopeVertex {
    Node(PathBuf),
    Root(PathBuf),
    UnconfirmedRoot,
}

#[derive(Default)]
struct ScopeGraph {
    edges: BTreeMap<ScopeVertex, BTreeMap<ScopeVertex, Option<String>>>,
    unknown: Vec<(PathBuf, PathBuf, String)>,
    roots: BTreeSet<ScopeVertex>,
}

#[derive(Clone, PartialEq, Eq)]
struct ReachPath {
    used: BTreeSet<PathBuf>,
    reason: Option<String>,
}

impl ScopeGraph {
    fn connect(&mut self, a: ScopeVertex, b: ScopeVertex, reason: Option<String>) {
        insert_link(
            self.edges.entry(a.clone()).or_default(),
            b.clone(),
            reason.clone(),
        );
        insert_link(self.edges.entry(b).or_default(), a, reason);
    }

    fn build(tauri: &[(PathBuf, PathBuf)], roots: &BTreeMap<PathBuf, CargoRoot>) -> Self {
        let mut graph = Self {
            roots: roots
                .values()
                .filter_map(|root| match root {
                    CargoRoot::Known(path) => Some(ScopeVertex::Root(path.clone())),
                    CargoRoot::Unknown(_) => None,
                })
                .collect(),
            ..Default::default()
        };
        for (path, node) in tauri {
            match &roots[path] {
                CargoRoot::Known(root) => graph.connect(
                    ScopeVertex::Node(node.clone()),
                    ScopeVertex::Root(root.clone()),
                    None,
                ),
                CargoRoot::Unknown(error) => {
                    graph.unknown.push((
                        path.clone(),
                        node.clone(),
                        unverified_cargo_reason(error),
                    ));
                    graph.roots.insert(ScopeVertex::UnconfirmedRoot);
                }
            }
        }
        graph
    }

    fn reach(&self, directory: &Path) -> BTreeMap<ScopeVertex, Option<String>> {
        let start = ScopeVertex::Node(directory.to_path_buf());
        let mut reached = BTreeMap::from([(start.clone(), None)]);
        let first = ReachPath {
            used: BTreeSet::new(),
            reason: None,
        };
        let mut paths = BTreeMap::from([(start.clone(), vec![first.clone()])]);
        let mut queue = VecDeque::from([(start, first)]);
        while let Some((vertex, path)) = queue.pop_front() {
            if !paths[&vertex].contains(&path) {
                continue;
            }
            if let Some(edges) = self.edges.get(&vertex) {
                for (next, reason) in edges {
                    let next_path = ReachPath {
                        used: path.used.clone(),
                        reason: path.reason.clone().or_else(|| reason.clone()),
                    };
                    Self::visit(
                        &mut paths,
                        &mut reached,
                        &mut queue,
                        next.clone(),
                        next_path,
                    );
                }
            }
            for (manifest, node, reason) in &self.unknown {
                let from_node =
                    matches!(&vertex, ScopeVertex::Node(directory) if directory == node);
                let from_root =
                    matches!(&vertex, ScopeVertex::Root(_) | ScopeVertex::UnconfirmedRoot);
                if path.used.contains(manifest) || (!from_node && !from_root) {
                    continue;
                }
                let mut next_path = path.clone();
                next_path.used.insert(manifest.clone());
                next_path.reason = next_path.reason.or_else(|| Some(reason.clone()));
                // Aliases share this single unknown membership. Visit all their
                // Node installs together without assigning it another root.
                for (_, alias_node, _) in self
                    .unknown
                    .iter()
                    .filter(|(target, _, _)| target == manifest)
                {
                    Self::visit(
                        &mut paths,
                        &mut reached,
                        &mut queue,
                        ScopeVertex::Node(alias_node.clone()),
                        next_path.clone(),
                    );
                }
                if from_node {
                    for root in &self.roots {
                        Self::visit(
                            &mut paths,
                            &mut reached,
                            &mut queue,
                            root.clone(),
                            next_path.clone(),
                        );
                    }
                }
            }
        }
        reached
    }

    fn visit(
        paths: &mut BTreeMap<ScopeVertex, Vec<ReachPath>>,
        reached: &mut BTreeMap<ScopeVertex, Option<String>>,
        queue: &mut VecDeque<(ScopeVertex, ReachPath)>,
        vertex: ScopeVertex,
        path: ReachPath,
    ) {
        let alternatives = paths.entry(vertex.clone()).or_default();
        // A smaller consumed set can take every continuation of a larger one.
        // Keep confirmed paths over uncertain ones. This also closes cycles.
        let dominates = |a: &ReachPath, b: &ReachPath| {
            a.used.is_subset(&b.used) && (a.reason.is_none() || b.reason.is_some())
        };
        if alternatives.iter().any(|old| dominates(old, &path)) {
            return;
        }
        alternatives.retain(|old| !dominates(&path, old));
        alternatives.push(path.clone());
        insert_link(reached, vertex.clone(), path.reason.clone());
        queue.push_back((vertex, path));
    }
}

/// Gate と停止が同じ所属 snapshot と同じ到達規則を見る。
fn linked_scopes(
    summary: &UpdateSummary,
    scopes: &ScopeContext<'_>,
    directory: &Path,
) -> LinkedScopes {
    let mut linked = LinkedScopes {
        paths: BTreeSet::new(),
        node_directories: BTreeMap::from([(directory.to_path_buf(), None)]),
        unverified: BTreeMap::new(),
    };
    let tauri: Vec<_> = scopes
        .manifests
        .iter()
        .filter_map(|manifest| {
            let project = tauri_project_directory(manifest, scopes.manifests)?;
            Some((
                scopes.cargo.identity(&manifest.path).to_path_buf(),
                node_resolution_directory(&project.join("package.json")),
            ))
        })
        .collect();
    if tauri.is_empty() {
        linked.paths.extend(
            summary
                .manifests
                .iter()
                .filter(|manifest| {
                    manifest.language == Language::Node
                        && node_resolution_directory(&manifest.path) == directory
                })
                .map(|manifest| manifest.path.clone()),
        );
        return linked;
    }
    let roots = scopes.cargo.roots();
    let reached = ScopeGraph::build(&tauri, roots).reach(directory);
    for (vertex, reason) in &reached {
        if let ScopeVertex::Node(directory) = vertex {
            linked
                .node_directories
                .insert(directory.clone(), reason.clone());
        }
    }
    let reached_cargo = reached
        .keys()
        .any(|vertex| !matches!(vertex, ScopeVertex::Node(_)));
    if reached_cargo {
        // Preserve stops for original Rust manifests omitted by parse/check and
        // for aliases, even when they have no actual update to record as failed.
        for (manifest, root) in roots {
            let cause = match root {
                CargoRoot::Known(root) => reached.get(&ScopeVertex::Root(root.clone())).cloned(),
                CargoRoot::Unknown(error) => Some(Some(unverified_cargo_reason(error))),
            };
            if let Some(cause) = cause {
                linked.paths.insert(manifest.clone());
                if let Some(reason) = cause {
                    linked.unverified.insert(manifest.clone(), reason);
                }
            }
        }
    }
    for manifest in &summary.manifests {
        match manifest.language {
            Language::Node => {
                let directory = node_resolution_directory(&manifest.path);
                if let Some(cause) = linked.node_directories.get(&directory) {
                    linked.paths.insert(manifest.path.clone());
                    if let Some(reason) = cause {
                        linked
                            .unverified
                            .insert(manifest.path.clone(), reason.clone());
                    }
                }
            }
            Language::Rust if reached_cargo => {
                let cause = match &roots[scopes.cargo.identity(&manifest.path)] {
                    CargoRoot::Known(root) => {
                        reached.get(&ScopeVertex::Root(root.clone())).cloned()
                    }
                    CargoRoot::Unknown(error) => Some(Some(unverified_cargo_reason(error))),
                };
                if let Some(cause) = cause {
                    linked.paths.insert(manifest.path.clone());
                    if let Some(reason) = cause {
                        linked.unverified.insert(manifest.path.clone(), reason);
                    }
                }
            }
            _ => {}
        }
    }
    linked
}

fn repair_install_command(age: Duration) -> String {
    let minutes = pnpm_age_minutes(age);
    format!(
        "npm_config_minimum_release_age={minutes} pnpm_config_minimum_release_age={minutes} pnpm install"
    )
}

fn report_lines(scope: &BlockedScope) -> Vec<String> {
    let directory = scope.directory.display();
    let mut lines = vec![format!(
        "  {directory} — {}; manifests were not changed and pnpm install was skipped",
        scope.failure.reason()
    )];
    match &scope.failure {
        Failure::Audit(audit) => {
            lines.extend(
                audit
                    .failure_messages()
                    .into_iter()
                    .map(|message| format!("  {directory} — {message}")),
            );
            if !audit.violations.is_empty() {
                let command = repair_install_command(
                    scope.policy.as_ref().unwrap().install_min_age().unwrap(),
                );
                lines.push(format!("  {directory} — to rebuild under the age policy, run `pnpm clean --lockfile`, then `{command}` in this directory. Age minutes are calculated from the fixed cutoff when this command is printed. If running it later, rerun depup to regenerate the instructions. pnpm 11 or later is required for clean --lockfile, which deletes node_modules and the lockfile. This re-resolves the whole lockfile, so unrelated dependency versions can change"));
            }
        }
        Failure::Policy(error) | Failure::Lock(error) => {
            lines.push(format!("  {directory} — {error}"))
        }
    }
    lines
}

#[cfg(test)]
mod tests;
