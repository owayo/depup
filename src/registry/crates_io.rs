//! crates.io API アダプタ
//!
//! crates.io からクレートのバージョン情報を取得する。
//! API エンドポイント: https://crates.io/api/v1/crates/{crate}
//!
//! 注意: crates.io は User-Agent ヘッダが必要 (HttpClient で処理済み)
//! かつレート制限あり (1リクエスト/秒)。

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

/// crates.io API のベース URL
const CRATES_IO_API_URL: &str = "https://crates.io/api/v1/crates";

/// レート制限: 1リクエスト/秒
const RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(1);

/// レート制限付き crates.io アダプタ
pub struct CratesIoAdapter {
    client: HttpClient,
    rate_limiter: Arc<Semaphore>,
    last_request: std::sync::Mutex<Option<Instant>>,
}

/// crates.io クレートレスポンス
#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    /// クレート情報
    versions: Vec<CrateVersion>,
}

/// クレートバージョン情報
#[derive(Debug, Deserialize)]
struct CrateVersion {
    /// バージョン番号
    num: String,
    /// 作成日時タイムスタンプ
    created_at: String,
    /// このバージョンが yank されているか
    yanked: bool,
}

impl CratesIoAdapter {
    /// 新しい crates.io アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self {
            client,
            rate_limiter: Arc::new(Semaphore::new(1)),
            last_request: std::sync::Mutex::new(None),
        }
    }

    /// クレート用の URL を構築
    fn build_url(&self, crate_name: &str) -> String {
        format!("{}/{}", CRATES_IO_API_URL, crate_name)
    }

    /// リクエスト前にレート制限を適用
    async fn apply_rate_limit(&self) {
        let _permit = self.rate_limiter.acquire().await.unwrap();

        // 待機が必要か確認
        let elapsed = {
            let last_request = self.last_request.lock().unwrap();
            last_request.map(|t| t.elapsed())
        };

        if let Some(elapsed) = elapsed
            && elapsed < RATE_LIMIT_INTERVAL
        {
            tokio::time::sleep(RATE_LIMIT_INTERVAL - elapsed).await;
        }

        // 最終リクエスト時刻を更新
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
        // レート制限を適用
        self.apply_rate_limit().await;

        let url = self.build_url(crate_name);
        let response: CratesIoResponse = self
            .client
            .get_json(&url, crate_name, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for version in response.versions {
            // yank されたバージョンをスキップ
            if version.yanked {
                continue;
            }

            if let Ok(released_at) = version.created_at.parse::<DateTime<Utc>>() {
                versions.push(VersionInfo::new(&version.num, released_at));
            }
        }

        // バージョンでソート
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
