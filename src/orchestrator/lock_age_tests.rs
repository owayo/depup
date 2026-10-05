use super::*;
use crate::manifest::parse_registry_entries;
use crate::test_support::fake_crates_io::FakeCratesIo;
use crate::test_support::local_registry::{LocalRegistry, TestProject};
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;

#[tokio::test]
async fn unchanged_lock_detects_a_previously_incomplete_rollback() {
    let registry = LocalRegistry::new();
    for version in ["1.0.0", "1.0.1"] {
        registry.publish("library", version, &[]);
    }
    let project = TestProject::new(&registry, &manifest("library = '1.0.0'\n"));
    project.cargo_ok(&["generate-lockfile", "--offline"]);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    assert_eq!(project.locked_versions("library"), ["1.0.1"]);
    let dates = FakeCratesIo::new()
        .release("library", "1.0.0", old())
        .release("library", "1.0.1", fresh());
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let cargo = cargo_for(&project);
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: project.path(),
                cutoff: cutoff(),
                exemptions: &Default::default(),
                baseline: &baseline,
                preferred: &PreferredVersions::new(),
                adapter: &dates,
                cargo: &cargo,
                budget: LOCK_AGE_AUDIT_BUDGET,
            },
            None,
        )
        .await;
    assert_eq!(project.locked_versions("library"), ["1.0.1"]);
    assert!(result.has_unresolved(), "{result:?}");
    let failure = adjustment(&result, "library");
    assert_eq!(failure.to, None);
    assert_eq!(failure.target, None);
    assert!(
        matches!(&failure.status, LockAgeStatus::NotAttempted(reason) if reason.contains("rollback minimum"))
    );
    assert!(lock_is_accepted(&project));
}

#[tokio::test]
async fn a_release_absent_from_the_check_cache_is_refetched_and_rolled_back() {
    let registry = LocalRegistry::new();
    for version in ["1.0.0", "1.0.1"] {
        registry.publish("library", version, &[]);
    }
    let project = TestProject::new(&registry, &manifest("library = '=1.0.0'\n"));
    let baseline = install(&project, &manifest("library = '1.0.0'\n"));
    let dates = FakeCratesIo::new()
        .release("library", "1.0.0", old())
        .release("library", "1.0.1", fresh());
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    orchestrator.versions.cache.lock().await.insert(
        (Language::Rust, "library".into()),
        vec![VersionInfo::new("1.0.0", old())],
    );
    let cargo = cargo_for(&project);
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: project.path(),
                cutoff: cutoff(),
                exemptions: &Default::default(),
                baseline: &baseline,
                preferred: &PreferredVersions::new(),
                adapter: &dates,
                cargo: &cargo,
                budget: LOCK_AGE_AUDIT_BUDGET,
            },
            None,
        )
        .await;
    assert_eq!(project.locked_versions("library"), ["1.0.0"]);
    assert_eq!(dates.fetch_count(), 1);
    assert!(!result.has_unresolved(), "{result:?}");
    assert!(lock_is_accepted(&project));
}

#[tokio::test]
async fn missing_locked_version_metadata_is_unverified() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.lock"),
        r#"
version = 4
[[package]]
name = "library"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )
    .unwrap();
    let dates = FakeCratesIo::new().release("library", "1.0.0", old());
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: dir.path(),
                cutoff: cutoff(),
                exemptions: &crate::update::AgeExemptions::default(),
                baseline: &RegistryLockEntries::new(),
                preferred: &PreferredVersions::new(),
                adapter: &dates,
                cargo: &CargoCommand::new(),
                budget: LOCK_AGE_AUDIT_BUDGET,
            },
            None,
        )
        .await;
    assert_eq!(
        adjustment(&result, "library").status,
        LockAgeStatus::ReleaseDateUnavailable
    );
    assert!(result.has_unresolved());
    assert_eq!(dates.fetch_count(), 3);
}

#[tokio::test]
async fn unreadable_or_invalid_lock_cannot_pass_the_audit() {
    let dir = tempfile::tempdir().unwrap();
    let dates = FakeCratesIo::new();
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    for content in [
        None,
        Some("broken {{{"),
        Some("[[package]]\nname = 'library'\n"),
    ] {
        if let Some(content) = content {
            std::fs::write(dir.path().join("Cargo.lock"), content).unwrap();
        }
        let result = orchestrator
            .audit_lock_age(
                &LockAgeAudit {
                    project_dir: dir.path(),
                    cutoff: cutoff(),
                    exemptions: &crate::update::AgeExemptions::default(),
                    baseline: &RegistryLockEntries::new(),
                    preferred: &PreferredVersions::new(),
                    adapter: &dates,
                    cargo: &CargoCommand::new(),
                    budget: LOCK_AGE_AUDIT_BUDGET,
                },
                None,
            )
            .await;
        assert!(!result.problems.is_empty());
        assert!(result.has_unresolved());
    }
    assert_eq!(dates.fetch_count(), 0);
}

