//! Update filter configuration
//!
//! This module provides the UpdateFilter struct that encapsulates
//! all filter options for update judgment.

use crate::domain::Language;
use std::time::Duration;

/// Filter configuration for update judgment
#[derive(Debug, Clone, Default)]
pub struct UpdateFilter {
    /// Languages to process (empty means all)
    pub languages: Vec<Language>,
    /// Packages to exclude from updates
    pub exclude: Vec<String>,
    /// If non-empty, only update these packages
    pub only: Vec<String>,
    /// Include pinned versions in updates
    pub include_pinned: bool,
    /// Minimum age for versions to be considered
    pub min_age: Option<Duration>,
}

impl UpdateFilter {
    /// Create a new UpdateFilter with default settings (process all)
    pub fn new() -> Self {
        Self::default()
    }

    /// Set languages to process
    pub fn with_languages(mut self, languages: Vec<Language>) -> Self {
        self.languages = languages;
        self
    }

    /// Set packages to exclude
    pub fn with_exclude(mut self, exclude: Vec<String>) -> Self {
        self.exclude = exclude;
        self
    }

    /// Set packages to include (only list)
    pub fn with_only(mut self, only: Vec<String>) -> Self {
        self.only = only;
        self
    }

    /// Set whether to include pinned versions
    pub fn with_include_pinned(mut self, include: bool) -> Self {
        self.include_pinned = include;
        self
    }

    /// Set minimum age for versions
    pub fn with_min_age(mut self, age: Duration) -> Self {
        self.min_age = Some(age);
        self
    }

    /// Check if a language should be processed
    pub fn should_process_language(&self, language: Language) -> bool {
        if self.languages.is_empty() {
            return true; // No filter means process all
        }
        self.languages.contains(&language)
    }

    /// Check if a package should be processed based on filters
    pub fn should_process_package(&self, name: &str) -> bool {
        // If --only is specified, only process those packages
        if !self.only.is_empty() {
            return self.only.iter().any(|p| p == name);
        }
        // If --exclude is specified, skip those packages
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
        // When both only and exclude are set, only takes precedence
        let filter = UpdateFilter::new()
            .with_only(vec!["foo".to_string()])
            .with_exclude(vec!["foo".to_string()]);
        // "foo" is in only list, so it should be processed despite being in exclude
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
