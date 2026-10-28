use super::*;
use crate::domain::{ChangeLevel, OsvBlocked, VersionSpec, VersionSpecKind};
use crate::output::{JsonFormatter, OutputFormatter, TextFormatter, Verbosity};
use chrono::TimeZone;
use clap::Parser;

struct OfflineRegistry;

#[async_trait::async_trait]
impl RegistryAdapter for OfflineRegistry {
    fn language(&self) -> Language {
        Language::Rust
    }
    fn registry_name(&self) -> &'static str {
        "offline"
    }
    async fn fetch_versions(
        &self,
        _: &str,
    ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
        panic!("OSV diagnostics must use versions already fetched")
    }
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap()
}

fn dependency() -> Dependency {
    Dependency::new(
        "library",
        VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0").with_prefix("^"),
        false,
        Language::Rust,
    )
}

fn version(value: &str, days: i64) -> VersionInfo {
    VersionInfo::new(value, now() - chrono::Duration::days(days))
}

fn vulnerable() -> Result<OsvCheck, String> {
    Ok(OsvCheck::Vulnerable(vec!["TEST-ADVISORY".into()]))
}

async fn run(
    dep: &Dependency,
    filter: UpdateFilter,
    versions: Vec<VersionInfo>,
    responses: &[(&str, Result<OsvCheck, String>)],
) -> UpdateResult {
    let dir = tempfile::TempDir::new().unwrap();
    let args = CliArgs::parse_from([
        "depup",
        "--no-osv",
        "--no-age",
        dir.path().to_str().unwrap(),
    ]);
    let orchestrator = Orchestrator::new(args).unwrap();
    let checker = OsvChecker::mock(responses);
    let mut warnings = Vec::new();
    let mut context = OsvJudgeContext {
        checker: &checker,
        ecosystem: "crates.io",
        bar: None,
        warnings: &mut warnings,
    };
    judge_with_osv(
        &orchestrator,
        &OfflineRegistry,
        &UpdateJudge::with_time(filter, now()),
        dep,
        versions,
        &mut context,
    )
    .await
    .unwrap()
}

fn blocked(result: &UpdateResult) -> &OsvBlocked {
    let UpdateResult::Skip {
        reason: SkipReason::KnownVulnerabilities(diagnostic),
        ..
    } = result
    else {
        panic!("expected OSV-blocked result, got {result:?}");
    };
    diagnostic
}

#[tokio::test]
async fn exhausted_osv_candidates_report_current_advisories_and_age_waiting_safe_version() {
    let result = run(
        &dependency(),
        UpdateFilter::new().with_min_age(Duration::from_secs(7 * 86400)),
        vec![
            version("1.0.0", 40),
            version("1.0.1", 20),
            version("1.0.2", 15),
            version("1.0.3", 2),
        ],
        &[
            ("1.0.0", vulnerable()),
            ("1.0.1", vulnerable()),
            ("1.0.2", vulnerable()),
            ("1.0.3", Ok(OsvCheck::Safe)),
        ],
    )
    .await;
    let diagnostics = blocked(&result);
    assert_eq!(
        diagnostics.skipped_versions,
        ["1.0.2 (TEST-ADVISORY)", "1.0.1 (TEST-ADVISORY)"]
    );
    assert!(diagnostics.current_vulnerable);
    assert_eq!(diagnostics.current_advisories, ["TEST-ADVISORY"]);
    let waiting = diagnostics.safe_after_age.as_ref().unwrap();
    assert_eq!(waiting.version, "1.0.3");
    assert_eq!(waiting.eligible_at, now() + chrono::Duration::days(5));
    assert!(diagnostics.current_check_error.is_none());
    assert!(diagnostics.waiting_check_error.is_none());

    let mut manifest = crate::domain::ManifestUpdateResult::new("Cargo.toml", Language::Rust);
    manifest.add_result(result);
    for verbosity in [Verbosity::Normal, Verbosity::Verbose] {
        let mut output = Vec::new();
        TextFormatter::with_color(verbosity, true, false)
            .format_manifest(&manifest, &mut output)
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("no eligible update without known vulnerabilities"));
        assert!(output.contains("library manifest baseline remains at 1.0.0"));
        assert!(output.contains("current version has known vulnerabilities (TEST-ADVISORY)"));
        assert!(output.contains(
            "1.0.3 has no known vulnerabilities; satisfies --age at 2024-06-06T12:00:00+00:00"
        ));
        assert!(!output.contains("1 latest"));
        let mut summary = crate::domain::UpdateSummary::new(true);
        summary.manifests.push(manifest.clone());
        let report = OrchestratorResult {
            summary,
            errors: Vec::new(),
            write_results: Vec::new(),
        };
        let mut json = Vec::new();
        JsonFormatter::new(verbosity)
            .format(&report, &mut json)
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let skip = &json["manifests"][0]["skips"][0];
        assert_eq!(skip["reason"], "known_vulnerabilities");
        assert_eq!(skip["osv"]["current_advisories"][0], "TEST-ADVISORY");
        assert_eq!(skip["osv"]["safe_after_age"]["version"], "1.0.3");
        assert_eq!(json["summary"]["updates"], 0);
        assert_eq!(json["summary"]["skips"], 1);
    }
}

#[tokio::test]
async fn exhausted_osv_without_age_and_unknown_current_are_explicit() {
    let result = run(
        &dependency(),
        UpdateFilter::new(),
        vec![version("1.0.1", 20)],
        &[("1.0.1", vulnerable()), ("1.0.0", Err("offline".into()))],
    )
    .await;
    let diagnostic = blocked(&result);
    assert!(!diagnostic.current_vulnerable);
    assert!(diagnostic.current_version.is_none());
    assert!(
        diagnostic
            .current_check_error
            .as_deref()
            .unwrap()
            .contains("not queried")
    );
    assert!(diagnostic.safe_after_age.is_none());
}

