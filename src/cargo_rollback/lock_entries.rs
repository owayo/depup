//! Cargo.lock のパッケージと依存辺を、取得元・多重度を含めて意味で比較する。

use crate::manifest::parse_dependency_ref;
use std::collections::{BTreeMap, BTreeSet};

pub(super) type LockId = (String, String, Option<String>);

/// 参照先のパッケージへ解決した依存辺。Cargo は曖昧でない最短の形
/// （`"toml"`、`"toml 0.9.8"`、または取得元付き）で参照を書くため、
/// 同名の別バージョンが lock に入る、または抜けるだけでも、変わっていない辺の
/// 表記が変わる。一意のパッケージを指さない参照は元の文字列を保持し、
/// 文字列でない要素は TOML 表現を保持する。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Edge {
    Package(LockId),
    Unresolved(String),
    Invalid(String),
}

/// lock のパッケージを、そのほかのフィールドと、参照先パッケージの多重集合としての
/// 依存辺で意味的に比較する。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LockEntry {
    pub(super) fields: toml::Value,
    pub(super) edges: BTreeMap<Edge, usize>,
}

impl LockEntry {
    /// 操作が lock から外したパッケージへの辺を Cargo が付け替えただけで、このパッケージが
    /// 変わったかどうか。`cargo update -p X` は X と、前の lock で X から到達できるパッケージを
    /// lock から外す。範囲外の依存元でも、その中を指す辺は、版要求が許す同じ名前・取得元の
    /// 別の版へ付け替わる。版要求が系列をまたぐ範囲 (`windows-sys >=0.52, <0.62` など) なら、
    /// 別の系列の版にもなる。消えた辺は `unlocked` の中を指し、同じ名前・取得元の追加辺と
    /// 対にならなければならない。辺の純増・純減や、そのほかのフィールドの変化は、
    /// この規則では説明できない。
    pub(super) fn rewired(&self, after: &Self, unlocked: &BTreeSet<LockId>) -> bool {
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

pub(super) fn lock_tables(content: &str) -> BTreeMap<LockId, LockEntry> {
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
