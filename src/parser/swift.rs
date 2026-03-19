//! Swift (SPM) のバージョン指定パーサ。
//!
//! プレーンな semver 文字列 (例: "1.2.3") を解釈する。
//! バージョン制約の種類 (from:, exact:, .upToNextMinor, ranges) は
//! マニフェストパーサ側で決定する。

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Swift バージョン指定パーサ
pub struct SwiftVersionParser;

/// 裸の semver バージョン: 1.2.3 or 1.2
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)$").unwrap());

impl VersionParser for SwiftVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        if let Some(caps) = BARE_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(VersionSpec::new(VersionSpecKind::Exact, trimmed, version));
        }

        None
    }

    fn language(&self) -> Language {
        Language::Swift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        SwiftVersionParser.parse(version)
    }

    #[test]
    fn test_parse_semver() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
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
    fn test_parse_with_whitespace() {
        let spec = parse("  1.2.3  ").unwrap();
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_swift_parser_language() {
        let parser = SwiftVersionParser;
        assert_eq!(parser.language(), Language::Swift);
    }

    #[test]
    fn test_format_updated() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_parse_four_part_version() {
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.4");
    }

    #[test]
    fn test_parse_rejects_prerelease_suffix() {
        // Swift パーサはプレーンな数値バージョンのみ受理する
        assert!(parse("1.2.3-beta").is_none());
    }

    #[test]
    fn test_parse_rejects_v_prefix() {
        // Swift のバージョン文字列に v 接頭辞は付かない
        assert!(parse("v1.2.3").is_none());
    }
}
