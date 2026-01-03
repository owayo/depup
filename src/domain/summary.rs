//! Update result summary types
//!
//! Provides structures for tracking update results at file and overall levels.

use super::{Language, UpdateResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Update result for a single manifest file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestUpdateResult {
    /// Path to the manifest file
    pub path: PathBuf,
    /// Language of this manifest
    pub language: Language,
    /// Individual dependency update results
    pub results: Vec<UpdateResult>,
    /// Whether the file was actually modified
    pub modified: bool,
}

impl ManifestUpdateResult {
    /// Creates a new ManifestUpdateResult
    pub fn new(path: impl Into<PathBuf>, language: Language) -> Self {
        Self {
            path: path.into(),
            language,
            results: Vec::new(),
            modified: false,
        }
    }

    /// Adds an update result
    pub fn add_result(&mut self, result: UpdateResult) {
        if result.is_update() {
            self.modified = true;
        }
        self.results.push(result);
    }

    /// Returns the number of updates
    pub fn update_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_update()).count()
    }

    /// Returns the number of skips
    pub fn skip_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_skip()).count()
    }

    /// Returns all updates
    pub fn updates(&self) -> impl Iterator<Item = &UpdateResult> {
        self.results.iter().filter(|r| r.is_update())
    }

    /// Returns all skips
    pub fn skips(&self) -> impl Iterator<Item = &UpdateResult> {
        self.results.iter().filter(|r| r.is_skip())
    }

    /// Returns true if any dependencies were updated
    pub fn has_updates(&self) -> bool {
        self.update_count() > 0
    }
}

/// Overall summary of all update operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSummary {
    /// Results for each manifest file processed
    pub manifests: Vec<ManifestUpdateResult>,
    /// Whether this was a dry run
    pub dry_run: bool,
}

impl UpdateSummary {
    /// Creates a new UpdateSummary
    pub fn new(dry_run: bool) -> Self {
        Self {
            manifests: Vec::new(),
            dry_run,
        }
    }

    /// Adds a manifest result
    pub fn add_manifest(&mut self, manifest: ManifestUpdateResult) {
        self.manifests.push(manifest);
    }

    /// Returns the total number of files processed
    pub fn files_processed(&self) -> usize {
        self.manifests.len()
    }

    /// Returns the total number of files modified
    pub fn files_modified(&self) -> usize {
        self.manifests.iter().filter(|m| m.modified).count()
    }

    /// Returns the total number of dependencies updated
    pub fn total_updates(&self) -> usize {
        self.manifests.iter().map(|m| m.update_count()).sum()
    }

    /// Returns the total number of dependencies skipped
    pub fn total_skips(&self) -> usize {
        self.manifests.iter().map(|m| m.skip_count()).sum()
    }

    /// Returns the total number of dependencies processed
    pub fn total_dependencies(&self) -> usize {
        self.manifests.iter().map(|m| m.results.len()).sum()
    }

    /// Returns true if any files were modified
    pub fn has_changes(&self) -> bool {
        self.files_modified() > 0
    }

    /// Returns manifests for a specific language
    pub fn by_language(&self, language: Language) -> impl Iterator<Item = &ManifestUpdateResult> {
        self.manifests
            .iter()
            .filter(move |m| m.language == language)
    }

    /// Returns all updates across all manifests
    pub fn all_updates(&self) -> impl Iterator<Item = &UpdateResult> {
        self.manifests.iter().flat_map(|m| m.updates())
    }

    /// Returns all skips across all manifests
    pub fn all_skips(&self) -> impl Iterator<Item = &UpdateResult> {
        self.manifests.iter().flat_map(|m| m.skips())
    }
}

