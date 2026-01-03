//! PyPI JSON API adapter
//!
//! Fetches package version information from PyPI.
//! API endpoint: https://pypi.org/pypi/{package}/json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// PyPI API base URL
const PYPI_API_URL: &str = "https://pypi.org/pypi";

/// PyPI adapter
pub struct PyPIAdapter {
    client: HttpClient,
}

/// PyPI package metadata response
#[derive(Debug, Deserialize)]
struct PyPIResponse {
    /// Release information keyed by version
    releases: HashMap<String, Vec<ReleaseInfo>>,
}

/// Release file information
#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    /// Upload time for the release file
    upload_time_iso_8601: Option<String>,
}

impl PyPIAdapter {
    /// Create a new PyPI adapter
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Build the URL for a package
    fn build_url(&self, package: &str) -> String {
        format!("{}/{}/json", PYPI_API_URL, package)
    }
}

#[async_trait]
impl RegistryAdapter for PyPIAdapter {
    fn language(&self) -> Language {
        Language::Python
    }

    fn registry_name(&self) -> &'static str {
        "PyPI"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package);
        let response: PyPIResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for (version, release_files) in response.releases {
            // Get the earliest upload time from release files
            let mut earliest_time: Option<DateTime<Utc>> = None;

            for file_info in release_files {
                if let Some(time_str) = &file_info.upload_time_iso_8601 {
                    if let Ok(time) = time_str.parse::<DateTime<Utc>>() {
                        earliest_time = Some(match earliest_time {
                            Some(current) if time < current => time,
                            Some(current) => current,
                            None => time,
                        });
                    }
                }
            }

            if let Some(released_at) = earliest_time {
                versions.push(VersionInfo::new(&version, released_at));
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
    fn test_pypi_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(adapter.language(), Language::Python);
    }

    #[test]
    fn test_pypi_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(adapter.registry_name(), "PyPI");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(
            adapter.build_url("requests"),
            "https://pypi.org/pypi/requests/json"
        );
    }

    #[test]
    fn test_build_url_with_dashes() {
        let client = HttpClient::new().unwrap();
        let adapter = PyPIAdapter::new(client);
        assert_eq!(
            adapter.build_url("flask-restful"),
            "https://pypi.org/pypi/flask-restful/json"
        );
    }
}
