//! Manifest file writing and update operations
//!
//! This module provides:
//! - ManifestWriter for applying version updates to manifest files
//! - Dry-run mode support (no actual file modifications)
//! - Format preservation when updating versions
//! - Parse error handling with graceful continuation

use crate::domain::{Language, ManifestUpdateResult, UpdateResult};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use std::fs;
use std::path::Path;

/// Writer for manifest files that applies version updates
pub struct ManifestWriter {
    /// Whether to run in dry-run mode (no file modifications)
    dry_run: bool,
}

/// Result of applying updates to a manifest file
#[derive(Debug)]
pub struct WriteResult {
    /// Path to the manifest file
    pub path: std::path::PathBuf,
    /// Number of updates successfully applied
    pub updates_applied: usize,
    /// Number of updates that failed
    pub updates_failed: usize,
    /// Whether the file was actually modified
    pub file_modified: bool,
    /// Errors encountered during update
    pub errors: Vec<String>,
}

impl WriteResult {
    /// Create a new WriteResult
    fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            updates_applied: 0,
            updates_failed: 0,
            file_modified: false,
            errors: Vec::new(),
        }
    }

    /// Returns true if any updates were successfully applied
    pub fn has_updates(&self) -> bool {
        self.updates_applied > 0
    }

    /// Returns true if any errors occurred
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

impl ManifestWriter {
    /// Create a new ManifestWriter
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }

    /// Create a ManifestWriter in dry-run mode
    pub fn dry_run() -> Self {
        Self { dry_run: true }
    }

    /// Check if this writer is in dry-run mode
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Apply updates from a ManifestUpdateResult to the actual file
    pub fn apply_updates(
        &self,
        manifest_result: &ManifestUpdateResult,
        parser: &dyn ManifestParser,
    ) -> Result<WriteResult, ManifestError> {
        let path = &manifest_result.path;
        let mut result = WriteResult::new(path);

        // Read current file content
        let content = fs::read_to_string(path).map_err(|e| ManifestError::ReadError {
            path: path.clone(),
            source: e,
        })?;

        // Apply each update sequentially
        let mut current_content = content.clone();

        for update in manifest_result.results.iter() {
            if let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = update
            {
                match parser.update_version(&current_content, &dependency.name, new_version) {
                    Ok(updated_content) => {
                        current_content = updated_content;
                        result.updates_applied += 1;
                    }
                    Err(e) => {
                        result.updates_failed += 1;
                        result
                            .errors
                            .push(format!("Failed to update {}: {}", dependency.name, e));
                    }
                }
            }
        }

        // Write back to file if not in dry-run mode and there were changes
        if result.updates_applied > 0 && !self.dry_run {
            fs::write(path, &current_content).map_err(|e| ManifestError::WriteError {
                path: path.clone(),
                source: e,
            })?;
            result.file_modified = true;
        }

        Ok(result)
    }

    /// Apply updates to multiple manifest files
    pub fn apply_all_updates(
        &self,
        manifests: &[ManifestUpdateResult],
        get_parser: impl Fn(Language) -> Box<dyn ManifestParser>,
    ) -> Vec<WriteResult> {
        manifests
            .iter()
            .filter_map(|manifest| {
                // Only process manifests that have updates
                if !manifest.has_updates() {
                    return None;
                }

                let parser = get_parser(manifest.language);
                match self.apply_updates(manifest, parser.as_ref()) {
                    Ok(result) => Some(result),
                    Err(e) => {
                        let mut result = WriteResult::new(&manifest.path);
                        result
                            .errors
                            .push(format!("Failed to process manifest: {}", e));
                        Some(result)
                    }
                }
            })
            .collect()
    }
}

/// Read a manifest file content safely
pub fn read_manifest(path: &Path) -> Result<String, ManifestError> {
    fs::read_to_string(path).map_err(|e| ManifestError::ReadError {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Write content to a manifest file
pub fn write_manifest(path: &Path, content: &str) -> Result<(), ManifestError> {
    fs::write(path, content).map_err(|e| ManifestError::WriteError {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use std::io::Write;
    use tempfile::TempDir;

    fn sample_dependency(name: &str, version: &str, language: Language) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, false, language)
    }

    fn create_temp_package_json(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("package.json");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_manifest_writer_new() {
        let writer = ManifestWriter::new(false);
        assert!(!writer.is_dry_run());

        let writer = ManifestWriter::new(true);
        assert!(writer.is_dry_run());
    }

    #[test]
    fn test_manifest_writer_dry_run_constructor() {
        let writer = ManifestWriter::dry_run();
        assert!(writer.is_dry_run());
    }

    #[test]
    fn test_write_result_new() {
        let result = WriteResult::new("/path/to/file");
        assert_eq!(result.path, std::path::PathBuf::from("/path/to/file"));
        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed, 0);
        assert!(!result.file_modified);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_write_result_has_updates() {
        let mut result = WriteResult::new("/path/to/file");
        assert!(!result.has_updates());

        result.updates_applied = 1;
        assert!(result.has_updates());
    }

    #[test]
    fn test_write_result_has_errors() {
        let mut result = WriteResult::new("/path/to/file");
        assert!(!result.has_errors());

        result.errors.push("error".to_string());
        assert!(result.has_errors());
    }

    #[test]
    fn test_apply_updates_dry_run() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::dry_run();
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert!(!result.file_modified); // Not modified in dry-run mode

        // Verify file content unchanged
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("4.17.21"));
        assert!(!content.contains("4.18.0"));
    }

    #[test]
    fn test_apply_updates_actual_write() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert!(result.file_modified);

        // Verify file content changed
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("^4.18.0"));
        assert!(!content.contains("4.17.21"));
    }

    #[test]
    fn test_apply_updates_multiple_packages() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21",
    "express": "^4.18.0"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        let dep1 = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep1, "4.18.0"));

        let dep2 = sample_dependency("express", "4.18.0", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep2, "4.19.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 2);
        assert!(result.file_modified);

        // Verify both packages updated
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("^4.18.0")); // lodash
        assert!(content.contains("^4.19.0")); // express
    }

    #[test]
    fn test_apply_updates_handles_failed_update() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        // Valid update
        let dep1 = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep1, "4.18.0"));

        // Invalid update (package doesn't exist)
        let dep2 = sample_dependency("nonexistent", "1.0.0", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep2, "2.0.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed, 1);
        assert!(result.has_errors());
        assert!(result.file_modified); // File still modified for successful updates
    }

    #[test]
    fn test_apply_updates_no_updates() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        // ManifestUpdateResult with only skips, no updates
        let manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 0);
        assert!(!result.file_modified);
    }

    #[test]
    fn test_apply_updates_file_not_found() {
        let manifest_result =
            ManifestUpdateResult::new("/nonexistent/path/package.json", Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        let mut manifest_result = manifest_result;
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser);

        assert!(result.is_err());
    }

    #[test]
    fn test_read_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"{"name": "test"}"#;
        let path = create_temp_package_json(&temp_dir, content);

        let result = read_manifest(&path).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_read_manifest_not_found() {
        let result = read_manifest(Path::new("/nonexistent/path/file.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_write_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.json");
        let content = r#"{"name": "test"}"#;

        write_manifest(&path, content).unwrap();

        let result = fs::read_to_string(&path).unwrap();
        assert_eq!(result, content);
    }
}
