//! Go Module Proxy アダプタ
//!
//! Go Module Proxy からモジュールバージョン情報を取得する。
//! API エンドポイント:
//! - バージョン一覧: https://proxy.golang.org/{module}/@v/list
//! - バージョン情報: https://proxy.golang.org/{module}/@v/{version}.info

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::{VersionInfo, compare_semver_versions, is_prerelease_version};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Go Module Proxy のベース URL
const GO_PROXY_URL: &str = "https://proxy.golang.org";

/// Go Module Proxy アダプタ
pub struct GoProxyAdapter {
    client: HttpClient,
}

/// バージョン情報レスポンス
#[derive(Debug, Deserialize)]
struct VersionInfoResponse {
    /// バージョン文字列
    #[serde(rename = "Version")]
    version: String,
    /// バージョンが作成された時刻
    #[serde(rename = "Time")]
    time: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Retraction {
    Exact(String),
    Range { lower: String, upper: String },
}

impl GoProxyAdapter {
    /// 新しい Go Proxy アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// バージョン一覧用の URL を構築
    fn build_list_url(&self, module: &str) -> String {
        // モジュールパスを URL エンコード (大文字小文字を区別しない検索のため)
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/list", encoded_module)
    }

    /// バージョン情報用の URL を構築
    ///
    /// Go Module Proxy プロトコルは `$module` と `$version` の両方を case-encode するため
    /// (https://go.dev/ref/mod#goproxy-protocol)、バージョンにも適用する。
    /// 適用しないと `v1.0.0-RC1` のような大文字入りバージョンの `.info` 取得が
    /// 404 になり、そのバージョンが候補から silent に欠落する。
    fn build_info_url(&self, module: &str, version: &str) -> String {
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/{}.info", encoded_module, Self::case_encode(version))
    }

    /// 指定バージョンの `go.mod` 取得 URL を構築する。
    fn build_mod_url(&self, module: &str, version: &str) -> String {
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/{}.mod", encoded_module, Self::case_encode(version))
    }

    /// タグ付きバージョンがないモジュール向けの `@latest` URL を構築する。
    fn build_latest_url(&self, module: &str) -> String {
        format!("{}/@latest", Self::encode_module_path(module))
    }

    /// Go Proxy URL 用にモジュールパスをエンコード
    fn encode_module_path(module: &str) -> String {
        format!("{}/{}", GO_PROXY_URL, Self::case_encode(module))
    }

    /// Go Module Proxy プロトコルの case-encoding (ASCII 大文字 → `!` + 小文字)
    ///
    /// 大文字小文字を区別しないファイルシステム上での曖昧さを避けるためのエンコードで、
    /// モジュールパスとバージョン文字列の両方に適用される。
    /// Go の仕様では ASCII 大文字のみが対象なので `is_ascii_uppercase` で判定する
    /// (Unicode 大文字まで変換すると仕様外のエンコードになる)。
    fn case_encode(s: &str) -> String {
        let mut encoded = String::with_capacity(s.len());

        for ch in s.chars() {
            if ch.is_ascii_uppercase() {
                encoded.push('!');
                encoded.push(ch.to_ascii_lowercase());
            } else {
                encoded.push(ch);
            }
        }

        encoded
    }
}

/// プロキシのバージョン一覧から、retract 情報を保持する生の最新バージョンを選ぶ。
///
/// Go の `@latest` と同じく、安定版が1件でもあれば最上位の安定版を選び、
/// 安定版がなければ最上位のプレリリースを選ぶ。retract 適用前の一覧を使うため、
/// 最新版が自身を retract している場合も、その `go.mod` を情報源にできる。
fn latest_version_for_retractions<'a>(
    versions: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut latest_release: Option<&str> = None;
    let mut latest_prerelease: Option<&str> = None;

    for version in versions {
        let latest = if is_prerelease_version(version) {
            &mut latest_prerelease
        } else {
            &mut latest_release
        };
        let should_replace = match *latest {
            Some(current) => {
                compare_semver_versions(version, current) == std::cmp::Ordering::Greater
            }
            None => true,
        };
        if should_replace {
            *latest = Some(version);
        }
    }

    latest_release.or(latest_prerelease)
}

/// `go.mod` から単一版・閉区間の retract 指示を抽出する。
fn parse_retractions(content: &str) -> Vec<Retraction> {
    let mut retractions = Vec::new();
    let mut in_retract_block = false;

    for line in content.lines() {
        let logical = line.split("//").next().unwrap_or("").trim();
        if logical.is_empty() {
            continue;
        }

        if logical.starts_with("retract (") || logical == "retract (" {
            in_retract_block = true;
            continue;
        }
        if in_retract_block && logical == ")" {
            in_retract_block = false;
            continue;
        }

        let spec = if in_retract_block {
            logical
        } else if let Some(spec) = logical.strip_prefix("retract ") {
            spec.trim()
        } else {
            continue;
        };

        if let Some(retraction) = parse_retraction_spec(spec) {
            retractions.push(retraction);
        }
    }

    retractions
}

