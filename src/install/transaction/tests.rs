use super::*;
use crate::domain::{
    Dependency, GitSource, ManifestUpdateResult, UpdateResult, VersionSpec, VersionSpecKind,
};
use crate::manifest::{ManifestWriter, get_parser};
use crate::update::AgePolicy;
use clap::Parser;

fn result() -> OrchestratorResult {
    OrchestratorResult {
        summary: UpdateSummary::new(false),
        write_results: vec![],
        errors: vec![],
    }
}
fn update(
    result: &mut OrchestratorResult,
    path: &Path,
    language: Language,
    before: &str,
    package: &str,
) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, before).unwrap();
    let mut manifest = ManifestUpdateResult::new(path, language);
    manifest.add_result(UpdateResult::update(
        Dependency::new(
            package,
            VersionSpec::new(VersionSpecKind::Exact, "1.0.0", "1.0.0"),
            false,
            language,
        ),
        "2.0.0",
    ));
    let write = ManifestWriter::new(false)
        .apply_updates(&manifest, get_parser(language).as_ref())
        .unwrap();
    assert!(write.file_modified, "{:?}", write.errors);
    result.write_results.push(write);
    result.summary.add_manifest(manifest);
}
fn node(result: &mut OrchestratorResult, directory: &Path) {
    update(
        result,
        &directory.join("package.json"),
        Language::Node,
        "{\"dependencies\":{\"example\":\"1.0.0\"}}\n",
        "example",
    );
}
fn job(directory: &Path, language: Language, manifests: &[PathBuf]) -> super::super::InstallJob {
    super::super::InstallJob {
        directory: identity(directory),
        language,
        manifests: manifests.into(),
        scope: Ok(InstallScope::Full),
        policy: Ok(AgePolicy {
            min_age: None,
            evaluated_at: chrono::Utc::now(),
            exemptions: Default::default(),
        }),
    }
}
fn plan(jobs: Vec<super::super::InstallJob>) -> InstallPlan {
    InstallPlan {
        jobs,
        wrapper_updates: vec![],
        node_audits: vec![],
        baselines: BTreeMap::new(),
    }
}
fn failure(command: &PreparedInstall) -> InstallResult {
    InstallResult::failure(
        command.language,
        command.argv.join(" "),
        String::new(),
        "simulated failure".into(),
    )
}
fn success(command: &PreparedInstall) -> InstallResult {
    InstallResult::success(
        command.language,
        command.argv.join(" "),
        String::new(),
        String::new(),
    )
}

