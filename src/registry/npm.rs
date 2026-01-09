//! npm Registry adapter
//!
//! Fetches package version information from the npm registry.
//! API endpoint: https://registry.npmjs.org/{package}

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::{compare_versions, VersionInfo};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// npm registry base URL
const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org";

/// npm Registry adapter
pub struct NpmAdapter {
    client: HttpClient,
}

/// npm package metadata response
#[derive(Debug, Deserialize)]
struct NpmPackageResponse {
    /// Distribution tags (latest, next, etc.)
    #[serde(rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    /// Version time information
    time: HashMap<String, String>,
    /// Available versions
    versions: HashMap<String, serde_json::Value>,
}

impl NpmAdapter {
    /// Create a new npm adapter
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Build the URL for a package
    fn build_url(&self, package: &str) -> String {
        format!("{}/{}", NPM_REGISTRY_URL, package)
    }
}

#[async_trait]
impl RegistryAdapter for NpmAdapter {
    fn language(&self) -> Language {
        Language::Node
    }

    fn registry_name(&self) -> &'static str {
        "npm"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package);
        let response: NpmPackageResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        // Get the official "latest" version from dist-tags
        // This is the version npm considers stable
        let latest_version = response.dist_tags.get("latest");

        let mut versions = Vec::new();

        for (version, _) in response.versions {
            // Skip versions newer than dist-tags.latest
            // This handles cases where npm has published pre-release versions
            // (e.g., 7.3.0-integration-...) with version numbers higher than
            // the current stable release (e.g., 7.2.0)
            if let Some(latest) = latest_version {
                if compare_versions(&version, latest) == std::cmp::Ordering::Greater {
                    // This version is newer than the official latest - skip it
                    continue;
                }
            }

            // Get the publish time for this version
            if let Some(time_str) = response.time.get(&version) {
                if let Ok(released_at) = time_str.parse::<DateTime<Utc>>() {
                    versions.push(VersionInfo::new(&version, released_at));
                }
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
    fn test_npm_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = NpmAdapter::new(client);
        assert_eq!(adapter.language(), Language::Node);
    }

    #[test]
    fn test_npm_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = NpmAdapter::new(client);
        assert_eq!(adapter.registry_name(), "npm");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = NpmAdapter::new(client);
        assert_eq!(
            adapter.build_url("lodash"),
            "https://registry.npmjs.org/lodash"
        );
    }

    #[test]
    fn test_build_url_scoped_package() {
        let client = HttpClient::new().unwrap();
        let adapter = NpmAdapter::new(client);
        assert_eq!(
            adapter.build_url("@types/node"),
            "https://registry.npmjs.org/@types/node"
        );
    }

    #[test]
    fn test_prerelease_version_greater_than_latest() {
        // Prisma-style integration versions should be filtered out
        // because they are greater than the official "latest" tag
        let latest = "7.2.0";
        let prerelease = "7.3.0-integration-fix-6-19-0-cloudflare-accelerate-engine.1";

        // The prerelease version should be considered greater than latest
        assert_eq!(
            compare_versions(prerelease, latest),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_stable_version_not_filtered() {
        // Stable versions older than or equal to latest should not be filtered
        let latest = "7.2.0";

        // Same version
        assert_eq!(compare_versions("7.2.0", latest), std::cmp::Ordering::Equal);

        // Older versions
        assert_eq!(compare_versions("7.1.0", latest), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("6.0.0", latest), std::cmp::Ordering::Less);
    }
}
