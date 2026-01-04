//! Java/Gradle version specification parser
//!
//! Handles:
//! - Fixed versions: `1.2.3`
//! - Versions with prerelease: `1.2.3-SNAPSHOT`, `1.2.3-alpha1`
//!
//! Note: Gradle primarily uses fixed versions. Variable references
//! (e.g., `$version`, `${version}`) are resolved by the manifest parser.

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Parser for Java/Gradle version specifications
pub struct JavaVersionParser;

// Regex patterns for Gradle version specifications
// Gradle uses simple versions, optionally with prerelease identifiers

// Standard version: 1.2.3 or 1.2.3-SNAPSHOT or 1.2.3.RELEASE
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*(?:[.-][A-Za-z0-9]+)*)$").unwrap());

impl VersionParser for JavaVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
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
}
