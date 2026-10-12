use super::*;
use crate::cli::CliArgs;
use crate::domain::{GitSource, UpdateSummary, VersionSpec, VersionSpecKind};
use clap::Parser;

fn result(path: &Path, dependencies: Vec<Dependency>) -> OrchestratorResult {
    let mut manifest = ManifestUpdateResult::new(path, Language::Rust);
    for dependency in dependencies {
        manifest.add_result(UpdateResult::update(dependency, "2.0.0"));
    }
    let mut summary = UpdateSummary::new(false);
    summary.add_manifest(manifest);
    OrchestratorResult {
        summary,
        write_results: Vec::new(),
        errors: Vec::new(),
    }
}

fn dep(name: &str, requirement: &str) -> Dependency {
    Dependency::production(
        name,
        VersionSpec::new(VersionSpecKind::Caret, requirement, requirement),
        Language::Rust,
    )
}

#[test]
fn scope_uses_applied_real_names_and_pre_install_versions() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = 'example'\nversion = '0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(temp.path().join("Cargo.lock"), "version = 4\n[[package]]\nname = 'selected'\nversion = '1.0.5'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n[[package]]\nname = 'failed'\nversion = '1.0.5'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n").unwrap();
    let mut result = result(
        &temp.path().join("Cargo.toml"),
        vec![
            dep("selected", "1.0.0").with_manifest_name("alias"),
            dep("failed", "1.0.0"),
        ],
    );
    result.summary.manifests[0].record_write_error(1, "write failed");
    let mut args = CliArgs::parse_from(["depup", "--only", "alias", "--no-age"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, temp.path(), true);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].scope,
        Ok(InstallScope::Packages(vec![
            "registry+https://github.com/rust-lang/crates.io-index#selected@1.0.5".into()
        ]))
    );
    result.summary.manifests[0].record_write_error(0, "write failed");
    assert!(
        InstallPlan::new_scoped(&orchestrator, &result, &None, temp.path(), true)
            .jobs
            .is_empty()
    );
}

#[test]
fn scope_selects_only_the_direct_member_edge_when_old_ranges_overlap() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = 'app'\nversion = '0.1.0'\n[dependencies]\nselected = '2.0.0'\n",
    )
    .unwrap();
    std::fs::write(temp.path().join("Cargo.lock"), "version = 4\n[[package]]\nname = 'app'\nversion = '0.1.0'\ndependencies = ['selected 1.0.5']\n[[package]]\nname = 'selected'\nversion = '1.0.5'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n[[package]]\nname = 'selected'\nversion = '1.1.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\n[[package]]\nname = 'selected'\nversion = '1.0.5'\nsource = 'registry+https://example.com/index'\n").unwrap();
    let result = result(&temp.path().join("Cargo.toml"), vec![dep("selected", "1")]);
    assert_eq!(
        cargo_scope(temp.path(), &[&result.summary.manifests[0]]),
        Ok(InstallScope::Packages(vec![
            "registry+https://github.com/rust-lang/crates.io-index#selected@1.0.5".into()
        ]))
    );
}

#[test]
fn git_scope_distinguishes_same_name_and_version_by_url_and_reference() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("Cargo.lock"), "version = 4\n[[package]]\nname = 'selected'\nversion = '1.0.0'\nsource = 'git+https://example.com/selected.git?branch=main#0123456789abcdef0123456789abcdef01234567'\n[[package]]\nname = 'selected'\nversion = '1.0.0'\nsource = 'git+https://example.com/selected.git?branch=other#0123456789abcdef0123456789abcdef01234567'\n[[package]]\nname = 'selected'\nversion = '1.0.0'\nsource = 'git+https://example.com/fork.git?branch=main#0123456789abcdef0123456789abcdef01234567'\n").unwrap();
    let dependency = dep("selected", "main").with_git_source(GitSource::new(
        "https://example.com/selected",
        GitReference::Branch("main".into()),
    ));
    let result = result(&temp.path().join("Cargo.toml"), vec![dependency]);
    assert_eq!(
        cargo_scope(temp.path(), &[&result.summary.manifests[0]]),
        Ok(InstallScope::Packages(vec![
            "git+https://example.com/selected.git?branch=main#selected@1.0.0".into()
        ]))
    );
}

