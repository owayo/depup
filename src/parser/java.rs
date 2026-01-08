//! Java/Gradle version specification parser
//!
//! Handles:
//! - Fixed versions: `1.2.3`, `1.2.3-SNAPSHOT`, `1.2.3-alpha1`
//! - Prefix versions: `1.2.+` (matches any version starting with 1.2)
//! - Dynamic versions: `latest.release`, `latest.integration`
//! - Maven-style ranges: `[1.0,2.0]`, `[1.0,)`, `(,2.0]`, `[1.0,2.0)`
//!
//! Note: Variable references (e.g., `$version`, `${version}`)
//! are resolved by the manifest parser.

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Parser for Java/Gradle version specifications
pub struct JavaVersionParser;

// Regex patterns for Gradle version specifications

// Standard version: 1.2.3 or 1.2.3-SNAPSHOT or 1.2.3.RELEASE
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*(?:[.-][A-Za-z0-9]+)*)$").unwrap());

// Prefix version: 1.2.+ or 1.+
static PREFIX_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)\.\+$").unwrap());

// Dynamic versions: latest.release, latest.integration
static DYNAMIC_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(latest\.(?:release|integration))$").unwrap());

// Maven-style range: [1.0,2.0], [1.0,), (,2.0], [1.0,2.0)
// Format: [(] lower , upper [)] where lower/upper can be empty or version
static MAVEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\[\(](\d+(?:\.\d+)*(?:[.-][A-Za-z0-9]+)?)?\s*,\s*(\d+(?:\.\d+)*(?:[.-][A-Za-z0-9]+)?)?[\]\)]$").unwrap()
});

impl VersionParser for JavaVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for Maven-style range: [1.0,2.0], [1.0,), (,2.0]
        if MAVEN_RANGE_RE.is_match(trimmed) {
            // Extract the lower bound as the base version, if present
            if let Some(caps) = MAVEN_RANGE_RE.captures(trimmed) {
                let lower = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let upper = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                // Use lower bound as version if present, otherwise use upper
                let version = if !lower.is_empty() {
                    lower
                } else if !upper.is_empty() {
                    upper
                } else {
                    ""
                };
                return Some(VersionSpec::new(VersionSpecKind::Range, trimmed, version));
            }
        }

        // Check for prefix version: 1.2.+
        if let Some(caps) = PREFIX_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                version,
            ));
        }

        // Check for dynamic version: latest.release, latest.integration
        if DYNAMIC_VERSION_RE.is_match(trimmed) {
            return Some(VersionSpec::new(VersionSpecKind::Wildcard, trimmed, ""));
        }

        // Check for standard version (including prerelease identifiers)
        if let Some(caps) = VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(VersionSpec::new(VersionSpecKind::Exact, trimmed, version));
        }

        None
    }

    fn language(&self) -> Language {
        Language::Java
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        JavaVersionParser.parse(version)
    }

    // Basic version tests
    #[test]
    fn test_parse_simple_version() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_major_minor() {
        let spec = parse("1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_major_only() {
        let spec = parse("1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_four_segments() {
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.4");
    }

    // Prerelease version tests
    #[test]
    fn test_parse_snapshot() {
        let spec = parse("1.2.3-SNAPSHOT").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-SNAPSHOT");
    }

    #[test]
    fn test_parse_alpha() {
        let spec = parse("1.2.3-alpha1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-alpha1");
    }

    #[test]
    fn test_parse_beta() {
        let spec = parse("2.0.0-beta2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "2.0.0-beta2");
    }

    #[test]
    fn test_parse_rc() {
        let spec = parse("3.0.0-RC1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "3.0.0-RC1");
    }

    #[test]
    fn test_parse_release() {
        let spec = parse("5.0.0.RELEASE").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "5.0.0.RELEASE");
    }

    #[test]
    fn test_parse_final() {
        let spec = parse("4.0.0.Final").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "4.0.0.Final");
    }

    // Edge case tests
    #[test]
    fn test_parse_empty() {
        assert!(parse("").is_none());
    }

    #[test]
    fn test_parse_whitespace() {
        assert!(parse("   ").is_none());
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse("not-a-version").is_none());
    }

    #[test]
    fn test_parse_variable_reference() {
        // Variable references should not be parsed by version parser
        // They are handled by manifest parser
        assert!(parse("$wicketVersion").is_none());
        assert!(parse("${wicketVersion}").is_none());
    }

    #[test]
    fn test_parse_with_leading_trailing_whitespace() {
        let spec = parse("  1.2.3  ").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
    }

    // Format updated tests
    #[test]
    fn test_format_updated_simple() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_snapshot() {
        let spec = parse("1.2.3-SNAPSHOT").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    // Language test
    #[test]
    fn test_java_parser_language() {
        let parser = JavaVersionParser;
        assert_eq!(parser.language(), Language::Java);
    }

    // Gradle version range tests (user-requested)
    // implementation("org.springframework:spring-core:5.3.8")
    #[test]
    fn test_parse_gradle_exact_version() {
        let spec = parse("5.3.8").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "5.3.8");
        assert!(spec.is_pinned());
    }

    // implementation("org.springframework:spring-core:5.3.+")
    #[test]
    fn test_parse_gradle_prefix_version() {
        let spec = parse("5.3.+").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "5.3");
        assert!(!spec.is_pinned());
    }

    // implementation("org.springframework:spring-core:latest.release")
    #[test]
    fn test_parse_gradle_latest_release() {
        let spec = parse("latest.release").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_latest_integration() {
        let spec = parse("latest.integration").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert!(!spec.is_pinned());
    }

    // implementation("org.springframework:spring-core:[5.2.0, 5.3.8]")
    #[test]
    fn test_parse_gradle_maven_range_closed() {
        let spec = parse("[5.2.0, 5.3.8]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "5.2.0"); // lower bound
        assert!(!spec.is_pinned());
    }

    // implementation("org.springframework:spring-core:[5.2.0,)")
    #[test]
    fn test_parse_gradle_maven_range_open_upper() {
        let spec = parse("[5.2.0,)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "5.2.0"); // lower bound
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_maven_range_open_lower() {
        let spec = parse("(,2.0.0]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "2.0.0"); // upper bound when lower is empty
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_maven_range_exclusive() {
        let spec = parse("(1.0.0,2.0.0)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0"); // lower bound
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_prefix_version_single_segment() {
        let spec = parse("1.+").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1");
        assert!(!spec.is_pinned());
    }
}