#[tokio::test]
async fn verified_direct_and_transitive_releases_survive_final_lock_audit() {
    let registry = LocalRegistry::new();
    for version in ["1.0.0", "1.0.1"] {
        registry.publish("trusted-child", version, &[]);
        registry.publish("trusted-parent", version, &[("trusted-child", version)]);
        registry.publish("ordinary", version, &[]);
    }
    let project = TestProject::new(
        &registry,
        &manifest("trusted-parent = '=1.0.0'\nordinary = '=1.0.0'\n"),
    );
    let unpinned = manifest("trusted-parent = '1.0.0'\nordinary = '1.0.0'\n");
    let baseline = install(&project, &unpinned);
    let mut dates = FakeCratesIo::new();
    for name in ["trusted-parent", "trusted-child", "ordinary"] {
        dates = dates
            .release(name, "1.0.0", old())
            .release(name, "1.0.1", fresh());
    }
    for name in ["trusted-parent", "trusted-child"] {
        dates = dates.publisher(
            name,
            "1.0.1",
            crate::update::PublisherEvidence::GithubTrustedPublisher {
                owner: "example-dev".into(),
            },
        );
    }
    let config = crate::global_config::GlobalConfig {
        age: Some("2w".into()),
        age_exempt: crate::update::AgeExemptions {
            github: vec!["example-dev".into()],
        },
        ..Default::default()
    };
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"]))
        .unwrap()
        .with_global_config(Some(config));
    let cargo = cargo_for(&project);
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: project.path(),
                cutoff: cutoff(),
                exemptions: &orchestrator.resolved_age_exemptions(),
                baseline: &baseline,
                preferred: &PreferredVersions::new(),
                adapter: &dates,
                cargo: &cargo,
                budget: LOCK_AGE_AUDIT_BUDGET,
            },
            None,
        )
        .await;
    assert_eq!(project.locked_versions("trusted-parent"), ["1.0.1"]);
    assert_eq!(project.locked_versions("trusted-child"), ["1.0.1"]);
    assert_eq!(project.locked_versions("ordinary"), ["1.0.0"]);
    assert_eq!(result.unchecked, 0);
    assert_eq!(result.adjustments.len(), 1);
    assert_eq!(project.read("Cargo.toml"), unpinned);
    assert!(lock_is_accepted(&project));
}

/// この日時より後に公開された版を違反とする
fn cutoff() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 16, 0, 0, 0).unwrap()
}

/// 期間を十分に満たす版の公開日
fn old() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap()
}

/// 期間を満たす版の公開日 (基準日時の少し前)
fn mature() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 4, 0, 0, 0).unwrap()
}

/// 期間を満たさない版の公開日
fn fresh() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 25, 0, 0, 0).unwrap()
}

fn manifest(dependencies: &str) -> String {
    format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}"
    )
}

fn cargo_for(project: &TestProject) -> CargoCommand {
    project.cargo_envs().into_iter().fold(
        CargoCommand::new().program(TestProject::cargo_program()),
        |cargo, (key, value)| cargo.env(key, value),
    )
}

/// 2 つの頂点 (fam-a / fam-b) が fam-core を `=` で固定し合う一族を公開する。
/// wasm-bindgen 一族 (web-sys と wasm-bindgen-test がどちらも wasm-bindgen を `=` で掴む) と同じ形
fn publish_family(registry: &LocalRegistry) {
    for version in ["1.0.0", "1.0.1", "1.0.2"] {
        let exact = format!("={version}");
        registry.publish("fam-core", version, &[]);
        registry.publish("fam-a", version, &[("fam-core", exact.as_str())]);
        registry.publish("fam-b", version, &[("fam-core", exact.as_str())]);
    }
}

/// 一族の公開日: 1.0.0 は古く、1.0.1 は期間を満たし、1.0.2 は新しすぎる
fn with_family_dates(fake: FakeCratesIo) -> FakeCratesIo {
    ["fam-core", "fam-a", "fam-b"]
        .into_iter()
        .fold(fake, |fake, name| {
            fake.release(name, "1.0.0", old())
                .release(name, "1.0.1", mature())
                .release(name, "1.0.2", fresh())
        })
}

