//! 更新結果の出力フォーマット
//!
//! このモジュールが提供するもの:
//! - 人間が読みやすいテキスト出力
//! - 機械処理用 JSON 出力
//! - 変更内容を表示する diff 出力

mod diff;
mod json;
mod text;

pub use diff::DiffFormatter;
pub use json::JsonFormatter;
pub use text::TextFormatter;

use crate::domain::{ManifestUpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use std::io::Write;

/// 出力フォーマットオプション
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// 人間が読みやすいテキスト出力
    #[default]
    Text,
    /// 機械処理用 JSON 出力
    Json,
    /// unified diff 形式
    Diff,
}

/// 出力の詳細度レベル
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    /// 最小限の出力
    Quiet,
    /// 通常の出力
    #[default]
    Normal,
    /// 追加情報付きの詳細出力
    Verbose,
}

/// 出力フォーマットの設定
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// 出力フォーマット (text, json, diff)
    pub format: OutputFormat,
    /// 詳細度レベル
    pub verbosity: Verbosity,
    /// dry-run かどうか
    pub dry_run: bool,
    /// カラー表示を使用するか (対応時)
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
    /// 新しい出力設定を作成
    pub fn new(format: OutputFormat, verbosity: Verbosity, dry_run: bool) -> Self {
        Self {
            format,
            verbosity,
            dry_run,
            color: true,
        }
    }

    /// CLI 引数から設定を作成
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

/// 出力フォーマッタのトレイト
pub trait OutputFormatter {
    /// オーケストレータの結果をフォーマットして書き出す
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()>;

    /// サマリのみをフォーマットして書き出す
    fn format_summary(
        &self,
        summary: &UpdateSummary,
        writer: &mut dyn Write,
    ) -> std::io::Result<()>;

    /// 単一マニフェストの結果をフォーマットして書き出す
    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()>;
}

/// 設定に基づいて出力フォーマッタを作成
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

    #[test]
    fn test_create_formatter_text() {
        let config = OutputConfig::new(OutputFormat::Text, Verbosity::Normal, false);
        let _formatter = create_formatter(config);
    }

    #[test]
    fn test_create_formatter_json() {
        let config = OutputConfig::new(OutputFormat::Json, Verbosity::Normal, false);
        let _formatter = create_formatter(config);
    }

    #[test]
    fn test_create_formatter_diff() {
        let config = OutputConfig::new(OutputFormat::Diff, Verbosity::Normal, false);
        let _formatter = create_formatter(config);
    }

    #[test]
    fn test_output_format_debug() {
        let text = format!("{:?}", OutputFormat::Text);
        assert_eq!(text, "Text");
        let json = format!("{:?}", OutputFormat::Json);
        assert_eq!(json, "Json");
        let diff = format!("{:?}", OutputFormat::Diff);
        assert_eq!(diff, "Diff");
    }

    #[test]
    fn test_verbosity_debug() {
        assert_eq!(format!("{:?}", Verbosity::Quiet), "Quiet");
        assert_eq!(format!("{:?}", Verbosity::Normal), "Normal");
        assert_eq!(format!("{:?}", Verbosity::Verbose), "Verbose");
    }

    #[test]
    fn test_output_config_debug() {
        let config = OutputConfig::default();
        let debug = format!("{:?}", config);
        assert!(debug.contains("OutputConfig"));
    }
}
