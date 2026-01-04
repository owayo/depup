//! Update judgment logic for dependencies
//!
//! This module provides:
//! - Update filter configuration from CLI args
//! - Version info from registry with release date
//! - Update judgment engine that decides whether to update or skip

mod filter;
mod version_info;

pub use filter::UpdateFilter;
pub use version_info::{compare_versions, is_prerelease_version, VersionInfo};

use crate::domain::{Dependency, SkipReason, UpdateResult};
use chrono::{DateTime, Utc};

/// Update judgment engine that decides whether to update a dependency
pub struct UpdateJudge {
    /// Filter configuration
    filter: UpdateFilter,
    /// Current time for age calculations
    now: DateTime<Utc>,
}

impl UpdateJudge {
    /// Create a new UpdateJudge with the given filter
    pub fn new(filter: UpdateFilter) -> Self {
        Self {
            filter,
            now: Utc::now(),
        }
    }

    /// Create a new UpdateJudge with a custom current time (for testing)
    pub fn with_time(filter: UpdateFilter, now: DateTime<Utc>) -> Self {
        Self { filter, now }
    }

    /// Check if a dependency should be processed at all
    /// Returns Some(SkipReason) if it should be skipped, None if it should be processed
    pub fn should_skip(&self, dependency: &Dependency) -> Option<SkipReason> {
        // Check language filter
        if !self.filter.should_process_language(dependency.language) {
            return Some(SkipReason::LanguageFiltered);
        }

        // Check package filters (exclude/only)
        if !self.filter.should_process_package(&dependency.name) {
            if !self.filter.only.is_empty() {
                return Some(SkipReason::NotInOnlyList);
            } else {
                return Some(SkipReason::Excluded);
            }
        }

        // Check pinned version (unless --include-pinned)
        if dependency.is_pinned() && !self.filter.include_pinned {
            return Some(SkipReason::Pinned);
        }

        None
    }

