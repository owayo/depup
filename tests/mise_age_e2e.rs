//! mise の age 設定と公開日不明の候補を、ネットワークなしで CLI 全体から検証する。
#![cfg(unix)]

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct MiseProject {
    _root: tempfile::TempDir,
    project: PathBuf,
    bin: PathBuf,
    config: PathBuf,
}

impl MiseProject {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let bin = root.path().join("bin");
        let config = root.path().join(".config");
        for dir in [&project, &bin, &config] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(project.join("mise.toml"), "[tools]\npython = '3.14.7'\n").unwrap();
        fs::create_dir_all(config.join("mise")).unwrap();
        fs::write(config.join("mise/config.toml"), "").unwrap();
        fs::write(config.join("custom.toml"), "").unwrap();
        let script = bin.join("mise");
        support::write_script(
            &script,
            r#"#!/bin/sh
case "$1" in
    ls-remote)
        printf '%s\n' "$@" "$MISE_MINIMUM_RELEASE_AGE" > invocation
        printf '%s\n' '[{"version":"3.14.7","created_at":"2020-01-01T00:00:00Z"},{"version":"3.15.0"}]'
        ;;
    install) printf '%s\n' "$@" "$MISE_MINIMUM_RELEASE_AGE" > installed-age ;;
    *) exit 1 ;;
esac
"#,
            0o700,
        );
        Self {
            _root: root,
            project,
            bin,
            config,
        }
    }

    fn command(&self) -> Command {
        let mut command = self.command_for("python");
        command.args(["--max-change", "major", "--json"]);
        command
    }

    fn command_for(&self, tool: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_depup"));
        command
            .env_clear()
            .env(
                "PATH",
                std::env::join_paths([
                    self.bin.as_path(),
                    std::path::Path::new("/usr/bin"),
                    std::path::Path::new("/bin"),
                ])
                .unwrap(),
            )
            .env("XDG_CONFIG_HOME", &self.config)
            .env(
                "MISE_GLOBAL_CONFIG_FILE",
                self.config.join("mise/config.toml"),
            )
            .current_dir(&self.project)
            .args(["--mise", "--only", tool, "--no-osv", "--verbose"]);
        command
    }

    fn stub_package_manager_versions(&self) {
        support::write_script(
            &self.bin.join("mise"),
            r#"#!/bin/sh
case "$1" in
    ls-remote)
        if [ -n "${EDIT_PACKAGE_MANAGER:-}" ]; then
            printf '%s' "$EDIT_PACKAGE_MANAGER" > package.json
        fi
        printf '%s\n' '[{"version":"12.15.0"},{"version":"12.18.0"}]'
        ;;
    install) cp package.json during-install.json && [ "${FAIL_INSTALL:-}" != 1 ] ;;
    *) exit 1 ;;
esac
"#,
            0o700,
        );
    }

    fn run(&self, command: &mut Command) -> serde_json::Value {
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid JSON: {error}: {output:?}"))
    }
}

#[test]
fn mise_install_targets_a_member_from_the_parent_project() {
    let project = MiseProject::new();
    let member = project.project.join("sub");
    fs::create_dir(&member).unwrap();
    let original = "[tools]\npython = '3.14.7'\n";
    fs::write(member.join("mise.toml"), original).unwrap();

    let report =
        project.run(
            project
                .command()
                .args(["--no-age", "--include-pinned", "--install", "sub"]),
        );
    assert_eq!(report["summary"]["updates"], 1);
    assert!(member.join("installed-age").is_file());
    assert!(!project.project.join("installed-age").exists());
    assert_eq!(
        fs::read_to_string(member.join("mise.toml")).unwrap(),
        original.replace("3.14.7", "3.15.0")
    );
    assert_eq!(
        fs::read_to_string(project.project.join("mise.toml")).unwrap(),
        original
    );
}

