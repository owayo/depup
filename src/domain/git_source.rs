//! Git 依存関係の情報型定義。
//!
//! Cargo.toml など一部のマニフェストで利用される git 依存を表す。

use serde::{Deserialize, Serialize};
use std::fmt;

/// Git 依存が参照する対象種別
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GitReference {
    /// ブランチ指定 (例: `branch = "main"`)
    Branch(String),
    /// タグ指定 (例: `tag = "v1.2.3"`)
    Tag(String),
    /// リビジョン固定 (例: `rev = "abc123"`)
    Rev(String),
    /// リファレンス省略 (= デフォルトブランチ)
    DefaultBranch,
}

impl GitReference {
    /// 人間向けの簡易表示名を返す (例: `branch=main`, `tag=v1.2.3`)
    pub fn display_name(&self) -> String {
        match self {
            GitReference::Branch(b) => format!("branch={}", b),
            GitReference::Tag(t) => format!("tag={}", t),
            GitReference::Rev(r) => {
                let short = r.chars().take(8).collect::<String>();
                format!("rev={}", short)
            }
            GitReference::DefaultBranch => "branch=HEAD".to_string(),
        }
    }

    /// このリファレンスが固定 (rev) かどうか
    pub fn is_pinned(&self) -> bool {
        matches!(self, GitReference::Rev(_))
    }

    /// branch/tag 名などの raw 値を返す (DefaultBranch は None)
    pub fn raw_value(&self) -> Option<&str> {
        match self {
            GitReference::Branch(v) | GitReference::Tag(v) | GitReference::Rev(v) => Some(v),
            GitReference::DefaultBranch => None,
        }
    }
}

impl fmt::Display for GitReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// マニフェストで宣言された git 依存
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSource {
    /// リポジトリ URL (例: `https://github.com/owner/repo.git`)
    pub url: String,
    /// リファレンス種別
    pub reference: GitReference,
    /// ロックファイルから取得した現在のコミットハッシュ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_commit: Option<String>,
}

impl GitSource {
    /// 新しい `GitSource` を作る
    pub fn new(url: impl Into<String>, reference: GitReference) -> Self {
        Self {
            url: url.into(),
            reference,
            current_commit: None,
        }
    }

    /// 現在のコミットハッシュをセットする (ビルダ)
    pub fn with_current_commit(mut self, commit: impl Into<String>) -> Self {
        self.current_commit = Some(commit.into());
        self
    }

    /// 短縮コミットハッシュ (8 文字) を返す
    pub fn short_current_commit(&self) -> Option<String> {
        self.current_commit
            .as_deref()
            .map(|c| c.chars().take(8).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_reference_display_name() {
        assert_eq!(
            GitReference::Branch("main".into()).display_name(),
            "branch=main"
        );
        assert_eq!(
            GitReference::Tag("v1.2.3".into()).display_name(),
            "tag=v1.2.3"
        );
        assert_eq!(
            GitReference::Rev("abcdef0123456789".into()).display_name(),
            "rev=abcdef01"
        );
        assert_eq!(GitReference::DefaultBranch.display_name(), "branch=HEAD");
    }

    #[test]
    fn test_git_reference_is_pinned() {
        assert!(GitReference::Rev("abc".into()).is_pinned());
        assert!(!GitReference::Branch("main".into()).is_pinned());
        assert!(!GitReference::Tag("v1".into()).is_pinned());
        assert!(!GitReference::DefaultBranch.is_pinned());
    }

    #[test]
    fn test_git_reference_raw_value() {
        assert_eq!(
            GitReference::Branch("main".into()).raw_value(),
            Some("main")
        );
        assert_eq!(
            GitReference::Tag("v1.2.3".into()).raw_value(),
            Some("v1.2.3")
        );
        assert_eq!(GitReference::Rev("abc".into()).raw_value(), Some("abc"));
        assert_eq!(GitReference::DefaultBranch.raw_value(), None);
    }

    #[test]
    fn test_git_source_new() {
        let src = GitSource::new(
            "https://github.com/owner/repo.git",
            GitReference::Branch("main".into()),
        );
        assert_eq!(src.url, "https://github.com/owner/repo.git");
        assert_eq!(src.reference, GitReference::Branch("main".into()));
        assert!(src.current_commit.is_none());
    }

    #[test]
    fn test_git_source_with_current_commit() {
        let src = GitSource::new("https://example.com/r.git", GitReference::DefaultBranch)
            .with_current_commit("abcdef0123456789");
        assert_eq!(src.current_commit.as_deref(), Some("abcdef0123456789"));
        assert_eq!(src.short_current_commit().as_deref(), Some("abcdef01"));
    }

    #[test]
    fn test_serde_roundtrip_branch() {
        let src = GitSource::new(
            "https://github.com/owner/repo.git",
            GitReference::Branch("main".into()),
        )
        .with_current_commit("0123456789abcdef");

        let json = serde_json::to_string(&src).unwrap();
        let parsed: GitSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, src);
    }

    #[test]
    fn test_serde_roundtrip_default_branch_skips_commit() {
        // current_commit が None の場合は JSON に出現しない
        let src = GitSource::new("https://example.com/a.git", GitReference::DefaultBranch);
        let json = serde_json::to_string(&src).unwrap();
        assert!(!json.contains("current_commit"));
    }
}