#[test]
fn missing_lock_uses_workspace_reconciliation_and_broken_lock_fails() {
    let temp = tempfile::tempdir().unwrap();
    let result = result(&temp.path().join("Cargo.toml"), vec![dep("selected", "1")]);
    assert_eq!(
        cargo_scope(temp.path(), &[&result.summary.manifests[0]]),
        Ok(InstallScope::CargoWorkspace)
    );
    std::fs::write(temp.path().join("Cargo.lock"), "broken {{").unwrap();
    assert!(cargo_scope(temp.path(), &[&result.summary.manifests[0]]).is_err());
}

#[test]
fn unresolved_branch_update_fails_instead_of_silently_reconciling() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.lock"),
        "version = 4\npackage = []\n",
    )
    .unwrap();
    let dependency = dep("selected", "main").with_git_source(GitSource::new(
        "https://example.com/selected",
        GitReference::Branch("main".into()),
    ));
    let result = result(&temp.path().join("Cargo.toml"), vec![dependency]);
    assert!(cargo_scope(temp.path(), &[&result.summary.manifests[0]]).is_err());
}

#[test]
fn composer_targets_are_real_lowercase_names_and_missing_lock_is_initial_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let result = result(
        &temp.path().join("composer.json"),
        vec![dep("Vendor/Selected", "1")],
    );
    assert_eq!(
        composer_scope(temp.path(), &[&result.summary.manifests[0]]),
        Ok(InstallScope::Full)
    );
    std::fs::write(temp.path().join("composer.lock"), "{}").unwrap();
    assert_eq!(
        composer_scope(temp.path(), &[&result.summary.manifests[0]]),
        Ok(InstallScope::Packages(vec!["vendor/selected".into()]))
    );
}

#[test]
fn shared_workspace_lock_combines_member_targets_once() {
    use crate::test_support::local_registry::{LocalRegistry, TestProject};
    let registry = LocalRegistry::new();
    for name in ["first", "second", "untouched"] {
        registry.publish(name, "1.0.0", &[]);
    }
    let project = TestProject::new(
        &registry,
        "[workspace]\nmembers = ['first-app', 'second-app']\nresolver = '3'\n",
    );
    let member = |name: &str, dependency: &str, version: &str| {
        format!(
            "[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\n{dependency} = '{version}'\nuntouched = '1.0.0'\n"
        )
    };
    project.write(
        "first-app/Cargo.toml",
        &member("first-app", "first", "1.0.0"),
    );
    project.write("first-app/src/lib.rs", "");
    project.write(
        "second-app/Cargo.toml",
        &member("second-app", "second", "1.0.0"),
    );
    project.write("second-app/src/lib.rs", "");
    project.cargo_ok(&["generate-lockfile"]);
    for name in ["first", "second"] {
        registry.publish(name, "2.0.0", &[]);
    }
    registry.publish("untouched", "1.0.1", &[]);
    project.write(
        "first-app/Cargo.toml",
        &member("first-app", "first", "2.0.0"),
    );
    project.write(
        "second-app/Cargo.toml",
        &member("second-app", "second", "2.0.0"),
    );
    let mut result = result(
        &project.path().join("first-app/Cargo.toml"),
        vec![dep("first", "1.0.0")],
    );
    let mut second =
        ManifestUpdateResult::new(project.path().join("second-app/Cargo.toml"), Language::Rust);
    second.add_result(UpdateResult::update(dep("second", "1.0.0"), "2.0.0"));
    result.summary.add_manifest(second);
    let mut args = CliArgs::parse_from(["depup", "--only", "first,second", "--no-age"]);
    args.path = project.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let directories = Some(vec![
        project.path().join("first-app"),
        project.path().join("second-app"),
    ]);
    let plan = InstallPlan::new_scoped(&orchestrator, &result, &directories, project.path(), true);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].directory,
        std::fs::canonicalize(project.path()).unwrap()
    );
    let InstallScope::Packages(selectors) = plan.jobs[0].scope.as_ref().unwrap() else {
        panic!("expected selective Cargo update");
    };
    assert_eq!(selectors.len(), 2);
    let mut command = vec!["update"];
    for selector in selectors {
        command.extend(["--package", selector.as_str()]);
    }
    project.cargo_ok(&command);
    assert_eq!(project.locked_versions("first"), ["2.0.0"]);
    assert_eq!(project.locked_versions("second"), ["2.0.0"]);
    assert_eq!(project.locked_versions("untouched"), ["1.0.0"]);
}

