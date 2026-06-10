//! 更新フィルタ設定
//!
//! このモジュールは更新判定のためのフィルタオプションを
//! カプセル化する UpdateFilter 構造体を提供する。

use crate::domain::{ChangeLevel, Dependency, Language, SkipReason};
use std::time::Duration;

/// 更新判定用のフィルタ設定
#[derive(Debug, Clone, Default)]
pub struct UpdateFilter {
    /// 処理対象の言語 (空の場合は全言語)
    pub languages: Vec<Language>,
    /// 更新から除外するパッケージ
    pub exclude: Vec<String>,
    /// 空でない場合、これらのパッケージのみ更新
    pub only: Vec<String>,
    /// ピン留めバージョンも更新に含める
    pub include_pinned: bool,
    /// バージョンが考慮されるための最小経過日数
    pub min_age: Option<Duration>,
    /// 許容する変更レベルの上限 (例: `Patch` を指定すると patch のみ許可)
    pub max_change: Option<ChangeLevel>,
}

impl UpdateFilter {
    /// デフォルト設定 (全処理) で新しいUpdateFilterを作成する
    pub fn new() -> Self {
        Self::default()
    }

    /// 処理対象の言語を設定する
    pub fn with_languages(mut self, languages: Vec<Language>) -> Self {
        self.languages = languages;
        self
    }

    /// 除外するパッケージを設定する
    pub fn with_exclude(mut self, exclude: Vec<String>) -> Self {
        self.exclude = exclude;
        self
    }

    /// 対象パッケージ (onlyリスト) を設定する
    pub fn with_only(mut self, only: Vec<String>) -> Self {
        self.only = only;
        self
    }

    /// ピン留めバージョンを含めるかどうかを設定する
    pub fn with_include_pinned(mut self, include: bool) -> Self {
        self.include_pinned = include;
        self
    }

    /// バージョンの最小経過日数を設定する
    pub fn with_min_age(mut self, age: Duration) -> Self {
        self.min_age = Some(age);
        self
    }

    /// 許容する変更レベルの上限を設定する
    pub fn with_max_change(mut self, level: ChangeLevel) -> Self {
        self.max_change = Some(level);
        self
    }

    /// 言語を処理すべきかチェックする
    pub fn should_process_language(&self, language: Language) -> bool {
        if self.languages.is_empty() {
            return true; // フィルタなしは全言語処理を意味する
        }
        self.languages.contains(&language)
    }

    /// フィルタに基づいてパッケージを処理すべきかチェックする
    ///
    /// 名前のみで判定する簡易版 (後方互換 API)。Cargo のリネーム依存
    /// (manifest_name) も考慮する判定は `package_filter_skip_reason` を使うこと。
    pub fn should_process_package(&self, name: &str) -> bool {
        self.package_filter_decision(name, None).is_none()
    }

    /// 依存関係をパッケージフィルタ (only/exclude) で処理対象にすべきか判定する。
    /// スキップすべき場合は `Some(SkipReason)`、処理する場合は `None` を返す。
    ///
    /// Cargo のリネーム依存では実パッケージ名とマニフェスト上のキー名の両方を
    /// フィルタ名として受け付ける。`--only` は `--exclude` より優先される。
    pub fn package_filter_skip_reason(&self, dependency: &Dependency) -> Option<SkipReason> {
        let manifest_name = dependency.manifest_name();
        let manifest_name = (manifest_name != dependency.name).then_some(manifest_name);
        self.package_filter_decision(&dependency.name, manifest_name)
    }

