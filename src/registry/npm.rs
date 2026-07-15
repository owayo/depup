//! npm レジストリアダプタ
//!
//! npm レジストリからパッケージバージョン情報を取得する。
//! API エンドポイント: https://registry.npmjs.org/{package}

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::{VersionInfo, compare_semver_versions, is_prerelease_version};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::IgnoredAny;
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
    ///
    /// キー (バージョン文字列) のみ使用する。値は packument 全体のメタデータで
    /// 巨大になりうるため、`IgnoredAny` で読み捨ててメモリ/CPU を節約する。
    versions: HashMap<String, IgnoredAny>,
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

    /// dist-tags.latest との比較に基づき、このバージョンを候補から除外すべきか判定する
    ///
    /// npm は `is_prerelease_version` が検出できない非定型プレリリース
    /// (例: `7.3.0-integration-x.1`) を公式の安定リリースより高いバージョン番号で
    /// 公開していることがあるため、「latest 超かつ安定版に見える」バージョンのみ除外する。
    ///
    /// 一方、canary/beta 等の検出可能なプレリリースは latest 超でも保持する。
    /// プレリリースチャネル利用者 (現在版がプレリリース) が新しいプレリリースへ
    /// 更新できるようにするためで、安定版利用者は judge 側の `stable_candidates` が
    /// プレリリースを除外するため引き続き保護される。
    fn should_skip_version(version: &str, latest: Option<&str>) -> bool {
        let Some(latest) = latest else {
            return false;
        };
        compare_semver_versions(version, latest) == std::cmp::Ordering::Greater
            && !is_prerelease_version(version)
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
        let latest_version = response.dist_tags.get("latest").map(|s| s.as_str());

        let mut versions = Vec::new();

        for (version, _) in response.versions {
            // dist-tags.latest より新しい「安定版に見える」バージョンをスキップ
            // (検出可能なプレリリース (canary/beta 等) は latest 超でも保持する。
            //  詳細は should_skip_version のドキュメントコメントを参照)
            if Self::should_skip_version(&version, latest_version) {
                continue;
            }

            // このバージョンの公開時刻を取得
            if let Some(time_str) = response.time.get(&version)
                && let Ok(released_at) = time_str.parse::<DateTime<Utc>>()
            {
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
        // Prisma スタイルの integration バージョンはフィルタされるべき
        // 公式の "latest" タグより大きいため
        let latest = "7.2.0";
        let prerelease = "7.3.0-integration-fix-6-19-0-cloudflare-accelerate-engine.1";

        // プレリリースバージョンは latest より大きいとみなされるべき
        assert_eq!(
            compare_semver_versions(prerelease, latest),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_stable_version_not_filtered() {
        // latest 以前の安定バージョンはフィルタされないべき
        let latest = "7.2.0";

        // 同じバージョン
        assert_eq!(
            compare_semver_versions("7.2.0", latest),
            std::cmp::Ordering::Equal
        );

        // 古いバージョン
        assert_eq!(
            compare_semver_versions("7.1.0", latest),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_semver_versions("6.0.0", latest),
            std::cmp::Ordering::Less
        );
    }

    /// バグ回帰テスト: latest より新しい「検出可能なプレリリース」(canary/beta 等) は
    /// 候補に保持される。以前は latest 超のバージョンを一律除外していたため、
    /// プレリリースチャネル利用者が新しいプレリリースへ更新できなかった。
    /// (安定版利用者は judge 側の stable_candidates がプレリリースを除外するため保護される)
    #[test]
    fn test_should_skip_keeps_detectable_prerelease_above_latest() {
        let latest = Some("19.2.0");
        assert!(!NpmAdapter::should_skip_version(
            "19.3.0-canary.456",
            latest
        ));
        assert!(!NpmAdapter::should_skip_version("20.0.0-beta.1", latest));
        assert!(!NpmAdapter::should_skip_version("19.3.0-rc.1", latest));
    }

    /// latest より新しい「安定版に見える」バージョンは引き続き除外される
    /// (npm が latest タグを意図的に古い安定版へ向けているケースを尊重する)
    #[test]
    fn test_should_skip_drops_stable_looking_version_above_latest() {
        let latest = Some("19.2.0");
        assert!(NpmAdapter::should_skip_version("19.3.0", latest));
        assert!(NpmAdapter::should_skip_version("20.0.0", latest));
    }

    /// 非定型プレリリース (is_prerelease_version が検出できない形式) は
    /// 従来どおり latest 超で除外される (このフィルタの本来の目的)
    #[test]
    fn test_should_skip_drops_untypical_prerelease_above_latest() {
        let latest = Some("7.2.0");
        assert!(NpmAdapter::should_skip_version(
            "7.3.0-integration-fix-6-19-0-cloudflare-accelerate-engine.1",
            latest
        ));
    }

    /// latest 以下のバージョンは安定版・プレリリースを問わず保持される
    #[test]
    fn test_should_skip_keeps_versions_at_or_below_latest() {
        let latest = Some("19.2.0");
        assert!(!NpmAdapter::should_skip_version("19.2.0", latest));
        assert!(!NpmAdapter::should_skip_version("19.1.0", latest));
        assert!(!NpmAdapter::should_skip_version("19.2.0-canary.1", latest));
    }

    /// dist-tags.latest が存在しない場合は何も除外しない
    #[test]
    fn test_should_skip_without_latest_tag_keeps_everything() {
        assert!(!NpmAdapter::should_skip_version("19.3.0", None));
        assert!(!NpmAdapter::should_skip_version("19.3.0-canary.456", None));
    }

    /// versions の値 (packument メタデータ) は IgnoredAny で読み捨てられ、
    /// キーと dist-tags / time は正しくデシリアライズされる
    #[test]
    fn test_deserialize_npm_response_ignores_version_values() {
        let json = r#"{
            "dist-tags": {"latest": "1.1.0"},
            "time": {
                "created": "2023-01-01T00:00:00Z",
                "1.0.0": "2023-01-01T00:00:00Z",
                "1.1.0": "2023-06-01T00:00:00Z"
            },
            "versions": {
                "1.0.0": {"name": "pkg", "dependencies": {"a": "^1.0.0"}, "dist": {"tarball": "..."}},
                "1.1.0": {"name": "pkg", "dependencies": {"b": "^2.0.0"}, "dist": {"tarball": "..."}}
            }
        }"#;
        let response: NpmPackageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.dist_tags.get("latest").map(|s| s.as_str()),
            Some("1.1.0")
        );
        assert_eq!(response.versions.len(), 2);
        assert!(response.versions.contains_key("1.0.0"));
        assert!(response.versions.contains_key("1.1.0"));
        assert_eq!(response.time.len(), 3);
    }
}