#[test]
fn nested_independent_workspace_without_lock_does_not_use_outer_lock() {
    use crate::test_support::local_registry::{LocalRegistry, TestProject};
    let registry = LocalRegistry::new();
    registry.publish("selected", "1.0.0", &[]);
    registry.publish("selected", "2.0.0", &[]);
    let project = TestProject::new(
        &registry,
        "[package]\nname = 'outer'\nversion = '0.1.0'\n[workspace]\nexclude = ['nested']\n[dependencies]\nselected = '1.0.0'\n",
    );
    project.cargo_ok(&["generate-lockfile"]);
    let outer_lock = project.read("Cargo.lock");
    project.write(".depup", ".\nnested\n");
    project.write(
        "nested/Cargo.toml",
        "[workspace]\nmembers = ['inner']\nresolver = '3'\n",
    );
    project.write("nested/inner/Cargo.toml", "[package]\nname = 'inner'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nselected = '2.0.0'\n");
    project.write("nested/inner/src/lib.rs", "");
    let result = result(
        &project.path().join("nested/inner/Cargo.toml"),
        vec![dep("selected", "1.0.0")],
    );
    let mut args = CliArgs::parse_from(["depup", "--only", "selected", "--no-age"]);
    args.path = project.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let directories = Some(vec![
        project.path().to_path_buf(),
        project.path().join("nested"),
    ]);
    let plan = InstallPlan::new_scoped(&orchestrator, &result, &directories, project.path(), true);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(
        plan.jobs[0].directory,
        std::fs::canonicalize(project.path().join("nested")).unwrap()
    );
    assert_eq!(plan.jobs[0].scope, Ok(InstallScope::CargoWorkspace));
    let output = std::process::Command::new(TestProject::cargo_program())
        .args(["update", "--workspace"])
        .current_dir(&plan.jobs[0].directory)
        .envs(project.cargo_envs())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(project.read("Cargo.lock"), outer_lock);
    let packages = crate::manifest::parse_lock_packages(&project.read("nested/Cargo.lock"));
    assert_eq!(
        packages
            .iter()
            .find(|package| package.name == "selected")
            .unwrap()
            .version,
        "2.0.0"
    );
}