    /// Judge whether to update a dependency given available versions
    pub fn judge(
        &self,
        dependency: &Dependency,
        available_versions: &[VersionInfo],
    ) -> UpdateResult {
        // First check if we should skip this dependency
        if let Some(reason) = self.should_skip(dependency) {
            return UpdateResult::skip(dependency.clone(), reason);
        }

        // If no versions available, skip
        if available_versions.is_empty() {
            return UpdateResult::skip(
                dependency.clone(),
                SkipReason::FetchFailed("no versions available".to_string()),
            );
        }

        // Filter out pre-release versions (alpha, beta, canary, dev, etc.) by default
        // Only consider stable releases unless the current version is already a prerelease
        let current_is_prerelease = is_prerelease_version(dependency.version());
        let stable_versions: Vec<&VersionInfo> = if current_is_prerelease {
            // If current version is prerelease, allow prerelease updates
            available_versions.iter().collect()
        } else {
            // Otherwise, only consider stable versions
            available_versions
                .iter()
                .filter(|v| !v.is_prerelease())
                .collect()
        };

        // Filter versions by age if specified
        let eligible_versions: Vec<&VersionInfo> = if let Some(min_age) = self.filter.min_age {
            let min_release_time = self.now - chrono::Duration::from_std(min_age).unwrap();
            stable_versions
                .into_iter()
                .filter(|v| v.released_at <= min_release_time)
                .collect()
        } else {
            stable_versions
        };

        if eligible_versions.is_empty() {
            return UpdateResult::skip(dependency.clone(), SkipReason::NoSuitableVersion);
        }

        // Find the latest eligible version (uses VersionInfo's Ord which does proper semver comparison)
        let latest = eligible_versions.iter().max().unwrap();

        // Check if already at latest or current version is newer (prevents downgrades)
        // compare_versions returns Less if current < latest, so we only update in that case
        if version_info::compare_versions(dependency.version(), &latest.version)
            != std::cmp::Ordering::Less
        {
            return UpdateResult::skip_already_latest(dependency.clone());
        }

        // Return update result with release date
        UpdateResult::update_with_date(dependency.clone(), &latest.version, latest.released_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Language, VersionSpec, VersionSpecKind};
    use chrono::TimeZone;
    use std::time::Duration;

    fn make_dependency(name: &str, version: &str, language: Language, pinned: bool) -> Dependency {
        let kind = if pinned {
            VersionSpecKind::Exact
        } else {
            VersionSpecKind::Caret
        };
        let spec = if pinned {
            VersionSpec::new(kind, version, version)
        } else {
            VersionSpec::new(kind, &format!("^{}", version), version).with_prefix("^")
        };
        Dependency::new(name, spec, false, language)
    }

    fn make_version_info(version: &str, days_ago: i64) -> VersionInfo {
        let released_at = Utc::now() - chrono::Duration::days(days_ago);
        VersionInfo::new(version, released_at)
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap()
    }

    fn make_version_info_at(version: &str, date: DateTime<Utc>) -> VersionInfo {
        VersionInfo::new(version, date)
    }

    #[test]
    fn test_judge_simple_update() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("1.1.0", 50),
            make_version_info("2.0.0", 10),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_judge_already_latest() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "2.0.0", Language::Node, false);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("2.0.0", 10),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_skip_pinned() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, true);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        }
    }

    #[test]
    fn test_judge_include_pinned() {
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, true);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
    }

    #[test]
    fn test_judge_exclude_package() {
        let filter = UpdateFilter::new().with_exclude(vec!["lodash".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Excluded);
        }
    }

    #[test]
    fn test_judge_only_list() {
        let filter = UpdateFilter::new().with_only(vec!["express".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::NotInOnlyList);
        }
    }

    #[test]
    fn test_judge_only_list_match() {
        let filter = UpdateFilter::new().with_only(vec!["lodash".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
    }

    #[test]
    fn test_judge_language_filter() {
        let filter = UpdateFilter::new().with_languages(vec![Language::Python]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::LanguageFiltered);
        }
    }

    #[test]
    fn test_judge_language_filter_match() {
        let filter = UpdateFilter::new().with_languages(vec![Language::Node]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
    }

    #[test]
    fn test_judge_age_filter() {
        let now = fixed_time();
        let filter = UpdateFilter::new().with_min_age(Duration::from_secs(7 * 24 * 60 * 60)); // 7 days
        let judge = UpdateJudge::with_time(filter, now);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);

        // Version released 3 days ago (too recent)
        let recent = make_version_info_at("2.0.0", now - chrono::Duration::days(3));
        // Version released 10 days ago (eligible)
        let old = make_version_info_at("1.5.0", now - chrono::Duration::days(10));

        let versions = vec![old, recent];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // Should update to 1.5.0 because 2.0.0 is too recent
            assert_eq!(new_version, "1.5.0");
        }
    }

    #[test]
    fn test_judge_age_filter_no_suitable() {
        let now = fixed_time();
        let filter = UpdateFilter::new().with_min_age(Duration::from_secs(30 * 24 * 60 * 60)); // 30 days
        let judge = UpdateJudge::with_time(filter, now);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);

        // All versions too recent
        let versions = vec![
            make_version_info_at("2.0.0", now - chrono::Duration::days(3)),
            make_version_info_at("1.5.0", now - chrono::Duration::days(10)),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::NoSuitableVersion);
        }
    }

    #[test]
    fn test_judge_no_versions() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions: Vec<VersionInfo> = vec![];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert!(matches!(reason, SkipReason::FetchFailed(_)));
        }
    }

    #[test]
    fn test_should_skip_returns_none_for_normal() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_should_skip_returns_reason_for_pinned() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, true);
        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Pinned));
    }

    #[test]
    fn test_judge_prevents_downgrade() {
        // Regression test: ensure 0.13 is not "downgraded" to 0.9.1
        // This was a bug where string comparison was used instead of semver
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("mockall", "0.13.0", Language::Rust, false);
        let versions = vec![
            make_version_info("0.9.1", 100),
            make_version_info("0.10.0", 80),
            make_version_info("0.11.0", 60),
            make_version_info("0.12.0", 40),
            make_version_info("0.13.0", 20), // current version
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip because already at latest (0.13.0 >= 0.13.0)
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_multi_digit_version_comparison() {
        // Ensure 1.10.0 > 1.9.0 (not string comparison where "1.9.0" > "1.10.0")
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("serde", "1.9.0", Language::Rust, false);
        let versions = vec![
            make_version_info("1.8.0", 100),
            make_version_info("1.9.0", 80),
            make_version_info("1.10.0", 60),
            make_version_info("1.11.0", 40),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // Should update to 1.11.0, not stay at 1.9.0 or downgrade
            assert_eq!(new_version, "1.11.0");
        }
    }

    #[test]
    fn test_judge_no_downgrade_when_current_is_newer() {
        // If current version is newer than all available, skip (don't downgrade)
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("cocoa", "0.26.0", Language::Rust, false);
        let versions = vec![
            make_version_info("0.9.2", 200),
            make_version_info("0.20.0", 100),
            make_version_info("0.25.0", 50),
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip - current 0.26.0 > latest available 0.25.0
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_filters_prerelease_versions() {
        // Regression test: stable versions should not update to prerelease
        // e.g., react 19.2.1 should NOT update to 19.3.0-canary-xxx
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("react", "19.2.1", Language::Node, false);
        let versions = vec![
            make_version_info("19.2.0", 30),
            make_version_info("19.2.1", 20),
            make_version_info("19.3.0-canary-52684925-20251110", 5), // prerelease - should be ignored
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip - already at latest STABLE version
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_filters_various_prerelease_types() {
        // Test that all prerelease types are filtered: alpha, beta, rc, dev, canary
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("typescript", "5.9.0", Language::Node, false);
        let versions = vec![
            make_version_info("5.8.0", 100),
            make_version_info("5.9.0", 50),
            make_version_info("6.0.0-dev.20260103", 10), // dev - should be ignored
            make_version_info("6.0.0-beta.1", 8),        // beta - should be ignored
            make_version_info("6.0.0-alpha.5", 6),       // alpha - should be ignored
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip - already at latest STABLE version (5.9.0)
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_updates_to_stable_not_prerelease() {
        // When both stable and prerelease are newer, should update to stable
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("vite", "7.0.0", Language::Node, false);
        let versions = vec![
            make_version_info("7.0.0", 50),
            make_version_info("7.1.0", 20), // stable - should be selected
            make_version_info("8.0.0-beta.5", 10), // prerelease - should be ignored
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // Should update to 7.1.0, not 8.0.0-beta.5
            assert_eq!(new_version, "7.1.0");
        }
    }

    #[test]
    fn test_judge_prerelease_current_allows_prerelease_update() {
        // If current version is prerelease, allow updating to newer prerelease
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // User is on a canary version, so they probably want canary updates
        let spec = VersionSpec::new(
            VersionSpecKind::Caret,
            "^19.3.0-canary-123",
            "19.3.0-canary-123",
        );
        let dep = Dependency::new("react", spec, false, Language::Node);

        let versions = vec![
            make_version_info("19.2.1", 30),
            make_version_info("19.3.0-canary-123", 20),
            make_version_info("19.3.0-canary-456", 10), // newer canary
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // Should update to newer canary
            assert_eq!(new_version, "19.3.0-canary-456");
        }
    }

    #[test]
    fn test_judge_no_suitable_stable_version() {
        // If all newer versions are prerelease, and current is stable, no suitable version
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("prettier", "3.7.0", Language::Node, false);
        let versions = vec![
            make_version_info("3.6.0", 50),
            make_version_info("3.7.0", 30),
            make_version_info("4.0.0-alpha.13", 10), // only newer version is alpha
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip - already at latest STABLE version
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }
}
