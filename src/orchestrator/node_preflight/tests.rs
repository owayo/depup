use super::*;
use crate::cargo_scope::tests::TableResolver;
use crate::cargo_scope::{CargoAnchor, CargoMembership, CargoScopeResolver, LocatedCargoRoot};
use crate::domain::Language;
use crate::install::InstallPlan;
use crate::manifest::detect_manifests;
use crate::node_lock::{NodeAgeViolation, NodeUnverifiedDependency};
use crate::orchestrator::{Orchestrator, OrchestratorResult};
use crate::progress::Progress;
use crate::update::VersionInfo;
use clap::Parser;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
struct FakeAuditor {
    override_settings:
        Mutex<BTreeMap<PathBuf, Result<crate::pnpm_overrides::PnpmOverrides, String>>>,
    override_calls: Mutex<Vec<PathBuf>>,
    results: Mutex<BTreeMap<PathBuf, NodeLockAuditResult>>,
    calls: Mutex<Vec<(PathBuf, AgePolicy)>>,
}

#[async_trait]
impl ExistingLockAuditor for FakeAuditor {
    async fn overrides(
        &self,
        directory: &Path,
    ) -> Result<crate::pnpm_overrides::PnpmOverrides, String> {
        self.override_calls
            .lock()
            .unwrap()
            .push(directory.to_path_buf());
        self.override_settings
            .lock()
            .unwrap()
            .get(directory)
            .cloned()
            .unwrap_or_else(|| Ok(Default::default()))
    }

    async fn audit(
        &self,
        directory: &Path,
        policy: &AgePolicy,
        _bar: Option<&ProgressBar>,
    ) -> NodeLockAuditResult {
        self.calls
            .lock()
            .unwrap()
            .push((directory.into(), policy.clone()));
        self.results
            .lock()
            .unwrap()
            .remove(directory)
            .unwrap_or_default()
    }
}

fn write_project(directory: &Path) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("package.json"),
        r#"{"dependencies":{"example":"^1.0.0"}}"#,
    )
    .unwrap();
}

#[tokio::test]
async fn pnpm_overrides_are_cached_per_workspace_and_protect_dry_run_without_age() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    std::fs::write(root.join("pnpm-workspace.yaml"), "packages: ['apps/*']\n").unwrap();
    let a = root.join("apps/a");
    let b = root.join("apps/b");
    let outside = root.join("independent");
    for member in [&a, &b, &outside] {
        write_project(member);
    }
    let orchestrator = orchestrator(
        &root,
        &["--dry-run", "--install", "--no-age", "--include-pinned"],
    )
    .await;
    let auditor = FakeAuditor::default();
    auditor.override_settings.lock().unwrap().insert(
        root.clone(),
        Ok(crate::pnpm_overrides::PnpmOverrides::for_selectors(&[
            "example",
        ])),
    );
    let result = run(&orchestrator, &[&root, &outside], &auditor).await;
    assert_eq!(result.summary.total_skips(), 2);
    assert_eq!(result.summary.total_updates(), 1);
    assert_eq!(
        auditor
            .override_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|path| **path == root)
            .count(),
        1
    );
    assert!(result.summary.all_skips().all(|item| matches!(
        item,
        crate::domain::UpdateResult::Skip {
            reason: crate::domain::SkipReason::PnpmOverride(_),
            ..
        }
    )));
    assert!(auditor.calls.lock().unwrap().is_empty());
    for member in [&a, &b] {
        assert!(
            std::fs::read_to_string(member.join("package.json"))
                .unwrap()
                .contains("^1.0.0")
        );
    }
}

#[tokio::test]
async fn pnpm_unverified_overrides_stop_updates_before_write() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    let root = temp.path().canonicalize().unwrap();
    let orchestrator = orchestrator(&root, &["--no-age"]).await;
    let auditor = FakeAuditor::default();
    auditor
        .override_settings
        .lock()
        .unwrap()
        .insert(root.clone(), Err("configuration unavailable".into()));
    let result = run(&orchestrator, &[&root], &auditor).await;
    assert_eq!(result.summary.total_updates(), 0);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.to_string().contains("effective pnpm overrides"))
    );
    assert!(
        std::fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("^1.0.0")
    );
}

#[tokio::test]
async fn pnpm_override_queries_respect_package_filters() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    let root = temp.path().canonicalize().unwrap();
    for options in [
        vec!["--no-age", "--exclude", "example"],
        vec!["--no-age", "--only", "other"],
    ] {
        let orchestrator = orchestrator(&root, &options).await;
        let auditor = FakeAuditor::default();
        auditor
            .override_settings
            .lock()
            .unwrap()
            .insert(root.clone(), Err("must not be queried".into()));
        let result = run(&orchestrator, &[&root], &auditor).await;
        assert_eq!(result.summary.total_skips(), 1);
        assert!(result.errors.is_empty());
        assert!(auditor.override_calls.lock().unwrap().is_empty());
    }
}

fn lock(directory: &Path) {
    std::fs::write(directory.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
}

fn violation(orchestrator: &Orchestrator) -> NodeLockAuditResult {
    let policy = orchestrator.resolved_age_policy_for(&orchestrator.args.path);
    NodeLockAuditResult {
        violations: vec![NodeAgeViolation {
            name: "existing-child".into(),
            version: "1.0.1".into(),
            released_at: orchestrator.evaluated_at - chrono::Duration::days(1),
            cutoff: policy.cutoff().unwrap(),
        }],
        ..Default::default()
    }
}

async fn orchestrator(root: &Path, options: &[&str]) -> Orchestrator {
    let mut args = crate::cli::CliArgs::parse_from(
        ["depup", "--no-osv", "--quiet"]
            .into_iter()
            .chain(options.iter().copied()),
    );
    args.path = root.into();
    let orchestrator = Orchestrator::new(args).unwrap();
    let mut cache = orchestrator.versions.cache.lock().await;
    cache.insert(
        (Language::Node, "example".into()),
        vec![
            VersionInfo::new(
                "1.0.0",
                orchestrator.evaluated_at - chrono::Duration::days(90),
            ),
            VersionInfo::new(
                "2.0.0",
                orchestrator.evaluated_at - chrono::Duration::days(60),
            ),
        ],
    );
    drop(cache);
    orchestrator
}

async fn run(
    orchestrator: &Orchestrator,
    dirs: &[&Path],
    auditor: &FakeAuditor,
) -> OrchestratorResult {
    let manifests: Vec<_> = dirs.iter().flat_map(|dir| detect_manifests(dir)).collect();
    orchestrator
        .process_manifests_with_auditor(
            &manifests,
            &mut Progress::new(false),
            auditor,
            &crate::cargo_scope::SystemCargoScopeResolver,
        )
        .await
}

#[tokio::test]
async fn existing_lock_violation_blocks_workspace_writes_and_install_only() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let web = workspace.join("apps/web");
    let worker = workspace.join("apps/worker");
    let unchanged = workspace.join("apps/policy");
    let other = temp.path().join("other");
    for dir in [&workspace, &web, &worker, &unchanged, &other] {
        write_project(dir);
    }
    std::fs::write(unchanged.join("package.json"), "{\"name\":\"policy\"}").unwrap();
    std::fs::write(unchanged.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
    std::fs::write(
        workspace.join("pnpm-workspace.yaml"),
        "packages: ['apps/*']\n",
    )
    .unwrap();
    lock(&workspace);
    lock(&other);
    let originals: Vec<_> = [&workspace, &web, &worker, &unchanged]
        .into_iter()
        .map(|dir| {
            (
                dir.join("package.json"),
                std::fs::read(dir.join("package.json")).unwrap(),
            )
        })
        .collect();
    let lock_before = std::fs::read(workspace.join("pnpm-lock.yaml")).unwrap();
    let orchestrator = orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&workspace).unwrap(),
        violation(&orchestrator),
    );
    let result = run(&orchestrator, &[&workspace, &other], &auditor).await;
    for (path, bytes) in originals {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(
        std::fs::read(workspace.join("pnpm-lock.yaml")).unwrap(),
        lock_before
    );
    assert_eq!(result.summary.total_failed(), 3);
    assert_eq!(result.summary.total_updates(), 1);
    assert!(
        std::fs::read_to_string(other.join("package.json"))
            .unwrap()
            .contains("^2.0.0")
    );
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(plan.node_audits.len(), 1);
    assert_eq!(
        plan.jobs[0].directory,
        std::fs::canonicalize(&other).unwrap()
    );
    assert_eq!(plan.node_audits[0].directory, plan.jobs[0].directory);
    let calls = auditor.calls.lock().unwrap();
    let policy = &calls
        .iter()
        .find(|(dir, _)| dir == &std::fs::canonicalize(&workspace).unwrap())
        .unwrap()
        .1;
    assert_eq!(policy.min_age, Some(Duration::from_secs(30 * 86400)));
}

