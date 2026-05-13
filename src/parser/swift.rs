//! Swift (SPM) のバージョン指定パーサ。
//!
//! プレーンな semver 文字列 (例: "1.2.3", "1.0.0-beta.1") を解釈する。
//! バージョン制約の種類 (from:, exact:, .upToNextMinor, ranges) は
//! マニフェストパーサ側で決定する。
//!
//! SPM の Version 型は semver 2.0.0 に準拠しており、プレリリース識別子と
//! ビルドメタデータを許容する (Apple Developer Documentation の Version 参照)。

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Swift バージョン指定パーサ
pub struct SwiftVersionParser;

/// 裸の semver バージョン: 1.2.3 / 1.2 / 1.0.0-beta.1 / 1.0.0+build / 1.0.0-rc.1+build
/// SPM は semver 2.0.0 準拠のためプレリリース識別子とビルドメタデータを許容する
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());

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
    fn test_parse_accepts_prerelease_suffix() {
        // SPM は semver 2.0.0 準拠なのでプレリリース識別子付きバージョンも受理する
        let spec = parse("1.2.3-beta").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-beta");
    }

    #[test]
    fn test_parse_rejects_v_prefix() {
        // Swift のバージョン文字列に v 接頭辞は付かない
        assert!(parse("v1.2.3").is_none());
    }

    #[test]
    fn test_parse_two_segment_version() {
        // 2セグメントバージョンが正しくパースされる
        let spec = parse("1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_leading_zeros_accepted() {
        // 先頭ゼロ付きバージョンも数字列として許容される
        let spec = parse("01.02.03").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "01.02.03");
    }

    #[test]
    fn test_parse_prerelease_alpha_accepted() {
        // alpha プレリリースも semver 2.0.0 仕様により受理される
        let spec = parse("1.0.0-alpha").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0-alpha");
    }

    #[test]
    fn test_parse_prerelease_rc_accepted() {
        // RC プレリリースも semver 2.0.0 仕様により受理される
        let spec = parse("2.0.0-rc.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "2.0.0-rc.1");
    }

    #[test]
    fn test_parse_prerelease_dotted_segments() {
        // 複数セグメントのプレリリース識別子
        let spec = parse("1.0.0-beta.1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0-beta.1.2");
    }

    #[test]
    fn test_parse_build_metadata() {
        // ビルドメタデータ付きバージョン
        let spec = parse("1.0.0+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0+build");
    }

    #[test]
    fn test_parse_prerelease_and_build_metadata() {
        // プレリリース + ビルドメタデータの組み合わせ
        let spec = parse("1.0.0-rc.1+sha.abc123").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0-rc.1+sha.abc123");
    }

    #[test]
    fn test_format_updated_prerelease_to_stable() {
        // プレリリース版から安定版への更新
        let spec = parse("1.0.0-beta.1").unwrap();
        assert_eq!(spec.format_updated("1.0.0"), "1.0.0");
    }

    #[test]
    fn test_format_updated_two_segment() {
        // 2セグメントバージョンの更新フォーマット
        let spec = parse("1.0").unwrap();
        assert_eq!(spec.format_updated("2.1"), "2.1");
    }
}
