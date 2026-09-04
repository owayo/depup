//! RubyGems レジストリアダプタ
//!
//! RubyGems レジストリからパッケージバージョン情報を取得する。
//! API エンドポイント: https://rubygems.org/api/v1/versions/{gem}.json

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::{HttpClient, RegistryAdapter, is_valid_registry_id_segment};
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
    /// 作成日時タイムスタンプ (欠損時は None → UNIX_EPOCH 扱い)
    #[serde(default)]
    created_at: Option<String>,
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

    /// gem 名が RubyGems の命名規則 ([A-Za-z0-9._-]) に沿うことを検証する
    ///
    /// gem 名は `build_url` で URL パスへ直接連結される。Gemfile 側の記述次第で
    /// `a/../rails` のようなドットセグメントや `?` / `#` が混ざると、URL の正規化で
    /// **別の gem** の版を取得して書き戻す (= 意図しない依存への差し替え) 恐れがある。
    /// Maven Central / GitHub Tags と共通の検証 (`is_valid_registry_id_segment`) を使う。
    fn validate_package_name(&self, package: &str) -> Result<(), RegistryError> {
        if !is_valid_registry_id_segment(package) {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: self.registry_name().to_string(),
                reason: "gem names may contain only [A-Za-z0-9._-] characters".to_string(),
            });
        }
        Ok(())
    }

    /// `created_at` からリリース日時を解決する
    ///
    /// 欠損またはパース不能な場合は UNIX_EPOCH (= 「十分古い」) へフォールバックする。
    /// 以前は候補ごと捨てていたため、日付が読めないバージョンが無言で候補から消え、
    /// 有効な更新を取りこぼしていた。`Utc::now()` をフォールバックに使うと、デフォルト
    /// 有効の age フィルタ (1w) が「リリース直後」とみなして永久に除外してしまう。
    /// packagist / go_proxy / github_tags と同じ「日付不明 = UNIX_EPOCH」方針に揃える。
    fn resolve_released_at(created_at: Option<&str>) -> DateTime<Utc> {
        created_at
            .and_then(|t| t.parse::<DateTime<Utc>>().ok())
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
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
        self.validate_package_name(package)?;

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
            // (欠損・不正時は UNIX_EPOCH = 「十分古い」扱い。候補ごと捨てると
            //  公開日不明のバージョンが無言で消え、有効な更新を取りこぼす)
            let released_at = Self::resolve_released_at(version_info.created_at.as_deref());
            versions.push(VersionInfo::new(&version_info.number, released_at));
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

    /// バグ回帰テスト: `created_at` が欠損・不正な場合も UNIX_EPOCH (= 十分古い) として
    /// 候補に残す。以前は `if let Ok(...)` に else が無く、日付を読めないバージョンを
    /// 丸ごと捨てていたため、有効な更新が無言で取りこぼされていた。
    #[test]
    fn test_resolve_released_at_missing_falls_back_to_epoch() {
        assert_eq!(
            RubyGemsAdapter::resolve_released_at(None),
            DateTime::<Utc>::UNIX_EPOCH
        );
        assert_eq!(
            RubyGemsAdapter::resolve_released_at(Some("not-a-date")),
            DateTime::<Utc>::UNIX_EPOCH
        );
        assert_eq!(
            RubyGemsAdapter::resolve_released_at(Some("")),
            DateTime::<Utc>::UNIX_EPOCH
        );
    }

    #[test]
    fn test_resolve_released_at_keeps_valid_timestamp() {
        // 正常なエントリの日付は従来どおり保持する
        let expected = "2023-10-05T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            RubyGemsAdapter::resolve_released_at(Some("2023-10-05T12:00:00Z")),
            expected
        );
    }

    #[test]
    fn test_deserialize_version_info_without_created_at() {
        // created_at が無いレスポンスでもデシリアライズは成功し、日付は epoch 扱いになる
        let json = r#"{"number": "1.0.0"}"#;
        let info: RubyGemsVersionInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.number, "1.0.0");
        assert_eq!(
            RubyGemsAdapter::resolve_released_at(info.created_at.as_deref()),
            DateTime::<Utc>::UNIX_EPOCH
        );
    }

    /// バグ回帰テスト: gem 名を URL パスへ連結する前に文字種を検証する。
    /// `a/../rails` のようなドットセグメントは URL 正規化で別 gem を指してしまう。
    #[test]
    fn test_validate_package_name_rejects_url_injection() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        for invalid in [
            "",
            "a/../rails",
            "a/b",
            "x?y=1",
            "x#frag",
            "..",
            ".",
            "with space",
            "rails\n",
            "レイルズ",
        ] {
            let err = adapter.validate_package_name(invalid).unwrap_err();
            assert!(
                matches!(err, RegistryError::InvalidPackageName { .. }),
                "expected InvalidPackageName for {:?}, got {:?}",
                invalid,
                err
            );
        }
    }

    #[test]
    fn test_validate_package_name_accepts_gem_names() {
        let client = HttpClient::new().unwrap();
        let adapter = RubyGemsAdapter::new(client);
        for valid in [
            "rails",
            "rspec-rails",
            "activerecord-import",
            "net-http",
            "concurrent-ruby",
            "ruby_parser",
            "rack.test",
            "i18n",
        ] {
            assert!(
                adapter.validate_package_name(valid).is_ok(),
                "expected {:?} to be accepted",
                valid
            );
        }
    }
}
