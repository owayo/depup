use super::*;
use crate::domain::{ManifestUpdateResult, UpdateSummary};
use clap::Parser;
use std::sync::Mutex;

const SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

fn lock(version: &str) -> String {
    format!(
        "version = 4\n[[package]]\nname = 'example'\nversion = '{version}'\nsource = '{SOURCE}'\n"
    )
}

fn args(directory: &Path) -> CliArgs {
    let mut args = CliArgs::parse_from([
        "depup",
        "--install",
        "--refresh-lock",
        "--no-age",
        "--no-osv",
        "--quiet",
    ]);
    args.path = directory.into();
    args
}

fn fixture(root: &Path, languages: &[(PathBuf, Language)]) -> (OrchestratorResult, InstallPlan) {
    let mut result = OrchestratorResult {
        summary: UpdateSummary::new(false),
        write_results: Vec::new(),
        errors: Vec::new(),
    };
    let jobs = languages
        .iter()
        .map(|(directory, language)| {
            std::fs::create_dir_all(directory).unwrap();
            let manifest = directory.join(if *language == Language::Rust {
                "Cargo.toml"
            } else {
                "package.json"
            });
            std::fs::write(
                &manifest,
                if *language == Language::Rust {
                    "[package]\nname='app'\nversion='0.1.0'\n"
                } else {
                    "{}"
                },
            )
            .unwrap();
            std::fs::write(
                directory.join(if *language == Language::Rust {
                    "Cargo.lock"
                } else {
                    "pnpm-lock.yaml"
                }),
                lock("1.0.0"),
            )
            .unwrap();
            result
                .summary
                .add_manifest(ManifestUpdateResult::new(&manifest, *language));
            InstallJob {
                directory: directory.clone(),
                language: *language,
                manifests: vec![manifest],
                scope: Ok(InstallScope::Refresh),
                policy: Ok(AgePolicy {
                    min_age: None,
                    evaluated_at: chrono::Utc::now(),
                    exemptions: Default::default(),
                }),
            }
        })
        .collect();
    let _ = root;
    (
        result,
        InstallPlan {
            jobs,
            wrapper_updates: Vec::new(),
            node_audits: Vec::new(),
            baselines: BTreeMap::new(),
        },
    )
}

#[derive(Default)]
struct FakeVerification {
    node_version: Option<String>,
    seen: Mutex<Vec<String>>,
    osv_error: Option<String>,
    age_error: bool,
    rollback: Option<String>,
    external: Option<PathBuf>,
}

#[async_trait(?Send)]
impl Verification for FakeVerification {
    async fn snapshot(&self, job: &InstallJob) -> Result<Snapshot, String> {
        let content = std::fs::read_to_string(lock_path(job)).map_err(|error| error.to_string())?;
        let packages = crate::manifest::parse_registry_entries(&content)
            .into_iter()
            .flat_map(|(name, versions)| {
                versions
                    .into_iter()
                    .map(move |version| (name.clone(), version))
            })
            .collect();
        let protected = parse_lock_packages(&content)
            .into_iter()
            .filter(|package| {
                package
                    .source
                    .as_deref()
                    .is_some_and(|source| !is_crates_io_source(source))
            })
            .map(|package| {
                serde_json::json!([package.name, package.version, package.source]).to_string()
            })
            .collect();
        Ok(Snapshot {
            bytes: Some(content.into_bytes()),
            packages,
            protected,
            node_direct_edges: self
                .node_version
                .as_ref()
                .map_or_else(BTreeMap::new, |version| {
                    BTreeMap::from([(
                        job.directory.canonicalize().unwrap(),
                        BTreeMap::from([("example".into(), ("example".into(), version.clone()))]),
                    )])
                }),
            ..Default::default()
        })
    }
    async fn cargo_age(
        &self,
        job: &InstallJob,
        _: &RegistryLockEntries,
        _: &PreferredVersions,
        _: &Progress,
    ) -> (LockAgeAuditResult, Option<AuditReceipt>) {
        if let Some(version) = &self.rollback {
            let path = lock_path(job);
            let before = std::fs::read(&path).unwrap();
            let after = lock(version).into_bytes();
            std::fs::write(&path, &after).unwrap();
            return (
                LockAgeAuditResult::default(),
                Some(AuditReceipt { before, after }),
            );
        }
        (
            LockAgeAuditResult {
                problems: if self.age_error {
                    vec!["release date unavailable".into()]
                } else {
                    Vec::new()
                },
                ..Default::default()
            },
            None,
        )
    }
    async fn node_age(&self, _: &InstallJob, _: &Progress) -> Result<(), String> {
        if self.age_error {
            Err("young lock version".into())
        } else {
            Ok(())
        }
    }
    async fn osv(&self, _: Language, name: &str, version: &str) -> Result<(), String> {
        self.seen.lock().unwrap().push(format!("{name}@{version}"));
        if let Some(path) = &self.external {
            std::fs::write(path, "external edit").unwrap();
        }
        self.osv_error.clone().map_or(Ok(()), Err)
    }
}

