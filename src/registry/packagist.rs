//! Packagist Registry adapter
//!
//! Fetches package version information from the Packagist registry.
//! API endpoint: https://repo.packagist.org/p2/{vendor}/{package}.json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;

/// Packagist registry base URL
const PACKAGIST_API_URL: &str = "https://repo.packagist.org/p2";

/// Packagist Registry adapter
pub struct PackagistAdapter {
    client: HttpClient,
}

impl PackagistAdapter {
    /// Create a new Packagist adapter
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Build the URL for a package
    /// Package names are in the format vendor/package
    fn build_url(&self, package: &str) -> String {
        format!("{}/{}.json", PACKAGIST_API_URL, package)
    }
}

#[async_trait]
impl RegistryAdapter for PackagistAdapter {
    fn language(&self) -> Language {
        Language::Php
    }

    fn registry_name(&self) -> &'static str {
        "packagist"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        // TODO: Implement Packagist version fetching in Task 5.2
        let _ = (self.build_url(package), &self.client);
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packagist_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(adapter.language(), Language::Php);
    }

    #[test]
    fn test_packagist_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(adapter.registry_name(), "packagist");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(
            adapter.build_url("laravel/framework"),
            "https://repo.packagist.org/p2/laravel/framework.json"
        );
    }
}
