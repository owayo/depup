//! Cargo.lock の依存グラフ。
//!
//! `=` で互いを固定し合う crate 群 (wasm-bindgen 一族など) は 1 件ずつ差し戻せないため、
//! 一族の範囲 (連結成分) と、差し戻しの起点にする頂点を lock の依存辺から求める。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::manifest::{LockDependencyRef, LockPackage, parse_lock_packages};

/// Cargo.lock 上のパッケージを名前と版で識別するキー
pub type PackageKey = (String, String);

/// Cargo.lock の依存グラフ (ノードは lock の全パッケージ。path / git / 私設 registry も含む)
///
/// ノードは (名前, 版) で識別する。同じ (名前, 版) が複数の source にある場合
/// (crates.io の crate と同じ版の git fork を併用する等) は 1 ノードにまとめるため、
/// どちらの source への依存かは区別できない (制限)。まとめた結果として自分自身へ
/// 向く辺は張らない。
///
/// 依存参照 (`dependencies` 配列の要素) の解決規則:
/// - `"name"`: 同名のノードが 1 つだけならそれ。同名の版が複数あれば解決しない
/// - `"name version"`: 名前と版が一致するノード
/// - `"name version (source)"`: source も一致するものを優先し、無ければ名前と版の一致で
///   解決する。ノードを (名前, 版) でまとめているので、どちらでも同じノードになる
///
/// 解決できない参照 (lock に無い名前・版) には辺を張らない。
#[derive(Debug, Clone, Default)]
pub struct LockGraph {
    /// ノード → 直接依存しているパッケージ。依存を持たないノードも空集合で持つ
    dependencies: BTreeMap<PackageKey, BTreeSet<PackageKey>>,
    /// ノード → 直接依存されているパッケージ (依存元)
    dependents: BTreeMap<PackageKey, BTreeSet<PackageKey>>,
}

impl LockGraph {
    pub fn from_packages(packages: &[LockPackage]) -> Self {
        let mut graph = Self::default();
        // 名前 → lock にある版。`"name"` だけの参照を解決するのに使う
        let mut versions_by_name: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        for package in packages {
            let key = (package.name.clone(), package.version.clone());
            graph.dependencies.entry(key.clone()).or_default();
            graph.dependents.entry(key).or_default();
            versions_by_name
                .entry(&package.name)
                .or_default()
                .insert(&package.version);
        }

        for package in packages {
            let from = (package.name.clone(), package.version.clone());
            for dependency in &package.dependencies {
                let Some(to) = resolve_dependency(&versions_by_name, dependency) else {
                    continue;
                };
                if to == from {
                    continue;
                }
                graph
                    .dependents
                    .entry(to.clone())
                    .or_default()
                    .insert(from.clone());
                graph
                    .dependencies
                    .entry(from.clone())
                    .or_default()
                    .insert(to);
            }
        }
        graph
    }

    pub fn from_lock_content(content: &str) -> Self {
        Self::from_packages(&parse_lock_packages(content))
    }

    /// `key` に直接依存しているパッケージ (依存元)。名前順 → 版順に並べる。lock に無ければ空
    pub fn dependents(&self, key: &PackageKey) -> Vec<PackageKey> {
        self.dependents
            .get(key)
            .map(sorted_keys)
            .unwrap_or_default()
    }

    /// `key` が直接依存しているパッケージ。名前順 → 版順
    pub fn dependencies(&self, key: &PackageKey) -> Vec<PackageKey> {
        self.dependencies
            .get(key)
            .map(sorted_keys)
            .unwrap_or_default()
    }

    /// `group` の中で、group 内の他のパッケージから依存されていないもの (一族の「頂点」)。入力順を保つ。
    /// group 内に lock に無いキーがあれば、それは依存元を持たないものとして頂点に含める。
    /// 重複したキーは最初の 1 つだけを返す
    pub fn tops(&self, group: &[PackageKey]) -> Vec<PackageKey> {
        let members: HashSet<&PackageKey> = group.iter().collect();
        unique_in_order(group)
            .into_iter()
            .filter(|key| {
                self.dependents
                    .get(*key)
                    .is_none_or(|parents| !parents.iter().any(|parent| members.contains(parent)))
            })
            .cloned()
            .collect()
    }

