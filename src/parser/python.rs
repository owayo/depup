//! Python (pip/poetry) version specification parser
//!
//! Handles version formats:
//! - Exact: `==1.2.3`
//! - Caret: `^1.2.3` (Poetry)
//! - Tilde: `~1.2.3` or `~=1.2.3` (compatible release)
//! - Comparison: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`, `!=1.2.3`
//! - Wildcard: `*`, `1.*`
//! - Range: `>=1.0,<2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Python version specification parser
pub struct PythonVersionParser;

// Regex patterns for Python version specifications
static EXACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^==(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static CARET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static COMPATIBLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~=(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>=(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<=(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<(\d+(?:\.\d+)*(?:[a-zA-Z]\d+)?)$").unwrap());
static RANGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[<>=!]+\d+(?:\.\d+)*,\s*[<>=!]+\d+(?:\.\d+)*$").unwrap());
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*$|^\d+(?:\.\d+)*\.\*$").unwrap());

impl VersionParser for PythonVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for exact version (==1.2.3)
        if let Some(caps) = EXACT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("=="),
            );
        }

        // Check for caret (^1.2.3) - Poetry style
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Check for tilde (~1.2.3) - Poetry style
        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~"),
            );
        }

        // Check for compatible release (~=1.2.3) - PEP 440
        if let Some(caps) = COMPATIBLE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~="),
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

        // Check for range (>=1.0,<2.0)
        if RANGE_RE.is_match(trimmed) {
            // Extract the first version from range for reference
            let first_version = trimmed
                .split(',')
                .next()
                .and_then(|s| {
                    s.trim_start_matches(|c: char| !c.is_ascii_digit())
                        .split(|c: char| !c.is_ascii_digit() && c != '.')
                        .next()
                })
                .unwrap_or("")
                .to_string();
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                first_version,
            ));
        }

        // Check for wildcard (*, 1.*)
        if WILDCARD_RE.is_match(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                trimmed,
            ));
        }

        None
    }

    fn language(&self) -> Language {
        Language::Python
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        PythonVersionParser.parse(version)
    }

    #[test]
    fn test_parse_exact() {
        let spec = parse("==1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("==".to_string()));
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_exact_with_prerelease() {
        let spec = parse("==1.2.3a1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3a1");
    }

    #[test]
    fn test_parse_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("^".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_tilde() {
        let spec = parse("~1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_compatible_release() {
        let spec = parse("~=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~=".to_string()));
    }

    #[test]
    fn test_parse_greater_or_equal() {
        let spec = parse(">=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some(">=".to_string()));
    }

    #[test]
    fn test_parse_greater() {
        let spec = parse(">1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Greater);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some(">".to_string()));
    }

    #[test]
    fn test_parse_less_or_equal() {
        let spec = parse("<=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::LessOrEqual);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_less() {
        let spec = parse("<1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Less);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_range() {
        let spec = parse(">=1.0,<2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, ">=1.0,<2.0");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_range_with_space() {
        let spec = parse(">=1.0, <2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_wildcard_star() {
        let spec = parse("*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_wildcard_partial() {
        let spec = parse("1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse("").is_none());
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse("not-a-version").is_none());
    }

    #[test]
    fn test_format_updated_exact() {
        let spec = parse("==1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "==2.0.0");
    }

    #[test]
    fn test_format_updated_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "^2.0.0");
    }

    #[test]
    fn test_format_updated_gte() {
        let spec = parse(">=1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), ">=2.0.0");
    }

    #[test]
    fn test_language() {
        assert_eq!(PythonVersionParser.language(), Language::Python);
    }

    #[test]
    fn test_parse_range_extracts_first_version() {
        let spec = parse(">=3.5.0,<4.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, ">=3.5.0,<4.0.0");
        assert_eq!(spec.version, "3.5.0"); // 最初のバージョンが抽出される
    }

    #[test]
    fn test_format_updated_range_has_no_prefix_suffix() {
        // Range型はprefix/suffixが設定されていないため、
        // format_updatedは新バージョンのみを返す（これは期待される動作）
        // 呼び出し側でRange型を特別に処理する必要がある
        let spec = parse(">=3.5.0,<4.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert!(spec.prefix.is_none());
        assert!(spec.suffix.is_none());
        // 注意: Range型のformat_updatedは不完全な結果を返すため、
        // 呼び出し側（pyproject_toml.rs）でRange型を特別に処理している
        assert_eq!(spec.format_updated("4.0.0"), "4.0.0");
    }
}