/// retract の単一バージョンまたは閉区間を解釈する。
fn parse_retraction_spec(spec: &str) -> Option<Retraction> {
    let spec = spec.trim();
    if let Some(inner) = spec
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        let (lower, upper) = inner.split_once(',')?;
        let lower = unquote_version(lower.trim())?;
        let upper = unquote_version(upper.trim())?;
        return Some(Retraction::Range { lower, upper });
    }

    unquote_version(spec).map(Retraction::Exact)
}

/// go.mod の ident、二重引用符、raw string で記述されたバージョンを取り出す。
fn unquote_version(version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() {
        return None;
    }
    let is_quoted = version.len() >= 2
        && ((version.starts_with('"') && version.ends_with('"'))
            || (version.starts_with('`') && version.ends_with('`')));
    let unquoted = if is_quoted {
        &version[1..version.len() - 1]
    } else {
        version
    };
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

/// バージョンが retract の単一版または包含範囲に該当するか判定する。
fn is_retracted(version: &str, retractions: &[Retraction]) -> bool {
    retractions.iter().any(|retraction| match retraction {
        Retraction::Exact(retracted) => {
            compare_semver_versions(version, retracted) == std::cmp::Ordering::Equal
        }
        Retraction::Range { lower, upper } => {
            compare_semver_versions(version, lower) != std::cmp::Ordering::Less
                && compare_semver_versions(version, upper) != std::cmp::Ordering::Greater
        }
    })
}

/// Go Proxy の `.info` 応答を共通のバージョン情報へ変換する。
///
/// `Time` はプロキシ仕様上省略可能なため、欠落または不正値なら UNIX epoch を使い、
/// `--age` によって日付不明の正当なバージョンが除外されないようにする。
fn into_version_info(info: VersionInfoResponse) -> VersionInfo {
    let released_at = info
        .time
        .as_deref()
        .and_then(|time| time.parse::<DateTime<Utc>>().ok())
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    VersionInfo::new(info.version, released_at)
}

#[async_trait]
impl RegistryAdapter for GoProxyAdapter {
    fn language(&self) -> Language {
        Language::Go
    }

    fn registry_name(&self) -> &'static str {
        "Go Proxy"
    }

    async fn fetch_versions(&self, module: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        // まずバージョン一覧を取得
        let list_url = self.build_list_url(module);
        let version_list = self
            .client
            .get_text(&list_url, module, self.registry_name())
            .await?;

        let version_strings: Vec<&str> = version_list
            .lines()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .collect();

        if version_strings.is_empty() {
            let latest_url = self.build_latest_url(module);
            let latest = self
                .client
                .get_json::<VersionInfoResponse>(&latest_url, module, self.registry_name())
                .await?;
            let mod_url = self.build_mod_url(module, &latest.version);
            let latest_go_mod = self
                .client
                .get_text(&mod_url, module, self.registry_name())
                .await?;
            let retractions = parse_retractions(&latest_go_mod);
            return if is_retracted(&latest.version, &retractions) {
                Ok(Vec::new())
            } else {
                Ok(vec![into_version_info(latest)])
            };
        }

        let latest_version = latest_version_for_retractions(version_strings.iter().copied())
            .ok_or_else(|| RegistryError::InvalidResponse {
                package: module.to_string(),
                registry: self.registry_name().to_string(),
                message: "retract 情報を取得する最新バージョンを決定できません".to_string(),
            })?;
        let mod_url = self.build_mod_url(module, latest_version);
        let latest_go_mod = self
            .client
            .get_text(&mod_url, module, self.registry_name())
            .await?;
        let retractions = parse_retractions(&latest_go_mod);

        // 各バージョンについて、リリース時刻を取得するために情報をフェッチ
        let mut versions = Vec::new();

        for version_str in version_strings {
            if is_retracted(version_str, &retractions) {
                continue;
            }

            let info_url = self.build_info_url(module, version_str);
            match self
                .client
                .get_json::<VersionInfoResponse>(&info_url, module, self.registry_name())
                .await
            {
                Ok(info) => versions.push(into_version_info(info)),
                Err(_) => {
                    // 特定バージョンの情報が取得できない場合はスキップ
                    continue;
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
    fn test_go_proxy_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(adapter.language(), Language::Go);
    }

    #[test]
    fn test_go_proxy_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(adapter.registry_name(), "Go Proxy");
    }

    #[test]
    fn test_encode_module_path_simple() {
        assert_eq!(
            GoProxyAdapter::encode_module_path("github.com/gin-gonic/gin"),
            "https://proxy.golang.org/github.com/gin-gonic/gin"
        );
    }

    #[test]
    fn test_encode_module_path_with_uppercase() {
        // 大文字は !小文字 にエンコードされるべき
        assert_eq!(
            GoProxyAdapter::encode_module_path("github.com/Azure/azure-sdk-for-go"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go"
        );
    }

    #[test]
    fn test_build_list_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_list_url("github.com/gin-gonic/gin"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/list"
        );
    }

    #[test]
    fn test_build_info_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/gin-gonic/gin", "v1.9.0"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/v1.9.0.info"
        );
    }

    /// バグ回帰テスト: Go Module Proxy プロトコルは `$version` も case-encode する。
    /// 以前はモジュールパスにしか適用していなかったため、`v1.0.0-RC1` のような
    /// 大文字入りバージョンの `.info` 取得が 404 になり候補から silent に欠落していた。
    #[test]
    fn test_case_encode_version_with_uppercase() {
        assert_eq!(GoProxyAdapter::case_encode("v1.0.0-RC1"), "v1.0.0-!r!c1");
    }

    #[test]
    fn test_case_encode_lowercase_unchanged() {
        assert_eq!(GoProxyAdapter::case_encode("v1.9.0"), "v1.9.0");
        assert_eq!(
            GoProxyAdapter::case_encode("v1.2.3-beta.1"),
            "v1.2.3-beta.1"
        );
    }

    #[test]
    fn test_build_info_url_encodes_version_case() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/gin-gonic/gin", "v1.0.0-RC1"),
            "https://proxy.golang.org/github.com/gin-gonic/gin/@v/v1.0.0-!r!c1.info"
        );
    }

    #[test]
    fn test_build_info_url_encodes_both_module_and_version() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_info_url("github.com/Azure/azure-sdk-for-go", "v1.0.0-RC1"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go/@v/v1.0.0-!r!c1.info"
        );
    }

    #[test]
    fn test_build_mod_url_encodes_both_module_and_version() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_mod_url("github.com/Azure/azure-sdk-for-go", "v1.0.0-RC1"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go/@v/v1.0.0-!r!c1.mod"
        );
    }

    #[test]
    fn test_build_latest_url_encodes_module() {
        let client = HttpClient::new().unwrap();
        let adapter = GoProxyAdapter::new(client);
        assert_eq!(
            adapter.build_latest_url("github.com/Azure/azure-sdk-for-go"),
            "https://proxy.golang.org/github.com/!azure/azure-sdk-for-go/@latest"
        );
    }

    #[test]
    fn test_info_without_time_uses_unix_epoch() {
        let response: VersionInfoResponse =
            serde_json::from_str(r#"{"Version":"v0.0.0-20240101000000-abcdef123456"}"#).unwrap();
        let info = into_version_info(response);

        assert_eq!(info.version, "v0.0.0-20240101000000-abcdef123456");
        assert_eq!(info.released_at, DateTime::<Utc>::UNIX_EPOCH);
    }

    #[test]
    fn test_info_with_invalid_time_uses_unix_epoch() {
        let response = VersionInfoResponse {
            version: "v1.0.0".to_string(),
            time: Some("not-a-date".to_string()),
        };
        let info = into_version_info(response);

        assert_eq!(info.released_at, DateTime::<Utc>::UNIX_EPOCH);
    }

    #[test]
    fn test_latest_version_for_retractions_prefers_release() {
        let versions = ["v1.9.0", "v2.0.0-rc.1", "v1.10.0"];
        assert_eq!(latest_version_for_retractions(versions), Some("v1.10.0"));
    }

    #[test]
    fn test_latest_version_for_retractions_uses_prerelease_without_release() {
        let versions = ["v2.0.0-beta.1", "v2.0.0-rc.1"];
        assert_eq!(
            latest_version_for_retractions(versions),
            Some("v2.0.0-rc.1")
        );
    }

    #[test]
    fn test_parse_retractions_supports_single_block_range_and_quotes() {
        let content = r#"
module example.com/lib

retract v1.0.0 // 誤って公開した版
retract (
    [v1.1.0, v1.2.0]
    "v1.3.0"
    [`v1.4.0`, "v1.5.0"]
)
"#;

        assert_eq!(
            parse_retractions(content),
            vec![
                Retraction::Exact("v1.0.0".to_string()),
                Retraction::Range {
                    lower: "v1.1.0".to_string(),
                    upper: "v1.2.0".to_string(),
                },
                Retraction::Exact("v1.3.0".to_string()),
                Retraction::Range {
                    lower: "v1.4.0".to_string(),
                    upper: "v1.5.0".to_string(),
                },
            ]
        );
    }

    #[test]
    fn test_is_retracted_matches_exact_and_inclusive_range() {
        let retractions = vec![
            Retraction::Exact("v1.0.0".to_string()),
            Retraction::Range {
                lower: "v1.2.0".to_string(),
                upper: "v1.4.0".to_string(),
            },
        ];

        assert!(is_retracted("1.0.0", &retractions));
        assert!(is_retracted("v1.2.0", &retractions));
        assert!(is_retracted("v1.3.0", &retractions));
        assert!(is_retracted("v1.4.0", &retractions));
        assert!(!is_retracted("v1.1.0", &retractions));
        assert!(!is_retracted("v1.4.1", &retractions));
    }
}
