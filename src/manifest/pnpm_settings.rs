//! pnpm ワークスペース設定の読み取り
//!
//! 以下の優先順で pnpm 設定を読み取る:
//! - .npmrc (minimum-release-age=14400 / minimum-release-age = 10d) - 裸の数値は分単位
//! - pnpm-workspace.yaml (minimumReleaseAge: 14400) - 値は分単位
//! - package.json (pnpm.minimumReleaseAge / pnpm.settings.minimumReleaseAge) - 数値は分単位

use crate::domain::{checked_age, checked_age_from_minutes};
use std::path::Path;
use std::time::Duration;

/// pnpm ワークスペース設定
#[derive(Debug, Clone, Default)]
pub struct PnpmSettings {
    /// パッケージの最低リリース経過時間
    pub minimum_release_age: Option<Duration>,
}

impl PnpmSettings {
    /// ディレクトリから pnpm 設定を読み取る
    ///
    /// 優先順に確認する:
    /// 1. .npmrc (minimum-release-age 設定)
    /// 2. pnpm-workspace.yaml (minimumReleaseAge、分単位)
    /// 3. package.json の設定（pnpm.minimumReleaseAge / pnpm.settings.minimumReleaseAge）
    pub fn from_dir(dir: &Path) -> Self {
        let mut settings = PnpmSettings::default();
        if let Some((age, _source)) = Self::minimum_release_age_with_source(dir) {
            settings.minimum_release_age = Some(age);
        }
        settings
    }

    /// minimumReleaseAge を値の出所 (ファイル名) 付きで読み取る
    ///
    /// 優先順は `from_dir` と同じ: .npmrc > pnpm-workspace.yaml > package.json。
    /// 通知メッセージなどで「どのファイル由来の設定か」を表示する用途に使う
    pub fn minimum_release_age_with_source(dir: &Path) -> Option<(Duration, &'static str)> {
        if let Some(age) = read_npmrc_minimum_release_age(dir) {
            return Some((age, ".npmrc"));
        }
        if let Some(age) = read_workspace_yaml_minimum_release_age(dir) {
            return Some((age, "pnpm-workspace.yaml"));
        }
        if let Some(age) = read_package_json_minimum_release_age(dir) {
            return Some((age, "package.json"));
        }
        None
    }

    /// 指定なしと、設定の読取・解釈失敗を区別する。
    /// 有効な値の優先順位は既存の読み取り経路と同じ。
    pub(crate) fn checked_minimum_release_age_with_source(
        dir: &Path,
    ) -> Result<Option<(Duration, &'static str)>, String> {
        for source in [".npmrc", "pnpm-workspace.yaml", "package.json"] {
            if let Some(age) = checked_release_age(dir, source)? {
                return Ok(Some((age, source)));
            }
        }
        Ok(None)
    }
}

/// 期間文字列をパースする。形式: Nd (日), Nw (週), Nm (月)
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, unit) = if let Some(n) = s.strip_suffix('d') {
        (n, 'd')
    } else if let Some(n) = s.strip_suffix('w') {
        (n, 'w')
    } else {
        (s.strip_suffix('m')?, 'm')
    };

    let num: u64 = num_str.parse().ok()?;

    // checked_mul でオーバーフローを防止（不正な設定値による panic/wrap を回避）
    let seconds = match unit {
        'd' => num.checked_mul(86_400)?,
        'w' => num.checked_mul(604_800)?,
        'm' => num.checked_mul(2_592_000)?,
        _ => return None,
    };

    // 乗算が通っても chrono の DateTime 範囲を超える値 (例: `999999999d`) は
    // カットオフ算出で panic するため、CLI の `--age` と同じ上限で弾く。
    checked_age(seconds)
}

/// minimumReleaseAge の設定値をパースする
///
/// pnpm ネイティブの裸の数値 (例: `14400`) は分単位として解釈し、
/// `10d` のようなサフィックス付きは `parse_duration` に委ねる
fn parse_release_age_value(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(minutes) = value.parse::<u64>() {
        // オーバーフローと上限超過の両方を弾く (巨大値でカットオフ算出が panic しないように)
        return checked_age_from_minutes(minutes);
    }
    parse_duration(value)
}

fn read_npmrc_minimum_release_age(dir: &Path) -> Option<Duration> {
    checked_release_age(dir, ".npmrc").ok().flatten()
}

