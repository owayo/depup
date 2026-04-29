//! Ruby (RubyGems/Bundler) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `= 1.2.3`, `1.2.3`
//! - ペシミスティック制約: `~> 1.2`, `~> 1.2.3`
//! - 比較演算子: `>=`, `<`, `>`, `<=`
//! - 複合制約: `>= 1.0, < 2.0`, `>= 1.0 < 2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Ruby バージョン指定パーサ
pub struct RubyVersionParser;

// Ruby バージョン指定用正規表現
// 演算子とバージョンの間のスペースは省略可
// プレリリース識別子は複数のドット/ハイフン区切りに対応する
// 例: `1.0.0.pre.1`, `7.0.0.alpha.2`, `1.2.3-beta.1`

// ペシミスティック制約: ~> 1.2 or ~> 1.2.3 or ~> 1.0.0.pre.1
static PESSIMISTIC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~>\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// = 接頭辞付き固定: = 1.2.3
static EXACT_EQ_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// 以上: >= 1.2.3
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>=\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// より大きい: > 1.2.3
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^>\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// 以下: <= 1.2.3
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<=\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// より小さい: < 1.2.3
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());
static NOT_EQUAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^!=\s*(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// 裸のバージョン (固定): 1.2.3
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*)$").unwrap());

// 複合制約パターン (個別パースの前に検出する)
static COMPOUND_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",").unwrap());
static COMPOUND_SPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[<>=~!].*\s+[<>=~!]").unwrap());
static VERSION_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+(?:\.\d+)*(?:[-.][A-Za-z0-9]+)*").unwrap());

fn extract_first_version(raw: &str) -> String {
    VERSION_TOKEN_RE
        .find(raw)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

impl RubyVersionParser {
    /// 単一のバージョン制約を解釈する（複合制約ではない）
    fn parse_single(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // ペシミスティック制約 (~> 1.2.3)
        if let Some(caps) = PESSIMISTIC_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~> "),
            );
        }

        // = 接頭辞付き固定 (= 1.2.3)
        if let Some(caps) = EXACT_EQ_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("= "),
            );
        }

        // 以上 (>= 1.2.3)
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">= "),
            );
        }

        // より大きい (> 1.2.3)
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix("> "),
            );
        }

        // 以下 (<= 1.2.3)
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<= "),
            );
        }

        // より小さい (< 1.2.3)
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("< "),
            );
        }

        if let Some(caps) = NOT_EQUAL_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            return Some(VersionSpec::new(VersionSpecKind::Range, trimmed, version));
        }

        // 裸のバージョン (1.2.3) — 固定扱い
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

        // 複合制約 (>= 1.0, < 2.0)
        if COMPOUND_RE.is_match(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        if COMPOUND_SPACE_RE.is_match(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // 単一制約として解釈する
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

    // ペシミスティック制約のテスト
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

    // 固定バージョンのテスト
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

    // 比較演算子のテスト
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

    // 複合制約のテスト
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

    #[test]
    fn test_parse_compound_space_without_comma() {
        let spec = parse(">= 1.0 < 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_not_equal() {
        let spec = parse("!= 1.5.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.5.0");
    }

    // 境界ケースのテスト
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

    // 更新書式のテスト
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

    // language のテスト
    #[test]
    fn test_ruby_parser_language() {
        let parser = RubyVersionParser;
        assert_eq!(parser.language(), Language::Ruby);
    }

    // 複数セグメントバージョンのテスト
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

    #[test]
    fn test_parse_pessimistic_single_segment() {
        // ~> 0 は >= 0, < 1
        let spec = parse("~> 0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_compound_space_separator() {
        // スペース区切りの複合制約
        let spec = parse(">= 1.0 < 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_no_version() {
        // バージョンなし（空文字列）
        assert!(parse("").is_none());
    }

    #[test]
    fn test_parse_prerelease_version() {
        // プレリリースバージョン
        let spec = parse("1.2.3.pre").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.pre");
    }

    #[test]
    fn test_format_updated_pessimistic_minor() {
        let spec = parse("~> 1.2").unwrap();
        assert_eq!(spec.format_updated("2.0"), "~> 2.0");
    }

    #[test]
    fn test_format_updated_not_equal() {
        let spec = parse("!= 1.5.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_pessimistic_four_segments() {
        // ~> 1.2.3.4 のような4セグメントペシミスティック制約
        let spec = parse("~> 1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3.4");
    }

    #[test]
    fn test_parse_zero_padded_version() {
        // ゼロパディングされたバージョン — 正規表現は数字列を許容するためパースされる
        let spec = parse("01.02.03").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "01.02.03");
    }

    #[test]
    fn test_parse_prerelease_hyphen_format() {
        // ハイフン区切りのプレリリースバージョン
        let spec = parse("1.2.3-beta1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-beta1");
    }

    #[test]
    fn test_parse_pessimistic_major_only_nonzero() {
        // ~> 2 のようなメジャーのみペシミスティック制約
        let spec = parse("~> 2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "2");
    }

    #[test]
    fn test_format_updated_four_segments() {
        // 4セグメントバージョンの更新フォーマット
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.format_updated("1.2.3.5"), "1.2.3.5");
    }

    // RubyGems / Bundler でよく使われるドット区切りプレリリースを許容する
    #[test]
    fn test_parse_dotted_prerelease_multiple_segments() {
        // Rails の慣用的な書き方: `7.0.0.alpha.2`, `1.0.0.pre.1`
        let spec = parse("7.0.0.alpha.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "7.0.0.alpha.2");
    }

    #[test]
    fn test_parse_dotted_prerelease_pre_dot_one() {
        let spec = parse("1.0.0.pre.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0.pre.1");
    }

    #[test]
    fn test_parse_pessimistic_dotted_prerelease() {
        let spec = parse("~> 7.0.0.alpha.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "7.0.0.alpha.2");
        assert_eq!(spec.prefix, Some("~> ".to_string()));
    }

    #[test]
    fn test_parse_gte_dotted_prerelease() {
        let spec = parse(">= 1.0.0.pre.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0.0.pre.1");
    }

    #[test]
    fn test_parse_exact_eq_dotted_prerelease() {
        let spec = parse("= 7.0.0.beta.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "7.0.0.beta.1");
    }

    #[test]
    fn test_format_updated_dotted_prerelease() {
        let spec = parse("~> 7.0.0.alpha.2").unwrap();
        assert_eq!(spec.format_updated("7.0.0.alpha.3"), "~> 7.0.0.alpha.3");
    }
}
