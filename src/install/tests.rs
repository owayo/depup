use super::*;
use crate::cli::CliArgs;
use crate::domain::{
    Dependency, ManifestUpdateResult, UpdateSummary, VersionSpec, VersionSpecKind,
};
use clap::Parser;

#[test]
fn shared_install_includes_a_member_with_no_updates_and_does_not_mix_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    let member = root.join("member");
    let sibling = temp.path().join("other");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(member.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
    std::fs::write(sibling.join(".npmrc"), "minimum-release-age=90d\n").unwrap();
    let mut args = CliArgs::parse_from(["depup", "--no-age"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let mut result = result_with_update(root.join("package.json"), Language::Node);
    for dir in [&member, &sibling] {
        result.summary.add_manifest(ManifestUpdateResult::new(
            dir.join("package.json"),
            Language::Node,
        ));
    }
    let policy = install_age_policy(&orchestrator, &result, &root, Language::Node).unwrap();
    assert_eq!(
        policy.min_age,
        Some(std::time::Duration::from_secs(30 * 86400))
    );
    assert!(policy.exemptions.is_empty());
}

#[test]
fn a_missing_lock_is_an_unverified_age_audit() {
    let temp = tempfile::tempdir().unwrap();
    let mut args = CliArgs::parse_from(["depup", "--age", "2w", "--quiet"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let result = result_with_update(temp.path().join("Cargo.toml"), Language::Rust);
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    let audit = plan.rust_audit_plan(&orchestrator, &result);
    assert!(audit.locks.is_empty());
    assert_eq!(audit.missing_lock_dirs, vec![temp.path().to_path_buf()]);
}

#[test]
fn lock_policy_includes_siblings_when_started_inside_a_member() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let member = root.join("member");
    let other = root.join("other");
    for dir in [&member, &other] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = 'example'\nversion = '1.0.0'\n",
        )
        .unwrap();
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = ['member', 'other']\n",
    )
    .unwrap();
    std::fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    std::fs::write(other.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
    let mut args = CliArgs::parse_from(["depup", "--no-age"]);
    args.path = member.clone();
    let orchestrator = Orchestrator::new(args).unwrap();
    let result = result_with_update(member.join("Cargo.toml"), Language::Rust);
    let groups = RustLockGroups::collect(&orchestrator, &result);
    assert_eq!(groups.locks.len(), 1);
    let policy = &groups.locks.values().next().unwrap().policy;
    assert_eq!(
        policy.min_age,
        Some(std::time::Duration::from_secs(30 * 86400))
    );
    assert!(policy.exemptions.is_empty());
}

fn result_with_update(path: PathBuf, language: Language) -> OrchestratorResult {
    let spec = VersionSpec::new(VersionSpecKind::Caret, "1.0.0", "1.0.0");
    let dep = Dependency::production("pkg", spec, language);
    let mut manifest = ManifestUpdateResult::new(path, language);
    manifest.add_result(UpdateResult::update(dep, "2.0.0"));

    let mut summary = UpdateSummary::new(false);
    summary.add_manifest(manifest);

    OrchestratorResult {
        summary,
        write_results: Vec::new(),
        errors: Vec::new(),
    }
}

#[test]
fn test_nearest_monorepo_dir_uses_deepest_match() {
    let root = PathBuf::from("/repo");
    let app = PathBuf::from("/repo/apps/web");
    let manifest = app.join("package.json");
    let dirs = vec![root.clone(), app.clone()];

    assert_eq!(nearest_monorepo_dir(&manifest, &dirs, &root), app);
}

#[test]
fn test_build_install_map_uses_nested_monorepo_dir() {
    let root = PathBuf::from("/repo");
    let app = PathBuf::from("/repo/apps/web");
    let result = result_with_update(app.join("package.json"), Language::Node);
    let dirs = Some(vec![root.clone(), app.clone()]);

    let install_map = build_install_map(&result, &dirs, &root);

    assert_eq!(install_map.len(), 1);
    assert_eq!(install_map[0].0, app);
    assert_eq!(install_map[0].1, vec![Language::Node]);
}
fn write_lock(directory: &Path, version: &str) {
    std::fs::write(directory.join("Cargo.lock"), format!(
        "version = 4\n[[package]]\nname = 'pkg'\nversion = '{version}'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n"
    )).unwrap();
}

#[test]
fn audit_keeps_the_pre_install_baseline_and_displayed_target() {
    let temp = tempfile::tempdir().unwrap();
    let mut args = CliArgs::parse_from(["depup", "--age", "2w"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let result = result_with_update(temp.path().join("Cargo.toml"), Language::Rust);
    write_lock(temp.path(), "1.0.0");
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    write_lock(temp.path(), "2.0.1");
    let audit = plan.rust_audit_plan(&orchestrator, &result);
    assert_eq!(audit.locks.len(), 1);
    assert_eq!(audit.locks[0].baseline["pkg"], vec!["1.0.0"]);
    assert_eq!(audit.locks[0].preferred["pkg"], vec!["2.0.0"]);
    let reports = audit.mismatch_reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, temp.path().join("Cargo.toml"));
    assert_eq!(reports[0].1[0].locked, "2.0.1");
}

#[test]
fn a_lock_created_by_install_has_an_empty_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let mut args = CliArgs::parse_from(["depup", "--age", "2w"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let result = result_with_update(temp.path().join("Cargo.toml"), Language::Rust);
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    write_lock(temp.path(), "2.0.0");
    let audit = plan.rust_audit_plan(&orchestrator, &result);
    assert!(audit.missing_lock_dirs.is_empty());
    assert_eq!(audit.locks.len(), 1);
    assert!(audit.locks[0].baseline.is_empty());
}