#[tokio::test]
async fn pnpm_refresh_rolls_back_failed_direct_targets_and_accepts_newer_resolution() {
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use crate::manifest::{ManifestWriter, PackageJsonParser};
    for version in [None, Some("1.1.0"), Some("1.3.0")] {
        let temp = tempfile::tempdir().unwrap();
        let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Node)]);
        let path = temp.path().join("package.json");
        let original = r#"{"dependencies":{"example":"^1.0.0"}}"#;
        std::fs::write(&path, original).unwrap();
        result.summary.manifests[0].add_result(UpdateResult::update(
            Dependency::new(
                "example",
                VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0"),
                false,
                Language::Node,
            ),
            "1.2.0",
        ));
        result.write_results.push(
            ManifestWriter::new(false)
                .apply_updates(&result.summary.manifests[0], &PackageJsonParser)
                .unwrap(),
        );
        let args = args(temp.path());
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let verifier = FakeVerification {
            node_version: version.map(str::to_string),
            ..Default::default()
        };
        let mut calls = 0;
        let outcome = run_with(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            &verifier,
            |command| {
                calls += 1;
                resolved(command)
            },
        )
        .await;
        assert!(!outcome.install_failed);
        if version == Some("1.3.0") {
            assert!(!outcome.audit_failed);
            assert_eq!(calls, 2);
            assert_eq!(result.summary.total_updates(), 1);
        } else {
            assert!(outcome.audit_failed);
            assert_eq!(calls, 1);
            assert_eq!(result.summary.total_updates(), 0);
            assert_eq!(result.summary.total_failed(), 1);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(
                std::fs::read_to_string(temp.path().join("pnpm-lock.yaml")).unwrap(),
                lock("1.0.0")
            );
            assert!(
                result.summary.lock_refreshes[0]
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("target 1.2.0")
            );
        }
    }
}

fn resolved(command: &PreparedInstall) -> InstallResult {
    if command.argv[1] == "update" {
        std::fs::write(
            command
                .directory
                .join(if command.language == Language::Rust {
                    "Cargo.lock"
                } else {
                    "pnpm-lock.yaml"
                }),
            lock("1.2.0"),
        )
        .unwrap();
    }
    InstallResult::success(
        command.language,
        command.argv.join(" "),
        String::new(),
        String::new(),
    )
}

#[tokio::test]
async fn direct_zero_refresh_runs_once_and_materializes_only_verified_pnpm() {
    let temp = tempfile::tempdir().unwrap();
    let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Node)]);
    let args = args(temp.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let mut commands = Vec::new();
    let verifier = FakeVerification::default();
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &verifier,
        |command| {
            commands.push(command.argv.clone());
            resolved(command)
        },
    )
    .await;
    assert!(!outcome.install_failed && !outcome.audit_failed);
    assert_eq!(commands.len(), 2);
    assert!(commands[0].contains(&"--lockfile-only".into()));
    assert!(commands[0].contains(&"--ignore-scripts".into()));
    assert!(commands[1].contains(&"--frozen-lockfile".into()));
    assert!(!commands[1].contains(&"--ignore-scripts".into()));
    assert_eq!(verifier.seen.lock().unwrap().as_slice(), ["example@1.2.0"]);
    assert_eq!(result.summary.total_updates(), 0);
    assert_eq!(result.summary.lock_refreshes[0].status, "completed");
    assert!(result.summary.lock_refreshes[0].changed);
    let mut json = Vec::new();
    crate::output::create_formatter(crate::output::OutputConfig::from_cli(
        true, false, false, false, false,
    ))
    .format(&result, &mut json)
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(json["summary"]["updates"], 0);
    assert_eq!(json["lock_refreshes"][0]["status"], "completed");
}