    /// `group` を、group 内のパッケージ同士の直接の依存辺 (向きは無視) だけでつないだ連結成分に分ける。
    /// group の外のパッケージは経由しない。各成分の中は入力順、成分の並びは各成分の最初の要素の入力順。
    /// lock に無いキーは単独の成分になる。重複したキーは最初の 1 つだけを扱う
    pub fn components(&self, group: &[PackageKey]) -> Vec<Vec<PackageKey>> {
        let members = unique_in_order(group);
        let position: HashMap<&PackageKey, usize> = members
            .iter()
            .enumerate()
            .map(|(index, key)| (*key, index))
            .collect();
        let mut visited = vec![false; members.len()];
        let mut components = Vec::new();

        // 入力順に未到達のメンバーから辿るので、成分は最初の要素の入力順に並ぶ
        for start in 0..members.len() {
            if visited[start] {
                continue;
            }
            visited[start] = true;
            let mut component = vec![start];
            let mut stack = vec![start];
            while let Some(current) = stack.pop() {
                for neighbor in self.neighbors(members[current]) {
                    // group の外のパッケージは経由しない
                    let Some(&next) = position.get(neighbor) else {
                        continue;
                    };
                    if !visited[next] {
                        visited[next] = true;
                        component.push(next);
                        stack.push(next);
                    }
                }
            }
            component.sort_unstable();
            components.push(
                component
                    .into_iter()
                    .map(|index| members[index].clone())
                    .collect(),
            );
        }
        components
    }

    /// 向きを無視した隣接ノード (依存先と依存元)
    fn neighbors(&self, key: &PackageKey) -> impl Iterator<Item = &PackageKey> {
        self.dependencies
            .get(key)
            .into_iter()
            .flatten()
            .chain(self.dependents.get(key).into_iter().flatten())
    }
}

/// 依存参照を lock のノードへ解決する (規則は `LockGraph` の doc を参照)
fn resolve_dependency(
    versions_by_name: &HashMap<&str, BTreeSet<&str>>,
    dependency: &LockDependencyRef,
) -> Option<PackageKey> {
    let versions = versions_by_name.get(dependency.name.as_str())?;
    // source はノードの識別に使わない ((名前, 版) でまとめているため)
    let version = match dependency.version.as_deref() {
        Some(version) if versions.contains(version) => version,
        Some(_) => return None,
        // 同名の版が複数あると、どれを指すか決められない
        None if versions.len() == 1 => *versions.first()?,
        None => return None,
    };
    Some((dependency.name.clone(), version.to_string()))
}

/// 重複したキーを除き、最初に現れた順に並べる
fn unique_in_order(group: &[PackageKey]) -> Vec<&PackageKey> {
    let mut seen = HashSet::new();
    group.iter().filter(|key| seen.insert(*key)).collect()
}

/// 名前順 → 版順に並べる
fn sorted_keys<'a>(keys: impl IntoIterator<Item = &'a PackageKey>) -> Vec<PackageKey> {
    let mut keys: Vec<PackageKey> = keys.into_iter().cloned().collect();
    keys.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| compare_versions(&a.1, &b.1)));
    keys
}

/// 版の並び順。semver として読める版は semver の順 (文字列順だと `0.10.0` が `0.9.0` より
/// 前に来る) で、読めない版はその後ろに文字列順で置く
fn compare_versions(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(x), Ok(y)) => x.cmp(&y).then_with(|| a.cmp(b)),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, version: &str) -> PackageKey {
        (name.to_string(), version.to_string())
    }

    /// wasm-bindgen 一族を模した lock。
    /// web-sys と wasm-bindgen-test が一族の頂点で、js-sys / wasm-bindgen / wasm-bindgen-futures
    /// などは互いに `=` で固定される側。app (workspace member) と gloo-timers は一族の外から
    /// 依存する。syn は 1.x と 2.x が共存するので、参照は `"syn 2.0.77"` の形になる
    const WASM_BINDGEN_LOCK: &str = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "serde",
 "wasm-bindgen-test",
 "web-sys",
]

[[package]]
name = "bumpalo"
version = "3.16.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "cfg-if"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "gloo-timers"
version = "0.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "js-sys",
 "wasm-bindgen",
]