#[test]
fn excluded_independent_package_uses_its_own_existing_or_new_lock() {
    use crate::test_support::local_registry::{LocalRegistry, TestProject};
    for existing_lock in [true, false] {
        let registry = LocalRegistry::new();
        registry.publish("selected", "1.0.0", &[]);
        registry.publish("selected", "2.0.0", &[]);
        let project = TestProject::new(
            &registry,
            "[package]\nname = 'outer'\nversion = '0.1.0'\n[workspace]\nexclude = ['nested']\n[dependencies]\nselected = '1.0.0'\n",
        );
        project.cargo_ok(&["generate-lockfile"]);
        let outer_lock = project.read("Cargo.lock");
        project.write(".depup", ".\nnested\n");
        let manifest = |version: &str| {
            format!(
                "[package]\nname = 'nested'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nselected = '{version}'\n"
            )
        };
        project.write("nested/Cargo.toml", &manifest("1.0.0"));
        project.write("nested/src/lib.rs", "");
        if existing_lock {
            project.cargo_ok(&["generate-lockfile", "--manifest-path", "nested/Cargo.toml"]);
        }
        project.write("nested/Cargo.toml", &manifest("2.0.0"));
        let result = result(
            &project.path().join("nested/Cargo.toml"),
            vec![dep("selected", "1.0.0")],
        );
        let mut args = CliArgs::parse_from(["depup", "--only", "selected", "--no-age"]);
        args.path = project.path().to_path_buf();
        let orchestrator = Orchestrator::new(args).unwrap();
        let directories = Some(vec![
            project.path().to_path_buf(),
            project.path().join("nested"),
        ]);
        let plan =
            InstallPlan::new_scoped(&orchestrator, &result, &directories, project.path(), true);
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(
            plan.jobs[0].directory,
            std::fs::canonicalize(project.path().join("nested")).unwrap()
        );
        let mut command = vec!["update"];
        match plan.jobs[0].scope.as_ref().unwrap() {
            InstallScope::CargoWorkspace => {
                assert!(!existing_lock);
                command.push("--workspace");
            }
            InstallScope::Packages(selectors) => {
                assert!(existing_lock);
                for selector in selectors {
                    command.extend(["--package", selector]);
                }
            }
            InstallScope::Full | InstallScope::CargoFull => {
                panic!("selective install must not fall back to full update")
            }
        }
        let output = std::process::Command::new(TestProject::cargo_program())
            .args(command)
            .current_dir(&plan.jobs[0].directory)
            .envs(project.cargo_envs())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(project.read("Cargo.lock"), outer_lock);
        let packages = crate::manifest::parse_lock_packages(&project.read("nested/Cargo.lock"));
        assert_eq!(
            packages
                .iter()
                .find(|package| package.name == "selected")
                .unwrap()
                .version,
            "2.0.0"
        );
    }
}

#[test]
fn cargo_location_respects_implicit_path_members_and_explicit_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let implicit = temp.path().join("implicit");
    for (path, content) in [
        (
            implicit.join("Cargo.toml"),
            "[workspace]\nmembers = ['app']\n",
        ),
        (
            implicit.join("app/Cargo.toml"),
            "[package]\nname = 'app'\nversion = '0.1.0'\n[dependencies]\nhelper = {path = '../helper'}\n",
        ),
        (
            implicit.join("helper/Cargo.toml"),
            "[package]\nname = 'helper'\nversion = '0.1.0'\n",
        ),
        (
            temp.path().join("central/Cargo.toml"),
            "[workspace]\nmembers = ['../explicit-app']\n",
        ),
        (
            temp.path().join("explicit-app/Cargo.toml"),
            "[package]\nname = 'explicit-app'\nversion = '0.1.0'\nworkspace = '../central'\n",
        ),
    ] {
        std::fs::create_dir_all(path.parent().unwrap().join("src")).unwrap();
        std::fs::write(path.parent().unwrap().join("src/lib.rs"), "").unwrap();
        std::fs::write(path, content).unwrap();
    }
    assert_eq!(
        rust_resolution_directory(&implicit.join("helper/Cargo.toml"), temp.path()).unwrap(),
        std::fs::canonicalize(&implicit).unwrap()
    );
    let explicit_manifest = temp.path().join("explicit-app/Cargo.toml");
    assert_eq!(
        rust_resolution_directory(&explicit_manifest, temp.path()).unwrap(),
        std::fs::canonicalize(temp.path().join("central")).unwrap()
    );
    assert!(
        rust_resolution_directory(&explicit_manifest, explicit_manifest.parent().unwrap()).is_err()
    );
}

#[test]
fn cargo_location_failure_stays_a_scoped_preparation_failure() {
    let temp = tempfile::tempdir().unwrap();
    let result = result(
        &temp.path().join("Cargo.toml"),
        vec![dep("selected", "1.0.0")],
    );
    let mut args = CliArgs::parse_from(["depup", "--only", "selected", "--no-age"]);
    args.path = temp.path().to_path_buf();
    let orchestrator = Orchestrator::new(args).unwrap();
    let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, temp.path(), true);
    assert_eq!(plan.jobs.len(), 1);
    assert!(plan.jobs[0].scope.is_err());
    std::fs::write(temp.path().join("Cargo.toml"), "broken {{").unwrap();
    let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, temp.path(), true);
    assert_eq!(plan.jobs.len(), 1);
    assert!(plan.jobs[0].scope.is_err());
}
