use super::*;
use crate::cargo_rollback::audit::{LockAgeAudit, LockAgeAuditor};
use crate::cargo_rollback::scratch::CargoCommand;
use crate::domain::{Dependency, ManifestUpdateResult, UpdateResult, VersionSpec, VersionSpecKind};
use crate::manifest::{
    CargoTomlParser, ManifestParser, ManifestWriter, PackageJsonParser, get_parser,
};
use crate::node_lock::{NodeAgeViolation, NodeUnverifiedDependency};
use crate::test_support::fake_crates_io::FakeCratesIo;
use crate::test_support::local_registry::{LocalRegistry, TestProject};
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

fn old() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

fn young() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap()
}

fn cutoff() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap()
}

/// 偽の crates.io と実物の Cargo で Cargo 監査を行う。Node の結果は指定したものを使う。
struct Fixed {
    dates: FakeCratesIo,
    cargo: CargoCommand,
    node_baseline: NodeBaseline,
    node_audit: Mutex<Option<NodeLockAuditResult>>,
    /// ほかのプロセスが書き換える状況として、監査の直後に Cargo.lock を編集する。
    edit_after_audit: bool,
}

impl Fixed {
    fn node(baseline: NodeBaseline, audit: NodeLockAuditResult) -> Self {
        Self {
            dates: FakeCratesIo::new(),
            cargo: CargoCommand::new(),
            node_baseline: baseline,
            node_audit: Mutex::new(Some(audit)),
            edit_after_audit: false,
        }
    }
}

#[async_trait(?Send)]
impl Checks for Fixed {
    async fn rust_lock(
        &self,
        lock: &RustLockPlan,
        _: Option<&ProgressBar>,
    ) -> (LockAgeAuditResult, Option<AuditReceipt>) {
        let input = LockAgeAudit {
            project_dir: &lock.directory,
            cutoff: cutoff(),
            exemptions: &lock.policy.exemptions,
            baseline: &lock.baseline,
            preferred: &lock.preferred,
            adapter: &self.dates,
            cargo: &self.cargo,
            budget: Duration::from_secs(60),
        };
        let versions = crate::registry::versions::VersionFetcher::new(4, 1);
        let mut receipt = None;
        let result = LockAgeAuditor::new(&input, &versions, false)
            .run_tracked(None, &mut receipt)
            .await;
        if self.edit_after_audit {
            let path = lock.directory.join("Cargo.lock");
            let content = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, format!("{content}# edited after the audit\n")).unwrap();
        }
        (result, receipt)
    }
    async fn node_baseline(&self, _: &Path, _: &str) -> NodeBaseline {
        self.node_baseline.clone()
    }
    async fn node_lock(
        &self,
        _: &Path,
        _: &str,
        _: &AgePolicy,
        _: Option<&ProgressBar>,
    ) -> NodeLockAuditResult {
        self.node_audit.lock().unwrap().take().unwrap_or_default()
    }
    async fn pnpm_targets(&self, _: &Path, _: &mut UpdateSummary) -> Vec<String> {
        Vec::new()
    }
}

fn args(directory: &Path) -> CliArgs {
    let mut args = CliArgs::parse_from(["depup", "--install", "--no-osv", "--quiet"]);
    args.path = directory.into();
    args
}

/// `top` を `=1.0.0` から `=1.1.0` へ更新し、その後 `cargo update` が
/// `top 1.1.0` の要件に合わせて `leaf` を解決する。cutoff より新しいのは `leaf 1.2.0` だけ。
struct RustRun {
    _registry: LocalRegistry,
    project: TestProject,
    manifest: String,
    lock: String,
    result: OrchestratorResult,
    checks: Fixed,
}

