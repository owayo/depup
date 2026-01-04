//! Text output formatter for human-readable display
//!
//! This module provides:
//! - Human-readable update result display with colors
//! - Semantic version change type indication (major/minor/patch)
//! - Production vs development dependency grouping
//! - Skipped package display with reasons
//! - Summary with detailed breakdown

use crate::domain::{Language, ManifestUpdateResult, SkipReason, UpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use crate::output::{OutputFormatter, Verbosity};
use chrono::{DateTime, Utc};
use colored::Colorize;
use std::io::Write;

/// Semantic version change type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionChangeType {
    /// Major version change (breaking)
    Major,
    /// Minor version change (features)
    Minor,
    /// Patch version change (fixes)
    Patch,
    /// Unknown or unparseable
    Unknown,
}

impl VersionChangeType {
    /// Determine the change type between two versions
    pub fn from_versions(old: &str, new: &str) -> Self {
        let parse = |v: &str| -> Option<(u64, u64, u64)> {
            let v = v.strip_prefix('v').unwrap_or(v);
            // Split by . and - to handle prerelease suffixes
            let parts: Vec<&str> = v.split(['.', '-']).collect();
            if parts.len() >= 3 {
                Some((
                    parts[0].parse().ok()?,
                    parts[1].parse().ok()?,
                    parts[2].parse().ok()?,
                ))
            } else if parts.len() == 2 {
                Some((parts[0].parse().ok()?, parts[1].parse().ok()?, 0))
            } else if parts.len() == 1 {
                Some((parts[0].parse().ok()?, 0, 0))
            } else {
                None
            }
        };

        match (parse(old), parse(new)) {
            (Some((old_major, old_minor, _)), Some((new_major, new_minor, _))) => {
                if new_major != old_major {
                    VersionChangeType::Major
                } else if new_minor != old_minor {
                    VersionChangeType::Minor
                } else {
                    VersionChangeType::Patch
                }
            }
            _ => VersionChangeType::Unknown,
        }
    }

    /// Get the display label with color
    pub fn colored_label(&self) -> String {
        match self {
            VersionChangeType::Major => "major".red().bold().to_string(),
            VersionChangeType::Minor => "minor".yellow().to_string(),
            VersionChangeType::Patch => "patch".green().to_string(),
            VersionChangeType::Unknown => "?".dimmed().to_string(),
        }
    }

    /// Get the plain label
    pub fn label(&self) -> &'static str {
        match self {
            VersionChangeType::Major => "major",
            VersionChangeType::Minor => "minor",
            VersionChangeType::Patch => "patch",
            VersionChangeType::Unknown => "?",
        }
    }
}

/// Text formatter for human-readable output
pub struct TextFormatter {
    /// Verbosity level
    verbosity: Verbosity,
    /// Whether this is a dry-run
    dry_run: bool,
    /// Whether to use colors
    color: bool,
}

impl TextFormatter {
    /// Create a new text formatter
    pub fn new(verbosity: Verbosity, dry_run: bool) -> Self {
        Self {
            verbosity,
            dry_run,
            color: true,
        }
    }

    /// Create a new text formatter with color option
    pub fn with_color(verbosity: Verbosity, dry_run: bool, color: bool) -> Self {
        Self {
            verbosity,
            dry_run,
            color,
        }
    }

    /// Get the dry-run prefix if applicable
    fn dry_run_prefix(&self) -> String {
        if self.dry_run {
            if self.color {
                format!("{} ", "(dry-run)".cyan())
            } else {
                "(dry-run) ".to_string()
            }
        } else {
            String::new()
        }
    }

    /// Format a skip reason for display
    fn format_skip_reason(&self, reason: &SkipReason) -> String {
        match reason {
            SkipReason::Pinned => "pinned".to_string(),
            SkipReason::AlreadyLatest => "latest".to_string(),
            SkipReason::Excluded => "excluded".to_string(),
            SkipReason::NotInOnlyList => "not in --only".to_string(),
            SkipReason::FetchFailed(msg) => format!("fetch failed: {}", msg),
            SkipReason::LanguageFiltered => "filtered".to_string(),
            SkipReason::NoSuitableVersion => "no suitable version".to_string(),
            SkipReason::ParseError(msg) => format!("parse error: {}", msg),
        }
    }

