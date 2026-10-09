//! 更新結果サマリの型定義
//!
//! ファイル単位および全体レベルの更新結果を追跡する構造体を提供する。

use super::{Language, UpdateResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 単一マニフェストファイルの更新結果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestUpdateResult {
    /// マニフェストファイルのパス
    pub path: PathBuf,
    /// このマニフェストの言語
    pub language: Language,
    /// 個別の依存関係更新結果
    pub results: Vec<UpdateResult>,
    /// ファイルが実際に変更されたかどうか
    pub modified: bool,
}

impl ManifestUpdateResult {
    /// 新しいManifestUpdateResultを作成する
    pub fn new(path: impl Into<PathBuf>, language: Language) -> Self {
        Self {
            path: path.into(),
            language,
            results: Vec::new(),
            modified: false,
        }
    }

    /// 更新結果を追加する
    pub fn add_result(&mut self, result: UpdateResult) {
        if result.is_update() {
            self.modified = true;
        }
        self.results.push(result);
    }

    /// judge が更新すると決めた結果を、書き込みの成否を問わずに返す。
    ///
    /// 書き込みの段階 (と、それより前の段階) はこちらを使う。書き込んだ後の
    /// 件数・出力・install は、失敗を除いた `updates` を使う
    pub fn judged_updates(&self) -> impl Iterator<Item = &UpdateResult> {
        self.results.iter().filter(|r| r.is_update())
    }

    /// judge が更新すると決めた結果があるかどうかを返す
    pub fn has_judged_updates(&self) -> bool {
        self.judged_updates().next().is_some()
    }

    /// 更新件数を返す (書き込めなかった更新は数えない)
    pub fn update_count(&self) -> usize {
        self.updates().count()
    }

    /// 書き込めなかった更新の件数を返す
    pub fn failed_count(&self) -> usize {
        self.failed_updates().count()
    }

    /// スキップ件数を返す
    pub fn skip_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_skip()).count()
    }

    /// 更新を返す (書き込めなかった更新は含めない)。
    ///
    /// Cargo.toml を書き換えない更新 (git の branch 依存のように Cargo.lock だけが変わるもの) と、
    /// 書き換えても内容が同じだった更新は、書き込みに失敗していないので含める
    pub fn updates(&self) -> impl Iterator<Item = &UpdateResult> {
        self.results
            .iter()
            .filter(|r| r.is_update() && r.write_error().is_none())
    }

    /// 書き込めなかった更新を返す
    pub fn failed_updates(&self) -> impl Iterator<Item = &UpdateResult> {
        self.results.iter().filter(|r| r.write_error().is_some())
    }

    /// 全スキップを返す
    pub fn skips(&self) -> impl Iterator<Item = &UpdateResult> {
        self.results.iter().filter(|r| r.is_skip())
    }

    /// 依存関係が更新されたかどうかを返す (書き込めなかった更新は数えない)
    pub fn has_updates(&self) -> bool {
        self.update_count() > 0
    }

    /// `results` の `index` 番目の更新を、書き込めなかったものとして記録する
    pub fn record_write_error(&mut self, index: usize, reason: impl Into<String>) {
        if let Some(result) = self.results.get_mut(index) {
            result.record_write_error(reason);
        }
    }
}

/// 言語ごとの件数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageCounts {
    /// 言語
    pub language: Language,
    /// 更新の数 (書き込めなかった更新は数えない)
    pub updates: usize,
    /// 書き込めなかった更新の数
    pub failed: usize,
    /// スキップの数
    pub skips: usize,
}

/// 全更新操作の総合サマリ
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSummary {
    /// 処理された各マニフェストファイルの結果
    pub manifests: Vec<ManifestUpdateResult>,
    /// ドライランかどうか
    pub dry_run: bool,
}

impl UpdateSummary {
    /// 新しいUpdateSummaryを作成する
    pub fn new(dry_run: bool) -> Self {
        Self {
            manifests: Vec::new(),
            dry_run,
        }
    }

    /// マニフェスト結果を追加する
    pub fn add_manifest(&mut self, manifest: ManifestUpdateResult) {
        self.manifests.push(manifest);
    }

    /// 処理されたファイルの合計数を返す
    pub fn files_processed(&self) -> usize {
        self.manifests.len()
    }

    /// 変更されたファイルの合計数を返す
    pub fn files_modified(&self) -> usize {
        self.manifests.iter().filter(|m| m.modified).count()
    }

    /// 更新された依存関係の合計数を返す (書き込めなかった更新は数えない)
    pub fn total_updates(&self) -> usize {
        self.manifests.iter().map(|m| m.update_count()).sum()
    }

