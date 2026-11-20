//! 実際の Git を使い、ローカルの取得元だけでサブモジュールの更新を検証する。

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const SUBMODULE: &str = "third_party/test module";
const OTHER: &str = "vendor/other";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Depup Test",
            "-c",
            "user.email=depup@example.com",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

struct Fixture {
    _temp: TempDir,
    upstream: PathBuf,
    parent: PathBuf,
    old: String,
    new: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let upstream = temp.path().join("upstream");
        let parent = temp.path().join("parent");
        for dir in [&upstream, &parent] {
            fs::create_dir(dir).unwrap();
            git(dir, &["init", "--initial-branch=main"]);
        }
        fs::write(upstream.join("data.txt"), "old\n").unwrap();
        git(&upstream, &["add", "data.txt"]);
        git(&upstream, &["commit", "-m", "Initial upstream"]);
        let old = git(&upstream, &["rev-parse", "HEAD"]);
        for (name, path) in [("source-alias", SUBMODULE), ("other", OTHER)] {
            git(
                &parent,
                &["submodule", "add", "--name", name, "../upstream", path],
            );
        }
        git(&parent, &["commit", "-m", "Record submodules"]);
        git(&parent, &["submodule", "update"]);
        fs::write(upstream.join("data.txt"), "new\n").unwrap();
        git(&upstream, &["commit", "-am", "Advance upstream"]);
        let new = git(&upstream, &["rev-parse", "HEAD"]);
        Self {
            _temp: temp,
            upstream,
            parent,
            old,
            new,
        }
    }

    fn depup(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_depup"))
            .current_dir(&self.parent)
            .arg("--submodules")
            .args(args)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str], exit_code: i32) -> Value {
        let mut flags = vec!["--json"];
        flags.extend_from_slice(args);
        let output = self.depup(&flags);
        assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn head(&self, path: &str) -> String {
        git(&self.parent.join(path), &["rev-parse", "HEAD"])
    }
}

#[test]
fn preview_and_update_gitlinks_in_a_subdirectory_without_staging() {
    let fixture = Fixture::new();
    let modules = fs::read(fixture.parent.join(".gitmodules")).unwrap();
    let preview = fixture.json(&["--dry-run", "third_party"], 0);
    assert_eq!(preview["summary"]["updates"], 1);
    assert_eq!(preview["submodules"][0]["path"], SUBMODULE);
    assert_eq!(preview["submodules"][0]["from"], fixture.old);
    assert_eq!(preview["submodules"][0]["to"], fixture.new);
    assert_eq!(preview["submodules"][0]["reference"], "HEAD");
    assert_eq!(fixture.head(SUBMODULE), fixture.old);
    assert_eq!(git(&fixture.parent, &["status", "--porcelain"]), "");

    let diff = fixture.depup(&["--dry-run", "--diff", "--only", SUBMODULE]);
    assert!(diff.status.success(), "{diff:?}");
    let diff = String::from_utf8(diff.stdout).unwrap();
    assert!(diff.contains(&format!("-Subproject commit {}", fixture.old)));
    assert!(diff.contains(&format!("+Subproject commit {}", fixture.new)));

    let updated = fixture.json(&["--only", SUBMODULE, "--exclude", SUBMODULE], 0);
    assert_eq!(updated["summary"]["updates"], 1);
    assert_eq!(updated["submodules"][0]["status"], "updated");
    assert_eq!(fixture.head(SUBMODULE), fixture.new);
    assert_eq!(fixture.head(OTHER), fixture.old);
    assert_eq!(
        git(&fixture.parent, &["diff", "--cached", "--name-only"]),
        ""
    );
    assert_eq!(
        fs::read(fixture.parent.join(".gitmodules")).unwrap(),
        modules
    );
    assert_eq!(
        fs::read_to_string(fixture.parent.join(SUBMODULE).join("data.txt"))
            .unwrap()
            .trim(),
        "new"
    );

    let repeated = fixture.json(&["--only", SUBMODULE], 0);
    assert_eq!(repeated["summary"]["updates"], 0);
    assert_eq!(
        repeated["submodules"][0]["reason"],
        "checked-out commit differs from the recorded pointer"
    );
}

#[test]
fn honors_parent_branch_configuration_and_preserves_local_state() {
    let fixture = Fixture::new();
    let parent = &fixture.parent;
    let module = parent.join(SUBMODULE);
    git(&fixture.upstream, &["branch", "held", &fixture.old]);
    git(
        parent,
        &[
            "config",
            "--file",
            ".gitmodules",
            "submodule.source-alias.branch",
            "held",
        ],
    );
    git(parent, &["add", ".gitmodules"]);
    git(
        parent,
        &["commit", "-m", "Track held branch in .gitmodules"],
    );
    assert_eq!(
        fixture.json(&["--dry-run", "--only", SUBMODULE], 0)["summary"]["skips"],
        1
    );
    git(parent, &["config", "submodule.source-alias.branch", "."]);
    let preview = fixture.json(&["--dry-run", "--only", SUBMODULE], 0);
    assert_eq!(preview["submodules"][0]["reference"], "refs/heads/main");
    assert_eq!(preview["submodules"][0]["to"], fixture.new);

    fs::write(module.join("local.txt"), "local work").unwrap();
    git(parent, &["submodule", "deinit", "--", OTHER]);
    let skipped = fixture.json(&[], 0);
    assert_eq!(skipped["summary"]["skips"], 2);
    assert_eq!(
        skipped["submodules"][0]["reason"],
        "working tree has local changes"
    );
    assert!(
        skipped["submodules"][1]["reason"]
            .as_str()
            .unwrap()
            .contains("uninitialized")
    );
    assert_eq!(fixture.head(SUBMODULE), fixture.old);
    assert_eq!(
        fs::read_to_string(module.join("local.txt")).unwrap(),
        "local work"
    );
    fs::remove_file(module.join("local.txt")).unwrap();

    git(
        parent,
        &["config", "submodule.source-alias.branch", "missing-branch"],
    );
    let failed = fixture.json(&["--only", SUBMODULE], 2);
    assert_eq!(failed["summary"]["failed"], 1);
    assert_eq!(failed["submodules"][0]["status"], "failed");
    assert_eq!(fixture.head(SUBMODULE), fixture.old);

    git(parent, &["config", "submodule.source-alias.branch", "main"]);
    assert_eq!(
        fixture.json(&["--only", SUBMODULE], 0)["summary"]["updates"],
        1
    );
    git(parent, &["add", "--", SUBMODULE]);
    let staged = fixture.json(&["--only", SUBMODULE], 0);
    assert_eq!(
        staged["submodules"][0]["reason"],
        "submodule pointer has staged changes"
    );
    git(parent, &["commit", "-m", "Record new submodule pointer"]);
    git(parent, &["config", "submodule.source-alias.branch", "held"]);
    let rewind = fixture.json(&["--only", SUBMODULE], 0);
    assert_eq!(
        rewind["submodules"][0]["reason"],
        "remote commit is not a verified fast-forward"
    );
    assert_eq!(fixture.head(SUBMODULE), fixture.new);
}
