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
use serde_json::Value;
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
    /// 非推奨の判定に必要な値だけを保持し、dependencies / dist 等は読み捨てる。
    versions: HashMap<String, NpmVersionMetadata>,
}

/// lock の監査では latest や deprecated に関わらず、固定された版の公開日を読む。
#[derive(Debug, Deserialize)]
struct NpmPublicationResponse {
    #[serde(default)]
    time: HashMap<String, Value>,
}

impl NpmPublicationResponse {
    fn into_publication_dates(self) -> HashMap<String, DateTime<Utc>> {
        self.time
            .into_iter()
            .filter_map(|(version, value)| {
                let released_at = value.as_str()?.parse().ok()?;
                Some((version, released_at))
            })
            .collect()
    }
}

/// npm の各版のメタデータ。未知フィールドは serde が読み捨てる。
#[derive(Debug, Deserialize)]
struct NpmVersionMetadata {
    /// 作者由来の値なので、型が違っても取得全体を失敗させない。
    #[serde(default)]
    deprecated: Option<Value>,
}

impl NpmVersionMetadata {
    /// npm-pick-manifest と同じ JS の真偽値で非推奨を判定する。
    /// 空文字列は npm deprecate による指定解除を意味する。
    fn is_deprecated(&self) -> bool {
        match &self.deprecated {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(Value::String(message)) => !message.is_empty(),
            Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
            Some(Value::Array(_) | Value::Object(_)) => true,
        }
    }
}

impl NpmPackageResponse {
    /// 更新候補を公開時刻付きのバージョン一覧へ変換する。
    fn into_versions(self) -> Vec<VersionInfo> {
        // dist-tags から公式の "latest" バージョンを取得
        // npm が安定版とみなすバージョン
        let latest_version = self.dist_tags.get("latest").map(|s| s.as_str());

        let mut versions = Vec::new();

        for (version, metadata) in self.versions {
            if metadata.is_deprecated() {
                continue;
            }

            // dist-tags.latest より新しい「安定版に見える」バージョンをスキップ
            // (検出可能なプレリリース (canary/beta 等) は latest 超でも保持する。
            //  詳細は should_skip_version のドキュメントコメントを参照)
            if NpmAdapter::should_skip_version(&version, latest_version) {
                continue;
            }

            // このバージョンの公開時刻を取得
            if let Some(time_str) = self.time.get(&version)
                && let Ok(released_at) = time_str.parse::<DateTime<Utc>>()
            {
                versions.push(VersionInfo::new(&version, released_at));
            }
        }

        // バージョンでソート
        versions.sort();

        versions
    }
}

impl NpmAdapter {
    /// 新しい npm アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// 更新候補の除外規則を通さず、インストール済みの版を照合する公開日を取得する。
    pub async fn fetch_publication_dates(
        &self,
        package: &str,
    ) -> Result<HashMap<String, DateTime<Utc>>, RegistryError> {
        self.validate_package_name(package)?;
        let response: NpmPublicationResponse = self
            .client
            .get_json(&self.build_url(package), package, self.registry_name())
            .await?;
        Ok(response.into_publication_dates())
    }

    /// パッケージ用の URL を構築
    fn build_url(&self, package: &str) -> String {
        format!("{}/{}", NPM_REGISTRY_URL, package)
    }

    /// パッケージ名が npm の命名規則に収まっていることを検証する。
    ///
    /// 名前は `build_url` で URL パスへ直接埋め込まれる。`reqwest` が使う `url`
    /// crate は WHATWG URL 仕様どおりドットセグメントを正規化するため、
    /// `"a/../lodash": "^1.0.0"` のようなキーがあると lodash の版を取得して
    /// `a/../lodash` へ書き戻してしまう。`?` / `#` はクエリ・フラグメントとして
    /// 解釈される。npm alias (`npm:<real>@<range>`) の実名はマニフェストの**値**から
    /// 切り出されるため、名前の出所はキーだけではない点にも注意。
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        let invalid = |reason: &str| RegistryError::InvalidPackageName {
            name: package.to_string(),
            registry: self.registry_name().to_string(),
            reason: reason.to_string(),
        };

        // scope 付き (`@scope/name`) は `/` を 1 個だけ含む
        let (scope, name) = match package.strip_prefix('@') {
            Some(rest) => {
                let (scope, name) = rest
                    .split_once('/')
                    .ok_or_else(|| invalid("scoped package must be @scope/name"))?;
                (Some(scope), name)
            }
            None => (None, package),
        };

