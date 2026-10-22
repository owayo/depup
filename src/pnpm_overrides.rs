//! pnpm の実効 override によって、直接依存の宣言に対する誤解を招く更新を防ぐ。
use crate::domain::Dependency;
use crate::parser::{NodeVersionParser, VersionParser};
use std::path::Path;

#[derive(Clone, Default)]
pub(crate) struct PnpmOverrides(Vec<(String, String)>);

impl PnpmOverrides {
    #[cfg(test)]
    pub(crate) fn for_selectors(selectors: &[&str]) -> Self {
        Self(
            selectors
                .iter()
                .map(|selector| ((*selector).into(), "effective pnpm overrides".into()))
                .collect(),
        )
    }

    pub(crate) fn reason(&self, dependency: &Dependency) -> Option<String> {
        self.0.iter().find_map(|(selector, source)| {
            // スコープ付きパッケージの最初の @ は、版の区切りではなく名前の一部。
            let suffix = selector
                .char_indices()
                .skip(1)
                .find(|(_, ch)| *ch == '@')
                .map(|(offset, _)| offset);
            let name = suffix.map_or(selector.as_str(), |offset| &selector[..offset]);
            if name.contains('>') {
                return None;
            }
            if let Some(offset) = suffix {
                let version = &selector[offset + 1..];
                // comparator set は空白でトークンを分ける。版トークンの後で空白なしに続く > は、
                // 子の名前が数字の場合も含め、pnpm の親子の区切りになる。
                let parent_separator = version.char_indices().any(|(index, ch)| {
                    ch == '>'
                        && index > 0
                        && !version[..index].ends_with(char::is_whitespace)
                        && &version[..index] != "~"
                });
                let valid_constraint = NodeVersionParser.parse(version).is_some();
                if version.contains('>') && (parent_separator || !valid_constraint) {
                    return None;
                }
            }
            (name == dependency.name || name == dependency.manifest_name()).then(|| {
                let control = if suffix.is_some() {
                    "potentially controlled"
                } else {
                    "controlled"
                };
                format!("{control} by {source} for {}", dependency.manifest_name())
            })
        })
    }

    pub(crate) async fn read(directory: &Path) -> Result<Self, String> {
        if !matches!(
            crate::package_manager::SystemPackageManager::new().resolve_scoped_package_manager(
                crate::domain::Language::Node,
                directory,
                &crate::package_manager::InstallScope::Refresh
            ),
            Some((_, "pnpm"))
        ) {
            return Ok(Self::default());
        }
        let program = which::which_in("pnpm", std::env::var_os("PATH"), directory)
            .unwrap_or_else(|_| "pnpm".into());
        Self::read_with(directory, &program).await
    }

    async fn read_with(directory: &Path, program: &Path) -> Result<Self, String> {
        let output = query(
            directory,
            program,
            &["config", "get", "overrides", "--json"],
        )
        .await?;
        let mut overrides = Self::default();
        if !output
            .bytes()
            .all(|byte| byte.is_ascii_whitespace() || byte == 0x0b)
        {
            let value: serde_json::Value = serde_json::from_str(&output)
                .map_err(|_| "pnpm overrides configuration is not valid JSON".to_string())?;
            overrides.add(&value, "effective pnpm overrides")?;
        }
        let package = match std::fs::read(directory.join("package.json")) {
            Ok(package) => package,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(overrides),
            Err(_) => return Err("could not read package.json for pnpm overrides".into()),
        };
        let package: serde_json::Value = serde_json::from_slice(&package)
            .map_err(|_| "invalid package.json for pnpm overrides".to_string())?;
        if let Some(legacy) = package.get("pnpm").and_then(|pnpm| pnpm.get("overrides")) {
            let version = query(directory, program, &["--version"]).await?;
            let version = semver::Version::parse(version.trim())
                .map_err(|_| "could not determine pnpm version for legacy overrides".to_string())?;
            if version.major < 11 {
                overrides.add(legacy, "package.json pnpm.overrides")?;
            }
        }
        Ok(overrides)
    }

