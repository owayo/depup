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
use chrono::Utc;
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

/// GitHub API のベース URL
const GITHUB_API_URL: &str = "https://api.github.com";

/// semver タグパターン ('v' プレフィックスは任意)
static SEMVER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[vV]?(\d+\.\d+\.\d+)$").unwrap());

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

    /// パッケージ名が "owner/repo" 形式であることを検証
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        let parts: Vec<&str> = package.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "expected format: owner/repo".to_string(),
            });
        }
        Ok(())
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

        let url = self.build_url(package);

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
            status
                if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN =>
            {
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

        let tags: Vec<GitHubTag> =
            response
                .json()
                .await
                .map_err(|e| RegistryError::InvalidResponse {
                    package: package.to_string(),
                    registry: self.registry_name().to_string(),
                    message: format!("failed to parse JSON: {}", e),
                })?;

        let mut versions = Vec::new();
        let now = Utc::now();

        for tag in tags {
            // タグ名から semver を抽出
            if let Some(caps) = SEMVER_RE.captures(&tag.name) {
                let version = caps.get(1).unwrap().as_str();
                // リリース日のフォールバックとして現在時刻を使用
                versions.push(VersionInfo::new(version, now));
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
        assert!(SEMVER_RE.is_match("1.0.0"));
        assert!(SEMVER_RE.is_match("v1.0.0"));
        assert!(SEMVER_RE.is_match("V1.0.0"));
        assert!(SEMVER_RE.is_match("v10.20.30"));
        assert!(!SEMVER_RE.is_match("1.0"));
        assert!(!SEMVER_RE.is_match("v1.0"));
        assert!(!SEMVER_RE.is_match("not-a-version"));
        assert!(!SEMVER_RE.is_match("1.0.0-beta.1"));
    }

    #[test]
    fn test_semver_regex_extracts_version() {
        let caps = SEMVER_RE.captures("v1.2.3").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3");

        let caps = SEMVER_RE.captures("V1.2.3").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3");

        let caps = SEMVER_RE.captures("1.2.3").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "1.2.3");
    }

    #[test]
    fn test_deserialize_github_tag() {
        let json = r#"{"name": "1.0.0", "zipball_url": "...", "tarball_url": "...", "commit": {"sha": "abc", "url": "..."}}"#;
        let tag: GitHubTag = serde_json::from_str(json).unwrap();
        assert_eq!(tag.name, "1.0.0");
    }
}
