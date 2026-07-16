//! Maven Central Search API アダプタ
//!
//! Maven Central から Java パッケージのバージョン情報を取得する。
//! API エンドポイント: https://search.maven.org/solrsearch/select
//!
//! クエリ形式: q=g:{groupId}+AND+a:{artifactId}&core=gav&rows=100&wt=json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter, is_valid_registry_id_segment};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

/// Maven Central Search API のベース URL
const MAVEN_CENTRAL_API_URL: &str = "https://search.maven.org/solrsearch/select";

/// 取得するバージョンの最大数
const MAX_VERSIONS: u32 = 100;

/// Maven Central アダプタ
pub struct MavenCentralAdapter {
    client: HttpClient,
}

/// Maven Central 検索レスポンス
#[derive(Debug, Deserialize)]
struct MavenSearchResponse {
    response: MavenResponseBody,
}

/// Maven Central レスポンスボディ
#[derive(Debug, Deserialize)]
struct MavenResponseBody {
    docs: Vec<MavenVersionDoc>,
}

/// Maven Central バージョンドキュメント
#[derive(Debug, Deserialize)]
struct MavenVersionDoc {
    /// バージョン文字列
    v: String,
    /// エポックからのミリ秒タイムスタンプ
    timestamp: i64,
}

impl MavenCentralAdapter {
    /// 新しい Maven Central アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// group:artifact 形式の検索 URL を構築
    fn build_url(&self, package: &str) -> Result<String, RegistryError> {
        // パッケージ形式: "group:artifact" (例: "org.apache.wicket:wicket-core")
        let parts: Vec<&str> = package.split(':').collect();
        if parts.len() != 2 {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected format 'groupId:artifactId'".to_string(),
            });
        }
        let (group, artifact) = (parts[0], parts[1]);

        // Maven coordinates に不正な文字が含まれていないか検証する
        // (URLクエリ文字列へのインジェクション防止。GitHub Tags と共通の検証)
        if !is_valid_registry_id_segment(group) || !is_valid_registry_id_segment(artifact) {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "groupId and artifactId must contain only alphanumeric characters, dots, hyphens, and underscores".to_string(),
            });
        }

        Ok(format!(
            "{}?q=g:{}+AND+a:{}&core=gav&rows={}&wt=json",
            MAVEN_CENTRAL_API_URL, group, artifact, MAX_VERSIONS
        ))
    }

    /// ミリ秒タイムスタンプを DateTime<Utc> に変換
    fn timestamp_to_datetime(timestamp_ms: i64) -> Option<DateTime<Utc>> {
        Utc.timestamp_millis_opt(timestamp_ms).single()
    }
}

#[async_trait]
impl RegistryAdapter for MavenCentralAdapter {
    fn language(&self) -> Language {
        Language::Java
    }

    fn registry_name(&self) -> &'static str {
        "Maven Central"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package)?;
        let response: MavenSearchResponse = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for doc in response.response.docs {
            if let Some(released_at) = Self::timestamp_to_datetime(doc.timestamp) {
                versions.push(VersionInfo::new(&doc.v, released_at));
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
    use chrono::Datelike;

    #[test]
    fn test_maven_central_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        assert_eq!(adapter.language(), Language::Java);
    }

    #[test]
    fn test_maven_central_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        assert_eq!(adapter.registry_name(), "Maven Central");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        let url = adapter.build_url("org.apache.wicket:wicket-core").unwrap();
        assert!(url.starts_with("https://search.maven.org/solrsearch/select"));
        assert!(url.contains("q=g:org.apache.wicket+AND+a:wicket-core"));
        assert!(url.contains("core=gav"));
        assert!(url.contains("wt=json"));
    }

    #[test]
    fn test_build_url_invalid_format() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);

        // アーティファクトなし
        let result = adapter.build_url("org.apache.wicket");
        assert!(result.is_err());

        // パーツが多すぎる
        let result = adapter.build_url("a:b:c");
        assert!(result.is_err());
    }

    #[test]
    fn test_timestamp_to_datetime() {
        // 2024-01-15T10:30:00Z = 1705314600000 ms
        let timestamp_ms = 1705314600000_i64;
        let dt = MavenCentralAdapter::timestamp_to_datetime(timestamp_ms).unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn test_timestamp_to_datetime_zero() {
        let dt = MavenCentralAdapter::timestamp_to_datetime(0).unwrap();
        assert_eq!(dt.year(), 1970);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_deserialize_response() {
        let json = r#"
        {
            "response": {
                "docs": [
                    {"v": "9.12.0", "timestamp": 1705314600000},
                    {"v": "9.11.0", "timestamp": 1702722600000}
                ]
            }
        }
        "#;

        let response: MavenSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.response.docs.len(), 2);
        assert_eq!(response.response.docs[0].v, "9.12.0");
        assert_eq!(response.response.docs[0].timestamp, 1705314600000);
    }
}