#[tokio::test]
async fn young_vulnerable_and_unverified_versions_restore_without_installing() {
    for (young, error) in [
        (true, None),
        (false, Some("known vulnerability")),
        (false, Some("OSV request failed")),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Node)]);
        let args = args(temp.path());
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let verifier = FakeVerification {
            age_error: young,
            osv_error: error.map(str::to_owned),
            ..Default::default()
        };
        let mut calls = 0;
        let outcome = run_with(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            &verifier,
            |command| {
                calls += 1;
                resolved(command)
            },
        )
        .await;
        assert!(outcome.audit_failed && !outcome.install_failed);
        assert_eq!(calls, 1);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("pnpm-lock.yaml")).unwrap(),
            lock("1.0.0")
        );
    }
}

#[tokio::test]
async fn osv_uses_final_cargo_age_rollback_version_and_restores_on_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Rust)]);
    let args = args(temp.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let verifier = FakeVerification {
        rollback: Some("1.1.0".into()),
        osv_error: Some("known vulnerability".into()),
        ..Default::default()
    };
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &verifier,
        resolved,
    )
    .await;
    assert!(outcome.audit_failed && !outcome.install_failed);
    assert_eq!(verifier.seen.lock().unwrap().as_slice(), ["example@1.1.0"]);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("Cargo.lock")).unwrap(),
        lock("1.0.0")
    );
}

#[tokio::test]
async fn tauri_linked_units_restore_together_and_an_independent_unit_completes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app");
    let rust = root.join("src-tauri");
    let other = temp.path().join("other");
    let (mut result, plan) = fixture(
        temp.path(),
        &[
            (root.clone(), Language::Node),
            (rust.clone(), Language::Rust),
            (other.clone(), Language::Node),
        ],
    );
    let args = args(temp.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &FakeVerification::default(),
        |command| {
            if command.directory == rust {
                InstallResult::failure(
                    command.language,
                    "cargo update".into(),
                    String::new(),
                    "resolver failed".into(),
                )
            } else {
                resolved(command)
            }
        },
    )
    .await;
    assert!(outcome.install_failed);
    assert_eq!(
        std::fs::read_to_string(root.join("pnpm-lock.yaml")).unwrap(),
        lock("1.0.0")
    );
    assert_eq!(
        std::fs::read_to_string(other.join("pnpm-lock.yaml")).unwrap(),
        lock("1.2.0")
    );
}

#[tokio::test]
async fn external_lock_change_during_osv_is_kept_and_reported() {
    let temp = tempfile::tempdir().unwrap();
    let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Rust)]);
    let args = args(temp.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let path = temp.path().join("Cargo.lock");
    let verifier = FakeVerification {
        external: Some(path.clone()),
        ..Default::default()
    };
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &verifier,
        resolved,
    )
    .await;
    assert!(outcome.audit_failed);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external edit");
}

#[test]
fn global_max_change_is_rejected_before_any_work() {
    let mut args = args(Path::new("."));
    args.max_change = Some(crate::domain::ChangeLevel::Patch);
    assert!(args.validate_refresh_lock().is_err());
}

#[tokio::test]
async fn dry_run_never_executes_a_package_manager() {
    let temp = tempfile::tempdir().unwrap();
    let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Node)]);
    let mut args = args(temp.path());
    args.dry_run = true;
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &FakeVerification::default(),
        |_| panic!("dry-run must not execute"),
    )
    .await;
    assert!(!outcome.install_failed && !outcome.audit_failed);
}

