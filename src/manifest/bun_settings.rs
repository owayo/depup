//! bun (`bunfig.toml`) から `minimumReleaseAge` を読み取る
//!
//! 形式:
//! ```toml
//! [install]
//! minimumReleaseAge = 259200  # 秒単位 (例: 3日)
//! ```
//!
//! 参考: <https://bun.com/docs/runtime/bunfig>

use std::path::Path;
use std::time::Duration;

/// `bunfig.toml` から読み取った install 関連設定
#[derive(Debug, Clone, Default)]
pub struct BunSettings {
    /// `[install].minimumReleaseAge` の値 (秒単位を `Duration` に変換)
    pub minimum_release_age: Option<Duration>,
}

impl BunSettings {
    /// 指定ディレクトリ直下の `bunfig.toml` を読む。
    /// 存在しない/読めない/パース失敗時は全フィールド `None`。
    pub fn from_dir(dir: &Path) -> Self {
        let path = dir.join("bunfig.toml");
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        Self::parse(&content)
    }

    /// 文字列をパースして設定を取り出す (テスト用に公開)
    pub fn parse(content: &str) -> Self {
        let parsed: toml::Value = match toml::from_str(content) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };

        let seconds = parsed
            .get("install")
            .and_then(|t| t.get("minimumReleaseAge"))
            .and_then(|v| v.as_integer())
            .filter(|n| *n >= 0)
            .map(|n| n as u64);

        Self {
            minimum_release_age: seconds.map(Duration::from_secs),
        }
    }
}

/// 指定ディレクトリに `bunfig.toml` が存在するか
pub fn has_bunfig(dir: &Path) -> bool {
    dir.join("bunfig.toml").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_install_section() {
        let content = r#"
[install]
minimumReleaseAge = 259200
"#;
        let settings = BunSettings::parse(content);
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(259200))
        );
    }

    #[test]
    fn test_parse_missing_section_returns_default() {
        let content = "registry = \"https://registry.npmjs.org\"\n";
        let settings = BunSettings::parse(content);
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_parse_invalid_toml_returns_default() {
        let content = "[install\nminimumReleaseAge = 100";
        let settings = BunSettings::parse(content);
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_parse_negative_is_ignored() {
        // 負数は不正値として無視
        let content = r#"
[install]
minimumReleaseAge = -10
"#;
        let settings = BunSettings::parse(content);
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_parse_string_value_is_ignored() {
        // bun の仕様は秒の整数値のみ。文字列は無視
        let content = r#"
[install]
minimumReleaseAge = "3d"
"#;
        let settings = BunSettings::parse(content);
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_parse_zero_is_valid() {
        let content = r#"
[install]
minimumReleaseAge = 0
"#;
        let settings = BunSettings::parse(content);
        assert_eq!(settings.minimum_release_age, Some(Duration::from_secs(0)));
    }

    #[test]
    fn test_from_dir_no_file() {
        let dir = TempDir::new().unwrap();
        let settings = BunSettings::from_dir(dir.path());
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_from_dir_with_file() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("bunfig.toml"),
            "[install]\nminimumReleaseAge = 86400\n",
        )
        .unwrap();
        let settings = BunSettings::from_dir(dir.path());
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(86400))
        );
    }

    #[test]
    fn test_has_bunfig_detects_file() {
        let dir = TempDir::new().unwrap();
        assert!(!has_bunfig(dir.path()));
        fs::write(dir.path().join("bunfig.toml"), "").unwrap();
        assert!(has_bunfig(dir.path()));
    }
}
