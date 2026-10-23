//! install 時点の未解消の版と関係が残る、監査の部分的な変更だけを取り消す。

use super::*;
use crate::cargo_rollback::batch::{RestorationScope, RestoreCandidate, restore_installed};
use crate::cargo_rollback::graph::PackageKey;
use crate::manifest::{is_crates_io_source, parse_dependency_ref, parse_lock_packages};
use std::collections::BTreeSet;

const LOCK_AGE_RESTORE_BUDGET: Duration = Duration::from_secs(60);

type LockId = (String, String, Option<String>);

/// 監査した Cargo 操作の直前と直後で観測した変更。説明できない
/// チェックポイントの変化があれば、その取得元と semver 系列を監査の所有対象から永久に外す。
/// 単一crateの Cargo コマンドは実際の lock に書く。その依存範囲内の同時編集は
/// Cargo 自身の変更と区別できない。チェックポイントではコマンド間に観測した変更を
/// 保持し、一時 workspace からの反映では元ファイルを確認する。
/// パッケージは意味で比較する（[`LockEntry`]）ため、依存参照の
/// 表記だけが変わっても変更とはみなさない。
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

/// 参照先のパッケージへ解決した依存辺。Cargo は曖昧でない最短の形
/// （`"toml"`、`"toml 0.9.8"`、または取得元付き）で参照を書くため、
/// 同名の別バージョンが lock に入る、または抜けるだけでも、変わっていない辺の
/// 表記が変わる。一意のパッケージを指さない参照は元の文字列を保持し、
/// 文字列でない要素は TOML 表現を保持する。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Edge {
    Package(LockId),
    Unresolved(String),
    Invalid(String),
}

/// lock のパッケージを、そのほかのフィールドと、参照先パッケージの多重集合としての
/// 依存辺で意味的に比較する。
#[derive(Clone, Debug, PartialEq)]
struct LockEntry {
    fields: toml::Value,
    edges: BTreeMap<Edge, usize>,
}