/// `=` で古い版に固定した manifest で lock を作ってから `unpinned` へ戻し、
/// `--install` と同じ `cargo update` を実行する。install 前の lock を返す
fn install(project: &TestProject, unpinned: &str) -> RegistryLockEntries {
    project.cargo_ok(&["generate-lockfile"]);
    project.write("Cargo.toml", unpinned);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    project.cargo_ok(&["update"]);
    baseline
}

async fn audit_with(
    project: &TestProject,
    baseline: &RegistryLockEntries,
    preferred: &PreferredVersions,
    adapter: &FakeCratesIo,
    budget: Duration,
) -> LockAgeAuditResult {
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let cargo = cargo_for(project);
    let audit = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &crate::update::AgeExemptions::default(),
        baseline,
        preferred,
        adapter,
        cargo: &cargo,
        budget,
    };
    orchestrator.audit_lock_age(&audit, None).await
}

async fn audit(
    project: &TestProject,
    baseline: &RegistryLockEntries,
    adapter: &FakeCratesIo,
) -> LockAgeAuditResult {
    audit_with(
        project,
        baseline,
        &PreferredVersions::new(),
        adapter,
        LOCK_AGE_AUDIT_BUDGET,
    )
    .await
}

fn adjustment<'a>(result: &'a LockAgeAuditResult, name: &str) -> &'a LockAgeAdjustment {
    result
        .adjustments
        .iter()
        .find(|adjustment| adjustment.name == name)
        .unwrap_or_else(|| panic!("{name} is not in {:?}", result.adjustments))
}

/// 元の manifest (`=` の固定を持たない) がそのまま lock を受け入れるか
fn lock_is_accepted(project: &TestProject) -> bool {
    project.cargo(&["update", "--workspace", "--locked"]).0
}

/// 頂点が 2 つある `=` 一族は 1 件ずつでは戻せないが、まとめて解き直して戻す。
/// 利用者の Cargo.toml は書き換えず、元の manifest で `--locked` が通る
#[tokio::test]
async fn test_family_with_two_tops_is_rolled_back_together() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
    );
    let unpinned = manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n");
    let baseline = install(&project, &unpinned);
    assert_eq!(project.locked_versions("fam-core"), vec!["1.0.2"]);

    let dates = with_family_dates(FakeCratesIo::new());
    let result = audit(&project, &baseline, &dates).await;

    for name in ["fam-a", "fam-b", "fam-core"] {
        assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
        let adjustment = adjustment(&result, name);
        assert_eq!(adjustment.status, LockAgeStatus::Downgraded, "{name}");
        assert_eq!(adjustment.to.as_deref(), Some("1.0.1"), "{name}");
    }
    assert_eq!(result.unchecked, 0);
    assert_eq!(project.read("Cargo.toml"), unpinned);
    assert!(lock_is_accepted(&project));
}

/// 直接依存を経由しない (推移依存だけの) 一族も、まとめて解き直して戻す
#[tokio::test]
async fn test_transitive_only_family_is_rolled_back_together() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    registry.publish("wrap-x", "1.0.0", &[("fam-a", "1.0.0")]);
    registry.publish("wrap-y", "1.0.0", &[("fam-b", "1.0.0")]);
    let project = TestProject::new(
        &registry,
        &manifest(
            "wrap-x = \"1.0.0\"\nwrap-y = \"1.0.0\"\nfam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n",
        ),
    );
    let unpinned = manifest("wrap-x = \"1.0.0\"\nwrap-y = \"1.0.0\"\n");
    let baseline = install(&project, &unpinned);
    assert_eq!(project.locked_versions("fam-core"), vec!["1.0.2"]);

    let dates = with_family_dates(FakeCratesIo::new())
        .release("wrap-x", "1.0.0", old())
        .release("wrap-y", "1.0.0", old());
    let result = audit(&project, &baseline, &dates).await;

    for name in ["fam-a", "fam-b", "fam-core"] {
        assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
        assert_eq!(
            adjustment(&result, name).status,
            LockAgeStatus::Downgraded,
            "{name}"
        );
    }
    assert_eq!(project.read("Cargo.toml"), unpinned);
    assert!(lock_is_accepted(&project));
}

