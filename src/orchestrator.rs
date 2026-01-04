//! Update orchestrator for coordinating the entire update workflow
//!
//! This module provides:
//! - Workflow coordination: detect → parse → fetch → judge → write
//! - Parallel registry queries with rate limiting
//! - Dry-run mode support
//! - Language and package filter application
//! - Error handling with partial continuation

use crate::cli::CliArgs;
use crate::domain::{Language, ManifestUpdateResult, SkipReason, UpdateResult, UpdateSummary};
use crate::manifest::{
    detect_manifests, get_parser, has_pnpm_workspace, ManifestWriter, PnpmSettings, WriteResult,
};
use crate::progress::Progress;
use crate::registry::{
    CratesIoAdapter, GoProxyAdapter, HttpClient, NpmAdapter, PackagistAdapter, PyPIAdapter,
    RegistryAdapter, RubyGemsAdapter,
};
use crate::update::{UpdateFilter, UpdateJudge, VersionInfo};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Default concurrency limit for registry requests
const DEFAULT_CONCURRENCY: usize = 10;

/// Concurrency limit for crates.io (rate limited)
const CRATES_IO_CONCURRENCY: usize = 1;

/// Orchestrator for coordinating the update workflow
pub struct Orchestrator {
    /// CLI arguments for configuration
    args: CliArgs,
    /// HTTP client for registry requests
    client: HttpClient,
    /// Semaphore for general concurrency control
    general_semaphore: Arc<Semaphore>,
    /// Semaphore for crates.io specific rate limiting
    crates_io_semaphore: Arc<Semaphore>,
}

/// Result of running the orchestrator
pub struct OrchestratorResult {
    /// Update summary with all results
    pub summary: UpdateSummary,
    /// Write results for each manifest
    pub write_results: Vec<WriteResult>,
    /// Errors encountered during processing
    pub errors: Vec<OrchestratorError>,
}

