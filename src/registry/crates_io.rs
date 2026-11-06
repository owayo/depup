//! crates.io アダプタ
//!
//! crates.io からクレートのバージョン情報を取得する。版の一覧と公開日は、
//! cargo と同じ sparse index (https://index.crates.io/{path}) から先に取る。
//! sparse index は静的なファイルの配信で、並列に取得してよい。
//! index で一覧を確定できないときと、公開者の確認が要るときは
//! API (https://crates.io/api/v1/crates/{crate}) を使う。
//!
//! 注意: crates.io は User-Agent ヘッダが必要 (HttpClient で処理済み)。
//! API はレート制限あり (1リクエスト/秒)。

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

/// crates.io の sparse index のベース URL
const CRATES_IO_INDEX_URL: &str = "https://index.crates.io";

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

    /// API へ問い合わせる順番を取る。呼び出し元は、再試行を含む問い合わせが終わるまで
    /// この許可を保持すること (並行する問い合わせの送信が間に挟まらないように)。
    async fn acquire(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.semaphore.acquire().await.unwrap()
    }

    /// 前の送信から `RATE_LIMIT_INTERVAL` たつまで待ち、この送信の時刻を記録する。
    /// 再試行を含む API への各送信の前に、`acquire` の許可を持ったまま呼ぶ (#100)。
    async fn wait_for_interval(&self) {
        let elapsed = self
            .last_request
            .lock()
            .unwrap()
            .map(|last_request| last_request.elapsed());
        if let Some(elapsed) = elapsed
            && elapsed < RATE_LIMIT_INTERVAL
        {
            tokio::time::sleep(RATE_LIMIT_INTERVAL - elapsed).await;
        }
        *self.last_request.lock().unwrap() = Some(Instant::now());
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
    /// 版の一覧に、API だけが返す公開者の情報が要るか (`requiring_publishers`)
    publishers_required: bool,
    api_url: String,
    index_url: String,
}

/// crates.io クレートレスポンス
#[derive(Debug, Deserialize)]
struct CratesIoResponse {
    /// クレート情報
    versions: Vec<CrateVersion>,
}

impl CratesIoResponse {
    fn into_yanked_versions(mut self) -> Vec<VersionInfo> {
        self.versions.retain(|version| version.yanked);
        self.into_versions(true)
    }

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

/// Sparse index の 1 行 (1 版)。公開日は yank によって変わらない。
#[derive(Deserialize)]
struct IndexRelease {
    name: String,
    vers: String,
    #[serde(default)]
    yanked: bool,
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

/// sparse index の内容から、更新候補 (yank 済みを除く版) を API の経路と同じ昇順で返す。
///
/// 一覧を確定できない内容では `None` を返し、呼び出し側は API を使う。空の応答、
/// 壊れた行、別の名前の行、yank されていない版の公開日の欠落・不正が該当する。
/// 公開日を欠いた版を黙って落とすと、age の判定と候補が API の結果と食い違うため。
/// 全版が yank 済みの正常な応答は、空の一覧として返す。
fn index_candidates(content: &str, package: &str) -> Option<Vec<VersionInfo>> {
    let mut versions = Vec::new();
    let mut releases = 0;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        releases += 1;
        let release: IndexRelease = serde_json::from_str(line).ok()?;
        if !release.name.eq_ignore_ascii_case(package) {
            return None;
        }
        if release.yanked {
            continue;
        }
        let released_at = release.pubtime?.parse::<DateTime<Utc>>().ok()?;
        versions.push(VersionInfo::new(release.vers, released_at));
    }
    if releases == 0 {
        return None;
    }
    versions.sort();
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
        Self {
            client,
            rate_limit,
            publishers_required: false,
            api_url: CRATES_IO_API_URL.to_string(),
            index_url: CRATES_IO_INDEX_URL.to_string(),
        }
    }

    /// 版の一覧に公開者の情報が要るかを設定する。
    ///
    /// 公開者は API だけが返す。公開者で age を免除する設定があるときは、版の一覧を
    /// sparse index ではなく API から取る。設定の有無は実行全体で一律に決めること。
    /// スコープごとに切り替えると、公開者のない一覧のキャッシュを、免除が有効な
    /// スコープで使い回してしまう。
    pub fn requiring_publishers(mut self, required: bool) -> Self {
        self.publishers_required = required;
        self
    }

    #[cfg(test)]
    pub(crate) fn publishers_required(&self) -> bool {
        self.publishers_required
    }

    /// クレート用の URL を構築
    fn build_url(&self, crate_name: &str) -> String {
        format!("{}/{}", self.api_url, crate_name)
    }

    /// クレートの sparse index のファイルの URL
    fn index_file_url(&self, crate_name: &str) -> String {
        format!("{}/{}", self.index_url, index_path(crate_name))
    }

