//! HTTP クライアント共通基盤
//!
//! 共有 HTTP クライアントを提供する:
//! - タイムアウトと User-Agent の設定
//! - 指数バックオフによるリトライ (最大3回)
//! - レート制限のエラーハンドリング

use crate::error::RegistryError;
use reqwest::Client;
use std::time::Duration;

/// HTTP リクエストのデフォルトタイムアウト (30秒)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// デフォルトの User-Agent ヘッダ
const DEFAULT_USER_AGENT: &str = concat!("depup/", env!("CARGO_PKG_VERSION"));

/// 最大リトライ回数
const MAX_RETRIES: u32 = 3;

/// 指数バックオフの基本遅延 (ミリ秒)
const BASE_DELAY_MS: u64 = 100;

/// リトライロジック付き HTTP クライアントラッパー
#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    max_retries: u32,
}

impl HttpClient {
    /// デフォルト設定で HTTP クライアントを作成
    pub fn new() -> Result<Self, RegistryError> {
        Self::with_config(DEFAULT_TIMEOUT, DEFAULT_USER_AGENT)
    }

    /// カスタム設定で HTTP クライアントを作成
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

    /// 最大リトライ回数を設定
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// 内部の reqwest クライアントを取得
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// リトライ付き GET リクエストを実行
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, RegistryError> {
        self.get_with_context(url, "", "").await
    }

    /// エラーコンテキスト付きリトライ GET リクエストを実行
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
                    // レート制限チェック
                    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        last_error = Some(RegistryError::RateLimitExceeded {
                            registry: registry.to_string(),
                        });

                        if attempt < self.max_retries {
                            // 指数バックオフでリトライ
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            delay *= 2;
                            continue;
                        }
                        // 最終リトライでも 429 の場合は RateLimitExceeded を返す
                        return Err(last_error.unwrap());
                    }

                    // 404 Not Found チェック
                    if response.status() == reqwest::StatusCode::NOT_FOUND {
                        return Err(RegistryError::PackageNotFound {
                            package: package.to_string(),
                            registry: registry.to_string(),
                        });
                    }

                    // その他のエラーチェック
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
                    // タイムアウトチェック
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
                        // 指数バックオフでリトライ
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

    /// GET リクエストを実行し JSON レスポンスをパース (パースエラー時リトライ)
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<T, RegistryError> {
        let mut last_error = None;
        let mut delay = BASE_DELAY_MS;

        for attempt in 0..=self.max_retries {
            // レスポンスを取得 (get_with_context 内でリトライ済み)
            let response = match self.get_with_context(url, package, registry).await {
                Ok(resp) => resp,
                Err(e) => return Err(e), // ネットワークエラーは get_with_context 内でリトライ済み
            };

            // JSON パースを試行
            match response.json::<T>().await {
                Ok(parsed) => return Ok(parsed),
                Err(e) => {
                    last_error = Some(RegistryError::InvalidResponse {
                        package: package.to_string(),
                        registry: registry.to_string(),
                        message: format!("failed to parse JSON: {}", e),
                    });

                    if attempt < self.max_retries {
                        // 指数バックオフでリトライ
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                        continue;
                    }
                }
            }
        }

        Err(
            last_error.unwrap_or_else(|| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: registry.to_string(),
                message: "unknown JSON parse error".to_string(),
            }),
        )
    }

    /// GET リクエストを実行しテキストレスポンスを取得 (エラー時リトライ)
    pub async fn get_text(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<String, RegistryError> {
        let mut last_error = None;
        let mut delay = BASE_DELAY_MS;

        for attempt in 0..=self.max_retries {
            // レスポンスを取得 (get_with_context 内でリトライ済み)
            let response = match self.get_with_context(url, package, registry).await {
                Ok(resp) => resp,
                Err(e) => return Err(e), // ネットワークエラーは get_with_context 内でリトライ済み
            };

            // テキスト取得を試行
            match response.text().await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    last_error = Some(RegistryError::InvalidResponse {
                        package: package.to_string(),
                        registry: registry.to_string(),
                        message: format!("failed to get text response: {}", e),
                    });

                    if attempt < self.max_retries {
                        // 指数バックオフでリトライ
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                        continue;
                    }
                }
            }
        }

        Err(
            last_error.unwrap_or_else(|| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: registry.to_string(),
                message: "unknown text parse error".to_string(),
            }),
        )
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