#[tokio::test]
async fn age_waiting_osv_failure_does_not_claim_a_safe_release() {
    let result = run(
        &dependency(),
        UpdateFilter::new().with_min_age(Duration::from_secs(7 * 86400)),
        vec![
            version("1.0.0", 40),
            version("1.0.1", 20),
            version("1.0.2", 1),
        ],
        &[
            ("1.0.1", vulnerable()),
            ("1.0.0", Ok(OsvCheck::Safe)),
            ("1.0.2", Err("offline".into())),
        ],
    )
    .await;
    let diagnostic = blocked(&result);
    assert!(!diagnostic.current_vulnerable);
    assert!(diagnostic.current_advisories.is_empty());
    assert!(diagnostic.safe_after_age.is_none());
    assert_eq!(diagnostic.waiting_check_error.as_deref(), Some("offline"));
}

#[tokio::test]
async fn age_waiting_versions_keep_range_reject_prerelease_and_max_change_filters() {
    let mut dep = dependency();
    dep.version_spec = VersionSpec::new(VersionSpecKind::Range, ">=1.0.0, <1.1.0", "1.0.0");
    dep.version_spec.rejected_versions.push("1.0.3".into());
    let result = run(
        &dep,
        UpdateFilter::new()
            .with_min_age(Duration::from_secs(7 * 86400))
            .with_max_change(ChangeLevel::Patch),
        vec![
            version("1.0.0", 40),
            version("1.0.1", 20),
            version("1.0.2", 2),
            version("1.0.3", 1),
            version("1.0.4-rc.1", 1),
            version("1.1.0", 1),
        ],
        &[
            ("1.0.1", vulnerable()),
            ("1.0.0", Ok(OsvCheck::Safe)),
            ("1.0.2", Ok(OsvCheck::Safe)),
        ],
    )
    .await;
    let diagnostic = blocked(&result);
    assert_eq!(diagnostic.safe_after_age.as_ref().unwrap().version, "1.0.2");
    assert!(diagnostic.waiting_check_error.is_none());
}

#[tokio::test]
async fn current_baseline_lookup_failure_is_not_a_safe_verdict() {
    let result = run(
        &dependency(),
        UpdateFilter::new(),
        vec![version("1.0.0", 40), version("1.0.1", 20)],
        &[("1.0.0", Err("offline".into())), ("1.0.1", vulnerable())],
    )
    .await;
    let diagnostic = blocked(&result);
    assert_eq!(diagnostic.current_version.as_deref(), Some("1.0.0"));
    assert_eq!(diagnostic.current_check_error.as_deref(), Some("offline"));
    assert!(!diagnostic.current_vulnerable);
}

#[tokio::test]
async fn exhausted_candidates_keep_diagnostics_when_judge_would_return_another_skip_reason() {
    for max_change in [None, Some(ChangeLevel::Patch)] {
        let mut filter = UpdateFilter::new().with_min_age(Duration::from_secs(7 * 86400));
        filter.max_change = max_change;
        let mut versions = vec![
            version("1.0.0", 2),
            version("1.0.1", 20),
            version("1.0.2", 1),
        ];
        if max_change.is_some() {
            versions.push(version("2.0.0", 20));
        }
        let result = run(
            &dependency(),
            filter,
            versions,
            &[
                ("1.0.0", vulnerable()),
                ("1.0.1", vulnerable()),
                ("1.0.2", Ok(OsvCheck::Safe)),
            ],
        )
        .await;
        let diagnostic = blocked(&result);
        assert!(diagnostic.current_vulnerable);
        assert_eq!(diagnostic.safe_after_age.as_ref().unwrap().version, "1.0.2");
        assert!(
            super::tauri_sync_protected(&result),
            "Tauri synchronization must not reintroduce an OSV-rejected version"
        );
    }
}

#[tokio::test]
async fn current_baseline_uses_the_matching_published_spelling() {
    let mut dep = dependency();
    dep.language = Language::Go;
    dep.version_spec = VersionSpec::new(VersionSpecKind::Exact, "v1.0.0", "1.0.0");
    let result = run(
        &dep,
        UpdateFilter::new(),
        vec![version("v1.0.0", 40), version("v1.0.1", 20)],
        &[("v1.0.0", vulnerable()), ("v1.0.1", vulnerable())],
    )
    .await;
    let diagnostic = blocked(&result);
    assert_eq!(diagnostic.current_version.as_deref(), Some("v1.0.0"));
    assert!(diagnostic.current_vulnerable);
}

#[tokio::test]
async fn safe_fallback_and_ordinary_latest_keep_existing_results() {
    let result = run(
        &dependency(),
        UpdateFilter::new(),
        vec![version("1.0.1", 20), version("1.0.2", 10)],
        &[("1.0.2", vulnerable()), ("1.0.1", Ok(OsvCheck::Safe))],
    )
    .await;
    assert!(
        matches!(result, UpdateResult::Update { new_version, osv_checked: true, osv_skipped, .. } if new_version == "1.0.1" && osv_skipped.len() == 1)
    );
    let result = run(
        &dependency(),
        UpdateFilter::new(),
        vec![version("1.0.0", 40)],
        &[],
    )
    .await;
    assert!(matches!(
        result,
        UpdateResult::Skip {
            reason: SkipReason::AlreadyLatest,
            ..
        }
    ));
}
