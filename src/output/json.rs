//! 機械処理用 JSON 出力フォーマッタ
//!
//! このモジュールが提供するもの:
//! - 更新結果の JSON シリアライズ
//! - ファイルごとの構造化された更新/スキップ情報

use crate::domain::{Language, ManifestUpdateResult, SkipReason, UpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use crate::output::{OutputFormatter, Verbosity};
use serde::Serialize;
use std::io::Write;

/// 機械可読出力用 JSON フォーマッタ
pub struct JsonFormatter {
    /// 詳細度レベルが出力の詳細さに影響
    verbosity: Verbosity,
}

impl JsonFormatter {
    /// 新しい JSON フォーマッタを作成
    pub fn new(verbosity: Verbosity) -> Self {
        Self { verbosity }
    }
}

/// 全結果の JSON 表現
#[derive(Serialize)]
struct JsonOutput {
    /// dry-run だったかどうか
    dry_run: bool,
    /// サマリ統計
    summary: JsonSummary,
    /// マニフェストごとの結果
    manifests: Vec<JsonManifest>,
    /// 発生したエラー
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

/// サマリ統計の JSON 表現
#[derive(Serialize)]
struct JsonSummary {
    /// 更新の総数
    updates: usize,
    /// スキップの総数
    skips: usize,
    /// 言語別の内訳
    #[serde(skip_serializing_if = "Vec::is_empty")]
    by_language: Vec<JsonLanguageSummary>,
}

/// 言語別サマリの JSON 表現
#[derive(Serialize)]
struct JsonLanguageSummary {
    /// 言語名
    language: String,
    /// 更新数
    updates: usize,
    /// スキップ数
    skips: usize,
}

/// マニフェスト結果の JSON 表現
#[derive(Serialize)]
struct JsonManifest {
    /// マニフェストファイルのパス
    path: String,
    /// マニフェストの言語
    language: String,
    /// 更新のリスト
    updates: Vec<JsonUpdate>,
    /// スキップのリスト (verbose モードのみ)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skips: Vec<JsonSkip>,
}

/// 更新の JSON 表現
#[derive(Serialize)]
struct JsonUpdate {
    /// パッケージ名
    name: String,
    /// 旧バージョン
    from: String,
    /// 新バージョン
    to: String,
    /// 開発依存かどうか
    dev: bool,
}

/// スキップの JSON 表現
#[derive(Serialize)]
struct JsonSkip {
    /// パッケージ名
    name: String,
    /// 現在のバージョン
    version: String,
    /// スキップ理由
    reason: String,
    /// 現在のバージョンがリリースされた日時 (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    released_at: Option<String>,
}

impl JsonFormatter {
    /// スキップ理由を文字列に変換
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

    /// マニフェスト結果を JSON 表現に変換
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
                    if let UpdateResult::Skip {
                        dependency,
                        reason,
                        released_at,
                    } = result
                    {
                        Some(JsonSkip {
                            name: dependency.name.clone(),
                            version: dependency.version_spec.version.clone(),
                            reason: Self::skip_reason_to_string(reason),
                            released_at: released_at.map(|d| d.to_rfc3339()),
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

        // 有効な JSON であることを検証
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

        // verbose モードではスキップが含まれるべき
        assert!(
            !parsed["manifests"][0]["skips"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(parsed["manifests"][0]["skips"][0]["name"], "express");
        assert_eq!(
            parsed["manifests"][0]["skips"][0]["reason"],
            "already_latest"
        );

        // 言語別内訳が含まれるべき
        assert!(
            !parsed["summary"]["by_language"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_format_json_quiet() {
        let formatter = JsonFormatter::new(Verbosity::Quiet);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();

        // quiet モードではスキップが含まれないべき (フィールドが省略または空)
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

    #[test]
    fn test_format_manifest_json() {
        // 単一マニフェストのJSON出力を確認
        let formatter = JsonFormatter::new(Verbosity::Normal);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("Cargo.toml"), Language::Rust);
        let dep = sample_dependency("serde", "1.0.0");
        manifest.add_result(UpdateResult::update(dep, "1.1.0"));

        let mut output = Vec::new();
        formatter.format_manifest(&manifest, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();
        assert_eq!(parsed["path"], "Cargo.toml");
        assert_eq!(parsed["language"], "Rust");
        assert_eq!(parsed["updates"][0]["name"], "serde");
        assert_eq!(parsed["updates"][0]["from"], "1.0.0");
        assert_eq!(parsed["updates"][0]["to"], "1.1.0");
    }

    #[test]
    fn test_format_json_dry_run() {
        // dry-run フラグの出力確認
        let formatter = JsonFormatter::new(Verbosity::Normal);
        let summary = UpdateSummary::new(true);
        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();
        assert_eq!(parsed["dry_run"], true);
    }

    #[test]
    fn test_format_json_with_errors() {
        // エラーが含まれる場合の出力確認
        use crate::orchestrator::OrchestratorError;
        let formatter = JsonFormatter::new(Verbosity::Normal);
        let summary = UpdateSummary::new(false);
        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: vec![OrchestratorError::RegistryError {
                package: "serde".into(),
                message: "fetch failed: timeout".into(),
            }],
        };
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();
        let errors = parsed["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].as_str().unwrap().contains("fetch failed"));
    }

    #[test]
    fn test_skip_reason_to_string_all_variants() {
        // 全SkipReason変換の確認
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::NotInOnlyList),
            "not_in_only_list"
        );
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::FetchFailed("timeout".into())),
            "fetch_failed: timeout"
        );
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::LanguageFiltered),
            "language_filtered"
        );
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::NoSuitableVersion),
            "no_suitable_version"
        );
        assert_eq!(
            JsonFormatter::skip_reason_to_string(&SkipReason::ParseError("invalid".into())),
            "parse_error: invalid"
        );
    }

    #[test]
    fn test_format_json_empty_result() {
        // 更新もスキップもない空の結果
        let formatter = JsonFormatter::new(Verbosity::Normal);
        let summary = UpdateSummary::new(false);
        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();
        assert_eq!(parsed["summary"]["updates"], 0);
        assert_eq!(parsed["summary"]["skips"], 0);
        assert!(parsed["manifests"].as_array().unwrap().is_empty());
        // エラーがない場合はerrorsフィールドが省略される
        assert!(parsed.get("errors").is_none() || parsed["errors"].as_array().unwrap().is_empty());
    }
}
