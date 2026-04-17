//! `Cargo.lock` から git 依存の現在のコミットハッシュを抽出する。
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
//! このモジュールは `name` -> `(url, sha)` のマップを返す。

use std::collections::HashMap;
use std::path::Path;
use toml::Value;

/// git 依存のロック情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLockEntry {
    /// 正規化された URL (クエリ・フラグメントなし)
    pub url: String,
    /// 現在のコミットハッシュ (40 文字)
    pub commit: String,
}

/// 指定された `Cargo.lock` パスから git 依存をすべて読み込む
pub fn read_git_entries(path: &Path) -> HashMap<String, GitLockEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_git_entries(&content)
}

/// `Cargo.lock` 文字列から git 依存を抽出する
pub fn parse_git_entries(content: &str) -> HashMap<String, GitLockEntry> {
    let mut result = HashMap::new();
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
            // 同一名の複数バリアント (別の ref) がある場合は最初のものを優先する。
            // 同じ Cargo.toml には同一依存を複数回書けないため通常は問題ない。
            result.entry(name.to_string()).or_insert(entry);
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

        let xojo = entries.get("tree-sitter-xojo").unwrap();
        assert_eq!(xojo.url, "https://github.com/owayo/tree-sitter-xojo.git");
        assert_eq!(xojo.commit, "045c52a6db5390da14d96c0e4804a6208552dc8f");

        let another = entries.get("another-git").unwrap();
        assert_eq!(another.url, "https://github.com/foo/bar.git");
        assert_eq!(another.commit, "0000000000000000000000000000000000000001");

        // registry dep / path dep は含まれない
        assert!(!entries.contains_key("serde"));
        assert!(!entries.contains_key("local"));
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
}
