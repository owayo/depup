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

/// クレートバージョン情報
#[derive(Debug, Deserialize)]
struct CrateVersion {
    /// バージョン番号
    num: String,
    /// 作成日時タイムスタンプ
    created_at: String,
    /// このバージョンが yank されているか
    yanked: bool,
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
        // 名前の検証はレート制限を取る前に行う (不正名で 1 秒の枠を消費しない)
        self.validate_crate_name(crate_name)?;

        // レート制限を適用（HTTP リクエスト完了まで許可を保持する）
        let _permit = self.apply_rate_limit().await;

        let url = self.build_url(crate_name);
        let response: CratesIoResponse = self
            .client
            .get_json(&url, crate_name, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for version in response.versions {
            // yank されたバージョンをスキップ
            if version.yanked {
                continue;
            }

            if let Ok(released_at) = version.created_at.parse::<DateTime<Utc>>() {
                versions.push(VersionInfo::new(&version.num, released_at));
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