        let is_valid_segment = |segment: &str| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '~'))
        };

        if !is_valid_segment(name) || scope.is_some_and(|scope| !is_valid_segment(scope)) {
            return Err(invalid(
                "expected (@scope/)name with [A-Za-z0-9._~-] characters",
            ));
        }
        Ok(())
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
        self.validate_package_name(package)?;

        let url = self.build_url(package);
        let response: NpmPackageResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        Ok(response.into_versions())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_audit_dates_include_deprecated_and_above_latest_versions() {
        let response: NpmPublicationResponse = serde_json::from_value(serde_json::json!({
            "dist-tags": {"latest": "1.0.0"},
            "versions": {
                "1.0.0": {},
                "1.1.0": {"deprecated": "use another release"},
                "2.0.0": {}
            },
            "time": {
                "1.0.0": "2026-09-01T00:00:00Z",
                "1.1.0": "2026-09-30T00:00:00Z",
                "2.0.0": "2026-10-01T00:00:00Z",
                "invalid": "not-a-date",
                "wrong-type": 12
            }
        }))
        .unwrap();
        let dates = response.into_publication_dates();
        assert_eq!(dates.len(), 3);
        assert!(dates.contains_key("1.1.0"));
        assert!(dates.contains_key("2.0.0"));
        assert!(!dates.contains_key("invalid"));
        assert!(!dates.contains_key("wrong-type"));
    }

    #[test]
    fn test_npm_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = NpmAdapter::new(client);
        assert_eq!(adapter.language(), Language::Node);
    }

    /// 回帰テスト: パッケージ名の URL インジェクションを弾く。
    ///
    /// `a/../lodash` は `url` crate のドットセグメント正規化で `lodash` に解決され、
    /// 無関係なパッケージの版を取得して元のキーへ書き戻してしまう。
    #[test]
    fn test_validate_package_name_rejects_url_injection() {
        let adapter = NpmAdapter::new(HttpClient::new().unwrap());
        for name in [
            "a/../lodash",
            "..",
            ".",
            "lodash?x=1",
            "lodash#frag",
            "",
            "a/b",
            "@scope",
            "@scope/",
            "@/name",
            "@scope/a/b",
            "lodash%2f..",
        ] {
            assert!(
                adapter.validate_package_name(name).is_err(),
                "不正なパッケージ名を受理してはならない: {name:?}"
            );
        }
    }

    #[test]
    fn test_validate_package_name_accepts_npm_names() {
        let adapter = NpmAdapter::new(HttpClient::new().unwrap());
        for name in [
            "lodash",
            "typescript",
            "@types/node",
            "@preact/compat",
            "socket.io",
            "left-pad",
            "some_pkg",
        ] {
            assert!(
                adapter.validate_package_name(name).is_ok(),
                "正当なパッケージ名を弾いてはならない: {name:?}"
            );
        }
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

    /// 不要なメタデータは読み捨て、キーと dist-tags / time を保持する。
    #[test]
    fn test_deserialize_npm_response_ignores_unused_metadata() {
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

    /// レジストリの非推奨メタデータを経由して minor 更新先を決める回帰テスト。
    #[test]
    fn test_deprecated_release_is_not_selected_for_minor_update() {
        use crate::domain::{ChangeLevel, UpdateResult};
        use crate::manifest::{ManifestParser, PackageJsonParser};
        use crate::update::{UpdateFilter, UpdateJudge};

        let response: NpmPackageResponse = serde_json::from_value(serde_json::json!({
            "dist-tags": {"latest": "7.0.0"},
            "time": {
                "6.4.6": "2024-01-01T00:00:00Z",
                "6.9.1": "2024-02-01T00:00:00Z",
                "6.10.0": "2024-03-01T00:00:00Z",
                "7.0.0": "2024-04-01T00:00:00Z"
            },
            "versions": {
                "6.4.6": {}, "6.9.1": {},
                "6.10.0": {"deprecated": "Breaking changes; use 6.9.1 or 7.0.0"},
                "7.0.0": {}
            }
        }))
        .unwrap();
        let versions = response.into_versions();
        let deps = PackageJsonParser
            .parse(r#"{"dependencies":{"@testing-library/jest-dom":"^6.4.6"}}"#)
            .unwrap();
        let judge = UpdateJudge::new(UpdateFilter::new().with_max_change(ChangeLevel::Minor));
        assert!(matches!(judge.judge(&deps[0], &versions),
            UpdateResult::Update { new_version, .. } if new_version == "6.9.1"));
        // 現在版が非推奨でも、候補に残った古い版へ下げない。
        let deps = PackageJsonParser
            .parse(r#"{"dependencies":{"@testing-library/jest-dom":"^6.10.0"}}"#)
            .unwrap();
        assert!(judge.judge(&deps[0], &versions).is_skip());
    }

    #[test]
    fn test_all_deprecated_releases_leave_dependency_unchanged() {
        use crate::manifest::{ManifestParser, PackageJsonParser};
        use crate::update::{UpdateFilter, UpdateJudge};

        let response: NpmPackageResponse = serde_json::from_value(serde_json::json!({
            "dist-tags": {"latest": "1.1.0"},
            "time": {"1.0.0": "2024-01-01T00:00:00Z", "1.1.0": "2024-02-01T00:00:00Z"},
            "versions": {
                "1.0.0": {"deprecated": "Unsupported"},
                "1.1.0": {"deprecated": "Unsupported"}
            }
        }))
        .unwrap();
        let versions = response.into_versions();
        assert!(versions.is_empty());
        let deps = PackageJsonParser
            .parse(r#"{"dependencies":{"pkg":"^1.0.0"}}"#)
            .unwrap();
        assert!(
            UpdateJudge::new(UpdateFilter::new())
                .judge(&deps[0], &versions)
                .is_skip()
        );
    }

    /// 指定解除・未指定を保持し、プレリリースでも非推奨なら除外する。
    #[test]
    fn test_deprecation_metadata_without_latest_tag() {
        let response: NpmPackageResponse = serde_json::from_value(serde_json::json!({
            "dist-tags": {},
            "time": {
                "1.0.0": "2024-01-01T00:00:00Z",
                "1.1.0": "2024-02-01T00:00:00Z",
                "1.2.0": "2024-03-01T00:00:00Z",
                "1.3.0-beta.1": "2024-04-01T00:00:00Z",
                "1.3.0-beta.2": "2024-05-01T00:00:00Z",
                "1.4.0": "invalid timestamp",
                "1.6.0": "2024-06-01T00:00:00Z"
            },
            "versions": {
                "1.0.0": {"deprecated": null},
                "1.1.0": {"deprecated": ""},
                "1.2.0": {"dependencies": {"other": "^1"}, "dist": {"tarball": "..."}},
                "1.3.0-beta.1": {"deprecated": "Do not use"},
                "1.3.0-beta.2": {},
                "1.4.0": {}, "1.5.0": {}, "1.6.0": {"deprecated": " "}
            }
        }))
        .unwrap();
        let versions = response.into_versions();
        assert_eq!(
            versions
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["1.0.0", "1.1.0", "1.2.0", "1.3.0-beta.2"]
        );
        assert_eq!(
            versions[1].released_at,
            "2024-02-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn test_deprecation_filter_preserves_latest_and_prerelease_rules() {
        let response: NpmPackageResponse = serde_json::from_value(serde_json::json!({
            "dist-tags": {"latest": "1.1.0"},
            "time": {
                "1.0.0": "2024-01-01T00:00:00Z",
                "1.1.0": "2024-02-01T00:00:00Z",
                "1.2.0": "2024-03-01T00:00:00Z",
                "2.0.0-beta.1": "2024-04-01T00:00:00Z",
                "2.0.0-beta.2": "2024-05-01T00:00:00Z"
            },
            "versions": {
                "1.0.0": {}, "1.1.0": {"deprecated": "Use 1.0.0"},
                "1.2.0": {}, "2.0.0-beta.1": {},
                "2.0.0-beta.2": {"deprecated": "Broken prerelease"}
            }
        }))
        .unwrap();
        assert_eq!(
            response
                .into_versions()
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["1.0.0", "2.0.0-beta.1"]
        );
    }

    #[test]
    fn test_deprecated_values_follow_npm_truthiness() {
        // 型違いの値があっても、同じ応答の通常版は取得できる。
        let mut manifests = serde_json::Map::new();
        let mut time = serde_json::Map::new();
        for (index, deprecated) in [
            Value::Null,
            serde_json::json!(false),
            serde_json::json!(0),
            serde_json::json!(""),
            serde_json::json!(true),
            serde_json::json!(1),
            serde_json::json!(-1),
            serde_json::json!(" "),
            serde_json::json!([]),
            serde_json::json!({}),
        ]
        .into_iter()
        .enumerate()
        {
            let version = format!("1.0.{index}");
            manifests.insert(
                version.clone(),
                serde_json::json!({"deprecated": deprecated}),
            );
            time.insert(version, serde_json::json!("2024-01-01T00:00:00Z"));
        }
        let json =
            serde_json::json!({"dist-tags": {}, "versions": manifests, "time": time}).to_string();
        let response: NpmPackageResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(
            response
                .into_versions()
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["1.0.0", "1.0.1", "1.0.2", "1.0.3"]
        );
    }
}