impl LockEntry {
    /// 操作が lock から外したパッケージへの辺を Cargo が付け替えただけで、このパッケージが
    /// 変わったかどうか。`cargo update -p X` は X と、前の lock で X から到達できるパッケージを
    /// lock から外す。範囲外の依存元でも、その中を指す辺は、版要求が許す同じ名前・取得元の
    /// 別の版へ付け替わる。版要求が系列をまたぐ範囲 (`windows-sys >=0.52, <0.62` など) なら、
    /// 別の系列の版にもなる。消えた辺は `unlocked` の中を指し、同じ名前・取得元の追加辺と
    /// 対にならなければならない。辺の純増・純減や、そのほかのフィールドの変化は、
    /// この規則では説明できない。
    fn rewired(&self, after: &Self, unlocked: &BTreeSet<LockId>) -> bool {
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
                let pair = added
                    .iter()
                    .position(|new| new.0 == old.0 && new.2 == old.2);
                match pair {
                    Some(index) if unlocked.contains(old) => {
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
    // git の取得元の `#<commit>` は参照には含まれない。
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

    /// 実際の lock に監査済みの変更だけがあり、スナップショット全体で置換できるかを返す。
    /// 監査した操作の範囲外で変わったレジストリ・git のパッケージは、選択的復元と
    /// 同様に保持する。path パッケージは workspace のマニフェストに従うため、
    /// スナップショット側で別途確認する。
    pub(super) fn allows_snapshot_restore(&self, content: Option<&[u8]>) -> bool {
        !self.between_commands_changed
            && self.external_packages().is_empty()
            && content == Some(self.checkpoint.as_bytes())
    }

    /// 監査した操作の範囲外で変わったレジストリ・git のパッケージ（`name version`）。
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

    /// 実際の lock に書く単一 crate の Cargo コマンド (`cargo update -p name@ver --precise`) の前後。
    /// コマンドの実行中に外から書かれた変更も同じ差分に入りうるので、Cargo が書き換えうる範囲
    /// ([`LockEntry::rewired`]) の外の変化は外部変更とする。
    pub(super) fn applied(&mut self, before: &str, after: &str, selected: &[PackageKey]) {
        self.record(before, after, selected, true);
    }

    /// 一時 workspace で解いた lock を、元の lock が変わっていないことを確かめてから反映した前後。
    /// 差分はすべて depup が書いたものなので、外部変更を含まない。固定用の別名依存が lock に
    /// 無い版を指すと、Cargo は取得元のパッケージをすべて lock から外して解き直すため、選んだ
    /// 範囲の外の依存元の辺も付け替わる。選んだ範囲の変更だけを所有し、範囲外の変化は
    /// 所有も外部変更もせず、以前の操作による記録をそのまま残す。
    pub(super) fn committed(&mut self, before: &str, after: &str, selected: &[PackageKey]) {
        self.record(before, after, selected, false);
    }

    fn record(&mut self, before: &str, after: &str, selected: &[PackageKey], in_place: bool) {
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
        // 到達可能性と、取得元・系列が対になる端点を、変化がなくなるまで一緒に展開する。
        // どちらのグラフで見つけた端点からも、変更前後の依存がさらに見つかりうる。
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
        if in_place {
            // Cargo が lock から外すのは、選んだパッケージと、前の lock でそこから到達できる
            // パッケージ。範囲外の依存元でも、その中を指す辺は付け替わる。依存元自体は
            // 変わっていないため、監査の所有対象にも外部変更にも追加せず、以前の操作による
            // 記録をそのまま残す。到達は取得元込みの辺で辿る (同じ名前・版の別の取得元の
            // パッケージは、Cargo が外していない)。
            let mut unlocked = BTreeSet::new();
            let mut pending: Vec<LockId> = before_ids
                .iter()
                .filter(|id| selected.iter().any(|root| id.0 == root.0 && id.1 == root.1))
                .cloned()
                .collect();
            while let Some(id) = pending.pop() {
                if !unlocked.insert(id.clone()) {
                    continue;
                }
                if let Some(entry) = before_tables.get(&id) {
                    pending.extend(entry.edges.keys().filter_map(|edge| match edge {
                        Edge::Package(dependency) => Some(dependency.clone()),
                        Edge::Unresolved(_) | Edge::Invalid(_) => None,
                    }));
                }
            }
            let rewired = |key: &LockId| {
                before_tables
                    .get(key)
                    .zip(after_tables.get(key))
                    .is_some_and(|(before, after)| before.rewired(after, &unlocked))
            };
            self.disown(
                changed
                    .iter()
                    .filter(|key| !ours.contains(*key) && !rewired(key))
                    .cloned()
                    .collect(),
            );
        }
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
            // すでに元のレジストリのパッケージIDへ戻っている。
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
            // LockGraph は同名・同版のIDをまとめる。接続経路に重複する取得元があると
            // 関係を確定できないため、復元範囲を推定しない。
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
                // 再導入する依存も I に固定し、別の第3の版にはしない。
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
        // 後続の監査済みコマンドが、以前に明示的に差し戻した版を再び動かすこともある。
        state.log.reconcile_with_lock(&current);
        // 未訪問の I の版が安全だと、キャッシュ済みの公開情報から判断できる場合もある。
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
                    state.changes.committed(&expected_lock, &committed, &selected);
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

    /// Tauri の lock に相当する構成。`toml` が2系列あるため、Cargo は `cargo_toml` の
    /// 依存参照に版を付け、`toml` の移動に伴ってその参照を書き換える。
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

        // 依存元について以前の操作が記録した内容は、そのまま残す。
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

        // 同じ書き換えでも、監査した操作の間に起きた場合は外部変更とする。
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
        // 別系列の `toml` があると、Cargo は同じ辺の表記に版を追加する。
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

        // 取得元の接尾辞は2つの取得元を区別する。git の参照には commit が含まれない。
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
            // 範囲内の変わっていないパッケージへの辺を追加する場合。
            (
                toml_family("0.9.5", &["toml 0.9.5", "serde", "ghost 1.0.0"]),
                &moved_toml,
            ),
            // 変更したパッケージを指さない参照を使う場合。
            (
                toml_family("0.9.5", &["toml 0.9.5", "ghost 2.0.0"]),
                &moved_toml,
            ),
            // 別のフィールドも変わった場合。
            (
                retargeted.replace(
                    "dependencies = [\"toml 0.9.5\"",
                    "checksum = \"changed\"\ndependencies = [\"toml 0.9.5\"",
                ),
                &moved_toml,
            ),
            // 元の対象を動かしていない操作の場合。
            (retargeted.clone(), &moved_serde),
            // 操作が lock から外していないパッケージへの辺を、別の系列へ付け替える場合。
            (
                toml_family("0.9.8", &["toml 0.8.2", "ghost 1.0.0"]),
                &moved_serde,
            ),
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

    /// Issue #58: `cargo update -p user@1.1.0 --precise 1.0.0` は `user` と、前の lock で
    /// そこから到達できる `toml 0.9.8` を lock から外す。`toml 0.9.8` は残って表も
    /// 変わらないが、範囲外の `cargo_toml` の辺は、版要求 (`toml >=0.8, <0.10` のような
    /// 系列をまたぐ範囲) が許す `toml 0.8.2` へ付け替わる。これは Cargo 自身の変更。
    #[test]
    fn repointing_into_the_unlocked_scope_is_cargos_own_change() {
        let family = |user: &str, cargo_toml: &str| {
            lock(&[
                ("cargo_toml", "0.22.0", PUBLIC, &[cargo_toml]),
                ("system-deps", "7.0.0", PUBLIC, &["toml 0.8.2"]),
                ("toml", "0.8.2", PUBLIC, &[]),
                ("toml", "0.9.8", PUBLIC, &[]),
                ("user", user, PUBLIC, &["toml 0.9.8"]),
            ])
        };
        let installed = family("1.1.0", "toml 0.9.8");
        let after = family("1.0.0", "toml 0.8.2");
        let cargo_toml = ("cargo_toml".to_string(), "0.22.0".to_string());
        let mut changes = AuditChanges::default();
        changes.applied(&installed, &after, &[("user".into(), "1.1.0".into())]);
        assert!(!changes.externally_changed(&cargo_toml));
        assert!(!changes.owns("cargo_toml", "0.22.0"));
        assert!(changes.owns("user", "1.1.0") && changes.owns("user", "1.0.0"));
        assert!(changes.allows_snapshot_restore(Some(after.as_bytes())));

        // `system-deps` を選んだ操作は `toml 0.9.8` を lock から外さないので、Cargo はその辺を
        // 付け替えない (`user` の変化も範囲外)。
        let mut changes = AuditChanges::default();
        changes.applied(
            &installed,
            &after,
            &[("system-deps".into(), "7.0.0".into())],
        );
        assert!(changes.externally_changed(&cargo_toml));
        assert!(!changes.allows_snapshot_restore(Some(after.as_bytes())));

        // 同じ名前・版でも、別の取得元のパッケージは Cargo が lock から外していない。
        let public = format!("toml 0.9.8 ({PUBLIC})");
        let private = format!("toml 0.9.8 ({PRIVATE})");
        let sources = |user: &str, cargo_toml: &str| {
            lock(&[
                ("cargo_toml", "0.22.0", PUBLIC, &[cargo_toml]),
                ("toml", "0.8.2", PRIVATE, &[]),
                ("toml", "0.9.8", PRIVATE, &[]),
                ("toml", "0.9.8", PUBLIC, &[]),
                ("user", user, PUBLIC, &[public.as_str()]),
            ])
        };
        let installed = sources("1.1.0", &private);
        let after = sources("1.0.0", "toml 0.8.2");
        let mut changes = AuditChanges::default();
        changes.applied(&installed, &after, &[("user".into(), "1.1.0".into())]);
        assert!(
            changes
                .external_packages()
                .contains(&"cargo_toml 0.22.0".to_string()),
            "{:?}",
            changes.external_packages()
        );
        assert!(!changes.allows_snapshot_restore(Some(after.as_bytes())));
    }

    /// 一時 workspace からの反映は、元の lock が変わっていないことを確かめてから書くので、
    /// 差分に外部変更は入らない。固定用の依存が取得元を丸ごと lock から外すため、選んだ範囲の
    /// 外の依存元 (`cargo_toml`) の辺も付け替わるが、所有も外部変更もしない。
    #[test]
    fn committed_resolutions_never_record_external_changes() {
        let installed = toml_family("0.9.8", &["toml 0.8.2"]);
        let after = toml_family("0.9.5", &["toml 0.9.5"]);
        let cargo_toml = ("cargo_toml".to_string(), "0.22.0".to_string());
        let selected = [("toml".to_string(), "0.9.8".to_string())];
        let mut changes = AuditChanges::default();
        changes.committed(&installed, &after, &selected);
        assert!(!changes.externally_changed(&cargo_toml));
        assert!(!changes.owns("cargo_toml", "0.22.0"));
        assert!(changes.owns("toml", "0.9.8") && changes.owns("toml", "0.9.5"));
        assert!(changes.allows_snapshot_restore(Some(after.as_bytes())));

        // 範囲外の依存元について、以前の操作が記録した内容はそのまま残す。
        let id = (
            cargo_toml.0.clone(),
            cargo_toml.1.clone(),
            Some(PUBLIC.to_string()),
        );
        let mut earlier = AuditChanges::default();
        earlier.owned.insert(id.clone());
        earlier.committed(&installed, &after, &selected);
        assert!(earlier.owns("cargo_toml", "0.22.0"));
        let mut earlier = AuditChanges::default();
        earlier.external.insert(id);
        earlier.committed(&installed, &after, &selected);
        assert!(earlier.externally_changed(&cargo_toml));

        // 反映の前にコマンドの間で変わっていれば、従来どおり外部変更とする。
        let edited = installed.replace("version = \"7.0.0\"", "version = \"7.0.1\"");
        let mut changes = AuditChanges::default();
        changes.observe(&installed);
        changes.committed(&edited, &after, &selected);
        assert!(changes.externally_changed(&("system-deps".into(), "7.0.0".into())));
        assert!(!changes.allows_snapshot_restore(Some(after.as_bytes())));
    }

    #[test]
    fn rewiring_pairs_every_unlocked_edge_with_one_of_the_same_package() {
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
        let unlocked: BTreeSet<_> = [
            id("toml", "0.9.8"),
            id("toml", "0.8.2"),
            id("syn", "2.0.10"),
        ]
        .into_iter()
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
            // 版要求が系列をまたぐ範囲なら、別の系列の版へも付け替わる。
            (entry(&[("toml", "0.9.5", 3), ("syn", "2.0.9", 1)]), true),
            // 同じ2本の辺の一方に、対応する辺が残らない場合。
            (
                entry(&[
                    ("toml", "0.9.5", 1),
                    ("toml", "0.8.1", 1),
                    ("syn", "2.0.9", 1),
                ]),
                false,
            ),
            // 付け替えに加えて、別の辺が追加または削除される場合。
            (
                entry(&[
                    ("toml", "0.9.5", 3),
                    ("toml", "0.8.1", 1),
                    ("syn", "2.0.9", 1),
                ]),
                false,
            ),
            (entry(&[("toml", "0.9.5", 2), ("toml", "0.8.1", 1)]), false),
            // 別の名前のパッケージへは付け替わらない。
            (
                entry(&[
                    ("toml", "0.9.5", 2),
                    ("toml", "0.8.1", 1),
                    ("serde", "1.0.0", 1),
                ]),
                false,
            ),
        ] {
            assert_eq!(before.rewired(&after, &unlocked), explained, "{after:?}");
        }
        let after = entry(&[
            ("toml", "0.9.5", 2),
            ("toml", "0.8.1", 1),
            ("syn", "2.0.9", 1),
        ]);
        // lock から外れていないパッケージへの辺は、Cargo が付け替えない。
        assert!(!before.rewired(&after, &BTreeSet::new()));
        let mut partial = unlocked.clone();
        partial.remove(&id("toml", "0.8.2"));
        assert!(!before.rewired(&after, &partial));
        // 別の取得元の同じ名前へは付け替わらない。
        let mut elsewhere = after.clone();
        elsewhere.edges.remove(&Edge::Package(id("syn", "2.0.9")));
        elsewhere.edges.insert(
            Edge::Package((
                "syn".into(),
                "2.0.9".into(),
                Some("git+https://example.com/syn?branch=main#0123abcd".into()),
            )),
            1,
        );
        assert!(!before.rewired(&elsewhere, &unlocked));
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
        // 文字列の要素と文字列でない要素は、同じ辺にはまとめない。
        assert_ne!(entry("[\"true\"]"), entry("[true]"));
        // 配列でないリスト値は、比較対象のフィールド側に残す。
        assert_ne!(entry("\"toml\""), entry("\"serde\""));
        assert_eq!(entry("\"toml\""), entry("\"toml\""));
    }

    #[test]
    fn path_packages_alone_do_not_prevent_snapshot_restoration() {
        // path パッケージには取得元がないため、スナップショットがマニフェストを確認する。
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
