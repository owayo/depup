//! Update decision result types

use super::Dependency;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Reason why a dependency update was skipped
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Version is pinned (exact version specified)
    Pinned,
    /// Already at the latest version
    AlreadyLatest,
    /// Package was excluded via --exclude flag
    Excluded,
    /// Package not in --only list
    NotInOnlyList,
    /// Failed to fetch version info from registry
    FetchFailed(String),
    /// No suitable version found (e.g., age filter excluded all versions)
    NoSuitableVersion,
    /// Version parsing failed
    ParseError(String),
    /// Language filter excluded this package
    LanguageFiltered,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Pinned => write!(f, "pinned version"),
            SkipReason::AlreadyLatest => write!(f, "already at latest"),
            SkipReason::Excluded => write!(f, "excluded by --exclude"),
            SkipReason::NotInOnlyList => write!(f, "not in --only list"),
            SkipReason::FetchFailed(msg) => write!(f, "fetch failed: {}", msg),
            SkipReason::NoSuitableVersion => write!(f, "no suitable version"),
            SkipReason::ParseError(msg) => write!(f, "parse error: {}", msg),
            SkipReason::LanguageFiltered => write!(f, "language filtered"),
        }
    }
}

/// Result of an update decision for a single dependency
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateResult {
    /// Dependency will be updated
    Update {
        /// The dependency being updated
        dependency: Dependency,
        /// The new version to update to
        new_version: String,
    },
    /// Dependency update was skipped
    Skip {
        /// The dependency that was skipped
        dependency: Dependency,
        /// The reason for skipping
        reason: SkipReason,
    },
}

impl UpdateResult {
    /// Creates an Update result
    pub fn update(dependency: Dependency, new_version: impl Into<String>) -> Self {
        UpdateResult::Update {
            dependency,
            new_version: new_version.into(),
        }
    }

    /// Creates a Skip result
    pub fn skip(dependency: Dependency, reason: SkipReason) -> Self {
        UpdateResult::Skip { dependency, reason }
    }

    /// Creates a Skip result for pinned version
    pub fn skip_pinned(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::Pinned)
    }

    /// Creates a Skip result for already at latest
    pub fn skip_already_latest(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::AlreadyLatest)
    }

    /// Creates a Skip result for excluded package
    pub fn skip_excluded(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::Excluded)
    }

    /// Creates a Skip result for not in only list
    pub fn skip_not_in_only_list(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::NotInOnlyList)
    }

    /// Creates a Skip result for fetch failure
    pub fn skip_fetch_failed(dependency: Dependency, message: impl Into<String>) -> Self {
        Self::skip(dependency, SkipReason::FetchFailed(message.into()))
    }

    /// Returns true if this is an update result
    pub fn is_update(&self) -> bool {
        matches!(self, UpdateResult::Update { .. })
    }

    /// Returns true if this is a skip result
    pub fn is_skip(&self) -> bool {
        matches!(self, UpdateResult::Skip { .. })
    }

    /// Returns the dependency reference
    pub fn dependency(&self) -> &Dependency {
        match self {
            UpdateResult::Update { dependency, .. } => dependency,
            UpdateResult::Skip { dependency, .. } => dependency,
        }
    }

    /// Returns the package name
    pub fn package_name(&self) -> &str {
        &self.dependency().name
    }
}

