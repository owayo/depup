//! Swift Package Manager 用 GitHub Tags API アダプタ
//!
//! GitHub Tags API からパッケージバージョン情報を取得する。
//! API エンドポイント: https://api.github.com/repos/{owner}/{repo}/tags
//!
//! 認証: GITHUB_TOKEN または GH_TOKEN 環境変数による任意認証。
//! 非 GitHub URL はマニフェストパーサレベルでスキップされる。

use crate::domain::Language;
use crate::error::RegistryError;
use crate::parser::{SwiftVersionParser, VersionParser};
use crate::registry::client::map_status_error;
use crate::registry::{HttpClient, RegistryAdapter, is_valid_registry_id_segment};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// GitHub API のベース URL
const GITHUB_API_URL: &str = "https://api.github.com";

/// 1 ページあたりのタグ取得件数 (GitHub API の上限値)
const TAGS_PER_PAGE: usize = 100;

/// ページネーションで取得する最大ページ数 (安全弁)
///
/// per_page=100 なので最大 1000 タグまで取得する。巨大リポジトリで
/// 無制限にページを辿ってレート制限を浪費しないための上限。
const MAX_TAG_PAGES: usize = 10;

/// タグ名から SPM が受理する semver バージョンを取り出す ('v' / 'V' プレフィックスは任意)
///
/// SPM は semver 2.0.0 準拠のため、`v1.0.0-beta.1` のようなプレリリース識別子付きタグや
/// `1.0.0+build.123` のようなビルドメタデータ付きタグを持ちうる。これらも取得対象に含め、
/// 安定版/プレリリースの選別は他レジストリ (npm/PyPI/crates.io 等) と同様に
/// `UpdateJudge::stable_candidates` へ委ねる。デフォルトでは安定版利用者がプレリリースへ
/// 誤更新されないよう judge がフィルタし、現在版がプレリリースの場合のみ候補に残す。
///
/// 受理判定は `Package.swift` 側と同じ `SwiftVersionParser` へ委譲し、SPM の Version の
/// 定義を 1 箇所に集約する。以前は取得側とマニフェスト側で semver の正規表現を二重管理して
/// おり、取得側だけが数値識別子の先頭ゼロを許していた。`2024.01.15` のような CalVer タグは
/// SPM の `Version` (semver 2.0.0 厳格パース) では invalid だが `2024 > 1` のため semver
/// タグと混在するリポジトリでは必ず最新候補として選ばれ、`swift build` が
/// `Invalid semantic version string` で manifest ごと読めなくなる版を書き込んでいた。
fn extract_semver_tag(tag: &str) -> Option<&str> {
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
    // パーサは前後の空白を許容するが、タグ名では空白付きの形を受理しない。
    // 取得側だけ余分な形を通すと、書き戻し時にマニフェスト側と食い違う
    if version.trim() != version {
        return None;
    }
    SwiftVersionParser.parse(version).map(|_| version)
}

/// ページ取得ループが `MAX_TAG_PAGES` で打ち切られたかを判定する
///
/// `page` は 0 起点のページ番号、`has_next` は `Link` ヘッダに rel="next" があるか。
fn is_page_limit_truncated(page: usize, has_next: bool) -> bool {
    has_next && page + 1 == MAX_TAG_PAGES
}

/// タグ一覧がページ上限で打ち切られたことを伝える警告文を組み立てる
///
/// 無言で打ち切ると、1000 タグを超えるリポジトリで古い系列のタグが候補から静かに落ち、
/// `--max-change patch` などで系列内に留まる依存が AlreadyLatest と誤判定される。
/// ネットワーク越しの取りこぼしは利用者から見て「更新が無い」と区別がつかないため通知する。
fn page_limit_warning(package: &str) -> String {
    format!(
        "⚠ {}: GitHub tag list truncated at {} pages ({} tags); older tags were not fetched",
        package,
        MAX_TAG_PAGES,
        MAX_TAG_PAGES * TAGS_PER_PAGE
    )
}

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

