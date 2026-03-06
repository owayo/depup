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
    ManifestInfo, ManifestWriter, PnpmSettings, WriteResult, detect_manifests, get_parser,
    has_pnpm_workspace,
};
use crate::progress::Progress;
use crate::registry::{
    CratesIoAdapter, GitHubTagsAdapter, GoProxyAdapter, HttpClient, MavenCentralAdapter,
    NpmAdapter, PackagistAdapter, PyPIAdapter, RegistryAdapter, RubyGemsAdapter,
};
use crate::tauri_sync::{TAURI_CRATE, TAURI_NPM_PACKAGES, TauriVersionSync};
use crate::update::{UpdateFilter, UpdateJudge, VersionInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// Default concurrency limit for registry requests
const DEFAULT_CONCURRENCY: usize = 10;

/// Concurrency limit for crates.io (rate limited)
const CRATES_IO_CONCURRENCY: usize = 1;

/// Cache for version information keyed by (language, package_name)
pub type VersionCache = Arc<Mutex<HashMap<(Language, String), Vec<VersionInfo>>>>;

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
    /// Version cache shared across directories
    version_cache: VersionCache,
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
            version_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Create an orchestrator with a custom HTTP client (for testing)
    pub fn with_client(args: CliArgs, client: HttpClient) -> Self {
        Self {
            args,
            client,
            general_semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            crates_io_semaphore: Arc::new(Semaphore::new(CRATES_IO_CONCURRENCY)),
            version_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set a shared version cache (for monorepo multi-directory runs)
    pub fn with_cache(mut self, cache: VersionCache) -> Self {
        self.version_cache = cache;
        self
    }

    /// Run the update workflow
    pub async fn run(&self) -> OrchestratorResult {
        self.run_with_progress(!self.args.quiet).await
    }

    /// Run the update workflow with optional progress display
    pub async fn run_with_progress(&self, show_progress: bool) -> OrchestratorResult {
        let mut progress = Progress::new(show_progress);

        // Step 1: Detect manifest files
        progress.spinner("Detecting manifest files...");
        let manifests = detect_manifests(&self.args.path);
        progress.finish_and_clear();

        self.process_manifests(&manifests, &mut progress).await
    }

    /// Run the update workflow across multiple directories (monorepo mode)
    ///
    /// Detects manifests in each directory, shares the version cache,
    /// and produces a combined result.
    pub async fn run_directories(&self, directories: &[PathBuf]) -> OrchestratorResult {
        let mut progress = Progress::new(!self.args.quiet);

        // Step 1: Detect manifest files across all directories
        progress.spinner("Detecting manifest files...");
        let mut all_manifests = Vec::new();
        for dir in directories {
            let manifests = detect_manifests(dir);
            all_manifests.extend(manifests);
        }
        progress.finish_and_clear();

        self.process_manifests(&all_manifests, &mut progress).await
    }

    /// Process detected manifests: parse, fetch versions, judge updates, and write results
    async fn process_manifests(
        &self,
        manifests: &[ManifestInfo],
        progress: &mut Progress,
    ) -> OrchestratorResult {
        let mut summary = UpdateSummary::new(self.args.dry_run);
        let mut errors = Vec::new();

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

        for manifest_info in manifests {
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

        // Step 3.5: Synchronize Tauri versions if this is a Tauri project
        let is_tauri = manifests.iter().any(|m| m.is_tauri_rust);
        if is_tauri {
            progress.spinner("Synchronizing Tauri versions...");
            self.synchronize_tauri_versions(&mut summary, &mut errors)
                .await;
            progress.finish_and_clear();
        }

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
            if self.args.ruby {
                languages.push(Language::Ruby);
            }
            if self.args.php {
                languages.push(Language::Php);
            }
            if self.args.java {
                languages.push(Language::Java);
            }
            if self.args.swift {
                languages.push(Language::Swift);
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
            Language::Java => self.args.java,
            Language::Swift => self.args.swift,
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
            Language::Java => Box::new(MavenCentralAdapter::new(self.client.clone())),
            Language::Swift => Box::new(GitHubTagsAdapter::new(self.client.clone())),
        }
    }

    /// Fetch versions from registry with concurrency control and caching
    async fn fetch_versions(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        let cache_key = (adapter.language(), package.to_string());

        // Check cache first
        {
            let cache = self.version_cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // Use appropriate semaphore based on registry
        let semaphore = if adapter.language() == Language::Rust {
            &self.crates_io_semaphore
        } else {
            &self.general_semaphore
        };

        let _permit = semaphore.acquire().await.unwrap();

        let result = adapter
            .fetch_versions(package)
            .await
            .map_err(|e| e.to_string())?;

        // Store in cache
        {
            let mut cache = self.version_cache.lock().await;
            cache.insert(cache_key, result.clone());
        }

        Ok(result)
    }

    /// Synchronize Tauri package versions (@tauri-apps/api, @tauri-apps/cli, and tauri crate)
    ///
    /// Ensures all packages have matching major.minor versions to prevent
    /// Tauri build errors.
    async fn synchronize_tauri_versions(
        &self,
        summary: &mut UpdateSummary,
        errors: &mut Vec<OrchestratorError>,
    ) {
        use crate::tauri_sync::extract_major_minor;

        // Find all Tauri npm packages in Node manifests
        // Returns: Vec<(manifest_idx, result_idx, result, current_version)>
        let npm_packages: Vec<(usize, usize, UpdateResult, String)> = summary
            .manifests
            .iter()
            .enumerate()
            .filter(|(_, m)| m.language == Language::Node)
            .flat_map(|(mi, m)| {
                m.results
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| TAURI_NPM_PACKAGES.contains(&r.package_name()))
                    .map(move |(ri, r)| {
                        let current = r.dependency().version().to_string();
                        (mi, ri, r.clone(), current)
                    })
            })
            .collect();

        // Find tauri crate in Rust manifests
        let crate_info: Option<(usize, usize, UpdateResult, String)> = summary
            .manifests
            .iter()
            .enumerate()
            .filter(|(_, m)| m.language == Language::Rust)
            .flat_map(|(mi, m)| {
                m.results
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.package_name() == TAURI_CRATE)
                    .map(move |(ri, r)| {
                        let current = r.dependency().version().to_string();
                        (mi, ri, r.clone(), current)
                    })
            })
            .next();

        // If no tauri packages found at all, nothing to sync
        if npm_packages.is_empty() && crate_info.is_none() {
            return;
        }

        // Get the crate's target version (either from update or current)
        let _crate_target = crate_info.as_ref().map(|(_, _, r, current)| match r {
            UpdateResult::Update { new_version, .. } => new_version.clone(),
            _ => current.clone(),
        });

        // Get first npm package's current version for reference
        let npm_current = npm_packages.first().map(|(_, _, _, v)| v.as_str());

        // Determine effective versions (after any pending updates)
        let npm_effective = npm_packages.first().map(|(_, _, r, current)| match r {
            UpdateResult::Update { new_version, .. } => new_version.as_str(),
            _ => current.as_str(),
        });

        let crate_effective = crate_info.as_ref().map(|(_, _, r, current)| match r {
            UpdateResult::Update { new_version, .. } => new_version.as_str(),
            _ => current.as_str(),
        });

        // Check if versions already match - if so, no sync needed
        if let (Some(npm_v), Some(crate_v)) = (npm_effective, crate_effective)
            && let (Some(npm_mm), Some(crate_mm)) =
                (extract_major_minor(npm_v), extract_major_minor(crate_v))
            && npm_mm == crate_mm
        {
            return;
        }

        // Versions don't match - need to sync

        // Fetch versions from both registries
        let npm_adapter = self.get_adapter(Language::Node);
        let crate_adapter = self.get_adapter(Language::Rust);

        // Use first npm package name for version fetch (they all share versions)
        let npm_pkg_name = TAURI_NPM_PACKAGES[0];
        let npm_versions = match self.fetch_versions(&*npm_adapter, npm_pkg_name).await {
            Ok(v) => v,
            Err(e) => {
                errors.push(OrchestratorError::RegistryError {
                    package: npm_pkg_name.to_string(),
                    message: format!("Failed to fetch for Tauri sync: {}", e),
                });
                return;
            }
        };

        let crate_versions = match self.fetch_versions(&*crate_adapter, TAURI_CRATE).await {
            Ok(v) => v,
            Err(e) => {
                errors.push(OrchestratorError::RegistryError {
                    package: TAURI_CRATE.to_string(),
                    message: format!("Failed to fetch for Tauri sync: {}", e),
                });
                return;
            }
        };

        // Create sync helper and get synchronized versions
        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let npm_update_result = npm_packages.first().map(|(_, _, r, _)| r);
        let crate_update_result = crate_info.as_ref().map(|(_, _, r, _)| r);

        let (npm_target_version, crate_target_version) = sync.synchronize_with_current(
            npm_current,
            npm_update_result,
            crate_info.as_ref().map(|(_, _, _, v)| v.as_str()),
            crate_update_result,
        );

        // Apply npm version adjustments to all Tauri npm packages
        if let Some(ref target) = npm_target_version {
            for (manifest_idx, result_idx, original, _current) in &npm_packages {
                match original {
                    UpdateResult::Update { dependency, .. } => {
                        // Adjust existing update
                        let adjusted = UpdateResult::update(dependency.clone(), target);
                        summary.manifests[*manifest_idx].results[*result_idx] = adjusted;
                    }
                    UpdateResult::Skip { dependency, .. } => {
                        // Create new update from skip
                        let adjusted = UpdateResult::update(dependency.clone(), target);
                        summary.manifests[*manifest_idx].results[*result_idx] = adjusted;
                        summary.manifests[*manifest_idx].modified = true;
                    }
                }
            }
        }

        // Apply crate version adjustment
        if let Some(ref target) = crate_target_version
            && let Some((manifest_idx, result_idx, original, _)) = crate_info
        {
            match original {
                UpdateResult::Update { dependency, .. } => {
                    let adjusted = UpdateResult::update(dependency, target);
                    summary.manifests[manifest_idx].results[result_idx] = adjusted;
                }
                UpdateResult::Skip { dependency, .. } => {
                    let adjusted = UpdateResult::update(dependency, target);
                    summary.manifests[manifest_idx].results[result_idx] = adjusted;
                    summary.manifests[manifest_idx].modified = true;
                }
            }
        }
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
        assert!(orchestrator.should_process_language(Language::Java));
    }

    #[test]
    fn test_should_process_language_with_filter() {
        let args = make_args(&["depup", "--node"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Node));
        assert!(!orchestrator.should_process_language(Language::Python));
        assert!(!orchestrator.should_process_language(Language::Rust));
        assert!(!orchestrator.should_process_language(Language::Go));
        assert!(!orchestrator.should_process_language(Language::Java));

        // Test Java-only filter
        let args = make_args(&["depup", "--java"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        assert!(orchestrator.should_process_language(Language::Java));
        assert!(!orchestrator.should_process_language(Language::Node));
        assert!(!orchestrator.should_process_language(Language::Python));
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
    fn test_get_adapter_java() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let adapter = orchestrator.get_adapter(Language::Java);
        assert_eq!(adapter.language(), Language::Java);
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

    #[tokio::test]
    async fn test_version_cache_prevents_duplicate_fetches() {
        let args = make_args(&["depup"]);
        let orchestrator = Orchestrator::new(args).unwrap();

        // Pre-populate cache with a known package
        let cache_key = (Language::Node, "lodash".to_string());
        {
            let mut cache = orchestrator.version_cache.lock().await;
            cache.insert(
                cache_key,
                vec![VersionInfo {
                    version: "4.17.21".to_string(),
                    released_at: chrono::Utc::now(),
                }],
            );
        }

        // Fetch the same package — should return cached result without network access
        let adapter = orchestrator.get_adapter(Language::Node);
        let result = orchestrator.fetch_versions(&*adapter, "lodash").await;

        assert!(result.is_ok());
        let versions = result.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "4.17.21");
    }

    #[tokio::test]
    async fn test_run_directories_with_root_included() {
        let dir = TempDir::new().unwrap();

        // Create root manifest
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // Create subdirectory with manifest
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub").join("Cargo.toml"),
            "[package]\nname = \"sub\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        // Build directories list using DepupConfig::directories_with_root
        let config = crate::config::DepupConfig {
            directories: vec![dir.path().join("sub")],
        };
        let dirs = config.directories_with_root(dir.path());

        assert_eq!(dirs.len(), 2);

        // Run orchestrator with these directories (dry-run, no network)
        let args = make_args_with_path(dir.path(), &["--dry-run"]);
        let orchestrator = Orchestrator::new(args).unwrap();
        let result = orchestrator.run_directories(&dirs).await;

        // Both directories' manifests should be detected
        // (they have no dependencies so 0 updates, but no errors either)
        assert!(result.errors.is_empty());
    }
}
