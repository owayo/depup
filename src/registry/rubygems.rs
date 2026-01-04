//! RubyGems Registry adapter
//!
//! Fetches package version information from the RubyGems registry.
//! API endpoint: https://rubygems.org/api/v1/versions/{gem}.json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;

/// RubyGems registry base URL
const RUBYGEMS_API_URL: &str = "https://rubygems.org/api/v1/versions";

/// RubyGems Registry adapter
pub struct RubyGemsAdapter {
    client: HttpClient,
}

impl RubyGemsAdapter {
    /// Create a new RubyGems adapter
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Build the URL for a gem
    fn build_url(&self, gem: &str) -> String {
        format!("{}/{}.json", RUBYGEMS_API_URL, gem)
    }
}

#[async_trait]
impl RegistryAdapter for RubyGemsAdapter {
    fn language(&self) -> Language {
        Language::Ruby
    }

    fn registry_name(&self) -> &'static str {
        "rubygems"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        // TODO: Implement RubyGems version fetching in Task 5.1
        let _ = (self.build_url(package), &self.client);
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rubygems_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(adapter.language(), Language::Ruby);
    }

    #[test]
    fn test_rubygems_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(adapter.registry_name(), "rubygems");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(
            adapter.build_url("rails"),
            "https://rubygems.org/api/v1/versions/rails.json"
        );
    }
}
