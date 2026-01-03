//! Integration tests for depup
//!
//! These tests verify:
//! - Manifest detection across multiple languages
//! - Manifest update format preservation
//! - Registry response parsing

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Test fixture directory creation helper
fn create_test_dir() -> TempDir {
    tempfile::tempdir().expect("Failed to create temp directory")
}

mod manifest_detection {
    use super::*;

    /// Test detection of multiple manifests in a single directory
    #[test]
    fn test_detect_multiple_languages() {
        let temp_dir = create_test_dir();

        // Create package.json (Node.js)
        let package_json = r#"{
            "name": "test-package",
            "dependencies": {
                "lodash": "^4.17.21"
            }
        }"#;
        fs::write(temp_dir.path().join("package.json"), package_json).unwrap();

        // Create pyproject.toml (Python)
        let pyproject = r#"[project]
name = "test-package"
dependencies = [
    "requests>=2.28.0"
]
"#;
        fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

        // Create Cargo.toml (Rust)
        let cargo_toml = r#"[package]
name = "test-package"
version = "0.1.0"

[dependencies]
serde = "1.0"
"#;
        fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

        // Create go.mod (Go)
        let go_mod = r#"module example.com/test

go 1.21

require github.com/gin-gonic/gin v1.9.0
"#;
        fs::write(temp_dir.path().join("go.mod"), go_mod).unwrap();

        // Use the detect_manifests function
        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        // Should detect all 4 manifest files
        assert_eq!(manifests.len(), 4, "Should detect 4 manifest files");

        // Verify each language is represented
        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(
            languages.contains(&depup::domain::Language::Node),
            "Should detect Node.js manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Python),
            "Should detect Python manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Rust),
            "Should detect Rust manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Go),
            "Should detect Go manifest"
        );
    }

    /// Test detection with partial manifests (some languages only)
    #[test]
    fn test_detect_partial_manifests() {
        let temp_dir = create_test_dir();

        // Create only Node.js and Python manifests
        let package_json = r#"{"name": "test", "dependencies": {"express": "^4.18.0"}}"#;
        fs::write(temp_dir.path().join("package.json"), package_json).unwrap();

        let pyproject = r#"[project]
name = "test"
dependencies = ["flask>=2.0.0"]
"#;
        fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 2, "Should detect 2 manifest files");
    }

    /// Test empty directory
    #[test]
    fn test_detect_empty_directory() {
        let temp_dir = create_test_dir();
        let manifests = depup::manifest::detect_manifests(temp_dir.path());
        assert!(
            manifests.is_empty(),
            "Should detect no manifests in empty directory"
        );
    }

    /// Test non-existent directory
    #[test]
    fn test_detect_nonexistent_directory() {
        let manifests = depup::manifest::detect_manifests(&PathBuf::from("/nonexistent/path"));
        assert!(
            manifests.is_empty(),
            "Should return empty for non-existent directory"
        );
    }
}

mod manifest_update_format_preservation {
    use super::*;
    use depup::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
    use depup::manifest::get_parser;

    /// Test package.json format preservation with caret versions
    #[test]
    fn test_package_json_caret_preservation() {
        let content = r#"{
  "name": "test",
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;

        let parser = get_parser(Language::Node);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "lodash");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);

        // Update the version
        let updated = parser.update_version(content, "lodash", "4.18.0").unwrap();
        assert!(
            updated.contains("\"^4.18.0\""),
            "Should preserve caret prefix: {}",
            updated
        );
    }

    /// Test package.json format preservation with tilde versions
    #[test]
    fn test_package_json_tilde_preservation() {
        let content = r#"{
  "dependencies": {
    "express": "~4.18.0"
  }
}"#;

        let parser = get_parser(Language::Node);
        let updated = parser.update_version(content, "express", "4.19.0").unwrap();
        assert!(
            updated.contains("\"~4.19.0\""),
            "Should preserve tilde prefix: {}",
            updated
        );
    }

    /// Test pyproject.toml format preservation
    #[test]
    fn test_pyproject_toml_gte_preservation() {
        let content = r#"[project]
dependencies = [
    "requests>=2.28.0",
]
"#;

        let parser = get_parser(Language::Python);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let updated = parser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(
            updated.contains(">=2.31.0"),
            "Should preserve >= prefix: {}",
            updated
        );
    }

    /// Test Cargo.toml format preservation
    #[test]
    fn test_cargo_toml_bare_version_preservation() {
        let content = r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0.190"
"#;

        let parser = get_parser(Language::Rust);
        let deps = parser.parse(content).unwrap();

        assert!(deps.iter().any(|d| d.name == "serde"));

        let updated = parser.update_version(content, "serde", "1.0.195").unwrap();
        // Cargo bare version should be preserved (no prefix)
        assert!(
            updated.contains("\"1.0.195\""),
            "Should update bare version: {}",
            updated
        );
    }

    /// Test Cargo.toml inline table format preservation
    #[test]
    fn test_cargo_toml_inline_table_preservation() {
        let content = r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
tokio = { version = "1.35", features = ["full"] }
"#;

        let parser = get_parser(Language::Rust);
        let updated = parser.update_version(content, "tokio", "1.40").unwrap();

        // Should preserve inline table format
        assert!(
            updated.contains("{ version = \"1.40\"") || updated.contains("{version = \"1.40\""),
            "Should preserve inline table: {}",
            updated
        );
        assert!(
            updated.contains("features = [\"full\"]"),
            "Should preserve features: {}",
            updated
        );
    }

    /// Test go.mod format preservation
    #[test]
    fn test_go_mod_v_prefix_preservation() {
        let content = r#"module example.com/test

go 1.21

require github.com/gin-gonic/gin v1.9.0
"#;

        let parser = get_parser(Language::Go);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");

        let updated = parser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(
            updated.contains("v1.10.0"),
            "Should preserve v prefix: {}",
            updated
        );
    }

    /// Test go.mod comment preservation
    #[test]
    fn test_go_mod_comment_preservation() {
        let content = r#"module example.com/test

go 1.21

require (
    github.com/gin-gonic/gin v1.9.0 // indirect
    github.com/stretchr/testify v1.8.0 // pinned
)
"#;

        let parser = get_parser(Language::Go);
        let deps = parser.parse(content).unwrap();

        // stretchr/testify should be marked as pinned
        let testify = deps.iter().find(|d| d.name.contains("testify"));
        assert!(testify.is_some());
        assert!(
            testify.unwrap().version_spec.is_pinned(),
            "Should detect pinned comment"
        );

        // Update gin
        let updated = parser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(
            updated.contains("// indirect"),
            "Should preserve comments: {}",
            updated
        );
    }
}