#[test]
fn tauri_workspace_inheritance_uses_only_the_apps_root_edge() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("Cargo.toml");
    std::fs::write(&manifest,"[package]\nname='app'\nversion='0.1.0'\n[dependencies]\nui={workspace=true}\n[workspace.dependencies]\nui={package='tauri',version='2'}\n").unwrap();
    let content = format!(
        "version=4\n[[package]]\nname='app'\nversion='0.1.0'\ndependencies=['tauri 2.10.0']\n[[package]]\nname='other'\nversion='0.1.0'\ndependencies=['tauri 1.5.0']\n[[package]]\nname='tauri'\nversion='2.10.0'\nsource='{SOURCE}'\n[[package]]\nname='tauri'\nversion='1.5.0'\nsource='{SOURCE}'\n[[package]]\nname='tauri-build'\nversion='9.0.0'\nsource='{SOURCE}'\n"
    );
    assert_eq!(locked_tauri_version(&manifest, &content).unwrap(), "2.10.0");
    assert!(
        locked_tauri_version(&manifest, &content.replace("name='app'", "name='missing'")).is_err()
    );
}

#[tokio::test]
async fn real_cargo_refresh_keeps_mature_transitives_after_young_versions_are_rolled_back() {
    use crate::cargo_rollback::audit::{LockAgeAudit, LockAgeAuditor};
    use crate::cargo_rollback::scratch::CargoCommand;
    use crate::test_support::{
        fake_crates_io::FakeCratesIo,
        local_registry::{LocalRegistry, TestProject},
    };
    use chrono::TimeZone;
    let registry = LocalRegistry::new();
    registry
        .publish("leaf", "1.0.0", &[])
        .publish("leaf", "1.1.0", &[])
        .publish("leaf", "1.2.0", &[])
        .publish("top", "1.0.0", &[("leaf", "1")]);
    let project = TestProject::new(
        &registry,
        "[package]\nname='app'\nversion='0.1.0'\nedition='2024'\n[dependencies]\ntop='=1.0.0'\n",
    );
    project.cargo_ok(&["generate-lockfile"]);
    project.cargo_ok(&["update", "-p", "leaf", "--precise", "1.0.0"]);
    let old = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let young = chrono::Utc.with_ymd_and_hms(2026, 1, 28, 0, 0, 0).unwrap();
    let dates = FakeCratesIo::new()
        .release("top", "1.0.0", old)
        .release("leaf", "1.0.0", old)
        .release("leaf", "1.1.0", old)
        .release("leaf", "1.2.0", young);
    let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
    for (key, value) in project.cargo_envs() {
        cargo = cargo.env(key, value);
    }
    struct Local<'a> {
        system: SystemVerification<'a>,
        dates: FakeCratesIo,
        cargo: CargoCommand,
        seen: Mutex<Vec<String>>,
    }
    #[async_trait(?Send)]
    impl Verification for Local<'_> {
        async fn snapshot(&self, job: &InstallJob) -> Result<Snapshot, String> {
            self.system.snapshot(job).await
        }
        async fn cargo_age(
            &self,
            job: &InstallJob,
            baseline: &RegistryLockEntries,
            preferred: &PreferredVersions,
            _: &Progress,
        ) -> (LockAgeAuditResult, Option<AuditReceipt>) {
            let policy = job.policy.as_ref().unwrap();
            let input = LockAgeAudit {
                project_dir: &job.directory,
                cutoff: policy.cutoff().unwrap(),
                exemptions: &policy.exemptions,
                baseline,
                preferred,
                adapter: &self.dates,
                cargo: &self.cargo,
                budget: Duration::from_secs(30),
            };
            let versions = crate::registry::versions::VersionFetcher::new(4, 1);
            let mut receipt = None;
            let result = LockAgeAuditor::new(&input, &versions, false)
                .run_tracked(None, &mut receipt)
                .await;
            assert!(
                receipt.is_some(),
                "the real audit must attest its final bytes"
            );
            (result, receipt)
        }
        async fn node_age(&self, _: &InstallJob, _: &Progress) -> Result<(), String> {
            panic!("Cargo-only fixture")
        }
        async fn osv(&self, _: Language, name: &str, version: &str) -> Result<(), String> {
            self.seen.lock().unwrap().push(format!("{name}@{version}"));
            Ok(())
        }
    }
    let args = args(project.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let mut result = OrchestratorResult {
        summary: UpdateSummary::new(false),
        write_results: vec![],
        errors: vec![],
    };
    result.summary.add_manifest(ManifestUpdateResult::new(
        project.path().join("Cargo.toml"),
        Language::Rust,
    ));
    let mut plan = InstallPlan::new(&orchestrator, &result, &None, project.path());
    assert_eq!(plan.jobs.len(), 1);
    plan.jobs[0].policy.as_mut().unwrap().min_age = Some(Duration::from_secs(14 * 86400));
    plan.jobs[0].policy.as_mut().unwrap().evaluated_at =
        chrono::Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap();
    let verifier = Local {
        system: SystemVerification(&orchestrator),
        dates,
        cargo,
        seen: Mutex::new(Vec::new()),
    };
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &verifier,
        |command| {
            project.cargo_ok(&["update"]);
            InstallResult::success(
                command.language,
                "cargo update".into(),
                String::new(),
                String::new(),
            )
        },
    )
    .await;
    assert!(
        !outcome.audit_failed && !outcome.install_failed,
        "{:?}",
        result.summary.lock_refreshes
    );
    assert_eq!(
        read_audit_lock(&project.path().join("Cargo.lock"))
            .unwrap()
            .1["leaf"],
        ["1.1.0"]
    );
    assert_eq!(verifier.seen.lock().unwrap().as_slice(), ["leaf@1.1.0"]);
    assert_eq!(result.summary.total_updates(), 0);
}

