//! crates.io API アダプタ
//!
//! crates.io からクレートのバージョン情報を取得する。
//! API エンドポイント: https://crates.io/api/v1/crates/{crate}
//!
//! 注意: crates.io は User-Agent ヘッダが必要 (HttpClient で処理済み)
//! かつレート制限あり (1リクエスト/秒)。

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, Instant};

/// crates.io API のベース URL
const CRATES_IO_API_URL: &str = "https://crates.io/api/v1/crates";

/// レート制限: 1リクエスト/秒
const RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(1);

/// crates.io の 1 リクエスト/秒 制限を保持する共有状態。
///
/// crawler policy はクライアント全体に対して間隔を求めるため、状態をアダプタの
/// インスタンスに閉じ込めるとマニフェスト境界やフェーズ境界 (check → post-install
/// の lock 監査) で間隔がリセットされ、直前のリクエストから 1 秒経たずに次が飛ぶ。
/// `Orchestrator` が 1 つ持って全アダプタへ配ることで、実行全体で間隔を守る。
#[derive(Debug)]
pub struct CratesIoRateLimit {
    /// 同時実行を 1 に絞るセマフォ (間隔の判定と更新を直列化する)
    semaphore: Semaphore,
    /// 直近のリクエスト時刻
    last_request: std::sync::Mutex<Option<Instant>>,
}

impl CratesIoRateLimit {
    /// 新しいレート制限状態を作る
    pub fn new() -> Self {
        Self {
            semaphore: Semaphore::new(1),
            last_request: std::sync::Mutex::new(None),
        }
    }
}

impl Default for CratesIoRateLimit {
    fn default() -> Self {
        Self::new()
    }
}

/// レート制限付き crates.io アダプタ
pub struct CratesIoAdapter {
    client: HttpClient,
    rate_limit: Arc<CratesIoRateLimit>,
}

/// crates.io クレートレスポンス
#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    /// クレート情報
    versions: Vec<CrateVersion>,
}

impl CratesIoResponse {
    fn into_versions(self, include_yanked: bool) -> Vec<VersionInfo> {
        let mut versions = Vec::new();
        for version in self.versions {
            if version.yanked && !include_yanked {
                continue;
            }
            if let Ok(released_at) = version.created_at.parse::<DateTime<Utc>>() {
                let mut info = VersionInfo::new(&version.num, released_at);
                info.publisher = version.publisher();
                versions.push(info);
            }
        }
        versions.sort();
        versions
    }
}

/// Sparse index の公開日は yank によって変わらない。
#[derive(Deserialize)]
struct IndexRelease {
    name: String,
    vers: String,
    #[serde(default)]
    pubtime: Option<String>,
}