/// 環境変数の候補列から実際に使うトークンを選ぶ。
///
/// `std::env::var` は「セット済みだが空文字」に対して `Err` ではなく `Ok("")` を返す。
/// 空値をそのまま採用すると `Authorization: Bearer ` (トークン部が空) を送ることになり、
/// GitHub は不正な資格情報として 401 を返す — ヘッダ自体を付けなければ未認証で成功して
/// いた取得が、空の env var を置いただけで全滅する。空値を「未設定」として読み飛ばし、
/// 後続の候補 (`GH_TOKEN`) へフォールバックさせる。
fn select_github_token(candidates: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    candidates
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

impl GitHubTagsAdapter {
    /// 新しい GitHub Tags アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        // まず GITHUB_TOKEN を試し、次に GH_TOKEN を試す
        let token = select_github_token(
            ["GITHUB_TOKEN", "GH_TOKEN"]
                .into_iter()
                .map(|key| std::env::var(key).ok()),
        );

        Self { client, token }
    }

    /// リポジトリ用のタグ URL を構築
    fn build_url(&self, owner_repo: &str) -> String {
        format!(
            "{}/repos/{}/tags?per_page={}",
            GITHUB_API_URL, owner_repo, TAGS_PER_PAGE
        )
    }

    /// パッケージ名が "owner/repo" 形式かつ GitHub の許容文字のみであることを検証する。
    /// owner/repo に `?` `#` `/` `..` 等が混ざると `build_url` で URL クエリ汚染や
    /// パストラバーサルが起きうるため、Maven Central と共通の検証
    /// (`is_valid_registry_id_segment`) で文字種を限定する。
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        let parts: Vec<&str> = package.split('/').collect();
        if parts.len() != 2
            || !is_valid_registry_id_segment(parts[0])
            || !is_valid_registry_id_segment(parts[1])
        {
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
/// URL 例: `<https://api.github.com/...?page=2>; rel="next", <https://api.github.com/...?page=5>; rel="last"`
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

        // 次ページが残ったままページ上限に達したか (取りこぼしの通知用)
        let mut truncated = false;

        // `Link` ヘッダの rel="next" を辿って全ページ取得する
        // (per_page=100 の 1 ページのみだと 100 タグ超のリポジトリで最新タグを取り逃す)。
        // 安全弁として最大 MAX_TAG_PAGES ページ (= 1000 タグ) まで。
        for page in 0..MAX_TAG_PAGES {
            // 適切なヘッダ付きでリクエストを構築し、client.rs の共通リトライ経路
            // (429/5xx の再試行 + Retry-After 尊重 + 送信エラー変換) で実行する。
            // 403 / 401 は GitHub 固有の解釈が必要なため、リトライ経路では変換せず
            // 生の Response を受け取ってこちらでステータスを最終判定する。
            let response = self
                .client
                .get_response_with_retry(
                    || {
                        let mut request = self.client.inner().get(&url);
                        request = request.header("Accept", "application/vnd.github+json");
                        if let Some(ref token) = self.token {
                            request = request.header("Authorization", format!("Bearer {}", token));
                        }
                        request
                    },
                    package,
                    self.registry_name(),
                )
                .await?;

            // HTTP ステータスコードを処理。
            // GitHub 固有の解釈が必要な 403 / 401 を先に処理し、
            // 404 / 429 / その他の非成功ステータスは client.rs の共通マッピングへ委ねる。
            let status = response.status();
            if status == reqwest::StatusCode::FORBIDDEN {
                // GitHub はレート制限超過を 403 + X-RateLimit-Remaining: 0 で返す。
                // 認証エラーと区別して報告する (classify_forbidden を参照)。
                let remaining = response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok());
                return Err(classify_forbidden(remaining));
            }
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(RegistryError::AuthenticationError {
                    registry: self.registry_name().to_string(),
                    message: format!("HTTP {}", status),
                });
            }
            if let Some(error) = map_status_error(status, package, self.registry_name()) {
                return Err(error);
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
                // タグ名から semver を抽出 (SPM が読めない形のタグはここで落とす)
                if let Some(version) = extract_semver_tag(&tag.name) {
                    // GitHub Tags API はリリース日を返さない。
                    // `Utc::now()` を使うと `--age` フィルタが全 Swift 更新を抑制してしまうため、
                    // age フィルタを通過させるための「十分古い」値として UNIX_EPOCH を採用する。
                    versions.push(VersionInfo::new(version, DateTime::<Utc>::UNIX_EPOCH));
                }
            }

            truncated = is_page_limit_truncated(page, next_url.is_some());
            match next_url {
                Some(next) => url = next,
                None => break,
            }
        }

        // ページ上限で打ち切った場合は取りこぼしを通知する (無言の切り捨てを避ける)
        if truncated {
            eprintln!("{}", page_limit_warning(package));
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
    fn test_extract_semver_tag_matches() {
        // 安定版タグ
        assert!(extract_semver_tag("1.0.0").is_some());
        assert!(extract_semver_tag("v1.0.0").is_some());
        assert!(extract_semver_tag("V1.0.0").is_some());
        assert!(extract_semver_tag("v10.20.30").is_some());
        // SPM は semver 2.0.0 準拠なのでプレリリース/ビルドメタデータ付きタグも取得対象に含める
        // (安定版/プレリリースの選別は UpdateJudge::stable_candidates に委ねる)
        assert!(extract_semver_tag("1.0.0-beta.1").is_some());
        assert!(extract_semver_tag("v1.0.0-rc.1").is_some());
        assert!(extract_semver_tag("V1.0.0-rc.1+sha.abc").is_some());
        assert!(extract_semver_tag("1.0.0+build.123").is_some());
        // 不正な形式は弾く
        assert!(extract_semver_tag("1.0").is_none());
        assert!(extract_semver_tag("v1.0").is_none());
        assert!(extract_semver_tag("not-a-version").is_none());
        assert!(extract_semver_tag("1.0.0-").is_none()); // 末尾ハイフンのみは不可
        assert!(extract_semver_tag("1.0.0+").is_none()); // 末尾プラスのみは不可
        assert!(extract_semver_tag("1.0.0-alpha..1").is_none()); // 空の識別子は不可
    }

    #[test]
    fn test_extract_semver_tag_extracts_version() {
        assert_eq!(extract_semver_tag("v1.2.3"), Some("1.2.3"));
        assert_eq!(extract_semver_tag("V1.2.3"), Some("1.2.3"));
        assert_eq!(extract_semver_tag("1.2.3"), Some("1.2.3"));

        // プレリリース/ビルドメタデータも含めて返す ('v' プレフィックスのみ除去)
        assert_eq!(extract_semver_tag("v1.2.3-beta.1"), Some("1.2.3-beta.1"));
        assert_eq!(
            extract_semver_tag("1.2.3-rc.1+sha.abc"),
            Some("1.2.3-rc.1+sha.abc")
        );
    }

    /// バグ回帰テスト: 先頭ゼロを含むタグは候補にしない。
    ///
    /// SPM の `Version` は semver 2.0.0 の厳格パースなので `2024.01.15` のような
    /// CalVer タグを `Package.swift` に書き込むと `swift build` が
    /// `Invalid semantic version string` で manifest ごと読み込みに失敗する。しかも
    /// `2024 > 1` なので通常の semver タグと混在するリポジトリでは必ず最新として
    /// 選ばれてしまう。以前は取得側の正規表現だけが先頭ゼロを許していた。
    #[test]
    fn test_extract_semver_tag_rejects_leading_zeros() {
        assert!(extract_semver_tag("2024.01.15").is_none());
        assert!(extract_semver_tag("v2024.01.15").is_none());
        assert!(extract_semver_tag("1.02.0").is_none());
        assert!(extract_semver_tag("1.2.03").is_none());
        assert!(extract_semver_tag("01.2.3").is_none());
        // プレリリースの数値識別子も先頭ゼロは invalid (semver 2.0.0)
        assert!(extract_semver_tag("1.2.3-01").is_none());
    }

    /// 取得側 (GitHub Tags) とマニフェスト側 (`Package.swift`) の受理範囲が一致すること。
    /// 二重管理していた正規表現を `SwiftVersionParser` へ委譲した回帰テスト。
    #[test]
    fn test_extract_semver_tag_agrees_with_manifest_parser() {
        for tag in [
            "1.2.3",
            "v1.2.3",
            "2024.01.15",
            "1.02.0",
            "1.2.03",
            "1.0.0-beta.1",
            "1.0.0+build.123",
            "1.0.0-rc.1+sha.abc",
            "1.2.3-01",
            "1.0",
            "not-a-version",
        ] {
            let extracted = extract_semver_tag(tag);
            let body = tag.strip_prefix(['v', 'V']).unwrap_or(tag);
            assert_eq!(
                extracted.is_some(),
                SwiftVersionParser.parse(body).is_some(),
                "tag {} disagrees with SwiftVersionParser",
                tag
            );
        }
    }

    #[test]
    fn test_extract_semver_tag_rejects_whitespace() {
        // タグ名に空白が混ざる形は受理しない (パーサ側の trim に引きずられない)
        assert!(extract_semver_tag(" 1.2.3").is_none());
        assert!(extract_semver_tag("1.2.3 ").is_none());
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
        assert_eq!(TAGS_PER_PAGE, 100);
    }

    /// バグ回帰テスト: MAX_TAG_PAGES を使い切っても次ページが残っている場合は
    /// 打ち切りとして検知する。以前は無警告で切り捨てており、1000 タグを超える
    /// リポジトリで古い系列のタグが静かに候補から落ちていた。
    #[test]
    fn test_is_page_limit_truncated() {
        // 最終ページで次ページが残っている = 打ち切り
        assert!(is_page_limit_truncated(MAX_TAG_PAGES - 1, true));
        // 最終ページだが次ページがない = 全件取得できた
        assert!(!is_page_limit_truncated(MAX_TAG_PAGES - 1, false));
        // 途中のページで次ページがある = まだループが続くので打ち切りではない
        assert!(!is_page_limit_truncated(0, true));
        assert!(!is_page_limit_truncated(MAX_TAG_PAGES - 2, true));
    }

    #[test]
    fn test_page_limit_warning_mentions_package_and_limit() {
        let message = page_limit_warning("apple/swift-nio");
        assert!(message.contains("apple/swift-nio"));
        assert!(message.contains("10 pages"));
        assert!(message.contains("1000 tags"));
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
