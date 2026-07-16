//! HTTP クライアント共通基盤
//!
//! 共有 HTTP クライアントを提供する:
//! - タイムアウトと User-Agent の設定
//! - 指数バックオフによるリトライ (最大3回、トランスポートエラー / 429 / 5xx が対象)
//! - レート制限のエラーハンドリング (429/503 の `Retry-After` ヘッダを尊重)

use crate::error::RegistryError;
use reqwest::{Client, StatusCode};
use std::time::Duration;

/// HTTP リクエストのデフォルトタイムアウト (30秒)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// デフォルトの User-Agent ヘッダ
const DEFAULT_USER_AGENT: &str = concat!("depup/", env!("CARGO_PKG_VERSION"));

/// 最大リトライ回数
const MAX_RETRIES: u32 = 3;

/// 指数バックオフの基本遅延 (ミリ秒)
const BASE_DELAY_MS: u64 = 100;

/// `Retry-After` ヘッダで待機する最大秒数 (クランプ上限)
///
/// サーバが極端に長い待機 (数分〜) を指示してきても CLI 全体が固まらないようにする。
const RETRY_AFTER_MAX_SECS: u64 = 10;

/// ステータスコードがリトライ対象かどうかを判定する
///
/// - 429 Too Many Requests: レート制限 (従来からの対象)
/// - 5xx Server Error: npm/PyPI 等の一時障害で頻出するため指数バックオフでリトライする
///
/// 404 や 4xx (429 以外) は恒久的なエラーなのでリトライしない。
fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// `Retry-After` ヘッダ値からリトライ前の待機秒数を計算する
///
/// 秒数値 (例: `Retry-After: 5`) のみ対応し、HTTP-date 形式や不正値は `None` を返す
/// (その場合は呼び出し側が指数バックオフへフォールバックする)。
/// 値は `RETRY_AFTER_MAX_SECS` (10 秒) にクランプする。
fn parse_retry_after_secs(header_value: Option<&str>) -> Option<u64> {
    let secs: u64 = header_value?.trim().parse().ok()?;
    Some(secs.min(RETRY_AFTER_MAX_SECS))
}

/// `Retry-After` を尊重すべきステータス (429 / 503) のレスポンスから待機秒数を取り出す
fn retry_after_for_status(status: StatusCode, response: &reqwest::Response) -> Option<u64> {
    if status != StatusCode::TOO_MANY_REQUESTS && status != StatusCode::SERVICE_UNAVAILABLE {
        return None;
    }
    parse_retry_after_secs(
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
    )
}

/// GET 送信エラー (トランスポート層) を `RegistryError` へ変換する共通マッピング
///
/// - タイムアウト → `Timeout`
/// - その他の送信エラー → `NetworkError`
///
/// `get_with_context` のリトライループと、独自にリクエストを組み立てるアダプタ
/// (GitHub Tags のようにカスタムヘッダ / ページネーションが必要で `get_json` を
/// 使えないもの) が同じ変換を共有する。
pub(crate) fn map_send_error(
    error: &reqwest::Error,
    package: &str,
    registry: &str,
) -> RegistryError {
    if error.is_timeout() {
        RegistryError::Timeout {
            package: package.to_string(),
            registry: registry.to_string(),
        }
    } else {
        RegistryError::NetworkError {
            package: package.to_string(),
            registry: registry.to_string(),
            message: error.to_string(),
        }
    }
}

/// HTTP ステータスコードを `RegistryError` へ変換する共通マッピング
///
/// - 429 Too Many Requests → `RateLimitExceeded`
/// - 404 Not Found → `PackageNotFound`
/// - その他の非成功ステータス (5xx = レジストリの一時障害を含む) → `NetworkError`
/// - 成功ステータス → `None`
///
/// 403 / 401 のようにレジストリ固有の解釈が必要なステータスは、
/// 呼び出し側 (GitHub Tags の `classify_forbidden` 等) がこの関数より先に処理する。
pub(crate) fn map_status_error(
    status: StatusCode,
    package: &str,
    registry: &str,
) -> Option<RegistryError> {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Some(RegistryError::RateLimitExceeded {
            registry: registry.to_string(),
        });
    }
    if status == StatusCode::NOT_FOUND {
        return Some(RegistryError::PackageNotFound {
            package: package.to_string(),
            registry: registry.to_string(),
        });
    }
    if !status.is_success() {
        return Some(RegistryError::NetworkError {
            package: package.to_string(),
            registry: registry.to_string(),
            message: format!("HTTP {}", status),
        });
    }
    None
}

