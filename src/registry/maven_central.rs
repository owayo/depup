//! Maven Central Search API adapter
//!
//! Fetches Java package version information from Maven Central.
//! API endpoint: https://search.maven.org/solrsearch/select
//!
//! Query format: q=g:{groupId}+AND+a:{artifactId}&core=gav&rows=100&wt=json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

/// Maven Central Search API base URL
const MAVEN_CENTRAL_API_URL: &str = "https://search.maven.org/solrsearch/select";

/// Maximum number of versions to fetch
const MAX_VERSIONS: u32 = 100;

/// Maven Central adapter
pub struct MavenCentralAdapter {
    client: HttpClient,
}

/// Maven Central search response
#[derive(Debug, Deserialize)]
struct MavenSearchResponse {
    response: MavenResponseBody,
}

/// Maven Central response body
#[derive(Debug, Deserialize)]
struct MavenResponseBody {
    docs: Vec<MavenVersionDoc>,
}

/// Maven Central version document
#[derive(Debug, Deserialize)]
struct MavenVersionDoc {
    /// Version string
    v: String,
    /// Timestamp in milliseconds since epoch
    timestamp: i64,
}

impl MavenCentralAdapter {
    /// Create a new Maven Central adapter
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Build search URL for group:artifact
    fn build_url(&self, package: &str) -> Result<String, RegistryError> {
        // package format: "group:artifact" (e.g., "org.apache.wicket:wicket-core")
        let parts: Vec<&str> = package.split(':').collect();
        if parts.len() != 2 {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected format 'groupId:artifactId'".to_string(),
            });
        }
        let (group, artifact) = (parts[0], parts[1]);
        Ok(format!(
            "{}?q=g:{}+AND+a:{}&core=gav&rows={}&wt=json",
            MAVEN_CENTRAL_API_URL, group, artifact, MAX_VERSIONS
        ))
    }

    /// Convert timestamp in milliseconds to DateTime<Utc>
    fn timestamp_to_datetime(timestamp_ms: i64) -> Option<DateTime<Utc>> {
        Utc.timestamp_millis_opt(timestamp_ms).single()
    }
}

#[async_trait]
impl RegistryAdapter for MavenCentralAdapter {
    fn language(&self) -> Language {
        Language::Java
    }

    fn registry_name(&self) -> &'static str {
        "Maven Central"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package)?;
        let response: MavenSearchResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for doc in response.response.docs {
            if let Some(released_at) = Self::timestamp_to_datetime(doc.timestamp) {
                versions.push(VersionInfo::new(&doc.v, released_at));
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
    use chrono::Datelike;

    #[test]
    fn test_maven_central_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        assert_eq!(adapter.language(), Language::Java);
    }

    #[test]
    fn test_maven_central_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        assert_eq!(adapter.registry_name(), "Maven Central");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        let url = adapter.build_url("org.apache.wicket:wicket-core").unwrap();
        assert!(url.starts_with("https://search.maven.org/solrsearch/select"));
        assert!(url.contains("q=g:org.apache.wicket+AND+a:wicket-core"));
        assert!(url.contains("core=gav"));
        assert!(url.contains("wt=json"));
    }

    #[test]
    fn test_build_url_invalid_format() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);

        // Missing artifact
        let result = adapter.build_url("org.apache.wicket");
        assert!(result.is_err());

        // Too many parts
        let result = adapter.build_url("a:b:c");
        assert!(result.is_err());
    }

    #[test]
    fn test_timestamp_to_datetime() {
        // 2024-01-15T10:30:00Z = 1705314600000 ms
        let timestamp_ms = 1705314600000_i64;
        let dt = MavenCentralAdapter::timestamp_to_datetime(timestamp_ms).unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn test_timestamp_to_datetime_zero() {
        let dt = MavenCentralAdapter::timestamp_to_datetime(0).unwrap();
        assert_eq!(dt.year(), 1970);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_deserialize_response() {
        let json = r#"
        {
            "response": {
                "docs": [
                    {"v": "9.12.0", "timestamp": 1705314600000},
                    {"v": "9.11.0", "timestamp": 1702722600000}
                ]
            }
        }
        "#;

        let response: MavenSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.response.docs.len(), 2);
        assert_eq!(response.response.docs[0].v, "9.12.0");
        assert_eq!(response.response.docs[0].timestamp, 1705314600000);
    }
}
