//! パッケージバージョン情報を取得するレジストリアダプタ
//!
//! このモジュールが提供するもの:
//! - リトライロジック付き HTTP クライアント共通基盤
//! - npm レジストリアダプタ
//! - PyPI JSON API アダプタ
//! - crates.io API アダプタ
//! - Go Module Proxy アダプタ
//! - Maven Central アダプタ

mod client;
mod crates_io;
mod git_remote;
mod github_tags;
mod go_proxy;
mod maven_central;
mod npm;
mod packagist;
mod pypi;
mod rubygems;

pub use client::HttpClient;
pub use crates_io::CratesIoAdapter;
pub(crate) use git_remote::redact_url;
pub use git_remote::{GitRemote, GitRemoteError, GitRemoteRefs, parse_ls_remote_output};
pub use github_tags::GitHubTagsAdapter;
pub use go_proxy::GoProxyAdapter;
pub use maven_central::MavenCentralAdapter;
pub use npm::NpmAdapter;
pub use packagist::PackagistAdapter;
pub use pypi::PyPIAdapter;
pub use rubygems::RubyGemsAdapter;

use crate::domain::Language;
use crate::error::RegistryError;
use crate::update::VersionInfo;
use async_trait::async_trait;

/// レジストリアダプタのトレイト
#[async_trait]
pub trait RegistryAdapter: Send + Sync {
    /// このアダプタが扱う言語を取得
    fn language(&self) -> Language;

    /// レジストリ名を取得
    fn registry_name(&self) -> &'static str;

    /// パッケージの利用可能なバージョンを取得
    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError>;
}

/// Maven 座標 (groupId / artifactId) と GitHub owner/repo セグメントの共通文字種検証。
///
/// 空でなく、ASCII 英数字と `.` `-` `_` のみで構成される場合に有効とする。
/// `?` / `#` / `/` / `..` 等の混入による URL クエリ汚染・パストラバーサルを防ぐ
/// (URL インジェクション防止)。
pub(crate) fn is_valid_registry_id_segment(s: &str) -> bool {
    !s.is_empty()
        // `.` / `..` は URL のパス正規化で経路を書き換えるため、`.` を許可文字に
        // 含めている以上ここで明示的に弾く必要がある
        // (例: `https://api.github.com/repos/../swift-nio/tags` → `/swift-nio/tags`)
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_registry_id_segment_valid() {
        // Maven 座標 / GitHub owner/repo で使われる文字種は許可する
        assert!(is_valid_registry_id_segment("org.apache.wicket"));
        assert!(is_valid_registry_id_segment("wicket-core"));
        assert!(is_valid_registry_id_segment("swift_nio2"));
        assert!(is_valid_registry_id_segment("apple"));
    }

    #[test]
    fn test_is_valid_registry_id_segment_invalid() {
        // 空セグメントは不可
        assert!(!is_valid_registry_id_segment(""));
        // URL インジェクションに使える文字は不可
        assert!(!is_valid_registry_id_segment("a?b"));
        assert!(!is_valid_registry_id_segment("a#b"));
        assert!(!is_valid_registry_id_segment("a/b"));
        assert!(!is_valid_registry_id_segment("a b"));
        assert!(!is_valid_registry_id_segment("a:b"));
        // 非 ASCII は不可
        assert!(!is_valid_registry_id_segment("café"));
    }

    /// 回帰テスト: `.` を許可文字に含めているため `..` 単体が素通りし、
    /// `https://api.github.com/repos/../swift-nio/tags` が URL 正規化で
    /// `/swift-nio/tags` へ化けていた (doc が謳うパストラバーサル防御が無効だった)。
    #[test]
    fn test_is_valid_registry_id_segment_rejects_dot_segments() {
        assert!(!is_valid_registry_id_segment("."));
        assert!(!is_valid_registry_id_segment(".."));
        // 通常のドット入り識別子は引き続き許可する
        assert!(is_valid_registry_id_segment("a..b"));
        assert!(is_valid_registry_id_segment("org.apache.wicket"));
    }
}
