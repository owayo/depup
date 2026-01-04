//! Output formatting for update results
//!
//! This module provides:
//! - Text output for human-readable display
//! - JSON output for machine processing
//! - Diff output for showing changes

mod diff;
mod json;
mod text;

pub use diff::DiffFormatter;
pub use json::JsonFormatter;
pub use text::TextFormatter;

use crate::domain::{ManifestUpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use std::io::Write;

/// Output format options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable text output
    #[default]
    Text,
    /// JSON output for machine processing
    Json,
    /// Unified diff format
    Diff,
}

/// Output verbosity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    /// Minimal output
    Quiet,
    /// Normal output
    #[default]
    Normal,
    /// Detailed output with additional information
    Verbose,
}

/// Configuration for output formatting
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Output format (text, json, diff)
    pub format: OutputFormat,
    /// Verbosity level
    pub verbosity: Verbosity,
    /// Whether this is a dry-run
    pub dry_run: bool,
    /// Whether to use colors (when supported)
    pub color: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format: OutputFormat::default(),
            verbosity: Verbosity::default(),
            dry_run: false,
            color: true,
        }
    }
}

impl OutputConfig {
    /// Create a new output configuration
    pub fn new(format: OutputFormat, verbosity: Verbosity, dry_run: bool) -> Self {
        Self {
            format,
            verbosity,
            dry_run,
            color: true,
        }
    }

    /// Create configuration from CLI arguments
    pub fn from_cli(json: bool, diff: bool, verbose: bool, quiet: bool, dry_run: bool) -> Self {
        let format = if json {
            OutputFormat::Json
        } else if diff {
            OutputFormat::Diff
        } else {
            OutputFormat::Text
        };

        let verbosity = if quiet {
            Verbosity::Quiet
        } else if verbose {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        };

        Self {
            format,
            verbosity,
            dry_run,
            color: true,
        }
    }
}

/// Trait for output formatters
pub trait OutputFormatter {
    /// Format and write the orchestrator result
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()>;

    /// Format and write just the summary
    fn format_summary(
        &self,
        summary: &UpdateSummary,
        writer: &mut dyn Write,
    ) -> std::io::Result<()>;

    /// Format and write a single manifest result
    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()>;
}

/// Create an output formatter based on configuration
pub fn create_formatter(config: OutputConfig) -> Box<dyn OutputFormatter> {
    match config.format {
        OutputFormat::Text => Box::new(TextFormatter::new(config.verbosity, config.dry_run)),
        OutputFormat::Json => Box::new(JsonFormatter::new(config.verbosity)),
        OutputFormat::Diff => Box::new(DiffFormatter::new(config.dry_run)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_default() {
        assert_eq!(OutputFormat::default(), OutputFormat::Text);
    }

    #[test]
    fn test_verbosity_default() {
        assert_eq!(Verbosity::default(), Verbosity::Normal);
    }

    #[test]
    fn test_output_config_default() {
        let config = OutputConfig::default();
        assert_eq!(config.format, OutputFormat::Text);
        assert_eq!(config.verbosity, Verbosity::Normal);
        assert!(!config.dry_run);
        assert!(config.color);
    }

    #[test]
    fn test_output_config_new() {
        let config = OutputConfig::new(OutputFormat::Json, Verbosity::Quiet, true);
        assert_eq!(config.format, OutputFormat::Json);
        assert_eq!(config.verbosity, Verbosity::Quiet);
        assert!(config.dry_run);
    }

    #[test]
    fn test_output_config_from_cli_json() {
        let config = OutputConfig::from_cli(true, false, false, false, false);
        assert_eq!(config.format, OutputFormat::Json);
        assert_eq!(config.verbosity, Verbosity::Normal);
    }

    #[test]
    fn test_output_config_from_cli_diff() {
        let config = OutputConfig::from_cli(false, true, false, false, false);
        assert_eq!(config.format, OutputFormat::Diff);
    }

    #[test]
    fn test_output_config_from_cli_verbose() {
        let config = OutputConfig::from_cli(false, false, true, false, false);
        assert_eq!(config.verbosity, Verbosity::Verbose);
    }

    #[test]
    fn test_output_config_from_cli_quiet() {
        let config = OutputConfig::from_cli(false, false, false, true, false);
        assert_eq!(config.verbosity, Verbosity::Quiet);
    }

    #[test]
    fn test_output_config_from_cli_dry_run() {
        let config = OutputConfig::from_cli(false, false, false, false, true);
        assert!(config.dry_run);
    }
}
