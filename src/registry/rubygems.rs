//! RubyGems レジストリアダプタ
//!
//! RubyGems レジストリからパッケージバージョン情報を取得する。
//! API エンドポイント: https://rubygems.org/api/v1/versions/{gem}.json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter};
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// RubyGems レジストリのベース URL
const RUBYGEMS_API_URL: &str = "https://rubygems.org/api/v1/versions";

/// RubyGems レジストリアダプタ
pub struct RubyGemsAdapter {
    client: HttpClient,
}

/// RubyGems API レスポンスのバージョン情報
#[derive(Debug, Deserialize)]
struct RubyGemsVersionInfo {
    /// バージョン番号 (例: "7.1.0")
    number: String,
    /// 作成日時タイムスタンプ
    created_at: String,
    /// プラットフォーム (通常 "ruby")
    platform: Option<String>,
    /// このバージョンが yank されているか
    #[serde(default)]
    yanked: bool,
}

impl RubyGemsVersionInfo {
    /// ruby プラットフォーム向けのエントリかどうかを判定する
    ///
    /// RubyGems は同一バージョン番号を platform 別 (java / x86_64-linux 等) に
    /// 複数エントリで返すため、そのまま使うと候補の重複や異 platform の混入が起きる。
    /// デフォルトの "ruby" platform のみ採用する (フィールド欠損は "ruby" 扱い)。
    fn is_ruby_platform(&self) -> bool {
        self.platform.as_deref().unwrap_or("ruby") == "ruby"
    }
}

impl RubyGemsAdapter {
    /// 新しい RubyGems アダプタを作成
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// gem 用の URL を構築
    fn build_url(&self, gem: &str) -> String {
        format!("{}/{}.json", RUBYGEMS_API_URL, gem)
    }
}

#[async_trait]
impl RegistryAdapter for RubyGemsAdapter {
    fn language(&self) -> Language {
        Language::Ruby
    }

    fn registry_name(&self) -> &'static str {
        "rubygems"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let url = self.build_url(package);
        let response: Vec<RubyGemsVersionInfo> = self
            .client
            .get_json(&url, package, self.registry_name())
            .await?;

        let mut versions = Vec::new();

        for version_info in response {
            // yank されたバージョンをスキップ
            if version_info.yanked {
                continue;
            }

            // ruby 以外の platform 別エントリ (java / x86_64-linux 等) をスキップ
            // (同一バージョン番号の重複・異 platform 専用バージョンの混入を防ぐ)
            if !version_info.is_ruby_platform() {
                continue;
            }

            // 作成日時タイムスタンプをパース
            if let Ok(released_at) = version_info.created_at.parse::<DateTime<Utc>>() {
                versions.push(VersionInfo::new(&version_info.number, released_at));
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
    fn test_rubygems_adapter_language() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(adapter.language(), Language::Ruby);
    }

    #[test]
    fn test_rubygems_adapter_registry_name() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(adapter.registry_name(), "rubygems");
    }

    #[test]
    fn test_build_url() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(
            adapter.build_url("rails"),
            "https://rubygems.org/api/v1/versions/rails.json"
        );
    }

    #[test]
    fn test_build_url_with_dashes() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        assert_eq!(
            adapter.build_url("rspec-rails"),
            "https://rubygems.org/api/v1/versions/rspec-rails.json"
        );
    }

    #[test]
    fn test_deserialize_version_info() {
        let json = r#"{"number": "7.1.0", "created_at": "2023-10-05T12:00:00Z", "platform": "ruby", "yanked": false}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.number, "7.1.0");
        assert!(!info.yanked);
    }

    #[test]
    fn test_deserialize_version_info_yanked() {
        let json = r#"{"number": "7.0.0", "created_at": "2023-01-01T00:00:00Z", "yanked": true}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert!(info.yanked);
    }

    #[test]
    fn test_deserialize_version_info_minimal() {
        let json = r#"{"number": "1.0.0", "created_at": "2023-01-01T00:00:00Z"}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.number, "1.0.0");
        assert!(!info.yanked); // デフォルトは false
    }

    /// バグ回帰テスト: platform != "ruby" のエントリ (java 等) は候補から除外する。
    /// 以前は platform を見ていなかったため、同一バージョン番号の platform 別
    /// エントリが重複したり、異 platform 専用バージョンが候補に混入していた。
    #[test]
    fn test_java_platform_entry_is_filtered() {
        let json =
            r#"{"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z", "platform": "java"}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert!(!info.is_ruby_platform());
    }

    #[test]
    fn test_ruby_platform_entry_is_kept() {
        let json =
            r#"{"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z", "platform": "ruby"}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert!(info.is_ruby_platform());
    }

    #[test]
    fn test_missing_platform_treated_as_ruby() {
        // platform フィールド欠損は "ruby" 扱い
        let json = r#"{"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z"}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert!(info.is_ruby_platform());
    }

    #[test]
    fn test_native_platform_entries_are_filtered() {
        for platform in ["x86_64-linux", "arm64-darwin", "x64-mingw32", "jruby"] {
            let json = format!(
                r#"{{"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z", "platform": "{}"}}"#,
                platform
            );
            let info: RubyGemsVersionInfo = serde_json::from_str(&json).unwrap();
            assert!(
                !info.is_ruby_platform(),
                "platform {} should be filtered",
                platform
            );
        }
    }

    /// platform 混在レスポンスのフィルタ結果を検証する
    /// (nokogiri のように同一バージョンが ruby + 各ネイティブ platform で公開されるケース)
    #[test]
    fn test_platform_filter_on_mixed_response() {
        let json = r#"[
            {"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z", "platform": "ruby"},
            {"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z", "platform": "java"},
            {"number": "1.7.0", "created_at": "2023-01-01T00:00:00Z", "platform": "x86_64-linux"},
            {"number": "1.6.0", "created_at": "2022-01-01T00:00:00Z"}
        ]"#;
        let entries: Vec<RubyGemsVersionInfo> = serde_json::from_str(json).unwrap();
        let kept: Vec<&RubyGemsVersionInfo> =
            entries.iter().filter(|e| e.is_ruby_platform()).collect();
        // ruby platform (明示) + platform 欠損 の 2 件のみ残る
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].number, "1.7.0");
        assert_eq!(kept[1].number, "1.6.0");
    }
}
