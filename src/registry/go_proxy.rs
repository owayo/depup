//! Go Module Proxy アダプタ
//!
//! Go Module Proxy からモジュールバージョン情報を取得する。
//! API エンドポイント:
//! - バージョン一覧: https://proxy.golang.org/{module}/@v/list
//! - バージョン情報: https://proxy.golang.org/{module}/@v/{version}.info

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
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
    time: String,
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

        let version_strings: Vec<&str> = version_list.lines().collect();

        if version_strings.is_empty() {
            return Ok(Vec::new());
        }

        // 各バージョンについて、リリース時刻を取得するために情報をフェッチ
        let mut versions = Vec::new();

        for version_str in version_strings {
            let version_str = version_str.trim();
            if version_str.is_empty() {
                continue;
            }

            let info_url = self.build_info_url(module, version_str);
            match self
                .client
                .get_json::<VersionInfoResponse>(&info_url, module, self.registry_name())
                .await
            {
                Ok(info) => {
                    if let Ok(released_at) = info.time.parse::<DateTime<Utc>>() {
                        versions.push(VersionInfo::new(&info.version, released_at));
                    }
                }
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
}
