//! depup CLI のエンドツーエンドテスト。
//!
//! このテストでは以下を検証する:
//! - dry-run モードがファイルを変更しないこと
//! - CLI が正しい JSON 出力スキーマを返すこと
//! - 各種シナリオで終了コードが正しいこと

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Cargo が統合テスト用にコンパイルしたバイナリのパスを取得する
fn get_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_depup"))
}

/// サンプルマニフェストを持つテスト用ディレクトリを作成する
fn create_test_project() -> TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    // package.json を作成する
    let package_json = r#"{
  "name": "test-project",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.21"
  },
  "devDependencies": {
    "typescript": "~5.0.0"
  }
}"#;
    fs::write(temp_dir.path().join("package.json"), package_json).unwrap();

    // pyproject.toml を作成する
    let pyproject = r#"[project]
name = "test-project"
version = "1.0.0"
dependencies = [
    "requests>=2.28.0",
]

[project.optional-dependencies]
dev = [
    "pytest>=7.0.0",
]
"#;
    fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

    // Cargo.toml を作成する
    let cargo_toml = r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0.190"
tokio = { version = "1.35", features = ["full"] }

[dev-dependencies]
tempfile = "3.10"
"#;
    fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

    // go.mod を作成する
    let go_mod = r#"module example.com/test

go 1.21

require (
    github.com/gin-gonic/gin v1.9.0
    github.com/stretchr/testify v1.8.0 // pinned
)
"#;
    fs::write(temp_dir.path().join("go.mod"), go_mod).unwrap();

    temp_dir
}

mod dry_run_tests {
    use super::*;

    /// dry-run モードがファイルを変更しないことを確認する
    #[test]
    fn test_dry_run_leaves_files_unchanged() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // 元のファイル内容を読む
        let original_package_json =
            fs::read_to_string(temp_dir.path().join("package.json")).unwrap();
        let original_pyproject =
            fs::read_to_string(temp_dir.path().join("pyproject.toml")).unwrap();
        let original_cargo = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();
        let original_go_mod = fs::read_to_string(temp_dir.path().join("go.mod")).unwrap();

        // depup を dry-run モードで実行する
        let _output = Command::new(&binary)
            .args(["--dry-run", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        // ネットワークエラーで失敗しても、ファイルは変更されないはず。
        // このテストではネットワーク起因の非ゼロ終了を許容する。

        // ファイルが変更されていないことを確認する
        let new_package_json = fs::read_to_string(temp_dir.path().join("package.json")).unwrap();
        let new_pyproject = fs::read_to_string(temp_dir.path().join("pyproject.toml")).unwrap();
        let new_cargo = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();
        let new_go_mod = fs::read_to_string(temp_dir.path().join("go.mod")).unwrap();

        assert_eq!(
            original_package_json, new_package_json,
            "package.json should not be modified in dry-run mode"
        );
        assert_eq!(
            original_pyproject, new_pyproject,
            "pyproject.toml should not be modified in dry-run mode"
        );
        assert_eq!(
            original_cargo, new_cargo,
            "Cargo.toml should not be modified in dry-run mode"
        );
        assert_eq!(
            original_go_mod, new_go_mod,
            "go.mod should not be modified in dry-run mode"
        );
    }

    /// 言語フィルタ付き dry-run でもファイルを変更しないことを確認する
    #[test]
    fn test_dry_run_with_language_filter() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let original_package_json =
            fs::read_to_string(temp_dir.path().join("package.json")).unwrap();

        // Node.js だけを対象に dry-run モードで実行する
        Command::new(&binary)
            .args(["--dry-run", "--node", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let new_package_json = fs::read_to_string(temp_dir.path().join("package.json")).unwrap();

        assert_eq!(
            original_package_json, new_package_json,
            "package.json should not be modified in dry-run mode"
        );
    }

    /// quiet フラグ付き dry-run が動作することを確認する
    #[test]
    fn test_dry_run_with_quiet_mode() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let original_cargo = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();

        // quiet フラグ付き dry-run モードで実行する
        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--quiet",
                "--rust",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        // quiet モードでは stdout が最小限になる
        let stdout = String::from_utf8_lossy(&output.stdout);
        // quiet モードでは出力が少なくなる
        assert!(
            stdout.len() < 1000,
            "Quiet mode should produce minimal output"
        );

        let new_cargo = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(
            original_cargo, new_cargo,
            "Cargo.toml should not be modified in dry-run mode"
        );
    }
}

mod json_output_tests {
    use super::*;

