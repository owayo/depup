//! Version information from registry
//!
//! This module provides the VersionInfo struct that represents
//! a package version with its release date.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Information about a package version from the registry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// The version string (e.g., "1.2.3")
    pub version: String,
    /// When this version was released
    pub released_at: DateTime<Utc>,
}

impl VersionInfo {
    /// Create a new VersionInfo
    pub fn new(version: impl Into<String>, released_at: DateTime<Utc>) -> Self {
        Self {
            version: version.into(),
            released_at,
        }
    }

    /// Create a VersionInfo with current time as release date
    pub fn now(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            released_at: Utc::now(),
        }
    }

    /// Check if this version is a pre-release (alpha, beta, rc, canary, dev, etc.)
    pub fn is_prerelease(&self) -> bool {
        is_prerelease_version(&self.version)
    }
}

/// Pre-release identifiers to check for
const PRERELEASE_IDENTIFIERS: &[&str] = &[
    "alpha",
    "beta",
    "rc",
    "canary",
    "dev",
    "preview",
    "next",
    "nightly",
    "snapshot",
    "pre",
    "insiders",
    "experimental",
];

/// Check if a version string represents a pre-release version
pub fn is_prerelease_version(version: &str) -> bool {
    let lower = version.to_lowercase();
    PRERELEASE_IDENTIFIERS.iter().any(|id| lower.contains(id))
}

impl Ord for VersionInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare by version using semver-like comparison
        compare_versions(&self.version, &other.version)
    }
}

impl PartialOrd for VersionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Compare two version strings using semver-like rules
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_parts = |s: &str| -> Vec<u64> {
        // Remove leading 'v' if present
        let s = s.strip_prefix('v').unwrap_or(s);
        // Split by . and - and take only the numeric parts
        s.split(['.', '-']).filter_map(|p| p.parse().ok()).collect()
    };

    let parts_a = parse_parts(a);
    let parts_b = parse_parts(b);

    // Compare each part
    for (pa, pb) in parts_a.iter().zip(parts_b.iter()) {
        match pa.cmp(pb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    // If all common parts are equal, the longer version is greater
    parts_a.len().cmp(&parts_b.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_version_info_new() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.released_at, date);
    }

    #[test]
    fn test_version_info_now() {
        let before = Utc::now();
        let info = VersionInfo::now("1.0.0");
        let after = Utc::now();

        assert_eq!(info.version, "1.0.0");
        assert!(info.released_at >= before);
        assert!(info.released_at <= after);
    }

    #[test]
    fn test_version_comparison_simple() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("2.0.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_minor() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.1.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_patch() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.1");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_equal() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.0");
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_with_v_prefix() {
        let v1 = VersionInfo::now("v1.0.0");
        let v2 = VersionInfo::now("v2.0.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_mixed_prefix() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("v1.0.0");
        // Should be equal (v prefix is stripped)
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_different_lengths() {
        let v1 = VersionInfo::now("1.0");
        let v2 = VersionInfo::now("1.0.0");
        // 1.0 is considered less than 1.0.0 (fewer parts)
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_prerelease() {
        // This is a simplified comparison - it treats pre-release parts as numbers
        let v1 = VersionInfo::now("1.0.0-alpha");
        let v2 = VersionInfo::now("1.0.0-beta");
        // Since alpha/beta aren't numeric, they're ignored
        // This means 1.0.0-alpha == 1.0.0-beta in our simple comparison
        // For production use, we'd want full semver parsing
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_versions_basic() {
        assert_eq!(
            compare_versions("1.0.0", "1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(compare_versions("1.0.0", "2.0.0"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("2.0.0", "1.0.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_versions_multi_digit() {
        assert!(compare_versions("1.9.0", "1.10.0") == std::cmp::Ordering::Less);
        assert!(compare_versions("10.0.0", "9.0.0") == std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_serde_version_info() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);

        let json = serde_json::to_string(&info).unwrap();
        let parsed: VersionInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, "1.2.3");
        assert_eq!(parsed.released_at, date);
    }

    #[test]
    fn test_version_info_clone() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);
        let cloned = info.clone();

        assert_eq!(info, cloned);
    }

    #[test]
    fn test_version_sorting() {
        let mut versions = vec![
            VersionInfo::now("2.0.0"),
            VersionInfo::now("1.0.0"),
            VersionInfo::now("1.5.0"),
            VersionInfo::now("1.0.1"),
        ];

        versions.sort();

        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[1].version, "1.0.1");
        assert_eq!(versions[2].version, "1.5.0");
        assert_eq!(versions[3].version, "2.0.0");
    }

    #[test]
    fn test_find_max_version() {
        let versions = vec![
            VersionInfo::now("1.0.0"),
            VersionInfo::now("2.5.0"),
            VersionInfo::now("2.0.0"),
            VersionInfo::now("1.9.9"),
        ];

        let max = versions.iter().max().unwrap();
        assert_eq!(max.version, "2.5.0");
    }

    #[test]
    fn test_is_prerelease_stable_versions() {
        // Stable versions should NOT be detected as prerelease
        assert!(!is_prerelease_version("1.0.0"));
        assert!(!is_prerelease_version("2.5.3"));
        assert!(!is_prerelease_version("v1.0.0"));
        assert!(!is_prerelease_version("10.20.30"));
    }

    #[test]
    fn test_is_prerelease_alpha_beta_rc() {
        assert!(is_prerelease_version("1.0.0-alpha"));
        assert!(is_prerelease_version("1.0.0-alpha.1"));
        assert!(is_prerelease_version("1.0.0-beta"));
        assert!(is_prerelease_version("1.0.0-beta.2"));
        assert!(is_prerelease_version("1.0.0-rc.1"));
        assert!(is_prerelease_version("2.0.0-RC1"));
    }

    #[test]
    fn test_is_prerelease_canary_dev() {
        // React-style canary versions
        assert!(is_prerelease_version("19.3.0-canary-52684925-20251110"));
        // TypeScript-style dev versions
        assert!(is_prerelease_version("6.0.0-dev.20260103"));
        // Vite-style beta versions
        assert!(is_prerelease_version("8.0.0-beta.5"));
        // Prettier-style alpha
        assert!(is_prerelease_version("4.0.0-alpha.13"));
    }

    #[test]
    fn test_is_prerelease_other_identifiers() {
        assert!(is_prerelease_version("1.0.0-preview"));
        assert!(is_prerelease_version("1.0.0-next"));
        assert!(is_prerelease_version("1.0.0-nightly"));
        assert!(is_prerelease_version("1.0.0-snapshot"));
        assert!(is_prerelease_version("1.0.0-pre.1"));
        assert!(is_prerelease_version("1.0.0-insiders"));
        assert!(is_prerelease_version("1.0.0-experimental"));
    }

    #[test]
    fn test_version_info_is_prerelease() {
        let stable = VersionInfo::now("1.0.0");
        assert!(!stable.is_prerelease());

        let canary = VersionInfo::now("19.3.0-canary-52684925-20251110");
        assert!(canary.is_prerelease());

        let beta = VersionInfo::now("8.0.0-beta.5");
        assert!(beta.is_prerelease());
    }
}
