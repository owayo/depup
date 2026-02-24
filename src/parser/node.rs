//! Node.js (npm/yarn/pnpm) version specification parser
//!
//! Handles version formats:
//! - Exact: `1.2.3`
//! - Caret: `^1.2.3`, `^1.2`, `^1`
//! - Tilde: `~1.2.3`, `~1.2`, `~1`
//! - Comparison: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`
//! - Wildcard: `*`, `1.x`, `1.2.*`
//! - Range: `>=1.0.0 <2.0.0`, `1.0.0 - 2.0.0`, `^1 || ^2`
//! - Tag: `latest`, `next`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Node.js version specification parser
pub struct NodeVersionParser;

/// Normalize partial version to full semver (e.g., "2" -> "2.0.0", "2.1" -> "2.1.0")
fn normalize_version(version: &str) -> String {
    let version = version.strip_prefix('v').unwrap_or(version);

    // Handle prerelease/build suffix
    let (base, suffix) = if let Some(idx) = version.find(['-', '+']) {
        (&version[..idx], Some(&version[idx..]))
    } else {
        (version, None)
    };

    let parts: Vec<&str> = base.split('.').collect();
    let normalized = match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => base.to_string(),
    };

    match suffix {
        Some(s) => format!("{}{}", normalized, s),
        None => normalized,
    }
}

// Regex patterns for Node.js version specifications
// These patterns accept partial versions (e.g., ^2, ~2.1)
static CARET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>=\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<=\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static EQUAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=\s*(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static EXACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?)$").unwrap());
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:v?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,2}|\*)$").unwrap());
static RANGE_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v?\d+(?:\.\d+){0,2}(?:[-+][\w.-]+)?").unwrap());
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:latest|next|canary|beta|alpha|rc|stable|experimental)$").unwrap()
});

fn extract_first_version(raw: &str) -> String {
    RANGE_TOKEN_RE
        .find(raw)
        .map(|m| normalize_version(m.as_str()))
        .unwrap_or_default()
}

fn has_compound_range(raw: &str) -> bool {
    raw.contains("||") || raw.contains(" - ")
}

fn has_multi_comparator(raw: &str) -> bool {
    let mut count = 0usize;
    for token in raw.split_whitespace() {
        if token.starts_with(">=")
            || token.starts_with('>')
            || token.starts_with("<=")
            || token.starts_with('<')
            || token.starts_with('^')
            || token.starts_with('~')
        {
            count += 1;
        }
    }
    count >= 2
}

impl VersionParser for NodeVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for comparator-based compound ranges first
        if has_compound_range(trimmed) || has_multi_comparator(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // Check for caret range (^1.2.3, ^1.2, ^1)
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Check for tilde range (~1.2.3, ~1.2, ~1)
        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~"),
            );
        }

        // Check for greater than or equal (>=1.2.3, >=1.2, >=1)
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">="),
            );
        }

        // Check for greater than (>1.2.3, >1.2, >1)
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix(">"),
            );
        }

        // Check for less than or equal (<=1.2.3, <=1.2, <=1)
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<="),
            );
        }

        // Check for less than (<1.2.3, <1.2, <1)
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("<"),
            );
        }

        if let Some(caps) = EQUAL_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("="),
            );
        }

        // Check for wildcard (*, 1.x, 1.2.*)
        if WILDCARD_RE.is_match(trimmed)
            && (trimmed.contains('x') || trimmed.contains('X') || trimmed.contains('*'))
        {
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // Check for exact version / partial version (1.2.3, 1.2, 1)
        if let Some(caps) = EXACT_RE.captures(trimmed) {
            let raw_version = caps.get(1)?.as_str();
            let normalized = normalize_version(raw_version);
            if raw_version.matches('.').count() >= 2 {
                return Some(VersionSpec::new(
                    VersionSpecKind::Exact,
                    trimmed,
                    normalized,
                ));
            }
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                normalized,
            ));
        }

        // npm dist-tags (latest, next, canary) behave like moving targets.
        if TAG_RE.is_match(trimmed) {
            return Some(VersionSpec::new(VersionSpecKind::Wildcard, trimmed, ""));
        }

        None
    }

    fn language(&self) -> Language {
        Language::Node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        NodeVersionParser.parse(version)
    }

    #[test]
    fn test_parse_exact() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.raw, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_exact_with_prerelease() {
        let spec = parse("1.2.3-beta.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-beta.1");
        assert!(spec.is_pinned());
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
    fn test_parse_caret_with_prerelease() {
        let spec = parse("^1.2.3-alpha").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3-alpha");
    }

    #[test]
    fn test_parse_caret_major_only() {
        let spec = parse("^2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "2.0.0");
        assert_eq!(spec.raw, "^2");
        assert_eq!(spec.prefix, Some("^".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_caret_major_minor() {
        let spec = parse("^2.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "2.1.0");
        assert_eq!(spec.raw, "^2.1");
        assert_eq!(spec.prefix, Some("^".to_string()));
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
    fn test_parse_tilde_major_only() {
        let spec = parse("~2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "2.0.0");
        assert_eq!(spec.raw, "~2");
    }

    #[test]
    fn test_parse_tilde_major_minor() {
        let spec = parse("~2.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "2.1.0");
        assert_eq!(spec.raw, "~2.1");
    }

    #[test]
    fn test_parse_greater_or_equal() {
        let spec = parse(">=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some(">=".to_string()));
        assert!(!spec.is_pinned());
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
        assert_eq!(spec.prefix, Some("<=".to_string()));
    }

    #[test]
    fn test_parse_less() {
        let spec = parse("<1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Less);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("<".to_string()));
    }

    #[test]
    fn test_parse_range() {
        let spec = parse(">=1.0.0 <2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, ">=1.0.0 <2.0.0");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_hyphen_range() {
        let spec = parse("1.0.0 - 2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, "1.0.0 - 2.0.0");
    }

    #[test]
    fn test_parse_or_range() {
        let spec = parse("^1.0.0 || ^2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_space_comparators_range() {
        let spec = parse(">=1.0.0 <2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_wildcard_star() {
        let spec = parse("*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_wildcard_x() {
        let spec = parse("1.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_wildcard_minor() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_exact_with_equal() {
        let spec = parse("=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.prefix, Some("=".to_string()));
    }

    #[test]
    fn test_parse_partial_bare_as_range() {
        let spec = parse("1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_tag_latest() {
        let spec = parse("latest").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "");
    }

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
    fn test_format_updated_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "^2.0.0");
    }

    #[test]
    fn test_format_updated_tilde() {
        let spec = parse("~1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.3.0"), "~1.3.0");
    }

    #[test]
    fn test_format_updated_gte() {
        let spec = parse(">=1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), ">=2.0.0");
    }

    #[test]
    fn test_format_updated_exact() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_caret_partial() {
        let spec = parse("^2").unwrap();
        assert_eq!(spec.format_updated("2.10.0"), "^2.10.0");
    }

    #[test]
    fn test_format_updated_tilde_partial() {
        let spec = parse("~2.1").unwrap();
        assert_eq!(spec.format_updated("2.2.0"), "~2.2.0");
    }

    #[test]
    fn test_language() {
        assert_eq!(NodeVersionParser.language(), Language::Node);
    }
}