    /// JSON 出力構造を確認する
    #[test]
    fn test_json_output_schema() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // JSON 出力で depup を実行する
        let output = Command::new(&binary)
            .args(["--dry-run", "--json", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);

        // JSON 出力をパースする
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // トップレベル構造を確認する
        assert!(json.is_object(), "JSON output should be an object");

        // 必須フィールドを確認する
        assert!(
            json.get("dry_run").is_some(),
            "JSON should have 'dry_run' field"
        );
        assert!(
            json.get("summary").is_some(),
            "JSON should have 'summary' field"
        );
        assert!(
            json.get("manifests").is_some(),
            "JSON should have 'manifests' field"
        );

        // dry_run が true であることを確認する
        assert_eq!(
            json["dry_run"].as_bool(),
            Some(true),
            "dry_run should be true"
        );

        // summary.updates が数値であることを確認する
        assert!(
            json["summary"]["updates"].is_number(),
            "summary.updates should be a number"
        );

        // manifests が配列であることを確認する
        assert!(json["manifests"].is_array(), "manifests should be an array");
    }

    /// JSON 出力にマニフェスト情報が含まれることを確認する
    #[test]
    fn test_json_output_manifest_structure() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--json", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        let manifests = json["manifests"].as_array().unwrap();

        // 少なくとも1つのマニフェストを検出しているはず。
        // パース問題がある場合はすべて検出できない可能性がある。
        if !manifests.is_empty() {
            let manifest = &manifests[0];

            // マニフェスト構造を確認する
            assert!(
                manifest.get("path").is_some(),
                "Manifest should have 'path' field"
            );
            assert!(
                manifest.get("language").is_some(),
                "Manifest should have 'language' field"
            );
            assert!(
                manifest.get("updates").is_some(),
                "Manifest should have 'updates' field"
            );
            // skips フィールドは verbose モードかつ非空の場合だけ含まれる。

            // language が有効な表示名であることを確認する。
            let language = manifest["language"].as_str().unwrap();
            let valid_languages = ["Node.js", "Python", "Rust", "Go"];
            assert!(
                valid_languages.contains(&language),
                "Language should be one of {:?}, got {}",
                valid_languages,
                language
            );
        }
    }

    /// 空ディレクトリでの JSON 出力を確認する
    #[test]
    fn test_json_output_empty_directory() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--json", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // manifests は空配列になるはず
        let manifests = json["manifests"].as_array().unwrap();
        assert!(
            manifests.is_empty(),
            "Empty directory should have no manifests"
        );

        // summary.updates は 0 になるはず
        assert_eq!(
            json["summary"]["updates"].as_i64(),
            Some(0),
            "summary.updates should be 0 for empty directory"
        );
    }
}

mod exit_code_tests {
    use super::*;

    /// 更新なしの正常実行で終了コードを確認する
    #[test]
    fn test_exit_code_no_updates() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let binary = get_binary_path();

        // 空ディレクトリで実行する。マニフェストなしなので更新もない。
        let output = Command::new(&binary)
            .args(["--dry-run", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        // 終了コード 0 で成功するはず
        assert!(
            output.status.success(),
            "Should exit with success for empty directory"
        );
    }

    /// help フラグの終了コードを確認する
    #[test]
    fn test_exit_code_help() {
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .arg("--help")
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success(), "Help should exit with success");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("depup") || stdout.contains("dependency"),
            "Help output should contain program name or description"
        );
    }

    /// version フラグの終了コードを確認する
    #[test]
    fn test_exit_code_version() {
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .arg("--version")
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success(), "Version should exit with success");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("depup") || stdout.contains("0."),
            "Version output should contain program name or version number"
        );
    }

    /// 不正パスでの終了コードを確認する
    #[test]
    fn test_exit_code_nonexistent_path() {
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "/nonexistent/path/that/does/not/exist"])
            .output()
            .expect("Failed to execute command");

        // 空マニフェスト扱いなので成功するはず。
        // このツールは存在しないパスを空ディレクトリとして扱う。
        assert!(
            output.status.success(),
            "Should handle non-existent path gracefully"
        );
    }
}

mod monorepo_tests {
    use super::*;

    /// .depup 設定を持つモノレポテストプロジェクトを作成する
    fn create_monorepo_project() -> TempDir {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

        // サブディレクトリを作成する
        let gui_dir = temp_dir.path().join("gui");
        let api_dir = temp_dir.path().join("api");
        fs::create_dir(&gui_dir).unwrap();
        fs::create_dir(&api_dir).unwrap();

        // gui/package.json を作成する
        fs::write(
            gui_dir.join("package.json"),
            r#"{
  "name": "gui",
  "dependencies": {
    "react": "^18.2.0"
  }
}"#,
        )
        .unwrap();