impl fmt::Display for UpdateResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateResult::Update {
                dependency,
                new_version,
            } => {
                write!(
                    f,
                    "{}: {} → {}",
                    dependency.name,
                    dependency.version(),
                    new_version
                )
            }
            UpdateResult::Skip { dependency, reason } => {
                write!(f, "{}: skipped ({})", dependency.name, reason)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Language, VersionSpec, VersionSpecKind};

    fn sample_dependency() -> Dependency {
        Dependency::new(
            "lodash",
            VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^"),
            false,
            Language::Node,
        )
    }

    fn pinned_dependency() -> Dependency {
        Dependency::new(
            "lodash",
            VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3"),
            false,
            Language::Node,
        )
    }

    #[test]
    fn test_skip_reason_display() {
        assert_eq!(format!("{}", SkipReason::Pinned), "pinned version");
        assert_eq!(
            format!("{}", SkipReason::AlreadyLatest),
            "already at latest"
        );
        assert_eq!(format!("{}", SkipReason::Excluded), "excluded by --exclude");
        assert_eq!(
            format!("{}", SkipReason::NotInOnlyList),
            "not in --only list"
        );
        assert_eq!(
            format!("{}", SkipReason::FetchFailed("timeout".to_string())),
            "fetch failed: timeout"
        );
        assert_eq!(
            format!("{}", SkipReason::NoSuitableVersion),
            "no suitable version"
        );
        assert_eq!(
            format!("{}", SkipReason::ParseError("invalid".to_string())),
            "parse error: invalid"
        );
        assert_eq!(
            format!("{}", SkipReason::LanguageFiltered),
            "language filtered"
        );
    }

    #[test]
    fn test_update_result_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep.clone(), "2.0.0");

        assert!(result.is_update());
        assert!(!result.is_skip());
        assert_eq!(result.package_name(), "lodash");

        if let UpdateResult::Update {
            dependency,
            new_version,
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(new_version, "2.0.0");
        } else {
            panic!("Expected Update variant");
        }
    }

    #[test]
    fn test_update_result_skip() {
        let dep = sample_dependency();
        let result = UpdateResult::skip(dep.clone(), SkipReason::Excluded);

        assert!(!result.is_update());
        assert!(result.is_skip());
        assert_eq!(result.package_name(), "lodash");

        if let UpdateResult::Skip { dependency, reason } = result {
            assert_eq!(dependency, dep);
            assert_eq!(reason, SkipReason::Excluded);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_pinned() {
        let dep = pinned_dependency();
        let result = UpdateResult::skip_pinned(dep.clone());

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_already_latest() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_already_latest(dep);

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_excluded() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_excluded(dep);

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Excluded);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_not_in_only_list() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_not_in_only_list(dep);

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::NotInOnlyList);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_fetch_failed() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_fetch_failed(dep, "network error");

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::FetchFailed("network error".to_string()));
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_dependency() {
        let dep = sample_dependency();
        let update = UpdateResult::update(dep.clone(), "2.0.0");
        assert_eq!(update.dependency(), &dep);

        let skip = UpdateResult::skip(dep.clone(), SkipReason::Pinned);
        assert_eq!(skip.dependency(), &dep);
    }

    #[test]
    fn test_update_result_display_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0");
        assert_eq!(format!("{}", result), "lodash: 1.2.3 → 2.0.0");
    }

    #[test]
    fn test_update_result_display_skip() {
        let dep = sample_dependency();
        let result = UpdateResult::skip(dep, SkipReason::Pinned);
        assert_eq!(format!("{}", result), "lodash: skipped (pinned version)");
    }

    #[test]
    fn test_skip_reason_equality() {
        assert_eq!(SkipReason::Pinned, SkipReason::Pinned);
        assert_ne!(SkipReason::Pinned, SkipReason::Excluded);
    }

    #[test]
    fn test_skip_reason_clone() {
        let reason = SkipReason::FetchFailed("error".to_string());
        let cloned = reason.clone();
        assert_eq!(reason, cloned);
    }

    #[test]
    fn test_update_result_clone() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0");
        let cloned = result.clone();
        assert_eq!(result, cloned);
    }

    #[test]
    fn test_serde_skip_reason() {
        let reason = SkipReason::FetchFailed("timeout".to_string());
        let json = serde_json::to_string(&reason).unwrap();
        let parsed: SkipReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }

    #[test]
    fn test_serde_update_result_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"type\":\"update\""));
        let parsed: UpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn test_serde_update_result_skip() {
        let dep = sample_dependency();
        let result = UpdateResult::skip(dep, SkipReason::Excluded);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"type\":\"skip\""));
        let parsed: UpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }
}