/// 1 件ずつの差し戻しで済む crate の結果は変わらない (一族と同じ lock にあっても)
#[tokio::test]
async fn test_independent_crate_is_still_rolled_back_alone() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    for version in ["1.0.0", "1.0.1", "1.0.2"] {
        registry.publish("solo", version, &[]);
    }
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\nsolo = \"=1.0.0\"\n"),
    );
    let baseline = install(
        &project,
        &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\nsolo = \"1.0.0\"\n"),
    );

    let dates = with_family_dates(FakeCratesIo::new())
        .release("solo", "1.0.0", old())
        .release("solo", "1.0.1", mature())
        .release("solo", "1.0.2", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(project.locked_versions("solo"), vec!["1.0.1"]);
    let solo = adjustment(&result, "solo");
    assert_eq!(solo.status, LockAgeStatus::Downgraded);
    assert_eq!(solo.to.as_deref(), Some("1.0.1"));
    assert_eq!(project.locked_versions("fam-a"), vec!["1.0.1"]);
    assert!(lock_is_accepted(&project));
}

/// judge がこの実行で一族の 1 つだけ古い版 (OSV で新しい版を退けた等) を選んだ場合も、
/// その版を差し戻し先にし、残りの一族を噛み合う版へ揃える
#[tokio::test]
async fn test_judge_choice_is_used_as_rollback_target() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
    );
    let baseline = install(
        &project,
        &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
    );

    let dates = with_family_dates(FakeCratesIo::new());
    let mut preferred = PreferredVersions::new();
    preferred.insert("fam-a".to_string(), vec!["1.0.0".to_string()]);
    let result = audit_with(
        &project,
        &baseline,
        &preferred,
        &dates,
        LOCK_AGE_AUDIT_BUDGET,
    )
    .await;

    for name in ["fam-a", "fam-b", "fam-core"] {
        assert_eq!(project.locked_versions(name), vec!["1.0.0"], "{name}");
        assert_eq!(
            adjustment(&result, name).status,
            LockAgeStatus::Downgraded,
            "{name}"
        );
    }
    assert!(lock_is_accepted(&project));
}

/// Cargo.toml 自身が新しすぎる版を要求している crate は戻せない理由を示し、
/// それが同じ lock の一族のまとめ解きを巻き添えにしない
#[tokio::test]
async fn test_manifest_requirement_blocks_only_its_own_crate() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    registry.publish("blocker", "1.0.0", &[]);
    registry.publish("blocker", "1.0.1", &[]);
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
    );
    let unpinned = manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\nblocker = \"1.0.1\"\n");
    let baseline = install(&project, &unpinned);
    assert_eq!(project.locked_versions("blocker"), vec!["1.0.1"]);

    let dates = with_family_dates(FakeCratesIo::new())
        .release("blocker", "1.0.0", old())
        .release("blocker", "1.0.1", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(
        adjustment(&result, "blocker").status,
        LockAgeStatus::BlockedByManifest("^1.0.1".to_string())
    );
    assert_eq!(project.locked_versions("blocker"), vec!["1.0.1"]);
    for name in ["fam-a", "fam-b", "fam-core"] {
        assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
    }
    assert_eq!(project.read("Cargo.toml"), unpinned);
    assert!(lock_is_accepted(&project));
}

/// まとめ解きで新しく lock に入った crate も監査し、新しすぎれば差し戻す
#[tokio::test]
async fn test_crate_added_by_resolving_together_is_audited() {
    let registry = LocalRegistry::new();
    registry.publish("helper", "1.0.0", &[]);
    registry.publish("helper", "1.0.1", &[]);
    for version in ["1.0.0", "1.0.1", "1.0.2"] {
        let exact = format!("={version}");
        // 期間を満たす 1.0.1 だけが helper を必要とする
        if version == "1.0.1" {
            registry.publish("fam-core", version, &[("helper", "1.0.0")]);
        } else {
            registry.publish("fam-core", version, &[]);
        }
        registry.publish("fam-a", version, &[("fam-core", exact.as_str())]);
        registry.publish("fam-b", version, &[("fam-core", exact.as_str())]);
    }
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
    );
    let baseline = install(
        &project,
        &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
    );
    assert!(project.locked_versions("helper").is_empty());

    let dates = with_family_dates(FakeCratesIo::new())
        .release("helper", "1.0.0", old())
        .release("helper", "1.0.1", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(project.locked_versions("fam-core"), vec!["1.0.1"]);
    assert_eq!(project.locked_versions("helper"), vec!["1.0.0"]);
    assert_eq!(
        adjustment(&result, "helper").status,
        LockAgeStatus::Downgraded
    );
    assert!(lock_is_accepted(&project));
}