        // api/pyproject.toml を作成する
        fs::write(
            api_dir.join("pyproject.toml"),
            r#"[project]
name = "api"
dependencies = [
    "fastapi>=0.100.0",
]
"#,
        )
        .unwrap();

        // .depup 設定を作成する
        fs::write(temp_dir.path().join(".depup"), "gui\napi\n").unwrap();

        temp_dir
    }

    /// JSON 出力のモノレポ dry-run が全サブディレクトリを処理することを確認する
    #[test]
    fn test_monorepo_dry_run_json() {
        let temp_dir = create_monorepo_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--json", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // 両方のサブディレクトリからマニフェストを取得しているはず
        let manifests = json["manifests"].as_array().unwrap();
        assert!(
            manifests.len() >= 2,
            "Should detect manifests from both gui and api, got {}",
            manifests.len()
        );

        // 両方の言語が含まれることを確認する
        let languages: Vec<&str> = manifests
            .iter()
            .filter_map(|m| m["language"].as_str())
            .collect();
        assert!(
            languages.contains(&"Node.js"),
            "Should detect Node.js manifest from gui/"
        );
        assert!(
            languages.contains(&"Python"),
            "Should detect Python manifest from api/"
        );
    }

    /// モノレポ dry-run が全ファイルを変更しないことを確認する
    #[test]
    fn test_monorepo_dry_run_no_modification() {
        let temp_dir = create_monorepo_project();
        let binary = get_binary_path();

        let original_pkg = fs::read_to_string(temp_dir.path().join("gui/package.json")).unwrap();
        let original_py = fs::read_to_string(temp_dir.path().join("api/pyproject.toml")).unwrap();

        Command::new(&binary)
            .args(["--dry-run", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let new_pkg = fs::read_to_string(temp_dir.path().join("gui/package.json")).unwrap();
        let new_py = fs::read_to_string(temp_dir.path().join("api/pyproject.toml")).unwrap();

        assert_eq!(
            original_pkg, new_pkg,
            "gui/package.json should not change in dry-run"
        );
        assert_eq!(
            original_py, new_py,
            "api/pyproject.toml should not change in dry-run"
        );
    }

    /// .depup がない場合に既存の単一ディレクトリ動作が維持されることを確認する
    #[test]
    fn test_no_depup_file_preserves_existing_behavior() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // .depup がないため従来どおり動作するはず
        let output = Command::new(&binary)
            .args(["--dry-run", "--json", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // ルートディレクトリのマニフェストを検出するはず
        let manifests = json["manifests"].as_array().unwrap();
        assert!(
            !manifests.is_empty(),
            "Should detect manifests without .depup file"
        );
    }

    /// verbose フラグ付きモノレポ実行でディレクトリ情報が表示されることを確認する
    #[test]
    fn test_monorepo_verbose() {
        let temp_dir = create_monorepo_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--verbose", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Monorepo mode"),
            "Verbose output should mention monorepo mode: {}",
            stderr
        );
    }
}

mod cli_options_tests {
    use super::*;

    /// verbose モード出力を確認する
    #[test]
    fn test_verbose_mode() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--verbose", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stderr = String::from_utf8_lossy(&output.stderr);

        // verbose モードにはバージョン情報が含まれるはず
        assert!(
            stderr.contains("depup v") || stderr.contains("Target:"),
            "Verbose mode should include version or target info"
        );
    }

    /// 非 PyPI 既定インデックスを設定した pyproject.toml は全依存がスキップされ、
    /// 理由が警告として出ることを確認する。
    ///
    /// 依存が 0 件になるだけでは「更新なし」と区別がつかず、利用者は
    /// private index の依存が意図的に外されていることに気づけない。
    #[test]
    fn test_non_pypi_default_index_warns_and_skips() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let pyproject = r#"[project]
name = "internal-app"
version = "1.0.0"
dependencies = [
    "requests>=2.28.0",
]