#[tokio::test]
async fn unverified_or_failed_existing_lock_blocks_without_clean_guidance() {
    for audit in [
        NodeLockAuditResult {
            unverified: vec![NodeUnverifiedDependency {
                name: "existing-child".into(),
                version: "1.0.1".into(),
                reason: "release date unavailable".into(),
            }],
            ..Default::default()
        },
        NodeLockAuditResult {
            errors: vec!["could not read pnpm-lock.yaml".into()],
            ..Default::default()
        },
        NodeLockAuditResult {
            errors: vec!["pnpm graph omitted the workspace root".into()],
            ..Default::default()
        },
    ] {
        let temp = tempfile::tempdir().unwrap();
        write_project(temp.path());
        lock(temp.path());
        let original = std::fs::read(temp.path().join("package.json")).unwrap();
        let orchestrator = orchestrator(temp.path(), &["--install"]).await;
        let scope = BlockedScope {
            directory: temp.path().into(),
            manifests: vec![],
            policy: Some(orchestrator.resolved_age_policy_for(temp.path())),
            failure: Failure::Audit(audit),
        };
        let report = report_lines(&scope).join("\n");
        assert!(!report.contains("pnpm clean"));
        assert!(report.contains("could not be verified"));
        assert!(!report.contains("violates --age"));
        let Failure::Audit(audit) = scope.failure else {
            unreachable!()
        };
        let auditor = FakeAuditor::default();
        auditor
            .results
            .lock()
            .unwrap()
            .insert(std::fs::canonicalize(temp.path()).unwrap(), audit);
        let result = run(&orchestrator, &[temp.path()], &auditor).await;
        assert_eq!(
            std::fs::read(temp.path().join("package.json")).unwrap(),
            original
        );
        assert_eq!(result.summary.total_failed(), 1);
        assert!(
            InstallPlan::new(&orchestrator, &result, &None, temp.path())
                .jobs
                .is_empty()
        );
    }
}

#[tokio::test]
async fn preflight_skips_noinstall_dryrun_noage_nolock_other_pm_and_no_updates() {
    for (options, lock_name, no_updates) in [
        (vec![], "pnpm-lock.yaml", false),
        (vec!["--install", "--dry-run"], "pnpm-lock.yaml", false),
        (vec!["--install", "--no-age"], "pnpm-lock.yaml", false),
        (vec!["--install"], "", false),
        (vec!["--install"], "package-lock.json", false),
        (vec!["--install"], "pnpm-lock.yaml", true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        write_project(temp.path());
        if !lock_name.is_empty() {
            std::fs::write(temp.path().join(lock_name), "lock").unwrap();
        }
        if no_updates {
            std::fs::write(
                temp.path().join("package.json"),
                r#"{"dependencies":{"example":"^2.0.0"}}"#,
            )
            .unwrap();
        }
        let orchestrator = orchestrator(temp.path(), &options).await;
        let auditor = FakeAuditor::default();
        let result = run(&orchestrator, &[temp.path()], &auditor).await;
        assert!(
            auditor.calls.lock().unwrap().is_empty(),
            "{options:?} {lock_name}"
        );
        assert_eq!(result.summary.total_failed(), 0);
        if no_updates {
            let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
            assert!(plan.jobs.is_empty());
            assert_eq!(plan.node_audits.len(), 1);
        }
    }
}

#[tokio::test]
async fn no_age_still_preflights_an_explicit_native_age() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    lock(temp.path());
    std::fs::write(temp.path().join(".npmrc"), "minimum-release-age=20d\n").unwrap();
    let orchestrator = orchestrator(temp.path(), &["--install", "--no-age"]).await;
    let auditor = FakeAuditor::default();
    let result = run(&orchestrator, &[temp.path()], &auditor).await;
    let calls = auditor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.min_age, Some(Duration::from_secs(20 * 86400)));
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    assert_eq!(calls[0].1, *plan.jobs[0].policy.as_ref().unwrap());
    assert_eq!(calls[0].1, *plan.node_audits[0].policy.as_ref().unwrap());
}

#[tokio::test]
async fn invalid_unchanged_member_policy_blocks_shared_writes_and_keeps_other_scope() {
    for unreadable in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let policy = workspace.join("apps/policy");
        let other = temp.path().join("other");
        for dir in [&workspace, &policy, &other] {
            write_project(dir);
        }
        std::fs::write(policy.join("package.json"), "{\"name\":\"policy\"}").unwrap();
        if unreadable {
            std::fs::create_dir(policy.join(".npmrc")).unwrap();
        } else {
            std::fs::write(policy.join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
        }
        std::fs::write(
            workspace.join("pnpm-workspace.yaml"),
            "packages: ['apps/*']\n",
        )
        .unwrap();
        lock(&workspace);
        lock(&other);
        let original = std::fs::read(workspace.join("package.json")).unwrap();
        let orchestrator = orchestrator(temp.path(), &["--install"]).await;
        let auditor = FakeAuditor::default();
        let result = run(&orchestrator, &[&workspace, &other], &auditor).await;
        assert_eq!(
            std::fs::read(workspace.join("package.json")).unwrap(),
            original
        );
        assert_eq!(result.summary.total_failed(), 1);
        assert_eq!(result.summary.total_updates(), 1);
        let calls = auditor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, std::fs::canonicalize(&other).unwrap());
        assert!(result.errors.iter().any(|error| {
            error
                .to_string()
                .contains("policy could not be read or interpreted")
        }));
        let scope = BlockedScope {
            directory: workspace,
            manifests: vec![],
            policy: None,
            failure: Failure::Policy("invalid setting".into()),
        };
        assert!(!report_lines(&scope).join("\n").contains("pnpm clean"));
    }
}

#[tokio::test]
async fn policy_resolution_error_is_a_preflight_failure() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    lock(temp.path());
    let orchestrator = orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    let result = run(&orchestrator, &[temp.path()], &auditor).await;
    let membership = CargoMembership::new(
        [],
        temp.path(),
        &crate::cargo_scope::SystemCargoScopeResolver,
    );
    let blocked = preflight(
        &result.summary,
        &ScopeContext {
            manifests: &detect_manifests(temp.path()),
            cargo: &membership,
        },
        &[],
        false,
        |_| Err("invalid native setting".into()),
        &auditor,
        None,
    )
    .await;
    assert_eq!(blocked.len(), 1);
    assert!(!report_lines(&blocked[0]).join("\n").contains("pnpm clean"));
}

#[cfg(unix)]
#[tokio::test]
async fn broken_pnpm_lock_symlink_never_falls_back_to_npm_or_writes() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    std::os::unix::fs::symlink("missing-lock", temp.path().join("pnpm-lock.yaml")).unwrap();
    let original = std::fs::read(temp.path().join("package.json")).unwrap();
    let orchestrator = orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    let result = run(&orchestrator, &[temp.path()], &auditor).await;
    assert!(auditor.calls.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read(temp.path().join("package.json")).unwrap(),
        original
    );
    assert_eq!(result.summary.total_failed(), 1);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.to_string().contains("could not be read or inspected"))
    );
    assert!(
        InstallPlan::new(&orchestrator, &result, &None, temp.path())
            .jobs
            .is_empty()
    );
}

#[test]
fn repair_guidance_uses_both_env_keys_and_ceils_subseconds() {
    assert_eq!(
        repair_install_command(Duration::new(60, 1)),
        "npm_config_minimum_release_age=2 pnpm_config_minimum_release_age=2 pnpm install"
    );
    let scope = BlockedScope {
        directory: PathBuf::from("workspace"),
        manifests: vec![],
        policy: Some(AgePolicy {
            min_age: Some(Duration::from_secs(14 * 86400)),
            evaluated_at: chrono::Utc::now() - chrono::Duration::minutes(2),
            exemptions: Default::default(),
        }),
        failure: Failure::Audit(NodeLockAuditResult {
            violations: vec![NodeAgeViolation {
                name: "existing-child".into(),
                version: "1.0.1".into(),
                released_at: chrono::Utc::now(),
                cutoff: chrono::Utc::now() - chrono::Duration::days(14),
            }],
            ..Default::default()
        }),
    };
    let report = report_lines(&scope).join("\n");
    assert!(report.contains("pnpm clean --lockfile"));
    assert!(report.contains("pnpm 11 or later"));
    assert!(report.contains("deletes node_modules and the lockfile"));
    assert!(report.contains("unrelated dependency versions can change"));
    assert!(report.contains("calculated from the fixed cutoff when this command is printed"));
    assert!(report.contains("rerun depup to regenerate the instructions"));
    let minutes: u64 = report
        .split("npm_config_minimum_release_age=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(minutes >= 20162);
}

fn write_rust(directory: &Path, name: &str, tauri: bool) {
    std::fs::create_dir_all(directory.join("src")).unwrap();
    std::fs::write(directory.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        format!(
            "[package]\nname = '{name}'\nversion = '0.1.0'\n[dependencies]\nexample = '1.0.0'\n{}",
            if tauri { "tauri = '2.0.0'\n" } else { "" },
        ),
    )
    .unwrap();
}

fn write_tauri(directory: &Path, name: &str) {
    write_project(directory);
    std::fs::write(
        directory.join("package.json"),
        r#"{"dependencies":{"@tauri-apps/api":"^2.0.0","example":"^1.0.0"}}"#,
    )
    .unwrap();
    write_rust(&directory.join("src-tauri"), name, true);
    lock(directory);
}

async fn tauri_orchestrator(root: &Path, options: &[&str]) -> Orchestrator {
    let orchestrator = orchestrator(root, options).await;
    let mut cache = orchestrator.versions.cache.lock().await;
    let date = orchestrator.evaluated_at - chrono::Duration::days(60);
    cache.insert(
        (Language::Rust, "example".into()),
        vec![VersionInfo::new("2.0.0", date)],
    );
    cache.insert(
        (Language::Rust, "tauri".into()),
        vec![VersionInfo::new("2.2.0", date)],
    );
    for name in crate::tauri_sync::TAURI_NPM_PACKAGES {
        cache.insert(
            (Language::Node, (*name).into()),
            vec![
                VersionInfo::new("2.2.0", date),
                VersionInfo::new("2.3.0", date),
            ],
        );
    }
    drop(cache);
    orchestrator
}

#[tokio::test]
async fn root_detected_tauri_members_join_the_blocked_node_and_cargo_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("apps/app");
    let helper = root.join("helper");
    let other = root.join("excluded");
    write_project(root);
    lock(root);
    write_tauri(&app, "app");
    std::fs::remove_file(app.join("pnpm-lock.yaml")).unwrap();
    write_rust(&helper, "helper", false);
    write_tauri(&other, "other");
    std::fs::write(root.join("pnpm-workspace.yaml"), "packages: ['apps/*']\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = ['apps/app/src-tauri', 'helper']\nexclude = ['excluded']\nresolver = '3'\n").unwrap();
    let detected = detect_manifests(root);
    let counterpart = detected
        .iter()
        .find(|manifest| manifest.path == app.join("src-tauri/Cargo.toml"))
        .unwrap();
    assert!(!counterpart.is_tauri_rust);
    let originals: Vec<_> = [
        root.join("package.json"),
        app.join("package.json"),
        app.join("src-tauri/Cargo.toml"),
        helper.join("Cargo.toml"),
    ]
    .into_iter()
    .map(|path| {
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    })
    .collect();
    let orchestrator = tauri_orchestrator(root, &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(root).unwrap(),
        violation(&orchestrator),
    );
    let result = run(&orchestrator, &[root, &other], &auditor).await;
    for (path, bytes) in originals {
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", path.display());
    }
    assert_eq!(result.summary.total_failed(), 6);
    assert_eq!(result.summary.total_updates(), 4);
    let dirs = Some(vec![root.into(), other.clone()]);
    for only in [false, true] {
        let plan = InstallPlan::new_scoped(&orchestrator, &result, &dirs, root, only);
        assert!(plan.jobs.iter().all(|job| {
            std::fs::canonicalize(&job.directory)
                .unwrap()
                .starts_with(std::fs::canonicalize(&other).unwrap())
        }));
        assert!(plan.jobs.iter().any(|job| job.language == Language::Rust));
    }
}

