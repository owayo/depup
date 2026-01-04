//! JSON output formatter for machine processing
//!
//! This module provides:
//! - JSON serialization of update results
//! - Structured file-by-file update/skip information

use crate::domain::{Language, ManifestUpdateResult, SkipReason, UpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use crate::output::{OutputFormatter, Verbosity};
use serde::Serialize;
use std::io::Write;

/// JSON formatter for machine-readable output
pub struct JsonFormatter {
    /// Verbosity level affects detail in output
    verbosity: Verbosity,
}

impl JsonFormatter {
    /// Create a new JSON formatter
    pub fn new(verbosity: Verbosity) -> Self {
        Self { verbosity }
    }
}

/// JSON representation of the full result
#[derive(Serialize)]
struct JsonOutput {
    /// Whether this was a dry-run
    dry_run: bool,
    /// Summary statistics
    summary: JsonSummary,
    /// Per-manifest results
    manifests: Vec<JsonManifest>,
    /// Errors encountered
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

/// JSON representation of summary statistics
#[derive(Serialize)]
struct JsonSummary {
    /// Total number of updates
    updates: usize,
    /// Total number of skips
    skips: usize,
    /// Breakdown by language
    #[serde(skip_serializing_if = "Vec::is_empty")]
    by_language: Vec<JsonLanguageSummary>,
}

/// JSON representation of per-language summary
#[derive(Serialize)]
struct JsonLanguageSummary {
    /// Language name
    language: String,
    /// Number of updates
    updates: usize,
    /// Number of skips
    skips: usize,
}

/// JSON representation of a manifest result
#[derive(Serialize)]
struct JsonManifest {
    /// Path to the manifest file
    path: String,
    /// Language of the manifest
    language: String,
    /// List of updates
    updates: Vec<JsonUpdate>,
    /// List of skips (only in verbose mode)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skips: Vec<JsonSkip>,
}

/// JSON representation of an update
#[derive(Serialize)]
struct JsonUpdate {
    /// Package name
    name: String,
    /// Old version
    from: String,
    /// New version
    to: String,
    /// Whether it's a dev dependency
    dev: bool,
}

/// JSON representation of a skip
#[derive(Serialize)]
struct JsonSkip {
    /// Package name
    name: String,
    /// Current version
    version: String,
    /// Skip reason
    reason: String,
}

impl JsonFormatter {
    /// Convert skip reason to string
    fn skip_reason_to_string(reason: &SkipReason) -> String {
        match reason {
            SkipReason::Pinned => "pinned".to_string(),
            SkipReason::AlreadyLatest => "already_latest".to_string(),
            SkipReason::Excluded => "excluded".to_string(),
            SkipReason::NotInOnlyList => "not_in_only_list".to_string(),
            SkipReason::FetchFailed(msg) => format!("fetch_failed: {}", msg),
            SkipReason::LanguageFiltered => "language_filtered".to_string(),
            SkipReason::NoSuitableVersion => "no_suitable_version".to_string(),
            SkipReason::ParseError(msg) => format!("parse_error: {}", msg),
        }
    }

    /// Convert manifest result to JSON representation
    fn manifest_to_json(&self, manifest: &ManifestUpdateResult) -> JsonManifest {
        let updates: Vec<JsonUpdate> = manifest
            .updates()
            .filter_map(|result| {
                if let UpdateResult::Update {
                    dependency,
                    new_version,
                    ..
                } = result
                {
                    Some(JsonUpdate {
                        name: dependency.name.clone(),
                        from: dependency.version_spec.version.clone(),
                        to: new_version.clone(),
                        dev: dependency.is_dev,
                    })
                } else {
                    None
                }
            })
            .collect();

        let skips: Vec<JsonSkip> = if self.verbosity == Verbosity::Verbose {
            manifest
                .skips()
                .filter_map(|result| {
                    if let UpdateResult::Skip { dependency, reason } = result {
                        Some(JsonSkip {
                            name: dependency.name.clone(),
                            version: dependency.version_spec.version.clone(),
                            reason: Self::skip_reason_to_string(reason),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        JsonManifest {
            path: manifest.path.display().to_string(),
            language: manifest.language.display_name().to_string(),
            updates,
            skips,
        }
    }
}

impl OutputFormatter for JsonFormatter {
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()> {
        let updates = result.summary.total_updates();
        let skips = result.summary.total_skips();

        let by_language: Vec<JsonLanguageSummary> = if self.verbosity == Verbosity::Verbose {
            Language::all()
                .iter()
                .filter_map(|language| {
                    let manifests: Vec<_> = result.summary.by_language(*language).collect();
                    if manifests.is_empty() {
                        None
                    } else {
                        let lang_updates: usize = manifests.iter().map(|m| m.update_count()).sum();
                        let lang_skips: usize = manifests.iter().map(|m| m.skip_count()).sum();
                        Some(JsonLanguageSummary {
                            language: language.display_name().to_string(),
                            updates: lang_updates,
                            skips: lang_skips,
                        })
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let output = JsonOutput {
            dry_run: result.summary.dry_run,
            summary: JsonSummary {
                updates,
                skips,
                by_language,
            },
            manifests: result
                .summary
                .manifests
                .iter()
                .map(|m| self.manifest_to_json(m))
                .collect(),
            errors: result.errors.iter().map(|e| e.to_string()).collect(),
        };

        let json = serde_json::to_string_pretty(&output).map_err(std::io::Error::other)?;

        writeln!(writer, "{}", json)?;

        Ok(())
    }

    fn format_summary(
        &self,
        summary: &UpdateSummary,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let updates = summary.total_updates();
        let skips = summary.total_skips();

        let output = JsonSummary {
            updates,
            skips,
            by_language: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&output).map_err(std::io::Error::other)?;

        writeln!(writer, "{}", json)?;

        Ok(())
    }

    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let output = self.manifest_to_json(manifest);

        let json = serde_json::to_string_pretty(&output).map_err(std::io::Error::other)?;

        writeln!(writer, "{}", json)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use std::path::PathBuf;

    fn sample_dependency(name: &str, version: &str) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, false, Language::Node)
    }

    fn create_test_result() -> OrchestratorResult {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let dep1 = sample_dependency("lodash", "4.17.21");
        manifest.add_result(UpdateResult::update(dep1, "4.18.0"));

        let dep2 = sample_dependency("express", "4.18.0");
        manifest.add_result(UpdateResult::skip(dep2, SkipReason::AlreadyLatest));

        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_json_formatter_new() {
        let formatter = JsonFormatter::new(Verbosity::Normal);
        assert_eq!(formatter.verbosity, Verbosity::Normal);
    }

    #[test]
    fn test_skip_reason_to_string() {
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::Pinned),
            "pinned"
        );
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::AlreadyLatest),
            "already_latest"
        );
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::Excluded),
            "excluded"
        );
    }

    #[test]
    fn test_format_json() {
        let formatter = JsonFormatter::new(Verbosity::Normal);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Verify it's valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();

        assert_eq!(parsed["dry_run"], false);
        assert_eq!(parsed["summary"]["updates"], 1);
        assert_eq!(parsed["summary"]["skips"], 1);
        assert_eq!(parsed["manifests"][0]["path"], "package.json");
        assert_eq!(parsed["manifests"][0]["updates"][0]["name"], "lodash");
        assert_eq!(parsed["manifests"][0]["updates"][0]["from"], "4.17.21");
        assert_eq!(parsed["manifests"][0]["updates"][0]["to"], "4.18.0");
    }

    #[test]
    fn test_format_json_verbose() {
        let formatter = JsonFormatter::new(Verbosity::Verbose);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();

        // Verbose mode should include skips
        assert!(!parsed["manifests"][0]["skips"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(parsed["manifests"][0]["skips"][0]["name"], "express");
        assert_eq!(
            parsed["manifests"][0]["skips"][0]["reason"],
            "already_latest"
        );

        // Should include by_language breakdown
        assert!(!parsed["summary"]["by_language"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_format_json_quiet() {
        let formatter = JsonFormatter::new(Verbosity::Quiet);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();

        // Quiet mode should not include skips (field is omitted or empty)
        let skips = &parsed["manifests"][0]["skips"];
        assert!(skips.is_null() || skips.as_array().map(|a| a.is_empty()).unwrap_or(true));
    }

    #[test]
    fn test_format_summary() {
        let formatter = JsonFormatter::new(Verbosity::Normal);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();
        assert_eq!(parsed["updates"], 0);
        assert_eq!(parsed["skips"], 0);
    }
}