fn index_path(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

fn index_release_dates(content: &str, package: &str) -> Option<Vec<VersionInfo>> {
    let mut versions = Vec::new();
    for line in content.lines() {
        let release: IndexRelease = serde_json::from_str(line).ok()?;
        if !release.name.eq_ignore_ascii_case(package) {
            return None;
        }
        if let Some(date) = release
            .pubtime
            .and_then(|date| date.parse::<DateTime<Utc>>().ok())
        {
            versions.push(VersionInfo::new(release.vers, date));
        }
    }
    Some(versions)
}

/// クレートバージョン情報
#[derive(Debug, Deserialize)]
struct CrateVersion {
    /// バージョン番号
    num: String,
    /// 作成日時タイムスタンプ
    created_at: String,
    /// このバージョンが yank されているか
    yanked: bool,
    #[serde(default)]
    published_by: serde_json::Value,
    #[serde(default)]
    trustpub_data: serde_json::Value,
}

impl CrateVersion {
    fn publisher(&self) -> crate::update::PublisherEvidence {
        use crate::update::PublisherEvidence;
        if self.trustpub_data["provider"].as_str() == Some("github")
            && let Some(repository) = self.trustpub_data["repository"].as_str()
            && let Some((owner, repo)) = repository.split_once('/')
            && crate::update::age_policy::valid_github_login(owner)
            && crate::registry::is_valid_registry_id_segment(repo)
        {
            return PublisherEvidence::GithubTrustedPublisher {
                owner: owner.into(),
            };
        }
        // Trusted Publishing の実行元が不明なら、設定を作った利用者へ帰属させない。
        if !self.trustpub_data.is_null() {
            return PublisherEvidence::Unknown;
        }
        if self.published_by["github_username_matches"].as_bool() == Some(true)
            && let Some(login) = self.published_by["login"].as_str()
            && crate::update::age_policy::valid_github_login(login)
        {
            return PublisherEvidence::GithubUser {
                login: login.into(),
            };
        }
        PublisherEvidence::Unknown
    }
}

impl CratesIoAdapter {
    /// 新しい crates.io アダプタを作成する (レート制限状態は専有)
    ///
    /// 実行全体で間隔を守りたい場合は `with_rate_limit` で状態を共有すること。
    pub fn new(client: HttpClient) -> Self {
        Self::with_rate_limit(client, Arc::new(CratesIoRateLimit::new()))
    }

    /// レート制限状態を共有する crates.io アダプタを作成する
    pub fn with_rate_limit(client: HttpClient, rate_limit: Arc<CratesIoRateLimit>) -> Self {
        Self { client, rate_limit }
    }

    /// クレート用の URL を構築
    fn build_url(&self, crate_name: &str) -> String {
        format!("{}/{}", CRATES_IO_API_URL, crate_name)
    }

    /// クレート名が crates.io の命名規則に収まっていることを検証する。
    ///
    /// 名前は `build_url` で URL パスへ直接埋め込まれる。`url` crate は WHATWG URL
    /// 仕様どおりドットセグメントを正規化するため、`a/../serde` のような名前が
    /// Cargo.toml にあると serde の版を取得して元のキーへ書き戻してしまう。
    /// `?` / `#` はクエリ・フラグメントとして解釈される。
    /// crates.io が許すのは英数字・`-`・`_` のみ。
    fn validate_crate_name(&self, crate_name: &str) -> Result<(), RegistryError> {
        let valid = !crate_name.is_empty()
            && crate_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if valid {
            Ok(())
        } else {
            Err(RegistryError::InvalidPackageName {
                name: crate_name.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected [A-Za-z0-9_-] characters".to_string(),
            })
        }
    }

    /// レート制限を適用し、セマフォ許可を返す。
    /// 呼び出し元は HTTP リクエスト完了までこの許可を保持すること。
    async fn apply_rate_limit(&self) -> tokio::sync::SemaphorePermit<'_> {
        let permit = self.rate_limit.semaphore.acquire().await.unwrap();

        // 待機が必要か確認
        let elapsed = {
            let last_request = self.rate_limit.last_request.lock().unwrap();
            last_request.map(|t| t.elapsed())
        };

        if let Some(elapsed) = elapsed
            && elapsed < RATE_LIMIT_INTERVAL
        {
            tokio::time::sleep(RATE_LIMIT_INTERVAL - elapsed).await;
        }

        // 最終リクエスト時刻を更新
        *self.rate_limit.last_request.lock().unwrap() = Some(Instant::now());

        permit
    }

    async fn api_versions(
        &self,
        crate_name: &str,
        include_yanked: bool,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        self.validate_crate_name(crate_name)?;
        let _permit = self.apply_rate_limit().await;
        let response: CratesIoResponse = self
            .client
            .get_json(
                &self.build_url(crate_name),
                crate_name,
                self.registry_name(),
            )
            .await?;
        Ok(response.into_versions(include_yanked))
    }
}