#[tokio::test]
async fn relative_failed_updates_and_parse_errors_never_run_refresh() {
    for parse_error in [false, true] {
        let temp = tempfile::tempdir_in(".").unwrap();
        let path = temp.path().join("Cargo.toml");
        let (mut result, mut plan) = fixture(temp.path(), &[(temp.path().into(), Language::Rust)]);
        if parse_error {
            result
                .errors
                .push(crate::orchestrator::OrchestratorError::ManifestParseError {
                    path: path.display().to_string(),
                    message: "invalid declaration".into(),
                });
        } else {
            let dependency = crate::domain::Dependency::new(
                "example",
                crate::domain::VersionSpec::new(
                    crate::domain::VersionSpecKind::Caret,
                    "^1.0.0",
                    "1.0.0",
                ),
                false,
                Language::Rust,
            );
            let mut update = UpdateResult::update(dependency, "1.2.0");
            update.record_write_error("write failed");
            result.summary.manifests[0].results.push(update);
        }
        let directory = temp.path().canonicalize().unwrap();
        let reason = blocked_reason(&result, &directory, Language::Rust)
            .expect("relative failure must block canonical root");
        plan.jobs[0].scope = Err(reason);
        plan.jobs[0].directory = directory;
        let args = args(temp.path());
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let outcome = run_with(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            &FakeVerification::default(),
            |_| panic!("a failed declaration must not be refreshed"),
        )
        .await;
        assert!(outcome.install_failed);
    }
}

