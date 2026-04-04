//! PyPI JSON API アダプタ
//!
//! PyPI からパッケージバージョン情報を取得する。
//! API エンドポイント: https://pypi.org/pypi/{package}/json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// PyPI API のベース URL
const PYPI_API_URL: &str = "https://pypi.org/pypi";

/// PyPI アダプタ
pub struct PyPIAdapter {
    client: HttpClient,
}

/// PyPI パッケージメタデータレスポンス
#[derive(Debug, Deserialize)]
struct PyPIResponse {
    /// バージョンごとのリリース情報
    releases: HashMap<String, Vec<ReleaseInfo>>,
}

/// リリースファイル情報
#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    /// リリースファイルのアップロード時刻
    upload_time_iso_8601: Option<String>,
}

impl PyPIAdapter {
    /// 新しい PyPI アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// パッケージ用の URL を構築
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
            // リリースファイルの中から最も早いアップロード時刻を取得
            let mut earliest_time: Option<DateTime<Utc>> = None;

            for file_info in release_files {
                if let Some(time_str) = &file_info.upload_time_iso_8601
                    && let Ok(time) = time_str.parse::<DateTime<Utc>>()
                {
                    earliest_time = Some(match earliest_time {
                        Some(current) if time < current => time,
                        Some(current) => current,
                        None => time,
                    });
                }
            }

            if let Some(released_at) = earliest_time {
                versions.push(VersionInfo::new(&version, released_at));
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
