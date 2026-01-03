//! Text output formatter for human-readable display
//!
//! This module provides:
//! - Human-readable update result display
//! - Pinned version skip display
//! - Verbose mode with additional information
//! - Quiet mode with minimal output
//! - Dry-run mode indication

use crate::domain::{Language, ManifestUpdateResult, SkipReason, UpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use crate::output::{OutputFormatter, Verbosity};
use std::io::Write;

/// Text formatter for human-readable output
pub struct TextFormatter {
    /// Verbosity level
    verbosity: Verbosity,
    /// Whether this is a dry-run
    dry_run: bool,
}

impl TextFormatter {
    /// Create a new text formatter
    pub fn new(verbosity: Verbosity, dry_run: bool) -> Self {
        Self { verbosity, dry_run }
    }

    /// Get the dry-run prefix if applicable
    fn dry_run_prefix(&self) -> &'static str {
        if self.dry_run {
            "(dry-run) "
        } else {
            ""
        }
    }

    /// Write a separator line
    fn write_separator(&self, writer: &mut dyn Write) -> std::io::Result<()> {
        if self.verbosity != Verbosity::Quiet {
            writeln!(writer)?;
        }
        Ok(())
    }

    /// Format a skip reason for display
    fn format_skip_reason(&self, reason: &SkipReason) -> String {
        match reason {
            SkipReason::Pinned => "pinned version".to_string(),
            SkipReason::AlreadyLatest => "already latest".to_string(),
            SkipReason::Excluded => "excluded by filter".to_string(),
            SkipReason::NotInOnlyList => "not in --only list".to_string(),
            SkipReason::FetchFailed(msg) => format!("fetch failed: {}", msg),
            SkipReason::LanguageFiltered => "language filtered".to_string(),
            SkipReason::NoSuitableVersion => "no suitable version".to_string(),
            SkipReason::ParseError(msg) => format!("parse error: {}", msg),
        }
    }
}

impl OutputFormatter for TextFormatter {
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()> {
        // In quiet mode, only show summary
        if self.verbosity == Verbosity::Quiet {
            return self.format_summary(&result.summary, writer);
        }

        // Format each manifest
        for manifest in &result.summary.manifests {
            self.format_manifest(manifest, writer)?;
        }

        // Format errors if any
        if !result.errors.is_empty() && self.verbosity != Verbosity::Quiet {
            self.write_separator(writer)?;
            writeln!(writer, "Errors:")?;
            for error in &result.errors {
                writeln!(writer, "  - {}", error)?;
            }
        }

        // Format summary
        self.write_separator(writer)?;
        self.format_summary(&result.summary, writer)?;

        Ok(())
    }

    fn format_summary(
        &self,
        summary: &UpdateSummary,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();
        let updates = summary.total_updates();
        let skips = summary.total_skips();

        if self.verbosity == Verbosity::Quiet {
            // Minimal output
            if updates > 0 {
                writeln!(writer, "{}{} updated", prefix, updates)?;
            } else {
                writeln!(writer, "{}No updates", prefix)?;
            }
        } else {
            // Normal/verbose output
            writeln!(writer, "{}Summary:", prefix)?;
            writeln!(writer, "  {} package(s) updated", updates)?;
            writeln!(writer, "  {} package(s) skipped", skips)?;

            if self.verbosity == Verbosity::Verbose {
                // Show breakdown by language
                writeln!(writer)?;
                writeln!(writer, "By language:")?;
                for language in Language::all() {
                    let manifests: Vec<_> = summary.by_language(*language).collect();
                    if !manifests.is_empty() {
                        let lang_updates: usize = manifests.iter().map(|m| m.update_count()).sum();
                        let lang_skips: usize = manifests.iter().map(|m| m.skip_count()).sum();
                        writeln!(
                            writer,
                            "  {}: {} updated, {} skipped",
                            language, lang_updates, lang_skips
                        )?;
                    }
                }
            }
        }

        Ok(())
    }

    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();

        // Skip empty manifests in quiet mode
        if self.verbosity == Verbosity::Quiet && !manifest.has_updates() {
            return Ok(());
        }

        // Write manifest header
        writeln!(writer, "{}{}", prefix, manifest.path.display())?;

        // Write updates
        for result in manifest.updates() {
            if let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = result
            {
                writeln!(
                    writer,
                    "  {} {} -> {}",
                    dependency.name, dependency.version_spec.version, new_version
                )?;
            }
        }

        // Write skips in verbose mode
        if self.verbosity == Verbosity::Verbose {
            for result in manifest.skips() {
                if let UpdateResult::Skip { dependency, reason } = result {
                    writeln!(
                        writer,
                        "  {} (skipped: {})",
                        dependency.name,
                        self.format_skip_reason(reason)
                    )?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use std::path::PathBuf;

    fn sample_dependency(name: &str, version: &str) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, &format!("^{}", version), version)
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
    fn test_text_formatter_new() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        assert_eq!(formatter.verbosity, Verbosity::Normal);
        assert!(!formatter.dry_run);
    }

    #[test]
    fn test_dry_run_prefix() {
        let formatter = TextFormatter::new(Verbosity::Normal, true);
        assert_eq!(formatter.dry_run_prefix(), "(dry-run) ");

        let formatter = TextFormatter::new(Verbosity::Normal, false);
        assert_eq!(formatter.dry_run_prefix(), "");
    }

    #[test]
    fn test_format_skip_reason() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);

        assert_eq!(
            formatter.format_skip_reason(&SkipReason::Pinned),
            "pinned version"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::AlreadyLatest),
            "already latest"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::Excluded),
            "excluded by filter"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::NotInOnlyList),
            "not in --only list"
        );
        assert!(formatter
            .format_skip_reason(&SkipReason::FetchFailed("timeout".to_string()))
            .contains("fetch failed"));
    }

    #[test]
    fn test_format_normal() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("package.json"));
        assert!(output_str.contains("lodash"));
        assert!(output_str.contains("4.17.21"));
        assert!(output_str.contains("4.18.0"));
        assert!(output_str.contains("Summary:"));
        assert!(output_str.contains("1 package(s) updated"));
        assert!(output_str.contains("1 package(s) skipped"));
    }

    #[test]
    fn test_format_quiet() {
        let formatter = TextFormatter::new(Verbosity::Quiet, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Quiet mode should be minimal
        assert!(output_str.contains("1 updated"));
        assert!(!output_str.contains("Summary:"));
    }

    #[test]
    fn test_format_verbose() {
        let formatter = TextFormatter::new(Verbosity::Verbose, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Verbose mode should show skipped packages
        assert!(output_str.contains("express"));
        assert!(output_str.contains("skipped"));
        assert!(output_str.contains("already latest"));
        assert!(output_str.contains("By language:"));
    }

    #[test]
    fn test_format_dry_run() {
        let formatter = TextFormatter::new(Verbosity::Normal, true);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("(dry-run)"));
    }

    #[test]
    fn test_format_summary_no_updates() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("0 package(s) updated"));
    }

    #[test]
    fn test_format_summary_quiet_no_updates() {
        let formatter = TextFormatter::new(Verbosity::Quiet, false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("No updates"));
    }
}