[[package]]
name = "js-sys"
version = "0.3.70"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "wasm-bindgen",
]

[[package]]
name = "legacy-derive"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "syn 1.0.109",
]

[[package]]
name = "proc-macro2"
version = "1.0.86"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "quote"
version = "1.0.37"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "proc-macro2",
]

[[package]]
name = "serde"
version = "1.0.210"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "serde_derive",
]

[[package]]
name = "serde_derive"
version = "1.0.210"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.77",
]

[[package]]
name = "syn"
version = "1.0.109"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "proc-macro2",
 "quote",
]

[[package]]
name = "syn"
version = "2.0.77"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "proc-macro2",
 "quote",
]

[[package]]
name = "wasm-bindgen"
version = "0.2.93"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "cfg-if",
 "wasm-bindgen-macro",
]

[[package]]
name = "wasm-bindgen-backend"
version = "0.2.93"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "bumpalo",
 "proc-macro2",
 "quote",
 "syn 2.0.77",
 "wasm-bindgen-shared",
]

[[package]]
name = "wasm-bindgen-futures"
version = "0.4.43"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "cfg-if",
 "js-sys",
 "wasm-bindgen",
]

[[package]]
name = "wasm-bindgen-macro"
version = "0.2.93"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "quote",
 "wasm-bindgen-macro-support",
]

[[package]]
name = "wasm-bindgen-macro-support"
version = "0.2.93"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.77",
 "wasm-bindgen-backend",
 "wasm-bindgen-shared",
]

[[package]]
name = "wasm-bindgen-shared"
version = "0.2.93"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "wasm-bindgen-test"
version = "0.3.43"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "js-sys",
 "wasm-bindgen",
 "wasm-bindgen-futures",
 "wasm-bindgen-test-macro",
]

[[package]]
name = "wasm-bindgen-test-macro"
version = "0.3.43"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.77",
]