    /// only/exclude 判定の共通実装。
    /// `manifest_name` は実パッケージ名と異なる場合のみ `Some` を渡す。
    fn package_filter_decision(
        &self,
        name: &str,
        manifest_name: Option<&str>,
    ) -> Option<SkipReason> {
        let matches_filter = |p: &String| p == name || manifest_name.is_some_and(|m| p == m);
        if !self.only.is_empty() {
            if !self.only.iter().any(matches_filter) {
                return Some(SkipReason::NotInOnlyList);
            }
            // --only が指定されている場合は --exclude より優先される
            return None;
        }
        if self.exclude.iter().any(matches_filter) {
            return Some(SkipReason::Excluded);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_filter() {
        let filter = UpdateFilter::new();
        assert!(filter.languages.is_empty());
        assert!(filter.exclude.is_empty());
        assert!(filter.only.is_empty());
        assert!(!filter.include_pinned);
        assert!(filter.min_age.is_none());
    }

    #[test]
    fn test_with_languages() {
        let filter = UpdateFilter::new().with_languages(vec![Language::Node, Language::Python]);
        assert_eq!(filter.languages.len(), 2);
        assert!(filter.languages.contains(&Language::Node));
        assert!(filter.languages.contains(&Language::Python));
    }

    #[test]
    fn test_with_exclude() {
        let filter = UpdateFilter::new().with_exclude(vec!["foo".to_string(), "bar".to_string()]);
        assert_eq!(filter.exclude, vec!["foo", "bar"]);
    }

    #[test]
    fn test_with_only() {
        let filter = UpdateFilter::new().with_only(vec!["foo".to_string()]);
        assert_eq!(filter.only, vec!["foo"]);
    }

    #[test]
    fn test_with_include_pinned() {
        let filter = UpdateFilter::new().with_include_pinned(true);
        assert!(filter.include_pinned);
    }

    #[test]
    fn test_with_min_age() {
        let filter = UpdateFilter::new().with_min_age(Duration::from_secs(86400));
        assert_eq!(filter.min_age, Some(Duration::from_secs(86400)));
    }

    #[test]
    fn test_should_process_language_no_filter() {
        let filter = UpdateFilter::new();
        assert!(filter.should_process_language(Language::Node));
        assert!(filter.should_process_language(Language::Python));
        assert!(filter.should_process_language(Language::Rust));
        assert!(filter.should_process_language(Language::Go));
    }

    #[test]
    fn test_should_process_language_with_filter() {
        let filter = UpdateFilter::new().with_languages(vec![Language::Node, Language::Python]);
        assert!(filter.should_process_language(Language::Node));
        assert!(filter.should_process_language(Language::Python));
        assert!(!filter.should_process_language(Language::Rust));
        assert!(!filter.should_process_language(Language::Go));
    }

    #[test]
    fn test_should_process_package_no_filter() {
        let filter = UpdateFilter::new();
        assert!(filter.should_process_package("any-package"));
        assert!(filter.should_process_package("another"));
    }

    #[test]
    fn test_should_process_package_with_exclude() {
        let filter = UpdateFilter::new().with_exclude(vec!["foo".to_string()]);
        assert!(!filter.should_process_package("foo"));
        assert!(filter.should_process_package("bar"));
    }

    #[test]
    fn test_should_process_package_with_only() {
        let filter = UpdateFilter::new().with_only(vec!["foo".to_string()]);
        assert!(filter.should_process_package("foo"));
        assert!(!filter.should_process_package("bar"));
    }

    #[test]
    fn test_should_process_package_only_takes_precedence() {
        // onlyとexcludeの両方が設定されている場合、onlyが優先される
        let filter = UpdateFilter::new()
            .with_only(vec!["foo".to_string()])
            .with_exclude(vec!["foo".to_string()]);
        // "foo"はonlyリストにあるため、excludeリストにあっても処理されるべき
        assert!(filter.should_process_package("foo"));
    }

    fn make_dep(name: &str) -> Dependency {
        use crate::domain::{VersionSpec, VersionSpecKind};
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", "1.0.0"), "1.0.0")
            .with_prefix("^");
        Dependency::new(name, spec, false, Language::Rust)
    }

    #[test]
    fn test_package_filter_skip_reason_no_filter() {
        let filter = UpdateFilter::new();
        assert_eq!(filter.package_filter_skip_reason(&make_dep("serde")), None);
    }

    #[test]
    fn test_package_filter_skip_reason_excluded() {
        let filter = UpdateFilter::new().with_exclude(vec!["serde".to_string()]);
        assert_eq!(
            filter.package_filter_skip_reason(&make_dep("serde")),
            Some(SkipReason::Excluded)
        );
        assert_eq!(filter.package_filter_skip_reason(&make_dep("tokio")), None);
    }

    #[test]
    fn test_package_filter_skip_reason_not_in_only() {
        let filter = UpdateFilter::new().with_only(vec!["serde".to_string()]);
        assert_eq!(filter.package_filter_skip_reason(&make_dep("serde")), None);
        assert_eq!(
            filter.package_filter_skip_reason(&make_dep("tokio")),
            Some(SkipReason::NotInOnlyList)
        );
    }

    #[test]
    fn test_package_filter_skip_reason_only_takes_precedence_over_exclude() {
        let filter = UpdateFilter::new()
            .with_only(vec!["serde".to_string()])
            .with_exclude(vec!["serde".to_string()]);
        assert_eq!(filter.package_filter_skip_reason(&make_dep("serde")), None);
    }

    /// 回帰テスト (judge との一本化): Cargo のリネーム依存では実パッケージ名と
    /// マニフェスト上のキー名のどちらでも only / exclude に一致する。
    /// 以前は filter 側の判定に manifest_name 一致が無く、judge 内の再実装と乖離していた。
    #[test]
    fn test_package_filter_skip_reason_matches_manifest_name() {
        let dep = make_dep("tokio").with_manifest_name("tokio_v1");

        // exclude はマニフェスト名でも一致する
        let filter = UpdateFilter::new().with_exclude(vec!["tokio_v1".to_string()]);
        assert_eq!(
            filter.package_filter_skip_reason(&dep),
            Some(SkipReason::Excluded)
        );

        // only はマニフェスト名でも一致する
        let filter = UpdateFilter::new().with_only(vec!["tokio_v1".to_string()]);
        assert_eq!(filter.package_filter_skip_reason(&dep), None);

        // 実パッケージ名でも従来どおり一致する
        let filter = UpdateFilter::new().with_only(vec!["tokio".to_string()]);
        assert_eq!(filter.package_filter_skip_reason(&dep), None);
    }

    #[test]
    fn test_should_process_package_consistent_with_skip_reason() {
        // 旧 API (名前のみ) と新 API の判定が一致する
        let filter = UpdateFilter::new().with_exclude(vec!["foo".to_string()]);
        assert_eq!(
            filter.should_process_package("foo"),
            filter
                .package_filter_skip_reason(&make_dep("foo"))
                .is_none()
        );
        assert_eq!(
            filter.should_process_package("bar"),
            filter
                .package_filter_skip_reason(&make_dep("bar"))
                .is_none()
        );
    }

    #[test]
    fn test_chained_builders() {
        let filter = UpdateFilter::new()
            .with_languages(vec![Language::Node])
            .with_exclude(vec!["lodash".to_string()])
            .with_include_pinned(true)
            .with_min_age(Duration::from_secs(86400));

        assert_eq!(filter.languages, vec![Language::Node]);
        assert_eq!(filter.exclude, vec!["lodash"]);
        assert!(filter.include_pinned);
        assert_eq!(filter.min_age, Some(Duration::from_secs(86400)));
    }
}