/// Errors that can occur during orchestration
#[derive(Debug)]
pub enum OrchestratorError {
    /// Failed to create HTTP client
    HttpClientError(String),
    /// Failed to detect manifests
    ManifestDetectionError(String),
    /// Failed to parse manifest
    ManifestParseError { path: String, message: String },
    /// Failed to fetch versions from registry
    RegistryError { package: String, message: String },
    /// Failed to write manifest
    WriteError { path: String, message: String },
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrchestratorError::HttpClientError(msg) => write!(f, "HTTP client error: {}", msg),
            OrchestratorError::ManifestDetectionError(msg) => {
                write!(f, "Manifest detection error: {}", msg)
            }
            OrchestratorError::ManifestParseError { path, message } => {
                write!(f, "Failed to parse {}: {}", path, message)
            }
            OrchestratorError::RegistryError { package, message } => {
                write!(f, "Failed to fetch {}: {}", package, message)
            }
            OrchestratorError::WriteError { path, message } => {
                write!(f, "Failed to write {}: {}", path, message)
            }
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl Orchestrator {
    /// Create a new orchestrator with the given CLI arguments
    pub fn new(args: CliArgs) -> Result<Self, OrchestratorError> {
        let client =
            HttpClient::new().map_err(|e| OrchestratorError::HttpClientError(e.to_string()))?;

        Ok(Self {
            args,
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
        })
    }

    /// Create an orchestrator with a custom HTTP client (for testing)
    pub fn with_client(args: CliArgs, client: HttpClient) -> Self {
        Self {
            args,
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
        }
    }

    /// Run the update workflow
    pub async fn run(&self) -> OrchestratorResult {
        self.run_with_progress(!self.args.quiet).await
    }

    /// Run the update workflow with optional progress display
    pub async fn run_with_progress(&self, show_progress: bool) -> OrchestratorResult {
        let mut progress = Progress::new(show_progress);
        let mut summary = UpdateSummary::new(self.args.dry_run);
        let mut errors = Vec::new();

        // Step 1: Detect manifest files
        progress.spinner("Detecting manifest files...");
        let manifests = detect_manifests(&self.args.path);
        progress.finish_and_clear();

        if manifests.is_empty() {
            return OrchestratorResult {
                summary,
                write_results: Vec::new(),
                errors,
            };
        }

        // Build update filter from CLI args
        let filter = self.build_filter();
        let judge = UpdateJudge::new(filter);

        // Step 2: Parse manifests and collect all dependencies
        progress.spinner("Parsing manifests...");
        let mut parsed_manifests = Vec::new();

        for manifest_info in &manifests {
            // Check language filter
            if !self.should_process_language(manifest_info.language) {
                continue;
            }

            // Parse the manifest
            let parser = get_parser(manifest_info.language);
            let content = match std::fs::read_to_string(&manifest_info.path) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(OrchestratorError::ManifestParseError {
                        path: manifest_info.path.display().to_string(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            let dependencies = match parser.parse(&content) {
                Ok(deps) => deps,
                Err(e) => {
                    errors.push(OrchestratorError::ManifestParseError {
                        path: manifest_info.path.display().to_string(),
                        message: e.to_string(),
                    });
                    continue;
                }
            };

            parsed_manifests.push((manifest_info, dependencies));
        }
        progress.finish_and_clear();

        // Count total dependencies for progress bar
        let total_deps: usize = parsed_manifests.iter().map(|(_, deps)| deps.len()).sum();

        // Step 3: Fetch versions and judge updates for each dependency
        progress.start(total_deps as u64, "Checking dependencies");

        for (manifest_info, dependencies) in parsed_manifests {
            let mut manifest_result =
                ManifestUpdateResult::new(&manifest_info.path, manifest_info.language);
            let adapter = self.get_adapter(manifest_info.language);

            for dep in dependencies {
                progress.set_message(&format!("Checking {}", &dep.name));

                // Check if we should skip this dependency early
                if let Some(reason) = judge.should_skip(&dep) {
                    manifest_result.add_result(UpdateResult::skip(dep, reason));
                    progress.inc();
                    continue;
                }

                // Fetch versions from registry
                let versions = match self.fetch_versions(&*adapter, &dep.name).await {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(OrchestratorError::RegistryError {
                            package: dep.name.clone(),
                            message: e.to_string(),
                        });
                        manifest_result
                            .add_result(UpdateResult::skip(dep, SkipReason::FetchFailed(e)));
                        progress.inc();
                        continue;
                    }
                };

                // Judge whether to update
                let result = judge.judge(&dep, &versions);
                manifest_result.add_result(result);
                progress.inc();
            }

            summary.add_manifest(manifest_result);
        }
        progress.finish_and_clear();

        // Step 4: Apply updates (unless dry-run)
        if !self.args.dry_run {
            progress.spinner("Writing updates...");
        }
        let writer = ManifestWriter::new(self.args.dry_run);
        let write_results = writer.apply_all_updates(&summary.manifests, get_parser);
        progress.finish_and_clear();

        // Collect write errors
        for result in &write_results {
            for error in &result.errors {
                errors.push(OrchestratorError::WriteError {
                    path: result.path.display().to_string(),
                    message: error.clone(),
                });
            }
        }

        OrchestratorResult {
            summary,
            write_results,
            errors,
        }
    }

    /// Build an UpdateFilter from CLI arguments
    fn build_filter(&self) -> UpdateFilter {
        let mut filter = UpdateFilter::new();

        // Language filter
        if self.args.has_language_filter() {
            let mut languages = Vec::new();
            if self.args.node {
                languages.push(Language::Node);
            }
            if self.args.python {
                languages.push(Language::Python);
            }
            if self.args.rust_lang {
                languages.push(Language::Rust);
            }
            if self.args.go {
                languages.push(Language::Go);
            }
            filter = filter.with_languages(languages);
        }

        // Package filters
        if !self.args.exclude.is_empty() {
            filter = filter.with_exclude(self.args.exclude.clone());
        }
        if !self.args.only.is_empty() {
            filter = filter.with_only(self.args.only.clone());
        }

        // Include pinned
        if self.args.include_pinned {
            filter = filter.with_include_pinned(true);
        }

        // Age filter
        // Priority: CLI --age > pnpm settings (for Node.js projects)
        if let Some(age) = self.args.age {
            filter = filter.with_min_age(age);
        } else if has_pnpm_workspace(&self.args.path) {
            // Read pnpm settings for minimum release age
            let pnpm_settings = PnpmSettings::from_dir(&self.args.path);
            if let Some(age) = pnpm_settings.minimum_release_age {
                filter = filter.with_min_age(age);
            }
        }

        filter
    }

    /// Check if a language should be processed based on CLI args
    fn should_process_language(&self, language: Language) -> bool {
        if !self.args.has_language_filter() {
            return true;
        }
        match language {
            Language::Node => self.args.node,
            Language::Python => self.args.python,
            Language::Rust => self.args.rust_lang,
            Language::Go => self.args.go,
            Language::Ruby => self.args.ruby,
            Language::Php => self.args.php,
        }
    }

    /// Get the appropriate registry adapter for a language
    fn get_adapter(&self, language: Language) -> Box<dyn RegistryAdapter + Send + Sync> {
        match language {
            Language::Node => Box::new(NpmAdapter::new(self.client.clone())),
            Language::Python => Box::new(PyPIAdapter::new(self.client.clone())),
            Language::Rust => Box::new(CratesIoAdapter::new(self.client.clone())),
            Language::Go => Box::new(GoProxyAdapter::new(self.client.clone())),
            Language::Ruby => Box::new(RubyGemsAdapter::new(self.client.clone())),
            Language::Php => Box::new(PackagistAdapter::new(self.client.clone())),
        }
    }

    /// Fetch versions from registry with concurrency control
    async fn fetch_versions(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        // Use appropriate semaphore based on registry
        let semaphore = if adapter.language() == Language::Rust {
            &self.crates_io_semaphore
        } else {
            &self.general_semaphore
        };

        let _permit = semaphore.acquire().await.unwrap();

        adapter
            .fetch_versions(package)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Configuration for the orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Maximum concurrent requests for general registries
    pub general_concurrency: usize,
    /// Maximum concurrent requests for crates.io
    pub crates_io_concurrency: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            general_concurrency: DEFAULT_CONCURRENCY,
            crates_io_concurrency: CRATES_IO_CONCURRENCY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::fs;
    use tempfile::TempDir;

    fn make_args(args: &[&str]) -> CliArgs {
        CliArgs::parse_from(args)
    }

    fn make_args_with_path(path: &std::path::Path, extra_args: &[&str]) -> CliArgs {
        let path_str = path.to_str().unwrap();
        let mut args = vec!["depup", path_str];
        args.extend(extra_args);
        CliArgs::parse_from(&args)
    }

    #[test]
    fn test_orchestrator_config_default() {
        let config = OrchestratorConfig::default();
        assert_eq!(config.general_concurrency, 10);
        assert_eq!(config.crates_io_concurrency, 1);
    }

    #[test]
    fn test_build_filter_no_args() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // No language filter
        assert!(filter.should_process_language(Language::Node));
        assert!(filter.should_process_language(Language::Python));
        assert!(filter.should_process_language(Language::Rust));
        assert!(filter.should_process_language(Language::Go));
    }

    #[test]
    fn test_build_filter_with_languages() {
        let args = make_args(&["depup", "--node", "--python"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.should_process_language(Language::Node));
        assert!(filter.should_process_language(Language::Python));
        assert!(!filter.should_process_language(Language::Rust));
        assert!(!filter.should_process_language(Language::Go));
    }

    #[test]
    fn test_build_filter_with_exclude() {
        let args = make_args(&["depup", "--exclude", "lodash", "--exclude", "react"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(!filter.should_process_package("lodash"));
        assert!(!filter.should_process_package("react"));
        assert!(filter.should_process_package("express"));
    }

    #[test]
    fn test_build_filter_with_only() {
        let args = make_args(&["depup", "--only", "lodash"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.should_process_package("lodash"));
        assert!(!filter.should_process_package("react"));
    }

    #[test]
    fn test_build_filter_with_include_pinned() {
        let args = make_args(&["depup", "--include-pinned"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.include_pinned);
    }

    #[test]
    fn test_build_filter_with_age() {
        let args = make_args(&["depup", "--age", "2w"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(14 * 24 * 60 * 60)
        );
    }

    #[test]
    fn test_should_process_language_no_filter() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Node));
        assert!(orchestrator.should_process_language(Language::Python));
        assert!(orchestrator.should_process_language(Language::Rust));
        assert!(orchestrator.should_process_language(Language::Go));
    }

    #[test]
    fn test_should_process_language_with_filter() {
        let args = make_args(&["depup", "--node"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Node));
        assert!(!orchestrator.should_process_language(Language::Python));
        assert!(!orchestrator.should_process_language(Language::Rust));
        assert!(!orchestrator.should_process_language(Language::Go));
    }

    #[test]
    fn test_get_adapter_node() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Node);
        assert_eq!(adapter.language(), Language::Node);
    }

    #[test]
    fn test_get_adapter_python() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Python);
        assert_eq!(adapter.language(), Language::Python);
    }

    #[test]
    fn test_get_adapter_rust() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Rust);
        assert_eq!(adapter.language(), Language::Rust);
    }