    /// sparse index から更新候補を取る。取得に失敗したとき (404 を含む) と、
    /// 内容から一覧を確定できないときは `None`。
    async fn index_versions(&self, crate_name: &str) -> Option<Vec<VersionInfo>> {
        let content = self
            .client
            .get_text(
                &self.index_file_url(crate_name),
                crate_name,
                self.registry_name(),
            )
            .await
            .ok()?;
        index_candidates(&content, crate_name)
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

    async fn api_versions(
        &self,
        crate_name: &str,
        include_yanked: bool,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        Ok(self
            .api_response(crate_name)
            .await?
            .into_versions(include_yanked))
    }

    /// API の応答。再試行を含む各送信の前に、実行全体で共有する 1 リクエスト/秒の間隔を
    /// 待つ (#100)。問い合わせの前に 1 回だけ待つと、503 などの再試行をバックオフ
    /// (100 ms〜) だけで送ってしまう。
    async fn api_response(&self, crate_name: &str) -> Result<CratesIoResponse, RegistryError> {
        self.validate_crate_name(crate_name)?;
        let _permit = self.rate_limit.acquire().await;
        self.client
            .get_json_paced(
                &self.build_url(crate_name),
                crate_name,
                self.registry_name(),
                || self.rate_limit.wait_for_interval(),
            )
            .await
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
        self.fetch_versions_including(crate_name, &[]).await
    }

    /// 公開者が要らなければ sparse index を先に引く。index で一覧を確定できないとき、
    /// または `required` の版が index にまだ無いとき (配信の反映待ち) は API を使い、
    /// エラーも API のものを返す。
    async fn fetch_versions_including(
        &self,
        crate_name: &str,
        required: &[String],
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        self.validate_crate_name(crate_name)?;
        if !self.publishers_required
            && let Some(versions) = self.index_versions(crate_name).await
            && required
                .iter()
                .all(|required| versions.iter().any(|info| info.version == *required))
        {
            return Ok(versions);
        }
        self.api_versions(crate_name, false).await
    }

    async fn fetch_yanked_versions(
        &self,
        crate_name: &str,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        Ok(self.api_response(crate_name).await?.into_yanked_versions())
    }

    async fn fetch_locked_versions(
        &self,
        crate_name: &str,
        locked: &[String],
        publisher_cutoff: Option<DateTime<Utc>>,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        self.validate_crate_name(crate_name)?;
        let url = self.index_file_url(crate_name);
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
                {"num":"1.0.1","created_at":"2026-01-02T00:00:00Z","yanked":true},
                {"num":"1.0.2","created_at":"invalid","yanked":true}
            ]}))
            .unwrap()
        };
        assert_eq!(response().into_versions(false).len(), 1);
        let locked = response().into_versions(true);
        assert_eq!(locked.len(), 2);
        assert_eq!(locked[1].version, "1.0.1");
        let yanked = response().into_yanked_versions();
        assert_eq!(yanked.len(), 1);
        assert_eq!(yanked[0].version, "1.0.1");
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
    fn index_candidates_exclude_yanked_sort_and_reject_unusable_content() {
        let content = r#"{"name":"Library","vers":"1.10.0","pubtime":"2026-01-03T00:00:00Z"}
{"name":"library","vers":"1.2.0","pubtime":"2026-01-02T00:00:00Z","yanked":false}
{"name":"library","vers":"1.1.0","pubtime":"2026-01-01T00:00:00Z","yanked":true}
{"name":"library","vers":"1.0.0","yanked":true}
"#;
        let versions = index_candidates(content, "library").unwrap();
        // yank 済みを除き、API の経路と同じ順に並べる。yank 済みの版は公開日を欠いてもよい
        assert_eq!(
            versions
                .iter()
                .map(|info| info.version.as_str())
                .collect::<Vec<_>>(),
            ["1.2.0", "1.10.0"]
        );
        assert_eq!(
            versions[0].released_at,
            Some("2026-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
        // 全版が yank 済みなら、空の一覧が正しい答え
        let yanked =
            r#"{"name":"library","vers":"1.0.0","pubtime":"2026-01-01T00:00:00Z","yanked":true}"#;
        assert_eq!(
            index_candidates(yanked, "library").map(|versions| versions.len()),
            Some(0)
        );
        // 一覧を確定できない内容は API に任せる
        for unusable in [
            "",
            "\n",
            "not json",
            r#"{"name":"other","vers":"1.0.0","pubtime":"2026-01-01T00:00:00Z"}"#,
            r#"{"name":"library","vers":"1.0.0"}"#,
            r#"{"name":"library","vers":"1.0.0","pubtime":null}"#,
            r#"{"name":"library","vers":"1.0.0","pubtime":"2026-13-01T00:00:00Z"}"#,
        ] {
            assert!(
                index_candidates(unusable, "library").is_none(),
                "{unusable:?}"
            );
        }
    }

    /// 決めた応答を返すサーバを、sparse index と API の両方の起点にする。
    fn served_adapter(base: &str) -> CratesIoAdapter {
        let mut adapter = CratesIoAdapter::new(HttpClient::new().unwrap());
        adapter.index_url = base.to_string();
        adapter.api_url = format!("{base}/api/v1/crates");
        adapter
    }

    const SERVED_INDEX: &str = concat!(
        r#"{"name":"library","vers":"1.0.0","pubtime":"2026-01-01T00:00:00Z"}"#,
        "\n",
        r#"{"name":"library","vers":"1.1.0","pubtime":"2026-02-01T00:00:00Z","yanked":true}"#,
        "\n",
    );

    const SERVED_API: &str = r#"{"versions":[{"num":"1.2.0","created_at":"2026-03-01T00:00:00.5Z","yanked":false,"published_by":{"login":"example-dev","github_username_matches":true}}]}"#;

    #[tokio::test]
    async fn versions_come_from_the_sparse_index_without_the_api() {
        use crate::test_support::http::serve_responses;
        // サーバは決めた要求だけに答えるので、API へ問い合わせると取得が失敗する
        let (base, handle) =
            serve_responses(vec![("GET /li/br/library ", "200 OK\r\n", SERVED_INDEX)]);
        let versions = served_adapter(&base)
            .fetch_versions("library")
            .await
            .unwrap();
        assert_eq!(
            versions
                .iter()
                .map(|info| info.version.as_str())
                .collect::<Vec<_>>(),
            ["1.0.0"]
        );
        handle.join().unwrap();
    }

    /// Issue #100: API への再試行も、前の送信から 1 秒の間隔を空けて送る。以前は間隔を
    /// 問い合わせの前に 1 回だけ確かめ、503 の再試行を 100 ms・200 ms の待ちで送っていた。
    #[tokio::test]
    async fn api_retries_keep_the_one_second_interval() {
        use crate::test_support::http::serve_responses;
        let request = "GET /api/v1/crates/library ";
        let unavailable = "503 Service Unavailable\r\n";
        let (base, handle) = serve_responses(vec![
            (request, unavailable, ""),
            (request, unavailable, ""),
            (request, "200 OK\r\n", SERVED_API),
        ]);
        let started = Instant::now();
        let yanked = served_adapter(&base)
            .fetch_yanked_versions("library")
            .await
            .unwrap();
        // 3 回の送信の間が 1 秒ずつ空く (再試行のバックオフは合わせても 1 秒に満たない)
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(1900), "{elapsed:?}");
        assert!(yanked.is_empty());
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn unusable_sparse_index_falls_back_to_the_api() {
        use crate::test_support::http::serve_responses;
        for (status, body) in [
            ("404 Not Found\r\n", ""),
            ("200 OK\r\n", ""),
            ("200 OK\r\n", "not json"),
            (
                "200 OK\r\n",
                r#"{"name":"other","vers":"1.0.0","pubtime":"2026-01-01T00:00:00Z"}"#,
            ),
            ("200 OK\r\n", r#"{"name":"library","vers":"1.0.0"}"#),
        ] {
            let (base, handle) = serve_responses(vec![
                ("GET /li/br/library ", status, body),
                ("GET /api/v1/crates/library ", "200 OK\r\n", SERVED_API),
            ]);
            let versions = served_adapter(&base)
                .fetch_versions("library")
                .await
                .unwrap();
            assert_eq!(versions.len(), 1, "{status:?} {body:?}");
            assert_eq!(versions[0].version, "1.2.0");
            handle.join().unwrap();
        }
    }

    #[tokio::test]
    async fn publishers_and_unpublished_required_versions_use_the_api() {
        use crate::test_support::http::serve_responses;
        use crate::update::PublisherEvidence;
        // 公開者が要る実行は sparse index を引かない
        let (base, handle) = serve_responses(vec![(
            "GET /api/v1/crates/library ",
            "200 OK\r\n",
            SERVED_API,
        )]);
        let versions = served_adapter(&base)
            .requiring_publishers(true)
            .fetch_versions("library")
            .await
            .unwrap();
        assert_eq!(
            versions[0].publisher,
            PublisherEvidence::GithubUser {
                login: "example-dev".into()
            }
        );
        handle.join().unwrap();

        // install が入れた版が index にまだ無ければ、API で取り直す
        let (base, handle) = serve_responses(vec![
            ("GET /li/br/library ", "200 OK\r\n", SERVED_INDEX),
            ("GET /api/v1/crates/library ", "200 OK\r\n", SERVED_API),
        ]);
        let versions = served_adapter(&base)
            .fetch_versions_including("library", &["1.2.0".into()])
            .await
            .unwrap();
        assert_eq!(versions[0].version, "1.2.0");
        handle.join().unwrap();

        // index にあれば API は使わない
        let (base, handle) =
            serve_responses(vec![("GET /li/br/library ", "200 OK\r\n", SERVED_INDEX)]);
        let versions = served_adapter(&base)
            .fetch_versions_including("library", &["1.0.0".into()])
            .await
            .unwrap();
        assert_eq!(versions[0].version, "1.0.0");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn invalid_crate_names_are_rejected_before_any_request() {
        let error = CratesIoAdapter::new(HttpClient::new().unwrap())
            .fetch_versions("a/../serde")
            .await
            .unwrap_err();
        assert!(matches!(error, RegistryError::InvalidPackageName { .. }));
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
