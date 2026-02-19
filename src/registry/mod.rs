//! Registry adapters for fetching package version information
//!
//! This module provides:
//! - HTTP client shared foundation with retry logic
//! - npm Registry adapter
//! - PyPI JSON API adapter
//! - crates.io API adapter
//! - Go Module Proxy adapter
//! - Maven Central adapter

mod client;
mod crates_io;
mod github_tags;
mod go_proxy;
mod maven_central;
mod npm;
mod packagist;
mod pypi;
mod rubygems;

pub use client::HttpClient;
pub use crates_io::CratesIoAdapter;
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

/// Trait for registry adapters
#[async_trait]
pub trait RegistryAdapter: Send + Sync {
    /// Get the language this adapter handles
    fn language(&self) -> Language;

    /// Get the registry name
    fn registry_name(&self) -> &'static str;

    /// Fetch available versions for a package
    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError>;
}

/// Create a registry adapter for the given language
pub fn create_adapter(language: Language, client: HttpClient) -> Box<dyn RegistryAdapter> {
    match language {
        Language::Node => Box::new(NpmAdapter::new(client)),
        Language::Python => Box::new(PyPIAdapter::new(client)),
        Language::Rust => Box::new(CratesIoAdapter::new(client)),
        Language::Go => Box::new(GoProxyAdapter::new(client)),
        Language::Ruby => Box::new(RubyGemsAdapter::new(client)),
        Language::Php => Box::new(PackagistAdapter::new(client)),
        Language::Java => Box::new(MavenCentralAdapter::new(client)),
        Language::Swift => Box::new(GitHubTagsAdapter::new(client)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_adapter_returns_correct_language() {
        let languages = vec![
            (Language::Node, "npm"),
            (Language::Python, "PyPI"),
            (Language::Rust, "crates.io"),
            (Language::Go, "Go Proxy"),
            (Language::Ruby, "rubygems"),
            (Language::Php, "packagist"),
            (Language::Java, "Maven Central"),
            (Language::Swift, "GitHub Tags"),
        ];

        for (lang, expected_registry) in languages {
            let client = HttpClient::new().unwrap();
            let adapter = create_adapter(lang, client);
            assert_eq!(adapter.language(), lang);
            assert_eq!(adapter.registry_name(), expected_registry);
        }
    }
}