#[tokio::test]
async fn failed_node_policy_keeps_tauri_rust_unwritten_and_independent_projects_running() {
    for unreadable in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let other = temp.path().join("other");
        write_tauri(&app, "app");
        write_tauri(&other, "other");
        if unreadable {
            std::fs::create_dir(app.join(".npmrc")).unwrap();
        } else {
            std::fs::write(app.join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
        }
        let node_before = std::fs::read(app.join("package.json")).unwrap();
        let rust_before = std::fs::read(app.join("src-tauri/Cargo.toml")).unwrap();
        let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
        let auditor = FakeAuditor::default();
        let result = run(&orchestrator, &[&app, &other], &auditor).await;
        assert_eq!(
            std::fs::read(app.join("package.json")).unwrap(),
            node_before
        );
        assert_eq!(
            std::fs::read(app.join("src-tauri/Cargo.toml")).unwrap(),
            rust_before
        );
        assert_eq!(result.summary.total_failed(), 2);
        assert_eq!(result.summary.total_updates(), 4);
        let calls = auditor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, std::fs::canonicalize(&other).unwrap());
        assert!(result.errors.iter().any(|error| matches!(error,
            crate::orchestrator::OrchestratorError::NodePreflightError { directory, message }
            if directory == &std::fs::canonicalize(&app).unwrap() && message.contains("policy could not be read or interpreted"))));
        let plan = InstallPlan::new_scoped(
            &orchestrator,
            &result,
            &Some(vec![app.clone(), other.clone()]),
            temp.path(),
            true,
        );
        assert!(plan.jobs.iter().all(|job| {
            std::fs::canonicalize(&job.directory)
                .unwrap()
                .starts_with(std::fs::canonicalize(&other).unwrap())
        }));
    }
}

