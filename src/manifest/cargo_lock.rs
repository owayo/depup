//! `Cargo.lock` を読む。
//!
//! - git 依存の現在のコミットハッシュ (`parse_git_entries`)
//! - crates.io の依存の lock 済みバージョン (`parse_registry_entries`。post-install の age 監査用)
//! - 依存グラフを組むための全パッケージと依存参照 (`parse_lock_packages`)
//!
//! `Cargo.lock` の `[[package]]` エントリは git 依存の場合、
//! `source = "git+<url>?<ref>=<name>#<sha>"` 形式になっている。
//! 例:
//! ```toml
//! [[package]]
//! name = "tree-sitter-xojo"
//! source = "git+https://github.com/owayo/tree-sitter-xojo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"
//! ```
//!
//! git 依存については `name` -> `(url, sha)` のマップを返す。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use toml::Value;

/// git 依存のロック情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLockEntry {
    /// 正規化された URL (クエリ・フラグメントなし)
    pub url: String,
    /// 現在のコミットハッシュ (40 文字)
    pub commit: String,
}

/// 指定された `Cargo.lock` パスから git 依存をすべて読み込む。
///
/// workspace ルートの lock には同一クレート名が異なる URL から
/// lock されているケース (fork と upstream の併用等) がありうるため、
/// 名前ごとに全エントリを保持する。
pub fn read_git_entries(path: &Path) -> HashMap<String, Vec<GitLockEntry>> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_git_entries(&content)
}

