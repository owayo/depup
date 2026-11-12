use super::*;
use crate::cli::CliArgs;
use crate::domain::{
    Dependency, ManifestUpdateResult, UpdateSummary, VersionSpec, VersionSpecKind,
};
use clap::Parser;

#[test]
fn no_manifest_updates_still_plan_an_existing_rust_lock_audit() {
    let temp = tempfile::tempdir().unwrap();
    write_lock(temp.path(), "1.0.1");
    let mut args = CliArgs::parse_from(["depup", "--age", "2w"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let mut summary = UpdateSummary::new(false);
    summary.add_manifest(ManifestUpdateResult::new(
        temp.path().join("Cargo.toml"),
        Language::Rust,
    ));
    let result = OrchestratorResult {
        summary,
        write_results: Vec::new(),
        errors: Vec::new(),
    };
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    let audit = plan.rust_audit_plan(&orchestrator, &result);
    assert_eq!(audit.locks.len(), 1);
    assert_eq!(audit.locks[0].baseline["pkg"], vec!["1.0.1"]);
}

#[test]
fn invalid_baseline_is_not_treated_as_a_new_lock() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("Cargo.lock"), "broken {{").unwrap();
    let mut args = CliArgs::parse_from(["depup", "--age", "2w"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let result = result_with_update(temp.path().join("Cargo.toml"), Language::Rust);
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    write_lock(temp.path(), "1.0.1");
    let audit = plan.rust_audit_plan(&orchestrator, &result);
    assert!(audit.locks[0].baseline_problem.is_some());
}

#[test]
fn unchanged_node_workspace_uses_all_members_strictest_age() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let member = root.join("apps/web");
    let other = root.join("apps/worker");
    for directory in [root, member.as_path(), other.as_path()] {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("package.json"), "{\"name\":\"example\"}").unwrap();
    }
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n",
    )
    .unwrap();
    std::fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    std::fs::write(other.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
    let mut args = CliArgs::parse_from(["depup", "--no-age"]);
    args.path = member.clone();
    let orchestrator = Orchestrator::new(args).unwrap();
    let mut result = result_with_update(member.join("package.json"), Language::Node);
    let plan = InstallPlan::new(&orchestrator, &result, &None, &member);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(plan.jobs[0].directory, std::fs::canonicalize(root).unwrap());
    assert_eq!(
        plan.jobs[0].policy.as_ref().unwrap().min_age,
        Some(std::time::Duration::from_secs(30 * 86400))
    );
    result.summary.manifests[0].results.clear();
    let plan = InstallPlan::new(&orchestrator, &result, &None, &member);
    assert!(plan.jobs.is_empty());
    assert_eq!(plan.node_audits.len(), 1);
    assert_eq!(
        plan.node_audits[0].policy.as_ref().unwrap().min_age,
        Some(std::time::Duration::from_secs(30 * 86400))
    );
}

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

/// `result_with_update` の更新を、マニフェストへ書き込めなかったことにした結果
fn result_with_failed_update(path: PathBuf, language: Language) -> OrchestratorResult {
    let mut result = result_with_update(path, language);
    result.summary.manifests[0].record_write_error(0, "Failed to update pkg");
    result
}

#[test]
fn build_install_map_skips_manifest_whose_updates_all_failed() {
    // 回帰 (#21): 書き込めなかった更新しかないマニフェストは何も書き換えていないので、
    // install の理由にしない (Rust の install は lock 全体を上げる `cargo update`)
    let root = PathBuf::from("/repo");
    let result = result_with_failed_update(root.join("Cargo.toml"), Language::Rust);

    assert!(build_install_map(&result, &None, &root).is_empty());
}

#[test]
fn audit_does_not_treat_an_unwritten_update_as_displayed() {
    // 書き込めなかった更新先は、差し戻し先の候補にも「表示した更新先と lock の食い違い」の
    // 注記にも使わない。lock の監査そのものは今までどおり計画する
    let temp = tempfile::tempdir().unwrap();
    let mut args = CliArgs::parse_from(["depup", "--age", "2w"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let result = result_with_failed_update(temp.path().join("Cargo.toml"), Language::Rust);
    write_lock(temp.path(), "1.0.0");
    let plan = InstallPlan::new(&orchestrator, &result, &None, temp.path());
    write_lock(temp.path(), "2.0.1");
    let audit = plan.rust_audit_plan(&orchestrator, &result);
    assert_eq!(audit.locks.len(), 1);
    assert_eq!(audit.locks[0].baseline["pkg"], vec!["1.0.0"]);
    assert!(!audit.locks[0].preferred.contains_key("pkg"));
    assert!(audit.mismatch_reports().is_empty());
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
