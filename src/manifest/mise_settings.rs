//! mise の `[settings]` から `minimum_release_age` を読み取る
//!
//! ```toml
//! [settings]
//! minimum_release_age = "7d"
//! minimum_release_age_excludes = ["trivy", "npm:*"]
//! ```
//!
//! mise 側の既定値は 24h だが、それは「明示的な設定がない」状態なので採用しない
//! (`mise settings get minimum_release_age` も未設定ならエラーを返す)。
//! ファイルに書かれている場合だけ、pnpm の `minimumReleaseAge` や bun の
//! `bunfig.toml` と同じ「プロジェクトポリシー」として扱う。
//!
//! 参考: <https://mise.jdx.dev/configuration/settings.html#minimum_release_age>

use crate::domain::checked_age;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// プロジェクト直下で mise が読む設定ファイル (優先度の高い順)。
///
/// `mise.local.toml` / `.mise.local.toml` は個人のローカル上書き (通常 gitignore)
/// なので、depup は更新対象にも age の読み取り元にもしない。
pub const MISE_CONFIG_FILENAMES: &[&str] = &[
    "mise.toml",
    ".mise.toml",
    "mise/config.toml",
    ".mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
];

/// mise の設定から読み取った値
#[derive(Debug, Clone, Default)]
pub struct MiseSettings {
    /// `[settings] minimum_release_age` (明示指定がある場合のみ)
    pub minimum_release_age: Option<Duration>,
    /// 値の出所となったファイル名 (通知表示用)
    pub source: Option<String>,
    /// `[settings] minimum_release_age_excludes` に指定されたツール
    pub minimum_release_age_excludes: Vec<String>,
}

impl MiseSettings {
    /// 指定ディレクトリ直下の mise 設定ファイルから設定を読む。
    ///
    /// 複数のファイルがある場合は mise の優先順 (`MISE_CONFIG_FILENAMES` の順) に
    /// 最初に見つかった `minimum_release_age` を採用する。
    pub fn from_dir(dir: &Path) -> Self {
        let mut merged = MiseSettings::default();
        for filename in MISE_CONFIG_FILENAMES {
            let path = dir.join(filename);
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let parsed = Self::parse(&content);
            if merged.minimum_release_age.is_none()
                && let Some(age) = parsed.minimum_release_age
            {
                merged.minimum_release_age = Some(age);
                merged.source = Some((*filename).to_string());
            }
            if merged.minimum_release_age_excludes.is_empty() {
                merged.minimum_release_age_excludes = parsed.minimum_release_age_excludes;
            }
        }
        merged
    }

    /// TOML 文字列から設定を取り出す (テスト用に公開)
    pub fn parse(content: &str) -> Self {
        let Ok(parsed) = toml::from_str::<toml::Value>(content) else {
            return Self::default();
        };
        let Some(settings) = parsed.get("settings").and_then(|v| v.as_table()) else {
            return Self::default();
        };

        let minimum_release_age = settings
            .get("minimum_release_age")
            .and_then(|v| v.as_str())
            .and_then(parse_mise_duration);

        let minimum_release_age_excludes = settings
            .get("minimum_release_age_excludes")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            minimum_release_age,
            source: None,
            minimum_release_age_excludes,
        }
    }
}

/// 指定ディレクトリに mise の設定ファイルがあるか
pub fn has_mise_config(dir: &Path) -> bool {
    MISE_CONFIG_FILENAMES
        .iter()
        .any(|name| dir.join(name).is_file())
}

/// ディレクトリ直下に存在する mise 設定ファイルのパスを優先度順に返す
pub fn mise_config_paths(dir: &Path) -> Vec<PathBuf> {
    MISE_CONFIG_FILENAMES
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .collect()
}

