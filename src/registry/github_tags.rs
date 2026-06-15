//! Swift Package Manager 用 GitHub Tags API アダプタ
//!
//! GitHub Tags API からパッケージバージョン情報を取得する。
//! API エンドポイント: https://api.github.com/repos/{owner}/{repo}/tags
//!
//! 認証: GITHUB_TOKEN または GH_TOKEN 環境変数による任意認証。
//! 非 GitHub URL はマニフェストパーサレベルでスキップされる。

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

/// GitHub API のベース URL
const GITHUB_API_URL: &str = "https://api.github.com";

/// ページネーションで取得する最大ページ数 (安全弁)
///
/// per_page=100 なので最大 1000 タグまで取得する。巨大リポジトリで
/// 無制限にページを辿ってレート制限を浪費しないための上限。
const MAX_TAG_PAGES: usize = 10;

/// semver タグパターン ('v' プレフィックスは任意、プレリリース/ビルドメタデータも許容)
///
/// SPM は semver 2.0.0 準拠のため、`v1.0.0-beta.1` のようなプレリリース識別子付きタグや
/// `1.0.0+build.123` のようなビルドメタデータ付きタグを持ちうる。これらも取得対象に含め、
/// 安定版/プレリリースの選別は他レジストリ (npm/PyPI/crates.io 等) と同様に
/// `UpdateJudge::stable_candidates` へ委ねる。デフォルトでは安定版利用者がプレリリースへ
/// 誤更新されないよう judge がフィルタし、現在版がプレリリースの場合のみ候補に残す。
/// 末尾の `-` / `+` や `alpha..1` のような空の識別子は弾く。
static SEMVER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[vV]?(\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)$",
    )
    .unwrap()
});

/// GitHub Tags API アダプタ
pub struct GitHubTagsAdapter {
    client: HttpClient,
    token: Option<String>,
}

/// GitHub API レスポンスのタグ情報
#[derive(Debug, Deserialize)]
struct GitHubTag {
    name: String,
}

impl GitHubTagsAdapter {
    /// 新しい GitHub Tags アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        // まず GITHUB_TOKEN を試し、次に GH_TOKEN を試す
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .ok();

        Self { client, token }
    }

    /// リポジトリ用のタグ URL を構築
    fn build_url(&self, owner_repo: &str) -> String {
        format!("{}/repos/{}/tags?per_page=100", GITHUB_API_URL, owner_repo)
    }

    /// パッケージ名が "owner/repo" 形式かつ GitHub の許容文字のみであることを検証する。
    /// owner/repo に `?` `#` `/` `..` 等が混ざると `build_url` で URL クエリ汚染や
    /// パストラバーサルが起きうるため、Maven Central と同様に文字種を限定する。
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        let parts: Vec<&str> = package.split('/').collect();
        let is_valid_segment = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        };
        if parts.len() != 2 || !is_valid_segment(parts[0]) || !is_valid_segment(parts[1]) {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected format: owner/repo with [A-Za-z0-9._-] characters".to_string(),
            });
        }
        Ok(())
    }
}

