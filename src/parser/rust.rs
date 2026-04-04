//! Rust (Cargo) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `=1.2.3`
//! - Caret: `1.2.3`, `^1.2.3`
//! - Tilde: `~1.2.3`
//! - 比較演算子: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`
//! - ワイルドカード: `1.*`
//! - レンジ: `>=1.0, <2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Rust/Cargo バージョン指定パーサ
pub struct RustVersionParser;

// Rust のバージョン指定用正規表現
static EXACT_PINNED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static CARET_EXPLICIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>=([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<=([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([\d]+(?:\.[\d]+)*(?:-[\w.]+)?)$").unwrap());
static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[<>=]+\s*[\d]+(?:\.[\d]+)*\s*,\s*[<>=]+\s*[\d]+(?:\.[\d]+)*$").unwrap()
});
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*$|^[\d]+(?:\.[\d]+)*\.\*$").unwrap());

impl VersionParser for RustVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // 固定バージョン
        if let Some(caps) = EXACT_PINNED_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("="),
            );
        }

        // 明示的 Caret
        if let Some(caps) = CARET_EXPLICIT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Tilde
        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~"),
            );
        }

        // 以上
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">="),
            );
        }

        // より大きい
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix(">"),
            );
        }

        // 以下
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<="),
            );
        }

        // より小さい
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("<"),
            );
        }

        // レンジ
        if RANGE_RE.is_match(trimmed) {
            // 比較基準として先頭のバージョンを残す
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

        // `*` は完全な浮動指定なので更新対象にしない
        if trimmed == "*" {
            return None;
        }

        // `1.*` は形を保ったまま更新する
        if WILDCARD_RE.is_match(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                trimmed,
            ));
        }

        // 裸のバージョンは Cargo では Caret 扱い
        if let Some(caps) = BARE_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(VersionSpec::new(VersionSpecKind::Caret, trimmed, version));
        }

        None
    }

    fn language(&self) -> Language {
        Language::Rust
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        RustVersionParser.parse(version)
    }

    #[test]
    fn test_parse_exact_pinned() {
        let spec = parse("=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("=".to_string()));
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_bare_version() {
        // Cargo では裸のバージョンは caret (^1.2.3) 扱い
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_bare_version_with_prerelease() {
        let spec = parse("1.2.3-beta.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3-beta.1");
    }

    #[test]
    fn test_parse_explicit_caret() {
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
        let spec = parse(">=1.0, <2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, ">=1.0, <2.0");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_range_no_space() {
        let spec = parse(">=1.0,<2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_range_with_spaces_after_operators() {
        // Cargo はオペレータ後のスペースを許容する
        let spec = parse(">= 1.0, < 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_partial_version_major_minor() {
        // Cargo では部分バージョンは caret 相当
        let spec = parse("1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_partial_version_major_only() {
        let spec = parse("1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_build_metadata_not_supported() {
        // ビルドメタデータ付きバージョンは Cargo.toml では使用されない
        assert!(parse("1.0.0+build").is_none());
    }

    #[test]
    fn test_parse_wildcard_star() {
        assert!(parse("*").is_none());
    }

    #[test]
    fn test_parse_wildcard_partial() {
        let spec = parse("1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_wildcard_minor() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
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
    fn test_format_updated_exact_pinned() {
        let spec = parse("=1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "=2.0.0");
    }

    #[test]
    fn test_format_updated_bare() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
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
    fn test_format_updated_wildcard_partial() {
        let spec = parse("1.*").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "2.*");
    }

    #[test]
    fn test_format_updated_wildcard_minor() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "2.3.*");
    }

    #[test]
    fn test_language() {
        assert_eq!(RustVersionParser.language(), Language::Rust);
    }
}