fn rust_run(requirement: &str, locked_leaf: &str) -> RustRun {
    let registry = LocalRegistry::new();
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        registry.publish("leaf", version, &[]);
    }
    registry
        .publish("top", "1.0.0", &[("leaf", "^1.0")])
        .publish("top", "1.1.0", &[("leaf", requirement)]);
    let manifest = "[package]\nname = 'app'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\ntop = '=1.0.0'\n";
    let project = TestProject::new(&registry, manifest);
    project.cargo_ok(&["generate-lockfile"]);
    project.cargo_ok(&["update", "-p", "leaf", "--precise", locked_leaf]);
    let lock = project.read("Cargo.lock");
    let top = CargoTomlParser
        .parse(manifest)
        .unwrap()
        .into_iter()
        .find(|dependency| dependency.name == "top")
        .unwrap();
    let mut update = ManifestUpdateResult::new(project.path().join("Cargo.toml"), Language::Rust);
    update.add_result(UpdateResult::update(top, "1.1.0"));
    let write = ManifestWriter::new(false)
        .apply_updates(&update, &CargoTomlParser)
        .unwrap();
    assert!(project.read("Cargo.toml").contains("'=1.1.0'"));
    let mut summary = UpdateSummary::new(false);
    summary.add_manifest(update);
    let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
    for (key, value) in project.cargo_envs() {
        cargo = cargo.env(key, value);
    }
    let dates = FakeCratesIo::new()
        .release("leaf", "1.0.0", old())
        .release("leaf", "1.1.0", old())
        .release("leaf", "1.2.0", young())
        .release("top", "1.0.0", old())
        .release("top", "1.1.0", old());
    RustRun {
        _registry: registry,
        project,
        manifest: manifest.into(),
        lock,
        result: OrchestratorResult {
            summary,
            write_results: vec![write],
            errors: vec![],
        },
        checks: Fixed {
            dates,
            cargo,
            node_baseline: NodeBaseline::Known(BTreeSet::new()),
            node_audit: Mutex::new(None),
            edit_after_audit: false,
        },
    }
}

impl RustRun {
    async fn install(&mut self) -> InstallOutcome {
        let args = args(self.project.path());
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let plan = InstallPlan::new(&orchestrator, &self.result, &None, self.project.path());
        let project = &self.project;
        run_with(
            &args,
            &orchestrator,
            &plan,
            &mut self.result,
            &self.checks,
            |command| {
                if command.language == Language::Rust {
                    project.cargo_ok(&["update"]);
                }
                InstallResult::success(
                    command.language,
                    command.argv.join(" "),
                    String::new(),
                    String::new(),
                )
            },
        )
        .await
    }

    fn canceled(&self) -> bool {
        self.result.summary.manifests[0].results.iter().any(|item| {
            item.write_error()
                .is_some_and(|error| error.contains("lock verification failed; update canceled"))
        })
    }
}

/// Issue #54: 更新したマニフェストが要求するため、この実行で追加したcrateが --age を
/// 満たさずに残る。該当単位を、実行前のマニフェストと lockfile へ戻す。
#[tokio::test]
async fn an_added_version_that_cannot_pass_age_restores_the_unit() {
    let mut run = rust_run("^1.2", "1.0.0");
    let outcome = run.install().await;
    assert!(!outcome.install_failed && outcome.audit_failed);
    assert_eq!(run.project.read("Cargo.toml"), run.manifest);
    assert_eq!(run.project.read("Cargo.lock"), run.lock);
    assert!(run.canceled());
    run.project.cargo_ok(&["update", "--workspace", "--locked"]);
}

/// 復元は Cargo.lock を保持する単位に従い、同じディレクトリの独立した
/// install の更新は保持する。
#[tokio::test]
async fn a_cargo_restore_keeps_an_independent_install_in_the_same_directory() {
    let mut run = rust_run("^1.2", "1.0.0");
    let gemfile = run.project.path().join("Gemfile");
    std::fs::write(
        &gemfile,
        "source \"https://rubygems.org\"\ngem \"example\", \"1.0.0\"\n",
    )
    .unwrap();
    let mut manifest = ManifestUpdateResult::new(&gemfile, Language::Ruby);
    manifest.add_result(UpdateResult::update(
        Dependency::new(
            "example",
            VersionSpec::new(VersionSpecKind::Exact, "1.0.0", "1.0.0"),
            false,
            Language::Ruby,
        ),
        "2.0.0",
    ));
    let write = ManifestWriter::new(false)
        .apply_updates(&manifest, get_parser(Language::Ruby).as_ref())
        .unwrap();
    run.result.write_results.push(write);
    run.result.summary.add_manifest(manifest);
    let outcome = run.install().await;
    assert!(!outcome.install_failed && outcome.audit_failed);
    assert_eq!(run.project.read("Cargo.toml"), run.manifest);
    assert_eq!(run.project.read("Cargo.lock"), run.lock);
    assert!(run.project.read("Gemfile").contains("\"2.0.0\""));
}

#[tokio::test]
async fn a_lockfile_that_the_audit_brings_within_age_is_kept() {
    let mut run = rust_run("^1.1", "1.0.0");
    let outcome = run.install().await;
    assert!(!outcome.install_failed && !outcome.audit_failed);
    assert!(run.project.read("Cargo.toml").contains("'=1.1.0'"));
    assert_eq!(run.project.locked_versions("leaf"), ["1.1.0"]);
    assert!(!run.canceled());
    run.project.cargo_ok(&["update", "--workspace", "--locked"]);
}

