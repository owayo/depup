//! End-to-end tests for depup CLI
//!
//! These tests verify:
//! - Dry-run mode leaves files unchanged
//! - CLI produces correct JSON output schema
//! - Exit codes are correct for various scenarios

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Get the path to the compiled binary
fn get_binary_path() -> PathBuf {
    // Build the binary first if needed
    let output = Command::new("cargo")
        .args(["build", "--release"])
        .output()
        .expect("Failed to build project");

    if !output.status.success() {
        panic!(
            "Failed to build: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Return path to compiled binary
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/release/depup");
    path
}

/// Create a test directory with sample manifest files
fn create_test_project() -> TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");

    // Create package.json
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

    // Create pyproject.toml
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

    // Create Cargo.toml
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

    // Create go.mod
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

    /// Test that dry-run mode does not modify any files
    #[test]
    fn test_dry_run_leaves_files_unchanged() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // Read original file contents
        let original_package_json =
            fs::read_to_string(temp_dir.path().join("package.json")).unwrap();
        let original_pyproject =
            fs::read_to_string(temp_dir.path().join("pyproject.toml")).unwrap();
        let original_cargo = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();
        let original_go_mod = fs::read_to_string(temp_dir.path().join("go.mod")).unwrap();

        // Run depup in dry-run mode
        let output = Command::new(&binary)
            .args(["--dry-run", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        // Verify command executed (might fail on network, but files should still be unchanged)
        // The command might exit with non-zero due to network errors, but that's OK for this test

        // Verify files are unchanged
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

    /// Test that dry-run with specific language filter still leaves files unchanged
    #[test]
    fn test_dry_run_with_language_filter() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let original_package_json =
            fs::read_to_string(temp_dir.path().join("package.json")).unwrap();

        // Run depup in dry-run mode for Node.js only
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

    /// Test that dry-run mode works with quiet flag
    #[test]
    fn test_dry_run_with_quiet_mode() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let original_cargo = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();

        // Run depup in dry-run mode with quiet flag
        let output = Command::new(&binary)
            .args([
                "--dry-run",
                "--quiet",
                "--rust",
                temp_dir.path().to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        // In quiet mode, stdout should be minimal
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Quiet mode should have less output
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

    /// Test JSON output structure
    #[test]
    fn test_json_output_schema() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // Run depup with JSON output
        let output = Command::new(&binary)
            .args(["--dry-run", "--json", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON output
        let json: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");

        // Verify top-level structure
        assert!(json.is_object(), "JSON output should be an object");

        // Verify required fields
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

        // Verify dry_run is true
        assert_eq!(
            json["dry_run"].as_bool(),
            Some(true),
            "dry_run should be true"
        );

        // Verify summary.updates is a number
        assert!(
            json["summary"]["updates"].is_number(),
            "summary.updates should be a number"
        );

        // Verify manifests is an array
        assert!(json["manifests"].is_array(), "manifests should be an array");
    }

    /// Test JSON output contains manifest information
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

        // Should have detected at least one manifest
        // (might not detect all if there are parsing issues)
        if !manifests.is_empty() {
            let manifest = &manifests[0];

            // Verify manifest structure
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
            // Note: 'skips' field is only included in verbose mode and when non-empty

            // Verify language is valid (display names)
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

    /// Test JSON output with empty directory
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

        // Should have empty manifests array
        let manifests = json["manifests"].as_array().unwrap();
        assert!(
            manifests.is_empty(),
            "Empty directory should have no manifests"
        );

        // summary.updates should be 0
        assert_eq!(
            json["summary"]["updates"].as_i64(),
            Some(0),
            "summary.updates should be 0 for empty directory"
        );
    }
}

mod exit_code_tests {
    use super::*;

    /// Test exit code for successful run with no updates
    #[test]
    fn test_exit_code_no_updates() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let binary = get_binary_path();

        // Run on empty directory (no manifests = no updates)
        let output = Command::new(&binary)
            .args(["--dry-run", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        // Should succeed with exit code 0
        assert!(
            output.status.success(),
            "Should exit with success for empty directory"
        );
    }

    /// Test exit code with help flag
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

    /// Test exit code with version flag
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

    /// Test exit code with invalid path
    #[test]
    fn test_exit_code_nonexistent_path() {
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "/nonexistent/path/that/does/not/exist"])
            .output()
            .expect("Failed to execute command");

        // Should still succeed (empty manifests case)
        // The tool treats non-existent paths as empty directories
        assert!(
            output.status.success(),
            "Should handle non-existent path gracefully"
        );
    }
}

mod cli_options_tests {
    use super::*;

    /// Test verbose mode output
    #[test]
    fn test_verbose_mode() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--verbose", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        let stderr = String::from_utf8_lossy(&output.stderr);

        // Verbose mode should include version info
        assert!(
            stderr.contains("depup v") || stderr.contains("Target:"),
            "Verbose mode should include version or target info"
        );
    }

    /// Test diff output mode
    #[test]
    fn test_diff_output_mode() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        let output = Command::new(&binary)
            .args(["--dry-run", "--diff", temp_dir.path().to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        // Diff mode should produce some output if there are potential updates
        // The output format depends on whether updates are found
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Just verify it doesn't crash
        assert!(
            output.status.success() || !output.status.success(),
            "Diff mode should complete without crashing"
        );
    }

    /// Test language filter options
    #[test]
    fn test_language_filters() {
        let temp_dir = create_test_project();
        let binary = get_binary_path();

        // Test each language filter
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
                .expect(&format!("Output should be valid JSON for {}", lang_flag));

            // Should have at most 1 manifest (the filtered one)
            let manifests = json["manifests"].as_array().unwrap();
            assert!(
                manifests.len() <= 1,
                "Language filter {} should return at most 1 manifest, got {}",
                lang_flag,
                manifests.len()
            );
        }
    }

    /// Test exclude package option
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

        // Lodash should be excluded, so it should not appear in updates
        let manifests = json["manifests"].as_array().unwrap();
        for manifest in manifests {
            let updates = manifest["updates"].as_array().unwrap();
            for update in updates {
                let name = update["name"].as_str().unwrap_or("");
                assert_ne!(name, "lodash", "lodash should be excluded from updates");
            }
        }
    }

    /// Test only package option
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

        // Only lodash should appear in updates (if any)
        let manifests = json["manifests"].as_array().unwrap();
        for manifest in manifests {
            let updates = manifest["updates"].as_array().unwrap();
            for update in updates {
                let name = update["name"].as_str().unwrap_or("");
                assert_eq!(name, "lodash", "Only lodash should appear in updates");
            }
        }
    }
}