impl Default for UpdateSummary {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, SkipReason, VersionSpec, VersionSpecKind};

    fn sample_dependency(name: &str) -> Dependency {
        Dependency::new(
            name,
            VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0").with_prefix("^"),
            false,
            Language::Node,
        )
    }

    fn sample_update(name: &str) -> UpdateResult {
        UpdateResult::update(sample_dependency(name), "2.0.0")
    }

    fn sample_skip(name: &str) -> UpdateResult {
        UpdateResult::skip(sample_dependency(name), SkipReason::Pinned)
    }

    #[test]
    fn test_manifest_update_result_new() {
        let result = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        assert_eq!(result.path, PathBuf::from("/path/to/package.json"));
        assert_eq!(result.language, Language::Node);
        assert!(result.results.is_empty());
        assert!(!result.modified);
    }

    #[test]
    fn test_manifest_update_result_add_update() {
        let mut result = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        result.add_result(sample_update("lodash"));

        assert_eq!(result.results.len(), 1);
        assert!(result.modified);
        assert_eq!(result.update_count(), 1);
        assert_eq!(result.skip_count(), 0);
    }

    #[test]
    fn test_manifest_update_result_add_skip() {
        let mut result = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        result.add_result(sample_skip("lodash"));

        assert_eq!(result.results.len(), 1);
        assert!(!result.modified);
        assert_eq!(result.update_count(), 0);
        assert_eq!(result.skip_count(), 1);
    }

    #[test]
    fn test_manifest_update_result_mixed() {
        let mut result = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        result.add_result(sample_update("lodash"));
        result.add_result(sample_skip("react"));
        result.add_result(sample_update("express"));

        assert_eq!(result.results.len(), 3);
        assert!(result.modified);
        assert_eq!(result.update_count(), 2);
        assert_eq!(result.skip_count(), 1);
        assert!(result.has_updates());
    }

    #[test]
    fn test_manifest_update_result_updates_iterator() {
        let mut result = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        result.add_result(sample_update("lodash"));
        result.add_result(sample_skip("react"));

        let updates: Vec<_> = result.updates().collect();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].package_name(), "lodash");
    }

    #[test]
    fn test_manifest_update_result_skips_iterator() {
        let mut result = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        result.add_result(sample_update("lodash"));
        result.add_result(sample_skip("react"));

        let skips: Vec<_> = result.skips().collect();
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0].package_name(), "react");
    }

    #[test]
    fn test_update_summary_new() {
        let summary = UpdateSummary::new(true);
        assert!(summary.manifests.is_empty());
        assert!(summary.dry_run);
    }

    #[test]
    fn test_update_summary_default() {
        let summary = UpdateSummary::default();
        assert!(summary.manifests.is_empty());
        assert!(!summary.dry_run);
    }

    #[test]
    fn test_update_summary_add_manifest() {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new("/path/to/package.json", Language::Node);
        manifest.add_result(sample_update("lodash"));
        summary.add_manifest(manifest);

        assert_eq!(summary.files_processed(), 1);
        assert_eq!(summary.files_modified(), 1);
    }

    #[test]
    fn test_update_summary_totals() {
        let mut summary = UpdateSummary::new(false);

        let mut manifest1 = ManifestUpdateResult::new("/package.json", Language::Node);
        manifest1.add_result(sample_update("lodash"));
        manifest1.add_result(sample_skip("react"));
        summary.add_manifest(manifest1);

        let mut manifest2 = ManifestUpdateResult::new("/Cargo.toml", Language::Rust);
        manifest2.add_result(sample_update("serde"));
        summary.add_manifest(manifest2);

        assert_eq!(summary.files_processed(), 2);
        assert_eq!(summary.files_modified(), 2);
        assert_eq!(summary.total_updates(), 2);
        assert_eq!(summary.total_skips(), 1);
        assert_eq!(summary.total_dependencies(), 3);
        assert!(summary.has_changes());
    }

    #[test]
    fn test_update_summary_no_changes() {
        let mut summary = UpdateSummary::new(false);

        let mut manifest = ManifestUpdateResult::new("/package.json", Language::Node);
        manifest.add_result(sample_skip("lodash"));
        summary.add_manifest(manifest);

        assert_eq!(summary.files_processed(), 1);
        assert_eq!(summary.files_modified(), 0);
        assert_eq!(summary.total_updates(), 0);
        assert!(!summary.has_changes());
    }

    #[test]
    fn test_update_summary_by_language() {
        let mut summary = UpdateSummary::new(false);

        let node_manifest = ManifestUpdateResult::new("/package.json", Language::Node);
        let rust_manifest = ManifestUpdateResult::new("/Cargo.toml", Language::Rust);
        summary.add_manifest(node_manifest);
        summary.add_manifest(rust_manifest);

        let node_results: Vec<_> = summary.by_language(Language::Node).collect();
        assert_eq!(node_results.len(), 1);
        assert_eq!(node_results[0].language, Language::Node);

        let rust_results: Vec<_> = summary.by_language(Language::Rust).collect();
        assert_eq!(rust_results.len(), 1);
        assert_eq!(rust_results[0].language, Language::Rust);

        let python_results: Vec<_> = summary.by_language(Language::Python).collect();
        assert_eq!(python_results.len(), 0);
    }

    #[test]
    fn test_update_summary_all_updates() {
        let mut summary = UpdateSummary::new(false);

        let mut manifest1 = ManifestUpdateResult::new("/package.json", Language::Node);
        manifest1.add_result(sample_update("lodash"));
        summary.add_manifest(manifest1);

        let mut manifest2 = ManifestUpdateResult::new("/Cargo.toml", Language::Rust);
        manifest2.add_result(sample_update("serde"));
        summary.add_manifest(manifest2);

        let all_updates: Vec<_> = summary.all_updates().collect();
        assert_eq!(all_updates.len(), 2);
    }

    #[test]
    fn test_update_summary_all_skips() {
        let mut summary = UpdateSummary::new(false);

        let mut manifest1 = ManifestUpdateResult::new("/package.json", Language::Node);
        manifest1.add_result(sample_skip("react"));
        summary.add_manifest(manifest1);

        let mut manifest2 = ManifestUpdateResult::new("/Cargo.toml", Language::Rust);
        manifest2.add_result(sample_skip("tokio"));
        summary.add_manifest(manifest2);

        let all_skips: Vec<_> = summary.all_skips().collect();
        assert_eq!(all_skips.len(), 2);
    }

    #[test]
    fn test_serde_manifest_update_result() {
        let mut result = ManifestUpdateResult::new("/package.json", Language::Node);
        result.add_result(sample_update("lodash"));

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ManifestUpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn test_serde_update_summary() {
        let mut summary = UpdateSummary::new(true);
        let manifest = ManifestUpdateResult::new("/package.json", Language::Node);
        summary.add_manifest(manifest);

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"dry_run\":true"));
        let parsed: UpdateSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, summary);
    }
}
