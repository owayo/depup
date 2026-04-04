//! Packagist レジストリアダプタ
//!
//! Packagist レジストリからパッケージバージョン情報を取得する。
//! API エンドポイント: https://repo.packagist.org/p2/{vendor}/{package}.json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// Packagist レジストリのベース URL
const PACKAGIST_API_URL: &str = "https://repo.packagist.org/p2";

/// Packagist レジストリアダプタ
pub struct PackagistAdapter {
    client: HttpClient,
}

/// Packagist API レスポンス形式 (p2 メタデータ API)
#[derive(Debug, Deserialize)]
struct PackagistResponse {
    /// パッケージ名からバージョンリストへのマップ
    packages: HashMap<String, Vec<PackagistVersionInfo>>,
}

/// Packagist API レスポンスのバージョン情報
#[derive(Debug, Deserialize)]
struct PackagistVersionInfo {
    /// バージョン文字列 (例: "v1.0.0" または "1.0.0")
    version: String,
    /// 比較用の正規化バージョン
    #[allow(dead_code)]
    version_normalized: Option<String>,
    /// ISO 8601 形式のリリースタイムスタンプ
    time: Option<String>,
}

impl PackagistAdapter {
    /// 新しい Packagist アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// パッケージ用の URL を構築
    /// パッケージ名は vendor/package 形式
    fn build_url(&self, package: &str) -> String {
        format!("{}/{}.json", PACKAGIST_API_URL, package)
    }

    /// 'v' プレフィックスがあれば除去してバージョン文字列を正規化
    fn normalize_version(version: &str) -> String {
        version.strip_prefix('v').unwrap_or(version).to_string()
    }

    /// dev または不安定バージョンかどうかを判定
    fn is_dev_version(version: &str) -> bool {
        let lower = version.to_lowercase();
        lower.contains("dev") || lower.contains("-dev")
    }
}

#[async_trait]
impl RegistryAdapter for PackagistAdapter {
    fn language(&self) -> Language {
        Language::Php
    }

    fn registry_name(&self) -> &'static str {
        "packagist"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package);
        let response: PackagistResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        // レスポンス内からパッケージを検索
        // キーは完全なパッケージ名 (vendor/package)
        if let Some(version_list) = response.packages.get(package) {
            for version_info in version_list {
                // dev バージョンをスキップ
                if Self::is_dev_version(&version_info.version) {
                    continue;
                }

                // リリースタイムスタンプをパース
                if let Some(ref time_str) = version_info.time
                    && let Ok(released_at) = time_str.parse::<DateTime<Utc>>()
                {
                    let normalized = Self::normalize_version(&version_info.version);
                    versions.push(VersionInfo::new(&normalized, released_at));
                }
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
    fn test_packagist_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(adapter.language(), Language::Php);
    }

    #[test]
    fn test_packagist_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(adapter.registry_name(), "packagist");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(
            adapter.build_url("laravel/framework"),
            "https://repo.packagist.org/p2/laravel/framework.json"
        );
    }

    #[test]
    fn test_build_url_with_nested_vendor() {
        let client = HttpClient::new().unwrap();
        let adapter = PackagistAdapter::new(client);
        assert_eq!(
            adapter.build_url("symfony/console"),
            "https://repo.packagist.org/p2/symfony/console.json"
        );
    }

    #[test]
    fn test_normalize_version_with_v_prefix() {
        assert_eq!(PackagistAdapter::normalize_version("v1.0.0"), "1.0.0");
    }

    #[test]
    fn test_normalize_version_without_prefix() {
        assert_eq!(PackagistAdapter::normalize_version("1.0.0"), "1.0.0");
    }

    #[test]
    fn test_is_dev_version() {
        assert!(PackagistAdapter::is_dev_version("dev-master"));
        assert!(PackagistAdapter::is_dev_version("dev-main"));
        assert!(PackagistAdapter::is_dev_version("1.0.x-dev"));
        assert!(!PackagistAdapter::is_dev_version("1.0.0"));
        assert!(!PackagistAdapter::is_dev_version("v2.0.0"));
    }

    #[test]
    fn test_deserialize_packagist_response() {
        let json = r#"{
            "packages": {
                "laravel/framework": [
                    {
                        "version": "v10.0.0",
                        "version_normalized": "10.0.0.0",
                        "time": "2023-02-14T15:00:00+00:00"
                    },
                    {
                        "version": "v9.0.0",
                        "version_normalized": "9.0.0.0",
                        "time": "2022-02-08T15:00:00+00:00"
                    }
                ]
            }
        }"#;
        let response: PackagistResponse = serde_json::from_str(json).unwrap();
        assert!(response.packages.contains_key("laravel/framework"));
        let versions = response.packages.get("laravel/framework").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, "v10.0.0");
    }

    #[test]
    fn test_deserialize_version_info() {
        let json = r#"{"version": "v1.0.0", "version_normalized": "1.0.0.0", "time": "2023-01-01T12:00:00+00:00"}"#;
        let info: PackagistVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.version, "v1.0.0");
        assert!(info.time.is_some());
    }

    #[test]
    fn test_deserialize_version_info_minimal() {
        let json = r#"{"version": "1.0.0"}"#;
        let info: PackagistVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.version, "1.0.0");
        assert!(info.time.is_none());
    }
}
