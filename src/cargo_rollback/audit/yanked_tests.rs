use super::*;
use crate::error::RegistryError;
use crate::test_support::fake_crates_io::FakeCratesIo;
use crate::test_support::local_registry::{LocalRegistry, TestProject};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::atomic::{AtomicUsize, Ordering};

fn old() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

fn cutoff() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap()
}

fn young() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap()
}

#[test]
fn yanked_fallback_requires_an_exact_mature_stable_preinstall_version() {
    let yanked = vec![
        VersionInfo::new("0.7.4", old()),
        VersionInfo::new("0.7.5", old()),
        VersionInfo::new("0.7.6", young()),
        VersionInfo::undated("0.7.7"),
        VersionInfo::new("0.7.8-rc.1", old()),
        VersionInfo::new("0.6.9", old()),
        VersionInfo::new("0.8.0", old()),
    ];
    assert_eq!(
        yanked_baseline_target(&yanked, &["0.7.5".into()], "0.7.10", cutoff()),
        Some("0.7.5".into())
    );
    for before in [
        vec![],
        vec!["0.7.3".into()],
        vec!["0.7.5+different".into()],
        vec!["0.7.6".into()],
        vec!["0.7.7".into()],
        vec!["0.7.8-rc.1".into()],
        vec!["0.6.9".into()],
        vec!["0.8.0".into()],
    ] {
        assert_eq!(
            yanked_baseline_target(&yanked, &before, "0.7.10", cutoff()),
            None,
            "{before:?}"
        );
    }
    assert_eq!(
        yanked_baseline_target(&yanked, &["0.7.5".into()], "0.7.5", cutoff()),
        None
    );
    assert_eq!(
        yanked_baseline_target(&yanked, &["0.7.5".into()], "0.7.4", cutoff()),
        None
    );
}

enum YankResponse {
    Confirmed,
    Failed,
    Delayed,
    FinalFailed,
}

struct CountingAdapter {
    inner: FakeCratesIo,
    calls: AtomicUsize,
    response: YankResponse,
}

#[async_trait]
impl RegistryAdapter for CountingAdapter {
    fn language(&self) -> Language {
        Language::Rust
    }
    fn registry_name(&self) -> &'static str {
        "fixed-yanks"
    }
    async fn fetch_versions(&self, name: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        self.inner.fetch_versions(name).await
    }
    async fn fetch_locked_versions(
        &self,
        name: &str,
        locked: &[String],
        cutoff: Option<chrono::DateTime<Utc>>,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        if matches!(self.response, YankResponse::FinalFailed) {
            return Err(RegistryError::package_not_found(name, self.registry_name()));
        }
        self.inner.fetch_locked_versions(name, locked, cutoff).await
    }
    async fn fetch_yanked_versions(&self, name: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.response {
            YankResponse::Failed => {
                return Err(RegistryError::package_not_found(name, self.registry_name()));
            }
            YankResponse::Delayed => tokio::time::sleep(Duration::from_secs(2)).await,
            YankResponse::Confirmed | YankResponse::FinalFailed => {}
        }
        self.inner.fetch_yanked_versions(name).await
    }
}

fn fixture(
    normal: bool,
) -> (
    LocalRegistry,
    TestProject,
    RegistryLockEntries,
    FakeCratesIo,
    CargoCommand,
) {
    let registry = LocalRegistry::new();
    registry.publish("sample", "1.0.0", &[]);
    let project = TestProject::new(
        &registry,
        "[package]\nname = 'app'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nsample = '1'\n",
    );
    project.cargo_ok(&["generate-lockfile"]);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    registry
        .yank("sample", "1.0.0")
        .publish("sample", "1.2.0", &[]);
    if normal {
        registry.publish("sample", "1.1.0", &[]);
    }
    project.cargo_ok(&["update"]);
    let mut adapter = FakeCratesIo::new()
        .release("sample", "1.0.0", old())
        .yank("sample", "1.0.0")
        .release("sample", "1.2.0", young());
    if normal {
        adapter = adapter.release("sample", "1.1.0", old());
    }
    let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
    for (key, value) in project.cargo_envs() {
        cargo = cargo.env(key, value);
    }
    (registry, project, baseline, adapter, cargo)
}