/// `start` から `boundary` まで上方向に最も近い `Cargo.lock` を探す。
///
/// workspace メンバーの lock はワークスペースルートにのみ存在するため、
/// マニフェストと同じディレクトリだけを見ると見つからないことがある。
/// `boundary` (通常は実行対象のルートディレクトリ) を越えては探さない。
pub fn find_cargo_lock_upward(start: &Path, boundary: &Path) -> Option<PathBuf> {
    // 相対・絶対パスや symlink 表記の違いで境界判定を素通りしない。
    let original_boundary = boundary;
    let boundary = std::fs::canonicalize(boundary).ok()?;
    let start = std::fs::canonicalize(start).ok()?;
    if !start.starts_with(&boundary) {
        return None;
    }
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join("Cargo.lock");
        if candidate.exists() {
            return Some(
                original_boundary
                    .join(dir.strip_prefix(&boundary).ok()?)
                    .join("Cargo.lock"),
            );
        }
        if dir == boundary {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// crates.io の Cargo.lock 上の source。sparse プロトコル (既定) で取得しても、lock には互換のため git index の URL が記録される
pub const CRATES_IO_LOCK_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// crates.io の sparse index を直接指す source
pub const CRATES_IO_SPARSE_SOURCE: &str = "sparse+https://index.crates.io/";

/// lock の `source` が crates.io を指すか (上の 2 つだけを真とする)
pub fn is_crates_io_source(source: &str) -> bool {
    source == CRATES_IO_LOCK_SOURCE || source == CRATES_IO_SPARSE_SOURCE
}

/// Cargo.lock の crates.io 依存エントリ (name -> [resolved versions])
///
/// 1 パッケージ名につき複数の異なるバージョンが lock されているケース
/// (依存ツリー内で同一クレートの別バージョンが共存する場合) に備え、
/// バージョンの Vec を保持する。
pub type RegistryLockEntries = HashMap<String, Vec<String>>;

/// 指定された `Cargo.lock` パスから crates.io の依存をすべて読み込む
pub fn read_registry_entries(path: &Path) -> RegistryLockEntries {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_registry_entries(&content)
}

/// `Cargo.lock` 文字列から crates.io の依存をすべて抽出する。
///
/// source が crates.io (`is_crates_io_source`) でないエントリ (git / path 依存、
/// 私設 registry) は除外する。この結果を使う post-install の age 監査は crates.io API の
/// 公開日で判定するため、私設 registry の同名 crate まで含めると別物の crate の公開日で
/// 違反を誤判定する。
pub fn parse_registry_entries(content: &str) -> RegistryLockEntries {
    let mut result: RegistryLockEntries = HashMap::new();
    let Ok(toml) = toml::from_str::<Value>(content) else {
        return result;
    };

    let Some(packages) = toml.get("package").and_then(|v| v.as_array()) else {
        return result;
    };

    for pkg in packages {
        let Some(name) = pkg.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(version) = pkg.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(source) = pkg.get("source").and_then(|v| v.as_str()) else {
            // source 無しは path 依存 (ワークスペースメンバー含む) なのでスキップ
            continue;
        };
        // crates.io のみ対象。監査は crates.io API の公開日で判定するため、私設 registry の
        // 同名 crate を crates.io で照会すると別物の公開日で誤判定する。git+ も除外される
        if !is_crates_io_source(source) {
            continue;
        }
        result
            .entry(name.to_string())
            .or_default()
            .push(version.to_string());
    }

    result
}

/// `Cargo.lock` 文字列から git 依存を抽出する。
/// 同一名の複数バリアント (別 URL / 別 ref) はすべて保持し、
/// 呼び出し側が URL で対応付けられるようにする。
pub fn parse_git_entries(content: &str) -> HashMap<String, Vec<GitLockEntry>> {
    let mut result: HashMap<String, Vec<GitLockEntry>> = HashMap::new();
    let Ok(toml) = toml::from_str::<Value>(content) else {
        return result;
    };

    let Some(packages) = toml.get("package").and_then(|v| v.as_array()) else {
        return result;
    };

    for pkg in packages {
        let Some(name) = pkg.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(source) = pkg.get("source").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(entry) = parse_git_source(source) {
            let entries = result.entry(name.to_string()).or_default();
            if !entries.contains(&entry) {
                entries.push(entry);
            }
        }
    }

    result
}

/// `git+<url>?<ref>=<name>#<sha>` 形式の source 文字列をパースする
fn parse_git_source(source: &str) -> Option<GitLockEntry> {
    let rest = source.strip_prefix("git+")?;
    let (before_hash, hash) = rest.rsplit_once('#')?;
    if hash.len() < 7 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // `?<ref>=<name>` のクエリ部分は URL から除外する
    let url = before_hash.split('?').next().unwrap_or(before_hash);
    Some(GitLockEntry {
        url: url.to_string(),
        commit: hash.to_string(),
    })
}

/// Cargo.lock の `[[package]]` 1 件 (依存グラフの構築用)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
    /// path 依存・workspace member は None
    pub source: Option<String>,
    /// `dependencies` 配列の各要素 (Cargo 自身も読めない形の要素は除く)
    pub dependencies: Vec<LockDependencyRef>,
}

/// `dependencies` 配列の 1 要素。lock の記法は `"name"` / `"name version"` / `"name version (source)"`
///
/// Cargo (lock の version 2 以降) は名前だけで一意に決まれば `"name"`、名前と版で決まれば
/// `"name version"` と、曖昧にならない最短の形で書く。source には git の `#<sha>` が付かない
/// (パッケージ側の `source` とは文字列として一致しないことがある)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockDependencyRef {
    pub name: String,
    pub version: Option<String>,
    pub source: Option<String>,
}

/// 指定された `Cargo.lock` パスから全パッケージを読み込む。読めなければ空
pub fn read_lock_packages(path: &Path) -> Vec<LockPackage> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_lock_packages(&content)
}

/// `Cargo.lock` 文字列から全 `[[package]]` を依存参照ごと読み出す。
///
/// `parse_registry_entries` と違い source で絞らない (path / git / 私設 registry も含む)。
/// 依存グラフの辺は crates.io 以外のパッケージを経由することがあるため。
/// `name` か `version` が欠けたエントリは飛ばす。
/// ファイル順に返す。TOML として読めなければ空
pub fn parse_lock_packages(content: &str) -> Vec<LockPackage> {
    let Ok(toml) = toml::from_str::<Value>(content) else {
        return Vec::new();
    };
    let Some(packages) = toml.get("package").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    packages
        .iter()
        .filter_map(|pkg| {
            let name = pkg.get("name")?.as_str()?;
            let version = pkg.get("version")?.as_str()?;
            let source = pkg.get("source").and_then(|v| v.as_str());
            let dependencies = pkg
                .get("dependencies")
                .and_then(|v| v.as_array())
                .map(|deps| {
                    deps.iter()
                        .filter_map(|dep| dep.as_str().and_then(parse_dependency_ref))
                        .collect()
                })
                .unwrap_or_default();
            Some(LockPackage {
                name: name.to_string(),
                version: version.to_string(),
                source: source.map(str::to_string),
                dependencies,
            })
        })
        .collect()
}

