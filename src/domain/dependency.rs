//! Dependency information structures

use super::{Language, VersionSpec};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents a package dependency
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// Package name
    pub name: String,
    /// Version specification
    pub version_spec: VersionSpec,
    /// Whether this is a development dependency
    pub is_dev: bool,
    /// The language/ecosystem this dependency belongs to
    pub language: Language,
    /// Optional variable name if version is defined via variable (e.g., Gradle def/val)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_name: Option<String>,
}

impl Dependency {
    /// Creates a new dependency
    pub fn new(
        name: impl Into<String>,
        version_spec: VersionSpec,
        is_dev: bool,
        language: Language,
    ) -> Self {
        Self {
            name: name.into(),
            version_spec,
            is_dev,
            language,
            variable_name: None,
        }
    }

    /// Sets the variable name for this dependency (builder pattern)
    pub fn with_variable(mut self, var_name: impl Into<String>) -> Self {
        self.variable_name = Some(var_name.into());
        self
    }

    /// Creates a new production dependency
    pub fn production(
        name: impl Into<String>,
        version_spec: VersionSpec,
        language: Language,
    ) -> Self {
        Self::new(name, version_spec, false, language)
    }

    /// Creates a new development dependency
    pub fn development(
        name: impl Into<String>,
        version_spec: VersionSpec,
        language: Language,
    ) -> Self {
        Self::new(name, version_spec, true, language)
    }

    /// Returns true if this dependency is pinned
    pub fn is_pinned(&self) -> bool {
        self.version_spec.is_pinned()
    }

    /// Returns the current version string
    pub fn version(&self) -> &str {
        &self.version_spec.version
    }
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dev_marker = if self.is_dev { " (dev)" } else { "" };
        write!(
            f,
            "{}@{}{} [{}]",
            self.name, self.version_spec, dev_marker, self.language
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn sample_version_spec() -> VersionSpec {
        VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^")
    }

    fn exact_version_spec() -> VersionSpec {
        VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3")
    }

    #[test]
    fn test_dependency_new() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert_eq!(dep.name, "lodash");
        assert!(!dep.is_dev);
        assert_eq!(dep.language, Language::Node);
    }

    #[test]
    fn test_dependency_production() {
        let dep = Dependency::production("react", sample_version_spec(), Language::Node);
        assert_eq!(dep.name, "react");
        assert!(!dep.is_dev);
    }

    #[test]
    fn test_dependency_development() {
        let dep = Dependency::development("jest", sample_version_spec(), Language::Node);
        assert_eq!(dep.name, "jest");
        assert!(dep.is_dev);
    }

    #[test]
    fn test_dependency_is_pinned() {
        let pinned = Dependency::new("lodash", exact_version_spec(), false, Language::Node);
        assert!(pinned.is_pinned());

        let not_pinned = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert!(!not_pinned.is_pinned());
    }

    #[test]
    fn test_dependency_version() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert_eq!(dep.version(), "1.2.3");
    }

    #[test]
    fn test_dependency_display_production() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let display = format!("{}", dep);
        assert_eq!(display, "lodash@^1.2.3 [Node.js]");
    }

    #[test]
    fn test_dependency_display_development() {
        let dep = Dependency::new("jest", sample_version_spec(), true, Language::Node);
        let display = format!("{}", dep);
        assert_eq!(display, "jest@^1.2.3 (dev) [Node.js]");
    }

    #[test]
    fn test_dependency_different_languages() {
        let node_dep = Dependency::production("lodash", sample_version_spec(), Language::Node);
        assert_eq!(node_dep.language, Language::Node);

        let python_dep = Dependency::production(
            "requests",
            VersionSpec::new(VersionSpecKind::Caret, "^2.28.0", "2.28.0"),
            Language::Python,
        );
        assert_eq!(python_dep.language, Language::Python);

        let rust_dep = Dependency::production(
            "serde",
            VersionSpec::new(VersionSpecKind::Caret, "1.0", "1.0"),
            Language::Rust,
        );
        assert_eq!(rust_dep.language, Language::Rust);

        let go_dep = Dependency::production(
            "github.com/gin-gonic/gin",
            VersionSpec::new(VersionSpecKind::Exact, "v1.9.0", "1.9.0"),
            Language::Go,
        );
        assert_eq!(go_dep.language, Language::Go);
    }

    #[test]
    fn test_dependency_equality() {
        let dep1 = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let dep2 = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert_eq!(dep1, dep2);
    }

    #[test]
    fn test_dependency_clone() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let cloned = dep.clone();
        assert_eq!(dep, cloned);
    }

    #[test]
    fn test_serde_dependency() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let json = serde_json::to_string(&dep).unwrap();
        let parsed: Dependency = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dep);
    }
}
