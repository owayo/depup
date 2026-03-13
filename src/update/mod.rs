//! Update judgment logic for dependencies
//!
//! This module provides:
//! - Update filter configuration from CLI args
//! - Version info from registry with release date
//! - Update judgment engine that decides whether to update or skip

mod filter;
mod version_info;

pub use filter::UpdateFilter;
pub use version_info::{VersionInfo, compare_versions, is_prerelease_version};

use crate::domain::{Dependency, SkipReason, UpdateResult, VersionSpecKind};
use chrono::{DateTime, Utc};
use regex::Regex;
use std::sync::LazyLock;

/// Common version token used in range upper-bound extraction.
const VERSION_TOKEN: &str = r"[vV]?\d+(?:\.\d+)*(?:[-+][0-9A-Za-z.-]+)?";
/// Mavenレンジ専用のバージョントークン。`2.0.Final` のような qualifier を許容する。
const MAVEN_VERSION_TOKEN: &str = r"[vV]?\d+(?:\.[0-9A-Za-z]+)*(?:[-+][0-9A-Za-z.-]+)?";

/// Regex to extract inclusive upper bound (`<=X`) from Range constraints.
static UPPER_BOUND_LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"<=\s*({VERSION_TOKEN})")).unwrap());
/// Regex to extract exclusive upper bound (`<X`) from Range constraints.
static UPPER_BOUND_LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"<\s*({VERSION_TOKEN})")).unwrap());
/// Regex to extract Swift closed-range upper bound (`A...B`) from Range constraints.
static UPPER_BOUND_SWIFT_CLOSED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\.\.\.\s*({VERSION_TOKEN})")).unwrap());
/// Regex to extract Swift half-open upper bound (`A..<B`) from Range constraints.
static UPPER_BOUND_SWIFT_HALF_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\.\.<\s*({VERSION_TOKEN})")).unwrap());
/// Regex to extract hyphen range upper bound (`A - B`) from Range constraints.
static UPPER_BOUND_HYPHEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"{VERSION_TOKEN}\s*-\s*({VERSION_TOKEN})")).unwrap());
/// Regex to extract Maven-style ranges (`[1.0,2.0)`, `(,2.0]`) from Range constraints.
static MAVEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^([\[\(\]])\s*({MAVEN_VERSION_TOKEN})?\s*,\s*({MAVEN_VERSION_TOKEN})?\s*([\]\)\[])$"
    ))
    .unwrap()
});

fn normalize_bound_version(version: &str) -> String {
    version
        .strip_prefix('v')
        .or_else(|| version.strip_prefix('V'))
        .unwrap_or(version)
        .to_string()
}

