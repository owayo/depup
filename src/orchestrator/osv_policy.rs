//! OSV の例外を、設定と確認できた現在版に結び付ける。
use super::*;
use crate::osv::{Assessment, RiskTradeoff};

impl Orchestrator {
    pub(crate) fn osv_active(&self) -> bool {
        self.osv_checker.is_some()
    }

    pub(crate) fn osv_risk_tradeoff_enabled(&self) -> bool {
        !self.args.no_osv_risk_tradeoff
            && self
                .global_config
                .as_ref()
                .and_then(|config| config.osv_risk_tradeoff)
                .unwrap_or(true)
    }

    pub(crate) async fn assess_osv_change(
        &self,
        language: Language,
        name: &str,
        before: &[String],
        after: &[String],
    ) -> Result<Option<RiskTradeoff>, String> {
        let (Some(checker), Some(ecosystem)) = (&self.osv_checker, language.osv_ecosystem()) else {
            return Ok(None);
        };
        match checker
            .assess_versions(
                ecosystem,
                name,
                before,
                after,
                self.osv_risk_tradeoff_enabled(),
            )
            .await?
        {
            Assessment::Safe => Ok(None),
            Assessment::Tradeoff(report) => Ok(Some(report)),
            Assessment::Rejected {
                advisories,
                baseline_error,
            } => Err(format!(
                "{name}@{} is vulnerable ({});{}",
                after.join(", "),
                advisories
                    .iter()
                    .map(|record| record.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                baseline_error.map_or_else(
                    || " risk-reduction conditions not met".into(),
                    |error| format!(" baseline OSV check failed: {error}")
                ),
            )),
        }
    }

    /// lock に複数の該当版があるときは推測せず、例外を使わない。
    pub(super) async fn osv_baselines(
        &self,
        manifest: &ManifestInfo,
        dependencies: &[Dependency],
    ) -> BTreeMap<String, String> {
        let mut result = BTreeMap::new();
        if !self.osv_active() || !self.osv_risk_tradeoff_enabled() {
            return result;
        }
        match manifest.language {
            Language::Rust => {
                let Some(path) = manifest
                    .path
                    .parent()
                    .and_then(|dir| find_cargo_lock_upward(dir, &self.rust_lock_boundary()))
                else {
                    return result;
                };
                let Ok((_, entries)) = crate::cargo_rollback::audit::read_audit_lock(&path) else {
                    return result;
                };
                for dep in dependencies {
                    let Ok(requirement) = semver::VersionReq::parse(&dep.version_spec.raw) else {
                        continue;
                    };
                    let matching: Vec<_> = entries
                        .get(&dep.name)
                        .into_iter()
                        .flatten()
                        .filter(|version| {
                            semver::Version::parse(version)
                                .is_ok_and(|version| requirement.matches(&version))
                        })
                        .collect();
                    if let [version] = matching.as_slice() {
                        result.insert(dep.manifest_name().to_string(), (*version).clone());
                    }
                }
            }
            Language::Node => {
                let entries = crate::node_lock::osv_direct_baselines(&manifest.path).await;
                for dep in dependencies {
                    if let Some((name, version)) = entries.get(dep.manifest_name())
                        && name == &dep.name
                    {
                        result.insert(dep.manifest_name().to_string(), version.clone());
                    }
                }
            }
            _ => {}
        }
        // lock を読めなかった Rust / Node は、manifest の下限や古いピンへ代替しない。
        // Gradle の素の版や go.mod の版は、競合解決で上がる下限であり固定版ではない。
        if matches!(
            manifest.language,
            Language::Python | Language::Ruby | Language::Php
        ) {
            for dep in dependencies {
                if dep.version_spec.kind == crate::domain::VersionSpecKind::Exact
                    && !dep.version().is_empty()
                {
                    result.insert(
                        dep.manifest_name().to_string(),
                        dep.version_spec.display_version(),
                    );
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[tokio::test]
    async fn java_and_go_declarations_do_not_provide_fixed_baselines() {
        use crate::domain::{VersionSpec, VersionSpecKind};
        let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup", "--osv"])).unwrap();
        for language in [Language::Java, Language::Go] {
            let dep = Dependency::new(
                "example",
                VersionSpec::new(VersionSpecKind::Exact, "1.0.0", "1.0.0"),
                false,
                language,
            );
            let manifest = ManifestInfo::new(language.manifest_filename(), language);
            assert!(
                orchestrator
                    .osv_baselines(&manifest, &[dep])
                    .await
                    .is_empty()
            );
        }
    }

    #[test]
    fn osv_tradeoff_is_enabled_without_a_config_key_and_can_be_disabled() {
        let mut args = CliArgs::parse_from(["depup", "--no-osv"]);
        let orchestrator = Orchestrator::new(args.clone())
            .unwrap()
            .with_global_config(Some(toml::from_str("age = '1w'").unwrap()));
        assert!(orchestrator.osv_risk_tradeoff_enabled());
        let config: GlobalConfig = toml::from_str("osv_risk_tradeoff = false").unwrap();
        assert!(
            !Orchestrator::new(args.clone())
                .unwrap()
                .with_global_config(Some(config))
                .osv_risk_tradeoff_enabled()
        );
        args.no_osv_risk_tradeoff = true;
        assert!(!Orchestrator::new(args).unwrap().osv_risk_tradeoff_enabled());
        let config: GlobalConfig =
            toml::from_str(crate::global_config::DEFAULT_CONFIG_CONTENT).unwrap();
        assert_eq!(config.osv_risk_tradeoff, Some(true));
    }

    #[tokio::test]
    async fn cargo_osv_baseline_uses_locked_version_and_refuses_ambiguous_or_private_versions() {
        use crate::domain::{VersionSpec, VersionSpecKind};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Cargo.toml");
        std::fs::write(
            &path,
            "[package]\nname='app'\nversion='0.1.0'\n[dependencies]\nexample='1'\n",
        )
        .unwrap();
        let orchestrator = Orchestrator::new(CliArgs::parse_from([
            "depup",
            "--osv",
            temp.path().to_str().unwrap(),
        ]))
        .unwrap();
        let dep = Dependency::new(
            "example",
            VersionSpec::new(VersionSpecKind::Caret, "1", "1.0.0"),
            false,
            Language::Rust,
        );
        let manifest = ManifestInfo::new(&path, Language::Rust);
        assert!(
            orchestrator
                .osv_baselines(&manifest, std::slice::from_ref(&dep))
                .await
                .is_empty()
        );
        let entry = |version: &str, source: &str| {
            format!("[[package]]\nname='example'\nversion='{version}'\nsource='{source}'\n")
        };
        let public = "registry+https://github.com/rust-lang/crates.io-index";
        std::fs::write(
            temp.path().join("Cargo.lock"),
            format!("version=4\n{}", entry("1.4.0", public)),
        )
        .unwrap();
        assert_eq!(
            orchestrator
                .osv_baselines(&manifest, std::slice::from_ref(&dep))
                .await["example"],
            "1.4.0"
        );
        for content in [
            format!(
                "version=4\n{}{}",
                entry("1.4.0", public),
                entry("1.5.0", public)
            ),
            format!(
                "version=4\n{}",
                entry("1.4.0", "registry+https://packages.example.com/index")
            ),
        ] {
            std::fs::write(temp.path().join("Cargo.lock"), content).unwrap();
            assert!(
                orchestrator
                    .osv_baselines(&manifest, std::slice::from_ref(&dep))
                    .await
                    .is_empty()
            );
        }
    }
}
