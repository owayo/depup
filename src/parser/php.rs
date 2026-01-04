//! PHP version specification parser
//!
//! Handles:
//! - Fixed versions: `1.2.3`
//! - Caret ranges: `^1.2.3`
//! - Tilde ranges: `~1.2.3`
//! - Comparison operators: `>=`, `<`, `>`, `<=`
//! - Compound constraints: `>=1.0 <2.0`
//! - Wildcards: `1.2.*`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Parser for PHP version specifications
pub struct PhpVersionParser;

// Regex patterns for PHP version specifications
// PHP/Composer uses standard semver-like patterns

// Caret range: ^1.2.3
static CARET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\^(\d+(?:\.\d+)*)$").unwrap());

// Tilde range: ~1.2.3
static TILDE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^~(\d+(?:\.\d+)*)$").unwrap());

// Greater than or equal: >=1.2.3
static GTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^>=\s*(\d+(?:\.\d+)*)$").unwrap());

// Greater than: >1.2.3
static GT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^>\s*(\d+(?:\.\d+)*)$").unwrap());

// Less than or equal: <=1.2.3
static LTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<=\s*(\d+(?:\.\d+)*)$").unwrap());

// Less than: <1.2.3
static LT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<\s*(\d+(?:\.\d+)*)$").unwrap());

// Wildcard: 1.2.*
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)\.\*$").unwrap());

// Bare version (exact): 1.2.3
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)$").unwrap());

// Compound constraint pattern - OR separator
static COMPOUND_OR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\|\|").unwrap());

// Space-separated compound: requires two operator-prefixed constraints
// e.g., ">=1.0 <2.0" or "^1.0 !=1.5"
// NOT ">=1.0" or ">= 1.0.0" (single constraint with optional space)
static COMPOUND_SPACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Match patterns like: >=1.0 <2.0, ^1.0 ~2.0, etc.
    // First constraint, then space, then another constraint starting with operator
    Regex::new(r"^[<>=^~!].*\s+[<>=^~!]").unwrap()
});

impl PhpVersionParser {
    /// Parse a single version constraint (not compound)
    fn parse_single(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for caret range (^1.2.3)
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Check for tilde range (~1.2.3)
        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~"),
            );
        }

        // Check for greater than or equal (>=1.2.3)
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">="),
            );
        }

        // Check for greater than (>1.2.3)
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix(">"),
            );
        }

        // Check for less than or equal (<=1.2.3)
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<="),
            );
        }

        // Check for less than (<1.2.3)
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("<"),
            );
        }

        // Check for wildcard (1.2.*)
        if let Some(caps) = WILDCARD_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Wildcard, trimmed, version).with_suffix(".*"),
            );
        }

        // Check for bare version (1.2.3) - treated as exact
        if let Some(caps) = BARE_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(VersionSpec::new(VersionSpecKind::Exact, trimmed, version));
        }

        None
    }
}

impl VersionParser for PhpVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for OR compound constraints (||)
        if COMPOUND_OR_RE.is_match(trimmed) {
            // For OR constraints, extract the first version for reference
            let parts: Vec<&str> = trimmed.split("||").collect();
            if let Some(first_part) = parts.first() {
                if let Some(first_spec) = self.parse_single(first_part.trim()) {
                    return Some(VersionSpec::new(
                        VersionSpecKind::Range,
                        trimmed,
                        first_spec.version,
                    ));
                }
            }
            return None;
        }

        // Check for space-separated compound constraints (>=1.0 <2.0)
        if COMPOUND_SPACE_RE.is_match(trimmed) {
            // For space-separated constraints, extract the first version for reference
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(first_part) = parts.first() {
                if let Some(first_spec) = self.parse_single(first_part) {
                    return Some(VersionSpec::new(
                        VersionSpecKind::Range,
                        trimmed,
                        first_spec.version,
                    ));
                }
            }
            return None;
        }

        // Parse single constraint
        self.parse_single(trimmed)
    }

    fn language(&self) -> Language {
        Language::Php
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        PhpVersionParser.parse(version)
    }

    // Caret range tests
    #[test]
    fn test_parse_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("^".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_caret_minor() {
        let spec = parse("^1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_caret_major() {
        let spec = parse("^1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1");
    }

    // Tilde range tests
    #[test]
    fn test_parse_tilde() {
        let spec = parse("~1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_tilde_minor() {
        let spec = parse("~1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2");
    }

    // Comparison operator tests
    #[test]
    fn test_parse_greater_or_equal() {
        let spec = parse(">=1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some(">=".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_greater_or_equal_with_space() {
        let spec = parse(">= 1.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_greater() {
        let spec = parse(">1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Greater);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some(">".to_string()));
    }

    #[test]
    fn test_parse_less_or_equal() {
        let spec = parse("<=2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::LessOrEqual);
        assert_eq!(spec.version, "2.0");
        assert_eq!(spec.prefix, Some("<=".to_string()));
    }

    #[test]
    fn test_parse_less() {
        let spec = parse("<2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Less);
        assert_eq!(spec.version, "2.0");
        assert_eq!(spec.prefix, Some("<".to_string()));
    }

    // Wildcard tests
    #[test]
    fn test_parse_wildcard() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.suffix, Some(".*".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_wildcard_major() {
        let spec = parse("1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1");
    }

    // Exact version tests
    #[test]
    fn test_parse_exact() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_exact_major_minor() {
        let spec = parse("1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2");
    }

    // Compound constraint tests
    #[test]
    fn test_parse_compound_space() {
        let spec = parse(">=1.0 <2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.raw, ">=1.0 <2.0");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_compound_space_multiple() {
        let spec = parse(">=1.0 <2.0 !=1.5.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_compound_or() {
        let spec = parse("^1.0 || ^2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.raw, "^1.0 || ^2.0");
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
    fn test_parse_with_leading_trailing_whitespace() {
        let spec = parse("  ^1.2.3  ").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
    }

    // Format updated tests
    #[test]
    fn test_format_updated_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.3.0"), "^1.3.0");
    }

    #[test]
    fn test_format_updated_tilde() {
        let spec = parse("~1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.2.5"), "~1.2.5");
    }

    #[test]
    fn test_format_updated_exact() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_wildcard() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.format_updated("1.3"), "1.3.*");
    }

    #[test]
    fn test_format_updated_gte() {
        let spec = parse(">=1.0").unwrap();
        assert_eq!(spec.format_updated("2.0"), ">=2.0");
    }

    // Language test
    #[test]
    fn test_php_parser_language() {
        let parser = PhpVersionParser;
        assert_eq!(parser.language(), Language::Php);
    }
}