#[test]
fn failed_node_install_restores_unchanged_partial_deleted_and_new_locks() {
    for change in ["unchanged", "partial", "deleted", "new"] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let mut result = result();
        node(&mut result, directory);
        let lock = directory.join("package-lock.json");
        if change != "new" {
            fs::write(&lock, "original lock\n").unwrap();
        }
        let before = result.write_results[0]
            .journal
            .as_ref()
            .unwrap()
            .before
            .clone();
        let plan = plan(vec![job(
            directory,
            Language::Node,
            &[directory.join("package.json")],
        )]);
        let mut tx = InstallTransactions::begin(&plan, &result, directory);
        tx.execute(|command| {
            match change {
                "partial" | "new" => fs::write(&lock, "partial").unwrap(),
                "deleted" => fs::remove_file(&lock).unwrap(),
                _ => {}
            };
            failure(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(
            fs::read(directory.join("package.json")).unwrap(),
            before,
            "{change}"
        );
        if change == "new" {
            assert!(!lock.exists());
        } else {
            assert_eq!(fs::read_to_string(lock).unwrap(), "original lock\n");
        }
        assert_eq!(result.summary.total_updates(), 0);
        assert_eq!(result.summary.total_failed(), 1);
        assert!(!result.summary.manifests[0].modified);
        assert!(report.covers_directory(directory));
    }
}

#[test]
fn independent_success_survives_shared_failure_and_remaining_jobs_are_skipped() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let mut result = result();
    node(&mut result, &a);
    node(&mut result, &b);
    let plan = plan(vec![
        job(&a, Language::Node, &[a.join("package.json")]),
        job(&a, Language::Node, &[a.join("package.json")]),
        job(&b, Language::Node, &[b.join("package.json")]),
    ]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    let mut calls = vec![];
    tx.execute(|command| {
        calls.push(command.directory.clone());
        fs::write(command.directory.join("package-lock.json"), "new lock").unwrap();
        if command.directory == identity(&a) {
            failure(command)
        } else {
            success(command)
        }
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(calls, vec![identity(&a), identity(&b)]);
    assert!(!a.join("package-lock.json").exists());
    assert_eq!(
        fs::read_to_string(b.join("package-lock.json")).unwrap(),
        "new lock"
    );
    assert!(
        fs::read_to_string(b.join("package.json"))
            .unwrap()
            .contains("2.0.0")
    );
    assert_eq!(result.summary.total_updates(), 1);
    assert_eq!(result.summary.total_failed(), 1);
}

#[test]
fn policy_scope_and_snapshot_errors_restore_journals_without_running_that_unit() {
    for error in ["policy", "scope", "snapshot"] {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        let mut result = result();
        node(&mut result, &a);
        node(&mut result, &b);
        let mut broken = job(&a, Language::Node, &[a.join("package.json")]);
        match error {
            "policy" => broken.policy = Err("bad policy".into()),
            "scope" => broken.scope = Err("bad scope".into()),
            _ => fs::create_dir(a.join("package-lock.json")).unwrap(),
        }
        let plan = plan(vec![
            broken,
            job(&b, Language::Node, &[b.join("package.json")]),
        ]);
        let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
        let mut calls = vec![];
        tx.execute(|command| {
            calls.push(command.directory.clone());
            success(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(calls, vec![identity(&b)], "{error}");
        assert_eq!(result.summary.total_failed(), 1);
        assert!(
            fs::read_to_string(a.join("package.json"))
                .unwrap()
                .contains("1.0.0")
        );
    }
}

#[test]
fn go_pm_rewrite_returns_to_writer_before_and_removes_created_sum() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("go.mod");
    let mut result = result();
    let before = "module example.com/app\n\ngo 1.20\n\nrequire example.com/library v1.0.0\n";
    update(
        &mut result,
        &path,
        Language::Go,
        before,
        "example.com/library",
    );
    let plan = plan(vec![job(
        temp.path(),
        Language::Go,
        std::slice::from_ref(&path),
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(|command| {
        fs::write(&path, "PM rewritten module\n").unwrap();
        fs::write(temp.path().join("go.sum"), "checksum").unwrap();
        failure(command)
    });
    let report = tx.rollback();
    assert!(report.has_failures());
    assert_eq!(fs::read_to_string(path).unwrap(), before);
    assert!(!temp.path().join("go.sum").exists());
}

#[test]
fn external_pm_output_edit_is_preserved_and_backup_contains_writer_before() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let mut result = result();
    node(&mut result, &a);
    node(&mut result, &b);
    let before = result.write_results[0]
        .journal
        .as_ref()
        .unwrap()
        .before
        .clone();
    fs::write(a.join("package-lock.json"), "old lock").unwrap();
    let plan = plan(vec![
        job(&a, Language::Node, &[a.join("package.json")]),
        job(&b, Language::Node, &[b.join("package.json")]),
    ]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(|command| {
        if command.directory == identity(&a) {
            fs::write(a.join("package.json"), "owned PM rewrite").unwrap();
            fs::write(a.join("package-lock.json"), "owned lock").unwrap();
            failure(command)
        } else {
            fs::write(a.join("package.json"), "external edit").unwrap();
            fs::write(a.join("package-lock.json"), "external lock").unwrap();
            success(command)
        }
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(
        fs::read_to_string(a.join("package.json")).unwrap(),
        "external edit"
    );
    assert_eq!(
        fs::read_to_string(a.join("package-lock.json")).unwrap(),
        "external lock"
    );
    assert!(result.summary.manifests[0].modified);
    assert_eq!(result.summary.total_failed(), 1);
    let FileOutcome::Conflict {
        backup: Ok(backup), ..
    } = &report.units[0].files[&identity(&a.join("package.json"))]
    else {
        panic!("missing conflict")
    };
    assert_eq!(fs::read(backup).unwrap(), before);
    fs::remove_dir_all(backup.parent().unwrap()).unwrap();
    for outcome in report.units[0].files.values() {
        if let FileOutcome::Conflict {
            backup: Ok(path), ..
        } = outcome
        {
            let _ = fs::remove_dir_all(path.parent().unwrap());
        }
    }
}

fn cargo(result: &mut OrchestratorResult, path: &Path, name: &str) {
    fs::create_dir_all(path.parent().unwrap().join("src")).unwrap();
    fs::write(path.parent().unwrap().join("src/lib.rs"), "").unwrap();
    update(
        result,
        path,
        Language::Rust,
        &format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n[dependencies]\nexample = \"1.0.0\"\n"
        ),
        "example",
    );
}

#[test]
fn shared_cargo_members_without_a_lock_form_one_failed_unit() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    let mut result = result();
    cargo(&mut result, &a.join("Cargo.toml"), "a");
    cargo(&mut result, &b.join("Cargo.toml"), "b");
    let plan = plan(vec![
        job(&a, Language::Rust, &[a.join("Cargo.toml")]),
        job(&b, Language::Rust, &[b.join("Cargo.toml")]),
    ]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    let mut calls = 0;
    tx.execute(|command| {
        calls += 1;
        fs::write(temp.path().join("Cargo.lock"), "partial lock").unwrap();
        failure(command)
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(
        calls,
        1,
        "{:?}",
        report
            .units
            .iter()
            .map(|unit| &unit.reason)
            .collect::<Vec<_>>()
    );
    assert!(!temp.path().join("Cargo.lock").exists());
    assert_eq!(result.summary.total_failed(), 2);
    for path in [a.join("Cargo.toml"), b.join("Cargo.toml")] {
        assert!(fs::read_to_string(path).unwrap().contains("1.0.0"));
    }
}

#[test]
fn tauri_node_and_lock_only_rust_updates_fail_together_in_either_direction() {
    for rust_fails in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut result = result();
        node(&mut result, temp.path());
        let rust = temp.path().join("src-tauri");
        fs::create_dir(&rust).unwrap();
        fs::write(
            rust.join("Cargo.toml"),
            "[package]\nname = \"desktop\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir(rust.join("src")).unwrap();
        fs::write(rust.join("src/lib.rs"), "").unwrap();
        fs::write(rust.join("Cargo.lock"), "original lock").unwrap();
        let mut manifest = ManifestUpdateResult::new(rust.join("Cargo.toml"), Language::Rust);
        let dependency = Dependency::new(
            "example",
            VersionSpec::new(VersionSpecKind::Any, "*", ""),
            false,
            Language::Rust,
        )
        .with_git_source(GitSource::new(
            "https://example.com/library.git",
            crate::domain::GitReference::Branch("main".into()),
        ));
        manifest.add_result(UpdateResult::update(dependency, "newcommit"));
        result.summary.add_manifest(manifest);
        let plan = plan(vec![
            job(
                temp.path(),
                Language::Node,
                &[temp.path().join("package.json")],
            ),
            job(temp.path(), Language::Rust, &[rust.join("Cargo.toml")]),
        ]);
        let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
        let mut calls = 0;
        tx.execute(|command| {
            calls += 1;
            if command.language == Language::Rust {
                fs::write(rust.join("Cargo.lock"), "new commit").unwrap();
            }
            if (command.language == Language::Rust) == rust_fails {
                failure(command)
            } else {
                success(command)
            }
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(
            calls,
            if rust_fails { 2 } else { 1 },
            "{:?}",
            report
                .units
                .iter()
                .map(|unit| &unit.reason)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            fs::read_to_string(rust.join("Cargo.lock")).unwrap(),
            "original lock"
        );
        assert_eq!(result.summary.total_failed(), 2);
    }
}

#[cfg(unix)]
#[test]
fn symlink_and_permissions_survive_recovery_and_replaced_links_are_kept() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for replacement in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real.json");
        let link = temp.path().join("package.json");
        fs::write(&real, "{\"dependencies\":{\"example\":\"1.0.0\"}}").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&real, &link).unwrap();
        let mut result = result();
        let mut manifest = ManifestUpdateResult::new(&link, Language::Node);
        manifest.add_result(UpdateResult::update(
            Dependency::new(
                "example",
                VersionSpec::new(VersionSpecKind::Exact, "1.0.0", "1.0.0"),
                false,
                Language::Node,
            ),
            "2.0.0",
        ));
        result.write_results.push(
            ManifestWriter::new(false)
                .apply_updates(&manifest, get_parser(Language::Node).as_ref())
                .unwrap(),
        );
        result.summary.add_manifest(manifest);
        let plan = plan(vec![job(
            temp.path(),
            Language::Node,
            std::slice::from_ref(&link),
        )]);
        let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
        tx.execute(failure);
        let other = temp.path().join("other.json");
        if replacement {
            fs::write(&other, "external").unwrap();
            fs::remove_file(&link).unwrap();
            symlink(&other, &link).unwrap();
        }
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o640
        );
        if replacement {
            assert_eq!(fs::read_to_string(&link).unwrap(), "external");
            assert!(result.summary.manifests[0].modified);
        } else {
            assert!(fs::read_to_string(&link).unwrap().contains("1.0.0"));
        }
        for outcome in report.units[0].files.values() {
            if let FileOutcome::Conflict {
                backup: Ok(path), ..
            } = outcome
            {
                fs::remove_dir_all(path.parent().unwrap()).unwrap();
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn wrapper_second_pass_failure_restores_binary_scripts_properties_and_skips_java() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path();
    let mut result = result();
    update(
        &mut result,
        &directory.join("gradle/wrapper/gradle-wrapper.properties"),
        Language::Java,
        "distributionUrl=https\\://services.gradle.org/distributions/gradle-1.0.0-bin.zip\n",
        crate::manifest::GRADLE_WRAPPER_PACKAGE,
    );
    update(
        &mut result,
        &directory.join("build.gradle"),
        Language::Java,
        "dependencies { implementation 'org.example:library:1.0.0' }\n",
        "org.example:library",
    );
    fs::write(
        directory.join("gradle/wrapper/gradle-wrapper.jar"),
        [0, 255, 1, 128],
    )
    .unwrap();
    fs::write(directory.join("gradlew"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(directory.join("gradlew"), fs::Permissions::from_mode(0o751)).unwrap();
    fs::write(directory.join("gradlew.bat"), "old batch script\r\n").unwrap();
    let mut plan = plan(vec![job(
        directory,
        Language::Java,
        &[directory.join("build.gradle")],
    )]);
    plan.wrapper_updates
        .push((directory.into(), "2.0.0".into()));
    let mut tx = InstallTransactions::begin(&plan, &result, directory);
    let mut calls = 0;
    tx.execute(|command| {
        calls += 1;
        assert_eq!(command.argv[1], "wrapper");
        fs::write(
            directory.join("gradle/wrapper/gradle-wrapper.properties"),
            "PM rewritten properties",
        )
        .unwrap();
        fs::write(
            directory.join("gradle/wrapper/gradle-wrapper.jar"),
            [9, 8, 7],
        )
        .unwrap();
        fs::write(directory.join("gradlew"), "new script").unwrap();
        fs::set_permissions(directory.join("gradlew"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(directory.join("gradlew.bat"), "new batch script").unwrap();
        if calls == 1 {
            success(command)
        } else {
            failure(command)
        }
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(calls, 2);
    assert_eq!(
        fs::read(directory.join("gradle/wrapper/gradle-wrapper.jar")).unwrap(),
        [0, 255, 1, 128]
    );
    assert_eq!(
        fs::read_to_string(directory.join("gradlew")).unwrap(),
        "#!/bin/sh\nexit 0\n"
    );
    assert_eq!(
        fs::metadata(directory.join("gradlew"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    assert_eq!(
        fs::read_to_string(directory.join("gradlew.bat")).unwrap(),
        "old batch script\r\n"
    );
    assert!(
        fs::read_to_string(directory.join("gradle/wrapper/gradle-wrapper.properties"))
            .unwrap()
            .contains("1.0.0")
    );
    assert_eq!(result.summary.total_failed(), 2);
}

#[test]
fn pnpm_workspace_restores_all_members_and_keeps_an_unrelated_project() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("workspace");
    let member = directory.join("apps/web");
    let other = temp.path().join("other");
    let mut result = result();
    node(&mut result, &directory);
    node(&mut result, &member);
    node(&mut result, &other);
    fs::write(
        directory.join("pnpm-workspace.yaml"),
        "packages:\n  - apps/*\n",
    )
    .unwrap();
    fs::write(directory.join("pnpm-lock.yaml"), "old lock").unwrap();
    let plan = plan(vec![
        job(
            &directory,
            Language::Node,
            &[directory.join("package.json"), member.join("package.json")],
        ),
        job(&other, Language::Node, &[other.join("package.json")]),
    ]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(|command| {
        if command.directory == identity(&directory) {
            fs::write(directory.join("pnpm-lock.yaml"), "new lock").unwrap();
            failure(command)
        } else {
            success(command)
        }
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    for path in [directory.join("package.json"), member.join("package.json")] {
        assert!(fs::read_to_string(path).unwrap().contains("1.0.0"));
    }
    assert_eq!(
        fs::read_to_string(directory.join("pnpm-lock.yaml")).unwrap(),
        "old lock"
    );
    assert_eq!(result.summary.total_failed(), 2);
    assert_eq!(result.summary.total_updates(), 1);
}

#[cfg(unix)]
#[test]
fn new_lock_replaced_with_a_symlink_after_pm_is_never_deleted() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let mut result = result();
    node(&mut result, temp.path());
    let plan = plan(vec![job(
        temp.path(),
        Language::Node,
        &[temp.path().join("package.json")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    let lock = temp.path().join("package-lock.json");
    tx.execute(|command| {
        fs::write(&lock, "owned generated lock").unwrap();
        failure(command)
    });
    let target = temp.path().join("external");
    fs::write(&target, "user data").unwrap();
    fs::remove_file(&lock).unwrap();
    symlink(&target, &lock).unwrap();
    let report = tx.rollback();
    assert!(
        fs::symlink_metadata(&lock)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "user data");
    assert!(
        report.units[0]
            .files
            .values()
            .any(|outcome| matches!(outcome, FileOutcome::Conflict { .. }))
    );
    for outcome in report.units[0].files.values() {
        if let FileOutcome::Conflict {
            backup: Ok(path), ..
        } = outcome
        {
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn a_real_spawn_failure_rolls_back_and_uses_the_prepared_manager_after_new_locks() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = result();
    node(&mut result, temp.path());
    let plan = plan(vec![job(
        temp.path(),
        Language::Node,
        &[temp.path().join("package.json")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    fs::write(temp.path().join("pnpm-lock.yaml"), "external new lock").unwrap();
    let prepared = tx.tasks[0].command.as_mut().unwrap().as_mut().unwrap();
    assert_eq!(prepared.package_manager, "npm");
    prepared.program = temp.path().join("missing-program");
    // A new lock is an external change, so it must stop execution rather than re-detect pnpm.
    let mut calls = 0;
    tx.execute(|command| {
        calls += 1;
        SystemPackageManager::new().run_prepared_install(command)
    });
    let report = tx.rollback();
    assert_eq!(calls, 0);
    assert!(report.has_failures());
    assert_eq!(
        fs::read_to_string(temp.path().join("pnpm-lock.yaml")).unwrap(),
        "external new lock"
    );
    for outcome in report.units[0].files.values() {
        if let FileOutcome::Conflict {
            backup: Ok(path), ..
        } = outcome
        {
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    let mut fresh = self::result();
    node(&mut fresh, temp.path());
    let mut tx = InstallTransactions::begin(&plan, &fresh, temp.path());
    tx.tasks[0]
        .command
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .program = temp.path().join("missing-program");
    let mut calls = 0;
    tx.execute(|command| {
        calls += 1;
        SystemPackageManager::new().run_prepared_install(command)
    });
    let report = tx.rollback();
    assert_eq!(calls, 1);
    assert!(report.has_failures());
}

#[test]
fn mise_dot_config_lock_and_deleted_dependency_metadata_are_recovered() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path();
    let mut result = result();
    update(
        &mut result,
        &directory.join(".mise.toml"),
        Language::Mise,
        "[tools]\nnode = \"1.0.0\"\n",
        "node",
    );
    fs::write(directory.join("mise.lock"), "old mise lock").unwrap();
    let receipt = directory.join(".mise/locks/pipx-tool/1.0.0");
    fs::create_dir_all(&receipt).unwrap();
    fs::write(receipt.join("uv.lock"), "old dependency lock").unwrap();
    fs::write(receipt.join("pyproject.toml"), "old dependency manifest").unwrap();
    let unrelated = directory.join(".mise/locks/unrelated");
    fs::write(&unrelated, "unowned file").unwrap();
    let plan = plan(vec![job(
        directory,
        Language::Mise,
        &[directory.join(".mise.toml")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, directory);
    let new_receipt = directory.join(".mise/locks/npm-tool/2.0.0~abcd1234");
    tx.execute(|command| {
        fs::write(directory.join("mise.lock"), "partial mise lock").unwrap();
        fs::remove_dir_all(&receipt).unwrap();
        fs::create_dir_all(&new_receipt).unwrap();
        fs::write(new_receipt.join("aube-lock.yaml"), "new dependency lock").unwrap();
        fs::write(new_receipt.join("package.json"), "new dependency manifest").unwrap();
        fs::write(&unrelated, "unowned PM output").unwrap();
        failure(command)
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(
        fs::read_to_string(directory.join("mise.lock")).unwrap(),
        "old mise lock"
    );
    assert!(!directory.join(".mise.lock").exists());
    assert_eq!(
        fs::read_to_string(receipt.join("uv.lock")).unwrap(),
        "old dependency lock"
    );
    assert_eq!(
        fs::read_to_string(receipt.join("pyproject.toml")).unwrap(),
        "old dependency manifest"
    );
    assert!(!new_receipt.join("aube-lock.yaml").exists());
    assert!(!new_receipt.join("package.json").exists());
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "unowned PM output");
    assert_eq!(result.summary.total_failed(), 1);
}

#[test]
fn scoped_cargo_in_tauri_root_uses_authoritative_root_and_preserves_excluded_package() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path();
    let rust = directory.join("src-tauri");
    let mut result = result();
    cargo(&mut result, &directory.join("Cargo.toml"), "root");
    cargo(&mut result, &rust.join("Cargo.toml"), "desktop");
    fs::write(directory.join("Cargo.lock"), "root lock").unwrap();
    fs::write(rust.join("Cargo.lock"), "desktop lock").unwrap();
    let mut scoped = job(directory, Language::Rust, &[directory.join("Cargo.toml")]);
    scoped.scope = Ok(InstallScope::CargoFull);
    let plan = plan(vec![scoped]);
    let mut tx = InstallTransactions::begin(&plan, &result, directory);
    tx.execute(|command| {
        assert_eq!(command.directory, identity(directory));
        assert_eq!(command.argv, ["cargo", "update"]);
        fs::write(directory.join("Cargo.lock"), "partial root lock").unwrap();
        failure(command)
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(
        fs::read_to_string(directory.join("Cargo.lock")).unwrap(),
        "root lock"
    );
    assert_eq!(
        fs::read_to_string(rust.join("Cargo.lock")).unwrap(),
        "desktop lock"
    );
    assert!(
        fs::read_to_string(directory.join("Cargo.toml"))
            .unwrap()
            .contains("1.0.0")
    );
    assert!(
        fs::read_to_string(rust.join("Cargo.toml"))
            .unwrap()
            .contains("2.0.0")
    );
    assert_eq!(result.summary.total_failed(), 1);
    assert_eq!(result.summary.total_updates(), 1);
}

#[test]
fn failed_cargo_unit_is_excluded_from_existing_and_missing_lock_audits() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = result();
    cargo(&mut result, &temp.path().join("Cargo.toml"), "app");
    let mut job = job(
        temp.path(),
        Language::Rust,
        &[temp.path().join("Cargo.toml")],
    );
    job.scope = Err("scoped preparation failed".into());
    let plan = plan(vec![job]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(|_| panic!("failed scope ran"));
    let report = tx.rollback();
    let args = crate::cli::CliArgs::parse_from(["depup", "--age", "2w"]);
    let mut args = args;
    args.path = temp.path().into();
    let orchestrator = crate::orchestrator::Orchestrator::new(args).unwrap();
    let mut audit = plan.rust_audit_plan(&orchestrator, &result);
    report.exclude_rust_audits(&mut audit);
    assert!(audit.locks.is_empty());
    assert!(audit.missing_lock_dirs.is_empty());
    fs::write(
        temp.path().join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let mut audit = plan.rust_audit_plan(&orchestrator, &result);
    report.exclude_rust_audits(&mut audit);
    assert!(audit.locks.is_empty());
    assert!(audit.missing_lock_dirs.is_empty());
}

#[test]
fn mise_monorepo_migration_restores_child_locks_and_shared_destination() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path();
    let app = directory.join("packages/app");
    let worker = directory.join("packages/worker");
    let mut result = result();
    fs::write(
        directory.join("mise.toml"),
        "monorepo_root = true\n[monorepo]\nlockfile = true\nconfig_roots = [\"packages/*\"]\n",
    )
    .unwrap();
    update(
        &mut result,
        &app.join("mise.toml"),
        Language::Mise,
        "[tools]\nnode = \"1.0.0\"\n",
        "node",
    );
    fs::create_dir_all(&worker).unwrap();
    fs::write(worker.join("mise.toml"), "[tools]\nnode = \"1.0.0\"\n").unwrap();
    for root in [directory, app.as_path(), worker.as_path()] {
        fs::write(
            root.join("mise.lock"),
            format!("old {} lock", root.file_name().unwrap().to_string_lossy()),
        )
        .unwrap();
    }
    let before: Vec<_> = [directory, app.as_path(), worker.as_path()]
        .iter()
        .map(|root| fs::read(root.join("mise.lock")).unwrap())
        .collect();
    let plan = plan(vec![job(&app, Language::Mise, &[app.join("mise.toml")])]);
    let mut tx = InstallTransactions::begin(&plan, &result, directory);
    tx.execute(|command| {
        fs::write(directory.join("mise.lock"), "partially merged").unwrap();
        fs::remove_file(app.join("mise.lock")).unwrap();
        fs::remove_file(worker.join("mise.lock")).unwrap();
        failure(command)
    });
    let report = tx.rollback();
    assert!(report.has_failures());
    for (root, before) in [directory, app.as_path(), worker.as_path()]
        .iter()
        .zip(before)
    {
        assert_eq!(fs::read(root.join("mise.lock")).unwrap(), before);
    }
}

#[test]
fn selected_uv_member_uses_its_confirmed_workspace_lock_and_keeps_an_excluded_project() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path();
    let member = directory.join("packages/member");
    let excluded = directory.join("packages/excluded");
    let mut result = result();
    fs::write(
        directory.join("pyproject.toml"),
        "[tool.uv.workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/excluded\"]\n",
    )
    .unwrap();
    fs::write(directory.join("uv.lock"), "shared old lock").unwrap();
    for root in [&member, &excluded] {
        update(
            &mut result,
            &root.join("pyproject.toml"),
            Language::Python,
            "[project]\nname = \"example\"\nversion = \"0.1.0\"\ndependencies = [\"example==1.0.0\"]\n",
            "example",
        );
        fs::write(root.join("uv.lock"), "local old lock").unwrap();
    }
    let plan = plan(vec![
        job(&member, Language::Python, &[member.join("pyproject.toml")]),
        job(
            &excluded,
            Language::Python,
            &[excluded.join("pyproject.toml")],
        ),
    ]);
    let mut tx = InstallTransactions::begin(&plan, &result, directory);
    tx.execute(|command| {
        assert_eq!(command.package_manager, "uv");
        if command.directory == identity(&member) {
            fs::write(directory.join("uv.lock"), "partial shared lock").unwrap();
            failure(command)
        } else {
            fs::write(excluded.join("uv.lock"), "successful local lock").unwrap();
            success(command)
        }
    });
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(
        fs::read_to_string(directory.join("uv.lock")).unwrap(),
        "shared old lock"
    );
    assert_eq!(
        fs::read_to_string(excluded.join("uv.lock")).unwrap(),
        "successful local lock"
    );
    assert_eq!(result.summary.total_failed(), 1);
    assert_eq!(result.summary.total_updates(), 1);
}

#[cfg(unix)]
#[test]
fn parent_symlink_replacement_after_pm_preserves_both_locations() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original");
    let foreign = temp.path().join("foreign");
    let alias = temp.path().join("alias");
    let mut result = result();
    node(&mut result, &original);
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("package.json"), "external data").unwrap();
    symlink(&original, &alias).unwrap();
    // Keep the writer's logical alias rather than only the canonical project path.
    let mut aliased = self::result();
    update(
        &mut aliased,
        &alias.join("package.json"),
        Language::Node,
        "{\"dependencies\":{\"example\":\"1.0.0\"}}",
        "example",
    );
    let plan = plan(vec![job(
        &alias,
        Language::Node,
        &[alias.join("package.json")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &aliased, temp.path());
    tx.execute(failure);
    fs::remove_file(&alias).unwrap();
    symlink(&foreign, &alias).unwrap();
    let report = tx.rollback();
    report.mark_summary(&mut aliased.summary);
    assert_eq!(
        fs::read_to_string(alias.join("package.json")).unwrap(),
        "external data"
    );
    assert!(
        fs::read_to_string(original.join("package.json"))
            .unwrap()
            .contains("2.0.0")
    );
    assert_eq!(aliased.summary.total_failed(), 1);
    assert!(report.covers_directory(&alias));
    assert!(report.covers_directory(&foreign));
    for outcome in report.units[0].files.values() {
        if let FileOutcome::Conflict {
            backup: Ok(path), ..
        } = outcome
        {
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn external_change_invalidates_ownership_permanently_even_if_bytes_return() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = result();
    node(&mut result, temp.path());
    let written = fs::read(temp.path().join("package.json")).unwrap();
    let plan = plan(vec![job(
        temp.path(),
        Language::Node,
        &[temp.path().join("package.json")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    fs::write(temp.path().join("package.json"), "external edit").unwrap();
    tx.execute(|_| panic!("ownership invalidated before PM"));
    fs::write(temp.path().join("package.json"), &written).unwrap();
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(fs::read(temp.path().join("package.json")).unwrap(), written);
    assert_eq!(result.summary.total_failed(), 1);
    assert!(result.summary.manifests[0].modified);
    for outcome in report.units[0].files.values() {
        if let FileOutcome::Conflict {
            backup: Ok(path), ..
        } = outcome
        {
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn restore_io_failure_keeps_that_manifest_failed_backs_it_up_and_continues_other_files() {
    use crate::output::OutputFormatter;
    let temp = tempfile::tempdir().unwrap();
    let mut result = result();
    node(&mut result, temp.path());
    let manifest = identity(&temp.path().join("package.json"));
    let before = result.write_results[0]
        .journal
        .as_ref()
        .unwrap()
        .before
        .clone();
    fs::write(temp.path().join("package-lock.json"), "old lock").unwrap();
    let plan = plan(vec![job(
        temp.path(),
        Language::Node,
        &[temp.path().join("package.json")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(|command| {
        fs::write(temp.path().join("package-lock.json"), "partial lock").unwrap();
        failure(command)
    });
    let report = tx.rollback_with(|path, record| {
        if path == manifest {
            FileOutcome::Failed {
                reason: "simulated recovery I/O failure".into(),
                backup: backup(&record.before),
            }
        } else {
            restore_record(record)
        }
    });
    report.mark_summary(&mut result.summary);
    assert_eq!(
        fs::read_to_string(temp.path().join("package-lock.json")).unwrap(),
        "old lock"
    );
    assert!(fs::read_to_string(&manifest).unwrap().contains("2.0.0"));
    assert_eq!(result.summary.total_failed(), 1);
    assert!(result.summary.manifests[0].modified);
    let FileOutcome::Failed {
        backup: Ok(copy), ..
    } = &report.units[0].files[&manifest]
    else {
        panic!("missing recovery failure")
    };
    assert_eq!(fs::read(copy).unwrap(), before);
    let mut output = vec![];
    crate::output::JsonFormatter::new(crate::output::Verbosity::Normal)
        .format(&result, &mut output)
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["summary"]["updates"], 0);
    assert_eq!(value["summary"]["failed"], 1);
    let mut report_text = vec![];
    report.print(&mut report_text).unwrap();
    let report_text = String::from_utf8(report_text).unwrap();
    assert!(report_text.contains("simulated recovery I/O failure"));
    assert!(report_text.contains("Pre-write copy"));
    fs::remove_dir_all(copy.parent().unwrap()).unwrap();
}

#[test]
fn nested_mise_monorepo_scope_fails_before_running_and_recovers_the_journal() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    let mut result = result();
    fs::write(
        temp.path().join("mise.toml"),
        "monorepo_root = true\n[monorepo]\nlockfile = true\n",
    )
    .unwrap();
    update(
        &mut result,
        &child.join("mise.toml"),
        Language::Mise,
        "monorepo_root = true\n[monorepo]\nlockfile = true\n[tools]\nnode = \"1.0.0\"\n",
        "node",
    );
    let plan = plan(vec![job(
        &child,
        Language::Mise,
        &[child.join("mise.toml")],
    )]);
    let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
    tx.execute(|_| panic!("unverified nested mise scope executed"));
    let report = tx.rollback();
    report.mark_summary(&mut result.summary);
    assert_eq!(result.summary.total_failed(), 1);
    assert!(
        fs::read_to_string(child.join("mise.toml"))
            .unwrap()
            .contains("1.0.0")
    );
}

fn shared_non_cargo_project(
    result: &mut OrchestratorResult,
    directory: &Path,
    member: &Path,
    language: Language,
) -> (PathBuf, PathBuf) {
    fs::create_dir_all(directory).unwrap();
    match language {
        Language::Python => {
            fs::write(
                directory.join("pyproject.toml"),
                "[tool.uv.workspace]\nmembers = [\"packages/*\"]\n",
            )
            .unwrap();
            let manifest = member.join("pyproject.toml");
            update(
                result,
                &manifest,
                language,
                "[project]\nname = \"example\"\nversion = \"0.1.0\"\ndependencies = [\"example==1.0.0\"]\n",
                "example",
            );
            fs::write(member.join("uv.lock"), "old member lock").unwrap();
            let lock = directory.join("uv.lock");
            fs::write(&lock, "original shared lock").unwrap();
            (manifest, lock)
        }
        Language::Mise => {
            fs::write(directory.join("mise.toml"), "monorepo_root = true\n[monorepo]\nlockfile = true\nconfig_roots = [\"packages/*\"]\n").unwrap();
            let manifest = member.join("mise.toml");
            update(
                result,
                &manifest,
                language,
                "[tools]\nnode = \"1.0.0\"\n",
                "node",
            );
            let lock = directory.join("mise.lock");
            fs::write(&lock, "original shared lock").unwrap();
            (manifest, lock)
        }
        _ => unreachable!(),
    }
}

#[test]
fn member_execution_boundary_does_not_prove_uv_or_mise_recovery_is_local() {
    for language in [Language::Python, Language::Mise] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let member = directory.join("packages/member");
        let mut result = result();
        let (manifest, lock) = shared_non_cargo_project(&mut result, directory, &member, language);
        let before = result.write_results[0]
            .journal
            .as_ref()
            .unwrap()
            .before
            .clone();
        let plan = plan(vec![job(
            &member,
            language,
            std::slice::from_ref(&manifest),
        )]);
        // This is the Cargo boundary obtained by main when started inside the member.
        let mut tx = InstallTransactions::begin(&plan, &result, &member);
        let mut calls = 0;
        tx.execute(|command| {
            calls += 1;
            fs::write(&lock, "partially rewritten shared lock").unwrap();
            failure(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(
            fs::read_to_string(&lock).unwrap(),
            "original shared lock",
            "{language}"
        );
        assert_eq!(
            calls, 0,
            "shared root outside recovery permission must stop before PM: {language}"
        );
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert_eq!(result.summary.total_failed(), 1);
    }
}

#[test]
fn uv_and_mise_policy_or_scope_error_poison_the_confirmed_shared_unit() {
    for language in [Language::Python, Language::Mise] {
        for error in ["policy", "scope", "policy-without-member-lock"] {
            let temp = tempfile::tempdir().unwrap();
            let directory = temp.path();
            let a = directory.join("packages/a");
            let b = directory.join("packages/b");
            let mut result = result();
            let (manifest_a, lock) = shared_non_cargo_project(&mut result, directory, &a, language);
            let (manifest_b, _) = shared_non_cargo_project(&mut result, directory, &b, language);
            let mut broken = job(&a, language, std::slice::from_ref(&manifest_a));
            if error == "policy-without-member-lock" && language == Language::Python {
                fs::remove_file(a.join("uv.lock")).unwrap();
            }
            if error != "scope" {
                broken.policy = Err("simulated invalid policy".into());
            } else {
                broken.scope = Err("simulated invalid scope".into());
            }
            let plan = plan(vec![
                broken,
                job(&b, language, std::slice::from_ref(&manifest_b)),
            ]);
            let mut tx = InstallTransactions::begin(&plan, &result, directory);
            let mut calls = 0;
            tx.execute(|command| {
                calls += 1;
                fs::write(&lock, "new shared lock").unwrap();
                success(command)
            });
            let report = tx.rollback();
            report.mark_summary(&mut result.summary);
            assert_eq!(calls, 0, "{language} {error}");
            assert_eq!(fs::read_to_string(&lock).unwrap(), "original shared lock");
            assert_eq!(result.summary.total_failed(), 2);
            assert_eq!(result.summary.total_updates(), 0);
            for write in &result.write_results {
                assert_eq!(
                    fs::read(&write.path).unwrap(),
                    write.journal.as_ref().unwrap().before
                );
            }
        }
    }
}

#[test]
fn unverified_uv_root_scope_stops_before_running_and_recovers_all_writes() {
    for workspace in [
        "members = [\"packages/**\"]",
        "members = [\"packages/*\"]\nexclude = [\"packages/**\"]",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let member = directory.join("packages/a");
        let mut result = result();
        let (manifest, lock) =
            shared_non_cargo_project(&mut result, directory, &member, Language::Python);
        let root_manifest = directory.join("pyproject.toml");
        update(
            &mut result,
            &root_manifest,
            Language::Python,
            &format!(
                "[project]\nname = \"root\"\nversion = \"0.1.0\"\ndependencies = [\"example==1.0.0\"]\n[tool.uv.workspace]\n{workspace}\n"
            ),
            "example",
        );
        let mut broken = job(&member, Language::Python, std::slice::from_ref(&manifest));
        broken.policy = Err("simulated invalid policy".into());
        let plan = plan(vec![
            job(
                directory,
                Language::Python,
                std::slice::from_ref(&root_manifest),
            ),
            broken,
        ]);
        let mut tx = InstallTransactions::begin(&plan, &result, directory);
        let mut calls = 0;
        tx.execute(|command| {
            calls += 1;
            fs::write(&lock, "new shared lock including the member requirements").unwrap();
            success(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(calls, 0, "{workspace}");
        assert_eq!(fs::read_to_string(&lock).unwrap(), "original shared lock");
        assert_eq!(result.summary.total_failed(), 2);
        for write in &result.write_results {
            assert_eq!(
                fs::read(&write.path).unwrap(),
                write.journal.as_ref().unwrap().before
            );
        }
    }
}

#[test]
fn uv_member_scope_errors_block_only_the_confirmed_workspace() {
    for relation in ["member", "excluded", "nonmember"] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let child = directory.join(if relation == "nonmember" {
            "other/a"
        } else {
            "packages/a"
        });
        let mut result = result();
        let root_manifest = directory.join("pyproject.toml");
        let exclude = if relation == "excluded" {
            "exclude = [\"packages/a\"]\n"
        } else {
            ""
        };
        update(
            &mut result,
            &root_manifest,
            Language::Python,
            &format!(
                "[project]\nname = \"root\"\nversion = \"0.1.0\"\ndependencies = [\"example==1.0.0\"]\n[tool.uv.workspace]\nmembers = [\"packages/*\"]\n{exclude}"
            ),
            "example",
        );
        let child_manifest = child.join("pyproject.toml");
        update(
            &mut result,
            &child_manifest,
            Language::Python,
            "[project]\nname = \"child\"\nversion = \"0.1.0\"\ndependencies = [\"example==1.0.0\"]\n[tool.uv.workspace]\nmembers = [\"nested/**\"]\n",
            "example",
        );
        let lock = directory.join("uv.lock");
        fs::write(&lock, "original shared lock").unwrap();
        fs::write(child.join("uv.lock"), "original child lock").unwrap();
        let plan = plan(vec![
            job(directory, Language::Python, &[root_manifest]),
            job(&child, Language::Python, &[child_manifest]),
        ]);
        let mut tx = InstallTransactions::begin(&plan, &result, directory);
        let mut calls = vec![];
        tx.execute(|command| {
            calls.push(command.directory.clone());
            fs::write(&lock, "successful root lock").unwrap();
            success(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        let related = relation == "member";
        assert_eq!(
            calls,
            if related {
                vec![]
            } else {
                vec![identity(directory)]
            },
            "{relation}"
        );
        assert_eq!(result.summary.total_failed(), if related { 2 } else { 1 });
        assert_eq!(result.summary.total_updates(), usize::from(!related));
        assert_eq!(
            fs::read_to_string(&lock).unwrap(),
            if related {
                "original shared lock"
            } else {
                "successful root lock"
            }
        );
        for write in &result.write_results {
            if related || write.path.starts_with(&child) {
                assert_eq!(
                    fs::read(&write.path).unwrap(),
                    write.journal.as_ref().unwrap().before
                );
            }
        }
    }
}

#[test]
#[cfg(unix)]
fn mise_preparation_failure_preserves_the_confirmed_shared_unit() {
    for (independent, failure_kind) in [
        (false, "lock"),
        (true, "lock"),
        (false, "config"),
        (true, "config"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("workspace");
        let a = directory.join("packages/a");
        let b = directory.join("packages/b");
        let mut result = result();
        let (manifest_a, lock) =
            shared_non_cargo_project(&mut result, &directory, &a, Language::Mise);
        let (manifest_b, _) = shared_non_cargo_project(&mut result, &directory, &b, Language::Mise);
        if failure_kind == "lock" {
            fs::write(a.join("mise.local.toml"), "[tools]\npython = \"3.0.0\"\n").unwrap();
            std::os::unix::fs::symlink("missing.lock", a.join("mise.local.lock")).unwrap();
        } else {
            fs::write(a.join("mise.local.toml"), "[tools").unwrap();
        }
        let mut jobs = vec![
            job(&a, Language::Mise, std::slice::from_ref(&manifest_a)),
            job(&b, Language::Mise, std::slice::from_ref(&manifest_b)),
        ];
        let standalone = temp.path().join("standalone");
        if independent {
            let manifest = standalone.join("mise.toml");
            update(
                &mut result,
                &manifest,
                Language::Mise,
                "[tools]\nnode = \"1.0.0\"\n",
                "node",
            );
            jobs.push(job(&standalone, Language::Mise, &[manifest]));
        }
        let plan = plan(jobs);
        let mut tx = InstallTransactions::begin(&plan, &result, temp.path());
        let mut calls = vec![];
        tx.execute(|command| {
            calls.push(command.directory.clone());
            if command.directory == identity(&b) {
                fs::write(&lock, "new shared lock").unwrap();
            }
            success(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(
            calls,
            if independent {
                vec![identity(&standalone)]
            } else {
                vec![]
            }
        );
        assert_eq!(fs::read_to_string(&lock).unwrap(), "original shared lock");
        assert_eq!(result.summary.total_failed(), 2);
        assert_eq!(result.summary.total_updates(), usize::from(independent));
        for write in result.write_results.iter().take(2) {
            assert_eq!(
                fs::read(&write.path).unwrap(),
                write.journal.as_ref().unwrap().before
            );
        }
        if failure_kind == "lock" {
            assert!(a.join("mise.local.lock").is_symlink());
        } else {
            assert_eq!(
                fs::read_to_string(a.join("mise.local.toml")).unwrap(),
                "[tools"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn mise_sidecar_directory_links_respect_recovery_boundary() {
    use std::os::unix::fs::PermissionsExt;
    for level in ["root", "tool", "version"] {
        for destination in ["outside", "inside", "dangling"] {
            let temp = tempfile::tempdir().unwrap();
            let directory = temp.path().join("project");
            let target = if destination == "outside" {
                temp.path().join("outside")
            } else {
                directory.join("metadata-target")
            };
            let relative = match level {
                "root" => "python/3.0.0",
                "tool" => "3.0.0",
                _ => "",
            };
            let receipt = target.join(relative);
            let metadata_lock = receipt.join("uv.lock");
            let metadata_manifest = receipt.join("pyproject.toml");
            if destination != "dangling" {
                fs::create_dir_all(&receipt).unwrap();
                fs::write(&metadata_lock, "original metadata lock").unwrap();
                fs::write(&metadata_manifest, "original metadata manifest").unwrap();
                fs::set_permissions(&metadata_lock, fs::Permissions::from_mode(0o600)).unwrap();
                fs::set_permissions(&metadata_manifest, fs::Permissions::from_mode(0o640)).unwrap();
            }
            let mut result = result();
            let manifest = directory.join("mise.toml");
            update(
                &mut result,
                &manifest,
                Language::Mise,
                "[tools]\npython = \"1.0.0\"\n",
                "python",
            );
            let lock = directory.join("mise.lock");
            fs::write(&lock, "original project lock").unwrap();
            let tree = directory.join(".mise/locks");
            let link = match level {
                "root" => tree.clone(),
                "tool" => tree.join("python"),
                _ => tree.join("python/3.0.0"),
            };
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let plan = plan(vec![job(
                &directory,
                Language::Mise,
                std::slice::from_ref(&manifest),
            )]);
            let mut tx = InstallTransactions::begin(&plan, &result, &directory);
            let mut calls = 0;
            tx.execute(|command| {
                calls += 1;
                if destination != "dangling" {
                    let logical = tree.join("python/3.0.0");
                    fs::write(logical.join("uv.lock"), "partially rewritten metadata lock")
                        .unwrap();
                    fs::write(
                        logical.join("pyproject.toml"),
                        "partially rewritten metadata manifest",
                    )
                    .unwrap();
                    fs::set_permissions(logical.join("uv.lock"), fs::Permissions::from_mode(0o644))
                        .unwrap();
                    fs::set_permissions(
                        logical.join("pyproject.toml"),
                        fs::Permissions::from_mode(0o600),
                    )
                    .unwrap();
                }
                failure(command)
            });
            let report = tx.rollback();
            report.mark_summary(&mut result.summary);
            assert_eq!(
                calls,
                usize::from(destination == "inside"),
                "{level} {destination}"
            );
            if destination == "dangling" {
                assert!(!target.exists());
            } else {
                assert_eq!(
                    fs::read_to_string(&metadata_lock).unwrap(),
                    "original metadata lock"
                );
                assert_eq!(
                    fs::read_to_string(&metadata_manifest).unwrap(),
                    "original metadata manifest"
                );
                assert_eq!(
                    fs::metadata(&metadata_lock).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                assert_eq!(
                    fs::metadata(&metadata_manifest)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o640
                );
            }
            assert_eq!(fs::read_to_string(&lock).unwrap(), "original project lock");
            assert_eq!(
                fs::read(&manifest).unwrap(),
                result.write_results[0].journal.as_ref().unwrap().before
            );
            assert_eq!(result.summary.total_failed(), 1);
            assert_eq!(fs::read_link(&link).unwrap(), target);
        }
    }
}

#[test]
#[cfg(unix)]
fn new_or_replaced_mise_directory_links_keep_existing_target_metadata() {
    for level in ["root", "tool", "version"] {
        for (replace, was_link) in [(false, false), (true, false), (true, true)] {
            let temp = tempfile::tempdir().unwrap();
            let directory = temp.path().join("project");
            let target = directory.join("metadata-target");
            let receipt = target.join(match level {
                "root" => "python/3.0.0",
                "tool" => "3.0.0",
                _ => "",
            });
            fs::create_dir_all(&receipt).unwrap();
            let metadata_lock = receipt.join("uv.lock");
            let metadata_manifest = receipt.join("pyproject.toml");
            fs::write(&metadata_lock, "existing target lock").unwrap();
            fs::write(&metadata_manifest, "existing target manifest").unwrap();
            let mut result = result();
            let manifest = directory.join("mise.toml");
            update(
                &mut result,
                &manifest,
                Language::Mise,
                "[tools]\npython = \"1.0.0\"\n",
                "python",
            );
            let lock = directory.join("mise.lock");
            fs::write(&lock, "original project lock").unwrap();
            let tree = directory.join(".mise/locks");
            let link = match level {
                "root" => tree.clone(),
                "tool" => tree.join("python"),
                _ => tree.join("python/3.0.0"),
            };
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            if was_link {
                let original = directory.join("original-target");
                fs::create_dir_all(&original).unwrap();
                std::os::unix::fs::symlink(&original, &link).unwrap();
            } else if replace {
                fs::create_dir(&link).unwrap();
            }
            let plan = plan(vec![job(
                &directory,
                Language::Mise,
                std::slice::from_ref(&manifest),
            )]);
            let mut tx = InstallTransactions::begin(&plan, &result, &directory);
            let mut calls = 0;
            tx.execute(|command| {
                calls += 1;
                if was_link {
                    fs::remove_file(&link).unwrap();
                } else if replace {
                    fs::remove_dir(&link).unwrap();
                }
                std::os::unix::fs::symlink(&target, &link).unwrap();
                failure(command)
            });
            let report = tx.rollback();
            report.mark_summary(&mut result.summary);
            assert_eq!(calls, 1);
            assert_eq!(
                fs::read_to_string(&metadata_lock).unwrap(),
                "existing target lock",
                "{level} replace={replace}"
            );
            assert_eq!(
                fs::read_to_string(&metadata_manifest).unwrap(),
                "existing target manifest"
            );
            assert_eq!(fs::read_to_string(&lock).unwrap(), "original project lock");
            assert_eq!(
                fs::read(&manifest).unwrap(),
                result.write_results[0].journal.as_ref().unwrap().before
            );
            assert_eq!(fs::read_link(&link).unwrap(), target);
            assert_eq!(result.summary.total_failed(), 1);
            for unit in &report.units {
                assert!(!unit.files.contains_key(&identity(&metadata_lock)));
                assert!(!unit.files.contains_key(&identity(&metadata_manifest)));
                for outcome in unit.files.values() {
                    if let FileOutcome::Conflict {
                        backup: Ok(path), ..
                    }
                    | FileOutcome::Failed {
                        backup: Ok(path), ..
                    } = outcome
                    {
                        assert_ne!(path.file_name().unwrap(), "originally-absent");
                        fs::remove_dir_all(path.parent().unwrap()).unwrap();
                    }
                }
            }
            let mut output = vec![];
            report.print(&mut output).unwrap();
            assert!(
                !String::from_utf8(output)
                    .unwrap()
                    .to_ascii_lowercase()
                    .contains("originally absent")
            );
        }
    }
}

#[test]
fn partial_mise_metadata_is_saved_before_a_new_receipt_marker() {
    for (marker, existing) in [
        ("uv.lock", "pyproject.toml"),
        ("aube-lock.yaml", "package.json"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let receipt = directory.join(".mise/locks/example/3.0.0");
        fs::create_dir_all(&receipt).unwrap();
        let metadata = receipt.join(existing);
        fs::write(&metadata, "existing partial metadata").unwrap();
        let mut result = result();
        let manifest = directory.join("mise.toml");
        update(
            &mut result,
            &manifest,
            Language::Mise,
            "[tools]\npython = \"1.0.0\"\n",
            "python",
        );
        let plan = plan(vec![job(
            directory,
            Language::Mise,
            std::slice::from_ref(&manifest),
        )]);
        let mut tx = InstallTransactions::begin(&plan, &result, directory);
        tx.execute(|command| {
            fs::write(receipt.join(marker), "new receipt marker").unwrap();
            fs::write(&metadata, "changed partial metadata").unwrap();
            failure(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(
            fs::read_to_string(&metadata).unwrap(),
            "existing partial metadata"
        );
        assert!(!receipt.join(marker).exists());
        assert_eq!(
            fs::read(&manifest).unwrap(),
            result.write_results[0].journal.as_ref().unwrap().before
        );
        assert_eq!(result.summary.total_failed(), 1);
    }
}

#[test]
fn no_manager_still_reports_observed_manifest_conflicts() {
    use crate::output::OutputFormatter;
    for deleted in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let (manifest, language, content, package) = if deleted {
            (
                directory.join("Gemfile"),
                Language::Ruby,
                "gem 'example', '1.0.0'\n",
                "example",
            )
        } else {
            (
                directory.join("gradle/libs.versions.toml"),
                Language::Java,
                "[libraries]\nexample = 'org.example:library:1.0.0'\n",
                "org.example:library",
            )
        };
        let mut result = result();
        update(&mut result, &manifest, language, content, package);
        let before = result.write_results[0]
            .journal
            .as_ref()
            .unwrap()
            .before
            .clone();
        if deleted {
            fs::remove_file(&manifest).unwrap();
        }
        let plan = plan(vec![job(
            directory,
            language,
            std::slice::from_ref(&manifest),
        )]);
        let mut tx = InstallTransactions::begin(&plan, &result, directory);
        assert_eq!(tx.commands().count(), 0);
        tx.execute(|_| panic!("manager should not be selected"));
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(report.has_failures(), deleted);
        if deleted {
            assert!(!manifest.exists());
            let FileOutcome::Conflict {
                backup: Ok(copy), ..
            } = &report.units[0].files[&identity(&manifest)]
            else {
                panic!("external deletion needs a pre-write copy");
            };
            assert_eq!(fs::read(copy).unwrap(), before);
            let mut output = vec![];
            report.print(&mut output).unwrap();
            assert!(
                String::from_utf8(output)
                    .unwrap()
                    .contains("Pre-write copy")
            );
            fs::remove_dir_all(copy.parent().unwrap()).unwrap();
        } else {
            assert!(fs::read_to_string(&manifest).unwrap().contains("2.0.0"));
        }
        let mut output = vec![];
        crate::output::JsonFormatter::new(crate::output::Verbosity::Normal)
            .format(&result, &mut output)
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["summary"]["failed"], usize::from(deleted));
        assert_eq!(value["summary"]["updates"], usize::from(!deleted));
    }
}

#[test]
#[cfg(unix)]
fn unknown_mise_link_does_not_discard_owned_sibling_metadata() {
    for level in ["tool", "version"] {
        for unknown_first in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let directory = temp.path();
            let tree = directory.join(".mise/locks");
            let stable = tree.join("stable-tool/1.0.0");
            fs::create_dir_all(&stable).unwrap();
            fs::write(stable.join("uv.lock"), "original stable lock").unwrap();
            fs::write(stable.join("pyproject.toml"), "original stable manifest").unwrap();
            let target = directory.join("metadata-target");
            let receipt = target.join(if level == "tool" { "3.0.0" } else { "" });
            fs::create_dir_all(&receipt).unwrap();
            fs::write(receipt.join("uv.lock"), "existing unknown-target lock").unwrap();
            fs::write(
                receipt.join("pyproject.toml"),
                "existing unknown-target manifest",
            )
            .unwrap();
            let unknown_name = if unknown_first {
                "a-unknown"
            } else {
                "z-unknown"
            };
            let owned_name = if unknown_first { "z-owned" } else { "a-owned" };
            let unknown = tree.join(unknown_name);
            let link = if level == "tool" {
                unknown
            } else {
                unknown.join("3.0.0")
            };
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            let owned = tree.join(owned_name).join("2.0.0");
            let mut result = result();
            let manifest = directory.join("mise.toml");
            update(
                &mut result,
                &manifest,
                Language::Mise,
                "[tools]\nnode = \"1.0.0\"\n",
                "node",
            );
            let plan = plan(vec![job(
                directory,
                Language::Mise,
                std::slice::from_ref(&manifest),
            )]);
            let mut tx = InstallTransactions::begin(&plan, &result, directory);
            tx.execute(|command| {
                if unknown_first {
                    std::os::unix::fs::symlink(&target, &link).unwrap();
                }
                fs::create_dir_all(&owned).unwrap();
                fs::write(owned.join("aube-lock.yaml"), "new owned lock").unwrap();
                fs::write(owned.join("package.json"), "new owned manifest").unwrap();
                fs::write(stable.join("uv.lock"), "changed stable lock").unwrap();
                fs::write(stable.join("pyproject.toml"), "changed stable manifest").unwrap();
                if !unknown_first {
                    std::os::unix::fs::symlink(&target, &link).unwrap();
                }
                failure(command)
            });
            let report = tx.rollback();
            report.mark_summary(&mut result.summary);
            assert!(
                !owned.join("aube-lock.yaml").exists(),
                "{level} unknown_first={unknown_first}"
            );
            assert!(!owned.join("package.json").exists());
            assert_eq!(
                fs::read_to_string(stable.join("uv.lock")).unwrap(),
                "original stable lock"
            );
            assert_eq!(
                fs::read_to_string(stable.join("pyproject.toml")).unwrap(),
                "original stable manifest"
            );
            assert_eq!(
                fs::read_to_string(receipt.join("uv.lock")).unwrap(),
                "existing unknown-target lock"
            );
            assert_eq!(
                fs::read_to_string(receipt.join("pyproject.toml")).unwrap(),
                "existing unknown-target manifest"
            );
            assert_eq!(fs::read_link(&link).unwrap(), target);
            assert_eq!(
                fs::read(&manifest).unwrap(),
                result.write_results[0].journal.as_ref().unwrap().before
            );
            assert_eq!(result.summary.total_failed(), 1);
        }
    }
}

#[test]
fn uv_policy_error_keeps_excluded_and_nested_projects_independent() {
    for nested in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let a = directory.join("packages/a");
        let b = directory.join("packages/b");
        let mut result = result();
        let (manifest_a, lock) =
            shared_non_cargo_project(&mut result, directory, &a, Language::Python);
        let (manifest_b, _) =
            shared_non_cargo_project(&mut result, directory, &b, Language::Python);
        if nested {
            result.summary.manifests.pop();
            result.write_results.pop();
            update(
                &mut result,
                &manifest_b,
                Language::Python,
                "[project]\nname = \"example\"\nversion = \"0.1.0\"\ndependencies = [\"example==1.0.0\"]\n[tool.uv.workspace]\nmembers = []\n",
                "example",
            );
        } else {
            fs::write(
                directory.join("pyproject.toml"),
                "[tool.uv.workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/b\"]\n",
            )
            .unwrap();
        }
        let mut broken = job(&a, Language::Python, std::slice::from_ref(&manifest_a));
        broken.policy = Err("simulated policy failure".into());
        let plan = plan(vec![
            broken,
            job(&b, Language::Python, std::slice::from_ref(&manifest_b)),
        ]);
        let mut tx = InstallTransactions::begin(&plan, &result, directory);
        let mut calls = vec![];
        tx.execute(|command| {
            calls.push(command.directory.clone());
            fs::write(b.join("uv.lock"), "successful independent lock").unwrap();
            success(command)
        });
        let report = tx.rollback();
        report.mark_summary(&mut result.summary);
        assert_eq!(calls, vec![identity(&b)]);
        assert_eq!(fs::read_to_string(&lock).unwrap(), "original shared lock");
        assert_eq!(
            fs::read_to_string(b.join("uv.lock")).unwrap(),
            "successful independent lock"
        );
        assert_eq!(result.summary.total_failed(), 1);
        assert_eq!(result.summary.total_updates(), 1);
    }
}
