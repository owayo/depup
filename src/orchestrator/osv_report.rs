//! age の差し戻し先のうち、復元処理の後も Cargo.lock に残る版の OSV 診断。

use super::{Orchestrator, OrchestratorError, OrchestratorResult};
use crate::cargo_rollback::audit::{LockAgeAuditResult, LockAgeStatus, read_audit_lock};
use crate::osv::{OsvCheck, OsvChecker};
use crate::progress::Progress;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 監査中の目標ではなく、実際に差し戻した先だけを記録する。
#[derive(Default)]
pub(crate) struct RollbackOsvChecks {
    targets: BTreeMap<PathBuf, BTreeSet<(String, String)>>,
}

impl RollbackOsvChecks {
    pub(crate) fn record(&mut self, directory: &Path, audit: &LockAgeAuditResult) {
        for adjustment in &audit.adjustments {
            if matches!(
                adjustment.status,
                LockAgeStatus::Downgraded | LockAgeStatus::Restored
            ) && let Some(to) = &adjustment.to
            {
                self.targets
                    .entry(directory.join("Cargo.lock"))
                    .or_default()
                    .insert((adjustment.name.clone(), to.clone()));
            }
        }
    }

    /// 表示だけを追加する。OSV の警告は install の採否や終了コードを変えない。
    pub(crate) async fn warn(
        &self,
        orchestrator: &Orchestrator,
        result: &mut OrchestratorResult,
        quiet: bool,
    ) {
        let Some(checker) = &orchestrator.osv_checker else {
            return;
        };
        let mut progress = Progress::new(!quiet);
        let warnings = self.warnings(checker, &mut progress).await;
        progress.finish_and_clear();
        for (package, message) in warnings {
            eprintln!("  ⚠ OSV: {message}");
            result
                .errors
                .push(OrchestratorError::OsvWarning { package, message });
        }
    }