    #[test]
    fn test_get_adapter_go() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Go);
        assert_eq!(adapter.language(), Language::Go);
    }

    #[test]
    fn test_orchestrator_error_display() {
        let err = OrchestratorError::HttpClientError("connection failed".to_string());
        assert!(err.to_string().contains("HTTP client error"));

        let err = OrchestratorError::ManifestDetectionError("not found".to_string());
        assert!(err.to_string().contains("Manifest detection error"));

        let err = OrchestratorError::ManifestParseError {
            path: "/path/to/file".to_string(),
            message: "invalid".to_string(),
        };
        assert!(err.to_string().contains("Failed to parse"));

        let err = OrchestratorError::RegistryError {
            package: "lodash".to_string(),
            message: "not found".to_string(),
        };
        assert!(err.to_string().contains("Failed to fetch lodash"));

        let err = OrchestratorError::WriteError {
            path: "/path/to/file".to_string(),
            message: "permission denied".to_string(),
        };
        assert!(err.to_string().contains("Failed to write"));
    }

    #[test]
    fn test_build_filter_with_pnpm_workspace_yaml() {
        let dir = TempDir::new().unwrap();

        // Create pnpm-workspace.yaml with minimumReleaseAge in minutes (14400 = 10 days)
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // Should have min_age from pnpm settings (14400 minutes = 864000 seconds)
        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(14400 * 60)
        );
    }

    #[test]
    fn test_build_filter_cli_age_overrides_pnpm() {
        let dir = TempDir::new().unwrap();

        // Create pnpm-workspace.yaml with minimumReleaseAge
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: []\nminimumReleaseAge: 14400\n",
        )
        .unwrap();

        // CLI --age should override pnpm settings
        let args = make_args_with_path(dir.path(), &["--age", "2w"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // Should have CLI age (2 weeks), not pnpm age (10 days)
        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(14 * 24 * 60 * 60) // 2 weeks
        );
    }

    #[test]
    fn test_build_filter_with_npmrc() {
        let dir = TempDir::new().unwrap();

        // Create pnpm-lock.yaml to indicate pnpm project
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        // Create .npmrc with minimum-release-age
        fs::write(dir.path().join(".npmrc"), "minimum-release-age=10d\n").unwrap();

        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // Should have min_age from .npmrc (10 days)
        assert!(filter.min_age.is_some());
        assert_eq!(
            filter.min_age.unwrap(),
            std::time::Duration::from_secs(10 * 24 * 60 * 60)
        );
    }

    #[test]
    fn test_build_filter_no_pnpm_no_age() {
        let dir = TempDir::new().unwrap();

        // No pnpm files, no --age flag
        let args = make_args_with_path(dir.path(), &[]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let filter = orchestrator.build_filter();

        // Should have no min_age
        assert!(filter.min_age.is_none());
    }
}