#[tokio::test]
async fn failed_node_policy_does_not_preflight_excluded_languages_dryrun_or_noinstall() {
    for options in [
        vec!["--rust", "--install"],
        vec!["--install", "--dry-run"],
        vec![],
    ] {
        let temp = tempfile::tempdir().unwrap();
        write_tauri(temp.path(), "app");
        std::fs::write(temp.path().join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
        let orchestrator = tauri_orchestrator(temp.path(), &options).await;
        let auditor = FakeAuditor::default();
        let result = run(&orchestrator, &[temp.path()], &auditor).await;
        assert_eq!(result.summary.total_failed(), 0);
        assert!(result.summary.total_updates() >= 2);
        assert!(auditor.calls.lock().unwrap().is_empty());
        assert!(result.errors.iter().all(|error| !matches!(
            error,
            crate::orchestrator::OrchestratorError::NodePreflightError { .. }
        )));
    }
}

#[tokio::test]
async fn failed_node_policy_without_actual_updates_does_not_preflight() {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    lock(temp.path());
    write_rust(&temp.path().join("src-tauri"), "app", false);
    let rust = temp.path().join("src-tauri/Cargo.toml");
    let content = std::fs::read_to_string(&rust)
        .unwrap()
        .replace("example = '1.0.0'", "example = '2.0.0'");
    std::fs::write(rust, content).unwrap();
    std::fs::write(temp.path().join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
    let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    let result = run(&orchestrator, &[temp.path()], &auditor).await;
    assert_eq!(result.summary.total_updates(), 0);
    assert_eq!(result.summary.total_failed(), 0);
    assert!(auditor.calls.lock().unwrap().is_empty());
    assert!(result.errors.iter().all(|error| !matches!(
        error,
        crate::orchestrator::OrchestratorError::NodePreflightError { .. }
    )));
}

struct UnavailableCargoResolver {
    independent: PathBuf,
    recovered: Option<PathBuf>,
}

struct MembershipFixture {
    temp: tempfile::TempDir,
    workspace: PathBuf,
    app: PathBuf,
    companion: PathBuf,
    helper: PathBuf,
    other: PathBuf,
}

impl MembershipFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let app = workspace.join("app");
        let companion = workspace.join("companion");
        let helper = workspace.join("helper");
        let other = temp.path().join("other");
        write_tauri(&app, "app");
        write_tauri(&companion, "companion");
        write_rust(&helper, "helper", false);
        write_tauri(&other, "other");
        std::fs::write(workspace.join("Cargo.toml"), "[workspace]\nmembers=['app/src-tauri', 'companion/src-tauri', 'helper']\nresolver='3'\n").unwrap();
        Self {
            temp,
            workspace,
            app,
            companion,
            helper,
            other,
        }
    }

    fn manifests(&self) -> Vec<ManifestInfo> {
        [&self.app, &self.companion, &self.helper, &self.other]
            .into_iter()
            .flat_map(|dir| detect_manifests(dir))
            .collect()
    }

    fn separate(&self, member: &str) -> PathBuf {
        let directory = self.workspace.join(member);
        let path = directory.join("Cargo.toml");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("[workspace]\n");
        std::fs::write(&path, content).unwrap();
        let members: Vec<_> = ["app/src-tauri", "companion/src-tauri", "helper"]
            .into_iter()
            .filter(|candidate| *candidate != member)
            .collect();
        std::fs::write(
            self.workspace.join("Cargo.toml"),
            format!("[workspace]\nmembers={members:?}\nexclude=['{member}']\nresolver='3'\n"),
        )
        .unwrap();
        std::fs::canonicalize(directory).unwrap()
    }

    fn unchanged(&self, directory: &Path) {
        std::fs::write(directory.join("package.json"), r#"{"name":"unchanged"}"#).unwrap();
        let path = directory.join("src-tauri/Cargo.toml");
        let content = std::fs::read_to_string(&path)
            .unwrap()
            .replace("example = '1.0.0'", "example = '2.0.0'")
            .replace("tauri = '2.0.0'\n", "");
        std::fs::write(path, content).unwrap();
    }

    fn resolver(&self, separate: Option<&Path>) -> TableResolver {
        let mut resolver = TableResolver::default();
        let workspace = std::fs::canonicalize(&self.workspace).unwrap();
        let other = std::fs::canonicalize(self.other.join("src-tauri")).unwrap();
        for manifest in self
            .manifests()
            .iter()
            .filter(|manifest| manifest.language == Language::Rust)
        {
            let target = std::fs::canonicalize(&manifest.path).unwrap();
            let root = if target.starts_with(&other) {
                &other
            } else if separate.is_some_and(|root| target.starts_with(root)) {
                separate.unwrap()
            } else {
                &workspace
            };
            resolver.known(&manifest.path, root);
        }
        resolver
    }

    fn snapshot(&self, directories: &[&Path]) -> BTreeMap<PathBuf, Vec<u8>> {
        directories
            .iter()
            .flat_map(|directory| {
                [
                    "package.json",
                    "src-tauri/Cargo.toml",
                    "Cargo.toml",
                    "pnpm-lock.yaml",
                ]
                .into_iter()
                .map(|name| directory.join(name))
            })
            .filter(|path| path.is_file())
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    fn assert_plan(
        &self,
        orchestrator: &Orchestrator,
        result: &OrchestratorResult,
        nodes: &[&Path],
        roots: &[&Path],
    ) {
        let directories = Some(vec![
            self.app.clone(),
            self.companion.clone(),
            self.helper.clone(),
            self.other.clone(),
        ]);
        for only in [false, true] {
            let plan =
                InstallPlan::new_scoped(orchestrator, result, &directories, self.temp.path(), only);
            let rust_dirs: Vec<PathBuf> = if only
                || result
                    .errors
                    .iter()
                    .any(|error| matches!(error, OrchestratorError::NodePreflightError { .. }))
            {
                roots
                    .iter()
                    .map(|path| std::fs::canonicalize(path).unwrap())
                    .collect()
            } else {
                result
                    .summary
                    .manifests
                    .iter()
                    .filter(|manifest| {
                        manifest.language == Language::Rust && manifest.has_updates()
                    })
                    .map(|manifest| {
                        let parent = manifest.path.parent().unwrap();
                        let project = if manifest.path.ends_with("src-tauri/Cargo.toml") {
                            parent.parent().unwrap()
                        } else {
                            parent
                        };
                        std::fs::canonicalize(project).unwrap()
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            };
            assert_eq!(plan.jobs.len(), nodes.len() + rust_dirs.len());
            for (language, expected) in nodes.iter().map(|path| (Language::Node, *path)).chain(
                rust_dirs
                    .iter()
                    .map(|path| (Language::Rust, path.as_path())),
            ) {
                let expected = std::fs::canonicalize(expected).unwrap();
                assert!(
                    plan.jobs.iter().any(|job| job.language == language
                        && std::fs::canonicalize(&job.directory).unwrap() == expected),
                    "missing job for {}",
                    expected.display()
                );
            }
            let audit_nodes: Vec<_> = result
                .summary
                .manifests
                .iter()
                .filter(|manifest| manifest.language == Language::Node)
                .map(|manifest| node_resolution_directory(&manifest.path))
                .filter(|directory| stopped_node_reason(result, directory).is_none())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            assert_eq!(plan.node_audits.len(), audit_nodes.len());
            for expected in audit_nodes {
                assert!(plan.node_audits.iter().any(|job| job.directory == expected));
            }
        }
    }
}

fn assert_unchanged(snapshot: BTreeMap<PathBuf, Vec<u8>>) {
    for (path, bytes) in snapshot {
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", path.display());
    }
}

fn stopped_node_reason<'a>(result: &'a OrchestratorResult, node: &Path) -> Option<&'a str> {
    let node = std::fs::canonicalize(node).unwrap();
    let messages: Vec<_> = result
        .errors
        .iter()
        .filter_map(|error| match error {
            OrchestratorError::NodePreflightError { directory, message } if directory == &node => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(messages.len() <= 1, "duplicate stopped Node scope");
    messages.first().copied()
}

#[tokio::test]
async fn recovered_or_unknown_counterparts_use_the_same_gate_and_stop_snapshot() {
    for failed_app in [false, true] {
        for answer in ["same", "different", "unknown"] {
            let fixture = MembershipFixture::new();
            let failed_project = if failed_app {
                &fixture.app
            } else {
                &fixture.companion
            };
            let failed = failed_project.join("src-tauri/Cargo.toml");
            let separate = (answer == "different").then(|| {
                fixture.separate(if failed_app {
                    "app/src-tauri"
                } else {
                    "companion/src-tauri"
                })
            });
            let mut resolver = fixture.resolver(separate.as_deref());
            resolver.fail(&failed);
            let response = match answer {
                "same" => Ok(std::fs::canonicalize(&fixture.workspace).unwrap()),
                "different" => Ok(separate.clone().unwrap()),
                _ => Err("secondary context could not inspect this member".into()),
            };
            resolver.answer(&failed, response);
            let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
            let auditor = FakeAuditor::default();
            auditor.results.lock().unwrap().insert(
                std::fs::canonicalize(&fixture.app).unwrap(),
                violation(&orchestrator),
            );
            let stops_other = failed_app && answer == "unknown";
            let stops_companion = answer != "different";
            let stops_helper = !failed_app || answer != "different";
            let mut stopped = vec![fixture.app.as_path()];
            if stops_companion {
                stopped.push(&fixture.companion);
            }
            if stops_helper {
                stopped.push(&fixture.helper);
            }
            if stops_other {
                stopped.push(&fixture.other);
            }
            let before = fixture.snapshot(&stopped);
            let result = orchestrator
                .process_manifests_with_auditor(
                    &fixture.manifests(),
                    &mut Progress::new(false),
                    &auditor,
                    &resolver,
                )
                .await;
            assert_unchanged(before);
            let failed_count = 4
                + usize::from(stops_companion) * 4
                + usize::from(stops_helper)
                + usize::from(stops_other) * 4;
            assert_eq!(result.summary.total_failed(), failed_count);
            assert_eq!(result.summary.total_updates(), 13 - failed_count);
            resolver.assert_cached();
            if stops_companion {
                let reason = stopped_node_reason(&result, &fixture.companion).unwrap();
                if answer == "unknown" {
                    assert!(reason.contains("simulated member toolchain failure"));
                    assert!(reason.contains("secondary context could not inspect"));
                    assert!(!reason.contains("shares a synchronized Tauri install"));
                } else {
                    assert!(reason.contains("shares a synchronized Tauri install"));
                }
            }
            let mut nodes = Vec::new();
            let mut roots = Vec::new();
            let other_root = fixture.other.join("src-tauri");
            if !stops_other {
                nodes.push(fixture.other.as_path());
                roots.push(other_root.as_path());
            }
            if !stops_companion {
                nodes.push(fixture.companion.as_path());
                roots.push(if failed_app {
                    fixture.workspace.as_path()
                } else {
                    separate.as_deref().unwrap()
                });
            }
            fixture.assert_plan(&orchestrator, &result, &nodes, &roots);
        }
    }
}

#[tokio::test]
async fn failed_node_policy_gates_mixed_roots_without_assuming_errors_are_independent() {
    for failed_counterpart in [false, true] {
        for unreadable in [false, true] {
            for answer in ["same", "different", "unknown"] {
                let fixture = MembershipFixture::new();
                fixture.unchanged(&fixture.app);
                fixture.unchanged(&fixture.companion);
                if unreadable {
                    std::fs::create_dir(fixture.app.join(".npmrc")).unwrap();
                } else {
                    std::fs::write(fixture.app.join(".npmrc"), "minimum-release-age=invalid\n")
                        .unwrap();
                }
                let member = if failed_counterpart {
                    "app/src-tauri"
                } else {
                    "helper"
                };
                let failed = fixture.workspace.join(member).join("Cargo.toml");
                let separate = (answer == "different").then(|| fixture.separate(member));
                let mut resolver = fixture.resolver(separate.as_deref());
                resolver.fail(&failed);
                resolver.answer(
                    &failed,
                    match answer {
                        "same" => Ok(std::fs::canonicalize(&fixture.workspace).unwrap()),
                        "different" => Ok(separate.clone().unwrap()),
                        _ => Err("secondary context could not inspect this member".into()),
                    },
                );
                let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
                let auditor = FakeAuditor::default();
                let stopped = answer != "different";
                let stops_other = failed_counterpart && answer == "unknown";
                let mut paths = vec![fixture.app.as_path(), fixture.companion.as_path()];
                if stopped {
                    paths.push(&fixture.helper);
                }
                if stops_other {
                    paths.push(&fixture.other);
                }
                let before = fixture.snapshot(&paths);
                let result = orchestrator
                    .process_manifests_with_auditor(
                        &fixture.manifests(),
                        &mut Progress::new(false),
                        &auditor,
                        &resolver,
                    )
                    .await;
                assert_unchanged(before);
                assert_eq!(
                    result.summary.total_failed(),
                    usize::from(stopped) + usize::from(stops_other) * 4
                );
                assert_eq!(
                    result.summary.total_updates(),
                    usize::from(!stopped) + usize::from(!stops_other) * 4
                );
                assert_eq!(
                    stopped_node_reason(&result, &fixture.app).is_some(),
                    stopped
                );
                assert!(result.errors.iter().any(|error| matches!(error, OrchestratorError::ManifestParseError { path, .. } if path == &fixture.app.join("package.json").display().to_string())));
                if stopped && answer == "unknown" {
                    assert!(result.errors.iter().any(|error| matches!(error, OrchestratorError::WriteError { message, .. } if message.contains("secondary context could not inspect"))));
                }
                resolver.assert_cached();
                let mut nodes = Vec::new();
                let mut roots = Vec::new();
                let other_root = fixture.other.join("src-tauri");
                if !stops_other {
                    nodes.push(fixture.other.as_path());
                    roots.push(other_root.as_path());
                }
                if !stopped {
                    roots.push(if failed_counterpart {
                        fixture.workspace.as_path()
                    } else {
                        separate.as_deref().unwrap()
                    });
                }
                fixture.assert_plan(&orchestrator, &result, &nodes, &roots);
            }
        }
    }
}

#[tokio::test]
async fn unknown_plain_rust_is_a_stopped_leaf_and_keeps_confirmed_independent_tauri_running() {
    let fixture = MembershipFixture::new();
    let mut resolver = fixture.resolver(None);
    resolver.fail(&fixture.helper.join("Cargo.toml"));
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.app).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[&fixture.app, &fixture.companion, &fixture.helper]);
    let result = orchestrator
        .process_manifests_with_auditor(
            &fixture.manifests(),
            &mut Progress::new(false),
            &auditor,
            &resolver,
        )
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 9);
    assert_eq!(result.summary.total_updates(), 4);
    assert!(stopped_node_reason(&result, &fixture.other).is_none());
    fixture.assert_plan(
        &orchestrator,
        &result,
        &[&fixture.other],
        &[&fixture.other.join("src-tauri")],
    );
    resolver.assert_cached();
}

#[tokio::test]
async fn shared_scope_node_update_alone_enters_failed_policy_preflight() {
    let fixture = MembershipFixture::new();
    fixture.unchanged(&fixture.app);
    fixture.unchanged(&fixture.companion);
    std::fs::write(
        fixture.companion.join("package.json"),
        r#"{"dependencies":{"example":"^1.0.0"}}"#,
    )
    .unwrap();
    let helper = fixture.helper.join("Cargo.toml");
    let content = std::fs::read_to_string(&helper)
        .unwrap()
        .replace("example = '1.0.0'", "example = '2.0.0'");
    std::fs::write(helper, content).unwrap();
    std::fs::write(fixture.app.join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
    let resolver = fixture.resolver(None);
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    let before = fixture.snapshot(&[&fixture.app, &fixture.companion, &fixture.helper]);
    let result = orchestrator
        .process_manifests_with_auditor(
            &fixture.manifests(),
            &mut Progress::new(false),
            &auditor,
            &resolver,
        )
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 1);
    assert_eq!(result.summary.total_updates(), 4);
    assert!(
        stopped_node_reason(&result, &fixture.companion)
            .unwrap()
            .contains("shares a synchronized Tauri install")
    );
    fixture.assert_plan(
        &orchestrator,
        &result,
        &[&fixture.other],
        &[&fixture.other.join("src-tauri")],
    );
    resolver.assert_cached();
}

#[tokio::test]
async fn two_unknown_counterparts_in_one_node_scope_can_reach_a_different_known_root() {
    let fixture = MembershipFixture::new();
    let friend = fixture.companion.join("apps/friend");
    write_tauri(&friend, "friend");
    std::fs::remove_file(friend.join("pnpm-lock.yaml")).unwrap();
    std::fs::write(
        fixture.companion.join("pnpm-workspace.yaml"),
        "packages: ['apps/*']\n",
    )
    .unwrap();
    std::fs::write(fixture.workspace.join("Cargo.toml"), "[workspace]\nmembers=['app/src-tauri', 'companion/src-tauri', 'companion/apps/friend/src-tauri', 'helper']\nresolver='3'\n").unwrap();
    let mut manifests = fixture.manifests();
    manifests.extend(detect_manifests(&friend));
    manifests.sort_by(|a, b| a.path.cmp(&b.path));
    manifests.dedup_by(|a, b| a.path == b.path);
    let mut resolver = fixture.resolver(None);
    resolver.fail(&fixture.companion.join("src-tauri/Cargo.toml"));
    resolver.fail(&friend.join("src-tauri/Cargo.toml"));
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.app).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[
        &fixture.app,
        &fixture.companion,
        &fixture.helper,
        &fixture.other,
        &friend,
    ]);
    let result = orchestrator
        .process_manifests_with_auditor(&manifests, &mut Progress::new(false), &auditor, &resolver)
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 17);
    assert_eq!(result.summary.total_updates(), 0);
    assert!(
        stopped_node_reason(&result, &fixture.other)
            .unwrap()
            .contains("could not be confirmed as independent")
    );
    let directories = Some(vec![
        fixture.app.clone(),
        fixture.companion.clone(),
        fixture.helper.clone(),
        fixture.other.clone(),
        friend,
    ]);
    for only in [false, true] {
        let plan = InstallPlan::new_scoped(
            &orchestrator,
            &result,
            &directories,
            fixture.temp.path(),
            only,
        );
        assert!(plan.jobs.is_empty());
        assert!(plan.node_audits.is_empty());
    }
    resolver.assert_cached();
}

