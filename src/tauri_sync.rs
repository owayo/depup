//! Tauri version synchronization
//!
//! Ensures that Tauri npm packages and tauri crate have matching
//! major.minor versions to prevent build errors.
//!
//! Tauri requires:
//! - npm: @tauri-apps/api, @tauri-apps/cli
//! - crates.io: tauri
//!   All must have the same major.minor version (e.g., 2.10.x)

use crate::domain::{Language, UpdateResult};
use crate::update::VersionInfo;
use std::cmp::Ordering;

/// Package names for Tauri npm packages
pub const TAURI_NPM_PACKAGES: &[&str] = &["@tauri-apps/api", "@tauri-apps/cli"];

/// Package name for Tauri crate
pub const TAURI_CRATE: &str = "tauri";

/// Extract major.minor from a version string
/// e.g., "2.10.1" -> Some((2, 10))
pub fn extract_major_minor(version: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        Some((major, minor))
    } else {
        None
    }
}

/// Compare major.minor versions
/// Returns true if they match
pub fn versions_match(v1: &str, v2: &str) -> bool {
    match (extract_major_minor(v1), extract_major_minor(v2)) {
        (Some((m1, n1)), Some((m2, n2))) => m1 == m2 && n1 == n2,
        _ => false,
    }
}

/// Find the best matching version from available versions
/// that has the same major.minor as the target version
pub fn find_matching_version(
    target_major_minor: (u32, u32),
    available_versions: &[VersionInfo],
) -> Option<&VersionInfo> {
    available_versions
        .iter()
        .filter(|v| !v.is_prerelease())
        .filter(|v| {
            if let Some((major, minor)) = extract_major_minor(&v.version) {
                major == target_major_minor.0 && minor == target_major_minor.1
            } else {
                false
            }
        })
        .max() // Get the latest patch version within the major.minor
}

/// Find the highest major.minor version available in both npm and crates.io
pub fn find_common_major_minor(
    npm_versions: &[VersionInfo],
    crate_versions: &[VersionInfo],
) -> Option<(u32, u32)> {
    // Collect all major.minor pairs from npm (stable only)
    let npm_pairs: std::collections::HashSet<(u32, u32)> = npm_versions
        .iter()
        .filter(|v| !v.is_prerelease())
        .filter_map(|v| extract_major_minor(&v.version))
        .collect();

    // Collect all major.minor pairs from crates.io (stable only)
    let crate_pairs: std::collections::HashSet<(u32, u32)> = crate_versions
        .iter()
        .filter(|v| !v.is_prerelease())
        .filter_map(|v| extract_major_minor(&v.version))
        .collect();

    // Find common pairs
    let common: Vec<(u32, u32)> = npm_pairs.intersection(&crate_pairs).copied().collect();

    // Return the highest common major.minor
    common.into_iter().max_by(|a, b| match a.0.cmp(&b.0) {
        Ordering::Equal => a.1.cmp(&b.1),
        ord => ord,
    })
}

/// Information about a Tauri package's current and target versions
#[derive(Debug, Clone)]
pub struct TauriPackageInfo {
    /// Package name
    pub name: String,
    /// Current version (from manifest)
    pub current_version: String,
    /// Target version (from update result, if any)
    pub target_version: Option<String>,
    /// Whether this package has an update pending
    pub has_update: bool,
}

/// Synchronize Tauri package versions
///
/// Ensures all Tauri packages have matching major.minor versions.
pub struct TauriVersionSync {
    /// Available npm versions for @tauri-apps/api (used for all npm packages)
    npm_versions: Vec<VersionInfo>,
    /// Available crates.io versions for tauri
    crate_versions: Vec<VersionInfo>,
}

impl TauriVersionSync {
    /// Create a new TauriVersionSync with available versions
    pub fn new(npm_versions: Vec<VersionInfo>, crate_versions: Vec<VersionInfo>) -> Self {
        Self {
            npm_versions,
            crate_versions,
        }
    }

    /// Get the synchronized target version for all packages
    ///
    /// Takes the target crate version and returns the major.minor that all packages
    /// should use. Returns (npm_target_version, crate_target_version).
    pub fn get_synchronized_versions(
        &self,
        crate_target_version: &str,
    ) -> Option<(String, String)> {
        let target_mm = extract_major_minor(crate_target_version)?;

        // Check if this major.minor is available in npm
        let npm_version = find_matching_version(target_mm, &self.npm_versions)?;
        let crate_version = find_matching_version(target_mm, &self.crate_versions)?;

        Some((npm_version.version.clone(), crate_version.version.clone()))
    }