    /// Calculate the maximum package name length for alignment
    fn max_name_length(&self, results: &[&UpdateResult]) -> usize {
        results
            .iter()
            .map(|r| match r {
                UpdateResult::Update { dependency, .. } => dependency.name.len(),
                UpdateResult::Skip { dependency, .. } => dependency.name.len(),
            })
            .max()
            .unwrap_or(0)
    }

    /// Format a single update line
    fn format_update_line(
        &self,
        name: &str,
        old_version: &str,
        new_version: &str,
        is_dev: bool,
        released_at: Option<DateTime<Utc>>,
        max_name_len: usize,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let change_type = VersionChangeType::from_versions(old_version, new_version);
        let dev_marker = if is_dev { " 🔧" } else { "" };

        // Format release date
        let date_display = released_at
            .map(|d| format!(" ({})", d.format("%Y/%m/%d %H:%M")))
            .unwrap_or_default();

        if self.color {
            let name_display = format!("{:width$}", name, width = max_name_len);
            let arrow = "→".dimmed();
            let change_label = change_type.colored_label();
            let dev_display = if is_dev {
                " 🔧".dimmed().to_string()
            } else {
                String::new()
            };
            let date_colored = released_at
                .map(|d| {
                    format!(" ({})", d.format("%Y/%m/%d %H:%M"))
                        .dimmed()
                        .to_string()
                })
                .unwrap_or_default();

            writeln!(
                writer,
                "  {} {} {} {} [{}]{}{}",
                name_display,
                old_version.dimmed(),
                arrow,
                new_version.bright_white().bold(),
                change_label,
                date_colored,
                dev_display
            )
        } else {
            writeln!(
                writer,
                "  {:width$} {} -> {} [{}]{}{}",
                name,
                old_version,
                new_version,
                change_type.label(),
                date_display,
                dev_marker,
                width = max_name_len
            )
        }
    }

    /// Format a single skip line
    fn format_skip_line(
        &self,
        name: &str,
        reason: &SkipReason,
        max_name_len: usize,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let reason_str = self.format_skip_reason(reason);

        if self.color {
            let name_display = format!("{:width$}", name, width = max_name_len);
            writeln!(
                writer,
                "  {} {}",
                name_display.dimmed(),
                format!("({})", reason_str).dimmed()
            )
        } else {
            writeln!(
                writer,
                "  {:width$} ({})",
                name,
                reason_str,
                width = max_name_len
            )
        }
    }

    /// Format manifest with grouped updates
    fn format_manifest_grouped(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();

        // Collect updates and skips
        let updates: Vec<_> = manifest.updates().collect();
        let skips: Vec<_> = manifest.skips().collect();

        // Skip empty manifests
        if updates.is_empty() && (self.verbosity != Verbosity::Verbose || skips.is_empty()) {
            return Ok(());
        }

        // Count updates and skips
        let update_count = updates.len();
        let skip_count = skips.len();

        // Separate production and dev dependencies
        let (prod_updates, dev_updates): (Vec<&UpdateResult>, Vec<&UpdateResult>) =
            updates.into_iter().partition(|r| {
                if let UpdateResult::Update { dependency, .. } = r {
                    !dependency.is_dev
                } else {
                    true
                }
            });

        // Write manifest header with counts
        let path_display = manifest.path.display().to_string();
        if self.color {
            let lang_display = format!("({})", manifest.language);
            write!(writer, "{}", prefix)?;
            write!(writer, "{}", path_display.bold())?;
            write!(writer, " {}", lang_display.dimmed())?;
            writeln!(
                writer,
                " — {} {}, {} {}",
                update_count.to_string().green(),
                if update_count == 1 {
                    "update"
                } else {
                    "updates"
                },
                skip_count.to_string().dimmed(),
                if skip_count == 1 { "skip" } else { "skips" }
            )?;
        } else {
            writeln!(
                writer,
                "{}{} ({}) — {} updates, {} skips",
                prefix, path_display, manifest.language, update_count, skip_count
            )?;
        }

        // Get max name length for alignment (use partitioned vectors)
        let all_results: Vec<&UpdateResult> = prod_updates
            .iter()
            .chain(dev_updates.iter())
            .copied()
            .collect();
        let max_name_len = self.max_name_length(&all_results).max(20);

        // Write production dependencies
        if !prod_updates.is_empty() {
            for result in &prod_updates {
                if let UpdateResult::Update {
                    dependency,
                    new_version,
                    released_at,
                    ..
                } = result
                {
                    self.format_update_line(
                        &dependency.name,
                        &dependency.version_spec.version,
                        new_version,
                        false,
                        *released_at,
                        max_name_len,
                        writer,
                    )?;
                }
            }
        }

        // Write dev dependencies
        if !dev_updates.is_empty() {
            for result in &dev_updates {
                if let UpdateResult::Update {
                    dependency,
                    new_version,
                    released_at,
                    ..
                } = result
                {
                    self.format_update_line(
                        &dependency.name,
                        &dependency.version_spec.version,
                        new_version,
                        true,
                        *released_at,
                        max_name_len,
                        writer,
                    )?;
                }
            }
        }

        // Write skips in verbose mode
        if self.verbosity == Verbosity::Verbose && !skips.is_empty() {
            writeln!(writer)?;
            if self.color {
                writeln!(writer, "  {}", "Skipped:".dimmed())?;
            } else {
                writeln!(writer, "  Skipped:")?;
            }
            let skip_results: Vec<&UpdateResult> = skips.iter().copied().collect();
            let skip_max_len = self.max_name_length(&skip_results).max(20);
            for result in &skips {
                if let UpdateResult::Skip { dependency, reason } = result {
                    self.format_skip_line(&dependency.name, reason, skip_max_len, writer)?;
                }
            }
        }

        writeln!(writer)?;
        Ok(())
    }

