//! crates.io API adapter
//!
//! Fetches crate version information from crates.io.
//! API endpoint: https://crates.io/api/v1/crates/{crate}
//!
//! Note: crates.io requires a User-Agent header (handled by HttpClient)
//! and has rate limiting (1 request/second).

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, Instant};

/// crates.io API base URL
const CRATES_IO_API_URL: &str = "https://crates.io/api/v1/crates";

/// Rate limit: 1 request per second
const RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(1);

/// crates.io adapter with rate limiting
pub struct CratesIoAdapter {
    client: HttpClient,
    rate_limiter: Arc<Semaphore>,
    last_request: std::sync::Mutex<Option<Instant>>,
}

/// crates.io crate response
#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    /// Crate information
    versions: Vec<CrateVersion>,
}

/// Crate version information
#[derive(Debug, Deserialize)]
struct CrateVersion {
    /// Version number
    num: String,
    /// Created at timestamp
    created_at: String,
    /// Whether this version is yanked
    yanked: bool,
}

impl CratesIoAdapter {
    /// Create a new crates.io adapter
    pub fn new(client: HttpClient) -> Self {
        Self {
            client,
            rate_limiter: Arc::new(Semaphore::new(1)),
            last_request: std::sync::Mutex::new(None),
        }
    }

    /// Build the URL for a crate
    fn build_url(&self, crate_name: &str) -> String {
        format!("{}/{}", CRATES_IO_API_URL, crate_name)
    }

    /// Apply rate limiting before making a request
    async fn apply_rate_limit(&self) {
        let _permit = self.rate_limiter.acquire().await.unwrap();

        // Check if we need to wait
        let elapsed = {
            let last_request = self.last_request.lock().unwrap();
            last_request.map(|t| t.elapsed())
        };

        if let Some(elapsed) = elapsed {
            if elapsed < RATE_LIMIT_INTERVAL {
                tokio::time::sleep(RATE_LIMIT_INTERVAL - elapsed).await;
            }
        }

        // Update last request time
        *self.last_request.lock().unwrap() = Some(Instant::now());
    }
}

#[async_trait]
impl RegistryAdapter for CratesIoAdapter {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn registry_name(&self) -> &'static str {
        "crates.io"
    }

    async fn fetch_versions(&self, crate_name: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        // Apply rate limiting
        self.apply_rate_limit().await;

        let url = self.build_url(crate_name);
        let response: CratesIoResponse = self
            .client
            .get_json(&url, crate_name, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for version in response.versions {
            // Skip yanked versions
            if version.yanked {
                continue;
            }

            if let Ok(released_at) = version.created_at.parse::<DateTime<Utc>>() {
                versions.push(VersionInfo::new(&version.num, released_at));
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
    fn test_crates_io_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(adapter.language(), Language::Rust);
    }

    #[test]
    fn test_crates_io_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(adapter.registry_name(), "crates.io");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(
            adapter.build_url("serde"),
            "https://crates.io/api/v1/crates/serde"
        );
    }

    #[test]
    fn test_build_url_with_underscores() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(
            adapter.build_url("serde_json"),
            "https://crates.io/api/v1/crates/serde_json"
        );
    }

    #[test]
    fn test_rate_limit_constants() {
        assert_eq!(RATE_LIMIT_INTERVAL, Duration::from_secs(1));
    }
}
