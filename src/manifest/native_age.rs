//! PM 自身の明示的な cooldown を、depup の既定値で弱めない。

use crate::domain::{Language, checked_age};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn native_min_age(
    language: Language,
    dir: &Path,
    now: DateTime<Utc>,
) -> Result<Option<Duration>, String> {
    let (env_key, config_name) = match language {
        Language::Python => ("UV_EXCLUDE_NEWER", "uv/uv.toml"),
        Language::Mise => ("MISE_MINIMUM_RELEASE_AGE", "mise/config.toml"),
        _ => return Ok(None),
    };
    let mut values = Vec::new();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")));
    let global = if language == Language::Mise {
        std::env::var_os("MISE_GLOBAL_CONFIG_FILE")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("MISE_CONFIG_DIR")
                    .map(|dir| PathBuf::from(dir).join("config.toml"))
            })
            .or_else(|| config_home.map(|home| home.join(config_name)))
    } else {
        config_home.map(|home| home.join(config_name))
    };
    if let Some(path) = global {
        values.extend(read_age_values(language, &path)?);
    }
    // native PM は実行ルートより上の設定も継承する。CLI 注入でその制約を弱めない。
    values.extend(read_scoped_values(language, dir)?);
    let custom_key = if language == Language::Python {
        "UV_CONFIG_FILE"
    } else {
        "MISE_CONFIG_FILE"
    };
    if let Some(path) = std::env::var_os(custom_key) {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            dir.join(path)
        };
        values.extend(read_age_values(language, &path)?);
    }
    if language == Language::Python {
        let system_dirs = std::env::var_os("XDG_CONFIG_DIRS")
            .map(|dirs| std::env::split_paths(&dirs).collect::<Vec<_>>())
            .unwrap_or_else(|| vec![PathBuf::from("/etc")]);
        for system_dir in system_dirs {
            values.extend(read_age_values(language, &system_dir.join("uv/uv.toml"))?);
        }
    }
    if let Ok(value) = std::env::var(env_key) {
        values.push(value);
    }
    values.into_iter().try_fold(None, |age, value| {
        parse_cutoff_age(&value, now)
            .map(|next| age.max(next))
            .map_err(|error| format!("{env_key}/native age setting: {error}"))
    })
}

fn read_scoped_values(language: Language, dir: &Path) -> Result<Vec<String>, String> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut values = Vec::new();
    let environments = std::env::var("MISE_ENV").unwrap_or_default();
    for scope in dir.ancestors() {
        if language == Language::Python {
            for name in ["uv.toml", "pyproject.toml"] {
                values.extend(read_age_values(language, &scope.join(name))?);
            }
        } else {
            for name in super::MISE_CONFIG_FILENAMES
                .iter()
                .chain(["mise.local.toml", ".mise.local.toml"].iter())
            {
                values.extend(read_age_values(language, &scope.join(name))?);
            }
            for environment in environments.split(',').filter(|env| {
                !env.is_empty()
                    && env
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            }) {
                for name in super::MISE_CONFIG_FILENAMES {
                    if let Some(stem) = name.strip_suffix(".toml") {
                        for name in [
                            format!("{stem}.{environment}.toml"),
                            format!("{stem}.{environment}.local.toml"),
                        ] {
                            values.extend(read_age_values(language, &scope.join(name))?);
                        }
                    }
                }
            }
        }
    }
    Ok(values)
}

fn read_age_values(language: Language, path: &Path) -> Result<Vec<String>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read native age settings: {error}")),
    };
    let document: toml::Value = toml::from_str(&content)
        .map_err(|error| format!("cannot parse native age settings: {error}"))?;
    let table = match language {
        Language::Python
            if path
                .file_name()
                .is_some_and(|name| name == "pyproject.toml") =>
        {
            document.get("tool").and_then(|tool| tool.get("uv"))
        }
        Language::Python => Some(&document),
        Language::Mise => document.get("settings"),
        _ => None,
    };
    let key = if language == Language::Mise {
        "minimum_release_age"
    } else {
        "exclude-newer"
    };
    let mut values = Vec::new();
    if let Some(value) = table.and_then(|table| table.get(key))
        && value.as_bool() != Some(false)
    {
        values.push(
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_datetime().map(ToString::to_string))
                .ok_or_else(|| format!("invalid native age setting {key}"))?,
        );
    }
    if language == Language::Python {
        // native の個別制約も、グローバル cutoff の注入で取りこぼさない。
        let package_values = table
            .and_then(|table| table.get("exclude-newer-package"))
            .and_then(toml::Value::as_table)
            .into_iter()
            .flat_map(|packages| packages.values());
        let index_values = table
            .and_then(|table| table.get("index"))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|index| index.get("exclude-newer"));
        for value in package_values.chain(index_values) {
            if value.as_bool() == Some(false) {
                continue;
            }
            values.push(
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("invalid native package/index age setting")?,
            );
        }
    }
    if language == Language::Mise {
        values.extend(
            super::MiseSettings::tool_minimum_release_ages(&content)
                .into_iter()
                .map(|tool| tool.raw),
        );
    }
    Ok(values)
}

