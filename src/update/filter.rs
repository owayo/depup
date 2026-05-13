//! 更新フィルタ設定
//!
//! このモジュールは更新判定のためのフィルタオプションを
//! カプセル化する UpdateFilter 構造体を提供する。

use crate::domain::{ChangeLevel, Language};
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
    pub fn should_process_package(&self, name: &str) -> bool {
        // --only が指定されている場合、それらのパッケージのみ処理
        if !self.only.is_empty() {
            return self.only.iter().any(|p| p == name);
        }
        // --exclude が指定されている場合、それらのパッケージをスキップ
        if self.exclude.iter().any(|p| p == name) {
            return false;
        }
        true
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