/// `dependencies` 配列の 1 要素を読む。
///
/// Cargo の `EncodablePackageId` の読み方に合わせ、空白 1 つで最大 3 つに区切り、
/// 3 つ目は括弧で囲まれた source とする。名前や版が空の要素、3 つ目が括弧で
/// 囲まれていない要素は Cargo 自身も拒否する形なので None を返す。
fn parse_dependency_ref(raw: &str) -> Option<LockDependencyRef> {
    let mut parts = raw.splitn(3, ' ');
    let name = parts.next().filter(|name| !name.is_empty())?;
    let version = match parts.next() {
        Some("") => return None,
        Some(version) => Some(version.to_string()),
        None => None,
    };
    let source = match parts.next() {
        Some(rest) => {
            let source = rest.strip_prefix('(')?.strip_suffix(')')?;
            if source.is_empty() {
                return None;
            }
            Some(source.to_string())
        }
        None => None,
    };
    Some(LockDependencyRef {
        name: name.to_string(),
        version,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_source_branch() {
        let src = "git+https://github.com/owayo/tree-sitter-xojo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f";
        let entry = parse_git_source(src).unwrap();
        assert_eq!(entry.url, "https://github.com/owayo/tree-sitter-xojo.git");
        assert_eq!(entry.commit, "045c52a6db5390da14d96c0e4804a6208552dc8f");
    }

    #[test]
    fn test_parse_git_source_tag() {
        let src = "git+https://github.com/foo/bar.git?tag=v1.2.3#abcdef1234567890abcdef1234567890abcdef12";
        let entry = parse_git_source(src).unwrap();
        assert_eq!(entry.url, "https://github.com/foo/bar.git");
        assert_eq!(entry.commit, "abcdef1234567890abcdef1234567890abcdef12");
    }

    #[test]
    fn test_parse_git_source_rev() {
        let src = "git+https://github.com/foo/bar.git?rev=abcdef#fedcba9876543210fedcba9876543210fedcba98";
        let entry = parse_git_source(src).unwrap();
        assert_eq!(entry.url, "https://github.com/foo/bar.git");
        assert_eq!(entry.commit, "fedcba9876543210fedcba9876543210fedcba98");
    }

    #[test]
    fn test_parse_git_source_no_query() {
        // デフォルトブランチ (クエリなし)
        let src = "git+https://github.com/foo/bar.git#1234567890abcdef1234567890abcdef12345678";
        let entry = parse_git_source(src).unwrap();
        assert_eq!(entry.url, "https://github.com/foo/bar.git");
        assert_eq!(entry.commit, "1234567890abcdef1234567890abcdef12345678");
    }

    #[test]
    fn test_parse_git_source_invalid() {
        // # がない
        assert!(parse_git_source("git+https://github.com/foo/bar.git").is_none());
        // git+ がない
        assert!(
            parse_git_source("registry+https://github.com/rust-lang/crates.io-index").is_none()
        );
        // hash が短すぎる
        assert!(parse_git_source("git+https://example.com/r.git#abc").is_none());
    }

    #[test]
    fn test_parse_git_entries_basic() {
        let content = r#"
version = 3

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "tree-sitter-xojo"
version = "0.1.0"
source = "git+https://github.com/owayo/tree-sitter-xojo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"

[[package]]
name = "local"
version = "0.1.0"
# no source -> path dependency

[[package]]
name = "another-git"
version = "0.2.0"
source = "git+https://github.com/foo/bar.git?tag=v1.0#0000000000000000000000000000000000000001"
"#;
        let entries = parse_git_entries(content);
        assert_eq!(entries.len(), 2);

        let xojo = &entries.get("tree-sitter-xojo").unwrap()[0];
        assert_eq!(xojo.url, "https://github.com/owayo/tree-sitter-xojo.git");
        assert_eq!(xojo.commit, "045c52a6db5390da14d96c0e4804a6208552dc8f");

        let another = &entries.get("another-git").unwrap()[0];
        assert_eq!(another.url, "https://github.com/foo/bar.git");
        assert_eq!(another.commit, "0000000000000000000000000000000000000001");

        // registry dep / path dep は含まれない
        assert!(!entries.contains_key("serde"));
        assert!(!entries.contains_key("local"));
    }

    #[test]
    fn test_parse_git_entries_same_name_different_urls() {
        // workspace ルートの lock では fork と upstream が同名で共存しうる
        let content = r#"
[[package]]
name = "foo"
version = "0.1.0"
source = "git+https://github.com/upstream/foo.git?branch=main#0000000000000000000000000000000000000001"

[[package]]
name = "foo"
version = "0.1.0"
source = "git+https://github.com/fork/foo.git?branch=main#0000000000000000000000000000000000000002"
"#;
        let entries = parse_git_entries(content);
        let foo = entries.get("foo").unwrap();
        assert_eq!(foo.len(), 2);
        assert!(foo.iter().any(|e| e.url.contains("upstream")));
        assert!(foo.iter().any(|e| e.url.contains("fork")));
    }

    #[test]
    fn test_find_cargo_lock_upward() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let member = root.join("crates").join("core");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(root.join("Cargo.lock"), "version = 3\n").unwrap();

        // メンバーディレクトリから上方向にルートの lock が見つかる
        assert_eq!(
            find_cargo_lock_upward(&member, root),
            Some(root.join("Cargo.lock"))
        );
        // boundary より上には探しに行かない
        assert_eq!(find_cargo_lock_upward(&member, &member), None);
    }

    #[test]
    fn lock_search_rejects_paths_outside_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let outside = dir.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "version = 4\n").unwrap();
        assert_eq!(find_cargo_lock_upward(&root, &root.join(".")), None);
        assert_eq!(find_cargo_lock_upward(&outside, &root), None);
    }

    #[test]
    fn test_parse_git_entries_invalid_toml_returns_empty() {
        let entries = parse_git_entries("this is not valid toml {{{");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_git_entries_missing_package_array() {
        let entries = parse_git_entries("version = 3\n");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_registry_entries_basic() {
        let content = r#"
version = 3

[[package]]
name = "serde"
version = "1.0.210"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "tokio"
version = "1.40.0"
source = "sparse+https://index.crates.io/"

[[package]]
name = "tree-sitter-xojo"
version = "0.1.0"
source = "git+https://github.com/owayo/tree-sitter-xojo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"

[[package]]
name = "local-crate"
version = "0.1.0"
# path dependency, no source
"#;
        let entries = parse_registry_entries(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("serde").unwrap(), &vec!["1.0.210".to_string()]);
        assert_eq!(entries.get("tokio").unwrap(), &vec!["1.40.0".to_string()]);
        // git 依存は除外される
        assert!(!entries.contains_key("tree-sitter-xojo"));
        // path 依存も除外される
        assert!(!entries.contains_key("local-crate"));
    }

    #[test]
    fn test_parse_registry_entries_multiple_versions() {
        // 同一 crate の別バージョンが lock されるケース (依存ツリー解決の結果)
        let content = r#"
[[package]]
name = "syn"
version = "1.0.109"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "2.0.77"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let entries = parse_registry_entries(content);
        assert_eq!(entries.len(), 1);
        let versions = entries.get("syn").unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions.contains(&"1.0.109".to_string()));
        assert!(versions.contains(&"2.0.77".to_string()));
    }

    #[test]
    fn test_parse_registry_entries_with_build_metadata() {
        // crates.io が build metadata 付きバージョンを返すケース
        // (例: `1.0.0+spec-1.1.0`)。enforce_lock_age_rust などで
        // バージョン文字列をそのまま扱うため、`+` 部分も保持する
        let content = r#"
[[package]]
name = "wasi"
version = "0.11.0+wasi-snapshot-preview1"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let entries = parse_registry_entries(content);
        assert_eq!(entries.len(), 1);
        let versions = entries.get("wasi").unwrap();
        assert_eq!(versions, &vec!["0.11.0+wasi-snapshot-preview1".to_string()]);
    }

    #[test]
    fn test_is_crates_io_source() {
        assert!(is_crates_io_source(
            "registry+https://github.com/rust-lang/crates.io-index"
        ));
        assert!(is_crates_io_source("sparse+https://index.crates.io/"));
        // 私設 registry
        assert!(!is_crates_io_source(
            "registry+https://my.example.com/index"
        ));
        assert!(!is_crates_io_source("sparse+https://my.example.com/index/"));
        // 完全一致だけを真とする
        assert!(!is_crates_io_source("sparse+https://index.crates.io"));
        assert!(!is_crates_io_source(
            "registry+https://github.com/rust-lang/crates.io-index.git"
        ));
        assert!(!is_crates_io_source(
            "git+https://github.com/rust-lang/crates.io-index#0000000000000000000000000000000000000001"
        ));
        assert!(!is_crates_io_source(""));
    }

    #[test]
    fn test_parse_registry_entries_excludes_private_registries() {
        // 私設 registry の同名 crate は crates.io で照会すると別物の公開日で誤判定するため除外する
        let content = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.210"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "serde"
version = "1.0.999"
source = "registry+https://my.example.com/index"

[[package]]
name = "tokio"
version = "1.40.0"
source = "sparse+https://my.example.com/index/"

[[package]]
name = "internal-utils"
version = "0.3.0"
source = "registry+https://my.example.com/index"
"#;
        let entries = parse_registry_entries(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.get("serde").unwrap(), &vec!["1.0.210".to_string()]);
        assert!(!entries.contains_key("tokio"));
        assert!(!entries.contains_key("internal-utils"));
    }

    fn dep_ref(name: &str, version: Option<&str>, source: Option<&str>) -> LockDependencyRef {
        LockDependencyRef {
            name: name.to_string(),
            version: version.map(str::to_string),
            source: source.map(str::to_string),
        }
    }

    #[test]
    fn test_parse_dependency_ref_notations() {
        assert_eq!(
            parse_dependency_ref("serde"),
            Some(dep_ref("serde", None, None))
        );
        assert_eq!(
            parse_dependency_ref("syn 2.0.77"),
            Some(dep_ref("syn", Some("2.0.77"), None))
        );
        assert_eq!(
            parse_dependency_ref("foo 0.1.0 (git+https://github.com/fork/foo.git?branch=main)"),
            Some(dep_ref(
                "foo",
                Some("0.1.0"),
                Some("git+https://github.com/fork/foo.git?branch=main")
            ))
        );
        // build metadata 付きの版もそのまま保持する
        assert_eq!(
            parse_dependency_ref("wasi 0.11.0+wasi-snapshot-preview1"),
            Some(dep_ref("wasi", Some("0.11.0+wasi-snapshot-preview1"), None))
        );
    }

    #[test]
    fn test_parse_dependency_ref_rejects_malformed() {
        // 名前が空
        assert_eq!(parse_dependency_ref(""), None);
        assert_eq!(parse_dependency_ref(" serde"), None);
        // 版が空 (空白が 2 つ続く)
        assert_eq!(parse_dependency_ref("syn  2.0.77"), None);
        // 3 つ目が括弧で囲まれていない
        assert_eq!(
            parse_dependency_ref("foo 1.0.0 registry+https://github.com/rust-lang/crates.io-index"),
            None
        );
        // 括弧の中が空
        assert_eq!(parse_dependency_ref("foo 1.0.0 ()"), None);
    }

    #[test]
    fn test_parse_lock_packages_dependency_notations() {
        // Cargo は曖昧にならない最短の形で依存参照を書く
        let content = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "serde",
 "syn 2.0.77",
 "foo 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)",
]
"#;
        let packages = parse_lock_packages(content);
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].dependencies,
            vec![
                dep_ref("serde", None, None),
                dep_ref("syn", Some("2.0.77"), None),
                dep_ref("foo", Some("1.0.0"), Some(CRATES_IO_LOCK_SOURCE)),
            ]
        );
    }

    #[test]
    fn test_parse_lock_packages_keeps_all_sources_in_file_order() {
        let content = r#"
version = 3

[[package]]
name = "my-app"
version = "0.1.0"
dependencies = [
 "foo 0.1.0 (git+https://github.com/fork/foo.git?branch=main)",
 "my-lib",
 "private-utils",
]

[[package]]
name = "my-lib"
version = "0.2.0"

[[package]]
name = "foo"
version = "0.1.0"
source = "git+https://github.com/fork/foo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"

[[package]]
name = "private-utils"
version = "0.3.0"
source = "sparse+https://my.example.com/index/"
"#;
        let packages = parse_lock_packages(content);
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["my-app", "my-lib", "foo", "private-utils"]);
        // path 依存・workspace member は source なし
        assert_eq!(packages[0].source, None);
        assert_eq!(packages[1].source, None);
        assert!(packages[1].dependencies.is_empty());
        // git source は `#<sha>` ごと保持する (依存参照側には付かない)
        assert_eq!(
            packages[2].source.as_deref(),
            Some(
                "git+https://github.com/fork/foo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"
            )
        );
        assert_eq!(
            packages[0].dependencies[0],
            dep_ref(
                "foo",
                Some("0.1.0"),
                Some("git+https://github.com/fork/foo.git?branch=main")
            )
        );
        // 私設 registry も落とさない
        assert_eq!(
            packages[3].source.as_deref(),
            Some("sparse+https://my.example.com/index/")
        );
    }

    #[test]
    fn test_parse_lock_packages_same_result_for_lock_versions() {
        // `version = 3` / `version = 4` / 旧形式 (version キーなし) で `[[package]]` の読み方は同じ
        let body = r#"
[[package]]
name = "js-sys"
version = "0.3.70"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1868808506b929d7b0cfa8f75951347aa71bb21144b7791bae35d9bccfcfe37a"
dependencies = [
 "wasm-bindgen",
]

[[package]]
name = "wasm-bindgen"
version = "0.2.93"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a82edfc16a6c469f5f44dc7b571814045d60404b55a0ee849f9bcfa2e63dd9b5"
"#;
        let expected = vec![
            LockPackage {
                name: "js-sys".to_string(),
                version: "0.3.70".to_string(),
                source: Some(CRATES_IO_LOCK_SOURCE.to_string()),
                dependencies: vec![dep_ref("wasm-bindgen", None, None)],
            },
            LockPackage {
                name: "wasm-bindgen".to_string(),
                version: "0.2.93".to_string(),
                source: Some(CRATES_IO_LOCK_SOURCE.to_string()),
                dependencies: Vec::new(),
            },
        ];
        for header in ["version = 3\n", "version = 4\n", ""] {
            let content = format!("{header}{body}");
            assert_eq!(
                parse_lock_packages(&content),
                expected,
                "header: {header:?}"
            );
        }
    }

    #[test]
    fn test_parse_lock_packages_invalid_toml_returns_empty() {
        assert!(parse_lock_packages("this is not valid toml {{{").is_empty());
        assert!(parse_lock_packages("version = 4\n").is_empty());
    }

    #[test]
    fn test_parse_lock_packages_skips_unreadable_entries() {
        let content = r#"
[[package]]
name = "no-version"

[[package]]
version = "1.0.0"

[[package]]
name = "ok"
version = "1.0.0"
dependencies = [
 "valid",
 "",
 "double  space",
 "no-parens 1.0.0 registry+https://github.com/rust-lang/crates.io-index",
 42,
]
"#;
        let packages = parse_lock_packages(content);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "ok");
        assert_eq!(packages[0].dependencies, vec![dep_ref("valid", None, None)]);
    }

    #[test]
    fn test_read_lock_packages() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("Cargo.lock");
        std::fs::write(
            &lock_path,
            "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.210\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        )
        .unwrap();
        let packages = read_lock_packages(&lock_path);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "serde");
        // 読めないパスは空
        assert!(read_lock_packages(&dir.path().join("missing.lock")).is_empty());
    }
}