fn read_workspace_yaml_minimum_release_age(dir: &Path) -> Option<Duration> {
    checked_release_age(dir, "pnpm-workspace.yaml")
        .ok()
        .flatten()
}

fn read_package_json_minimum_release_age(dir: &Path) -> Option<Duration> {
    checked_release_age(dir, "package.json").ok().flatten()
}

fn checked_release_age(dir: &Path, source: &str) -> Result<Option<Duration>, String> {
    let path = dir.join(source);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect {source}: {error}")),
        Ok(_) => {}
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {source}: {error}"))?;
    let raw = match source {
        ".npmrc" => content.lines().find_map(|line| {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            (key.trim() == "minimum-release-age").then(|| value.trim().to_string())
        }),
        "pnpm-workspace.yaml" => content.lines().find_map(|line| {
            line.trim()
                .strip_prefix("minimumReleaseAge:")
                .map(|value| value.split('#').next().unwrap_or(value).trim().to_string())
        }),
        _ => {
            let json: serde_json::Value = serde_json::from_str(&content)
                .map_err(|error| format!("invalid {source}: {error}"))?;
            json.get("pnpm")
                .and_then(|pnpm| {
                    pnpm.get("minimumReleaseAge").or_else(|| {
                        pnpm.get("settings")
                            .and_then(|settings| settings.get("minimumReleaseAge"))
                    })
                })
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string())
                })
        }
    };
    if let Some(raw) = raw {
        let value = raw.trim().trim_matches('"').trim_matches('\'');
        let age = parse_release_age_value(value)
            .ok_or_else(|| format!("invalid minimumReleaseAge in {source}"))?;
        return Ok(Some(age));
    }
    Ok(None)
}