#[tokio::test]
async fn conflicting_rechecks_stop_the_uncertain_node_but_do_not_bridge_known_roots() {
    let fixture = MembershipFixture::new();
    let failed = std::fs::canonicalize(fixture.companion.join("src-tauri/Cargo.toml")).unwrap();
    let mut resolver = fixture.resolver(None);
    resolver.fail(&failed);
    for located in resolver
        .primary
        .values()
        .filter_map(|answer| answer.as_ref().ok())
    {
        resolver.secondary.insert(
            (located.anchor.cwd.clone(), failed.clone()),
            Ok(located.root.clone()),
        );
    }
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.app).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[&fixture.app, &fixture.companion, &fixture.helper]);
    let result = orchestrator
        .process_manifests_with_auditor(
            &fixture.manifests(),
            &mut Progress::new(false),
            &auditor,
            &resolver,
        )
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 9);
    assert_eq!(result.summary.total_updates(), 4);
    assert!(
        stopped_node_reason(&result, &fixture.companion)
            .unwrap()
            .contains("anchors disagree")
    );
    fixture.assert_plan(
        &orchestrator,
        &result,
        &[&fixture.other],
        &[&fixture.other.join("src-tauri")],
    );
    resolver.assert_cached();
}

#[tokio::test]
async fn all_primary_cargo_failures_stop_the_unresolved_connected_install_without_retry_anchors() {
    let fixture = MembershipFixture::new();
    let mut resolver = fixture.resolver(None);
    for manifest in fixture
        .manifests()
        .iter()
        .filter(|manifest| manifest.language == Language::Rust)
    {
        resolver.fail(&manifest.path);
    }
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.app).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[
        &fixture.app,
        &fixture.companion,
        &fixture.helper,
        &fixture.other,
    ]);
    let result = orchestrator
        .process_manifests_with_auditor(
            &fixture.manifests(),
            &mut Progress::new(false),
            &auditor,
            &resolver,
        )
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 13);
    assert_eq!(result.summary.total_updates(), 0);
    fixture.assert_plan(&orchestrator, &result, &[], &[]);
    assert!(
        resolver
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(anchor, _)| anchor.is_none())
    );
    resolver.assert_cached();
}

#[tokio::test]
async fn cargo_membership_is_not_requested_for_inactive_preflight_or_no_actual_updates() {
    for options in [
        vec!["--rust", "--install"],
        vec!["--install", "--dry-run"],
        vec![],
        vec!["--install"],
    ] {
        let fixture = MembershipFixture::new();
        if options == ["--install"] {
            for directory in [&fixture.app, &fixture.companion, &fixture.other] {
                fixture.unchanged(directory);
            }
            let path = fixture.helper.join("Cargo.toml");
            let content = std::fs::read_to_string(&path)
                .unwrap()
                .replace("example = '1.0.0'", "example = '2.0.0'");
            std::fs::write(path, content).unwrap();
        }
        std::fs::write(fixture.app.join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
        let resolver = fixture.resolver(None);
        let orchestrator = tauri_orchestrator(fixture.temp.path(), &options).await;
        let auditor = FakeAuditor::default();
        let result = orchestrator
            .process_manifests_with_auditor(
                &fixture.manifests(),
                &mut Progress::new(false),
                &auditor,
                &resolver,
            )
            .await;
        assert!(resolver.calls.lock().unwrap().is_empty());
        assert_eq!(result.summary.total_failed(), 0);
        assert!(stopped_node_reason(&result, &fixture.app).is_none());
    }
}

#[tokio::test]
async fn valid_unchanged_node_scopes_are_audited_before_connected_rust_updates() {
    let fixture = MembershipFixture::new();
    fixture.unchanged(&fixture.app);
    fixture.unchanged(&fixture.companion);
    let resolver = fixture.resolver(None);
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.app).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[&fixture.app, &fixture.companion, &fixture.helper]);
    let result = orchestrator
        .process_manifests_with_auditor(
            &fixture.manifests(),
            &mut Progress::new(false),
            &auditor,
            &resolver,
        )
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 1);
    assert_eq!(result.summary.total_updates(), 4);
    assert_eq!(
        result
            .errors
            .iter()
            .filter(|error| matches!(error, OrchestratorError::CargoPreflightError { .. }))
            .count(),
        3
    );
    assert!(
        auditor
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(directory, _)| directory == &std::fs::canonicalize(&fixture.app).unwrap())
    );
    fixture.assert_plan(
        &orchestrator,
        &result,
        &[&fixture.other],
        &[&fixture.other.join("src-tauri")],
    );
    resolver.assert_cached();
}

#[cfg(unix)]
#[tokio::test]
async fn aliases_of_one_unknown_counterpart_do_not_bridge_confirmed_independent_roots() {
    let fixture = MembershipFixture::new();
    let alias_project = fixture.companion.join("apps/alias");
    std::fs::create_dir_all(&alias_project).unwrap();
    std::fs::write(alias_project.join("package.json"), r#"{"name":"alias"}"#).unwrap();
    std::os::unix::fs::symlink(
        fixture.companion.join("src-tauri"),
        alias_project.join("src-tauri"),
    )
    .unwrap();
    std::fs::write(
        fixture.companion.join("pnpm-workspace.yaml"),
        "packages: ['apps/*']\n",
    )
    .unwrap();
    let mut manifests = fixture.manifests();
    manifests.extend(detect_manifests(&alias_project));
    manifests.sort_by(|a, b| a.path.cmp(&b.path));
    manifests.dedup_by(|a, b| a.path == b.path);
    let mut resolver = fixture.resolver(None);
    let target = std::fs::canonicalize(fixture.companion.join("src-tauri/Cargo.toml")).unwrap();
    resolver.fail(&target);
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.app).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[
        &fixture.app,
        &fixture.companion,
        &fixture.helper,
        &alias_project,
    ]);
    let result = orchestrator
        .process_manifests_with_auditor(&manifests, &mut Progress::new(false), &auditor, &resolver)
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 11);
    assert_eq!(result.summary.total_updates(), 4);
    assert_eq!(result.errors.iter().filter(|error| matches!(error,
        OrchestratorError::CargoPreflightError { manifest, root: None, .. }
        if manifest == &std::fs::canonicalize(fixture.companion.join("src-tauri/Cargo.toml")).unwrap())).count(), 1);
    assert!(stopped_node_reason(&result, &fixture.other).is_none());
    fixture.assert_plan(
        &orchestrator,
        &result,
        &[&fixture.other],
        &[&fixture.other.join("src-tauri")],
    );
    resolver.assert_cached();
    assert_eq!(
        resolver
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(anchor, manifest)| anchor.is_none() && manifest == &target)
            .count(),
        1
    );
}

