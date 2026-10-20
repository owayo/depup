//! Undo only partial audited changes that remain related to unresolved install-time versions.

use super::*;
use crate::cargo_rollback::batch::{RestorationScope, RestoreCandidate, restore_installed};
use crate::cargo_rollback::graph::PackageKey;
use crate::manifest::{is_crates_io_source, parse_dependency_ref, parse_lock_packages};
use std::collections::BTreeSet;

const LOCK_AGE_RESTORE_BUDGET: Duration = Duration::from_secs(60);

type LockId = (String, String, Option<String>);

/// Changes observed immediately around an audited Cargo operation. An unexplained
/// checkpoint change revokes ownership of that source/semver series permanently.
/// A single-crate Cargo command writes the live lock; simultaneous edits in its
/// dependency scope cannot be distinguished from Cargo's own changes. Checkpoints
/// preserve changes observed between commands, and scratch commits check originals.
/// Packages are compared by meaning ([`LockEntry`]), so a dependency reference that
/// only changes its spelling is not a change.
#[derive(Clone, Default)]
pub(super) struct AuditChanges {
    checkpoint: String,
    between_commands_changed: bool,
    owned: BTreeSet<LockId>,
    external: BTreeSet<LockId>,
}

fn lock_ids(content: &str) -> BTreeSet<LockId> {
    parse_lock_packages(content)
        .into_iter()
        .map(|package| (package.name, package.version, package.source))
        .collect()
}

/// A dependency edge resolved to the package it names. Cargo writes each reference in
/// its shortest unambiguous form (`"toml"`, `"toml 0.9.8"`, or with a source), so the
/// text of an unchanged edge changes when another version of that name enters or
/// leaves the lock. A reference that names no single package keeps its text, and an
/// element that is not a string keeps its TOML form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Edge {
    Package(LockId),
    Unresolved(String),
    Invalid(String),
}

/// A lock package compared by meaning: its other fields, and its dependency edges
/// as a multiset of the packages they name.
#[derive(Clone, Debug, PartialEq)]
struct LockEntry {
    fields: toml::Value,
    edges: BTreeMap<Edge, usize>,
}

impl LockEntry {
    /// Whether the package changed only because an operation moved its dependencies.
    /// Each removed edge must name a package the operation `moved`, paired with an
    /// added edge to the same name, source and semver series inside its `scope`.
    /// A net addition or removal, or any other field, is not explained this way.
    fn rewired(
        &self,
        after: &Self,
        moved: &BTreeSet<LockId>,
        scope: &BTreeSet<PackageKey>,
    ) -> bool {
        if self.fields != after.fields {
            return false;
        }
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let edges: BTreeSet<_> = self.edges.keys().chain(after.edges.keys()).collect();
        for edge in edges {
            let before = self.edges.get(edge).copied().unwrap_or(0);
            let now = after.edges.get(edge).copied().unwrap_or(0);
            match edge {
                Edge::Unresolved(_) | Edge::Invalid(_) if before != now => return false,
                Edge::Unresolved(_) | Edge::Invalid(_) => {}
                Edge::Package(id) => {
                    removed.extend(std::iter::repeat_n(id, before.saturating_sub(now)));
                    added.extend(std::iter::repeat_n(id, now.saturating_sub(before)));
                }
            }
        }
        removed.len() == added.len()
            && removed.into_iter().all(|old| {
                let pair = added.iter().position(|new| {
                    same_line(old, new) && scope.contains(&(new.0.clone(), new.1.clone()))
                });
                match pair {
                    Some(index) if moved.contains(old) => {
                        added.swap_remove(index);
                        true
                    }
                    _ => false,
                }
            })
    }
}

fn package_id(package: &toml::Value) -> Option<LockId> {
    Some((
        package.get("name")?.as_str()?.into(),
        package.get("version")?.as_str()?.into(),
        package
            .get("source")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
    ))
}

fn resolve_edge(ids: &[LockId], reference: &toml::Value) -> Edge {
    let Some(text) = reference.as_str() else {
        return Edge::Invalid(reference.to_string());
    };
    let Some(reference) = parse_dependency_ref(text) else {
        return Edge::Unresolved(text.into());
    };
    // References omit a git source's `#<commit>`.
    let mut matches = ids.iter().filter(|id| {
        id.0 == reference.name
            && reference
                .version
                .as_ref()
                .is_none_or(|version| id.1 == *version)
            && reference.source.as_deref().is_none_or(|source| {
                id.2.as_deref()
                    .is_some_and(|own| own.split('#').next() == Some(source))
            })
    });
    match (matches.next(), matches.next()) {
        (Some(id), None) => Edge::Package(id.clone()),
        _ => Edge::Unresolved(text.into()),
    }
}

fn lock_tables(content: &str) -> BTreeMap<LockId, LockEntry> {
    let document: toml::Value =
        toml::from_str(content).unwrap_or(toml::Value::Table(Default::default()));
    let packages: Vec<_> = document
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let ids: Vec<_> = packages
        .iter()
        .filter_map(|package| package_id(package))
        .collect();
    packages
        .into_iter()
        .filter_map(|package| {
            let id = package_id(package)?;
            let mut fields = package.clone();
            let mut edges = BTreeMap::new();
            if let Some(table) = fields.as_table_mut()
                && table.get("dependencies").is_some_and(toml::Value::is_array)
                && let Some(toml::Value::Array(references)) = table.remove("dependencies")
            {
                for reference in &references {
                    *edges.entry(resolve_edge(&ids, reference)).or_insert(0) += 1;
                }
            }
            Some((id, LockEntry { fields, edges }))
        })
        .collect()
}