/// workspace (virtual manifest + member) でも写しでまとめて解き直し、member の
/// Cargo.toml は書き換えない
#[tokio::test]
async fn test_workspace_members_are_rolled_back_together() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    let project = TestProject::new(
        &registry,
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
    );
    let member = |name: &str, dependency: &str| {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependency}\n"
        )
    };
    project.write("crates/a/Cargo.toml", &member("a", "fam-a = \"=1.0.0\""));
    project.write("crates/a/src/lib.rs", "");
    project.write("crates/b/Cargo.toml", &member("b", "fam-b = \"=1.0.0\""));
    project.write("crates/b/src/lib.rs", "");
    project.cargo_ok(&["generate-lockfile"]);
    let member_a = member("a", "fam-a = \"1.0.0\"");
    let member_b = member("b", "fam-b = \"1.0.0\"");
    project.write("crates/a/Cargo.toml", &member_a);
    project.write("crates/b/Cargo.toml", &member_b);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    project.cargo_ok(&["update"]);
    assert_eq!(project.locked_versions("fam-core"), vec!["1.0.2"]);

    let dates = with_family_dates(FakeCratesIo::new());
    let result = audit(&project, &baseline, &dates).await;

    for name in ["fam-a", "fam-b", "fam-core"] {
        assert_eq!(project.locked_versions(name), vec!["1.0.1"], "{name}");
        assert_eq!(
            adjustment(&result, name).status,
            LockAgeStatus::Downgraded,
            "{name}"
        );
    }
    assert_eq!(project.read("crates/a/Cargo.toml"), member_a);
    assert_eq!(project.read("crates/b/Cargo.toml"), member_b);
    assert!(lock_is_accepted(&project));
}

