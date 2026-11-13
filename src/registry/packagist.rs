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

/// p2 の応答の `minified` が示す圧縮形式 (Composer の `MetadataMinifier`)
const MINIFIED_COMPOSER_2: &str = "composer/2.0";

/// 圧縮形式で、直前の版にあったキーが消えたことを表す値
const UNSET_MARKER: &str = "__unset";

/// Packagist レジストリアダプタ
pub struct PackagistAdapter {
    client: HttpClient,
}

/// Packagist API レスポンス形式 (p2 メタデータ API)
#[derive(Debug, Deserialize)]
struct PackagistResponse {
    /// パッケージ名からバージョンリストへのマップ
    packages: HashMap<String, Vec<PackagistVersionInfo>>,
    /// 版の一覧の圧縮形式 (`composer/2.0`)。無ければ各要素が完全な情報を持つ
    #[serde(default)]
    minified: Option<String>,
}

/// Packagist API レスポンスのバージョン情報
///
/// serde はデフォルトで未知の JSON フィールド (`version_normalized` 等) を無視するため、
/// 利用するフィールドだけを宣言する。
#[derive(Debug, Deserialize)]
struct PackagistVersionInfo {
    /// バージョン文字列 (例: "v1.0.0" または "1.0.0")。版ごとに必ず違うので、
    /// 圧縮形式でもどの要素にもある
    version: String,
    /// ISO 8601 形式のリリースタイムスタンプ。圧縮形式では直前の版と同じなら省かれ、
    /// 直前の版にあって消えるなら `"__unset"` になる
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