fn same_line(left: &LockId, right: &LockId) -> bool {
    left.0 == right.0 && left.2 == right.2 && (left.1 == right.1 || same_series(&left.1, &right.1))
}

impl AuditChanges {
    fn disown(&mut self, changes: BTreeSet<LockId>) {
        self.owned
            .retain(|key| !changes.iter().any(|other| same_line(key, other)));
        self.external.extend(changes);
    }

    pub(super) fn observe(&mut self, content: &str) {
        if !self.checkpoint.is_empty() {
            self.between_commands_changed |= self.checkpoint != content;
            let before = lock_tables(&self.checkpoint);
            let after = lock_tables(content);
            self.disown(
                before
                    .keys()
                    .chain(after.keys())
                    .filter(|key| before.get(*key) != after.get(*key))
                    .cloned()
                    .collect(),
            );
        }
        self.checkpoint = content.to_string();
    }

    /// Whether the live lock holds only audited changes, so the whole snapshot can replace
    /// it. A registry or git package changed outside an audited operation's scope is kept,
    /// as selective restoration keeps it. Path packages follow the workspace manifests,
    /// which the snapshot checks separately.
    pub(super) fn allows_snapshot_restore(&self, content: Option<&[u8]>) -> bool {
        !self.between_commands_changed
            && self.external_packages().is_empty()
            && content == Some(self.checkpoint.as_bytes())
    }

