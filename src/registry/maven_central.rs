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

/// 1 ページあたりの取得件数
const PAGE_ROWS: u32 = 200;

/// 安全弁: 取得する最大ページ数 (= 最大 2000 版)。
/// GitHub Tags が Link ヘッダを最大 10 ページ辿るのと同じ上限に揃える
const MAX_PAGES: u32 = 10;

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
    /// 全体のヒット件数 (1 ページ分の `docs` より多いことがある)
    #[serde(rename = "numFound", default)]
    num_found: u64,
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

    /// group:artifact 形式の検索 URL を構築 (`start` はページング用のオフセット)
    fn build_url(&self, package: &str, start: u32) -> Result<String, RegistryError> {
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
            "{}?q=g:{}+AND+a:{}&core=gav&rows={}&start={}&wt=json",
            MAVEN_CENTRAL_API_URL, group, artifact, PAGE_ROWS, start
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
        let mut versions = Vec::new();

        // `core=gav` は timestamp 降順で返すため、1 ページで打ち切ると「最新 N 版」しか
        // 得られない。古い系列に固定した依存 (`[1.11,1.12)` / `1.11.+`) の後継版が
        // 候補に 1 件も入らず AlreadyLatest と誤判定されるので numFound まで辿る
        for page in 0..MAX_PAGES {
            let start = page * PAGE_ROWS;
            let url = self.build_url(package, start)?;
            let response: MavenSearchResponse = self
                .client
                .get_json(&url, package, self.registry_name())
                .await?;

            let body = response.response;
            let fetched = body.docs.len() as u64;

            for doc in body.docs {
                if let Some(released_at) = Self::timestamp_to_datetime(doc.timestamp) {
                    versions.push(VersionInfo::new(&doc.v, released_at));
                }
            }

            // このページが空、または全件取り切ったら終了
            if fetched == 0 || u64::from(start) + fetched >= body.num_found {
                break;
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
        let url = adapter
            .build_url("org.apache.wicket:wicket-core", 0)
            .unwrap();
        assert!(url.starts_with("https://search.maven.org/solrsearch/select"));
        assert!(url.contains("q=g:org.apache.wicket+AND+a:wicket-core"));
        assert!(url.contains("core=gav"));
        assert!(url.contains("wt=json"));
        assert!(url.contains("start=0"));
    }

    /// ページングのオフセットが URL に反映される。
    /// `core=gav` は timestamp 降順なので、start を進めないと古い版に到達できない
    #[test]
    fn test_build_url_paging_offset() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);
        let url = adapter.build_url("org.slf4j:slf4j-api", 200).unwrap();
        assert!(url.contains("start=200"), "{url}");
        assert!(url.contains(&format!("rows={}", PAGE_ROWS)), "{url}");
    }

    #[test]
    fn test_build_url_invalid_format() {
        let client = HttpClient::new().unwrap();
        let adapter = MavenCentralAdapter::new(client);

        // アーティファクトなし
        let result = adapter.build_url("org.apache.wicket", 0);
        assert!(result.is_err());

        // パーツが多すぎる
        let result = adapter.build_url("a:b:c", 0);
        assert!(result.is_err());
    }

    /// `numFound` がページ長を超えるレスポンスをデシリアライズできる
    /// (このフィールドを読まないと 1 ページで打ち切って古い版を落とす)
    #[test]
    fn test_response_body_exposes_num_found() {
        let json = r#"{"response":{"numFound":106,"start":0,"docs":[{"v":"2.0.17","timestamp":1705314600000}]}}"#;
        let parsed: MavenSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.response.num_found, 106);
        assert_eq!(parsed.response.docs.len(), 1);
    }

    #[test]
    fn test_timestamp_to_datetime() {
        // 日時変換の例: 2024-01-15T10:30:00Z = 1705314600000ミリ秒
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
