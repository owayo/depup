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