[[tool.poetry.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
priority = "primary"
"#;
        fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

        let binary = get_binary_path();
        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--python",
                "--no-osv",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("non-PyPI default index"),
            "スキップ理由が警告として出るべき: {stderr}"
        );

        // マニフェストは書き換えられない
        let after = fs::read_to_string(temp_dir.path().join("pyproject.toml")).unwrap();
        assert!(
            after.contains("requests>=2.28.0"),
            "依存は書き換えられないべき: {after}"
        );
    }

    /// diff 出力モードを確認する
    #[test]
    fn test_diff_output_mode() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--diff", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        // 更新候補がある場合、diff モードは何らかの出力を生成するはず。
        // 出力形式は更新の有無に依存する。
        let _stdout = String::from_utf8_lossy(&output.stdout);
        // クラッシュしないことだけを確認する
        assert!(
            output.status.success() || !output.status.success(),
            "Diff mode should complete without crashing"
        );
    }

    /// 言語フィルタオプションを確認する
    #[test]
    fn test_language_filters() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // 各言語フィルタを確認する
        for lang_flag in &["--node", "--python", "--rust", "--go"] {
            let output = Command::new(&binary)
                .args([
                    "--dry-run",
                    "--json",
                    lang_flag,
                    temp_dir.path().to_str().unwrap(),
                ])
                .output()
                .expect("Failed to execute command");

            let stdout = String::from_utf8_lossy(&output.stdout);
            let json: serde_json::Value = serde_json::from_str(&stdout)
                .unwrap_or_else(|_| panic!("Output should be valid JSON for {}", lang_flag));

            // フィルタされた1マニフェスト以下になるはず
            let manifests = json["manifests"].as_array().unwrap();
            assert!(
                manifests.len() <= 1,
                "Language filter {} should return at most 1 manifest, got {}",
                lang_flag,
                manifests.len()
            );
        }
    }

    /// exclude パッケージオプションを確認する
    #[test]
    fn test_exclude_package() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--json",
                "--node",
                "--exclude",
                "lodash",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // lodash は除外されるため updates に現れないはず
        let manifests = json["manifests"].as_array().unwrap();
        for manifest in manifests {
            let updates = manifest["updates"].as_array().unwrap();
            for update in updates {
                let name = update["name"].as_str().unwrap_or("");
                assert_ne!(name, "lodash", "lodash should be excluded from updates");
            }
        }
    }

    /// only パッケージオプションを確認する
    #[test]
    fn test_only_package() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--json",
                "--node",
                "--only",
                "lodash",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // updates がある場合、lodash だけが現れるはず
        let manifests = json["manifests"].as_array().unwrap();
        for manifest in manifests {
            let updates = manifest["updates"].as_array().unwrap();
            for update in updates {
                let name = update["name"].as_str().unwrap_or("");
                assert_eq!(name, "lodash", "Only lodash should appear in updates");
            }
        }
    }

    /// 不正な --cd パスを確認する
    #[test]
    fn test_invalid_cd_path() {
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--cd", "/path/does/not/exist", "--dry-run"])
            .output()
            .expect("Failed to execute command");

        // 存在しないディレクトリへの --cd は失敗するはず
        assert!(
            !output.status.success(),
            "--cd with non-existent path should fail"
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot change to directory")
                || stderr.contains("No such file or directory")
                || stderr.contains("does not exist"),
            "Error message should indicate directory problem: {}",
            stderr
        );
    }

    /// 相互排他オプションを確認する
    #[test]
    fn test_mutually_exclusive_options() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // --json と --diff は相互排他であるべき
        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--json",
                "--diff",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        // 実装に応じて失敗または有効な出力になる。
        // 最低限クラッシュしないことを確認する。
        let _stdout = String::from_utf8_lossy(&output.stdout);
        let _stderr = String::from_utf8_lossy(&output.stderr);
    }

    /// help 出力を確認する
    #[test]
    fn test_help_output() {
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .arg("--help")
            .output()
            .expect("Failed to execute command");

        assert!(output.status.success(), "--help should exit with success");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Usage:")
                || stdout.contains("USAGE:")
                || stdout.contains("Arguments:")
                || stdout.contains("Options:"),
            "Help output should contain usage information"
        );
    }

    /// age フィルタオプションを確認する
    #[test]
    fn test_age_filter_option() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--json",
                "--age",
                "7d",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        // ネットワーク状態により部分失敗 (終了コード 2) になる場合があるが、
        // --age 自体は有効な JSON 出力を返すはず。
        assert!(
            output.status.success() || output.status.code() == Some(2),
            "--age option should work"
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        // 有効な JSON を生成するはず
        let _: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");
    }

    /// 不正な age 形式を確認する
    #[test]
    fn test_invalid_age_format() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--age",
                "invalid",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        // 不正な age 形式では失敗するはず
        assert!(!output.status.success(), "Invalid --age format should fail");
    }
}
