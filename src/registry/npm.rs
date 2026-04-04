//! npm レジストリアダプタ
//!
//! npm レジストリからパッケージバージョン情報を取得する。
//! API エンドポイント: https://registry.npmjs.org/{package}

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::{VersionInfo, compare_versions};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// npm レジストリのベース URL
const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org";

/// npm レジストリアダプタ
pub struct NpmAdapter {
    client: HttpClient,
}

/// npm パッケージメタデータレスポンス
#[derive(Debug, Deserialize)]
struct NpmPackageResponse {
    /// ディストリビューションタグ (latest, next 等)
    #[serde(rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    /// バージョンごとの公開時刻情報
    time: HashMap<String, String>,
    /// 利用可能なバージョン
    versions: HashMap<String, serde_json::Value>,
}

impl NpmAdapter {
    /// 新しい npm アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// パッケージ用の URL を構築
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

        // dist-tags から公式の "latest" バージョンを取得
        // npm が安定版とみなすバージョン
        let latest_version = response.dist_tags.get("latest");

        let mut versions = Vec::new();

        for (version, _) in response.versions {
            // dist-tags.latest より新しいバージョンをスキップ
            // npm がプレリリースバージョン (例: 7.3.0-integration-...) を
            // 現在の安定リリース (例: 7.2.0) より高いバージョン番号で
            // 公開しているケースに対応
            if let Some(latest) = latest_version
                && compare_versions(&version, latest) == std::cmp::Ordering::Greater
            {
                // This version is newer than the official latest - skip it
                continue;
            }

            // Get the publish time for this version
            if let Some(time_str) = response.time.get(&version)
                && let Ok(released_at) = time_str.parse::<DateTime<Utc>>()
            {
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