#[tokio::test]
async fn a_young_version_locked_before_the_run_does_not_restore_the_unit() {
    let mut run = rust_run("^1.2", "1.2.0");
    let outcome = run.install().await;
    // 未解消のまま（exit 2）だが、この実行で追加した版ではない。
    assert!(!outcome.install_failed && outcome.audit_failed);
    assert!(run.project.read("Cargo.toml").contains("'=1.1.0'"));
    assert_eq!(run.project.locked_versions("leaf"), ["1.2.0"]);
    assert_eq!(run.project.locked_versions("top"), ["1.1.0"]);
    assert!(!run.canceled());
}

/// 監査の外で変わった lockfile は採用しない。マニフェストは戻すが、
/// 変更された lockfile は手動復元用に残し、結果にもその旨を示す。
#[tokio::test]
async fn a_lockfile_changed_after_the_audit_is_kept_and_recovery_is_incomplete() {
    let mut run = rust_run("^1.1", "1.0.0");
    run.checks.edit_after_audit = true;
    let outcome = run.install().await;
    assert!(!outcome.install_failed && outcome.audit_failed);
    assert_eq!(run.project.read("Cargo.toml"), run.manifest);
    assert!(
        run.project
            .read("Cargo.lock")
            .ends_with("# edited after the audit\n")
    );
    assert!(run.result.summary.manifests[0].results.iter().any(|item| {
        item.write_error().is_some_and(|error| {
            error.contains("Cargo.lock changed outside the --age audit")
                && error.contains("recovery incomplete")
        })
    }));
}

#[test]
fn rust_restore_reasons_come_from_the_final_lockfile() {
    let baseline: RegistryLockEntries = [("existing".into(), vec!["1.0.0".into()])].into();
    let audit = |remaining: &[(&str, &str, RemainingAge)], problems: &[&str]| LockAgeAuditResult {
        remaining: remaining
            .iter()
            .map(|(name, version, age)| ((name.to_string(), version.to_string()), *age))
            .collect(),
        problems: problems.iter().map(|problem| problem.to_string()).collect(),
        ..Default::default()
    };
    assert_eq!(rust_restore_reason(&audit(&[], &[]), &baseline), None);
    // 実行前から lock にあった版は報告するが、復元の理由にはしない。
    let existing = audit(&[("existing", "1.0.0", RemainingAge::TooNew)], &[]);
    assert_eq!(rust_restore_reason(&existing, &baseline), None);
    // 差し戻し先も含め、この実行で入れた版は現在 lock されている版として判定する。
    let added = audit(
        &[
            ("existing", "1.0.1", RemainingAge::Unverified),
            ("new", "2.0.0", RemainingAge::TooNew),
        ],
        &[],
    );
    let reason = rust_restore_reason(&added, &baseline).unwrap();
    assert!(
        reason.contains("existing 1.0.1 (release date not verified)"),
        "{reason}"
    );
    assert!(reason.contains("new 2.0.0 (newer than --age)"), "{reason}");
    let unfinished = audit(&[], &["Cargo.lock changed during final age verification"]);
    assert!(
        rust_restore_reason(&unfinished, &baseline)
            .unwrap()
            .contains("did not finish")
    );
}

#[test]
fn many_versions_are_summarized() {
    let items: Vec<String> = (0..7).map(|index| format!("crate-{index} 1.0.0")).collect();
    assert_eq!(
        named(&items),
        "crate-0 1.0.0, crate-1 1.0.0, crate-2 1.0.0, crate-3 1.0.0, crate-4 1.0.0, and 2 more"
    );
    assert_eq!(named(&items[..2]), "crate-0 1.0.0, crate-1 1.0.0");
}

fn violation(name: &str, version: &str) -> NodeAgeViolation {
    NodeAgeViolation {
        name: name.into(),
        version: version.into(),
        released_at: young(),
        cutoff: cutoff(),
    }
}

fn unverified(name: &str, version: &str, kind: UnverifiedKind) -> NodeUnverifiedDependency {
    NodeUnverifiedDependency {
        name: name.into(),
        version: version.into(),
        reason: "release date unavailable".into(),
        kind,
    }
}