/// mise の duration 表記を `Duration` に変換する。
///
/// mise は Rust の humantime 系表記を使う (`30s` / `10m` / `1h` / `7d` / `2w` /
/// `1y`)。**`m` は分**であり、depup CLI の `--age` (`m` = 月) とは意味が違うため、
/// ここで専用にパースする。
///
/// `mise ls-remote --minimum-release-age` は絶対日付 (`2024-06-01`) も受け付けるが、
/// depup の age は「現在からの経過時間」なので相対表記だけを採用し、
/// 絶対日付は `None` (設定なし扱い) にする。
fn parse_mise_duration(value: &str) -> Option<Duration> {
    let text = value.trim();
    if text.is_empty() {
        return None;
    }

    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if digits == 0 {
        return None;
    }
    let (number, unit) = text.split_at(digits);
    let amount: u64 = number.parse().ok()?;

    let unit_secs: u64 = match unit.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        // humantime と同じく `m` は分
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
        "d" | "day" | "days" => 24 * 60 * 60,
        "w" | "week" | "weeks" => 7 * 24 * 60 * 60,
        "M" | "month" | "months" => 30 * 24 * 60 * 60,
        "y" | "year" | "years" => 365 * 24 * 60 * 60,
        // 単位なしの裸の数値は解釈が割れる (mise 側も受け付けない) ので採用しない。
        // `2024-06-01` のような絶対日付もここに落ちる。
        _ => return None,
    };

    amount.checked_mul(unit_secs).and_then(checked_age)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_minimum_release_age() {
        let settings = MiseSettings::parse("[settings]\nminimum_release_age = \"7d\"\n");
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(7 * 86400))
        );
    }

    #[test]
    fn test_parse_units() {
        assert_eq!(parse_mise_duration("30s"), Some(Duration::from_secs(30)));
        // humantime 準拠で `m` は分 (depup CLI の --age は月なので要注意)
        assert_eq!(parse_mise_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_mise_duration("24h"), Some(Duration::from_secs(86400)));
        assert_eq!(
            parse_mise_duration("7d"),
            Some(Duration::from_secs(7 * 86400))
        );
        assert_eq!(
            parse_mise_duration("2w"),
            Some(Duration::from_secs(14 * 86400))
        );
        assert_eq!(
            parse_mise_duration("1y"),
            Some(Duration::from_secs(365 * 86400))
        );
    }

    #[test]
    fn test_parse_rejects_absolute_date_and_garbage() {
        assert_eq!(parse_mise_duration("2024-06-01"), None);
        assert_eq!(parse_mise_duration("soon"), None);
        assert_eq!(parse_mise_duration(""), None);
        // 単位なしの裸の数値も採用しない
        assert_eq!(parse_mise_duration("3600"), None);
    }

    #[test]
    fn test_parse_rejects_overflow() {
        assert_eq!(parse_mise_duration("999999999999y"), None);
        assert_eq!(parse_mise_duration("18446744073709551615d"), None);
    }

    #[test]
    fn test_parse_without_settings_section() {
        let settings = MiseSettings::parse("[tools]\nnode = \"26.7.0\"\n");
        assert!(settings.minimum_release_age.is_none());
        assert!(settings.minimum_release_age_excludes.is_empty());
    }

    #[test]
    fn test_parse_excludes() {
        let settings = MiseSettings::parse(
            "[settings]\nminimum_release_age = \"7d\"\nminimum_release_age_excludes = [\"trivy\", \"npm:*\"]\n",
        );
        assert_eq!(
            settings.minimum_release_age_excludes,
            vec!["trivy", "npm:*"]
        );
    }

    #[test]
    fn test_parse_invalid_toml_is_default() {
        let settings = MiseSettings::parse("[settings\nminimum_release_age = ");
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_from_dir_reads_mise_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("mise.toml"),
            "[settings]\nminimum_release_age = \"14d\"\n[tools]\nnode = \"26.7.0\"\n",
        )
        .unwrap();

        let settings = MiseSettings::from_dir(dir.path());
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(14 * 86400))
        );
        assert_eq!(settings.source.as_deref(), Some("mise.toml"));
    }

    #[test]
    fn test_from_dir_prefers_higher_priority_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".config/mise")).unwrap();
        fs::write(
            dir.path().join(".config/mise/config.toml"),
            "[settings]\nminimum_release_age = \"30d\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("mise.toml"),
            "[settings]\nminimum_release_age = \"3d\"\n",
        )
        .unwrap();

        let settings = MiseSettings::from_dir(dir.path());
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(3 * 86400))
        );
        assert_eq!(settings.source.as_deref(), Some("mise.toml"));
    }

    /// ローカル上書き (`mise.local.toml`) は読まない
    #[test]
    fn test_from_dir_ignores_local_override() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("mise.local.toml"),
            "[settings]\nminimum_release_age = \"90d\"\n",
        )
        .unwrap();

        let settings = MiseSettings::from_dir(dir.path());
        assert!(settings.minimum_release_age.is_none());
    }

    #[test]
    fn test_has_mise_config() {
        let dir = TempDir::new().unwrap();
        assert!(!has_mise_config(dir.path()));
        fs::write(dir.path().join("mise.toml"), "[tools]\n").unwrap();
        assert!(has_mise_config(dir.path()));
    }

    #[test]
    fn test_mise_config_paths_are_ordered() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".mise.toml"), "[tools]\n").unwrap();
        fs::write(dir.path().join("mise.toml"), "[tools]\n").unwrap();

        let paths = mise_config_paths(dir.path());
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("mise.toml"));
        assert!(paths[1].ends_with(".mise.toml"));
    }
}