/// Extract upper bound version and inclusiveness from a Range constraint string.
///
/// Returns `(upper_bound, inclusive)` where:
/// - `<X` and `A..<B` return `(X, false)`
/// - `<=X` and `A...B` return `(X, true)`
fn extract_upper_bound(raw: &str) -> Option<(String, bool)> {
    let trimmed = raw.trim();

    if let Some(caps) = UPPER_BOUND_SWIFT_HALF_OPEN_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), false));
    }

    if let Some(caps) = UPPER_BOUND_LTE_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), true));
    }

    if let Some(caps) = UPPER_BOUND_LT_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), false));
    }

    if let Some(caps) = UPPER_BOUND_SWIFT_CLOSED_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), true));
    }

    if let Some(caps) = UPPER_BOUND_HYPHEN_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), true));
    }

    if let Some(caps) = MAVEN_RANGE_RE.captures(trimmed) {
        let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("").trim();
        if !upper.is_empty() {
            let inclusive = caps.get(4).map(|m| m.as_str()) == Some("]");
            return Some((normalize_bound_version(upper), inclusive));
        }
    }

    None
}

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

        // GoPinned (// pinned コメント付き) は always_pinned に関係なくスキップ
        if dependency.version_spec.kind == VersionSpecKind::GoPinned && !self.filter.include_pinned
        {
            return Some(SkipReason::Pinned);
        }

        // Check pinned version (unless --include-pinned or language always uses pinned versions)
        // Languages like Go and Java don't have range specifiers, so all versions are pinned.
        // For these languages, we should always include them even without --include-pinned.
        if dependency.is_pinned()
            && !self.filter.include_pinned
            && !dependency.language.always_pinned()
        {
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
        let age_filtered: Vec<&VersionInfo> = if let Some(min_age) = self.filter.min_age {
            let min_release_time = self.now - chrono::Duration::from_std(min_age).unwrap();
            stable_versions
                .into_iter()
                .filter(|v| v.released_at <= min_release_time)
                .collect()
        } else {
            stable_versions
        };

        // Filter versions by Range constraint upper bound if applicable
        // e.g., for ">=3.5.0,<4.0.0", exclude versions >= 4.0.0
        let eligible_versions: Vec<&VersionInfo> =
            if dependency.version_spec.kind == VersionSpecKind::Range {
                if let Some((upper_bound, inclusive)) =
                    extract_upper_bound(&dependency.version_spec.raw)
                {
                    age_filtered
                        .into_iter()
                        .filter(|v| {
                            match version_info::compare_versions(&v.version, &upper_bound) {
                                std::cmp::Ordering::Less => true,
                                std::cmp::Ordering::Equal => inclusive,
                                std::cmp::Ordering::Greater => false,
                            }
                        })
                        .collect()
                } else {
                    age_filtered
                }
            } else {
                age_filtered
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
            return UpdateResult::skip_already_latest_with_date(
                dependency.clone(),
                latest.released_at,
            );
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
            VersionSpec::new(kind, format!("^{}", version), version).with_prefix("^")
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
        if let UpdateResult::Skip {
            reason,
            released_at,
            ..
        } = result
        {
            assert_eq!(reason, SkipReason::AlreadyLatest);
            // released_at should be set from the latest version info
            assert!(released_at.is_some());
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
    fn test_should_skip_go_always_pinned_language() {
        // Go only supports exact versions, so pinned deps should NOT be skipped
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // Go dependency is always pinned (VersionSpecKind::Exact)
        let dep = make_dependency("github.com/gin-gonic/gin", "1.9.0", Language::Go, true);

        // Should NOT skip - Go is an always_pinned language
        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_should_skip_java_pinned_dependency() {
        // Java/Gradle supports version ranges (Maven-style, prefix versions, etc.)
        // so pinned dependencies SHOULD be skipped without --include-pinned
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // Java dependency with exact version (pinned)
        let dep = make_dependency(
            "org.springframework:spring-core",
            "6.0.0",
            Language::Java,
            true,
        );

        // Should skip - Java is NOT an always_pinned language
        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Pinned));
    }

    #[test]
    fn test_judge_go_pinned_without_include_pinned_flag() {
        // Go dependencies should be updated even without --include-pinned
        let filter = UpdateFilter::new(); // include_pinned = false
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("github.com/gin-gonic/gin", "1.9.0", Language::Go, true);
        let versions = vec![make_version_info("1.10.0", 10)];

        let result = judge.judge(&dep, &versions);
        // Should update because Go is always_pinned language
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.10.0");
        }
    }

    #[test]
    fn test_judge_java_pinned_without_include_pinned_flag() {
        // Java/Gradle supports version ranges, so pinned deps should be skipped
        // without --include-pinned flag
        let filter = UpdateFilter::new(); // include_pinned = false
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency(
            "org.springframework:spring-core",
            "6.0.0",
            Language::Java,
            true,
        );
        let versions = vec![make_version_info("6.1.0", 10)];

        let result = judge.judge(&dep, &versions);
        // Should skip because Java is NOT always_pinned and include_pinned = false
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        }
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

    #[test]
    fn test_extract_upper_bound() {
        // Test the helper function for extracting upper bound from Range constraint
        assert_eq!(
            super::extract_upper_bound(">=3.5.0,<4.0.0"),
            Some(("4.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0,<2.0"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0, <2.0"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0,<=2.0"),
            Some(("2.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("4.0.0...4.9.9"),
            Some(("4.9.9".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("4.0.0..<5.0.0"),
            Some(("5.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("1.2.0 - 2.0.0"),
            Some(("2.0.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0)"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("(,2.0]"),
            Some(("2.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("]1.0,2.0["),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0["),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0.0,<v2.0.0"),
            Some(("2.0.0".to_string(), false))
        );
        // No upper bound
        assert_eq!(super::extract_upper_bound(">=1.0"), None);
        // Only lower bound with >
        assert_eq!(super::extract_upper_bound(">1.0"), None);
    }

    #[test]
    fn test_extract_upper_bound_whitespace_handling() {
        // Regression test: leading/trailing whitespace should be trimmed (bug fix)
        assert_eq!(
            super::extract_upper_bound("  >=1.0,<2.0  "),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("  >=1.0,<=2.0  "),
            Some(("2.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound(" 4.0.0..<5.0.0 "),
            Some(("5.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(" 4.0.0...4.9.9 "),
            Some(("4.9.9".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("  1.2.0 - 2.0.0  "),
            Some(("2.0.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("  [1.0,2.0)  "),
            Some(("2.0".to_string(), false))
        );
    }

    #[test]
    fn test_extract_upper_bound_maven_inclusive_bracket() {
        // Maven-style inclusive bracket: [1.0,2.0]
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0]"),
            Some(("2.0".to_string(), true))
        );
        // Maven-style exclusive paren: (1.0,2.0)
        assert_eq!(
            super::extract_upper_bound("(1.0,2.0)"),
            Some(("2.0".to_string(), false))
        );
        // Maven lower-unbounded: (,2.0)
        assert_eq!(
            super::extract_upper_bound("(,2.0)"),
            Some(("2.0".to_string(), false))
        );
        // Maven single version: [1.0]
        assert_eq!(super::extract_upper_bound("[1.0]"), None);
        // Maven qualifier付き上限: [1.0,2.0.Final)
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0.Final)"),
            Some(("2.0.Final".to_string(), false))
        );
    }

    #[test]
    fn test_extract_upper_bound_v_prefix_normalization() {
        // V prefix should be stripped from the returned upper bound
        assert_eq!(
            super::extract_upper_bound(">=v1.0.0,<V2.0.0"),
            Some(("2.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("v1.0.0...v2.0.0"),
            Some(("2.0.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("v1.0.0..<v3.0.0"),
            Some(("3.0.0".to_string(), false))
        );
    }

    #[test]
    fn test_judge_hyphen_range_respects_upper_bound() {
        // npm hyphen range: 1.0.0 - 2.0.0 means >=1.0.0 <=2.0.0 (inclusive)
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("lodash", "1.0.0 - 2.0.0", "1.0.0", Language::Node);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("1.5.0", 50),
            make_version_info("2.0.0", 30), // included (hyphen range is inclusive)
            make_version_info("2.1.0", 10), // excluded
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_judge_maven_range_exclusive() {
        // Maven range: [1.0,2.0) means >=1.0 and <2.0
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "[1.0,2.0)", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 10), // excluded by exclusive upper bound
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_maven_range_exclusive_alt_brackets() {
        // Maven alternate bracket notation: ]1.0,2.0[ means >1.0 and <2.0
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "]1.0,2.0[", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 10), // excluded by exclusive upper bound
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_maven_range_with_qualifier_upper_bound() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep =
            make_range_dependency("org.example:lib", "[1.0,2.0.Final)", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.1", 10), // 上限より大きいので除外されるべき
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_maven_range_inclusive() {
        // Maven range: [1.0,2.0] means >=1.0 and <=2.0
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "[1.0,2.0]", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 30), // included by inclusive upper bound
            make_version_info("2.1", 10), // excluded
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0");
        }
    }

    #[test]
    fn test_judge_swift_half_open_range_respects_upper_bound() {
        // Swift half-open range: 4.0.0..<5.0.0 means >=4.0.0 and <5.0.0
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("vapor/vapor", "4.0.0..<5.0.0", "4.0.0", Language::Swift);
        let versions = vec![
            make_version_info("4.0.0", 100),
            make_version_info("4.99.0", 50),
            make_version_info("5.0.0", 10), // excluded by half-open range
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "4.99.0");
        }
    }

    fn make_range_dependency(
        name: &str,
        raw: &str,
        version: &str,
        language: Language,
    ) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Range, raw, version);
        Dependency::new(name, spec, false, language)
    }

    #[test]
    fn test_judge_range_respects_upper_bound() {
        // Regression test: paramiko>=3.5.0,<4.0.0 should NOT update to 4.0.0
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("paramiko", ">=3.5.0,<4.0.0", "3.5.0", Language::Python);
        let versions = vec![
            make_version_info("3.5.0", 100),
            make_version_info("3.6.0", 50),
            make_version_info("3.9.0", 20),
            make_version_info("4.0.0", 10), // Should be excluded by upper bound
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // Should update to 3.9.0, NOT 4.0.0
            assert_eq!(new_version, "3.9.0");
        }
    }

    #[test]
    fn test_judge_range_already_at_max_within_bound() {
        // If already at latest version within bound, should skip
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("paramiko", ">=3.5.0,<4.0.0", "3.9.0", Language::Python);
        let versions = vec![
            make_version_info("3.5.0", 100),
            make_version_info("3.9.0", 20), // current version
            make_version_info("4.0.0", 10), // excluded by upper bound
            make_version_info("4.1.0", 5),  // excluded by upper bound
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip because 3.9.0 is the latest within the bound
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_range_respects_inclusive_upper_bound() {
        // Inclusive upper bound should allow the boundary version (<=2.0)
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("package", ">=1.0,<=2.0", "1.0", Language::Python);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("2.0", 50), // included by upper bound
            make_version_info("2.1", 20), // excluded by upper bound
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0");
        }
    }

    #[test]
    fn test_judge_swift_closed_range_respects_upper_bound() {
        // Swift closed range (`...`) has an inclusive upper bound.
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("vapor/vapor", "4.0.0...4.9.9", "4.0.0", Language::Swift);
        let versions = vec![
            make_version_info("4.0.0", 100),
            make_version_info("4.9.9", 50), // included by upper bound
            make_version_info("5.0.0", 20), // excluded by upper bound
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "4.9.9");
        }
    }

    #[test]
    fn test_judge_range_no_suitable_version_all_above_bound() {
        // If all newer versions are above the upper bound, no suitable version
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("package", ">=1.0,<2.0", "1.0", Language::Python);
        let versions = vec![
            make_version_info("1.0", 100), // current version
            make_version_info("2.0", 50),  // excluded by upper bound
            make_version_info("3.0", 20),  // excluded by upper bound
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip - no version available within bounds that's newer
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_range_without_upper_bound_updates_normally() {
        // Range without upper bound (just >=) should update to latest
        // Note: This is technically not a proper Range in our parser,
        // but testing defensive behavior
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // Create a Range spec without upper bound (edge case)
        let spec = VersionSpec::new(VersionSpecKind::Range, ">=1.0", "1.0");
        let dep = Dependency::new("package", spec, false, Language::Python);

        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("2.0", 50),
            make_version_info("3.0", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // Should update to latest since no upper bound
            assert_eq!(new_version, "3.0");
        }
    }

    #[test]
    fn test_judge_short_version_equivalent_to_full() {
        // Regression test: "0.15" should be considered equal to "0.15.0"
        // So if current version is "0.15" and latest is "0.15.0", it's AlreadyLatest
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // Simulate vte = "0.15" where latest is 0.15.0
        let dep = make_dependency("vte", "0.15", Language::Rust, false);
        let versions = vec![
            make_version_info("0.14.0", 100),
            make_version_info("0.15.0", 50), // latest, but equivalent to current "0.15"
        ];

        let result = judge.judge(&dep, &versions);
        // Should skip because 0.15 == 0.15.0
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_extract_upper_bound_maven_exclusive() {
        // Maven 半開区間の上限抽出
        let result = extract_upper_bound("[1.0,2.0)");
        assert_eq!(result, Some(("2.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_maven_inclusive() {
        // Maven 閉区間の上限抽出
        let result = extract_upper_bound("[1.0,2.0]");
        assert_eq!(result, Some(("2.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_maven_alt_brackets() {
        // Maven 代替記法 ]1.0,2.0[ = exclusive both
        let result = extract_upper_bound("]1.0,2.0[");
        assert_eq!(result, Some(("2.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_swift_half_open() {
        // Swift 半開レンジ（引用符なしの内部表現）
        let result = extract_upper_bound("1.0.0..<2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_swift_closed() {
        // Swift 閉レンジ（引用符なしの内部表現）
        let result = extract_upper_bound("1.0.0...2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_hyphen_range() {
        let result = extract_upper_bound("1.0.0 - 2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_lte() {
        let result = extract_upper_bound(">=1.0.0 <=2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_lt() {
        let result = extract_upper_bound(">=1.0.0 <2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_v_prefix() {
        // v接頭辞は除去される
        let result = extract_upper_bound(">=v1.0.0 <v2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_maven_qualifier() {
        // Maven qualifier 付き上限
        let result = extract_upper_bound("[1.0,2.0.Final)");
        assert_eq!(result, Some(("2.0.Final".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_no_upper() {
        // 上限なし
        let result = extract_upper_bound(">=1.0.0");
        assert_eq!(result, None);
    }

    #[test]
    fn test_should_skip_go_pinned_without_include_pinned() {
        // GoPinned (// pinned コメント付き) は include_pinned なしでスキップされるべき
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);

        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Pinned));
    }

    #[test]
    fn test_should_skip_go_pinned_with_include_pinned() {
        // GoPinned でも --include-pinned なら更新対象になる
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);

        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_judge_go_pinned_skips_update() {
        // GoPinned 依存は judge でもスキップされるべき
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        }
    }

    #[test]
    fn test_judge_go_pinned_with_include_pinned_updates() {
        // GoPinned でも --include-pinned なら更新される
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_extract_upper_bound_maven_alt_inclusive() {
        // Maven 代替記法 ]1.0,2.0] = exclusive lower, inclusive upper
        let result = extract_upper_bound("]1.0,2.0]");
        assert_eq!(result, Some(("2.0".to_string(), true)));
    }

    #[test]
    fn test_judge_short_version_can_still_update() {
        // But if there's a newer version, short versions should still update
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // Simulate tokio = "1" where latest is 1.49.0
        let dep = make_dependency("tokio", "1", Language::Rust, false);
        let versions = vec![
            make_version_info("1.0.0", 200),
            make_version_info("1.48.0", 50),
            make_version_info("1.49.0", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.49.0");
        }
    }
}
