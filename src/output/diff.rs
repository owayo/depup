//! 変更内容を unified diff 風に表示するフォーマッタ。
//!
//! 提供内容:
//! - unified diff 形式の表示
//! - 更新前後バージョンの比較

use crate::domain::{ManifestUpdateResult, UpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use crate::output::OutputFormatter;
use std::io::Write;

/// バージョン変更を diff 形式で表示するフォーマッタ
pub struct DiffFormatter {
    /// dry-run かどうか
    dry_run: bool,
}

impl DiffFormatter {
    /// 新しい `DiffFormatter` を作る
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }

    /// 必要なら dry-run 接頭辞を返す
    fn dry_run_prefix(&self) -> &'static str {
        if self.dry_run { "(dry-run) " } else { "" }
    }
}

impl OutputFormatter for DiffFormatter {
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();

        for manifest in &result.summary.manifests {
            // 更新のないマニフェストは出力しない
            if !manifest.has_updates() {
                continue;
            }

            // diff ヘッダを書く
            writeln!(writer, "{}--- a/{}", prefix, manifest.path.display())?;
            writeln!(writer, "{}+++ b/{}", prefix, manifest.path.display())?;

            // 各更新を diff hunk として書く
            for result in manifest.updates() {
                if let UpdateResult::Update {
                    dependency,
                    new_version,
                    ..
                } = result
                {
                    let old_version = &dependency.version_spec.raw;
                    let new_formatted = dependency
                        .version_spec
                        .try_format_updated(new_version)
                        .unwrap_or_else(|| dependency.version_spec.raw.clone());

                    writeln!(writer, "@@ {} @@", dependency.name)?;
                    writeln!(writer, "-  \"{}\": \"{}\"", dependency.name, old_version)?;
                    writeln!(writer, "+  \"{}\": \"{}\"", dependency.name, new_formatted)?;
                }
            }

            writeln!(writer)?;
        }

        // 最後にサマリを書く
        let updates = result.summary.total_updates();
        writeln!(
            writer,
            "{}# {} package(s) would be updated",
            prefix, updates
        )?;

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

        writeln!(
            writer,
            "{}# {} package(s) updated, {} skipped",
            prefix, updates, skips
        )?;

        Ok(())
    }

    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();

        if !manifest.has_updates() {
            return Ok(());
        }

        // diff ヘッダを書く
        writeln!(writer, "{}--- a/{}", prefix, manifest.path.display())?;
        writeln!(writer, "{}+++ b/{}", prefix, manifest.path.display())?;

        // 各更新を diff hunk として書く
        for result in manifest.updates() {
            if let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = result
            {
                let old_version = &dependency.version_spec.raw;
                let new_formatted = dependency
                    .version_spec
                    .try_format_updated(new_version)
                    .unwrap_or_else(|| dependency.version_spec.raw.clone());

                writeln!(writer, "@@ {} @@", dependency.name)?;
                writeln!(writer, "-  \"{}\": \"{}\"", dependency.name, old_version)?;
                writeln!(writer, "+  \"{}\": \"{}\"", dependency.name, new_formatted)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
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

        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_diff_formatter_new() {
        let formatter = DiffFormatter::new(false);
        assert!(!formatter.dry_run);
    }

    #[test]
    fn test_dry_run_prefix() {
        let formatter = DiffFormatter::new(true);
        assert_eq!(formatter.dry_run_prefix(), "(dry-run) ");

        let formatter = DiffFormatter::new(false);
        assert_eq!(formatter.dry_run_prefix(), "");
    }

    #[test]
    fn test_format_diff() {
        let formatter = DiffFormatter::new(false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // diff 形式で出力されることを確認する
        assert!(output_str.contains("--- a/package.json"));
        assert!(output_str.contains("+++ b/package.json"));
        assert!(output_str.contains("@@ lodash @@"));
        assert!(output_str.contains("-  \"lodash\": \"^4.17.21\""));
        assert!(output_str.contains("+  \"lodash\": \"^4.18.0\""));
        assert!(output_str.contains("# 1 package(s) would be updated"));
    }

    #[test]
    fn test_format_diff_dry_run() {
        let formatter = DiffFormatter::new(true);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("(dry-run)"));
    }

    #[test]
    fn test_format_diff_no_updates() {
        let formatter = DiffFormatter::new(false);
        let summary = UpdateSummary::new(false);
        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // サマリ行だけが出力される
        assert!(output_str.contains("# 0 package(s) would be updated"));
        assert!(!output_str.contains("---"));
    }

    #[test]
    fn test_format_manifest() {
        let formatter = DiffFormatter::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);
        let dep = sample_dependency("lodash", "4.17.21");
        manifest.add_result(UpdateResult::update(dep, "4.18.0"));

        let mut output = Vec::new();
        formatter.format_manifest(&manifest, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("--- a/package.json"));
        assert!(output_str.contains("lodash"));
    }

    #[test]
    fn test_format_summary() {
        let formatter = DiffFormatter::new(false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("# 0 package(s) updated"));
    }

    #[test]
    fn test_format_summary_with_skips() {
        // スキップ数もサマリに含まれる
        let formatter = DiffFormatter::new(false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let dep = sample_dependency("express", "4.18.0");
        manifest.add_result(UpdateResult::skip(
            dep,
            crate::domain::SkipReason::AlreadyLatest,
        ));
        summary.add_manifest(manifest);

        let mut output = Vec::new();
        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("0 package(s) updated"));
        assert!(output_str.contains("1 skipped"));
    }

    #[test]
    fn test_format_manifest_no_updates() {
        // 更新なしのマニフェストは出力されない
        let formatter = DiffFormatter::new(false);
        let manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let mut output = Vec::new();
        formatter.format_manifest(&manifest, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.is_empty());
    }

    #[test]
    fn test_format_diff_multiple_updates() {
        // 複数パッケージの更新
        let formatter = DiffFormatter::new(false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let dep1 = sample_dependency("lodash", "4.17.21");
        manifest.add_result(UpdateResult::update(dep1, "4.18.0"));

        let dep2 = sample_dependency("axios", "1.6.0");
        manifest.add_result(UpdateResult::update(dep2, "1.7.0"));

        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("@@ lodash @@"));
        assert!(output_str.contains("@@ axios @@"));
        assert!(output_str.contains("# 2 package(s) would be updated"));
    }

    #[test]
    fn test_format_diff_multiple_manifests() {
        // 複数マニフェストファイルの更新
        let formatter = DiffFormatter::new(false);
        let mut summary = UpdateSummary::new(false);

        let mut manifest1 =
            ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);
        let dep1 = sample_dependency("lodash", "4.17.21");
        manifest1.add_result(UpdateResult::update(dep1, "4.18.0"));

        let mut manifest2 = ManifestUpdateResult::new(PathBuf::from("Cargo.toml"), Language::Rust);
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.0.0".to_string(), "1.0.0")
            .with_prefix("^");
        let dep2 = Dependency::new("serde", spec, false, Language::Rust);
        manifest2.add_result(UpdateResult::update(dep2, "1.1.0"));

        summary.add_manifest(manifest1);
        summary.add_manifest(manifest2);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("--- a/package.json"));
        assert!(output_str.contains("--- a/Cargo.toml"));
    }
}