    /// Count updates by change type
    fn count_by_change_type(&self, summary: &UpdateSummary) -> (usize, usize, usize, usize) {
        let mut major = 0;
        let mut minor = 0;
        let mut patch = 0;
        let mut unknown = 0;

        for manifest in &summary.manifests {
            for result in manifest.updates() {
                if let UpdateResult::Update {
                    dependency,
                    new_version,
                    ..
                } = result
                {
                    match VersionChangeType::from_versions(
                        &dependency.version_spec.version,
                        new_version,
                    ) {
                        VersionChangeType::Major => major += 1,
                        VersionChangeType::Minor => minor += 1,
                        VersionChangeType::Patch => patch += 1,
                        VersionChangeType::Unknown => unknown += 1,
                    }
                }
            }
        }

        (major, minor, patch, unknown)
    }

    /// Count skips by reason
    fn count_by_skip_reason(&self, summary: &UpdateSummary) -> Vec<(String, usize)> {
        use std::collections::HashMap;
        let mut counts: HashMap<String, usize> = HashMap::new();

        for manifest in &summary.manifests {
            for result in manifest.skips() {
                if let UpdateResult::Skip { reason, .. } = result {
                    let key = self.format_skip_reason(reason);
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
        }

        let mut result: Vec<_> = counts.into_iter().collect();
        result.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by count descending
        result
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
            self.format_manifest_grouped(manifest, writer)?;
        }

        // Format errors if any
        if !result.errors.is_empty() && self.verbosity != Verbosity::Quiet {
            if self.color {
                writeln!(writer, "{}:", "Errors".red().bold())?;
            } else {
                writeln!(writer, "Errors:")?;
            }
            for error in &result.errors {
                if self.color {
                    writeln!(writer, "  {} {}", "✗".red(), error)?;
                } else {
                    writeln!(writer, "  - {}", error)?;
                }
            }
            writeln!(writer)?;
        }

        // Format summary
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
                if self.color {
                    writeln!(
                        writer,
                        "{}{} {}",
                        prefix,
                        updates.to_string().green(),
                        "updated"
                    )?;
                } else {
                    writeln!(writer, "{}{} updated", prefix, updates)?;
                }
            } else {
                if self.color {
                    writeln!(writer, "{}{}", prefix, "No updates".dimmed())?;
                } else {
                    writeln!(writer, "{}No updates", prefix)?;
                }
            }
            return Ok(());
        }

        // Count by change type
        let (major, minor, patch, unknown) = self.count_by_change_type(summary);

