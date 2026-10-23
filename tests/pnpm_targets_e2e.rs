//! pnpm が成功終了しても、直接依存の未達の更新を見逃さない。
#![cfg(unix)]

mod support;

use serde_json::{Value, json};
use std::fs;
use std::process::Command;

#[test]
fn pnpm_stale_final_lock_reports_failure_in_every_output_format() {
    let project = tempfile::tempdir().unwrap();
    let commands = tempfile::tempdir().unwrap();
    let program = commands.path().join("pnpm");
    let graph = json!([{
        "path": project.path(),
        "dependencies": {
            "ms": {
                "from": "ms",
                "version": "2.1.2",
                "resolved": "https://registry.npmjs.org/ms/-/ms-2.1.2.tgz"
            }
        }
    }]);
    let graph = serde_json::to_string(&graph)
        .unwrap()
        .replace('\'', "'\\''");
    support::write_script(
        &program,
        &format!(
            "#!/bin/sh\ncase \"$1\" in\nconfig) printf null;;\ninstall|update) exit 0;;\nlist) printf '%s' '{graph}';;\n--version) printf '12.9.1';;\n*) exit 1;;\nesac\n"
        ),
        0o755,
    );
    let mut paths = vec![commands.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).unwrap();
    let before = "lockfileVersion: '9.0'\n";
    fs::write(project.path().join("pnpm-lock.yaml"), before).unwrap();

    for format in [Some("--json"), Some("--quiet"), Some("--diff"), None] {
        fs::write(
            project.path().join("package.json"),
            r#"{"name":"example","private":true,"dependencies":{"ms":"^2.1.1"}}"#,
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_depup"));
        command
            .args(["--node", "--install", "--no-age", "--no-osv"])
            .env("PATH", &path)
            .env("NO_COLOR", "1")
            .arg(project.path());
        if let Some(format) = format {
            command.arg(format);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{format:?}: {stdout}\n{stderr}"
        );
        assert!(
            stderr.contains("below the reported target"),
            "{format:?}: {stderr}"
        );
        if format == Some("--json") {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["summary"]["updates"], 0, "{value}");
            assert_eq!(value["summary"]["failed"], 1, "{value}");
        } else {
            assert!(stdout.contains("1 failed"), "{format:?}: {stdout}");
            assert!(
                !stdout.contains("1 package(s) updated"),
                "{format:?}: {stdout}"
            );
            assert!(!stdout.contains("1 updated"), "{format:?}: {stdout}");
        }
        assert_eq!(
            fs::read_to_string(project.path().join("pnpm-lock.yaml")).unwrap(),
            before
        );
    }
}
