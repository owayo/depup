//! pnpm workspace settings reader
//!
//! Reads pnpm configuration from (in priority order):
//! - .npmrc (minimum-release-age=10d)
//! - pnpm-workspace.yaml (minimumReleaseAge: 14400) - value in minutes
//! - package.json (pnpm.settings.minimumReleaseAge)

use std::path::Path;
use std::time::Duration;

/// pnpm workspace settings
#[derive(Debug, Clone, Default)]
pub struct PnpmSettings {
    /// Minimum release age for packages
    pub minimum_release_age: Option<Duration>,
}

impl PnpmSettings {
    /// Read pnpm settings from a directory
    ///
    /// Checks in order of priority:
    /// 1. .npmrc (minimum-release-age setting)
    /// 2. pnpm-workspace.yaml (minimumReleaseAge in minutes)
    /// 3. package.json (pnpm.settings.minimumReleaseAge)
    pub fn from_dir(dir: &Path) -> Self {
        let mut settings = PnpmSettings::default();

        // Try reading from .npmrc first (highest priority)
        if let Some(age) = read_npmrc_minimum_release_age(dir) {
            settings.minimum_release_age = Some(age);
            return settings;
        }

        // Try reading from pnpm-workspace.yaml
        if let Some(age) = read_workspace_yaml_minimum_release_age(dir) {
            settings.minimum_release_age = Some(age);
            return settings;
        }

        // Try reading from package.json
        if let Some(age) = read_package_json_minimum_release_age(dir) {
            settings.minimum_release_age = Some(age);
        }

        settings
    }
}

/// Parse duration string in format: Nd (days), Nw (weeks), Nm (months)
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

    let seconds = match unit {
        'd' => num * 24 * 60 * 60,      // days
        'w' => num * 7 * 24 * 60 * 60,  // weeks
        'm' => num * 30 * 24 * 60 * 60, // months (30 days)
        _ => return None,
    };

    Some(Duration::from_secs(seconds))
}

/// Read minimum-release-age from .npmrc file
fn read_npmrc_minimum_release_age(dir: &Path) -> Option<Duration> {
    let npmrc_path = dir.join(".npmrc");
    let content = std::fs::read_to_string(npmrc_path).ok()?;

    for line in content.lines() {
        let line = line.trim();
        // Skip comments
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Look for minimum-release-age setting
        if let Some(value) = line.strip_prefix("minimum-release-age=") {
            // Handle quoted values like "10d" or '2w'
            let value = value.trim();
            let value = value.trim_matches('"').trim_matches('\'');
            return parse_duration(value);
        }
    }

    None
}

/// Read minimumReleaseAge from pnpm-workspace.yaml
///
/// The value in pnpm-workspace.yaml is in minutes (e.g., 14400 = 10 days)
fn read_workspace_yaml_minimum_release_age(dir: &Path) -> Option<Duration> {
    let workspace_path = dir.join("pnpm-workspace.yaml");
    let content = std::fs::read_to_string(workspace_path).ok()?;

    // Simple YAML parsing for minimumReleaseAge
    for line in content.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("minimumReleaseAge:") {
            let value = value.trim();
            // Value is in minutes
            if let Ok(minutes) = value.parse::<u64>() {
                return Some(Duration::from_secs(minutes * 60));
            }
            // Also support quoted string format like "10d"
            let value = value.trim_matches('"').trim_matches('\'');
            return parse_duration(value);
        }
    }

    None
}

/// Read minimumReleaseAge from package.json pnpm.settings
fn read_package_json_minimum_release_age(dir: &Path) -> Option<Duration> {
    let package_json_path = dir.join("package.json");
    let content = std::fs::read_to_string(package_json_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Look for pnpm.settings.minimumReleaseAge
    let age_str = json
        .get("pnpm")?
        .get("settings")?
        .get("minimumReleaseAge")?
        .as_str()?;

    parse_duration(age_str)
}

/// Check if a directory has pnpm workspace configuration
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
        // 14400 minutes = 10 days
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
        // Both pnpm-workspace.yaml and package.json have settings
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
        // pnpm-workspace.yaml takes priority over package.json
        assert_eq!(
            settings.minimum_release_age,
            Some(Duration::from_secs(14400 * 60))
        );
    }

    #[test]
    fn test_pnpm_settings_npmrc_takes_priority() {
        let dir = create_temp_dir();
        // Both .npmrc and package.json have settings
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
        // .npmrc takes priority
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
}