/// install 前から期間内の版が入っていた crate は、それより古くせず install 前の版へ戻し、
/// 「期間を満たすよう差し戻した」とは区別して報告する
#[tokio::test]
async fn test_crate_returns_to_young_version_locked_before_install() {
    let registry = LocalRegistry::new();
    for version in ["1.0.0", "1.0.1", "1.0.2"] {
        registry.publish("solo", version, &[]);
    }
    let project = TestProject::new(&registry, &manifest("solo = \"=1.0.1\"\n"));
    let baseline = install(&project, &manifest("solo = \"1.0.1\"\n"));
    assert_eq!(project.locked_versions("solo"), vec!["1.0.2"]);

    let dates = FakeCratesIo::new()
        .release("solo", "1.0.0", old())
        .release(
            "solo",
            "1.0.1",
            Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap(),
        )
        .release("solo", "1.0.2", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(project.locked_versions("solo"), vec!["1.0.1"]);
    let solo = result
        .adjustments
        .iter()
        .find(|item| item.name == "solo" && item.from == "1.0.2")
        .unwrap();
    assert_eq!(solo.status, LockAgeStatus::Restored);
    assert_eq!(solo.to.as_deref(), Some("1.0.1"));
    assert_eq!(result.adjustments.len(), 1, "{result:?}");
    assert!(result.has_unresolved());
    assert!(lock_is_accepted(&project));
}

/// install 前の版が yank 済みでも、`--precise` で復活させず有効な古い版へ戻す。
#[tokio::test]
async fn a_yanked_baseline_is_not_restored_by_precise_rollback() {
    let registry = LocalRegistry::new();
    for version in ["0.8.2", "0.8.3", "0.8.4"] {
        registry.publish("solo", version, &[]);
    }
    let project = TestProject::new(&registry, &manifest("solo = '=0.8.3'\n"));
    project.cargo_ok(&["generate-lockfile"]);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    registry.yank("solo", "0.8.3");
    project.write("Cargo.toml", &manifest("solo = '0.8.2'\n"));
    project.cargo_ok(&["update"]);
    assert_eq!(project.locked_versions("solo"), ["0.8.4"]);

    // 本物の crates.io adapter と同じく、yank 済み版は候補一覧に含めない。
    let dates =
        FakeCratesIo::new()
            .release("solo", "0.8.2", old())
            .release("solo", "0.8.4", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(project.locked_versions("solo"), ["0.8.2"]);
    assert_eq!(
        adjustment(&result, "solo").status,
        LockAgeStatus::Downgraded
    );
    assert!(!result.has_unresolved());
    assert!(lock_is_accepted(&project));
}

/// 有効な差し戻し候補がなければ、yank 済みの install 前の版へ戻さず未解消を報告する。
#[tokio::test]
async fn a_yanked_baseline_without_a_mature_candidate_is_unresolved() {
    let registry = LocalRegistry::new();
    for version in ["0.8.2", "0.8.3", "0.8.4"] {
        registry.publish("solo", version, &[]);
    }
    let project = TestProject::new(&registry, &manifest("solo = '=0.8.3'\n"));
    project.cargo_ok(&["generate-lockfile"]);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    registry.yank("solo", "0.8.3");
    project.write("Cargo.toml", &manifest("solo = '0.8.2'\n"));
    project.cargo_ok(&["update"]);

    let dates = FakeCratesIo::new()
        .release("solo", "0.8.2", fresh())
        .release("solo", "0.8.4", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(project.locked_versions("solo"), ["0.8.4"]);
    assert_eq!(
        adjustment(&result, "solo").status,
        LockAgeStatus::NoOlderCandidate
    );
    assert!(result.has_unresolved());
}

/// まとめ解きの下限にも yank 済みの install 前の版を使わない。
#[tokio::test]
async fn a_yanked_baseline_does_not_block_joint_rollback() {
    let registry = LocalRegistry::new();
    for version in ["0.8.2", "0.8.3", "0.8.4"] {
        let exact = format!("={version}");
        registry.publish("fam-core", version, &[]);
        for name in ["fam-a", "fam-b"] {
            registry.publish(name, version, &[("fam-core", &exact)]);
        }
    }
    let project = TestProject::new(&registry, &manifest("fam-a = '=0.8.3'\nfam-b = '=0.8.3'\n"));
    project.cargo_ok(&["generate-lockfile"]);
    let baseline = parse_registry_entries(&project.read("Cargo.lock"));
    for name in ["fam-core", "fam-a", "fam-b"] {
        registry.yank(name, "0.8.3");
    }
    project.write(
        "Cargo.toml",
        &manifest("fam-a = '0.8.2'\nfam-b = '0.8.2'\n"),
    );
    project.cargo_ok(&["update"]);
    let mut dates = FakeCratesIo::new();
    for name in ["fam-core", "fam-a", "fam-b"] {
        assert_eq!(project.locked_versions(name), ["0.8.4"]);
        dates = dates
            .release(name, "0.8.2", old())
            .release(name, "0.8.4", fresh());
    }
    let result = audit(&project, &baseline, &dates).await;

    for name in ["fam-core", "fam-a", "fam-b"] {
        assert_eq!(project.locked_versions(name), ["0.8.2"]);
        assert_eq!(adjustment(&result, name).status, LockAgeStatus::Downgraded);
    }
    assert!(!result.has_unresolved());
    assert!(lock_is_accepted(&project));
}

/// 期間を満たす古い版が 1 つも無い crate は、差し戻せないものとして報告する
#[tokio::test]
async fn test_crate_without_older_mature_version_is_reported() {
    let registry = LocalRegistry::new();
    registry.publish("young", "0.1.0", &[]);
    let project = TestProject::new(&registry, &manifest(""));
    let unpinned = manifest("young = \"0.1.0\"\n");
    let baseline = install(&project, &unpinned);

    let dates = FakeCratesIo::new().release("young", "0.1.0", fresh());
    let result = audit(&project, &baseline, &dates).await;

    assert_eq!(
        adjustment(&result, "young").status,
        LockAgeStatus::NoOlderCandidate
    );
    assert_eq!(project.locked_versions("young"), vec!["0.1.0"]);
}

/// `--precise` が何も変えずに成功を返す cargo (directory source への置き換えと同じ振る舞い)
/// でも、lock を読み直して「差し戻した」と誤報告せず、まとめ解きで実際に戻す
#[cfg(unix)]
#[tokio::test]
async fn test_precise_that_changes_nothing_is_not_reported_as_rollback() {
    use std::os::unix::fs::PermissionsExt;

    let registry = LocalRegistry::new();
    for version in ["1.0.0", "1.0.1", "1.0.2"] {
        registry.publish("solo", version, &[]);
    }
    let project = TestProject::new(&registry, &manifest("solo = \"=1.0.0\"\n"));
    let baseline = install(&project, &manifest("solo = \"1.0.0\"\n"));
    assert_eq!(project.locked_versions("solo"), vec!["1.0.2"]);

    // `--precise` を含む起動だけ何もせずに exit 0 を返し、それ以外は本物の cargo へ渡す
    let stand_in = tempfile::TempDir::new().unwrap();
    let script = stand_in.path().join("cargo");
    std::fs::write(
        &script,
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--precise\" ]; then exit 0; fi\ndone\nexec \"$DEPUP_TEST_REAL_CARGO\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cargo = project
        .cargo_envs()
        .into_iter()
        .fold(
            CargoCommand::new().program(&script),
            |cargo, (key, value)| cargo.env(key, value),
        )
        .env("DEPUP_TEST_REAL_CARGO", TestProject::cargo_program());

    let dates = FakeCratesIo::new()
        .release("solo", "1.0.0", old())
        .release("solo", "1.0.1", mature())
        .release("solo", "1.0.2", fresh());
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let audit = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &crate::update::AgeExemptions::default(),
        baseline: &baseline,
        preferred: &PreferredVersions::new(),
        adapter: &dates,
        cargo: &cargo,
        budget: LOCK_AGE_AUDIT_BUDGET,
    };
    let result = orchestrator.audit_lock_age(&audit, None).await;

    assert_eq!(project.locked_versions("solo"), vec!["1.0.1"]);
    let solo = adjustment(&result, "solo");
    assert_eq!(solo.status, LockAgeStatus::Downgraded);
    assert_eq!(solo.to.as_deref(), Some("1.0.1"));
    assert!(lock_is_accepted(&project));
}

/// まとめ解きの結果を元の manifest が受け入れず、しかも元の lock へ戻せなかったときは、
/// 旧内容を退避した場所を `problems` で必ず知らせる (黙って壊れた lock を残さない)
#[cfg(unix)]
#[tokio::test]
async fn test_failed_restore_is_reported_with_backup() {
    use std::os::unix::fs::PermissionsExt;

    // root では書き込み禁止が効かず、この経路を再現できない
    let probe = tempfile::NamedTempFile::new().unwrap();
    std::fs::set_permissions(probe.path(), std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new()
        .write(true)
        .open(probe.path())
        .is_ok()
    {
        return;
    }

    let registry = LocalRegistry::new();
    publish_family(&registry);
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
    );
    let baseline = install(
        &project,
        &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
    );
    let before = project.read("Cargo.lock");

    // 元の manifest での検証 (`--locked`) だけを失敗させ、その前に Cargo.lock を
    // 書き込み禁止にして、元へ戻す書き込みも失敗させる
    let stand_in = tempfile::TempDir::new().unwrap();
    let script = stand_in.path().join("cargo");
    std::fs::write(
        &script,
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"--locked\" ]; then\n    chmod a-w Cargo.lock\n    echo 'error: simulated rejection' >&2\n    exit 1\n  fi\ndone\nexec \"$DEPUP_TEST_REAL_CARGO\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cargo = project
        .cargo_envs()
        .into_iter()
        .fold(
            CargoCommand::new().program(&script),
            |cargo, (key, value)| cargo.env(key, value),
        )
        .env("DEPUP_TEST_REAL_CARGO", TestProject::cargo_program());

    let dates = with_family_dates(FakeCratesIo::new());
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let audit = LockAgeAudit {
        project_dir: project.path(),
        cutoff: cutoff(),
        exemptions: &crate::update::AgeExemptions::default(),
        baseline: &baseline,
        preferred: &PreferredVersions::new(),
        adapter: &dates,
        cargo: &cargo,
        budget: LOCK_AGE_AUDIT_BUDGET,
    };
    let result = orchestrator.audit_lock_age(&audit, None).await;
    let lock_path = project.path().join("Cargo.lock");
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(result.problems.len(), 1, "{:?}", result.problems);
    let problem = &result.problems[0];
    assert!(problem.contains("simulated rejection"), "{problem}");
    assert!(problem.contains("restoring the previous"), "{problem}");
    let backup = problem
        .split("saved to ")
        .nth(1)
        .and_then(|rest| rest.split(" — ").next())
        .unwrap_or_else(|| panic!("no backup path in: {problem}"));
    assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
    std::fs::remove_file(backup).unwrap();
}

/// 時間予算が尽きていれば何も変えず、監査できなかった件数を返す
#[tokio::test]
async fn test_exhausted_budget_leaves_everything_unchecked() {
    let registry = LocalRegistry::new();
    publish_family(&registry);
    let project = TestProject::new(
        &registry,
        &manifest("fam-a = \"=1.0.0\"\nfam-b = \"=1.0.0\"\n"),
    );
    let baseline = install(
        &project,
        &manifest("fam-a = \"1.0.0\"\nfam-b = \"1.0.0\"\n"),
    );
    let before = project.read("Cargo.lock");

    let dates = with_family_dates(FakeCratesIo::new());
    let result = audit_with(
        &project,
        &baseline,
        &PreferredVersions::new(),
        &dates,
        Duration::ZERO,
    )
    .await;

    assert_eq!(result.unchecked, 3);
    assert_eq!(result.adjustments.len(), 3);
    assert!(
        result
            .adjustments
            .iter()
            .all(|adjustment| adjustment.status.is_unverified())
    );
    assert_eq!(project.read("Cargo.lock"), before);
    assert_eq!(dates.fetch_count(), 0);
}

/// 差し戻しの照会が予算切れになっても、全件検証には独立した枠がある。
#[tokio::test]
async fn final_verification_has_an_independent_time_budget() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct DelayedFirst(AtomicUsize);
    #[async_trait::async_trait]
    impl crate::registry::RegistryAdapter for DelayedFirst {
        fn language(&self) -> Language {
            Language::Rust
        }
        fn registry_name(&self) -> &'static str {
            "delayed-first"
        }
        async fn fetch_versions(
            &self,
            _: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok(vec![VersionInfo::new("1.0.0", old())])
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.lock"), "version = 4\n[[package]]\nname = 'library'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n").unwrap();
    let dates = DelayedFirst(AtomicUsize::new(0));
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: dir.path(),
                cutoff: cutoff(),
                exemptions: &Default::default(),
                baseline: &Default::default(),
                preferred: &Default::default(),
                adapter: &dates,
                cargo: &CargoCommand::new(),
                budget: Duration::from_millis(100),
            },
            None,
        )
        .await;
    assert_eq!(dates.0.load(Ordering::SeqCst), 2);
    assert!(!result.has_unresolved(), "{result:?}");
}

/// 候補から除外された lock 版を、公開日専用の取得経路で検証する。
#[tokio::test]
async fn candidate_filtered_locked_version_can_still_be_verified() {
    struct LockedOnly;
    #[async_trait::async_trait]
    impl crate::registry::RegistryAdapter for LockedOnly {
        fn language(&self) -> Language {
            Language::Rust
        }
        fn registry_name(&self) -> &'static str {
            "locked-only"
        }
        async fn fetch_versions(
            &self,
            _: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            Ok(vec![VersionInfo::new("1.0.0", old())])
        }
        async fn fetch_locked_versions(
            &self,
            _: &str,
            _: &[String],
            _: Option<DateTime<Utc>>,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            Ok(vec![VersionInfo::new("1.0.1", old())])
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.lock"), "version = 4\n[[package]]\nname = 'library'\nversion = '1.0.1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n").unwrap();
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: dir.path(),
                cutoff: cutoff(),
                exemptions: &Default::default(),
                baseline: &Default::default(),
                preferred: &Default::default(),
                adapter: &LockedOnly,
                cargo: &CargoCommand::new(),
                budget: LOCK_AGE_AUDIT_BUDGET,
            },
            None,
        )
        .await;
    assert!(!result.has_unresolved(), "{result:?}");
}

#[tokio::test]
async fn full_lock_verification_fetches_in_bounded_parallel_batches() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct ConcurrentDates {
        active: AtomicUsize,
        peak: AtomicUsize,
        count: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl crate::registry::RegistryAdapter for ConcurrentDates {
        fn language(&self) -> Language {
            Language::Rust
        }
        fn registry_name(&self) -> &'static str {
            "concurrent-dates"
        }
        async fn fetch_versions(
            &self,
            _: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![VersionInfo::new("1.0.0", old())])
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut content = "version = 4\n".to_string();
    for n in 0..64 {
        content.push_str(&format!("[[package]]\nname = 'library-{n}'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n"));
    }
    std::fs::write(dir.path().join("Cargo.lock"), &content).unwrap();
    let baseline = parse_registry_entries(&content);
    let dates = ConcurrentDates {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
    };
    let orchestrator = Orchestrator::new(CliArgs::parse_from(["depup"])).unwrap();
    let result = orchestrator
        .audit_lock_age(
            &LockAgeAudit {
                project_dir: dir.path(),
                cutoff: cutoff(),
                exemptions: &Default::default(),
                baseline: &baseline,
                preferred: &Default::default(),
                adapter: &dates,
                cargo: &CargoCommand::new(),
                budget: LOCK_AGE_AUDIT_BUDGET,
            },
            None,
        )
        .await;
    assert!(!result.has_unresolved(), "{result:?}");
    assert_eq!(dates.count.load(Ordering::SeqCst), 64);
    assert!((2..=8).contains(&dates.peak.load(Ordering::SeqCst)));
}