#[test]
fn unknown_counterpart_cannot_bridge_roots_without_a_distinct_node_connection() {
    let path = |name: &str| PathBuf::from(name);
    for extra in ["none", "alias", "known", "unknown"] {
        let mut tauri = vec![
            (path("origin/Cargo.toml"), path("origin")),
            (path("uncertain/Cargo.toml"), path("shared")),
            (path("independent/Cargo.toml"), path("independent")),
        ];
        let mut roots = BTreeMap::from([
            (
                path("origin/Cargo.toml"),
                crate::cargo_scope::CargoRoot::Known(path("root-a")),
            ),
            (
                path("uncertain/Cargo.toml"),
                crate::cargo_scope::CargoRoot::Unknown("member lookup failed".into()),
            ),
            (
                path("independent/Cargo.toml"),
                crate::cargo_scope::CargoRoot::Known(path("root-b")),
            ),
        ]);
        if extra == "alias" {
            tauri.push((path("uncertain/Cargo.toml"), path("shared")));
        } else if extra != "none" {
            tauri.push((path("additional/Cargo.toml"), path("shared")));
            roots.insert(
                path("additional/Cargo.toml"),
                if extra == "known" {
                    crate::cargo_scope::CargoRoot::Known(path("root-b"))
                } else {
                    crate::cargo_scope::CargoRoot::Unknown("second member lookup failed".into())
                },
            );
        }
        let reached = ScopeGraph::build(&tauri, &roots).reach(Path::new("origin"));
        assert!(reached.contains_key(&ScopeVertex::Node(path("shared"))));
        assert_eq!(
            reached.contains_key(&ScopeVertex::Root(path("root-b"))),
            matches!(extra, "known" | "unknown")
        );
        assert_eq!(
            reached.contains_key(&ScopeVertex::Node(path("independent"))),
            matches!(extra, "known" | "unknown")
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn stopped_unknown_origin_reaches_all_node_aliases_of_its_same_counterpart() {
    let fixture = MembershipFixture::new();
    let alias = fixture.temp.path().join("alias");
    write_project(&alias);
    lock(&alias);
    std::os::unix::fs::symlink(fixture.companion.join("src-tauri"), alias.join("src-tauri"))
        .unwrap();
    let mut manifests = fixture.manifests();
    manifests.extend(detect_manifests(&alias));
    let mut resolver = fixture.resolver(None);
    resolver.fail(&fixture.companion.join("src-tauri/Cargo.toml"));
    let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&fixture.companion).unwrap(),
        violation(&orchestrator),
    );
    let before = fixture.snapshot(&[
        &fixture.app,
        &fixture.companion,
        &fixture.helper,
        &fixture.other,
        &alias,
    ]);
    let result = orchestrator
        .process_manifests_with_auditor(&manifests, &mut Progress::new(false), &auditor, &resolver)
        .await;
    assert_unchanged(before);
    assert_eq!(result.summary.total_failed(), 16);
    assert_eq!(result.summary.total_updates(), 0);
    assert!(
        stopped_node_reason(&result, &alias)
            .unwrap()
            .contains("simulated member toolchain failure")
    );
    let directories = Some(vec![
        fixture.app.clone(),
        fixture.companion.clone(),
        fixture.helper.clone(),
        fixture.other.clone(),
        alias,
    ]);
    for only in [false, true] {
        let plan = InstallPlan::new_scoped(
            &orchestrator,
            &result,
            &directories,
            fixture.temp.path(),
            only,
        );
        assert!(plan.jobs.is_empty());
        assert!(plan.node_audits.is_empty());
    }
    resolver.assert_cached();
}

#[tokio::test]
async fn normal_install_does_not_redirect_independent_cargo_updates_to_stopped_tauri() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_tauri(root, "app");
    write_rust(root, "independent", false);
    let manifest = root.join("Cargo.toml");
    let mut content = std::fs::read_to_string(&manifest).unwrap();
    content.push_str("[workspace]\nexclude=['src-tauri']\nresolver='3'\n");
    std::fs::write(&manifest, content).unwrap();
    let before_node = std::fs::read(root.join("package.json")).unwrap();
    let before_tauri = std::fs::read(root.join("src-tauri/Cargo.toml")).unwrap();
    let orchestrator = tauri_orchestrator(root, &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(root).unwrap(),
        violation(&orchestrator),
    );
    let result = run(&orchestrator, &[root], &auditor).await;
    assert_eq!(
        std::fs::read(root.join("package.json")).unwrap(),
        before_node
    );
    assert_eq!(
        std::fs::read(root.join("src-tauri/Cargo.toml")).unwrap(),
        before_tauri
    );
    assert!(
        std::fs::read_to_string(&manifest)
            .unwrap()
            .contains("example = '2.0.0'")
    );
    assert_eq!(result.summary.total_failed(), 4);
    assert_eq!(result.summary.total_updates(), 1);
    let pm = SystemPackageManager::new();
    for only in [false, true] {
        let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, root, only);
        assert_eq!(plan.jobs.len(), 1);
        assert!(plan.node_audits.is_empty());
        let job = &plan.jobs[0];
        let scope = job.scope.as_ref().unwrap();
        let (effective_dir, manager) = pm
            .resolve_scoped_package_manager(job.language, &job.directory, scope)
            .unwrap();
        assert_eq!(
            std::fs::canonicalize(effective_dir).unwrap(),
            std::fs::canonicalize(root).unwrap()
        );
        assert_eq!(manager, "cargo");
        let args = pm
            .scoped_install_args(manager, job.policy.as_ref().unwrap(), scope)
            .unwrap();
        assert_eq!(
            args,
            if only {
                vec!["cargo", "update", "--workspace"]
            } else {
                vec!["cargo", "update"]
            }
        );
    }
}

#[tokio::test]
async fn normal_full_install_keeps_tauri_redirect_when_preflight_does_not_stop_a_scope() {
    let temp = tempfile::tempdir().unwrap();
    write_tauri(temp.path(), "app");
    write_rust(temp.path(), "independent", false);
    let manifest = temp.path().join("Cargo.toml");
    let mut content = std::fs::read_to_string(&manifest).unwrap();
    content.push_str("[workspace]\nexclude=['src-tauri']\nresolver='3'\n");
    std::fs::write(manifest, content).unwrap();
    let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
    let result = run(&orchestrator, &[temp.path()], &FakeAuditor::default()).await;
    assert!(stopped_node_reason(&result, temp.path()).is_none());
    let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, temp.path(), false);
    let job = plan
        .jobs
        .iter()
        .find(|job| job.language == Language::Rust)
        .unwrap();
    assert_eq!(job.scope, Ok(crate::package_manager::InstallScope::Full));
    let pm = SystemPackageManager::new();
    let (directory, _) = pm
        .resolve_scoped_package_manager(job.language, &job.directory, job.scope.as_ref().unwrap())
        .unwrap();
    assert_eq!(directory, temp.path().join("src-tauri"));
    assert_eq!(
        pm.scoped_install_args(
            "cargo",
            job.policy.as_ref().unwrap(),
            job.scope.as_ref().unwrap()
        )
        .unwrap(),
        ["cargo", "update"]
    );
}

fn rust_lock(directory: &Path, name: &str) {
    std::fs::write(directory.join("Cargo.lock"), format!("version = 3\n[[package]]\nname = '{name}'\nversion = '1.0.1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n")).unwrap();
}

#[tokio::test]
async fn stopped_cargo_locks_and_missing_locks_are_not_reintroduced_by_post_install_audit() {
    for existing in [false, true] {
        let fixture = MembershipFixture::new();
        // A confirmed independent root without updates must retain its audit.
        fixture.unchanged(&fixture.other);
        if existing {
            rust_lock(&fixture.workspace, "young-child");
        }
        rust_lock(&fixture.other.join("src-tauri"), "independent-child");
        let stopped_lock = fixture.workspace.join("Cargo.lock");
        let before = existing.then(|| std::fs::read(&stopped_lock).unwrap());
        let orchestrator = tauri_orchestrator(fixture.temp.path(), &["--install"]).await;
        let auditor = FakeAuditor::default();
        auditor.results.lock().unwrap().insert(
            std::fs::canonicalize(&fixture.app).unwrap(),
            violation(&orchestrator),
        );
        let result = run(
            &orchestrator,
            &[
                &fixture.app,
                &fixture.companion,
                &fixture.helper,
                &fixture.other,
            ],
            &auditor,
        )
        .await;
        let stops: Vec<_> = result
            .errors
            .iter()
            .filter_map(|error| match error {
                OrchestratorError::CargoPreflightError { manifest, root, .. } => {
                    Some((manifest, root))
                }
                _ => None,
            })
            .collect();
        assert_eq!(stops.len(), 3);
        let stopped_root = std::fs::canonicalize(&fixture.workspace).unwrap();
        assert!(
            stops
                .iter()
                .all(|(_, root)| root.as_ref() == Some(&stopped_root))
        );
        for only in [false, true] {
            let plan =
                InstallPlan::new_scoped(&orchestrator, &result, &None, fixture.temp.path(), only);
            assert!(plan.jobs.is_empty());
            let audit = plan.rust_audit_plan(&orchestrator, &result);
            assert_eq!(audit.locks.len(), 1);
            assert_eq!(
                std::fs::canonicalize(&audit.locks[0].directory).unwrap(),
                std::fs::canonicalize(fixture.other.join("src-tauri")).unwrap()
            );
            assert!(audit.missing_lock_dirs.is_empty());
        }
        if let Some(before) = before {
            assert_eq!(std::fs::read(stopped_lock).unwrap(), before);
        }
    }
}

