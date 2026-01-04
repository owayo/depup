//! RubyGems Registry adapter
//!
//! Fetches package version information from the RubyGems registry.
//! API endpoint: https://rubygems.org/api/v1/versions/{gem}.json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// RubyGems registry base URL
const RUBYGEMS_API_URL: &str = "https://rubygems.org/api/v1/versions";

/// RubyGems Registry adapter
pub struct RubyGemsAdapter {
    client: HttpClient,
}

/// RubyGems version info from API response
#[derive(Debug, Deserialize)]
struct RubyGemsVersionInfo {
    /// Version number (e.g., "7.1.0")
    number: String,
    /// Creation timestamp
    created_at: String,
    /// Platform (usually "ruby")
    #[allow(dead_code)]
    platform: Option<String>,
    /// Whether this version is yanked
    #[serde(default)]
    yanked: bool,
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
        let url = self.build_url(package);
        let response: Vec<RubyGemsVersionInfo> = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for version_info in response {
            // Skip yanked versions
            if version_info.yanked {
                continue;
            }

            // Parse the creation timestamp
            if let Ok(released_at) = version_info.created_at.parse::<DateTime<Utc>>() {
                versions.push(VersionInfo::new(&version_info.number, released_at));
            }
        }

        // Sort by version
        versions.sort();

        Ok(versions)
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

    #[test]
    fn test_build_url_with_dashes() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(
            adapter.build_url("rspec-rails"),
            "https://rubygems.org/api/v1/versions/rspec-rails.json"
        );
    }

    #[test]
    fn test_deserialize_version_info() {
        let json = r#"{"number": "7.1.0", "created_at": "2023-10-05T12:00:00Z", "platform": "ruby", "yanked": false}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.number, "7.1.0");
        assert!(!info.yanked);
    }

    #[test]
    fn test_deserialize_version_info_yanked() {
        let json = r#"{"number": "7.0.0", "created_at": "2023-01-01T00:00:00Z", "yanked": true}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert!(info.yanked);
    }

    #[test]
    fn test_deserialize_version_info_minimal() {
        let json = r#"{"number": "1.0.0", "created_at": "2023-01-01T00:00:00Z"}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.number, "1.0.0");
        assert!(!info.yanked); // defaults to false
    }
}