/// ディレクトリに pnpm ワークスペース設定があるかどうかを判定する
pub fn has_pnpm_workspace(dir: &Path) -> bool {
    dir.join("pnpm-workspace.yaml").exists() || dir.join("pnpm-lock.yaml").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_settings_distinguish_invalid_or_unreadable_from_absent() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            PnpmSettings::checked_minimum_release_age_with_source(temp.path()).unwrap(),
            None
        );
        for (source, content) in [
            (".npmrc", "minimum-release-age=invalid\n"),
            ("pnpm-workspace.yaml", "minimumReleaseAge: invalid\n"),
            ("package.json", r#"{"pnpm":{"minimumReleaseAge":false}}"#),
        ] {
            let path = temp.path().join(source);
            std::fs::write(&path, content).unwrap();
            assert!(
                PnpmSettings::checked_minimum_release_age_with_source(temp.path())
                    .unwrap_err()
                    .contains(source)
            );
            std::fs::remove_file(path).unwrap();
        }
        std::fs::create_dir(temp.path().join(".npmrc")).unwrap();
        assert!(
            PnpmSettings::checked_minimum_release_age_with_source(temp.path())
                .unwrap_err()
                .contains("could not read .npmrc")
        );
    }

    #[test]
    fn checked_settings_keep_valid_source_priority() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".npmrc"), "minimum-release-age=1w\n").unwrap();
        std::fs::write(
            temp.path().join("pnpm-workspace.yaml"),
            "minimumReleaseAge: 30d\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"pnpm":{"minimumReleaseAge":43200}}"#,
        )
        .unwrap();
        assert_eq!(
            PnpmSettings::checked_minimum_release_age_with_source(temp.path()).unwrap(),
            Some((Duration::from_secs(7 * 86400), ".npmrc"))
        );
        std::fs::remove_file(temp.path().join(".npmrc")).unwrap();
        assert_eq!(
            PnpmSettings::checked_minimum_release_age_with_source(temp.path()).unwrap(),
            Some((Duration::from_secs(30 * 86400), "pnpm-workspace.yaml"))
        );
        std::fs::remove_file(temp.path().join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(
            PnpmSettings::checked_minimum_release_age_with_source(temp.path()).unwrap(),
            Some((Duration::from_secs(30 * 86400), "package.json"))
        );
    }
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration("10d"), Some(Duration::from_secs(10 * 86400)));
    }

    #[test]
    fn test_parse_duration_weeks() {
        assert_eq!(parse_duration("1w"), Some(Duration::from_secs(7 * 86400)));
        assert_eq!(parse_duration("2w"), Some(Duration::from_secs(14 * 86400)));
    }

    #[test]
    fn test_parse_duration_months() {
        assert_eq!(parse_duration("1m"), Some(Duration::from_secs(30 * 86400)));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("10x"), None);
    }

    #[test]
    fn test_read_npmrc_minimum_release_age() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join(".npmrc"),
            "registry=https://registry.npmjs.org/\nminimum-release-age=10d\n",
        )
        .unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(10 * 86400)));
    }

    #[test]
    fn test_read_npmrc_minimum_release_age_with_quotes() {
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=\"2w\"\n").unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14 * 86400)));
    }

    #[test]
    fn test_read_npmrc_no_setting() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join(".npmrc"),
            "registry=https://registry.npmjs.org/\n",
        )
        .unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, None);
    }

    #[test]
    fn test_read_package_json_minimum_release_age() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "test",
                "pnpm": {
                    "settings": {
                        "minimumReleaseAge": "10d"
                    }
                }
            }"#,
        )
        .unwrap();

        let age = read_package_json_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(10 * 86400)));
    }

    #[test]
    fn test_read_package_json_no_pnpm_settings() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), r#"{"name": "test"}"#).unwrap();

        let age = read_package_json_minimum_release_age(dir.path());
        assert_eq!(age, None);
    }

    #[test]
    fn test_pnpm_settings_from_dir_npmrc() {
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=10d\n").unwrap();

        let settings = PnpmSettings::from_dir(dir.path());
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(10 * 86400))
        );
    }

    #[test]
    fn test_pnpm_settings_from_dir_package_json() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "pnpm": {
                    "settings": {
                        "minimumReleaseAge": "2w"
                    }
                }
            }"#,
        )
        .unwrap();

        let settings = PnpmSettings::from_dir(dir.path());
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(14 * 86400))
        );
    }

    #[test]
    fn test_read_workspace_yaml_minimum_release_age_minutes() {
        let dir = create_temp_dir();
        // 14400 分 = 10 日
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        let age = read_workspace_yaml_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14400 * 60)));
    }

    #[test]
    fn test_read_workspace_yaml_minimum_release_age_string() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: \"10d\"\n",
        )
        .unwrap();

        let age = read_workspace_yaml_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(10 * 86400)));
    }

    #[test]
    fn test_pnpm_settings_from_dir_workspace_yaml() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        let settings = PnpmSettings::from_dir(dir.path());
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(14400 * 60))
        );
    }

    #[test]
    fn test_pnpm_settings_workspace_yaml_priority_over_package_json() {
        let dir = create_temp_dir();
        // pnpm-workspace.yaml と package.json の両方に設定がある
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "pnpm": {
                    "settings": {
                        "minimumReleaseAge": "2w"
                    }
                }
            }"#,
        )
        .unwrap();

        let settings = PnpmSettings::from_dir(dir.path());
        // pnpm-workspace.yaml が package.json より優先される
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(14400 * 60))
        );
    }

    #[test]
    fn test_pnpm_settings_npmrc_takes_priority() {
        let dir = create_temp_dir();
        // .npmrc と package.json の両方に設定がある
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=10d\n").unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "pnpm": {
                    "settings": {
                        "minimumReleaseAge": "2w"
                    }
                }
            }"#,
        )
        .unwrap();

        let settings = PnpmSettings::from_dir(dir.path());
        // .npmrc が優先される
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(10 * 86400))
        );
    }

    #[test]
    fn test_pnpm_settings_from_dir_no_settings() {
        let dir = create_temp_dir();
        let settings = PnpmSettings::from_dir(dir.path());
        assert_eq!(settings.minimum_release_age, None);
    }

    #[test]
    fn test_has_pnpm_workspace() {
        let dir = create_temp_dir();
        assert!(!has_pnpm_workspace(dir.path()));

        fs::write(dir.path().join("pnpm-workspace.yaml"), "").unwrap();
        assert!(has_pnpm_workspace(dir.path()));
    }

    #[test]
    fn test_has_pnpm_workspace_lock_file() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert!(has_pnpm_workspace(dir.path()));
    }

    #[test]
    fn test_parse_duration_overflow() {
        // 巨大な数値でオーバーフローしないことを確認
        assert!(parse_duration("99999999999999999999d").is_none());
    }

    #[test]
    fn test_read_npmrc_minimum_release_age_bare_minutes() {
        // (回帰) pnpm ネイティブの分単位数値が読み取れる (14400 分 = 10 日)
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=14400\n").unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14400 * 60)));
    }

    #[test]
    fn test_read_npmrc_minimum_release_age_with_spaces_around_equals() {
        // (回帰) `key = value` 形式 (= の前後に空白) も読み取れる
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age = 10d\n").unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(10 * 86400)));
    }

    #[test]
    fn test_read_npmrc_minimum_release_age_spaces_and_bare_minutes() {
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age = 14400\n").unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14400 * 60)));
    }

    #[test]
    fn test_read_npmrc_does_not_match_prefixed_keys() {
        // `minimum-release-age-exclude` のような別キーには一致しない
        let dir = create_temp_dir();
        fs::write(
            dir.path().join(".npmrc"),
            "minimum-release-age-exclude[]=webpack\n",
        )
        .unwrap();

        let age = read_npmrc_minimum_release_age(dir.path());
        assert_eq!(age, None);
    }

    #[test]
    fn test_read_workspace_yaml_minimum_release_age_inline_comment() {
        // (回帰) インラインコメント付きでも値が読み取れる
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400 # 10 days\n",
        )
        .unwrap();

        let age = read_workspace_yaml_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14400 * 60)));
    }

    #[test]
    fn test_read_package_json_minimum_release_age_numeric() {
        // (回帰) pnpm.minimumReleaseAge の数値型 (分単位) を受け付ける
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "name": "test",
                "pnpm": {
                    "minimumReleaseAge": 14400
                }
            }"#,
        )
        .unwrap();

        let age = read_package_json_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14400 * 60)));
    }

    #[test]
    fn test_read_package_json_minimum_release_age_settings_numeric() {
        // pnpm.settings.minimumReleaseAge の数値型も受け付ける
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "pnpm": {
                    "settings": {
                        "minimumReleaseAge": 14400
                    }
                }
            }"#,
        )
        .unwrap();

        let age = read_package_json_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(14400 * 60)));
    }

    #[test]
    fn test_read_package_json_minimum_release_age_direct_string() {
        // pnpm.minimumReleaseAge 直下の文字列形式も受け付ける
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{
                "pnpm": {
                    "minimumReleaseAge": "10d"
                }
            }"#,
        )
        .unwrap();

        let age = read_package_json_minimum_release_age(dir.path());
        assert_eq!(age, Some(Duration::from_secs(10 * 86400)));
    }

    #[test]
    fn test_minimum_release_age_with_source_npmrc() {
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=14400\n").unwrap();

        let result = PnpmSettings::minimum_release_age_with_source(dir.path());
        assert_eq!(result, Some((Duration::from_secs(14400 * 60), ".npmrc")));
    }

    #[test]
    fn test_minimum_release_age_with_source_workspace_yaml() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400 # comment\n",
        )
        .unwrap();

        let result = PnpmSettings::minimum_release_age_with_source(dir.path());
        assert_eq!(
            result,
            Some((Duration::from_secs(14400 * 60), "pnpm-workspace.yaml"))
        );
    }

    #[test]
    fn test_minimum_release_age_with_source_package_json() {
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{"pnpm": {"minimumReleaseAge": 14400}}"#,
        )
        .unwrap();

        let result = PnpmSettings::minimum_release_age_with_source(dir.path());
        assert_eq!(
            result,
            Some((Duration::from_secs(14400 * 60), "package.json"))
        );
    }

    #[test]
    fn test_minimum_release_age_with_source_priority() {
        // .npmrc が pnpm-workspace.yaml / package.json より優先され、source も .npmrc になる
        let dir = create_temp_dir();
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=10d\n").unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"pnpm": {"minimumReleaseAge": 1440}}"#,
        )
        .unwrap();

        let result = PnpmSettings::minimum_release_age_with_source(dir.path());
        assert_eq!(result, Some((Duration::from_secs(10 * 86400), ".npmrc")));
    }

    #[test]
    fn test_minimum_release_age_with_source_none() {
        let dir = create_temp_dir();
        assert_eq!(
            PnpmSettings::minimum_release_age_with_source(dir.path()),
            None
        );
    }
}