#[test]
fn workspace_refresh_plans_one_job_per_lock_and_preserves_stricter_unchanged_members() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers=['a','b']\nresolver='3'\n",
    )
    .unwrap();
    std::fs::write(root.join("Cargo.lock"), "version=4\n").unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    std::fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages: ['a','b']\nminimumReleaseAge: 1440\n",
    )
    .unwrap();
    let mut result = OrchestratorResult {
        summary: UpdateSummary::new(false),
        write_results: vec![],
        errors: vec![],
    };
    for member in ["a", "b"] {
        let directory = root.join(member);
        std::fs::create_dir_all(directory.join("src")).unwrap();
        std::fs::write(directory.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            directory.join("Cargo.toml"),
            format!("[package]\nname='{member}'\nversion='0.1.0'\n"),
        )
        .unwrap();
        std::fs::write(directory.join("package.json"), "{}").unwrap();
    }
    std::fs::write(root.join("b/.npmrc"), "minimum-release-age=4320\n").unwrap();
    // あるメンバー内で起動し、兄弟のメンバー自体は更新対象にしない。
    for (path, language) in [
        (root.join("a/Cargo.toml"), Language::Rust),
        (root.join("a/package.json"), Language::Node),
    ] {
        result
            .summary
            .add_manifest(ManifestUpdateResult::new(path, language));
    }
    let args = args(root);
    let orchestrator = Orchestrator::new(args).unwrap();
    let plan = InstallPlan::new(&orchestrator, &result, &None, root);
    assert_eq!(plan.jobs.len(), 2);
    assert!(
        plan.jobs
            .iter()
            .all(|job| job.directory == root.canonicalize().unwrap())
    );
    let node = plan
        .jobs
        .iter()
        .find(|job| job.language == Language::Node)
        .unwrap();
    assert_eq!(
        node.policy.as_ref().unwrap().min_age,
        Some(Duration::from_secs(4320 * 60))
    );
    let runner = SystemPackageManager::new();
    let command = runner
        .prepare_install(
            Language::Node,
            &node.directory,
            node.policy.as_ref().unwrap(),
            &InstallScope::Refresh,
        )
        .unwrap()
        .unwrap();
    assert!(
        command.argv.contains(&"--recursive".into())
            && command.argv.contains(&"--include-workspace-root".into())
    );
    assert_eq!(command.env.len(), 2);
    assert_eq!(command.env[0].1, command.env[1].1);
    assert!(command.env[0].1.parse::<u64>().unwrap() >= 4320);
}

#[tokio::test]
async fn failed_frozen_install_restores_the_provisional_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Node)]);
    let args = args(temp.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &FakeVerification::default(),
        |command| {
            if command.argv[1] == "install" {
                InstallResult::failure(
                    command.language,
                    "pnpm install".into(),
                    String::new(),
                    "script failed".into(),
                )
            } else {
                resolved(command)
            }
        },
    )
    .await;
    assert!(outcome.install_failed);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("pnpm-lock.yaml")).unwrap(),
        lock("1.0.0")
    );
}

#[test]
fn an_audit_receipt_does_not_adopt_an_edit_after_the_auditors_final_read() {
    let temp = tempfile::tempdir().unwrap();
    let (result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Rust)]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(resolved);
    let path = temp.path().join("Cargo.lock");
    let before = std::fs::read(&path).unwrap();
    let after = lock("1.1.0").into_bytes();
    std::fs::write(&path, "external edit").unwrap();
    assert!(tx.accept_audit_lock(&path, &before, &after).is_err());
    tx.fail_verification(temp.path(), "unverified lock".into());
    let _ = tx.rollback();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external edit");
}

#[tokio::test]
async fn a_changed_private_package_is_rejected_without_a_public_osv_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let (mut result, plan) = fixture(temp.path(), &[(temp.path().into(), Language::Rust)]);
    let private = |version: &str| {
        format!(
            "{}\n[[package]]\nname='example'\nversion='{version}'\nsource='registry+https://registry.example.com/index'\n",
            lock("1.0.0")
        )
    };
    let path = temp.path().join("Cargo.lock");
    std::fs::write(&path, private("1.0.0")).unwrap();
    let args = args(temp.path());
    let orchestrator = Orchestrator::new(args.clone()).unwrap();
    let verifier = FakeVerification::default();
    let outcome = run_with(
        &args,
        &orchestrator,
        &plan,
        &mut result,
        &verifier,
        |command| {
            std::fs::write(&path, private("1.1.0")).unwrap();
            InstallResult::success(
                command.language,
                "cargo update".into(),
                String::new(),
                String::new(),
            )
        },
    )
    .await;
    assert!(outcome.audit_failed && !outcome.install_failed);
    assert!(verifier.seen.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), private("1.0.0"));
}