        // Normal/verbose output
        if self.color {
            writeln!(writer, "{}{}:", prefix, "Summary".bold())?;

            // Update breakdown
            if updates > 0 {
                write!(
                    writer,
                    "  {} package(s) updated",
                    updates.to_string().green()
                )?;
                write!(writer, " (")?;
                let mut parts = Vec::new();
                if major > 0 {
                    parts.push(format!("{} {}", major.to_string().red(), "major"));
                }
                if minor > 0 {
                    parts.push(format!("{} {}", minor.to_string().yellow(), "minor"));
                }
                if patch > 0 {
                    parts.push(format!("{} {}", patch.to_string().green(), "patch"));
                }
                if unknown > 0 {
                    parts.push(format!("{} {}", unknown.to_string().dimmed(), "other"));
                }
                write!(writer, "{}", parts.join(", "))?;
                writeln!(writer, ")")?;
            } else {
                writeln!(writer, "  {}", "No packages updated".dimmed())?;
            }

            // Skip summary
            if skips > 0 {
                write!(
                    writer,
                    "  {} package(s) skipped",
                    skips.to_string().dimmed()
                )?;
                if self.verbosity == Verbosity::Verbose {
                    let skip_counts = self.count_by_skip_reason(summary);
                    if !skip_counts.is_empty() {
                        write!(writer, " (")?;
                        let parts: Vec<_> = skip_counts
                            .iter()
                            .map(|(reason, count)| format!("{} {}", count, reason))
                            .collect();
                        write!(writer, "{}", parts.join(", ").dimmed())?;
                        write!(writer, ")")?;
                    }
                }
                writeln!(writer)?;
            }
        } else {
            writeln!(writer, "{}Summary:", prefix)?;
            if updates > 0 {
                let mut parts = Vec::new();
                if major > 0 {
                    parts.push(format!("{} major", major));
                }
                if minor > 0 {
                    parts.push(format!("{} minor", minor));
                }
                if patch > 0 {
                    parts.push(format!("{} patch", patch));
                }
                if unknown > 0 {
                    parts.push(format!("{} other", unknown));
                }
                writeln!(
                    writer,
                    "  {} package(s) updated ({})",
                    updates,
                    parts.join(", ")
                )?;
            } else {
                writeln!(writer, "  No packages updated")?;
            }
            writeln!(writer, "  {} package(s) skipped", skips)?;
        }

        // Verbose: show breakdown by language
        if self.verbosity == Verbosity::Verbose {
            writeln!(writer)?;
            if self.color {
                writeln!(writer, "{}:", "By language".dimmed())?;
            } else {
                writeln!(writer, "By language:")?;
            }
            for language in Language::all() {
                let manifests: Vec<_> = summary.by_language(*language).collect();
                if !manifests.is_empty() {
                    let lang_updates: usize = manifests.iter().map(|m| m.update_count()).sum();
                    let lang_skips: usize = manifests.iter().map(|m| m.skip_count()).sum();
                    if self.color {
                        writeln!(
                            writer,
                            "  {}: {} updated, {} skipped",
                            language.to_string().cyan(),
                            lang_updates.to_string().green(),
                            lang_skips.to_string().dimmed()
                        )?;
                    } else {
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
        self.format_manifest_grouped(manifest, writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use std::path::PathBuf;

    fn sample_dependency(name: &str, version: &str, is_dev: bool) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, &format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, is_dev, Language::Node)
    }

    fn create_test_result() -> OrchestratorResult {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // Production dependency - minor update
        let dep1 = sample_dependency("lodash", "4.17.21", false);
        manifest.add_result(UpdateResult::update(dep1, "4.18.0"));

        // Dev dependency - patch update
        let dep2 = sample_dependency("typescript", "5.0.0", true);
        manifest.add_result(UpdateResult::update(dep2, "5.0.1"));

        // Skipped
        let dep3 = sample_dependency("express", "4.18.0", false);
        manifest.add_result(UpdateResult::skip(dep3, SkipReason::AlreadyLatest));

        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_version_change_type_major() {
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "2.0.0"),
            VersionChangeType::Major
        );
        assert_eq!(
            VersionChangeType::from_versions("0.9.0", "1.0.0"),
            VersionChangeType::Major
        );
    }

    #[test]
    fn test_version_change_type_minor() {
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.1.0"),
            VersionChangeType::Minor
        );
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.5.0"),
            VersionChangeType::Minor
        );
    }