#[test]
fn node_restore_reasons_separate_added_existing_and_out_of_scope_versions() {
    let baseline = NodeBaseline::Known([("existing".to_string(), "1.0.0".to_string())].into());
    let audit = |violations: Vec<NodeAgeViolation>, unverified: Vec<NodeUnverifiedDependency>| {
        NodeLockAuditResult {
            violations,
            unverified,
            ..Default::default()
        }
    };
    // 実行前から lock にあった版と、監査対象外の取得元は、報告するだけにする。
    assert_eq!(
        node_restore_reason(
            "pnpm",
            &audit(
                vec![violation("existing", "1.0.0")],
                vec![unverified("private", "1.0.0", UnverifiedKind::OutsideScope)]
            ),
            &baseline
        ),
        None
    );
    for added in [
        audit(vec![violation("existing", "1.0.1")], vec![]),
        audit(
            vec![],
            vec![unverified("new", "1.0.0", UnverifiedKind::Unconfirmed)],
        ),
    ] {
        let reason = node_restore_reason("npm", &added, &baseline).unwrap();
        assert!(reason.contains("versions this run added"), "{reason}");
    }
    // 実行前の lockfile がないと、以前からあった版を区別できない。
    let unknown = NodeBaseline::Unknown("pnpm list failed".into());
    let reason = node_restore_reason(
        "pnpm",
        &audit(vec![violation("existing", "1.0.0")], vec![]),
        &unknown,
    )
    .unwrap();
    assert!(reason.contains("cannot be told apart"), "{reason}");
    assert!(reason.contains("pnpm list failed"), "{reason}");
    // 対応済み形式の lockfile をまったく確認できなかった場合は、成功としない。
    let failed = NodeLockAuditResult {
        errors: vec!["could not read pnpm-lock.yaml for --age audit".into()],
        ..Default::default()
    };
    assert!(
        node_restore_reason("pnpm", &failed, &baseline)
            .unwrap()
            .contains("could not be checked")
    );
    // 未対応の形式（npm lockfileVersion 1）は、baseline がなくても報告だけにする。
    let unsupported = NodeLockAuditResult {
        unsupported: vec!["package-lock.json lockfileVersion 1 is not supported".into()],
        ..Default::default()
    };
    assert!(unsupported.has_failures());
    assert_eq!(
        node_restore_reason(
            "npm",
            &unsupported,
            &NodeBaseline::Unknown("lockfileVersion 1".into())
        ),
        None
    );
}

/// Node の読み取り専用監査。この実行で追加した版が --age を満たさなければ package.json と
/// lockfile を戻す。実行前からあった版の違反なら更新を残す（どちらも exit 2）。
#[tokio::test]
async fn node_units_are_restored_only_for_versions_the_run_added() {
    for (failing, restored) in [("2.0.0", true), ("1.0.0", false)] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("package.json");
        let manifest = "{\"dependencies\":{\"example\":\"1.0.0\"}}";
        std::fs::write(&path, manifest).unwrap();
        let lock = temp.path().join("pnpm-lock.yaml");
        std::fs::write(&lock, "lockfileVersion: '9.0'\n# before the run\n").unwrap();
        let example = PackageJsonParser.parse(manifest).unwrap().remove(0);
        let mut update = ManifestUpdateResult::new(&path, Language::Node);
        update.add_result(UpdateResult::update(example, "2.0.0"));
        let write = ManifestWriter::new(false)
            .apply_updates(&update, &PackageJsonParser)
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(update);
        let mut result = OrchestratorResult {
            summary,
            write_results: vec![write],
            errors: vec![],
        };
        let args = args(temp.path());
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
        let checks = Fixed::node(
            NodeBaseline::Known([("example".to_string(), "1.0.0".to_string())].into()),
            NodeLockAuditResult {
                checked: 1,
                violations: vec![violation("example", failing)],
                ..Default::default()
            },
        );
        let outcome = run_with(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            &checks,
            |command| {
                std::fs::write(&lock, "lockfileVersion: '9.0'\n# installed\n").unwrap();
                InstallResult::success(
                    command.language,
                    "pnpm install".into(),
                    String::new(),
                    String::new(),
                )
            },
        )
        .await;
        assert!(!outcome.install_failed && outcome.audit_failed);
        let (package, locked) = (
            std::fs::read_to_string(&path).unwrap(),
            std::fs::read_to_string(&lock).unwrap(),
        );
        if restored {
            assert_eq!(package, manifest);
            assert_eq!(locked, "lockfileVersion: '9.0'\n# before the run\n");
        } else {
            assert_eq!(package, written);
            assert_eq!(locked, "lockfileVersion: '9.0'\n# installed\n");
        }
    }
}