[[package]]
name = "web-sys"
version = "0.3.70"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "js-sys",
 "wasm-bindgen",
]
"#;

    /// wasm-bindgen 一族 (頂点の web-sys を先頭、wasm-bindgen-test を途中に置いた入力順)
    fn wasm_bindgen_family() -> Vec<PackageKey> {
        vec![
            key("web-sys", "0.3.70"),
            key("js-sys", "0.3.70"),
            key("wasm-bindgen", "0.2.93"),
            key("wasm-bindgen-backend", "0.2.93"),
            key("wasm-bindgen-futures", "0.4.43"),
            key("wasm-bindgen-macro", "0.2.93"),
            key("wasm-bindgen-macro-support", "0.2.93"),
            key("wasm-bindgen-shared", "0.2.93"),
            key("wasm-bindgen-test", "0.3.43"),
            key("wasm-bindgen-test-macro", "0.3.43"),
        ]
    }

    #[test]
    fn test_tops_of_wasm_bindgen_family() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        // 一族の外 (app / gloo-timers) からの依存は頂点の判定に影響しない
        assert_eq!(
            graph.tops(&wasm_bindgen_family()),
            vec![key("web-sys", "0.3.70"), key("wasm-bindgen-test", "0.3.43")]
        );
    }

    #[test]
    fn test_components_of_wasm_bindgen_family_is_single() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        let family = wasm_bindgen_family();
        assert_eq!(graph.components(&family), vec![family]);
    }

    #[test]
    fn test_components_split_unrelated_groups() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        let group = vec![
            key("serde", "1.0.210"),
            key("web-sys", "0.3.70"),
            key("serde_derive", "1.0.210"),
            key("js-sys", "0.3.70"),
            key("wasm-bindgen", "0.2.93"),
        ];
        // 成分の並びは各成分の最初の要素の入力順、成分の中は入力順
        assert_eq!(
            graph.components(&group),
            vec![
                vec![key("serde", "1.0.210"), key("serde_derive", "1.0.210")],
                vec![
                    key("web-sys", "0.3.70"),
                    key("js-sys", "0.3.70"),
                    key("wasm-bindgen", "0.2.93"),
                ],
            ]
        );
    }

    #[test]
    fn test_components_do_not_connect_through_outside_packages() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        // wasm-bindgen-macro → wasm-bindgen-macro-support → wasm-bindgen-backend の鎖は、
        // 間の macro-support が group の外なのでつながらない
        let chain = vec![
            key("wasm-bindgen-macro", "0.2.93"),
            key("wasm-bindgen-backend", "0.2.93"),
        ];
        assert_eq!(
            graph.components(&chain),
            vec![
                vec![key("wasm-bindgen-macro", "0.2.93")],
                vec![key("wasm-bindgen-backend", "0.2.93")],
            ]
        );
        assert_eq!(graph.tops(&chain), chain);

        // 共通の依存先 (syn 2.0.77 / quote) が group の外なら、それを介してもつながらない
        let siblings = vec![
            key("serde_derive", "1.0.210"),
            key("wasm-bindgen-test-macro", "0.3.43"),
        ];
        assert_eq!(graph.components(&siblings).len(), 2);
    }

    #[test]
    fn test_versioned_reference_resolves_among_multiple_versions() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        assert_eq!(
            graph.dependencies(&key("wasm-bindgen-backend", "0.2.93")),
            vec![
                key("bumpalo", "3.16.0"),
                key("proc-macro2", "1.0.86"),
                key("quote", "1.0.37"),
                key("syn", "2.0.77"),
                key("wasm-bindgen-shared", "0.2.93"),
            ]
        );
        assert_eq!(
            graph.dependents(&key("syn", "2.0.77")),
            vec![
                key("serde_derive", "1.0.210"),
                key("wasm-bindgen-backend", "0.2.93"),
                key("wasm-bindgen-macro-support", "0.2.93"),
                key("wasm-bindgen-test-macro", "0.3.43"),
            ]
        );
        assert_eq!(
            graph.dependents(&key("syn", "1.0.109")),
            vec![key("legacy-derive", "0.1.0")]
        );
    }

    #[test]
    fn test_dependents_include_path_packages() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        // source を持たない workspace member (app) もノードになる
        assert_eq!(
            graph.dependents(&key("web-sys", "0.3.70")),
            vec![key("app", "0.1.0")]
        );
        assert_eq!(
            graph.dependencies(&key("app", "0.1.0")),
            vec![
                key("serde", "1.0.210"),
                key("wasm-bindgen-test", "0.3.43"),
                key("web-sys", "0.3.70"),
            ]
        );
    }

    #[test]
    fn test_name_only_reference_is_unresolved_when_ambiguous() {
        // Cargo は同名の版が複数あれば `"syn"` とは書かないが、壊れた lock でも推測で辺を張らない
        let content = r#"
[[package]]
name = "ambiguous"
version = "0.1.0"
dependencies = [
 "quote",
 "syn",
]

[[package]]
name = "quote"
version = "1.0.37"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "1.0.109"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "2.0.77"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let graph = LockGraph::from_lock_content(content);
        assert_eq!(
            graph.dependencies(&key("ambiguous", "0.1.0")),
            vec![key("quote", "1.0.37")]
        );
        assert!(graph.dependents(&key("syn", "1.0.109")).is_empty());
        assert!(graph.dependents(&key("syn", "2.0.77")).is_empty());
    }

    #[test]
    fn test_reference_with_source_and_merged_nodes() {
        let content = r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "bar 2.0.0 (registry+https://github.com/rust-lang/crates.io-index)",
 "baz 3.0.0 (registry+https://github.com/rust-lang/crates.io-index)",
 "foo 1.0.0 (git+https://github.com/fork/foo.git?branch=main)",
]

[[package]]
name = "bar"
version = "2.0.0"
source = "sparse+https://my.example.com/index/"

