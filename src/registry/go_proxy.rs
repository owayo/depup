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
    fn build_info_url(&self, module: &str, version: &str) -> String {
        let encoded_module = Self::encode_module_path(module);
        format!("{}/@v/{}.info", encoded_module, version)
    }

    /// Go Proxy URL 用にモジュールパスをエンコード
    fn encode_module_path(module: &str) -> String {
        // Go Proxy は大文字を !小文字 にエンコードするケースエンコードパスを使用
        let mut encoded = String::with_capacity(module.len() + GO_PROXY_URL.len() + 1);
        encoded.push_str(GO_PROXY_URL);
        encoded.push('/');

        for ch in module.chars() {
            if ch.is_uppercase() {
                encoded.push('!');
                for lower in ch.to_lowercase() {
                    encoded.push(lower);
                }
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
}