    /// 書き込めなかった更新の合計数を返す
    pub fn total_failed(&self) -> usize {
        self.manifests.iter().map(|m| m.failed_count()).sum()
    }

    /// スキップされた依存関係の合計数を返す
    pub fn total_skips(&self) -> usize {
        self.manifests.iter().map(|m| m.skip_count()).sum()
    }

    /// 処理された依存関係の合計数を返す
    pub fn total_dependencies(&self) -> usize {
        self.manifests.iter().map(|m| m.results.len()).sum()
    }

    /// ファイルが変更されたかどうかを返す
    pub fn has_changes(&self) -> bool {
        self.files_modified() > 0
    }

    /// 特定言語のマニフェストを返す
    pub fn by_language(&self, language: Language) -> impl Iterator<Item = &ManifestUpdateResult> {
        self.manifests
            .iter()
            .filter(move |m| m.language == language)
    }

    /// 非空の言語ごとの件数を `Language::all()` 順で返す
    pub fn language_breakdown(&self) -> Vec<LanguageCounts> {
        Language::all()
            .iter()
            .filter_map(|language| {
                let manifests: Vec<_> = self.by_language(*language).collect();
                if manifests.is_empty() {
                    None
                } else {
                    Some(LanguageCounts {
                        language: *language,
                        updates: manifests.iter().map(|m| m.update_count()).sum(),
                        failed: manifests.iter().map(|m| m.failed_count()).sum(),
                        skips: manifests.iter().map(|m| m.skip_count()).sum(),
                    })
                }
            })
            .collect()
    }

    /// `path` のマニフェストの `results` の `index` 番目の更新を、書き込めなかったものとして
    /// 記録する。マニフェストのパスは検出の段階で重複を除いてあるので、パスで 1 件に決まる
    pub fn record_write_error(&mut self, path: &Path, index: usize, reason: impl Into<String>) {
        if let Some(manifest) = self.manifests.iter_mut().find(|m| m.path == path) {
            manifest.record_write_error(index, reason);
        }
    }

    /// 全マニフェストの全更新を返す (書き込めなかった更新は含めない)
    pub fn all_updates(&self) -> impl Iterator<Item = &UpdateResult> {
        self.manifests.iter().flat_map(|m| m.updates())
    }

    /// 全マニフェストの全スキップを返す
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

    #[test]
    fn test_write_errors_are_counted_apart_from_updates() {
        // 回帰 (#21): 書き込めなかった更新は更新として数えず、別に数える。
        // judge が決めた更新 (judged_updates) には残る
        let mut summary = UpdateSummary::new(false);
        let mut node = ManifestUpdateResult::new("/package.json", Language::Node);
        node.add_result(sample_update("lodash"));
        node.add_result(sample_update("express"));
        node.add_result(sample_skip("react"));
        summary.add_manifest(node);
        let mut rust = ManifestUpdateResult::new("/Cargo.toml", Language::Rust);
        rust.add_result(sample_update("serde"));
        summary.add_manifest(rust);

        summary.record_write_error(Path::new("/package.json"), 1, "Failed to update express");
        summary.record_write_error(Path::new("/Cargo.toml"), 0, "Refusing to update serde");
        // Skip とマニフェストの外の位置には何もしない
        summary.record_write_error(Path::new("/package.json"), 2, "ignored");
        summary.record_write_error(Path::new("/package.json"), 9, "ignored");

        let node = &summary.manifests[0];
        assert_eq!(node.judged_updates().count(), 2);
        assert_eq!(node.update_count(), 1);
        assert_eq!(node.updates().next().unwrap().package_name(), "lodash");
        assert_eq!(node.failed_count(), 1);
        assert_eq!(
            node.failed_updates().next().unwrap().write_error(),
            Some("Failed to update express")
        );
        assert_eq!(node.skip_count(), 1);
        assert!(node.has_updates());

        // 全更新を書き込めなかったマニフェストは、更新が無いものとして扱う
        let rust = &summary.manifests[1];
        assert!(rust.has_judged_updates());
        assert!(!rust.has_updates());
        assert_eq!(rust.failed_count(), 1);

        assert_eq!(summary.total_updates(), 1);
        assert_eq!(summary.total_failed(), 2);
        assert_eq!(summary.all_updates().count(), 1);
        assert_eq!(
            summary.language_breakdown(),
            vec![
                LanguageCounts {
                    language: Language::Node,
                    updates: 1,
                    failed: 1,
                    skips: 1,
                },
                LanguageCounts {
                    language: Language::Rust,
                    updates: 0,
                    failed: 1,
                    skips: 0,
                },
            ]
        );
    }
}