#[async_trait]
impl RegistryAdapter for CratesIoAdapter {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn registry_name(&self) -> &'static str {
        "crates.io"
    }

    async fn fetch_versions(&self, crate_name: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        self.api_versions(crate_name, false).await
    }

    async fn fetch_locked_versions(
        &self,
        crate_name: &str,
        locked: &[String],
        publisher_cutoff: Option<DateTime<Utc>>,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        self.validate_crate_name(crate_name)?;
        let url = format!("https://index.crates.io/{}", index_path(crate_name));
        if let Ok(content) = self
            .client
            .get_text(&url, crate_name, self.registry_name())
            .await
            && let Some(versions) = index_release_dates(&content, crate_name)
            && locked
                .iter()
                .all(|locked| versions.iter().any(|info| info.version == *locked))
            && publisher_cutoff.is_none_or(|cutoff| {
                locked.iter().all(|locked| {
                    versions.iter().any(|info| {
                        info.version == *locked
                            && info.released_at.is_some_and(|date| date <= cutoff)
                    })
                })
            })
        {
            return Ok(versions);
        }
        // 古い index や若い版の公開者免除では API を使う。yank 済みの lock 版も含む。
        self.api_versions(crate_name, true).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_metadata_includes_yanked_dates_without_making_them_candidates() {
        let response = || {
            serde_json::from_value::<CratesIoResponse>(serde_json::json!({"versions":[
                {"num":"1.0.0","created_at":"2026-01-01T00:00:00Z","yanked":false},
                {"num":"1.0.1","created_at":"2026-01-02T00:00:00Z","yanked":true}
            ]}))
            .unwrap()
        };
        assert_eq!(response().into_versions(false).len(), 1);
        let locked = response().into_versions(true);
        assert_eq!(locked.len(), 2);
        assert_eq!(locked[1].version, "1.0.1");
        assert_eq!(
            locked[1].released_at,
            Some("2026-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
    }

    #[test]
    fn sparse_publication_dates_include_yanked_and_skip_missing_dates() {
        let content = r#"{"name":"library","vers":"1.0.0","pubtime":"2026-01-01T00:00:00Z","yanked":true}
{"name":"library","vers":"1.0.1","pubtime":null}
{"name":"library","vers":"1.0.2","pubtime":"invalid"}"#;
        let dates = index_release_dates(content, "library").unwrap();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0].version, "1.0.0");
        assert!(index_release_dates(content, "other").is_none());
        assert!(index_release_dates("invalid", "library").is_none());
        for (name, path) in [
            ("X", "1/x"),
            ("ab", "2/ab"),
            ("ABC", "3/a/abc"),
            ("serde", "se/rd/serde"),
        ] {
            assert_eq!(index_path(name), path);
        }
    }

    #[test]
    fn publisher_requires_registry_verified_identity() {
        use crate::update::PublisherEvidence;
        let base =
            serde_json::json!({"num":"1.2.3", "created_at":"2026-01-01T00:00:00Z", "yanked":false});
        let parse = |metadata: serde_json::Value| {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(metadata.as_object().unwrap().clone());
            serde_json::from_value::<CrateVersion>(value)
                .unwrap()
                .publisher()
        };
        assert_eq!(
            parse(
                serde_json::json!({"published_by":{"login":"example-dev", "github_username_matches":true}})
            ),
            PublisherEvidence::GithubUser {
                login: "example-dev".into()
            }
        );
        assert_eq!(
            parse(
                serde_json::json!({"trustpub_data":{"provider":"github", "repository":"example-dev/library"}})
            ),
            PublisherEvidence::GithubTrustedPublisher {
                owner: "example-dev".into()
            }
        );
        for metadata in [
            serde_json::json!({}),
            serde_json::json!({"published_by":{"login":"example-dev"}}),
            serde_json::json!({"published_by":{"login":"example-dev", "github_username_matches":false}}),
            serde_json::json!({"repository":"https://github.com/example-dev/library"}),
            serde_json::json!({"published_by":"example-dev"}),
            serde_json::json!({"trustpub_data":{"provider":"github", "repository":"example-dev/library/extra"}}),
            serde_json::json!({"trustpub_data":{"provider":"gitlab", "repository":"example-dev/library"}, "published_by":{"login":"example-dev", "github_username_matches":true}}),
        ] {
            assert_eq!(parse(metadata), PublisherEvidence::Unknown);
        }
    }

    #[test]
    fn test_crates_io_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(adapter.language(), Language::Rust);
    }

    #[test]
    fn test_crates_io_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(adapter.registry_name(), "crates.io");
    }

    /// 回帰テスト: クレート名の URL インジェクションを弾く。
    ///
    /// `a/../serde` は `url` crate のドットセグメント正規化で `serde` に解決され、
    /// 無関係なクレートの版を取得して元のキーへ書き戻してしまう。
    #[test]
    fn test_validate_crate_name_rejects_url_injection() {
        let adapter = CratesIoAdapter::new(HttpClient::new().unwrap());
        for name in [
            "a/../serde",
            "..",
            ".",
            "serde?x=1",
            "serde#frag",
            "",
            "a/b",
        ] {
            assert!(
                adapter.validate_crate_name(name).is_err(),
                "不正なクレート名を受理してはならない: {name:?}"
            );
        }
    }

    #[test]
    fn test_validate_crate_name_accepts_crate_names() {
        let adapter = CratesIoAdapter::new(HttpClient::new().unwrap());
        for name in ["serde", "serde_json", "async-trait", "pep440_rs", "x"] {
            assert!(
                adapter.validate_crate_name(name).is_ok(),
                "正当なクレート名を弾いてはならない: {name:?}"
            );
        }
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(
            adapter.build_url("serde"),
            "https://crates.io/api/v1/crates/serde"
        );
    }

    #[test]
    fn test_build_url_with_underscores() {
        let client = HttpClient::new().unwrap();
        let adapter = CratesIoAdapter::new(client);
        assert_eq!(
            adapter.build_url("serde_json"),
            "https://crates.io/api/v1/crates/serde_json"
        );
    }

    #[test]
    fn test_rate_limit_constants() {
        assert_eq!(RATE_LIMIT_INTERVAL, Duration::from_secs(1));
    }
}