#[tokio::test]
async fn independent_full_cargo_install_associates_only_its_actual_new_lock_and_keeps_php_full() {
    for monorepo in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        write_tauri(temp.path(), "app");
        write_rust(temp.path(), "independent", false);
        let path = temp.path().join("Cargo.toml");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("[workspace]\nexclude=['src-tauri']\nresolver='3'\n");
        std::fs::write(path, content).unwrap();
        std::fs::write(
            temp.path().join("composer.json"),
            r#"{"require":{"vendor/example":"^1.0.0"}}"#,
        )
        .unwrap();
        let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
        orchestrator.versions.cache.lock().await.insert(
            (Language::Php, "vendor/example".into()),
            vec![VersionInfo::new(
                "2.0.0",
                orchestrator.evaluated_at - chrono::Duration::days(60),
            )],
        );
        let auditor = FakeAuditor::default();
        auditor.results.lock().unwrap().insert(
            std::fs::canonicalize(temp.path()).unwrap(),
            violation(&orchestrator),
        );
        let result = run(&orchestrator, &[temp.path()], &auditor).await;
        let dirs = monorepo.then(|| vec![temp.path().to_path_buf()]);
        let plan = InstallPlan::new_scoped(&orchestrator, &result, &dirs, temp.path(), false);
        assert_eq!(plan.jobs.len(), 2);
        assert_eq!(
            plan.jobs
                .iter()
                .find(|job| job.language == Language::Rust)
                .unwrap()
                .scope,
            Ok(crate::package_manager::InstallScope::CargoFull)
        );
        assert_eq!(
            plan.jobs
                .iter()
                .find(|job| job.language == Language::Php)
                .unwrap()
                .scope,
            Ok(crate::package_manager::InstallScope::Full)
        );
        let missing = plan.rust_audit_plan(&orchestrator, &result);
        assert_eq!(missing.missing_lock_dirs.len(), 1);
        assert_eq!(
            std::fs::canonicalize(&missing.missing_lock_dirs[0]).unwrap(),
            std::fs::canonicalize(temp.path()).unwrap()
        );
        // Simulate only the new lock file that this confirmed install can create.
        rust_lock(temp.path(), "example");
        let audit = plan.rust_audit_plan(&orchestrator, &result);
        assert_eq!(audit.locks.len(), 1);
        assert_eq!(
            std::fs::canonicalize(&audit.locks[0].directory).unwrap(),
            std::fs::canonicalize(temp.path()).unwrap()
        );
        assert!(audit.locks[0].baseline.is_empty());
        assert!(audit.missing_lock_dirs.is_empty());
        assert!(!temp.path().join("src-tauri/Cargo.lock").exists());
    }
}

#[tokio::test]
async fn failed_full_cargo_root_preparation_is_an_error_and_never_restores_full_redirect() {
    let temp = tempfile::tempdir().unwrap();
    write_tauri(temp.path(), "app");
    write_rust(temp.path(), "independent", false);
    let path = temp.path().join("Cargo.toml");
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str("[workspace]\nexclude=['src-tauri']\nresolver='3'\n");
    std::fs::write(&path, content).unwrap();
    let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(temp.path()).unwrap(),
        violation(&orchestrator),
    );
    let result = run(&orchestrator, &[temp.path()], &auditor).await;
    std::fs::write(path, "invalid Cargo manifest").unwrap();
    for only in [false, true] {
        let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, temp.path(), only);
        assert_eq!(plan.jobs.len(), 1);
        assert!(
            plan.jobs[0]
                .scope
                .as_ref()
                .unwrap_err()
                .contains("Cannot locate Cargo workspace")
        );
        assert!(
            plan.rust_audit_plan(&orchestrator, &result)
                .missing_lock_dirs
                .is_empty()
        );
    }
}

struct MixedCargoResolver {
    workspace: PathBuf,
    failed: PathBuf,
    independent: PathBuf,
    recheck: Result<PathBuf, String>,
    calls: Mutex<Vec<(Option<PathBuf>, PathBuf)>>,
}

fn located(root: PathBuf, manifest: &Path) -> LocatedCargoRoot {
    LocatedCargoRoot {
        root,
        anchor: CargoAnchor {
            cwd: manifest.parent().unwrap().to_path_buf(),
            program: PathBuf::from("cargo"),
        },
    }
}

impl CargoScopeResolver for MixedCargoResolver {
    fn resolve(&self, manifest: &Path, _boundary: &Path) -> Result<LocatedCargoRoot, String> {
        let manifest = std::fs::canonicalize(manifest).unwrap();
        self.calls.lock().unwrap().push((None, manifest.clone()));
        if manifest == self.failed {
            Err("Cannot locate Cargo workspace: simulated member toolchain failure".into())
        } else if manifest.starts_with(&self.workspace) {
            Ok(located(self.workspace.clone(), &manifest))
        } else {
            Ok(located(self.independent.join("src-tauri"), &manifest))
        }
    }

    fn resolve_from(
        &self,
        anchor: &CargoAnchor,
        manifest: &Path,
        _boundary: &Path,
    ) -> Result<PathBuf, String> {
        self.calls
            .lock()
            .unwrap()
            .push((Some(anchor.cwd.clone()), manifest.to_path_buf()));
        self.recheck.clone()
    }
}

#[tokio::test]
async fn mixed_primary_failure_on_stopped_tauri_blocks_successful_shared_members() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let app = workspace.join("app");
    let companion = workspace.join("companion");
    let helper = workspace.join("helper");
    let other = temp.path().join("other");
    write_tauri(&app, "app");
    write_tauri(&companion, "companion");
    write_rust(&helper, "helper", false);
    write_tauri(&other, "other");
    std::fs::write(workspace.join("Cargo.toml"), "[workspace]\nmembers = ['app/src-tauri', 'companion/src-tauri', 'helper']\nresolver = '3'\n").unwrap();
    let paths = [
        app.join("package.json"),
        app.join("src-tauri/Cargo.toml"),
        companion.join("package.json"),
        companion.join("src-tauri/Cargo.toml"),
        helper.join("Cargo.toml"),
        app.join("pnpm-lock.yaml"),
        companion.join("pnpm-lock.yaml"),
    ];
    let originals: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    auditor.results.lock().unwrap().insert(
        std::fs::canonicalize(&app).unwrap(),
        violation(&orchestrator),
    );
    let resolver = MixedCargoResolver {
        workspace: std::fs::canonicalize(&workspace).unwrap(),
        failed: std::fs::canonicalize(app.join("src-tauri/Cargo.toml")).unwrap(),
        independent: std::fs::canonicalize(&other).unwrap(),
        recheck: Ok(std::fs::canonicalize(&workspace).unwrap()),
        calls: Mutex::default(),
    };
    let manifests: Vec<_> = [&app, &companion, &helper, &other]
        .into_iter()
        .flat_map(|dir| detect_manifests(dir))
        .collect();
    let result = orchestrator
        .process_manifests_with_auditor(&manifests, &mut Progress::new(false), &auditor, &resolver)
        .await;
    for (path, before) in paths.iter().zip(originals) {
        assert_eq!(std::fs::read(path).unwrap(), before, "{}", path.display());
    }
    assert_eq!(result.summary.total_failed(), 9);
    assert_eq!(result.summary.total_updates(), 4);
}

#[tokio::test]
async fn mixed_primary_failure_on_updated_member_enters_failed_node_policy_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let app = workspace.join("app");
    let helper = workspace.join("helper");
    let other = temp.path().join("other");
    write_project(&app);
    lock(&app);
    write_rust(&app.join("src-tauri"), "app", false);
    let counterpart = app.join("src-tauri/Cargo.toml");
    let content = std::fs::read_to_string(&counterpart)
        .unwrap()
        .replace("example = '1.0.0'", "example = '2.0.0'");
    std::fs::write(&counterpart, content).unwrap();
    std::fs::write(app.join(".npmrc"), "minimum-release-age=invalid\n").unwrap();
    write_rust(&helper, "helper", false);
    write_tauri(&other, "other");
    std::fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers = ['app/src-tauri', 'helper']\nresolver = '3'\n",
    )
    .unwrap();
    let before = std::fs::read(helper.join("Cargo.toml")).unwrap();
    let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
    let auditor = FakeAuditor::default();
    let resolver = MixedCargoResolver {
        workspace: std::fs::canonicalize(&workspace).unwrap(),
        failed: std::fs::canonicalize(helper.join("Cargo.toml")).unwrap(),
        independent: std::fs::canonicalize(&other).unwrap(),
        recheck: Ok(std::fs::canonicalize(&workspace).unwrap()),
        calls: Mutex::default(),
    };
    let manifests: Vec<_> = [&app, &helper, &other]
        .into_iter()
        .flat_map(|dir| detect_manifests(dir))
        .collect();
    let result = orchestrator
        .process_manifests_with_auditor(&manifests, &mut Progress::new(false), &auditor, &resolver)
        .await;
    assert_eq!(std::fs::read(helper.join("Cargo.toml")).unwrap(), before);
    assert_eq!(result.summary.total_failed(), 1);
    assert_eq!(result.summary.total_updates(), 4);
}

