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

mod policy;
pub(crate) use policy::Assessment;
pub use policy::{Advisory, AdvisoryGroup, RiskTradeoff, Severity};

type AdvisoryCache = std::collections::HashMap<(String, String, String), Vec<Advisory>>;
#[cfg(test)]
type MockAdvisories = std::collections::HashMap<String, Result<Vec<Advisory>, String>>;

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
    cache: Arc<tokio::sync::Mutex<AdvisoryCache>>,
    #[cfg(test)]
    mock_versions: Option<Arc<MockAdvisories>>,
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
    #[serde(default)]
    aliases: serde_json::Value,
    #[serde(default)]
    severity: serde_json::Value,
    #[serde(default)]
    affected: serde_json::Value,
    #[serde(default)]
    database_specific: serde_json::Value,
    #[serde(default)]
    withdrawn: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OsvResponse {
    #[serde(default)]
    vulns: Vec<OsvVuln>,
}

impl OsvVuln {
    fn into_advisory(self, ecosystem: &str, name: &str) -> Advisory {
        let ratings = |value: &serde_json::Value| -> Vec<policy::RawSeverity> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| {
                    Some(policy::RawSeverity {
                        kind: value.get("type")?.as_str()?.into(),
                        score: value.get("score")?.as_str()?.into(),
                    })
                })
                .collect()
        };
        let package_ratings: Vec<_> = self
            .affected
            .as_array()
            .into_iter()
            .flatten()
            .filter(|affected| {
                affected
                    .pointer("/package/ecosystem")
                    .and_then(serde_json::Value::as_str)
                    == Some(ecosystem)
                    && affected
                        .pointer("/package/name")
                        .and_then(serde_json::Value::as_str)
                        == Some(name)
            })
            .flat_map(|affected| ratings(&affected["severity"]))
            .collect();
        let severity = if package_ratings.is_empty() {
            policy::severity(
                self.database_specific
                    .get("severity")
                    .and_then(serde_json::Value::as_str),
                &ratings(&self.severity),
            )
        } else {
            policy::severity(None, &package_ratings)
        };
        Advisory {
            id: self.id,
            aliases: self
                .aliases
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
            severity,
        }
    }
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
            cache: Arc::new(tokio::sync::Mutex::new(AdvisoryCache::new())),
            #[cfg(test)]
            mock_versions: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn mock(versions: &[(&str, Result<OsvCheck, String>)]) -> Self {
        Self::mock_advisories(
            &versions
                .iter()
                .map(|(version, answer)| {
                    (
                        *version,
                        answer.clone().map(|check| match check {
                            OsvCheck::Safe => Vec::new(),
                            OsvCheck::Vulnerable(ids) => {
                                if ids.is_empty() {
                                    vec![Advisory {
                                        id: String::new(),
                                        aliases: Vec::new(),
                                        severity: Severity::Unknown,
                                    }]
                                } else {
                                    ids.into_iter()
                                        .map(|id| Advisory {
                                            id,
                                            aliases: Vec::new(),
                                            severity: Severity::Unknown,
                                        })
                                        .collect()
                                }
                            }
                        }),
                    )
                })
                .collect::<Vec<_>>(),
        )
    }

    #[cfg(test)]
    pub(crate) fn mock_advisories(versions: &[(&str, Result<Vec<Advisory>, String>)]) -> Self {
        let mut checker = Self::new().unwrap();
        checker.mock_versions = Some(Arc::new(
            versions
                .iter()
                .map(|(v, answer)| (v.to_string(), answer.clone()))
                .collect(),
        ));
        checker
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
        let records = self.advisories(ecosystem, name, version).await?;
        Ok(if records.is_empty() {
            OsvCheck::Safe
        } else {
            OsvCheck::Vulnerable(
                records
                    .into_iter()
                    .map(|record| record.id)
                    .filter(|id| !id.is_empty())
                    .collect(),
            )
        })
    }

    pub(crate) async fn advisories(
        &self,
        ecosystem: &str,
        name: &str,
        version: &str,
    ) -> Result<Vec<Advisory>, String> {
        #[cfg(test)]
        if let Some(versions) = &self.mock_versions {
            return versions.get(version).cloned().unwrap_or_else(|| {
                Err(format!(
                    "unexpected OSV query: {ecosystem}/{name}@{version}"
                ))
            });
        }
        let key = (ecosystem.to_owned(), name.to_owned(), version.to_owned());
        if let Some(cached) = self.cache.lock().await.get(&key) {
            return Ok(cached.clone());
        }
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

        let records = parsed
            .vulns
            .into_iter()
            .filter(|record| record.withdrawn.is_none())
            .map(|record| record.into_advisory(ecosystem, name))
            .collect::<Vec<_>>();
        self.cache.lock().await.insert(key, records.clone());
        Ok(records)
    }

    /// 複数版が共存する lock では、パッケージ全体の前後の集合を比べる。
    /// 古い版に問題が残っていれば、別の版を追加しただけで「解消」と数えない。
    pub(crate) async fn assess_versions(
        &self,
        ecosystem: &str,
        name: &str,
        before: &[String],
        after: &[String],
        allow_tradeoff: bool,
    ) -> Result<Assessment, String> {
        let mut candidate = Vec::new();
        for version in after {
            candidate.extend(self.advisories(ecosystem, name, version).await?);
        }
        if candidate.is_empty() {
            return Ok(Assessment::Safe);
        }
        let mut baseline = Vec::new();
        if allow_tradeoff && !before.is_empty() {
            for version in before {
                match self.advisories(ecosystem, name, version).await {
                    Ok(records) => baseline.extend(records),
                    Err(error) => {
                        return Ok(Assessment::Rejected {
                            advisories: candidate,
                            baseline_error: Some(error),
                        });
                    }
                }
            }
            if let Some(mut report) = policy::tradeoff(&baseline, &candidate) {
                report.baseline_versions = before.to_vec();
                report.candidate_versions = after.to_vec();
                return Ok(Assessment::Tradeoff(report));
            }
        }
        Ok(Assessment::Rejected {
            advisories: candidate,
            baseline_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_severity_overrides_top_level_and_malformed_metadata_stays_unknown() {
        let parsed: OsvVuln = serde_json::from_value(serde_json::json!({
            "id": "EXAMPLE", "database_specific": {"severity": "CRITICAL"},
            "affected": [{"package": {"ecosystem": "npm", "name": "example"},
                "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N"}]}]
        })).unwrap();
        assert_eq!(
            parsed.into_advisory("npm", "example").severity,
            Severity::Medium
        );
        let parsed: OsvVuln = serde_json::from_value(serde_json::json!({
            "id": "EXAMPLE", "aliases": {}, "severity": [{"type": 7, "score": null}],
            "database_specific": {"severity": []}
        }))
        .unwrap();
        assert_eq!(
            parsed.into_advisory("npm", "example").severity,
            Severity::Unknown
        );
    }

    #[tokio::test]
    async fn baseline_lookup_failure_never_grants_a_tradeoff() {
        let checker = OsvChecker::mock_advisories(&[
            ("1.0.0", Err("baseline unavailable".into())),
            (
                "1.1.0",
                Ok(vec![Advisory {
                    id: "NEW".into(),
                    aliases: vec![],
                    severity: Severity::Low,
                }]),
            ),
        ]);
        assert!(matches!(
            checker
                .assess_versions("npm", "example", &["1.0.0".into()], &["1.1.0".into()], true)
                .await
                .unwrap(),
            Assessment::Rejected {
                baseline_error: Some(_),
                ..
            }
        ));
    }

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