    /// パッケージ名が Composer の命名規則 (`vendor/package`) に収まっていることを検証する。
    ///
    /// 名前は `build_url` で URL パスへ直接埋め込まれる。`url` crate は WHATWG URL
    /// 仕様どおりドットセグメントを正規化するため、`a/../monolog/monolog` のような
    /// キーがあると別パッケージの版を取得して元のキーへ書き戻してしまう。
    /// `?` / `#` はクエリ・フラグメントとして解釈される。
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        let is_valid_segment = |segment: &str| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        };

        let valid = package
            .split_once('/')
            .is_some_and(|(vendor, name)| is_valid_segment(vendor) && is_valid_segment(name));
        if valid {
            Ok(())
        } else {
            Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected vendor/package with [A-Za-z0-9._-] characters".to_string(),
            })
        }
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

    /// `time` フィールドからリリース日時を解決する
    ///
    /// `time` が欠損またはパース不能な場合は日時不明として保持する。
    fn resolve_released_at(time: Option<&str>) -> Option<DateTime<Utc>> {
        time.and_then(|t| t.parse::<DateTime<Utc>>().ok())
    }

    /// 版の一覧の各要素の公開日時の文字列を、要素と同じ順で返す。
    ///
    /// p2 の応答は `"minified": "composer/2.0"` の圧縮形式で、各要素は直前の要素から
    /// 変わったキーだけを持つ (同じ値のキーは省き、消えたキーは `"__unset"`)。
    /// Composer の `MetadataMinifier::expand` と同じく直前の値を引き継いで読まないと、
    /// 同じコミットに打ったタグのように `time` が直前の版と同じ版の公開日が無いことになる (#164)。
    fn release_times(list: &[PackagistVersionInfo], minified: bool) -> Vec<Option<&str>> {
        let mut inherited: Option<&str> = None;
        list.iter()
            .map(|info| {
                let time = info.time.as_deref();
                if !minified {
                    return time;
                }
                match time {
                    Some(UNSET_MARKER) => inherited = None,
                    Some(time) => inherited = Some(time),
                    None => {}
                }
                inherited
            })
            .collect()
    }

    /// 版の一覧を、dev 版を除いた VersionInfo の昇順の一覧にする。
    ///
    /// 圧縮形式の展開は dev 版も含めた並びのまま行う (dev 版を先に除くと、
    /// その後の版が引き継ぐ値がずれる)。
    fn versions_from_list(list: &[PackagistVersionInfo], minified: bool) -> Vec<VersionInfo> {
        let mut versions: Vec<VersionInfo> = list
            .iter()
            .zip(Self::release_times(list, minified))
            .filter(|(info, _)| !Self::is_dev_version(&info.version))
            .map(|(info, time)| {
                // time が欠けても版自体は残し、age 判定に日付不明を伝える。
                let released_at = Self::resolve_released_at(time);
                VersionInfo::with_released_at(Self::normalize_version(&info.version), released_at)
            })
            .collect();
        versions.sort();
        versions
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
        self.validate_package_name(package)?;

        let url = self.build_url(package);
        let response: PackagistResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        // レスポンス内からパッケージを検索
        // キーは完全なパッケージ名 (vendor/package)。
        // Packagist はリクエスト URL の綴りに関わらず小文字へ正規化したキーで返すため、
        // composer.json 側が大文字混じり (`Laravel/Framework`) の場合に完全一致では
        // 引けない。大小無視のフォールバックを入れて「取得成功なのに候補 0 件 =
        // FetchFailed」と誤報告するのを防ぐ。
        let version_list = response.packages.get(package).or_else(|| {
            response
                .packages
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(package))
                .map(|(_, list)| list)
        });

        let minified = response.minified.as_deref() == Some(MINIFIED_COMPOSER_2);
        Ok(version_list
            .map(|list| Self::versions_from_list(list, minified))
            .unwrap_or_default())
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

    /// 回帰テスト: パッケージ名の URL インジェクションを弾く。
    ///
    /// `a/../monolog/monolog` は `url` crate のドットセグメント正規化で
    /// 別パッケージへ解決され、無関係な版を元のキーへ書き戻してしまう。
    #[test]
    fn test_validate_package_name_rejects_url_injection() {
        let adapter = PackagistAdapter::new(HttpClient::new().unwrap());
        for name in [
            "a/../monolog/monolog",
            "monolog",
            "monolog/monolog/extra",
            "../monolog",
            "vendor/..",
            "vendor/.",
            "vendor/pkg?x=1",
            "vendor/pkg#frag",
            "/pkg",
            "vendor/",
            "",
        ] {
            assert!(
                adapter.validate_package_name(name).is_err(),
                "不正なパッケージ名を受理してはならない: {name:?}"
            );
        }
    }

    #[test]
    fn test_validate_package_name_accepts_composer_names() {
        let adapter = PackagistAdapter::new(HttpClient::new().unwrap());
        for name in [
            "monolog/monolog",
            "symfony/http-foundation",
            "php-amqplib/php-amqplib",
            "laminas/laminas-diactoros",
            "phpunit/php-code-coverage",
        ] {
            assert!(
                adapter.validate_package_name(name).is_ok(),
                "正当なパッケージ名を弾いてはならない: {name:?}"
            );
        }
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

    #[test]
    fn test_resolve_released_at_missing_time_is_unknown() {
        assert_eq!(PackagistAdapter::resolve_released_at(None), None);
    }

    #[test]
    fn test_resolve_released_at_invalid_time_is_unknown() {
        assert_eq!(
            PackagistAdapter::resolve_released_at(Some("not-a-timestamp")),
            None
        );
    }

    /// 回帰テスト (#164): p2 の圧縮形式では、`time` を省いた版は直前の版の公開日時を
    /// 引き継ぎ、`"__unset"` の版は公開日時が無い。dev 版を除いても引き継ぎはずれない。
    #[test]
    fn minified_versions_inherit_the_release_time_of_the_previous_entry() {
        let json = r#"{
            "minified": "composer/2.0",
            "packages": {
                "symfony/deprecation-contracts": [
                    {"version": "v3.8.0", "version_normalized": "3.8.0.0", "time": "2026-06-05T06:23:12+00:00"},
                    {"version": "v3.7.1", "version_normalized": "3.7.1.0"},
                    {"version": "3.7.x-dev", "version_normalized": "3.7.9999999.9999999-dev", "time": "2026-04-13T15:52:40+00:00"},
                    {"version": "v3.7.0", "version_normalized": "3.7.0.0"},
                    {"version": "v3.6.0", "version_normalized": "3.6.0.0", "time": "__unset"},
                    {"version": "v3.5.0", "version_normalized": "3.5.0.0"}
                ]
            }
        }"#;
        let response: PackagistResponse = serde_json::from_str(json).unwrap();
        let minified = response.minified.as_deref() == Some(MINIFIED_COMPOSER_2);
        assert!(minified);
        let versions = PackagistAdapter::versions_from_list(
            &response.packages["symfony/deprecation-contracts"],
            minified,
        );

        let at = |time: &str| Some(time.parse::<DateTime<Utc>>().unwrap());
        let released: Vec<(String, Option<DateTime<Utc>>)> = versions
            .iter()
            .map(|v| (v.version.clone(), v.released_at))
            .collect();
        assert_eq!(
            released,
            vec![
                ("3.5.0".to_string(), None),
                ("3.6.0".to_string(), None),
                ("3.7.0".to_string(), at("2026-04-13T15:52:40+00:00")),
                ("3.7.1".to_string(), at("2026-06-05T06:23:12+00:00")),
                ("3.8.0".to_string(), at("2026-06-05T06:23:12+00:00")),
            ]
        );
    }

    /// 圧縮形式でない応答では、各要素をそのまま読む (`time` の無い版は日時不明)。
    #[test]
    fn unminified_versions_do_not_inherit_release_times() {
        let json = r#"{
            "packages": {
                "vendor/pkg": [
                    {"version": "2.0.0", "time": "2024-01-01T00:00:00+00:00"},
                    {"version": "1.0.0"}
                ]
            }
        }"#;
        let response: PackagistResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.minified, None);
        let versions =
            PackagistAdapter::versions_from_list(&response.packages["vendor/pkg"], false);
        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[0].released_at, None);
        assert!(versions[1].released_at.is_some());
    }

    #[test]
    fn test_resolve_released_at_parses_valid_time() {
        let resolved = PackagistAdapter::resolve_released_at(Some("2023-02-14T15:00:00+00:00"));
        assert_eq!(
            resolved,
            Some(
                "2023-02-14T15:00:00+00:00"
                    .parse::<DateTime<Utc>>()
                    .unwrap()
            )
        );
    }
}