[[package]]
name = "foo"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "foo"
version = "1.0.0"
source = "git+https://github.com/fork/foo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"
dependencies = [
 "foo 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)",
]
"#;
        let graph = LockGraph::from_lock_content(content);
        // 同じ (名前, 版) の 2 source は 1 ノードにまとまる
        assert_eq!(graph.dependencies.len(), 3);
        // source が一致しない参照 (bar) も名前と版で解決し、lock に無い版 (baz) には辺を張らない
        assert_eq!(
            graph.dependencies(&key("app", "0.1.0")),
            vec![key("bar", "2.0.0"), key("foo", "1.0.0")]
        );
        // まとめた結果の自分自身への辺は張らない
        assert!(graph.dependencies(&key("foo", "1.0.0")).is_empty());
        assert_eq!(
            graph.dependents(&key("foo", "1.0.0")),
            vec![key("app", "0.1.0")]
        );
        assert_eq!(
            graph.tops(&[key("foo", "1.0.0")]),
            vec![key("foo", "1.0.0")]
        );
    }

    #[test]
    fn test_neighbors_sorted_by_name_then_semver() {
        let content = r#"
[[package]]
name = "shared"
version = "1.0.0"

[[package]]
name = "user"
version = "2.0.0"
dependencies = ["shared"]

[[package]]
name = "user"
version = "0.10.0"
dependencies = ["shared"]

[[package]]
name = "user"
version = "not-semver"
dependencies = ["shared"]

[[package]]
name = "another"
version = "1.0.0"
dependencies = ["shared"]

[[package]]
name = "user"
version = "0.9.0"
dependencies = ["shared"]

[[package]]
name = "user"
version = "1.0.0-rc.1"
dependencies = ["shared"]
"#;
        let graph = LockGraph::from_lock_content(content);
        // 版は文字列順ではなく semver の順。読めない版は後ろに置く
        assert_eq!(
            graph.dependents(&key("shared", "1.0.0")),
            vec![
                key("another", "1.0.0"),
                key("user", "0.9.0"),
                key("user", "0.10.0"),
                key("user", "1.0.0-rc.1"),
                key("user", "2.0.0"),
                key("user", "not-semver"),
            ]
        );
    }

    #[test]
    fn test_keys_missing_from_lock() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        let missing = key("not-in-lock", "1.0.0");
        assert!(graph.dependents(&missing).is_empty());
        assert!(graph.dependencies(&missing).is_empty());
        // 版が違えば別のキー
        assert!(graph.dependents(&key("web-sys", "0.3.69")).is_empty());

        let group = vec![
            missing.clone(),
            key("web-sys", "0.3.70"),
            key("js-sys", "0.3.70"),
        ];
        // lock に無いキーは依存元を持たない頂点で、単独の成分になる
        assert_eq!(
            graph.tops(&group),
            vec![missing.clone(), key("web-sys", "0.3.70")]
        );
        assert_eq!(
            graph.components(&group),
            vec![
                vec![missing],
                vec![key("web-sys", "0.3.70"), key("js-sys", "0.3.70")],
            ]
        );
    }

    #[test]
    fn test_duplicate_keys_in_group() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        let group = vec![
            key("js-sys", "0.3.70"),
            key("web-sys", "0.3.70"),
            key("js-sys", "0.3.70"),
            key("web-sys", "0.3.70"),
        ];
        assert_eq!(graph.tops(&group), vec![key("web-sys", "0.3.70")]);
        assert_eq!(
            graph.components(&group),
            vec![vec![key("js-sys", "0.3.70"), key("web-sys", "0.3.70")]]
        );
    }

    #[test]
    fn test_empty_group_and_broken_lock() {
        let graph = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        assert!(graph.tops(&[]).is_empty());
        assert!(graph.components(&[]).is_empty());

        // TOML として読めない lock は空のグラフになる
        let broken = LockGraph::from_lock_content("this is not valid toml {{{");
        let web_sys = key("web-sys", "0.3.70");
        assert!(broken.dependents(&web_sys).is_empty());
        assert!(broken.dependencies(&web_sys).is_empty());
        assert_eq!(broken.tops(std::slice::from_ref(&web_sys)), vec![web_sys]);
    }

    #[test]
    fn test_from_packages_matches_from_lock_content() {
        let packages = parse_lock_packages(WASM_BINDGEN_LOCK);
        let from_packages = LockGraph::from_packages(&packages);
        let from_content = LockGraph::from_lock_content(WASM_BINDGEN_LOCK);
        assert_eq!(from_packages.dependencies, from_content.dependencies);
        assert_eq!(from_packages.dependents, from_content.dependents);
        // lock の全パッケージがノードになる
        assert_eq!(from_packages.dependencies.len(), packages.len());
    }
}
