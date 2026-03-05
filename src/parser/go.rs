//! Go (go mod) version specification parser
//!
//! Handles version formats:
//! - Semantic version: `v1.2.3`
//! - Prerelease: `v1.2.3-beta.1`
//! - Pseudo-version: `v0.0.0-20210101120000-abcdef123456`
//! - Extended pseudo-version: `v1.2.4-0.20240101010101-abcdef123456`
//!
//! Note: Go modules use `// pinned` comment to indicate pinned versions,
//! which is handled at the manifest parsing level, not here.

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Go module version specification parser
pub struct GoVersionParser;

// Regex patterns for Go version specifications
// Standard semver: v1.2.3, v1.2.3-beta.1, v1.2.3+meta
static SEMVER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(\+[0-9A-Za-z.-]+)?$").unwrap()
});

// Pseudo-version:
// - v0.0.0-20210101120000-abcdef123456
// - v1.2.3-0.20210101120000-abcdef123456
// - v1.2.4-beta.0.20210101120000-abcdef123456
static PSEUDO_VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+\.)?\d{14}-[a-f0-9]{12})(\+incompatible)?$")
        .unwrap()
});

impl VersionParser for GoVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Check for pseudo-version (commit-based)
        // These are treated as exact/pinned since they reference a specific commit
        if let Some(caps) = PSEUDO_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            let mut spec =
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("v");
            if caps.get(2).is_some() {
                spec = spec.with_suffix("+incompatible");
            }
            return Some(spec);
        }

        // Check for standard semver (v1.2.3)
        // In Go, all versions are effectively exact/pinned to what's in go.mod
        // The concept of ranges doesn't exist in go.mod
        if let Some(caps) = SEMVER_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            let mut spec =
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("v");
            if let Some(sfx) = caps.get(2) {
                spec = spec.with_suffix(sfx.as_str());
            }
            return Some(spec);
        }

        None
    }

    fn language(&self) -> Language {
        Language::Go
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        GoVersionParser.parse(version)
    }

    #[test]
    fn test_parse_semver() {
        let spec = parse("v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("v".to_string()));
        assert_eq!(spec.raw, "v1.2.3");
    }

    #[test]
    fn test_parse_semver_with_prerelease() {
        let spec = parse("v1.2.3-beta.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-beta.1");
        assert_eq!(spec.prefix, Some("v".to_string()));
    }

    #[test]
    fn test_parse_semver_with_rc() {
        let spec = parse("v1.2.3-rc1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-rc1");
    }

    #[test]
    fn test_parse_pseudo_version() {
        let spec = parse("v0.0.0-20210101120000-abcdef123456").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "0.0.0-20210101120000-abcdef123456");
        assert_eq!(spec.prefix, Some("v".to_string()));
        // Pseudo-versions should be treated as pinned
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_incompatible() {
        let spec = parse("v2.0.0+incompatible").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "2.0.0");
        assert_eq!(spec.prefix, Some("v".to_string()));
        assert_eq!(spec.suffix, Some("+incompatible".to_string()));
    }

    #[test]
    fn test_parse_extended_pseudo_version() {
        let spec = parse("v1.2.4-0.20210101120000-abcdef123456").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.4-0.20210101120000-abcdef123456");
    }

    #[test]
    fn test_parse_extended_pseudo_with_prerelease() {
        let spec = parse("v1.2.4-beta.0.20210101120000-abcdef123456").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.4-beta.0.20210101120000-abcdef123456");
    }

    #[test]
    fn test_parse_major_version() {
        let spec = parse("v2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "2.0.0");
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
    fn test_parse_invalid_no_v_prefix() {
        // Go versions must have v prefix
        assert!(parse("1.2.3").is_none());
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse("not-a-version").is_none());
    }

    #[test]
    fn test_format_updated_semver() {
        let spec = parse("v1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "v2.0.0");
    }

    #[test]
    fn test_format_updated_incompatible() {
        let spec = parse("v2.0.0+incompatible").unwrap();
        assert_eq!(spec.format_updated("3.0.0"), "v3.0.0+incompatible");
    }

    #[test]
    fn test_is_pinned() {
        // All Go versions are effectively pinned in go.mod
        let spec = parse("v1.2.3").unwrap();
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_language() {
        assert_eq!(GoVersionParser.language(), Language::Go);
    }

    #[test]
    fn test_parse_extended_pseudo_version_with_prefix() {
        // リリースタグ後のコミット
        let spec = parse("v1.2.4-0.20210101120000-abcdef123456").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.prefix, Some("v".to_string()));
    }

    #[test]
    fn test_parse_prerelease_extended_pseudo_version() {
        // プレリリースタグ後のコミット
        let spec = parse("v1.2.4-beta.0.20210101120000-abcdef123456").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_build_metadata() {
        // ビルドメタデータ (+incompatible 以外)
        let spec = parse("v2.0.0+meta").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
    }
}
