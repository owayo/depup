//! Ruby version specification parser
//!
//! Handles:
//! - Fixed versions: `= 1.2.3`, `1.2.3`
//! - Pessimistic constraints: `~> 1.2`, `~> 1.2.3`
//! - Comparison operators: `>=`, `<`, `>`, `<=`
//! - Compound constraints: `>= 1.0, < 2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Parser for Ruby version specifications
pub struct RubyVersionParser;

// Regex patterns for Ruby version specifications
// Ruby allows optional space between operator and version

// Pessimistic constraint: ~> 1.2 or ~> 1.2.3
static PESSIMISTIC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~>\s*(\d+(?:\.\d+)*)$").unwrap());

// Exact with = prefix: = 1.2.3
static EXACT_EQ_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=\s*(\d+(?:\.\d+)*)$").unwrap());

// Greater than or equal: >= 1.2.3
static GTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^>=\s*(\d+(?:\.\d+)*)$").unwrap());

// Greater than: > 1.2.3
static GT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^>\s*(\d+(?:\.\d+)*)$").unwrap());

// Less than or equal: <= 1.2.3
static LTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<=\s*(\d+(?:\.\d+)*)$").unwrap());

// Less than: < 1.2.3
static LT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<\s*(\d+(?:\.\d+)*)$").unwrap());

// Bare version (exact): 1.2.3
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)$").unwrap());

// Compound constraint pattern (to detect before individual parsing)
static COMPOUND_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",").unwrap());

impl RubyVersionParser {
    /// Parse a single version constraint (not compound)
    fn parse_single(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for pessimistic constraint (~> 1.2.3)
        if let Some(caps) = PESSIMISTIC_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~> "),
            );
        }

        // Check for exact with = prefix (= 1.2.3)
        if let Some(caps) = EXACT_EQ_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("= "),
            );
        }

        // Check for greater than or equal (>= 1.2.3)
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">= "),
            );
        }

        // Check for greater than (> 1.2.3)
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix("> "),
            );
        }

        // Check for less than or equal (<= 1.2.3)
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<= "),
            );
        }

        // Check for less than (< 1.2.3)
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("< "),
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

impl VersionParser for RubyVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for compound constraints (>= 1.0, < 2.0)
        if COMPOUND_RE.is_match(trimmed) {
            // For compound constraints, extract the first version for reference
            let parts: Vec<&str> = trimmed.split(',').collect();
            if let Some(first_part) = parts.first() {
                if let Some(first_spec) = self.parse_single(first_part) {
                    // Return as Range type with the first version as reference
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
        Language::Ruby
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        RubyVersionParser.parse(version)
    }

    // Pessimistic constraint tests
    #[test]
    fn test_parse_pessimistic_minor() {
        let spec = parse("~> 1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.prefix, Some("~> ".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_pessimistic_patch() {
        let spec = parse("~> 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~> ".to_string()));
    }

    #[test]
    fn test_parse_pessimistic_no_space() {
        let spec = parse("~>1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
    }

    // Exact version tests
    #[test]
    fn test_parse_exact_with_equals() {
        let spec = parse("= 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("= ".to_string()));
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_exact_bare() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_exact_no_space() {
        let spec = parse("=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
    }

    // Comparison operator tests
    #[test]
    fn test_parse_greater_or_equal() {
        let spec = parse(">= 1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some(">= ".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_greater_or_equal_no_space() {
        let spec = parse(">=1.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_greater() {
        let spec = parse("> 1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Greater);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some("> ".to_string()));
    }

    #[test]
    fn test_parse_less_or_equal() {
        let spec = parse("<= 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::LessOrEqual);
        assert_eq!(spec.version, "2.0");
        assert_eq!(spec.prefix, Some("<= ".to_string()));
    }

    #[test]
    fn test_parse_less() {
        let spec = parse("< 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Less);
        assert_eq!(spec.version, "2.0");
        assert_eq!(spec.prefix, Some("< ".to_string()));
    }

    // Compound constraint tests
    #[test]
    fn test_parse_compound() {
        let spec = parse(">= 1.0, < 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.raw, ">= 1.0, < 2.0");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_compound_multiple() {
        let spec = parse(">= 1.0, < 2.0, != 1.5.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
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
        let spec = parse("  ~> 1.2.3  ").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
    }

    // Format updated tests
    #[test]
    fn test_format_updated_pessimistic() {
        let spec = parse("~> 1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.3.0"), "~> 1.3.0");
    }

    #[test]
    fn test_format_updated_exact_with_equals() {
        let spec = parse("= 1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "= 2.0.0");
    }

    #[test]
    fn test_format_updated_bare() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_gte() {
        let spec = parse(">= 1.0").unwrap();
        assert_eq!(spec.format_updated("2.0"), ">= 2.0");
    }

    // Language test
    #[test]
    fn test_ruby_parser_language() {
        let parser = RubyVersionParser;
        assert_eq!(parser.language(), Language::Ruby);
    }

    // Version with multiple segments
    #[test]
    fn test_parse_major_only() {
        let spec = parse("1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_major_minor() {
        let spec = parse("1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_four_segments() {
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.4");
    }
}