    /// Get the highest common version available in both registries
    pub fn get_latest_common_version(&self) -> Option<(String, String)> {
        let common_mm = find_common_major_minor(&self.npm_versions, &self.crate_versions)?;

        let npm_version = find_matching_version(common_mm, &self.npm_versions)?;
        let crate_version = find_matching_version(common_mm, &self.crate_versions)?;

        Some((npm_version.version.clone(), crate_version.version.clone()))
    }

    /// Check if a package is a Tauri npm package
    pub fn is_tauri_npm_package(name: &str) -> bool {
        TAURI_NPM_PACKAGES.contains(&name)
    }

    /// Check if a package is the Tauri crate
    pub fn is_tauri_crate(name: &str) -> bool {
        name == TAURI_CRATE
    }

    /// Check if a package is any Tauri package
    pub fn is_tauri_package(name: &str, language: Language) -> bool {
        match language {
            Language::Node => Self::is_tauri_npm_package(name),
            Language::Rust => Self::is_tauri_crate(name),
            _ => false,
        }
    }

    /// Synchronize update results for Tauri packages
    ///
    /// Given current versions and update results, determines if versions need adjustment.
    /// Returns (npm_target, crate_target) where Some means the version should be updated/adjusted.
    pub fn synchronize_with_current(
        &self,
        npm_current: Option<&str>,
        npm_update: Option<&UpdateResult>,
        crate_current: Option<&str>,
        crate_update: Option<&UpdateResult>,
    ) -> (Option<String>, Option<String>) {
        // Determine effective target versions
        let npm_target = npm_update
            .and_then(|r| match r {
                UpdateResult::Update { new_version, .. } => Some(new_version.as_str()),
                _ => None,
            })
            .or(npm_current);

        let crate_target = crate_update
            .and_then(|r| match r {
                UpdateResult::Update { new_version, .. } => Some(new_version.as_str()),
                _ => None,
            })
            .or(crate_current);

        // If we don't have both, can't sync
        let (npm_v, crate_v) = match (npm_target, crate_target) {
            (Some(n), Some(c)) => (n, c),
            _ => return (None, None),
        };

        // Check if they already match
        if versions_match(npm_v, crate_v) {
            return (None, None);
        }

        // They don't match - find the highest common version
        let (npm_sync, crate_sync) = match self.get_latest_common_version() {
            Some(v) => v,
            None => return (None, None),
        };

        // Return versions only if they need to change from what would otherwise be set
        let npm_result = if npm_update.map(|r| r.is_update()).unwrap_or(false) {
            // npm has an update - check if it needs adjustment
            let planned = match npm_update.unwrap() {
                UpdateResult::Update { new_version, .. } => new_version.as_str(),
                _ => npm_v,
            };
            if planned != npm_sync {
                Some(npm_sync.clone())
            } else {
                None
            }
        } else {
            // npm doesn't have an update - need to add one if current differs
            if npm_current
                .map(|v| !versions_match(v, &crate_sync))
                .unwrap_or(false)
            {
                Some(npm_sync.clone())
            } else {
                None
            }
        };

        let crate_result = if crate_update.map(|r| r.is_update()).unwrap_or(false) {
            // crate has an update - check if it needs adjustment
            let planned = match crate_update.unwrap() {
                UpdateResult::Update { new_version, .. } => new_version.as_str(),
                _ => crate_v,
            };
            if planned != crate_sync {
                Some(crate_sync.clone())
            } else {
                None
            }
        } else {
            // crate doesn't have an update - need to add one if current differs
            if crate_current
                .map(|v| !versions_match(v, &npm_sync))
                .unwrap_or(false)
            {
                Some(crate_sync)
            } else {
                None
            }
        };

        (npm_result, crate_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use chrono::Utc;

    fn make_version_info(version: &str) -> VersionInfo {
        VersionInfo::new(version, Utc::now())
    }

    fn make_dependency(name: &str, version: &str, language: Language) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, false, language)
    }

    #[test]
    fn test_extract_major_minor() {
        assert_eq!(extract_major_minor("2.10.1"), Some((2, 10)));
        assert_eq!(extract_major_minor("1.0.0"), Some((1, 0)));
        assert_eq!(extract_major_minor("2.9"), Some((2, 9)));
        assert_eq!(extract_major_minor("invalid"), None);
    }

    #[test]
    fn test_versions_match() {
        assert!(versions_match("2.10.1", "2.10.0"));
        assert!(versions_match("2.10.1", "2.10.5"));
        assert!(!versions_match("2.10.1", "2.9.1"));
        assert!(!versions_match("2.10.1", "3.10.1"));
    }