impl CargoScopeResolver for UnavailableCargoResolver {
    fn resolve(&self, manifest: &Path, _boundary: &Path) -> Result<LocatedCargoRoot, String> {
        let manifest = std::fs::canonicalize(manifest).unwrap();
        if manifest.starts_with(&self.independent) {
            Ok(located(self.independent.join("src-tauri"), &manifest))
        } else {
            Err("Cannot locate Cargo executable: simulated missing command".into())
        }
    }

    fn resolve_from(
        &self,
        _anchor: &CargoAnchor,
        _manifest: &Path,
        _boundary: &Path,
    ) -> Result<PathBuf, String> {
        self.recovered
            .clone()
            .ok_or_else(|| "simulated recheck unavailable".into())
    }
}

#[tokio::test]
async fn unavailable_cargo_scopes_block_unverified_rust_and_node_but_keep_confirmed_independent_roots()
 {
    for recovered in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let app = workspace.join("app");
        let companion = workspace.join("companion");
        let helper = workspace.join("helper");
        let other = temp.path().join("other");
        write_tauri(&app, "app");
        write_tauri(&companion, "companion");
        write_rust(&helper, "helper", false);
        write_tauri(&other, "other");
        std::fs::write(workspace.join("Cargo.toml"), "[workspace]\nmembers = ['app/src-tauri', 'companion/src-tauri', 'helper']\nresolver = '3'\n").unwrap();
        let originals: Vec<_> = [
            app.join("package.json"),
            app.join("src-tauri/Cargo.toml"),
            companion.join("package.json"),
            companion.join("src-tauri/Cargo.toml"),
            helper.join("Cargo.toml"),
        ]
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
        let orchestrator = tauri_orchestrator(temp.path(), &["--install"]).await;
        let auditor = FakeAuditor::default();
        auditor.results.lock().unwrap().insert(
            std::fs::canonicalize(&app).unwrap(),
            violation(&orchestrator),
        );
        let resolver = UnavailableCargoResolver {
            independent: std::fs::canonicalize(&other).unwrap(),
            recovered: recovered.then(|| std::fs::canonicalize(&workspace).unwrap()),
        };
        let manifests: Vec<_> = [&companion, &app, &helper, &other]
            .into_iter()
            .flat_map(|dir| detect_manifests(dir))
            .collect();
        let result = orchestrator
            .process_manifests_with_auditor(
                &manifests,
                &mut Progress::new(false),
                &auditor,
                &resolver,
            )
            .await;
        for (path, bytes) in originals {
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", path.display());
        }
        assert_eq!(
            result.summary.total_failed(),
            if recovered { 9 } else { 13 }
        );
        assert_eq!(
            result.summary.total_updates(),
            if recovered { 4 } else { 0 }
        );
        let companion_scope = std::fs::canonicalize(&companion).unwrap();
        let messages: Vec<_> = result
            .errors
            .iter()
            .filter_map(|error| match error {
                crate::orchestrator::OrchestratorError::NodePreflightError {
                    directory,
                    message,
                } if directory == &companion_scope => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(messages.len(), 1);
        if recovered {
            assert!(messages[0].contains("shares a synchronized Tauri install"));
        } else {
            assert!(messages[0].contains("could not be confirmed as independent"));
            assert!(messages[0].contains("simulated missing command"));
            assert!(messages[0].contains("simulated recheck unavailable"));
            assert!(!messages[0].contains("shares a synchronized Tauri install"));
        }
        for only in [false, true] {
            let dirs = Some(vec![
                app.clone(),
                companion.clone(),
                helper.clone(),
                other.clone(),
            ]);
            let plan = InstallPlan::new_scoped(&orchestrator, &result, &dirs, temp.path(), only);
            assert!(plan.jobs.iter().all(|job| {
                std::fs::canonicalize(&job.directory)
                    .unwrap()
                    .starts_with(&resolver.independent)
            }));
            assert_eq!(plan.node_audits.len(), usize::from(recovered));
            if recovered {
                assert_eq!(plan.node_audits[0].directory, resolver.independent);
            } else {
                assert!(plan.jobs.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn tauri_preflight_stops_both_sides_and_shared_cargo_members_but_keeps_independent_roots() {
    for unchanged_companion in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let app = workspace.join("app");
        let helper = workspace.join("helper");
        let companion = workspace.join("companion");
        let excluded = workspace.join("excluded");
        let other = temp.path().join("other");
        write_tauri(&app, "app");
        write_tauri(&companion, "companion");
        if unchanged_companion {
            std::fs::write(
                companion.join("package.json"),
                r#"{"dependencies":{"@tauri-apps/api":"^2.3.0","example":"^2.0.0"}}"#,
            )
            .unwrap();
            let path = companion.join("src-tauri/Cargo.toml");
            let content = std::fs::read_to_string(&path)
                .unwrap()
                .replace("example = '1.0.0'", "example = '2.0.0'")
                .replace("tauri = '2.0.0'", "tauri = '2.3.0'");
            std::fs::write(path, content).unwrap();
        }
        write_tauri(&other, "other");
        write_rust(&helper, "helper", false);
        write_rust(&excluded, "excluded", false);
        std::fs::write(workspace.join("Cargo.toml"), "[workspace]\nmembers = ['app/src-tauri', 'helper', 'companion/src-tauri']\nexclude = ['excluded']\nresolver = '3'\n").unwrap();
        let originals: Vec<_> = [
            app.join("package.json"),
            app.join("src-tauri/Cargo.toml"),
            helper.join("Cargo.toml"),
            workspace.join("Cargo.toml"),
            companion.join("package.json"),
            companion.join("src-tauri/Cargo.toml"),
        ]
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
        let orchestrator = orchestrator(temp.path(), &["--install"]).await;
        {
            let mut cache = orchestrator.versions.cache.lock().await;
            let date = orchestrator.evaluated_at - chrono::Duration::days(60);
            cache.insert(
                (Language::Rust, "example".into()),
                vec![VersionInfo::new("2.0.0", date)],
            );
            cache.insert(
                (Language::Rust, "tauri".into()),
                vec![VersionInfo::new("2.2.0", date)],
            );
            for name in crate::tauri_sync::TAURI_NPM_PACKAGES {
                cache.insert(
                    (Language::Node, (*name).into()),
                    vec![
                        VersionInfo::new("2.2.0", date),
                        VersionInfo::new("2.3.0", date),
                    ],
                );
            }
        }
        let auditor = FakeAuditor::default();
        auditor.results.lock().unwrap().insert(
            std::fs::canonicalize(&app).unwrap(),
            violation(&orchestrator),
        );
        let result = run(
            &orchestrator,
            &[&companion, &app, &helper, &excluded, &other],
            &auditor,
        )
        .await;
        for (path, bytes) in originals {
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", path.display());
        }
        assert_eq!(
            result.summary.total_failed(),
            if unchanged_companion { 5 } else { 9 }
        );
        let failed_api = result
            .summary
            .manifests
            .iter()
            .find(|manifest| manifest.path == app.join("package.json"))
            .unwrap()
            .failed_updates()
            .find(|update| update.package_name() == "@tauri-apps/api")
            .unwrap();
        // Node's judge chose 2.3.0; synchronization already chose the common 2.2.0 before the preflight stopped it.
        assert!(
            matches!(failed_api, crate::domain::UpdateResult::Update { new_version, .. } if new_version == "2.2.0")
        );
        assert!(
            std::fs::read_to_string(excluded.join("Cargo.toml"))
                .unwrap()
                .contains("example = '2.0.0'")
        );
        assert!(
            std::fs::read_to_string(other.join("package.json"))
                .unwrap()
                .contains("^2.2.0")
        );
        assert!(
            std::fs::read_to_string(other.join("src-tauri/Cargo.toml"))
                .unwrap()
                .contains("tauri = '2.2.0'")
        );
        let dirs = Some(vec![
            workspace.clone(),
            app.clone(),
            helper.clone(),
            companion.clone(),
            excluded.clone(),
            other.clone(),
        ]);
        for only in [false, true] {
            let plan = InstallPlan::new_scoped(&orchestrator, &result, &dirs, temp.path(), only);
            assert!(plan.jobs.iter().all(|job| job.directory != workspace
                && job.directory != app
                && job.directory != helper
                && job.directory != companion));
            assert!(
                plan.node_audits
                    .iter()
                    .all(|job| job.directory != std::fs::canonicalize(&companion).unwrap())
            );
            assert!(result.errors.iter().any(|error| matches!(error,
            crate::orchestrator::OrchestratorError::NodePreflightError { directory, message }
            if directory == &std::fs::canonicalize(&companion).unwrap() && message.contains("shares a synchronized Tauri install"))));
            assert!(plan.jobs.iter().any(|job| job.language == Language::Rust
                && std::fs::canonicalize(&job.directory).unwrap()
                    == std::fs::canonicalize(&excluded).unwrap()));
            assert!(plan.jobs.iter().any(|job| {
                job.language == Language::Rust
                    && std::fs::canonicalize(&job.directory)
                        .unwrap()
                        .starts_with(std::fs::canonicalize(&other).unwrap())
            }));
        }
    }
}
