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
// 演算子後の空白を許容する（Cargo は `>= 1.2.3` のようなスペース付き指定を受け入れる）
// SemVer の prerelease (`-...`) と build metadata (`+...`) は同時指定を許容する
// (例: `1.2.3-rc.1+build123`)。ビルドメタデータはバージョン比較時には無視されるが、
// マニフェスト上の表記はそのまま保持する。
static EXACT_PINNED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static CARET_EXPLICIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>=\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<=\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<\s*([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
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
    fn test_parse_bare_with_build_metadata() {
        // ビルドメタデータ付きの裸バージョンは Cargo では caret 扱い
        let spec = parse("1.0.0+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.0.0+build");
    }

    #[test]
    fn test_parse_caret_with_build_metadata() {
        let spec = parse("^1.0.0+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.0.0+build");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    #[test]
    fn test_parse_exact_pinned_with_build_metadata() {
        let spec = parse("=1.0.0+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0+build");
    }

    #[test]
    fn test_parse_caret_with_prerelease_and_build() {
        // prerelease + build metadata の組み合わせ
        let spec = parse("^1.2.3-rc.1+build123").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3-rc.1+build123");
    }

    #[test]
    fn test_parse_gte_with_build_metadata() {
        let spec = parse(">=1.0.0+sha.abc123").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0.0+sha.abc123");
        assert_eq!(spec.prefix, Some(">=".to_string()));
    }

    #[test]
    fn test_format_updated_caret_with_build_metadata_to_stable() {
        // build metadata 付きから安定版への更新
        let spec = parse("^1.2.3-rc.1+build123").unwrap();
        assert_eq!(spec.format_updated("1.2.3"), "^1.2.3");
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

    #[test]
    fn test_parse_gte_with_space() {
        // Cargo は演算子後のスペースを許容する
        let spec = parse(">= 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some(">=".to_string()));
    }

    #[test]
    fn test_parse_lt_with_space() {
        let spec = parse("< 2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Less);
        assert_eq!(spec.version, "2.0.0");
    }

    #[test]
    fn test_parse_exact_pinned_with_space() {
        let spec = parse("= 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_tilde_with_space() {
        let spec = parse("~ 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_caret_with_space() {
        let spec = parse("^ 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
    }

    // --- 部分バージョン指定のエッジケース ---

    #[test]
    fn test_parse_caret_partial_major_minor() {
        // ^1.2 は >=1.2.0, <2.0.0 と同値
        let spec = parse("^1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    #[test]
    fn test_parse_caret_partial_major_only() {
        // ^1 は >=1.0.0, <2.0.0 と同値
        let spec = parse("^1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    #[test]
    fn test_parse_caret_zero_zero() {
        // ^0.0 は >=0.0.0, <0.1.0 と同値
        let spec = parse("^0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "0.0");
    }

    #[test]
    fn test_parse_caret_zero_zero_three() {
        // ^0.0.3 は >=0.0.3, <0.0.4 と同値（パッチ固定）
        let spec = parse("^0.0.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "0.0.3");
    }

    #[test]
    fn test_parse_tilde_partial_major_minor() {
        // ~1.2 は >=1.2.0, <1.3.0 と同値
        let spec = parse("~1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.prefix, Some("~".to_string()));
    }

    #[test]
    fn test_parse_tilde_partial_major_only() {
        // ~1 は >=1.0.0, <2.0.0 と同値
        let spec = parse("~1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1");
        assert_eq!(spec.prefix, Some("~".to_string()));
    }

    #[test]
    fn test_parse_range_single_segment_version() {
        // 単一セグメントバージョンのレンジ: >=1, <2
        let spec = parse(">=1, <2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_gt_with_space() {
        let spec = parse("> 1.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Greater);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_lte_with_space() {
        let spec = parse("<= 2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::LessOrEqual);
        assert_eq!(spec.version, "2.0.0");
    }
}
