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
            .args([
                "--mise",
                "--only",
                "python",
                "--no-osv",
                "--max-change",
                "major",
                "--verbose",
                "--json",
            ]);
        command
    }

    fn run(&self, command: &mut Command) -> serde_json::Value {
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid JSON: {error}: {output:?}"))
    }
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