/// `Link` ヘッダから `rel="next"` の URL を取り出す
///
/// GitHub のページネーション形式:
/// `<https://api.github.com/...?page=2>; rel="next", <https://api.github.com/...?page=5>; rel="last"`
fn parse_next_link(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let part = part.trim();
        let mut sections = part.split(';');
        let url_section = sections.next()?.trim();
        if !(url_section.starts_with('<') && url_section.ends_with('>')) {
            continue;
        }
        let url = &url_section[1..url_section.len() - 1];
        for param in sections {
            let param = param.trim();
            if param == r#"rel="next""# || param == "rel=next" {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// 403 Forbidden レスポンスをレート制限か認証エラーかに分類する
///
/// GitHub は未認証クライアントのレート制限超過を (429 ではなく)
/// 403 + `X-RateLimit-Remaining: 0` で返すため、このヘッダで分類する。
/// レート制限の場合は GITHUB_TOKEN / GH_TOKEN の設定を促すヒントを含める。
/// それ以外の 403 (SAML 強制、IP 制限等) は従来どおり認証エラーとして扱う。
fn classify_forbidden(rate_limit_remaining: Option<&str>) -> RegistryError {
    if rate_limit_remaining.is_some_and(|v| v.trim() == "0") {
        RegistryError::RateLimitExceeded {
            registry: "GitHub Tags (set GITHUB_TOKEN or GH_TOKEN to increase the API rate limit)"
                .to_string(),
        }
    } else {
        RegistryError::AuthenticationError {
            registry: "GitHub Tags".to_string(),
            message: "HTTP 403 Forbidden".to_string(),
        }
    }
}

#[async_trait]
impl RegistryAdapter for GitHubTagsAdapter {
    fn language(&self) -> Language {
        Language::Swift
    }

    fn registry_name(&self) -> &'static str {
        "GitHub Tags"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        self.validate_package_name(package)?;

        let mut versions = Vec::new();
        let mut url = self.build_url(package);

        // `Link` ヘッダの rel="next" を辿って全ページ取得する
        // (per_page=100 の 1 ページのみだと 100 タグ超のリポジトリで最新タグを取り逃す)。
        // 安全弁として最大 MAX_TAG_PAGES ページ (= 1000 タグ) まで。
        for _page in 0..MAX_TAG_PAGES {
            // 適切なヘッダ付きでリクエストを構築
            let mut request = self.client.inner().get(&url);
            request = request.header("Accept", "application/vnd.github+json");

            if let Some(ref token) = self.token {
                request = request.header("Authorization", format!("Bearer {}", token));
            }

            let response = request.send().await.map_err(|e| {
                if e.is_timeout() {
                    RegistryError::Timeout {
                        package: package.to_string(),
                        registry: self.registry_name().to_string(),
                    }
                } else {
                    RegistryError::NetworkError {
                        package: package.to_string(),
                        registry: self.registry_name().to_string(),
                        message: e.to_string(),
                    }
                }
            })?;

            // HTTP ステータスコードを処理
            match response.status() {
                status if status == reqwest::StatusCode::NOT_FOUND => {
                    return Err(RegistryError::PackageNotFound {
                        package: package.to_string(),
                        registry: self.registry_name().to_string(),
                    });
                }
                status if status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    return Err(RegistryError::RateLimitExceeded {
                        registry: self.registry_name().to_string(),
                    });
                }
                status if status == reqwest::StatusCode::FORBIDDEN => {
                    // GitHub はレート制限超過を 403 + X-RateLimit-Remaining: 0 で返す。
                    // 認証エラーと区別して報告する (classify_forbidden を参照)。
                    let remaining = response
                        .headers()
                        .get("x-ratelimit-remaining")
                        .and_then(|v| v.to_str().ok());
                    return Err(classify_forbidden(remaining));
                }
                status if status == reqwest::StatusCode::UNAUTHORIZED => {
                    return Err(RegistryError::AuthenticationError {
                        registry: self.registry_name().to_string(),
                        message: format!("HTTP {}", status),
                    });
                }
                status if !status.is_success() => {
                    return Err(RegistryError::NetworkError {
                        package: package.to_string(),
                        registry: self.registry_name().to_string(),
                        message: format!("HTTP {}", status),
                    });
                }
                _ => {}
            }

            // 次ページ URL は body の消費前にヘッダから取り出しておく
            let next_url = response
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_next_link);

            let tags: Vec<GitHubTag> =
                response
                    .json()
                    .await
                    .map_err(|e| RegistryError::InvalidResponse {
                        package: package.to_string(),
                        registry: self.registry_name().to_string(),
                        message: format!("failed to parse JSON: {}", e),
                    })?;

            for tag in tags {
                // タグ名から semver を抽出
                if let Some(caps) = SEMVER_RE.captures(&tag.name) {
                    let version = caps.get(1).unwrap().as_str();
                    // GitHub Tags API はリリース日を返さない。
                    // `Utc::now()` を使うと `--age` フィルタが全 Swift 更新を抑制してしまうため、
                    // age フィルタを通過させるための「十分古い」値として UNIX_EPOCH を採用する。
                    versions.push(VersionInfo::new(version, DateTime::<Utc>::UNIX_EPOCH));
                }
            }

            match next_url {
                Some(next) => url = next,
                None => break,
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
    fn test_github_tags_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = GitHubTagsAdapter::new(client);
        assert_eq!(adapter.language(), Language::Swift);
    }

    #[test]
    fn test_github_tags_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = GitHubTagsAdapter::new(client);
        assert_eq!(adapter.registry_name(), "GitHub Tags");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = GitHubTagsAdapter::new(client);
        assert_eq!(
            adapter.build_url("apple/swift-argument-parser"),
            "https://api.github.com/repos/apple/swift-argument-parser/tags?per_page=100"
        );
    }

    #[test]
    fn test_validate_package_name_valid() {
        let client = HttpClient::new().unwrap();
        let adapter = GitHubTagsAdapter::new(client);
        assert!(adapter.validate_package_name("apple/swift-nio").is_ok());
    }

    #[test]
    fn test_validate_package_name_invalid_no_slash() {
        let client = HttpClient::new().unwrap();
        let adapter = GitHubTagsAdapter::new(client);
        assert!(adapter.validate_package_name("swift-nio").is_err());
    }

    #[test]
    fn test_validate_package_name_invalid_empty_parts() {
        let client = HttpClient::new().unwrap();
        let adapter = GitHubTagsAdapter::new(client);
        assert!(adapter.validate_package_name("/swift-nio").is_err());
        assert!(adapter.validate_package_name("apple/").is_err());
    }

    #[test]
    fn test_semver_regex_matches() {
        // 安定版タグ
        assert!(SEMVER_RE.is_match("1.0.0"));
        assert!(SEMVER_RE.is_match("v1.0.0"));
        assert!(SEMVER_RE.is_match("V1.0.0"));
        assert!(SEMVER_RE.is_match("v10.20.30"));
        // SPM は semver 2.0.0 準拠なのでプレリリース/ビルドメタデータ付きタグも取得対象に含める
        // (安定版/プレリリースの選別は UpdateJudge::stable_candidates に委ねる)
        assert!(SEMVER_RE.is_match("1.0.0-beta.1"));
        assert!(SEMVER_RE.is_match("v1.0.0-rc.1"));
        assert!(SEMVER_RE.is_match("V1.0.0-rc.1+sha.abc"));
        assert!(SEMVER_RE.is_match("1.0.0+build.123"));
        // 不正な形式は弾く
        assert!(!SEMVER_RE.is_match("1.0"));
        assert!(!SEMVER_RE.is_match("v1.0"));
        assert!(!SEMVER_RE.is_match("not-a-version"));
        assert!(!SEMVER_RE.is_match("1.0.0-")); // 末尾ハイフンのみは不可
        assert!(!SEMVER_RE.is_match("1.0.0+")); // 末尾プラスのみは不可
        assert!(!SEMVER_RE.is_match("1.0.0-alpha..1")); // 空の識別子は不可
    }

    #[test]
    fn test_semver_regex_extracts_version() {
        let caps = SEMVER_RE.captures("v1.2.3").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3");

        let caps = SEMVER_RE.captures("V1.2.3").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3");

        let caps = SEMVER_RE.captures("1.2.3").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3");

        // プレリリース/ビルドメタデータも含めてキャプチャする ('v' プレフィックスのみ除去)
        let caps = SEMVER_RE.captures("v1.2.3-beta.1").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3-beta.1");

        let caps = SEMVER_RE.captures("1.2.3-rc.1+sha.abc").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3-rc.1+sha.abc");
    }

    #[test]
    fn test_deserialize_github_tag() {
        let json = r#"{"name": "1.0.0", "zipball_url": "...", "tarball_url": "...", "commit": {"sha": "abc", "url": "..."}}"#;
        let tag: GitHubTag = serde_json::from_str(json).unwrap();
        assert_eq!(tag.name, "1.0.0");
    }

    /// バグ回帰テスト: `Link` ヘッダの rel="next" を正しく取り出せる。
    /// 以前はページネーション未対応で、100 タグ超のリポジトリ
    /// (タグが新しい順とは限らない) で取得漏れが起きていた。
    #[test]
    fn test_parse_next_link_github_format() {
        let header = r#"<https://api.github.com/repositories/123/tags?per_page=100&page=2>; rel="next", <https://api.github.com/repositories/123/tags?per_page=100&page=5>; rel="last""#;
        assert_eq!(
            parse_next_link(header),
            Some("https://api.github.com/repositories/123/tags?per_page=100&page=2".to_string())
        );
    }

    #[test]
    fn test_parse_next_link_last_page_has_no_next() {
        // 最終ページの Link ヘッダには rel="next" が含まれない
        let header = r#"<https://api.github.com/repositories/123/tags?per_page=100&page=4>; rel="prev", <https://api.github.com/repositories/123/tags?per_page=100&page=1>; rel="first""#;
        assert_eq!(parse_next_link(header), None);
    }

    #[test]
    fn test_parse_next_link_next_not_first_entry() {
        // rel="next" が先頭以外に来ても取り出せる
        let header =
            r#"<https://example.com/p1>; rel="prev", <https://example.com/p3>; rel="next""#;
        assert_eq!(
            parse_next_link(header),
            Some("https://example.com/p3".to_string())
        );
    }

    #[test]
    fn test_parse_next_link_unquoted_rel() {
        // RFC 8288 上は rel=next (引用符なし) も有効
        let header = "<https://example.com/p2>; rel=next";
        assert_eq!(
            parse_next_link(header),
            Some("https://example.com/p2".to_string())
        );
    }

    #[test]
    fn test_parse_next_link_malformed_header() {
        assert_eq!(parse_next_link(""), None);
        assert_eq!(parse_next_link("garbage"), None);
        assert_eq!(
            parse_next_link(r#"https://example.com/p2; rel="next""#),
            None
        ); // <> なし
    }

    /// バグ回帰テスト: 403 + X-RateLimit-Remaining: 0 はレート制限として分類され、
    /// GITHUB_TOKEN の設定を促すヒントを含む。以前は全ての 403 を認証エラーとして
    /// 報告していたため、未認証クライアントのレート制限超過が原因と分からなかった。
    #[test]
    fn test_classify_forbidden_rate_limited() {
        let err = classify_forbidden(Some("0"));
        match &err {
            RegistryError::RateLimitExceeded { registry } => {
                assert!(registry.contains("GITHUB_TOKEN"));
                assert!(registry.contains("GH_TOKEN"));
            }
            other => panic!("expected RateLimitExceeded, got: {:?}", other),
        }
        // 空白付きヘッダ値も許容する
        assert!(matches!(
            classify_forbidden(Some(" 0 ")),
            RegistryError::RateLimitExceeded { .. }
        ));
    }

    #[test]
    fn test_classify_forbidden_auth_error() {
        // X-RateLimit-Remaining ヘッダなし → 従来どおり認証エラー
        assert!(matches!(
            classify_forbidden(None),
            RegistryError::AuthenticationError { .. }
        ));
        // remaining が 0 以外 (= レート制限ではない 403: SAML 強制や IP 制限等)
        assert!(matches!(
            classify_forbidden(Some("42")),
            RegistryError::AuthenticationError { .. }
        ));
    }

    #[test]
    fn test_max_tag_pages_limit() {
        // 安全弁: 最大 10 ページ (per_page=100 × 10 = 1000 タグ)
        assert_eq!(MAX_TAG_PAGES, 10);
    }

    /// バグ回帰テスト: GitHub Tags API はリリース日を返さないため、
    /// `--age` フィルタが Swift 更新を全スキップしないように
    /// `released_at` には UNIX_EPOCH (= 古いとして扱う) を使う。
    /// 以前は `Utc::now()` を使っていたため、`--age 1d` 等で全 Swift 更新が抑制されていた。
    #[test]
    fn test_version_info_uses_epoch_for_age_filter_compatibility() {
        let epoch = DateTime::<Utc>::UNIX_EPOCH;
        let info = VersionInfo::new("1.2.3", epoch);
        assert_eq!(info.released_at, epoch);
        // 通常の age 指定 (例: 1日前) のカットオフは UNIX_EPOCH (1970年) より新しいので、
        // epoch をリリース日とするバージョンは age フィルタを通過する。
        let cutoff_1d = Utc::now() - chrono::Duration::days(1);
        assert!(info.released_at <= cutoff_1d);
        let cutoff_1y = Utc::now() - chrono::Duration::days(365);
        assert!(info.released_at <= cutoff_1y);
    }
}
