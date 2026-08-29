//! OSV.dev API による脆弱性チェック
//!
//! 候補バージョンを <https://api.osv.dev/v1/query> に問い合わせ、
//! 既知の脆弱性が報告されていないか判定する。
//!
//! API はパブリックで、認証トークンは不要。

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;

/// OSV.dev API のエンドポイント
const OSV_QUERY_URL: &str = "https://api.osv.dev/v1/query";

/// API 呼び出しのタイムアウト
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// TCP 接続確立のタイムアウト
///
/// 全体タイムアウトとは別に接続確立だけを短く切り、経路が塞がれている環境で
/// 依存 1 件ごとに `REQUEST_TIMEOUT` いっぱい待たされるのを防ぐ。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 単一バージョンの脆弱性チェッカ。
///
/// `reqwest::Client` を共有して keep-alive を効かせる。
#[derive(Debug, Clone)]
pub struct OsvChecker {
    client: Arc<Client>,
}

#[derive(Debug, Serialize)]
struct OsvPackageRef<'a> {
    name: &'a str,
    ecosystem: &'a str,
}

#[derive(Debug, Serialize)]
struct OsvQuery<'a> {
    package: OsvPackageRef<'a>,
    version: &'a str,
}

#[derive(Debug, Deserialize)]
struct OsvVuln {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize, Default)]
struct OsvResponse {
    #[serde(default)]
    vulns: Vec<OsvVuln>,
}

/// 単一の脆弱性チェック結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsvCheck {
    /// 既知の脆弱性なし
    Safe,
    /// 既知の脆弱性あり (1 件以上の ID を保持)
    Vulnerable(Vec<String>),
}

impl OsvChecker {
    /// 新しいチェッカを構築する。
    ///
    /// 失敗時はクライアントが組み立てられなかったエラーを返す。
    pub fn new() -> Result<Self, String> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("depup/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("failed to build OSV HTTP client: {}", e))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    /// `name @ version` が `ecosystem` 上で脆弱と判定されるかを返す。
    ///
    /// API 呼び出しに失敗した場合はエラーメッセージを返す
    /// (呼び出し側で「チェックスキップ」とするかは判断)。
    pub async fn check(
        &self,
        ecosystem: &str,
        name: &str,
        version: &str,
    ) -> Result<OsvCheck, String> {
        let query = OsvQuery {
            package: OsvPackageRef { name, ecosystem },
            version,
        };

        let response = self
            .client
            .post(OSV_QUERY_URL)
            .json(&query)
            .send()
            .await
            .map_err(|e| format!("OSV request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("OSV returned status {}", response.status()));
        }

        let parsed: OsvResponse = response
            .json()
            .await
            .map_err(|e| format!("OSV response parse error: {}", e))?;

        if parsed.vulns.is_empty() {
            Ok(OsvCheck::Safe)
        } else {
            let ids = parsed
                .vulns
                .into_iter()
                .map(|v| v.id)
                .filter(|id| !id.is_empty())
                .collect();
            Ok(OsvCheck::Vulnerable(ids))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_checker_succeeds() {
        let _ = OsvChecker::new().expect("HTTP client should build");
    }

    #[test]
    fn test_response_parse_empty_object() {
        // {} は脆弱性なしとして扱う
        let parsed: OsvResponse = serde_json::from_str("{}").unwrap();
        assert!(parsed.vulns.is_empty());
    }

    #[test]
    fn test_response_parse_empty_vulns_array() {
        let parsed: OsvResponse = serde_json::from_str(r#"{"vulns":[]}"#).unwrap();
        assert!(parsed.vulns.is_empty());
    }

    #[test]
    fn test_response_parse_with_vulns() {
        let json = r#"{"vulns":[{"id":"GHSA-xxxx-yyyy-zzzz"},{"id":"CVE-2024-12345"}]}"#;
        let parsed: OsvResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.vulns.len(), 2);
        assert_eq!(parsed.vulns[0].id, "GHSA-xxxx-yyyy-zzzz");
        assert_eq!(parsed.vulns[1].id, "CVE-2024-12345");
    }

    #[test]
    fn test_query_serialization_shape() {
        let query = OsvQuery {
            package: OsvPackageRef {
                name: "lodash",
                ecosystem: "npm",
            },
            version: "4.17.20",
        };
        let body = serde_json::to_string(&query).unwrap();
        assert!(body.contains("\"name\":\"lodash\""));
        assert!(body.contains("\"ecosystem\":\"npm\""));
        assert!(body.contains("\"version\":\"4.17.20\""));
    }
}