#[test]
fn mise_python_install_respects_the_project_requirement() {
    let project = MiseProject::new();
    let manifest = project.project.join("mise.toml");
    let pyproject = project.project.join("pyproject.toml");
    let requirement = "[project]\nrequires-python = '>=3.14,<3.15'\n";
    fs::write(&pyproject, requirement).unwrap();
    fs::write(&manifest, "[tools]\npython = '3.14.6'\n").unwrap();

    let report = project.run(project.command().args(["--no-age", "--install"]));
    assert_eq!(report["manifests"][0]["updates"][0]["to"], "3.14.7");
    assert!(project.project.join("installed-age").is_file());
    let expected = "[tools]\npython = '3.14.7'\n";
    assert_eq!(fs::read_to_string(&manifest).unwrap(), expected);
    assert_eq!(fs::read_to_string(&pyproject).unwrap(), requirement);

    let report = project.run(project.command().args(["--no-age", "--install"]));
    assert_eq!(report["summary"]["updates"], 0);
    assert_eq!(fs::read_to_string(&manifest).unwrap(), expected);
}

#[test]
fn mise_package_manager_is_synchronized_before_install() {
    let project = MiseProject::new();
    let manifest = project.project.join("mise.toml");
    let package = project.project.join("package.json");
    let original = "\u{feff}{\r\n\t\"packageManager\": \"pnpm@12.15.0\",\r\n\t\"nested\": {\"packageManager\": \"pnpm@12.15.0\"}\r\n}";
    let expected = original.replacen("pnpm@12.15.0", "pnpm@12.18.0", 1);
    fs::write(&manifest, "[tools]\npnpm = '12.15.0'\n").unwrap();
    fs::write(&package, original).unwrap();
    project.stub_package_manager_versions();

    let preview = project
        .command_for("pnpm")
        .args(["--no-age", "--dry-run", "--diff"])
        .output()
        .unwrap();
    assert!(preview.status.success(), "{preview:?}");
    let diff = String::from_utf8(preview.stdout).unwrap();
    assert!(
        diff.contains("+  \"packageManager\": \"pnpm@12.18.0\""),
        "{diff}"
    );
    assert_eq!(fs::read_to_string(&package).unwrap(), original);

    let failed = project
        .command_for("pnpm")
        .args(["--no-age", "--install", "--json"])
        .env("FAIL_INSTALL", "1")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1), "{failed:?}");
    let report: serde_json::Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert!(report.get("package_manager_syncs").is_none(), "{report}");
    assert_eq!(
        fs::read_to_string(project.project.join("during-install.json")).unwrap(),
        expected
    );
    assert_eq!(fs::read_to_string(&package).unwrap(), original);
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        "[tools]\npnpm = '12.15.0'\n"
    );

    let report = project.run(
        project
            .command_for("pnpm")
            .args(["--no-age", "--install", "--json"]),
    );
    assert_eq!(report["summary"]["updates"], 1);
    assert_eq!(report["package_manager_syncs"][0]["from"], "pnpm@12.15.0");
    assert_eq!(report["package_manager_syncs"][0]["to"], "pnpm@12.18.0");
    assert_eq!(
        fs::read_to_string(&manifest).unwrap(),
        "[tools]\npnpm = '12.18.0'\n"
    );
    assert_eq!(fs::read_to_string(&package).unwrap(), expected);
    assert_eq!(
        fs::read_to_string(project.project.join("during-install.json")).unwrap(),
        expected
    );
}

#[test]
fn mise_package_manager_keeps_checksummed_and_unrelated_fields() {
    let project = MiseProject::new();
    project.stub_package_manager_versions();
    let manifest = project.project.join("mise.toml");
    let package = project.project.join("package.json");
    for (pin, warning) in [("pnpm@12.15.0+sha512.abcdef", true), ("yarn@4.5.0", false)] {
        let original = format!("{{\"packageManager\":\"{pin}\"}}");
        fs::write(&manifest, "[tools]\npnpm = '12.15.0'\n").unwrap();
        fs::write(&package, &original).unwrap();
        let report = project.run(project.command_for("pnpm").args(["--no-age", "--json"]));
        assert_eq!(report["summary"]["updates"], 1);
        assert!(report.get("package_manager_syncs").is_none(), "{report}");
        assert_eq!(
            report["errors"].to_string().contains("checksum"),
            warning,
            "{report}"
        );
        assert_eq!(fs::read_to_string(&package).unwrap(), original);
    }
}