/// リトライロジック付き HTTP クライアントラッパー
#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    max_retries: u32,
}

impl HttpClient {
    /// デフォルト設定で HTTP クライアントを作成
    pub fn new() -> Result<Self, RegistryError> {
        Self::with_config(DEFAULT_TIMEOUT, DEFAULT_USER_AGENT)
    }

    /// カスタム設定で HTTP クライアントを作成
    pub fn with_config(timeout: Duration, user_agent: &str) -> Result<Self, RegistryError> {
        let client = Client::builder()
            .timeout(timeout)
            .user_agent(user_agent)
            .build()
            .map_err(|e| RegistryError::NetworkError {
                package: String::new(),
                registry: "HTTP client".to_string(),
                message: format!("failed to create HTTP client: {}", e),
            })?;

        Ok(Self {
            client,
            max_retries: MAX_RETRIES,
        })
    }

    /// 最大リトライ回数を設定
    ///
    /// 現状は同一ファイル内のテストからのみ利用されるため test 専用とする
    /// (非 test コードで必要になったら `#[cfg(test)]` を外す)。
    #[cfg(test)]
    fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// 内部の reqwest クライアントを取得
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// リトライ付き GET リクエストを実行
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, RegistryError> {
        self.get_with_context(url, "", "").await
    }