#[tokio::test]
async fn normal_rollback_candidates_avoid_yanked_metadata_requests() {
    let (_registry, project, baseline, inner, cargo) = fixture(true);
    let adapter = CountingAdapter {
        inner,
        calls: AtomicUsize::new(0),
        response: YankResponse::Confirmed,
    };
    let input = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &Default::default(),
        baseline: &baseline,
        preferred: &Default::default(),
        adapter: &adapter,
        cargo: &cargo,
        budget: Duration::from_secs(30),
    };
    let versions = VersionFetcher::new(4, 1);
    let result = LockAgeAuditor::new(&input, &versions, false)
        .run(None)
        .await;
    assert!(!result.has_unresolved(), "{result:?}");
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(project.locked_versions("sample"), ["1.1.0"]);
    assert_eq!(result.adjustments[0].status, LockAgeStatus::Downgraded);
}

#[tokio::test]
async fn failed_yanked_lookup_is_cached_and_never_restores_the_baseline() {
    let (_registry, project, baseline, inner, cargo) = fixture(false);
    let adapter = CountingAdapter {
        inner,
        calls: AtomicUsize::new(0),
        response: YankResponse::Failed,
    };
    let input = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &Default::default(),
        baseline: &baseline,
        preferred: &Default::default(),
        adapter: &adapter,
        cargo: &cargo,
        budget: Duration::from_secs(30),
    };
    let versions = VersionFetcher::new(4, 1);
    let auditor = LockAgeAuditor::new(&input, &versions, false);
    let mut state = AuditState::new(input.budget);
    for pass in 0..2 {
        auditor
            .run_pass(&mut state, pass, false, None)
            .await
            .unwrap();
    }
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(project.locked_versions("sample"), ["1.2.0"]);
    assert_eq!(
        state.log.entries[&("sample".into(), "1.2.0".into())].status,
        LockAgeStatus::NoOlderCandidate
    );
}

#[tokio::test]
async fn yanked_lookup_timeout_stops_without_changing_the_lock() {
    let (_registry, project, baseline, inner, cargo) = fixture(false);
    let adapter = CountingAdapter {
        inner,
        calls: AtomicUsize::new(0),
        response: YankResponse::Delayed,
    };
    let input = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &Default::default(),
        baseline: &baseline,
        preferred: &Default::default(),
        adapter: &adapter,
        cargo: &cargo,
        budget: Duration::from_millis(500),
    };
    let versions = VersionFetcher::new(4, 1);
    let auditor = LockAgeAuditor::new(&input, &versions, false);
    let mut state = AuditState::new(input.budget);
    let before = project.read("Cargo.lock");
    let pass = auditor.run_pass(&mut state, 0, false, None).await.unwrap();
    assert!(pass.budget_exhausted);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(project.read("Cargo.lock"), before);
}

#[tokio::test]
async fn yanked_restoration_still_requires_final_release_date_verification() {
    let (_registry, project, baseline, inner, cargo) = fixture(false);
    let adapter = CountingAdapter {
        inner,
        calls: AtomicUsize::new(0),
        response: YankResponse::FinalFailed,
    };
    let input = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &Default::default(),
        baseline: &baseline,
        preferred: &Default::default(),
        adapter: &adapter,
        cargo: &cargo,
        budget: Duration::from_secs(30),
    };
    let versions = VersionFetcher::new(4, 1);
    let result = LockAgeAuditor::new(&input, &versions, false)
        .run(None)
        .await;
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(project.locked_versions("sample"), ["1.0.0"]);
    assert!(result.has_unresolved());
    assert_eq!(
        result.remaining.get(&("sample".into(), "1.0.0".into())),
        Some(&RemainingAge::Unverified)
    );
    assert_eq!(
        result.adjustments[0].status,
        LockAgeStatus::ReleaseDateUnavailable
    );
}