#[test]
fn mise_package_manager_uses_the_project_where_the_config_was_detected() {
    let project = MiseProject::new();
    project.stub_package_manager_versions();
    let member = project.project.join(".config");
    fs::create_dir(&member).unwrap();
    let config = member.join("mise.toml");
    let config_before = "[tools]\npnpm = '12.15.0'\n";
    let package_before = r#"{"packageManager":"pnpm@12.15.0"}"#;
    fs::write(&config, config_before).unwrap();
    fs::write(member.join("package.json"), package_before).unwrap();
    fs::write(project.project.join("package.json"), package_before).unwrap();

    project.run(
        project
            .command_for("pnpm")
            .args(["--no-age", "--json", ".config"]),
    );
    assert_eq!(
        fs::read_to_string(member.join("package.json")).unwrap(),
        package_before.replace("12.15.0", "12.18.0")
    );
    assert_eq!(
        fs::read_to_string(project.project.join("package.json")).unwrap(),
        package_before
    );

    fs::write(&config, config_before).unwrap();
    project.run(project.command_for("pnpm").args(["--no-age", "--json"]));
    assert_eq!(
        fs::read_to_string(project.project.join("package.json")).unwrap(),
        package_before.replace("12.15.0", "12.18.0")
    );
}

#[test]
fn mise_package_manager_respects_an_overlay_and_edits_during_version_lookup() {
    let project = MiseProject::new();
    project.stub_package_manager_versions();
    let manifest = project.project.join("mise.toml");
    let package = project.project.join("package.json");
    let overlay = project.project.join("mise.local.toml");
    let config = "[tools]\npnpm = '12.15.0'\n";
    let original = r#"{"packageManager":"pnpm@12.15.0"}"#;
    fs::write(&manifest, config).unwrap();
    fs::write(&package, original).unwrap();
    fs::write(&overlay, "[tools]\npnpm = 'latest'\n").unwrap();
    let report = project.run(project.command_for("pnpm").args(["--no-age", "--json"]));
    assert!(
        report["errors"]
            .to_string()
            .contains("multiple mise declarations"),
        "{report}"
    );
    assert_eq!(fs::read_to_string(&package).unwrap(), original);

    fs::remove_file(&overlay).unwrap();
    fs::write(&manifest, config).unwrap();
    let edited = "{\"packageManager\":\"pnpm@12.16.0\"}\n";
    let report = project.run(
        project
            .command_for("pnpm")
            .args(["--no-age", "--json"])
            .env("EDIT_PACKAGE_MANAGER", edited),
    );
    assert!(
        report["errors"]
            .to_string()
            .contains("packageManager changed"),
        "{report}"
    );
    assert_eq!(fs::read_to_string(&package).unwrap(), edited);

    fs::write(&manifest, config).unwrap();
    let older = r#"{"packageManager":"pnpm@11.0.0"}"#;
    fs::write(&package, older).unwrap();
    let report = project.run(project.command_for("pnpm").args([
        "--no-age",
        "--json",
        "--max-change",
        "minor",
    ]));
    assert!(
        report["errors"]
            .to_string()
            .contains("would exceed --max-change=minor"),
        "{report}"
    );
    assert_eq!(fs::read_to_string(&package).unwrap(), older);
}

#[test]
fn mise_no_age_accepts_unknown_dates_without_native_constraints() {
    let project = MiseProject::new();
    let report = project.run(project.command().args(["--no-age", "--dry-run"]));
    assert_eq!(report["summary"]["updates"], 1);
    assert_eq!(report["manifests"][0]["updates"][0]["to"], "3.15.0");
    assert!(
        report["manifests"][0]["updates"][0]
            .get("released_at")
            .is_none()
    );
    let invocation = fs::read_to_string(project.project.join("invocation")).unwrap();
    assert_eq!(
        invocation,
        "ls-remote\n--json\n--minimum-release-age\n0\n--\npython\n0\n"
    );
    assert_eq!(
        fs::read_to_string(project.project.join("mise.toml")).unwrap(),
        "[tools]\npython = '3.14.7'\n"
    );
}