    #[test]
    fn test_find_common_major_minor() {
        let npm_versions = vec![
            make_version_info("2.8.0"),
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.2"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let common = find_common_major_minor(&npm_versions, &crate_versions);
        // Highest common should be 2.10
        assert_eq!(common, Some((2, 10)));
    }

    #[test]
    fn test_find_common_major_minor_no_overlap() {
        let npm_versions = vec![make_version_info("2.8.0"), make_version_info("2.9.0")];
        let crate_versions = vec![make_version_info("2.10.0"), make_version_info("2.11.0")];

        let common = find_common_major_minor(&npm_versions, &crate_versions);
        assert_eq!(common, None);
    }

    #[test]
    fn test_find_matching_version() {
        let versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let matching = find_matching_version((2, 10), &versions);
        assert!(matching.is_some());
        assert_eq!(matching.unwrap().version, "2.10.1"); // Latest patch
    }

    #[test]
    fn test_is_tauri_package() {
        assert!(TauriVersionSync::is_tauri_package(
            "@tauri-apps/api",
            Language::Node
        ));
        assert!(TauriVersionSync::is_tauri_package(
            "@tauri-apps/cli",
            Language::Node
        ));
        assert!(TauriVersionSync::is_tauri_package("tauri", Language::Rust));
        assert!(!TauriVersionSync::is_tauri_package("tauri", Language::Node));
        assert!(!TauriVersionSync::is_tauri_package(
            "@tauri-apps/api",
            Language::Rust
        ));
        assert!(!TauriVersionSync::is_tauri_package(
            "lodash",
            Language::Node
        ));
    }

    #[test]
    fn test_is_tauri_npm_package() {
        assert!(TauriVersionSync::is_tauri_npm_package("@tauri-apps/api"));
        assert!(TauriVersionSync::is_tauri_npm_package("@tauri-apps/cli"));
        assert!(!TauriVersionSync::is_tauri_npm_package("tauri"));
        assert!(!TauriVersionSync::is_tauri_npm_package("lodash"));
    }

    #[test]
    fn test_synchronize_crate_only_updating() {
        // npm is at 2.9.1, crate is being updated to 2.10.1
        // npm should be updated to 2.10.x to match
        let npm_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.5"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let crate_dep = make_dependency(TAURI_CRATE, "2.9.5", Language::Rust);
        let crate_result = UpdateResult::update(crate_dep, "2.10.1");

        let (npm_adj, crate_adj) = sync.synchronize_with_current(
            Some("2.9.1"), // npm current
            None,          // npm not being updated
            Some("2.9.5"), // crate current
            Some(&crate_result),
        );

        // npm should be updated to 2.10.1 to match crate
        assert_eq!(npm_adj, Some("2.10.1".to_string()));
        // crate is already going to 2.10.1, no adjustment needed
        assert_eq!(crate_adj, None);
    }

    #[test]
    fn test_synchronize_both_updating_mismatch() {
        let npm_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.2"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let npm_dep = make_dependency("@tauri-apps/api", "2.8.0", Language::Node);
        let crate_dep = make_dependency(TAURI_CRATE, "2.8.0", Language::Rust);

        let npm_result = UpdateResult::update(npm_dep, "2.9.1");
        let crate_result = UpdateResult::update(crate_dep, "2.10.1");

        let (npm_adj, crate_adj) = sync.synchronize_with_current(
            Some("2.8.0"),
            Some(&npm_result),
            Some("2.8.0"),
            Some(&crate_result),
        );

        // Should adjust to highest common version (2.10)
        assert_eq!(npm_adj, Some("2.10.0".to_string()));
        assert_eq!(crate_adj, None); // crate is already at 2.10.1
    }

    #[test]
    fn test_synchronize_already_matching() {
        let npm_versions = vec![make_version_info("2.10.0")];
        let crate_versions = vec![make_version_info("2.10.1")];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let npm_dep = make_dependency("@tauri-apps/api", "2.9.0", Language::Node);
        let crate_dep = make_dependency(TAURI_CRATE, "2.9.0", Language::Rust);

        let npm_result = UpdateResult::update(npm_dep, "2.10.0");
        let crate_result = UpdateResult::update(crate_dep, "2.10.1");

        let (npm_adj, crate_adj) = sync.synchronize_with_current(
            Some("2.9.0"),
            Some(&npm_result),
            Some("2.9.0"),
            Some(&crate_result),
        );

        // Already matching major.minor, no adjustment needed
        assert_eq!(npm_adj, None);
        assert_eq!(crate_adj, None);
    }

    #[test]
    fn test_get_synchronized_versions() {
        let npm_versions = vec![
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.5"),
            make_version_info("2.10.1"),
            make_version_info("2.10.2"),
        ];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let result = sync.get_synchronized_versions("2.10.1");
        assert!(result.is_some());
        let (npm, crate_v) = result.unwrap();
        assert_eq!(npm, "2.10.1");
        assert_eq!(crate_v, "2.10.2"); // Latest patch in 2.10
    }
}
