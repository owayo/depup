//! HTTP client shared foundation
//!
//! This module provides a shared HTTP client with:
//! - Configurable timeout and User-Agent
//! - Exponential backoff retry logic (max 3 retries)
//! - Rate limit error handling

use crate::error::RegistryError;
use reqwest::Client;
use std::time::Duration;

/// Default timeout for HTTP requests (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default User-Agent header
const DEFAULT_USER_AGENT: &str = concat!("depup/", env!("CARGO_PKG_VERSION"));

/// Maximum number of retry attempts
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const BASE_DELAY_MS: u64 = 100;

/// HTTP client wrapper with retry logic
#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    max_retries: u32,
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Result<Self, RegistryError> {
        Self::with_config(DEFAULT_TIMEOUT, DEFAULT_USER_AGENT)
    }

    /// Create a new HTTP client with custom configuration
    pub fn with_config(timeout: Duration, user_agent: &str) -> Result<Self, RegistryError> {
        let client = Client::builder()
            .timeout(timeout)
            .user_agent(user_agent)
            .build()
            .map_err(|e| RegistryError::NetworkError {
                package: String::new(),
                registry: "HTTP client".to_string(),
                message: format!("failed to create HTTP client: {}", e),
            })?;

        Ok(Self {
            client,
            max_retries: MAX_RETRIES,
        })
    }

    /// Set the maximum number of retries
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Get the underlying reqwest client
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Perform a GET request with retry logic
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, RegistryError> {
        self.get_with_context(url, "", "").await
    }

    /// Perform a GET request with retry logic and error context
    pub async fn get_with_context(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<reqwest::Response, RegistryError> {
        let mut last_error = None;
        let mut delay = BASE_DELAY_MS;

        for attempt in 0..=self.max_retries {
            match self.client.get(url).send().await {
                Ok(response) => {
                    // Check for rate limiting
                    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        last_error = Some(RegistryError::RateLimitExceeded {
                            registry: registry.to_string(),
                        });

                        if attempt < self.max_retries {
                            // Wait before retrying with exponential backoff
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            delay *= 2;
                            continue;
                        }
                    }

                    // Check for 404 Not Found
                    if response.status() == reqwest::StatusCode::NOT_FOUND {
                        return Err(RegistryError::PackageNotFound {
                            package: package.to_string(),
                            registry: registry.to_string(),
                        });
                    }

                    // Check for other errors
                    if !response.status().is_success() {
                        let status = response.status();
                        return Err(RegistryError::NetworkError {
                            package: package.to_string(),
                            registry: registry.to_string(),
                            message: format!("HTTP {}", status),
                        });
                    }

                    return Ok(response);
                }
                Err(e) => {
                    // Check for timeout
                    if e.is_timeout() {
                        last_error = Some(RegistryError::Timeout {
                            package: package.to_string(),
                            registry: registry.to_string(),
                        });
                    } else {
                        last_error = Some(RegistryError::NetworkError {
                            package: package.to_string(),
                            registry: registry.to_string(),
                            message: e.to_string(),
                        });
                    }

                    if attempt < self.max_retries {
                        // Wait before retrying with exponential backoff
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RegistryError::NetworkError {
            package: package.to_string(),
            registry: registry.to_string(),
            message: "unknown error".to_string(),
        }))
    }

    /// Perform a GET request and parse JSON response with retry on parse errors
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<T, RegistryError> {
        let mut last_error = None;
        let mut delay = BASE_DELAY_MS;

        for attempt in 0..=self.max_retries {
            // First, get the response (this already has its own retry logic)
            let response = match self.get_with_context(url, package, registry).await {
                Ok(resp) => resp,
                Err(e) => return Err(e), // Network errors are already retried in get_with_context
            };

            // Try to parse JSON
            match response.json::<T>().await {
                Ok(parsed) => return Ok(parsed),
                Err(e) => {
                    last_error = Some(RegistryError::InvalidResponse {
                        package: package.to_string(),
                        registry: registry.to_string(),
                        message: format!("failed to parse JSON: {}", e),
                    });

                    if attempt < self.max_retries {
                        // Wait before retrying with exponential backoff
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                        continue;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RegistryError::InvalidResponse {
            package: package.to_string(),
            registry: registry.to_string(),
            message: "unknown JSON parse error".to_string(),
        }))
    }

    /// Perform a GET request and get text response with retry on parse errors
    pub async fn get_text(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<String, RegistryError> {
        let mut last_error = None;
        let mut delay = BASE_DELAY_MS;

        for attempt in 0..=self.max_retries {
            // First, get the response (this already has its own retry logic)
            let response = match self.get_with_context(url, package, registry).await {
                Ok(resp) => resp,
                Err(e) => return Err(e), // Network errors are already retried in get_with_context
            };

            // Try to get text
            match response.text().await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    last_error = Some(RegistryError::InvalidResponse {
                        package: package.to_string(),
                        registry: registry.to_string(),
                        message: format!("failed to get text response: {}", e),
                    });

                    if attempt < self.max_retries {
                        // Wait before retrying with exponential backoff
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                        continue;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RegistryError::InvalidResponse {
            package: package.to_string(),
            registry: registry.to_string(),
            message: "unknown text parse error".to_string(),
        }))
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("failed to create default HTTP client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_creation() {
        let client = HttpClient::new();
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_client_with_config() {
        let client = HttpClient::with_config(Duration::from_secs(60), "test-agent/1.0");
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_client_with_max_retries() {
        let client = HttpClient::new().unwrap().with_max_retries(5);
        assert_eq!(client.max_retries, 5);
    }

    #[test]
    fn test_http_client_default() {
        let client = HttpClient::default();
        assert_eq!(client.max_retries, MAX_RETRIES);
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
        assert!(DEFAULT_USER_AGENT.starts_with("depup/"));
        assert_eq!(MAX_RETRIES, 3);
        assert_eq!(BASE_DELAY_MS, 100);
    }
}
