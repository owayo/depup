//! Go Module Proxy adapter
//!
//! Fetches module version information from the Go Module Proxy.
//! API endpoints:
//! - List versions: https://proxy.golang.org/{module}/@v/list
//! - Version info: https://proxy.golang.org/{module}/@v/{version}.info

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Go Module Proxy base URL
const GO_PROXY_URL: &str = "https://proxy.golang.org";

/// Go Module Proxy adapter
pub struct GoProxyAdapter {
    client: HttpClient,
}

/// Version info response
#[derive(Debug, Deserialize)]
struct VersionInfoResponse {
    /// Version string
    #[serde(rename = "Version")]
    version: String,
    /// Time when the version was created
    #[serde(rename = "Time")]
    time: String,
}

impl GoProxyAdapter {
    /// Create a new Go Proxy adapter
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Build the URL for listing versions
    fn build_list_url(&self, module: &str) -> String {
        // URL encode the module path (replace / with %2F for case-insensitive lookup)
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/list", encoded_module)
    }

    /// Build the URL for version info
    fn build_info_url(&self, module: &str, version: &str) -> String {
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/{}.info", encoded_module, version)
    }

    /// Encode module path for the Go Proxy URL
    fn encode_module_path(module: &str) -> String {
        // Go Proxy uses case-encoded paths where uppercase letters become !lowercase
        let mut encoded = String::with_capacity(module.len() + GO_PROXY_URL.len() + 1);
        encoded.push_str(GO_PROXY_URL);
        encoded.push('/');

        for ch in module.chars() {
            if ch.is_uppercase() {
                encoded.push('!');
                for lower in ch.to_lowercase() {
                    encoded.push(lower);
                }
            } else {
                encoded.push(ch);
            }
        }

        encoded
    }
}

#[async_trait]
impl RegistryAdapter for GoProxyAdapter {
    fn language(&self) -> Language {
        Language::Go
    }

    fn registry_name(&self) -> &'static str {
        "Go Proxy"
    }

    async fn fetch_versions(&self, module: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        // First, get the list of versions
        let list_url = self.build_list_url(module);
        let version_list = self
            .client
            .get_text(&list_url, module, self.registry_name())
            .await?;

        let version_strings: Vec<&str> = version_list.lines().collect();

        if version_strings.is_empty() {
            return Ok(Vec::new());
        }

        // For each version, fetch the info to get the release time
        let mut versions = Vec::new();

        for version_str in version_strings {
            let version_str = version_str.trim();
            if version_str.is_empty() {
                continue;
            }

            let info_url = self.build_info_url(module, version_str);
            match self
                .client
                .get_json::<VersionInfoResponse>(&info_url, module, self.registry_name())
                .await
            {
                Ok(info) => {
                    if let Ok(released_at) = info.time.parse::<DateTime<Utc>>() {
                        versions.push(VersionInfo::new(&info.version, released_at));
                    }
                }
                Err(_) => {
                    // If we can't get info for a specific version, skip it
                    continue;
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
    fn test_go_proxy_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(adapter.language(), Language::Go);
    }

    #[test]
    fn test_go_proxy_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(adapter.registry_name(), "Go Proxy");
    }

    #[test]
    fn test_encode_module_path_simple() {
        assert_eq!(
            GoProxyAdapter::encode_module_path("github.com/gin-gonic/gin"),
            "https://proxy.golang.org/github.com/gin-gonic/gin"
        );
    }

    #[test]
    fn test_encode_module_path_with_uppercase() {
        // Uppercase letters should be encoded as !lowercase
        assert_eq!(
            GoProxyAdapter::encode_module_path("github.com/Azure/azure-sdk-for-go"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go"
        );
    }

    #[test]
    fn test_build_list_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_list_url("github.com/gin-gonic/gin"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/list"
        );
    }

    #[test]
    fn test_build_info_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/gin-gonic/gin", "v1.9.0"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/v1.9.0.info"
        );
    }
}