    /// Registry and git packages changed outside an audited operation (`name version`).
    pub(super) fn external_packages(&self) -> Vec<String> {
        self.external
            .iter()
            .filter(|key| {
                key.2
                    .as_deref()
                    .is_some_and(|source| !source.starts_with("path+"))
            })
            .map(|key| format!("{} {}", key.0, key.1))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(super) fn applied(&mut self, before: &str, after: &str, selected: &[PackageKey]) {
        self.observe(before);
        let before_ids = lock_ids(before);
        let after_ids = lock_ids(after);
        let before_tables = lock_tables(before);
        let after_tables = lock_tables(after);
        let changed: BTreeSet<_> = before_tables
            .keys()
            .chain(after_tables.keys())
            .filter(|key| before_tables.get(*key) != after_tables.get(*key))
            .cloned()
            .collect();
        let graphs = [
            LockGraph::from_lock_content(before),
            LockGraph::from_lock_content(after),
        ];
        let mut pending: Vec<_> = before_ids
            .union(&after_ids)
            .filter(|key| {
                selected
                    .iter()
                    .any(|selected| selected.0 == key.0 && same_series(&selected.1, &key.1))
            })
            .map(|key| (key.0.clone(), key.1.clone()))
            .collect();
        let mut related = BTreeSet::new();
        // Reachability and paired source/series endpoints form one fixed point.
        // An endpoint discovered in either graph can expose further old or new dependencies.
        while let Some(key) = pending.pop() {
            if !related.insert(key.clone()) {
                continue;
            }
            for graph in &graphs {
                pending.extend(graph.dependencies(&key));
            }
            for endpoint in changed
                .iter()
                .filter(|endpoint| endpoint.0 == key.0 && endpoint.1 == key.1)
            {
                pending.extend(
                    changed
                        .iter()
                        .filter(|other| same_line(endpoint, other))
                        .map(|other| (other.0.clone(), other.1.clone())),
                );
            }
        }
        let ours: BTreeSet<_> = changed
            .iter()
            .filter(|key| related.contains(&(key.0.clone(), key.1.clone())))
            .cloned()
            .collect();
        // Moving a package also rewrites the edges of its dependents outside the scope.
        // Such a dependent did not change itself, so it is neither ours nor external,
        // and what earlier operations recorded about it stays as it was.
        let rewired = |key: &LockId| {
            before_tables
                .get(key)
                .zip(after_tables.get(key))
                .is_some_and(|(before, after)| before.rewired(after, &ours, &related))
        };
        self.disown(
            changed
                .iter()
                .filter(|key| !ours.contains(*key) && !rewired(key))
                .cloned()
                .collect(),
        );
        for key in ours {
            if !self.external.iter().any(|other| same_line(&key, other)) {
                self.owned.insert(key);
            }
        }
        self.checkpoint = after.to_string();
    }

    pub(super) fn owns(&self, name: &str, version: &str) -> bool {
        self.owned.iter().any(|key| {
            key.0 == name && key.1 == version && key.2.as_deref().is_some_and(is_crates_io_source)
        })
    }

    pub(super) fn externally_changed(&self, key: &PackageKey) -> bool {
        self.external.iter().any(|other| {
            other.0 == key.0
                && same_series(&other.1, &key.1)
                && other.2.as_deref().is_some_and(is_crates_io_source)
        })
    }
}

struct RestorePlan {
    members: Vec<RestoreCandidate>,
    discarded: Vec<PackageKey>,
    anchors: Vec<String>,
    problem: Option<String>,
}

struct Node {
    installed: PackageKey,
    current: Option<PackageKey>,
    moved: bool,
}

fn keys(entries: &RegistryLockEntries) -> BTreeSet<PackageKey> {
    entries
        .iter()
        .flat_map(|(name, versions)| {
            versions
                .iter()
                .map(move |version| (name.clone(), version.clone()))
        })
        .collect()
}

fn restore_plans(
    current_lock: &str,
    state: &AuditState,
    baseline: &RegistryLockEntries,
) -> Result<Vec<RestorePlan>, String> {
    let mut changes = state.changes.clone();
    changes.observe(current_lock);
    let installed = keys(&parse_registry_entries(&state.installed_lock));
    let current = keys(&parse_registry_entries(current_lock));
    let baseline = keys(baseline);
    let installed_packages = parse_lock_packages(&state.installed_lock);
    let current_packages = parse_lock_packages(current_lock);
    let installed_tables = lock_tables(&state.installed_lock);
    let current_tables = lock_tables(current_lock);
    let original_graph = LockGraph::from_packages(&installed_packages);
    let current_graph = LockGraph::from_packages(&current_packages);
    let mut nodes: BTreeMap<PackageKey, Node> = BTreeMap::new();
    for adjustment in state
        .log
        .entries
        .values()
        .filter(|adjustment| adjustment.status.is_resolved() || adjustment.status.is_restored())
    {
        let from = (adjustment.name.clone(), adjustment.from.clone());
        let now = adjustment
            .to
            .as_ref()
            .map(|version| (adjustment.name.clone(), version.clone()));
        if now.as_ref().is_some_and(|key| !current.contains(key)) {
            continue;
        }
        if installed.contains(&from) && current.contains(&from) {
            // It has already returned to its original registry package ID.
            continue;
        }
        if now.is_none()
            && current.iter().any(|(name, version)| {
                name == &adjustment.name && same_series(version, &adjustment.from)
            })
        {
            continue;
        }
        let candidates: Vec<_> = installed
            .iter()
            .filter(|(name, version)| {
                name == &adjustment.name && same_series(version, &adjustment.from)
            })
            .collect();
        let original = if installed.contains(&from) {
            from
        } else {
            match candidates.as_slice() {
                [key] => (*key).clone(),
                [] => continue,
                _ => {
                    return Err(format!(
                        "ambiguous install-time version for {}",
                        adjustment.name
                    ));
                }
            }
        };
        if !changes.owns(&original.0, &original.1)
            || now
                .as_ref()
                .is_some_and(|key| !changes.owns(&key.0, &key.1))
        {
            continue;
        }
        nodes.insert(
            original.clone(),
            Node {
                installed: original,
                current: now,
                moved: true,
            },
        );
    }
    if nodes.is_empty() {
        return Ok(Vec::new());
    }
    for anchor in current.intersection(&installed) {
        if !baseline.contains(anchor) && !state.admitted.contains(anchor) {
            nodes.entry(anchor.clone()).or_insert_with(|| Node {
                installed: anchor.clone(),
                current: Some(anchor.clone()),
                moved: false,
            });
        }
    }
    let nodes: Vec<_> = nodes.into_values().collect();
    let original_reach: Vec<_> = nodes
        .iter()
        .map(|node| original_graph.reachable(&node.installed))
        .collect();
    let current_reach: Vec<_> = nodes
        .iter()
        .map(|node| {
            node.current
                .as_ref()
                .map(|key| current_graph.reachable(key))
                .unwrap_or_default()
        })
        .collect();
    let mut visited = HashSet::new();
    let mut plans = Vec::new();
    for start in 0..nodes.len() {
        if !visited.insert(start) {
            continue;
        }
        let mut component = vec![start];
        let mut pending = vec![start];
        while let Some(index) = pending.pop() {
            for other in 0..nodes.len() {
                let related = original_reach[index].contains(&nodes[other].installed)
                    || original_reach[other].contains(&nodes[index].installed)
                    || nodes[other]
                        .current
                        .as_ref()
                        .is_some_and(|key| current_reach[index].contains(key))
                    || nodes[index]
                        .current
                        .as_ref()
                        .is_some_and(|key| current_reach[other].contains(key));
                if related && visited.insert(other) {
                    component.push(other);
                    pending.push(other);
                }
            }
        }
        let anchors: BTreeSet<_> = component
            .iter()
            .filter(|index| !nodes[**index].moved)
            .map(|index| nodes[*index].installed.0.clone())
            .collect();
        if anchors.is_empty() || !component.iter().any(|index| nodes[*index].moved) {
            continue;
        }
        let result = (|| -> Result<_, String> {
            // LockGraph merges equal name/version IDs. A duplicate source on a connecting
            // path makes that relationship uncertain, so do not infer a restoration scope.
            for (graph, packages, selected) in [
                (
                    &original_graph,
                    &installed_packages,
                    component
                        .iter()
                        .map(|index| nodes[*index].installed.clone())
                        .collect::<BTreeSet<_>>(),
                ),
                (
                    &current_graph,
                    &current_packages,
                    component
                        .iter()
                        .filter_map(|index| nodes[*index].current.clone())
                        .collect(),
                ),
            ] {
                let mut counts = BTreeMap::new();
                for package in packages {
                    *counts
                        .entry((package.name.clone(), package.version.clone()))
                        .or_insert(0) += 1;
                }
                for (key, count) in counts {
                    if count > 1
                        && (selected.contains(&key)
                            || (selected
                                .iter()
                                .any(|start| graph.reachable(start).contains(&key))
                                && graph
                                    .reachable(&key)
                                    .iter()
                                    .any(|end| selected.contains(end))))
                    {
                        return Err(format!(
                            "ambiguous source in partial rollback relation: {} {}",
                            key.0, key.1
                        ));
                    }
                }
            }
            let mut members = BTreeMap::new();
            for index in component.iter().filter(|index| nodes[**index].moved) {
                let node = &nodes[*index];
                members.insert(
                    node.installed.clone(),
                    RestoreCandidate {
                        name: node.installed.0.clone(),
                        current: node.current.as_ref().map(|key| key.1.clone()),
                        installed: node.installed.1.clone(),
                    },
                );
                // Reintroduced dependencies are pinned to I as well, never to a third version.
                for key in original_graph
                    .reachable(&node.installed)
                    .intersection(&installed)
                {
                    if changes.externally_changed(key) {
                        return Err(format!(
                            "external change to required dependency {} prevents restoration",
                            key.0
                        ));
                    }
                    let versions: Vec<_> = current
                        .iter()
                        .filter(|(name, version)| name == &key.0 && same_series(version, &key.1))
                        .collect();
                    let changed_table = installed_tables.iter().any(|(id, table)| {
                        id.0 == key.0
                            && id.1 == key.1
                            && id.2.as_deref().is_some_and(is_crates_io_source)
                            && current_tables.get(id) != Some(table)
                    });
                    if changes.owns(&key.0, &key.1) && (!current.contains(key) || changed_table) {
                        let now = match versions.as_slice() {
                            [] => None,
                            [now] if changes.owns(&now.0, &now.1) => Some(now.1.clone()),
                            [_] => continue,
                            _ => {
                                return Err(format!(
                                    "ambiguous current version for restoring {}",
                                    key.0
                                ));
                            }
                        };
                        members
                            .entry(key.clone())
                            .or_insert_with(|| RestoreCandidate {
                                name: key.0.clone(),
                                current: now,
                                installed: key.1.clone(),
                            });
                    }
                }
            }
            for index in component.iter().filter(|index| nodes[**index].moved) {
                for key in &current_reach[*index] {
                    if !installed.contains(key) && changes.externally_changed(key) {
                        return Err(format!(
                            "external change to dependency {} prevents removing it during restoration",
                            key.0
                        ));
                    }
                }
            }
            let discarded: Vec<_> = component
                .iter()
                .filter(|index| nodes[**index].moved)
                .flat_map(|index| {
                    nodes[*index]
                        .current
                        .as_ref()
                        .map(|key| current_graph.reachable(key))
                        .unwrap_or_default()
                })
                .filter(|key| !installed.contains(key) && changes.owns(&key.0, &key.1))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let members: Vec<_> = members.into_values().collect();
            for member in &members {
                for (packages, version) in [
                    (&installed_packages, Some(&member.installed)),
                    (&current_packages, member.current.as_ref()),
                ] {
                    let Some(version) = version else {
                        continue;
                    };
                    let matches: Vec<_> = packages
                        .iter()
                        .filter(|package| {
                            package.name == member.name && package.version == *version
                        })
                        .collect();
                    if matches.len() != 1
                        || !matches[0]
                            .source
                            .as_deref()
                            .is_some_and(is_crates_io_source)
                    {
                        return Err(format!(
                            "ambiguous source for restoring {} {version}",
                            member.name
                        ));
                    }
                }
            }
            Ok((members, discarded))
        })();
        let (members, discarded, problem) = match result {
            Ok((members, discarded)) => (members, discarded, None),
            Err(problem) => (Vec::new(), Vec::new(), Some(problem)),
        };
        plans.push(RestorePlan {
            members,
            discarded,
            anchors: anchors.into_iter().collect(),
            problem,
        });
    }
    Ok(plans)
}

impl LockAgeAuditor<'_> {
    pub(super) async fn restore_mixed(&self, state: &mut AuditState) {
        let deadline = Instant::now() + LOCK_AGE_RESTORE_BUDGET;
        let (content, current) = match read_audit_lock(&self.input.project_dir.join("Cargo.lock")) {
            Ok(lock) => lock,
            Err(error) => {
                state.problems.push(error);
                return;
            }
        };
        // Later audited commands can move an earlier explicit rollback again.
        state.log.reconcile_with_lock(&current);
        // Cached release metadata may already establish that an unvisited I version is safe.
        for (name, versions) in &current {
            if let Some(cached) = self.versions.cached(self.input.adapter, name).await {
                for version in versions {
                    if cached
                        .iter()
                        .find(|info| info.version == *version)
                        .is_some_and(|info| {
                            self.input.exemptions.admits(
                                Language::Rust,
                                name,
                                info,
                                self.input.cutoff,
                            )
                        })
                    {
                        state.admitted.insert((name.clone(), version.clone()));
                    }
                }
            }
        }
        let plans = match restore_plans(&content, state, self.input.baseline) {
            Ok(plans) => plans,
            Err(error) => {
                state.problems.push(format!(
                    "could not safely restore partial age rollbacks: {error}"
                ));
                return;
            }
        };
        let mut expected_lock = content;
        for plan in plans {
            let reason = format!("{} could not be rolled back", plan.anchors.join(", "));
            if let Some(problem) = plan.problem {
                state.problems.push(format!(
                    "could not safely restore partial age rollbacks related to {}: {problem}",
                    plan.anchors.join(", ")
                ));
                continue;
            }
            let scope = RestorationScope {
                members: &plan.members,
                discarded: &plan.discarded,
                installed: &state.installed_lock,
                expected_lock: &expected_lock,
            };
            match restore_installed(self.input.project_dir, scope, self.input.cargo, deadline, CARGO_UPDATE_TIMEOUT).await {
                Ok(committed) => {
                    let selected: Vec<_> = plan.members.iter().map(|member| {
                        (member.name.clone(), member.installed.clone())
                    }).collect();
                    state.changes.applied(&expected_lock, &committed, &selected);
                    expected_lock = committed;
                    for member in &plan.members {
                        let key = (member.name.clone(), member.installed.clone());
                        if let Some(adjustment) = state.log.entries.get_mut(&key) {
                            adjustment.to = None;
                            adjustment.status = LockAgeStatus::Reverted(reason.clone());
                        } else {
                            state.log.record(&member.name, &member.installed, None, LockAgeStatus::Reverted(reason.clone()));
                        }
                    }
                }
                Err(error) => state.problems.push(format!("Cargo.lock mixes partial age rollbacks with unresolved {}; restoring the install-time combination failed: {error}", plan.anchors.join(", "))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: &str = "registry+https://github.com/rust-lang/crates.io-index";
    const PRIVATE: &str = "registry+https://registry.example.com/index";

    fn lock(packages: &[(&str, &str, &str, &[&str])]) -> String {
        let mut content = String::from("version = 4\n");
        for (name, version, source, dependencies) in packages {
            content.push_str(&format!("\n[[package]]\nname = {name:?}\nversion = {version:?}\nsource = {source:?}\ndependencies = {dependencies:?}\n"));
        }
        content
    }

    fn partial(installed: String, current: &str) -> AuditState {
        let mut state = AuditState::new(Duration::ZERO);
        state.changes.applied(
            &installed,
            current,
            &[
                ("z-top".into(), "1.2.0".into()),
                ("s-solo".into(), "1.2.0".into()),
            ],
        );
        state.installed_lock = installed;
        state.log.record(
            "z-top",
            "1.2.0",
            Some("1.1.0".into()),
            LockAgeStatus::Downgraded,
        );
        state.log.record(
            "s-solo",
            "1.2.0",
            Some("1.1.0".into()),
            LockAgeStatus::Downgraded,
        );
        state
    }

    fn mixed_locks() -> (String, String) {
        let installed = lock(&[
            (
                "root",
                "0.1.0",
                "path+file:///example",
                &["z-top", "s-solo"],
            ),
            ("z-top", "1.2.0", PUBLIC, &["middle"]),
            ("middle", "1.0.0", PUBLIC, &["a-dep"]),
            ("a-dep", "1.2.0", PUBLIC, &[]),
            ("s-solo", "1.2.0", PUBLIC, &[]),
        ]);
        let current = installed
            .replace(
                "name = \"z-top\"\nversion = \"1.2.0\"",
                "name = \"z-top\"\nversion = \"1.1.0\"",
            )
            .replace(
                "name = \"s-solo\"\nversion = \"1.2.0\"",
                "name = \"s-solo\"\nversion = \"1.1.0\"",
            );
        (installed, current)
    }

    #[test]
    fn checkpoint_changes_revoke_automatic_ownership_and_cannot_be_reclaimed() {
        let installed = lock(&[
            ("middle", "1.2.0", PUBLIC, &[]),
            ("solo", "1.2.0", PUBLIC, &[]),
        ]);
        let current = lock(&[
            ("middle", "1.1.0", PUBLIC, &[]),
            ("solo", "1.1.0", PUBLIC, &[]),
        ]);
        let external = lock(&[
            ("middle", "1.0.0", PUBLIC, &[]),
            ("solo", "1.1.0", PUBLIC, &[]),
        ]);
        let mut changes = AuditChanges::default();
        changes.applied(
            &installed,
            &current,
            &[
                ("middle".into(), "1.2.0".into()),
                ("solo".into(), "1.2.0".into()),
            ],
        );
        assert!(changes.owns("middle", "1.2.0"));
        changes.observe(&external);
        assert!(!changes.owns("middle", "1.2.0"));
        assert!(!changes.owns("middle", "1.1.0"));
        assert!(changes.owns("solo", "1.2.0"));
        changes.applied(&external, &current, &[("middle".into(), "1.0.0".into())]);
        assert!(!changes.owns("middle", "1.1.0"));
        assert!(changes.owns("solo", "1.1.0"));
    }

    #[test]
    fn checkpoint_checks_dependency_edges_and_checksums_without_a_version_change() {
        let installed = lock(&[("middle", "1.2.0", PUBLIC, &[])]);
        let current = lock(&[("middle", "1.1.0", PUBLIC, &[])]);
        for external in [
            format!("{current}checksum = 'changed'\n"),
            current.replace("dependencies = []", "dependencies = ['another']"),
        ] {
            let mut changes = AuditChanges::default();
            changes.applied(&installed, &current, &[("middle".into(), "1.2.0".into())]);
            assert!(changes.owns("middle", "1.1.0"));
            changes.observe(&external);
            assert!(!changes.owns("middle", "1.1.0"));
            assert!(!changes.owns("middle", "1.2.0"));
        }
    }

    #[test]
    fn review_new_edges_keep_both_ends_of_an_automatic_move_owned() {
        let installed = lock(&[
            ("a-middle", "1.3.0", PUBLIC, &[]),
            ("b-top", "1.2.0", PUBLIC, &[]),
            ("a-middle", "2.0.0", PUBLIC, &[]),
            ("a-middle", "1.5.0", PRIVATE, &[]),
        ]);
        let first = installed.replace(
            "name = \"a-middle\"\nversion = \"1.3.0\"",
            "name = \"a-middle\"\nversion = \"1.2.0\"",
        );
        let last = lock(&[
            ("a-middle", "1.1.0", PUBLIC, &[]),
            ("b-top", "1.1.0", PUBLIC, &["a-middle 1.1.0"]),
        ]);
        let mut changes = AuditChanges::default();
        changes.applied(&installed, &first, &[("a-middle".into(), "1.3.0".into())]);
        changes.applied(&first, &last, &[("b-top".into(), "1.2.0".into())]);
        assert!(changes.owns("a-middle", "1.3.0"));
        assert!(changes.owns("a-middle", "1.2.0"));
        assert!(changes.owns("a-middle", "1.1.0"));
        assert!(!changes.owns("a-middle", "2.0.0"));
        assert!(changes.external.iter().any(|key| key.0 == "a-middle"
            && key.1 == "1.5.0"
            && key.2.as_deref() == Some(PRIVATE)));
    }

    #[test]
    fn review_dependency_closures_and_paired_moves_expand_until_stable() {
        let before = lock(&[
            ("top", "1.2.0", PUBLIC, &[]),
            ("middle", "1.2.0", PUBLIC, &["support 1.2.0"]),
            ("support", "1.2.0", PUBLIC, &[]),
            ("leaf", "1.2.0", PUBLIC, &["tail"]),
            ("tail", "1.0.0", PUBLIC, &[]),
            ("support", "2.0.0", PUBLIC, &[]),
            ("support", "1.5.0", PRIVATE, &[]),
        ]);
        let after = lock(&[
            ("top", "1.1.0", PUBLIC, &["middle"]),
            ("middle", "1.1.0", PUBLIC, &[]),
            ("support", "1.1.0", PUBLIC, &["leaf"]),
            ("leaf", "1.1.0", PUBLIC, &[]),
        ]);
        let mut changes = AuditChanges::default();
        changes.applied(&before, &after, &[("top".into(), "1.2.0".into())]);
        for (name, versions) in [
            ("middle", &["1.1.0", "1.2.0"][..]),
            ("support", &["1.1.0", "1.2.0"][..]),
            ("leaf", &["1.1.0", "1.2.0"][..]),
            ("tail", &["1.0.0"][..]),
        ] {
            for version in versions {
                assert!(changes.owns(name, version), "{name} {version}");
            }
        }
        assert!(!changes.owns("support", "2.0.0"));
        assert!(changes.external.iter().any(|key| key.0 == "support"
            && key.1 == "1.5.0"
            && key.2.as_deref() == Some(PRIVATE)));
    }

    #[test]
    fn rejected_components_do_not_prevent_other_restoration_plans() {
        for ambiguous in [false, true] {
            let mut installed = lock(&[
                ("z-top", "1.2.0", PUBLIC, &["middle"]),
                ("middle", "1.2.0", PUBLIC, &["a-anchor"]),
                ("a-anchor", "1.2.0", PUBLIC, &[]),
                ("s-solo", "1.2.0", PUBLIC, &["b-anchor"]),
                ("b-anchor", "1.2.0", PUBLIC, &[]),
            ]);
            let mut current = installed
                .replace(
                    "name = \"z-top\"\nversion = \"1.2.0\"",
                    "name = \"z-top\"\nversion = \"1.1.0\"",
                )
                .replace(
                    "name = \"middle\"\nversion = \"1.2.0\"",
                    "name = \"middle\"\nversion = \"1.1.0\"",
                )
                .replace(
                    "name = \"s-solo\"\nversion = \"1.2.0\"",
                    "name = \"s-solo\"\nversion = \"1.1.0\"",
                );
            if ambiguous {
                installed.push_str(
                    lock(&[("middle", "1.2.0", PRIVATE, &[])])
                        .strip_prefix("version = 4\n")
                        .unwrap(),
                );
                current.push_str(
                    lock(&[("middle", "1.1.0", PRIVATE, &[])])
                        .strip_prefix("version = 4\n")
                        .unwrap(),
                );
            }
            let mut state = partial(installed, &current);
            if !ambiguous {
                current = current.replace(
                    "name = \"middle\"\nversion = \"1.1.0\"",
                    "name = \"middle\"\nversion = \"1.1.0\"\nchecksum = 'external'",
                );
            }
            state.admitted.insert(("middle".into(), "1.2.0".into()));
            let plans = restore_plans(&current, &state, &RegistryLockEntries::new()).unwrap();
            assert_eq!(plans.len(), 2);
            assert_eq!(
                plans.iter().filter(|plan| plan.problem.is_some()).count(),
                1
            );
            let good = plans.iter().find(|plan| plan.problem.is_none()).unwrap();
            assert_eq!(good.members.len(), 1);
            assert_eq!(good.members[0].name, "s-solo");
        }
    }

    #[test]
    fn relation_crosses_admitted_intermediates_without_joining_root_siblings() {
        let (installed, current) = mixed_locks();
        let mut state = partial(installed, &current);
        state.admitted.insert(("middle".into(), "1.0.0".into()));
        let plans = restore_plans(&current, &state, &RegistryLockEntries::new()).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].anchors, ["a-dep"]);
        assert_eq!(plans[0].members.len(), 1);
        assert_eq!(plans[0].members[0].name, "z-top");
    }

    #[test]
    fn baseline_and_admitted_versions_do_not_anchor_restoration() {
        let (installed, current) = mixed_locks();
        let mut state = partial(installed, &current);
        state.admitted.insert(("middle".into(), "1.0.0".into()));
        state.admitted.insert(("a-dep".into(), "1.2.0".into()));
        assert!(
            restore_plans(&current, &state, &RegistryLockEntries::new())
                .unwrap()
                .is_empty()
        );
        state.admitted.remove(&("a-dep".into(), "1.2.0".into()));
        let baseline = parse_registry_entries(&lock(&[("a-dep", "1.2.0", PUBLIC, &[])]));
        assert!(
            restore_plans(&current, &state, &baseline)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn same_version_from_a_private_source_is_not_an_already_restored_registry_id() {
        let installed = lock(&[
            ("z-top", "1.2.0", PUBLIC, &["a-dep"]),
            ("z-top", "1.1.0", PRIVATE, &[]),
            ("a-dep", "1.2.0", PUBLIC, &[]),
        ]);
        let current = lock(&[
            ("z-top", "1.1.0", PUBLIC, &["a-dep"]),
            ("z-top", "1.1.0", PRIVATE, &[]),
            ("a-dep", "1.2.0", PUBLIC, &[]),
        ]);
        let error = restore_plans(
            &current,
            &partial(installed, &current),
            &RegistryLockEntries::new(),
        )
        .unwrap()
        .remove(0)
        .problem
        .unwrap();
        assert!(error.contains("ambiguous source"), "{error}");
    }

    #[test]
    fn ambiguous_intermediate_sources_do_not_define_a_restoration_scope() {
        let (mut installed, mut current) = mixed_locks();
        let duplicate = lock(&[("middle", "1.0.0", PRIVATE, &[])])
            .strip_prefix("version = 4\n")
            .unwrap()
            .to_string();
        installed.push_str(&duplicate);
        current.push_str(&duplicate);
        let mut state = partial(installed, &current);
        state.admitted.insert(("middle".into(), "1.0.0".into()));
        let error = restore_plans(&current, &state, &RegistryLockEntries::new())
            .unwrap()
            .remove(0)
            .problem
            .unwrap();
        assert!(error.contains("ambiguous source"), "{error}");
    }

    /// The shape of a Tauri lock: `toml` has two series, so Cargo writes `cargo_toml`'s
    /// dependency with a version, and moving `toml` rewrites that reference.
    fn toml_family(version: &str, cargo_toml: &[&str]) -> String {
        lock(&[
            ("cargo_toml", "0.22.0", PUBLIC, cargo_toml),
            ("serde", "1.0.0", PUBLIC, &[]),
            ("system-deps", "7.0.0", PUBLIC, &["toml 0.8.2"]),
            ("toml", "0.8.2", PUBLIC, &[]),
            ("toml", version, PUBLIC, &["serde"]),
        ])
    }

    #[test]
    fn dependents_rewired_by_an_operation_are_neither_ours_nor_external() {
        let installed = toml_family("0.9.8", &["toml 0.9.8"]);
        let current = toml_family("0.9.5", &["toml 0.9.5"]);
        let selected = [("toml".to_string(), "0.9.8".to_string())];
        let cargo_toml = ("cargo_toml".to_string(), "0.22.0".to_string());
        let mut changes = AuditChanges::default();
        changes.applied(&installed, &current, &selected);
        assert!(!changes.externally_changed(&cargo_toml));
        assert!(!changes.owns("cargo_toml", "0.22.0"));
        assert!(changes.owns("toml", "0.9.8") && changes.owns("toml", "0.9.5"));
        assert!(changes.allows_snapshot_restore(Some(current.as_bytes())));

        // What earlier operations recorded about the dependent stays as it was.
        let id = (
            cargo_toml.0.clone(),
            cargo_toml.1.clone(),
            Some(PUBLIC.to_string()),
        );
        let mut earlier = AuditChanges::default();
        earlier.owned.insert(id.clone());
        earlier.applied(&installed, &current, &selected);
        assert!(earlier.owns("cargo_toml", "0.22.0"));
        let mut earlier = AuditChanges::default();
        earlier.external.insert(id);
        earlier.applied(&installed, &current, &selected);
        assert!(earlier.externally_changed(&cargo_toml));

        // The same rewrite between audited operations is an external change.
        let mut between = AuditChanges::default();
        between.observe(&installed);
        between.observe(&current);
        assert!(between.externally_changed(&cargo_toml));
    }

    #[test]
    fn reference_spelling_is_not_a_change() {
        let key = |source: &str| {
            (
                "cargo_toml".to_string(),
                "0.22.0".to_string(),
                Some(source.to_string()),
            )
        };
        // A second `toml` series makes Cargo add a version to the same edge.
        let single = lock(&[
            ("cargo_toml", "0.22.0", PUBLIC, &["toml"]),
            ("serde", "1.0.0", PUBLIC, &[]),
            ("toml", "0.9.8", PUBLIC, &["serde"]),
        ]);
        let duplicated = toml_family("0.9.8", &["toml 0.9.8"]);
        assert_eq!(
            lock_tables(&single)[&key(PUBLIC)],
            lock_tables(&duplicated)[&key(PUBLIC)]
        );
        let mut changes = AuditChanges::default();
        changes.observe(&single);
        changes.observe(&duplicated);
        assert!(!changes.externally_changed(&("cargo_toml".into(), "0.22.0".into())));

        // A source suffix names one of two sources; a git reference omits the commit.
        let git = "git+https://example.com/toml?branch=main#0123abcd";
        let sourced = lock(&[
            (
                "cargo_toml",
                "0.22.0",
                PUBLIC,
                &[
                    "toml 0.9.8 (registry+https://github.com/rust-lang/crates.io-index)",
                    "toml 0.9.8 (git+https://example.com/toml?branch=main)",
                ],
            ),
            ("toml", "0.9.8", PUBLIC, &[]),
            ("toml", "0.9.8", git, &[]),
        ]);
        let edges = &lock_tables(&sourced)[&key(PUBLIC)].edges;
        for source in [PUBLIC, git] {
            let edge = Edge::Package(("toml".into(), "0.9.8".into(), Some(source.into())));
            assert_eq!(edges.get(&edge), Some(&1), "{edges:?}");
        }
    }

    #[test]
    fn unexplained_dependent_changes_stay_external() {
        let installed = toml_family("0.9.8", &["toml 0.9.8", "ghost 1.0.0"]);
        let moved_toml = ("toml".to_string(), "0.9.8".to_string());
        let moved_serde = ("serde".to_string(), "1.0.0".to_string());
        let retargeted = toml_family("0.9.5", &["toml 0.9.5", "ghost 1.0.0"]);
        for (after, selected) in [
            // To another series.
            (
                toml_family("0.9.5", &["toml 0.8.2", "ghost 1.0.0"]),
                &moved_toml,
            ),
            // With an edge added to an unchanged package in scope.
            (
                toml_family("0.9.5", &["toml 0.9.5", "serde", "ghost 1.0.0"]),
                &moved_toml,
            ),
            // With a reference that names no package changed.
            (
                toml_family("0.9.5", &["toml 0.9.5", "ghost 2.0.0"]),
                &moved_toml,
            ),
            // With another field changed.
            (
                retargeted.replace(
                    "dependencies = [\"toml 0.9.5\"",
                    "checksum = \"changed\"\ndependencies = [\"toml 0.9.5\"",
                ),
                &moved_toml,
            ),
            // By an operation that did not move the old target.
            (retargeted.clone(), &moved_serde),
        ] {
            let mut changes = AuditChanges::default();
            changes.applied(&installed, &after, std::slice::from_ref(selected));
            assert!(
                changes.externally_changed(&("cargo_toml".into(), "0.22.0".into())),
                "{after}"
            );
            assert!(
                changes
                    .external_packages()
                    .contains(&"cargo_toml 0.22.0".to_string())
            );
            assert!(!changes.allows_snapshot_restore(Some(after.as_bytes())));
        }
    }

    #[test]
    fn rewiring_pairs_every_moved_edge_with_one_in_the_same_series() {
        let id = |name: &str, version: &str| {
            (
                name.to_string(),
                version.to_string(),
                Some(PUBLIC.to_string()),
            )
        };
        let entry = |edges: &[(&str, &str, usize)]| LockEntry {
            fields: toml::Value::Table(Default::default()),
            edges: edges
                .iter()
                .map(|(name, version, count)| (Edge::Package(id(name, version)), *count))
                .collect(),
        };
        let moved: BTreeSet<_> = [
            id("toml", "0.9.8"),
            id("toml", "0.8.2"),
            id("syn", "2.0.10"),
        ]
        .into_iter()
        .collect();
        let scope: BTreeSet<_> = [("toml", "0.9.5"), ("toml", "0.8.1"), ("syn", "2.0.9")]
            .into_iter()
            .map(|(name, version)| (name.to_string(), version.to_string()))
            .collect();
        let before = entry(&[
            ("toml", "0.9.8", 2),
            ("toml", "0.8.2", 1),
            ("syn", "2.0.10", 1),
        ]);
        for (after, explained) in [
            (
                entry(&[
                    ("toml", "0.9.5", 2),
                    ("toml", "0.8.1", 1),
                    ("syn", "2.0.9", 1),
                ]),
                true,
            ),
            // One of two identical edges is left without a partner.
            (
                entry(&[
                    ("toml", "0.9.5", 1),
                    ("toml", "0.8.1", 1),
                    ("syn", "2.0.9", 1),
                ]),
                false,
            ),
            // An edge is added or removed besides the moves.
            (
                entry(&[
                    ("toml", "0.9.5", 3),
                    ("toml", "0.8.1", 1),
                    ("syn", "2.0.9", 1),
                ]),
                false,
            ),
            (entry(&[("toml", "0.9.5", 2), ("toml", "0.8.1", 1)]), false),
            // The 0.8 edge cannot pair with a version from the 0.9 series.
            (entry(&[("toml", "0.9.5", 3), ("syn", "2.0.9", 1)]), false),
        ] {
            assert_eq!(
                before.rewired(&after, &moved, &scope),
                explained,
                "{after:?}"
            );
        }
        let after = entry(&[
            ("toml", "0.9.5", 2),
            ("toml", "0.8.1", 1),
            ("syn", "2.0.9", 1),
        ]);
        assert!(!before.rewired(&after, &BTreeSet::new(), &scope));
        assert!(!before.rewired(&after, &moved, &BTreeSet::new()));
    }

    #[test]
    fn malformed_dependency_lists_are_compared_as_written() {
        let entry = |dependencies: &str| {
            let content = format!(
                "version = 4\n\n[[package]]\nname = \"example\"\nversion = \"0.1.0\"\nsource = \"{PUBLIC}\"\ndependencies = {dependencies}\n"
            );
            lock_tables(&content)
                .remove(&("example".into(), "0.1.0".into(), Some(PUBLIC.into())))
                .unwrap()
        };
        // A string and a non-string element never collapse into one edge.
        assert_ne!(entry("[\"true\"]"), entry("[true]"));
        // A list that is not an array stays in the compared fields.
        assert_ne!(entry("\"toml\""), entry("\"serde\""));
        assert_eq!(entry("\"toml\""), entry("\"toml\""));
    }

    #[test]
    fn path_packages_alone_do_not_prevent_snapshot_restoration() {
        // Path packages have no source; the snapshot checks their manifests instead.
        let before = "version = 4\n\n[[package]]\nname = \"example\"\nversion = \"0.1.0\"\n";
        let after = format!("{before}\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n");
        let mut changes = AuditChanges::default();
        changes.applied(before, &after, &[]);
        assert!(changes.external.iter().any(|key| key.0 == "member"));
        assert!(changes.external_packages().is_empty());
        assert!(changes.allows_snapshot_restore(Some(after.as_bytes())));
    }

    #[test]
    fn absent_dependencies_are_reintroduced_at_their_install_time_versions() {
        let installed = lock(&[
            ("z-top", "1.2.0", PUBLIC, &["middle", "a-dep"]),
            ("middle", "1.0.0", PUBLIC, &[]),
            ("a-dep", "1.2.0", PUBLIC, &[]),
        ]);
        let current = lock(&[("a-dep", "1.2.0", PUBLIC, &[])]);
        let mut state = AuditState::new(Duration::ZERO);
        state
            .changes
            .applied(&installed, &current, &[("z-top".into(), "1.2.0".into())]);
        state.installed_lock = installed;
        state
            .log
            .record("z-top", "1.2.0", None, LockAgeStatus::Removed);
        let plans = restore_plans(&current, &state, &RegistryLockEntries::new()).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0]
                .members
                .iter()
                .map(|member| (&*member.name, &*member.installed))
                .collect::<Vec<_>>(),
            [("middle", "1.0.0"), ("z-top", "1.2.0")]
        );
        assert!(
            plans[0]
                .members
                .iter()
                .all(|member| member.current.is_none())
        );
    }
}