/// RFC3339 / 日付 / 単位付き期間 / ISO8601 の固定長期間を受理する。
fn parse_cutoff_age(value: &str, now: DateTime<Utc>) -> Result<Option<Duration>, String> {
    let value = value.trim();
    if value == "false" || value.is_empty() {
        return Ok(None);
    }
    let cutoff = value.parse::<DateTime<Utc>>().ok().or_else(|| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()?
            .and_hms_opt(0, 0, 0)
            .map(|date| {
                // uv の日付はローカル時刻。UTC 解釈の mise でも弱まらない早い方を使う。
                Local
                    .from_local_datetime(&date)
                    .earliest()
                    .map(|local| local.with_timezone(&Utc).min(date.and_utc()))
                    .unwrap_or_else(|| date.and_utc())
            })
    });
    if let Some(cutoff) = cutoff {
        return Ok(Some((now - cutoff).to_std().unwrap_or_default()));
    }
    if let Some(age) = super::mise_settings::parse_mise_duration(value) {
        return Ok(Some(age));
    }
    if let Some(value) = value.strip_prefix('P') {
        let mut time = false;
        let mut number = String::new();
        let mut seconds = 0u64;
        let mut seen = false;
        for c in value.chars() {
            if c == 'T' && number.is_empty() && !time {
                time = true;
                continue;
            }
            if c.is_ascii_digit() {
                number.push(c);
                continue;
            }
            let unit = match (time, c) {
                (false, 'W') => 604800,
                (false, 'D') => 86400,
                (true, 'H') => 3600,
                (true, 'M') => 60,
                (true, 'S') => 1,
                _ => return Err(format!("unsupported cutoff {value:?}")),
            };
            let amount = number
                .parse::<u64>()
                .map_err(|_| format!("invalid cutoff {value:?}"))?;
            seconds = amount
                .checked_mul(unit)
                .and_then(|amount| seconds.checked_add(amount))
                .ok_or("native age is too large")?;
            number.clear();
            seen = true;
        }
        if seen && number.is_empty() {
            return checked_age(seconds)
                .map(Some)
                .ok_or_else(|| "native age is too large".into());
        }
    }
    Err(format!(
        "unsupported cutoff {value:?}; refusing to override it"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_configuration_above_the_run_root_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(temp.path().join("uv.toml"), "exclude-newer = '30 days'\n").unwrap();
        std::fs::write(
            temp.path().join("mise.local.toml"),
            "[settings]\nminimum_release_age = '14d'\n",
        )
        .unwrap();
        assert!(
            read_scoped_values(Language::Python, &project)
                .unwrap()
                .contains(&"30 days".into())
        );
        assert!(
            read_scoped_values(Language::Mise, &project)
                .unwrap()
                .contains(&"14d".into())
        );
    }

    #[test]
    fn cutoff_formats_and_stricter_native_policies_are_preserved() {
        let now = "2026-10-04T00:00:00Z".parse().unwrap();
        for value in ["14d", "14 days", "P14D", "PT336H", "2026-09-20T00:00:00Z"] {
            assert_eq!(
                parse_cutoff_age(value, now).unwrap(),
                Some(Duration::from_secs(14 * 86400)),
                "{value}"
            );
        }
        assert!(
            parse_cutoff_age("2026-09-20", now).unwrap().unwrap()
                >= Duration::from_secs(14 * 86400)
        );
        assert_eq!(
            parse_cutoff_age("P1DT2H", now).unwrap(),
            Some(Duration::from_secs(26 * 3600))
        );
        assert!(parse_cutoff_age("unknown", now).is_err());
    }

    #[test]
    fn uv_and_mise_settings_use_their_own_tables() {
        let dir = tempfile::tempdir().unwrap();
        let uv = dir.path().join("pyproject.toml");
        std::fs::write(&uv, "[tool.uv]\nexclude-newer = '2026-09-20'\n").unwrap();
        assert_eq!(
            read_age_values(Language::Python, &uv).unwrap(),
            ["2026-09-20"]
        );
        std::fs::write(&uv, "[tool.uv]\nexclude-newer-package = { library = '30 days', other = false }\n[[tool.uv.index]]\nname = 'example'\nurl = 'https://packages.example.com/simple'\nexclude-newer = 'P14D'\n").unwrap();
        assert_eq!(
            read_age_values(Language::Python, &uv).unwrap(),
            ["30 days", "P14D"]
        );
        let mise = dir.path().join("config.toml");
        std::fs::write(&mise, "[settings]\nminimum_release_age = '14d'\n[tools]\nnode = { version = '22', minimum_release_age = '30d' }\n").unwrap();
        assert_eq!(
            read_age_values(Language::Mise, &mise).unwrap(),
            ["14d", "30d"]
        );
        std::fs::write(&mise, "[tools]\npython = [{ version = '3.12', minimum_release_age = '30d' }, { version = '3.13', minimum_release_age = '14d' }]\n").unwrap();
        assert_eq!(
            read_age_values(Language::Mise, &mise).unwrap(),
            ["30d", "14d"]
        );
        std::fs::write(&mise, "[settings]\nminimum_release_age = 14\n").unwrap();
        assert!(read_age_values(Language::Mise, &mise).is_err());
    }
}