    fn add(&mut self, value: &serde_json::Value, source: &str) -> Result<(), String> {
        if value.is_null() {
            return Ok(());
        }
        let values = value
            .as_object()
            .ok_or_else(|| "pnpm overrides configuration must be an object".to_string())?;
        if values.values().any(|value| !value.is_string()) {
            return Err("pnpm overrides configuration contains an unsupported value".into());
        }
        self.0.extend(
            values
                .keys()
                .map(|selector| (selector.clone(), source.into())),
        );
        Ok(())
    }
}

async fn query(directory: &Path, program: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new(program);
    command.current_dir(directory).args(args).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "pnpm overrides configuration query timed out".to_string())?
        .map_err(|_| "pnpm overrides configuration query could not be started".to_string())?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err("pnpm overrides configuration could not be verified".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "pnpm overrides configuration is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Language, VersionSpec, VersionSpecKind};

    fn dependency(name: &str) -> Dependency {
        Dependency::new(
            name,
            VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0"),
            false,
            Language::Node,
        )
    }

    #[test]
    fn pnpm_override_selectors_protect_direct_alias_and_scoped_dependencies() {
        for selector in [
            "example",
            "example@^1",
            "example@>v1.0.0",
            "example@>=v1.0.0",
            "@scope/example",
            "@scope/example@>=1",
        ] {
            let name = if selector.starts_with('@') {
                "@scope/example"
            } else {
                "example"
            };
            let overrides = PnpmOverrides(vec![(
                selector.into(),
                "pnpm-workspace.yaml overrides".into(),
            )]);
            assert!(overrides.reason(&dependency(name)).is_some(), "{selector}");
        }
        let alias = dependency("actual").with_manifest_name("alias");
        for selector in [
            "parent@1>v2",
            "parent@1>2fa",
            "parent@>=1>example",
            "parent>example",
        ] {
            assert!(
                PnpmOverrides::for_selectors(&[selector])
                    .reason(&dependency("parent"))
                    .is_none(),
                "{selector}"
            );
        }
        for selector in ["actual", "alias", "alias@^1"] {
            assert!(
                PnpmOverrides(vec![(selector.into(), "overrides".into())])
                    .reason(&alias)
                    .is_some()
            );
        }
        assert!(
            PnpmOverrides(vec![("parent>example".into(), "overrides".into())])
                .reason(&dependency("example"))
                .is_none()
        );
    }

    #[cfg(unix)]
    fn fake(directory: &Path, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let program = directory.join("fake-pnpm");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pnpm_legacy_overrides_are_inactive_since_version_11() {
        for major in [10, 11, 12] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(
                directory.path().join("package.json"),
                r#"{"pnpm":{"overrides":{"example":"1.0.0"}}}"#,
            )
            .unwrap();
            let program = fake(
                directory.path(),
                &format!(
                    "if [ \"$1\" = --version ]; then printf '{major}.0.0'; else printf null; fi"
                ),
            );
            let overrides = PnpmOverrides::read_with(directory.path(), &program)
                .await
                .unwrap();
            assert_eq!(
                overrides.reason(&dependency("example")).is_some(),
                major < 11
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pnpm_workspace_overrides_do_not_require_a_root_package_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let program = fake(directory.path(), "printf '{\"example\":\"1.0.0\"}'");
        let overrides = PnpmOverrides::read_with(directory.path(), &program)
            .await
            .unwrap();
        assert!(overrides.reason(&dependency("example")).is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pnpm_effective_overrides_accept_empty_and_fail_closed_on_invalid_config() {
        for output in ["", " \\t\\r\\n", "null", "{}"] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("package.json"), "{}").unwrap();
            let program = fake(directory.path(), &format!("printf '{output}'"));
            assert!(
                PnpmOverrides::read_with(directory.path(), &program)
                    .await
                    .is_ok()
            );
        }
        for script in [
            "exit 7",
            "printf invalid",
            "printf '[]'",
            "printf null; printf secret >&2",
        ] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("package.json"), "{}").unwrap();
            let program = fake(directory.path(), script);
            let error = PnpmOverrides::read_with(directory.path(), &program)
                .await
                .err()
                .expect("unverified overrides must fail");
            assert!(!error.contains("secret"));
        }
    }
}