    #[test]
    fn test_version_change_type_patch() {
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.0.1"),
            VersionChangeType::Patch
        );
        assert_eq!(
            VersionChangeType::from_versions("1.0.0", "1.0.10"),
            VersionChangeType::Patch
        );
    }

    #[test]
    fn test_version_change_type_with_v_prefix() {
        assert_eq!(
            VersionChangeType::from_versions("v1.0.0", "v2.0.0"),
            VersionChangeType::Major
        );
    }

    #[test]
    fn test_version_change_type_short_versions() {
        assert_eq!(
            VersionChangeType::from_versions("1.0", "2.0"),
            VersionChangeType::Major
        );
        assert_eq!(
            VersionChangeType::from_versions("1", "2"),
            VersionChangeType::Major
        );
    }

    #[test]
    fn test_text_formatter_new() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        assert_eq!(formatter.verbosity, Verbosity::Normal);
        assert!(!formatter.dry_run);
        assert!(formatter.color);
    }

    #[test]
    fn test_dry_run_prefix() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, true, false);
        assert_eq!(formatter.dry_run_prefix(), "(dry-run) ");

        let formatter = TextFormatter::with_color(Verbosity::Normal, false, false);
        assert_eq!(formatter.dry_run_prefix(), "");
    }

    #[test]
    fn test_format_skip_reason() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);

        assert_eq!(formatter.format_skip_reason(&SkipReason::Pinned), "pinned");
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::AlreadyLatest),
            "latest"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::Excluded),
            "excluded"
        );
        assert_eq!(
            formatter.format_skip_reason(&SkipReason::NotInOnlyList),
            "not in --only"
        );
        assert!(formatter
            .format_skip_reason(&SkipReason::FetchFailed("timeout".to_string()))
            .contains("fetch failed"));
    }

    #[test]
    fn test_format_normal() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, false, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("package.json"));
        assert!(output_str.contains("lodash"));
        assert!(output_str.contains("4.17.21"));
        assert!(output_str.contains("4.18.0"));
        assert!(output_str.contains("[minor]"));
        assert!(output_str.contains("typescript"));
        assert!(output_str.contains("[patch]"));
        assert!(output_str.contains("🔧"));
        assert!(output_str.contains("Summary:"));
        assert!(output_str.contains("2 package(s) updated"));
    }

    #[test]
    fn test_format_quiet() {
        let formatter = TextFormatter::with_color(Verbosity::Quiet, false, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Quiet mode should be minimal
        assert!(output_str.contains("2 updated"));
        assert!(!output_str.contains("Summary:"));
    }

    #[test]
    fn test_format_verbose() {
        let formatter = TextFormatter::with_color(Verbosity::Verbose, false, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Verbose mode should show skipped packages and language breakdown
        assert!(output_str.contains("express"));
        assert!(output_str.contains("Skipped:"));
        assert!(output_str.contains("latest"));
        assert!(output_str.contains("By language:"));
    }

    #[test]
    fn test_format_dry_run() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, true, false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("(dry-run)"));
    }

    #[test]
    fn test_format_summary_no_updates() {
        let formatter = TextFormatter::with_color(Verbosity::Normal, false, false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("No packages updated"));
    }

    #[test]
    fn test_format_summary_quiet_no_updates() {
        let formatter = TextFormatter::with_color(Verbosity::Quiet, false, false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("No updates"));
    }

    #[test]
    fn test_count_by_change_type() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // Major
        let dep1 = sample_dependency("pkg1", "1.0.0", false);
        manifest.add_result(UpdateResult::update(dep1, "2.0.0"));

        // Minor
        let dep2 = sample_dependency("pkg2", "1.0.0", false);
        manifest.add_result(UpdateResult::update(dep2, "1.1.0"));

        // Patch
        let dep3 = sample_dependency("pkg3", "1.0.0", false);
        manifest.add_result(UpdateResult::update(dep3, "1.0.1"));

        summary.add_manifest(manifest);

        let (major, minor, patch, unknown) = formatter.count_by_change_type(&summary);
        assert_eq!(major, 1);
        assert_eq!(minor, 1);
        assert_eq!(patch, 1);
        assert_eq!(unknown, 0);
    }

    #[test]
    fn test_count_by_change_type_with_unknown() {
        let formatter = TextFormatter::new(Verbosity::Normal, false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // Unknown (non-semver version)
        let dep1 = sample_dependency("pkg1", "latest", false);
        manifest.add_result(UpdateResult::update(dep1, "2.0.0"));

        summary.add_manifest(manifest);

        let (major, minor, patch, unknown) = formatter.count_by_change_type(&summary);
        assert_eq!(major, 0);
        assert_eq!(minor, 0);
        assert_eq!(patch, 0);
        assert_eq!(unknown, 1);
    }
}