mod registry_response_parsing {
    use chrono::{TimeZone, Utc};
    use depup::update::VersionInfo;

    /// Test npm response JSON parsing
    #[test]
    fn test_npm_response_structure() {
        // Simulate npm registry response structure
        let npm_response = r#"{
            "time": {
                "4.17.21": "2021-02-20T15:30:00.000Z",
                "4.17.20": "2021-01-12T10:00:00.000Z"
            },
            "versions": {
                "4.17.21": {},
                "4.17.20": {}
            }
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(npm_response).unwrap();

        let time = parsed.get("time").unwrap().as_object().unwrap();
        assert_eq!(time.len(), 2);
        assert!(time.contains_key("4.17.21"));

        let versions = parsed.get("versions").unwrap().as_object().unwrap();
        assert_eq!(versions.len(), 2);
    }

    /// Test PyPI response JSON parsing
    #[test]
    fn test_pypi_response_structure() {
        // Simulate PyPI JSON API response structure
        let pypi_response = r#"{
            "releases": {
                "2.28.0": [
                    {"upload_time_iso_8601": "2022-06-14T15:00:00.000Z"}
                ],
                "2.31.0": [
                    {"upload_time_iso_8601": "2023-05-22T15:00:00.000Z"}
                ]
            }
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(pypi_response).unwrap();

        let releases = parsed.get("releases").unwrap().as_object().unwrap();
        assert_eq!(releases.len(), 2);

        let v2_31 = releases.get("2.31.0").unwrap().as_array().unwrap();
        assert!(!v2_31.is_empty());
        assert!(v2_31[0].get("upload_time_iso_8601").is_some());
    }

    /// Test crates.io response JSON parsing
    #[test]
    fn test_crates_io_response_structure() {
        // Simulate crates.io API response structure
        let crates_response = r#"{
            "versions": [
                {"num": "1.0.195", "created_at": "2024-01-15T10:00:00.000Z"},
                {"num": "1.0.194", "created_at": "2024-01-10T10:00:00.000Z"}
            ]
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(crates_response).unwrap();

        let versions = parsed.get("versions").unwrap().as_array().unwrap();
        assert_eq!(versions.len(), 2);

        let v195 = &versions[0];
        assert_eq!(v195.get("num").unwrap().as_str().unwrap(), "1.0.195");
        assert!(v195.get("created_at").is_some());
    }

    /// Test Go proxy response parsing (plain text)
    #[test]
    fn test_go_proxy_list_response() {
        // Simulate Go proxy /@v/list response (plain text)
        let go_list_response = "v1.9.0\nv1.9.1\nv1.10.0\n";

        let versions: Vec<&str> = go_list_response.lines().collect();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0], "v1.9.0");
        assert_eq!(versions[2], "v1.10.0");
    }

    /// Test Go proxy .info response
    #[test]
    fn test_go_proxy_info_response() {
        // Simulate Go proxy /@v/version.info response
        let go_info_response = r#"{
            "Version": "v1.10.0",
            "Time": "2024-01-20T15:00:00Z"
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(go_info_response).unwrap();
        assert_eq!(parsed.get("Version").unwrap().as_str().unwrap(), "v1.10.0");
        assert!(parsed.get("Time").is_some());
    }

    /// Test VersionInfo sorting
    #[test]
    fn test_version_info_sorting() {
        let v1 = VersionInfo::new("1.0.0", Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let v2 = VersionInfo::new("1.0.1", Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap());
        let v3 = VersionInfo::new("1.1.0", Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap());

        let mut versions = vec![v3.clone(), v1.clone(), v2.clone()];
        versions.sort();

        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[1].version, "1.0.1");
        assert_eq!(versions[2].version, "1.1.0");
    }

    /// Test version comparison edge cases
    /// Note: The simplified version comparison ignores non-numeric pre-release identifiers
    #[test]
    fn test_version_comparison_edge_cases() {
        let now = Utc::now();

        let prerelease = VersionInfo::new("1.0.0-alpha", now);
        let stable = VersionInfo::new("1.0.0", now);
        let patch = VersionInfo::new("1.0.1", now);

        // Pre-release and stable compare equal in simplified comparison
        // (non-numeric "alpha" is filtered out, leaving [1,0,0] == [1,0,0])
        assert_eq!(prerelease.cmp(&stable), std::cmp::Ordering::Equal);
        // Stable should come before next patch
        assert!(stable < patch);
        // Pre-release also comes before next patch
        assert!(prerelease < patch);
    }
}