#[test]
fn mise_no_age_overrides_user_age_for_unknown_dates() {
    for source in ["environment", "global"] {
        for install in [false, true] {
            let project = MiseProject::new();
            let mut command = project.command();
            match source {
                "environment" => {
                    command.env("MISE_MINIMUM_RELEASE_AGE", "14d");
                }
                "global" => fs::write(
                    project.config.join("mise/config.toml"),
                    "[settings]\nminimum_release_age = '14d'\n",
                )
                .unwrap(),
                _ => unreachable!(),
            }
            command.arg("--no-age");
            if install {
                command.args(["--install", "--include-pinned"]);
            } else {
                command.arg("--dry-run");
            }
            let report = project.run(&mut command);
            assert_eq!(
                report["summary"]["updates"], 1,
                "{source}, install={install}: {report}"
            );
            assert_eq!(report["manifests"][0]["updates"][0]["to"], "3.15.0");
            let invocation = fs::read_to_string(project.project.join("invocation")).unwrap();
            assert_eq!(
                invocation, "ls-remote\n--json\n--minimum-release-age\n0\n--\npython\n0\n",
                "{source}"
            );
            if install {
                assert_eq!(
                    fs::read_to_string(project.project.join("installed-age")).unwrap(),
                    "install\n--minimum-release-age\n0\n0\n",
                    "{source}"
                );
                assert!(
                    fs::read_to_string(project.project.join("mise.toml"))
                        .unwrap()
                        .contains("3.15.0")
                );
            }
        }
    }
}

#[test]
fn mise_explicit_project_age_and_normal_runs_still_reject_unknown_dates() {
    for source in [
        "cli",
        "environment",
        "global",
        "global-tool",
        "project",
        "local",
        "tool",
        "custom",
        "overlay",
    ] {
        let project = MiseProject::new();
        let mut command = project.command();
        command.arg("--dry-run");
        if source == "cli" {
            command.args(["--age", "14d"]);
        } else {
            if matches!(source, "environment" | "global") {
                command.args(["--age", "1d"]);
            } else {
                command.arg("--no-age");
            }
            match source {
                "global-tool" => fs::write(
                    project.config.join("mise/config.toml"),
                    "[settings]\nminimum_release_age = '14d'\n[tools]\npython = { version = '3.14.7', minimum_release_age = '30d' }\n",
                ).unwrap(),
                "project" => fs::write(
                    project.project.join("mise.toml"),
                    "[tools]\npython = '3.14.7'\n[settings]\nminimum_release_age = '14d'\n",
                )
                .unwrap(),
                "environment" => {
                    command.env("MISE_MINIMUM_RELEASE_AGE", "14d");
                }
                "global" => fs::write(
                    project.config.join("mise/config.toml"),
                    "[settings]\nminimum_release_age = '14d'\n",
                )
                .unwrap(),
                "local" => fs::write(
                    project.project.join("mise.local.toml"),
                    "[settings]\nminimum_release_age = '14d'\n",
                )
                .unwrap(),
                "tool" => fs::write(
                    project.project.join("mise.toml"),
                    "[tools]\npython = { version = '3.14.7', minimum_release_age = '14d' }\n",
                )
                .unwrap(),
                "custom" => {
                    command.env("MISE_CONFIG_FILE", project.config.join("custom.toml"));
                    fs::write(
                        project.config.join("custom.toml"),
                        "[settings]\nminimum_release_age = '14d'\n",
                    )
                    .unwrap();
                }
                "overlay" => {
                    command.env("MISE_ENV", "test");
                    fs::write(
                        project.project.join("mise.test.toml"),
                        "[settings]\nminimum_release_age = '14d'\n",
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
        }
        let report = project.run(&mut command);
        assert_eq!(report["summary"]["updates"], 0, "{source}: {report}");
        assert_eq!(
            report["manifests"][0]["skips"][0]["reason"], "release_date_unknown: 3.15.0",
            "{source}"
        );
        let invocation = fs::read_to_string(project.project.join("invocation")).unwrap();
        assert!(
            !invocation.lines().any(|line| line == "0"),
            "{source}: {invocation}"
        );
    }
}

#[test]
fn mise_no_age_does_not_reclassify_global_settings_under_the_run_root() {
    let project = MiseProject::new();
    fs::write(project._root.path().join(".depup"), "project\n").unwrap();
    fs::write(
        project.config.join("mise/config.toml"),
        "[settings]\nminimum_release_age = '14d'\n",
    )
    .unwrap();
    let report = project.run(
        project
            .command()
            .arg(project._root.path())
            .args(["--no-age", "--dry-run"]),
    );
    assert_eq!(report["summary"]["updates"], 1, "{report}");
}
