//! pnpm ワークスペース設定の読み取り
//!
//! 以下の優先順で pnpm 設定を読み取る:
//! - .npmrc (minimum-release-age=14400 / minimum-release-age = 10d) - 裸の数値は分単位
//! - pnpm-workspace.yaml (minimumReleaseAge: 14400) - 値は分単位
//! - package.json (pnpm.minimumReleaseAge / pnpm.settings.minimumReleaseAge) - 数値は分単位

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
    /// 3. package.json (pnpm.minimumReleaseAge / pnpm.settings.minimumReleaseAge)
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
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 'm')
    } else {
        return None;
    };

    let num: u64 = num_str.parse().ok()?;

    // checked_mul でオーバーフローを防止（不正な設定値による panic/wrap を回避）
    let seconds = match unit {
        'd' => num.checked_mul(86_400)?,
        'w' => num.checked_mul(604_800)?,
        'm' => num.checked_mul(2_592_000)?,
        _ => return None,
    };

    Some(Duration::from_secs(seconds))
}

/// minimumReleaseAge の設定値をパースする
///
/// pnpm ネイティブの裸の数値 (例: `14400`) は分単位として解釈し、
/// `10d` のようなサフィックス付きは `parse_duration` に委ねる
fn parse_release_age_value(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(minutes) = value.parse::<u64>() {
        // オーバーフロー防止: checked_mul で安全に変換する
        return minutes.checked_mul(60).map(Duration::from_secs);
    }
    parse_duration(value)
}

/// .npmrc ファイルから minimum-release-age を読み取る
///
/// `key=value` に加えて `key = value` (= の前後の空白あり) も受け付ける。
/// 裸の数値は pnpm ネイティブの分単位として解釈する
fn read_npmrc_minimum_release_age(dir: &Path) -> Option<Duration> {
    let npmrc_path = dir.join(".npmrc");
    let content = std::fs::read_to_string(npmrc_path).ok()?;

    for line in content.lines() {
        let line = line.trim();
        // コメントをスキップする
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // minimum-release-age 設定を探す (= の前後の空白を許容)
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "minimum-release-age" {
            continue;
        }

        // "10d" や '2w' のようなクォート付き値を処理する
        let value = value.trim().trim_matches('"').trim_matches('\'');
        return parse_release_age_value(value);
    }

    None
}

/// pnpm-workspace.yaml から minimumReleaseAge を読み取る
///
/// pnpm-workspace.yaml の値は分単位 (例: 14400 = 10日)
fn read_workspace_yaml_minimum_release_age(dir: &Path) -> Option<Duration> {
    let workspace_path = dir.join("pnpm-workspace.yaml");
    let content = std::fs::read_to_string(workspace_path).ok()?;

    // minimumReleaseAge の簡易 YAML パース
    for line in content.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("minimumReleaseAge:") {
            // `14400 # comment` のようなインラインコメントを取り除く
            let value = value.split('#').next().unwrap_or(value).trim();
            // "10d" のようなクォート付き文字列形式もサポートする
            let value = value.trim_matches('"').trim_matches('\'');
            // 裸の数値は分単位、サフィックス付きは期間として解釈する
            return parse_release_age_value(value);
        }
    }

    None
}

/// package.json の pnpm 設定から minimumReleaseAge を読み取る
///
/// `pnpm.minimumReleaseAge` と `pnpm.settings.minimumReleaseAge` の両方を受け付け、
/// 数値型 (例: `14400`) は pnpm ネイティブの分単位として解釈する
fn read_package_json_minimum_release_age(dir: &Path) -> Option<Duration> {
    let package_json_path = dir.join("package.json");
    let content = std::fs::read_to_string(package_json_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let pnpm = json.get("pnpm")?;
    let value = pnpm.get("minimumReleaseAge").or_else(|| {
        pnpm.get("settings")
            .and_then(|settings| settings.get("minimumReleaseAge"))
    })?;

    // 数値型は分として解釈する
    if let Some(minutes) = value.as_u64() {
        // オーバーフロー防止: checked_mul で安全に変換する
        return minutes.checked_mul(60).map(Duration::from_secs);
    }
    parse_release_age_value(value.as_str()?)
}

/// ディレクトリに pnpm ワークスペース設定があるかどうかを判定する
pub fn has_pnpm_workspace(dir: &Path) -> bool {
    dir.join("pnpm-workspace.yaml").exists() || dir.join("pnpm-lock.yaml").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
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