    /// エラーコンテキスト付きリトライ GET リクエストを実行
    pub async fn get_with_context(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<reqwest::Response, RegistryError> {
        let mut last_error = None;
        let mut delay = BASE_DELAY_MS;

        for attempt in 0..=self.max_retries {
            match self.client.get(url).send().await {
                Ok(response) => {
                    let status = response.status();

                    // リトライ対象ステータス (429 / 5xx) のチェック
                    if is_retryable_status(status) {
                        // 429 → RateLimitExceeded / 5xx → NetworkError (一時障害) の変換は
                        // 共通マッピングに委ねる。リトライ対象は必ず非成功ステータスなので
                        // `map_status_error` は Some を返す。
                        last_error = map_status_error(status, package, registry);

                        if attempt < self.max_retries {
                            // 429/503 の Retry-After ヘッダ (秒数値) があれば尊重し、
                            // なければ指数バックオフで待機してリトライ
                            let wait = match retry_after_for_status(status, &response) {
                                Some(secs) => Duration::from_secs(secs),
                                None => Duration::from_millis(delay),
                            };
                            tokio::time::sleep(wait).await;
                            delay *= 2;
                            continue;
                        }
                        // 最終リトライでも失敗した場合はエラーを返す
                        return Err(last_error.unwrap());
                    }

                    // 恒久的なエラーステータス (404、リトライしても無駄な 4xx 等) の共通マッピング
                    if let Some(error) = map_status_error(status, package, registry) {
                        return Err(error);
                    }

                    return Ok(response);
                }
                Err(e) => {
                    // タイムアウト / トランスポートエラーを共通マッピングで変換
                    last_error = Some(map_send_error(&e, package, registry));

                    if attempt < self.max_retries {
                        // 指数バックオフでリトライ
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        delay *= 2;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RegistryError::NetworkError {
            package: package.to_string(),
            registry: registry.to_string(),
            message: "unknown error".to_string(),
        }))
    }

    /// GET リクエストを実行し JSON レスポンスをパース
    ///
    /// ネットワークリトライは `get_with_context` 内で完結するため、
    /// ここでは追加のリトライは行わない。
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<T, RegistryError> {
        let response = self.get_with_context(url, package, registry).await?;

        response
            .json::<T>()
            .await
            .map_err(|e| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: registry.to_string(),
                message: format!("JSON パース失敗: {}", e),
            })
    }

    /// GET リクエストを実行しテキストレスポンスを取得
    ///
    /// ネットワークリトライは `get_with_context` 内で完結するため、
    /// ここでは追加のリトライは行わない。
    pub async fn get_text(
        &self,
        url: &str,
        package: &str,
        registry: &str,
    ) -> Result<String, RegistryError> {
        let response = self.get_with_context(url, package, registry).await?;

        response
            .text()
            .await
            .map_err(|e| RegistryError::InvalidResponse {
                package: package.to_string(),
                registry: registry.to_string(),
                message: format!("テキストレスポンス取得失敗: {}", e),
            })
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("failed to create default HTTP client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_creation() {
        let client = HttpClient::new();
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_client_with_config() {
        let client = HttpClient::with_config(Duration::from_secs(60), "test-agent/1.0");
        assert!(client.is_ok());
    }

    #[test]
    fn test_http_client_with_max_retries() {
        let client = HttpClient::new().unwrap().with_max_retries(5);
        assert_eq!(client.max_retries, 5);
    }

    #[test]
    fn test_http_client_default() {
        let client = HttpClient::default();
        assert_eq!(client.max_retries, MAX_RETRIES);
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
        assert!(DEFAULT_USER_AGENT.starts_with("depup/"));
        assert_eq!(MAX_RETRIES, 3);
        assert_eq!(BASE_DELAY_MS, 100);
        assert_eq!(RETRY_AFTER_MAX_SECS, 10);
    }

    /// バグ回帰テスト: 5xx (npm/PyPI の一時障害で頻出) もリトライ対象になる。
    /// 以前はトランスポートエラーと 429 のみリトライし、5xx は即エラー返却していた。
    #[test]
    fn test_retryable_status_includes_5xx() {
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR)); // 500
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY)); // 502
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE)); // 503
        assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT)); // 504
    }

    #[test]
    fn test_retryable_status_includes_429() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
    }

    #[test]
    fn test_non_retryable_statuses() {
        // 成功・恒久的なクライアントエラーはリトライしない
        assert!(!is_retryable_status(StatusCode::OK)); // 200
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST)); // 400
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED)); // 401
        assert!(!is_retryable_status(StatusCode::FORBIDDEN)); // 403
        assert!(!is_retryable_status(StatusCode::NOT_FOUND)); // 404
    }

    /// 共通ステータスマッピング: 429 はレート制限として報告される
    #[test]
    fn test_map_status_error_rate_limit() {
        assert!(matches!(
            map_status_error(StatusCode::TOO_MANY_REQUESTS, "pkg", "reg"),
            Some(RegistryError::RateLimitExceeded { .. })
        ));
    }

    /// 共通ステータスマッピング: 404 はパッケージ未検出として報告される
    #[test]
    fn test_map_status_error_not_found() {
        assert!(matches!(
            map_status_error(StatusCode::NOT_FOUND, "pkg", "reg"),
            Some(RegistryError::PackageNotFound { .. })
        ));
    }

    /// 共通ステータスマッピング: その他の非成功 (4xx / 5xx) は NetworkError ("HTTP {status}")
    #[test]
    fn test_map_status_error_other_non_success() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::FORBIDDEN,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ] {
            match map_status_error(status, "pkg", "reg") {
                Some(RegistryError::NetworkError { message, .. }) => {
                    assert_eq!(message, format!("HTTP {}", status));
                }
                other => panic!("expected NetworkError for {}, got: {:?}", status, other),
            }
        }
    }

    /// 共通ステータスマッピング: 成功ステータスはエラーにならない
    #[test]
    fn test_map_status_error_success_is_none() {
        assert!(map_status_error(StatusCode::OK, "pkg", "reg").is_none());
        assert!(map_status_error(StatusCode::CREATED, "pkg", "reg").is_none());
    }

    #[test]
    fn test_parse_retry_after_seconds_value() {
        assert_eq!(parse_retry_after_secs(Some("5")), Some(5));
        assert_eq!(parse_retry_after_secs(Some("0")), Some(0));
        // 前後の空白は許容する
        assert_eq!(parse_retry_after_secs(Some(" 3 ")), Some(3));
    }

    #[test]
    fn test_parse_retry_after_clamps_to_max() {
        // サーバが長い待機を指示してきても 10 秒にクランプする
        assert_eq!(parse_retry_after_secs(Some("120")), Some(10));
        assert_eq!(parse_retry_after_secs(Some("11")), Some(10));
        assert_eq!(parse_retry_after_secs(Some("10")), Some(10));
        assert_eq!(parse_retry_after_secs(Some("9")), Some(9));
    }

    #[test]
    fn test_parse_retry_after_invalid_values() {
        // ヘッダなし
        assert_eq!(parse_retry_after_secs(None), None);
        // HTTP-date 形式は非対応 (指数バックオフへフォールバック)
        assert_eq!(
            parse_retry_after_secs(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        // 不正値
        assert_eq!(parse_retry_after_secs(Some("abc")), None);
        assert_eq!(parse_retry_after_secs(Some("-1")), None);
        assert_eq!(parse_retry_after_secs(Some("1.5")), None);
        assert_eq!(parse_retry_after_secs(Some("")), None);
    }
}