    async fn warnings(
        &self,
        checker: &OsvChecker,
        progress: &mut Progress,
    ) -> Vec<(String, String)> {
        // 公開日監査の予算とは独立。大きな差し戻しでも診断の待ち時間を制限する。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut warnings = Vec::new();
        for (path, targets) in &self.targets {
            let (before, entries) = match read_audit_lock(path) {
                Ok(lock) => lock,
                Err(error) => {
                    warnings.push((
                        "Cargo.lock".into(),
                        format!(
                            "{}: rollback destinations not checked: {error}",
                            path.display()
                        ),
                    ));
                    continue;
                }
            };
            let mut findings = Vec::new();
            for (name, version) in targets {
                // transaction の復元で消えた版、私設 registry の同名版は照会しない。
                if !entries
                    .get(name)
                    .is_some_and(|versions| versions.contains(version))
                {
                    continue;
                }
                progress.spinner(&format!(
                    "Checking rollback destination with OSV: {name} {version}"
                ));
                let ecosystem = crate::domain::Language::Rust
                    .osv_ecosystem()
                    .expect("Rust supports OSV");
                let checked = if tokio::time::Instant::now() >= deadline {
                    Err(())
                } else {
                    tokio::time::timeout_at(deadline, checker.check(ecosystem, name, version))
                        .await
                        .map_err(|_| ())
                };
                let detail = match checked {
                    Ok(Ok(OsvCheck::Safe)) => continue,
                    Ok(Ok(OsvCheck::Vulnerable(ids))) => format!(
                        "has known vulnerabilities ({})",
                        if ids.is_empty() {
                            "no advisory IDs".into()
                        } else {
                            ids.join(", ")
                        }
                    ),
                    Ok(Err(error)) => format!("could not be checked: {error}"),
                    Err(_) => {
                        "could not be checked: OSV rollback diagnostic time limit reached".into()
                    }
                };
                findings.push((
                    name.clone(),
                    format!(
                        "{}: {name}@{version}, retained after --age rollback, {detail}",
                        path.display()
                    ),
                ));
            }
            // 問い合わせ中に lock が変わった場合、古い版を「残っている」と報告しない。
            if std::fs::read_to_string(path).is_ok_and(|after| after == before) {
                warnings.extend(findings);
            } else {
                warnings.push((
                    "Cargo.lock".into(),
                    format!(
                        "{} changed during OSV checks; final rollback destinations not verified",
                        path.display()
                    ),
                ));
            }
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_rollback::audit::LockAgeAdjustment;

    fn audit() -> LockAgeAuditResult {
        LockAgeAuditResult {
            adjustments: vec![LockAgeAdjustment {
                name: "sample".into(),
                from: "1.0.2".into(),
                to: Some("1.0.1".into()),
                target: Some("1.0.0".into()),
                status: LockAgeStatus::Downgraded,
            }],
            ..Default::default()
        }
    }

    fn lock(directory: &Path, version: &str, source: &str) {
        std::fs::write(directory.join("Cargo.lock"), format!("version = 4\n[[package]]\nname = \"sample\"\nversion = \"{version}\"\nsource = \"{source}\"\n")).unwrap();
    }

    #[tokio::test]
    async fn rollback_osv_checks_actual_destination_in_final_lock() {
        use crate::output::{JsonFormatter, OutputFormatter, Verbosity};
        use clap::Parser;
        let temp = tempfile::tempdir().unwrap();
        lock(
            temp.path(),
            "1.0.1",
            "registry+https://github.com/rust-lang/crates.io-index",
        );
        let before = std::fs::read(temp.path().join("Cargo.lock")).unwrap();
        let mut checks = RollbackOsvChecks::default();
        checks.record(temp.path(), &audit());
        checks.record(temp.path(), &audit());
        let checker = OsvChecker::mock(&[(
            "1.0.1",
            Ok(OsvCheck::Vulnerable(vec!["TEST-ADVISORY".into()])),
        )]);
        let warnings = checks.warnings(&checker, &mut Progress::new(false)).await;
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].1.contains("sample@1.0.1"));
        assert!(warnings[0].1.contains("TEST-ADVISORY"));
        let mut orchestrator =
            Orchestrator::new(crate::cli::CliArgs::parse_from(["depup", "--no-osv"])).unwrap();
        orchestrator.osv_checker = Some(checker);
        let mut result = OrchestratorResult {
            summary: crate::domain::UpdateSummary::new(false),
            write_results: Vec::new(),
            errors: Vec::new(),
        };
        checks.warn(&orchestrator, &mut result, true).await;
        assert!(matches!(
            result.errors.as_slice(),
            [OrchestratorError::OsvWarning { .. }]
        ));
        let mut json = Vec::new();
        JsonFormatter::new(Verbosity::Normal)
            .format(&result, &mut json)
            .unwrap();
        let output: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert!(
            output["errors"][0]
                .as_str()
                .unwrap()
                .contains("TEST-ADVISORY")
        );
        assert_eq!(
            std::fs::read(temp.path().join("Cargo.lock")).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn rollback_osv_ignores_removed_destinations_and_private_registries() {
        let temp = tempfile::tempdir().unwrap();
        let mut checks = RollbackOsvChecks::default();
        checks.record(temp.path(), &audit());
        let checker = OsvChecker::mock(&[]);
        for (version, source) in [
            (
                "1.0.0",
                "registry+https://github.com/rust-lang/crates.io-index",
            ),
            ("1.0.1", "registry+https://packages.example.com/index"),
        ] {
            lock(temp.path(), version, source);
            assert!(
                checks
                    .warnings(&checker, &mut Progress::new(false))
                    .await
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn rollback_osv_reports_query_failures_without_changing_lock() {
        let temp = tempfile::tempdir().unwrap();
        lock(
            temp.path(),
            "1.0.1",
            "registry+https://github.com/rust-lang/crates.io-index",
        );
        let mut checks = RollbackOsvChecks::default();
        checks.record(temp.path(), &audit());
        let checker = OsvChecker::mock(&[("1.0.1", Err("offline".into()))]);
        let warnings = checks.warnings(&checker, &mut Progress::new(false)).await;
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].1.contains("could not be checked: offline"));
    }

    #[tokio::test]
    async fn rollback_osv_disabled_does_not_read_lock_or_warn() {
        use clap::Parser;
        let temp = tempfile::tempdir().unwrap();
        // lock が無くても、無効時は未確認という警告すら追加しない。
        let mut checks = RollbackOsvChecks::default();
        checks.record(temp.path(), &audit());
        let orchestrator =
            Orchestrator::new(crate::cli::CliArgs::parse_from(["depup", "--no-osv"])).unwrap();
        let mut result = OrchestratorResult {
            summary: crate::domain::UpdateSummary::new(false),
            write_results: Vec::new(),
            errors: Vec::new(),
        };
        checks.warn(&orchestrator, &mut result, true).await;
        assert!(result.errors.is_empty());
    }

    #[test]
    fn rollback_osv_ignores_reverted_removed_and_unsuccessful_attempts() {
        for status in [
            LockAgeStatus::Reverted("restored checkpoint".into()),
            LockAgeStatus::Removed,
            LockAgeStatus::NoOlderCandidate,
            LockAgeStatus::UpdateCommandFailed("resolver conflict".into()),
        ] {
            let mut audit = audit();
            audit.adjustments[0].status = status;
            let mut checks = RollbackOsvChecks::default();
            checks.record(Path::new("project"), &audit);
            assert!(checks.targets.is_empty());
        }
    }
}
