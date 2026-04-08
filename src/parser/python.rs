//! Python (pip/poetry) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `==1.2.3`
//! - Caret: `^1.2.3` (Poetry)
//! - Tilde: `~1.2.3`, `~=1.2.3`
//! - 比較演算子: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`, `!=1.2.3`, `===1.2.3`
//! - ワイルドカード: `1.*`
//! - レンジ: `>=1.0,<2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Python バージョン指定パーサ
pub struct PythonVersionParser;

// Python のバージョン指定用正規表現
static CARET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^\s*([0-9A-Za-z][0-9A-Za-z._!+-]*(?:\*)?)$").unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~\s*([0-9A-Za-z][0-9A-Za-z._!+-]*(?:\*)?)$").unwrap());
static OP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(===|==|!=|~=|>=|<=|>|<)\s*([0-9A-Za-z][0-9A-Za-z._!+-]*(?:\*)?)$").unwrap()
});
static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
            r"^(?:\s*(?:===|==|!=|~=|>=|<=|>|<)\s*[0-9A-Za-z][0-9A-Za-z._!+-]*(?:\*)?\s*,)+\s*(?:===|==|!=|~=|>=|<=|>|<)\s*[0-9A-Za-z][0-9A-Za-z._!+-]*(?:\*)?\s*$",
        )
        .unwrap()
});
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*$|^\d+(?:\.\d+)*\.\*$").unwrap());
static VERSION_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(?:\.\d+)*").unwrap());

fn normalize_for_compare(version: &str) -> String {
    let mut s = version.trim();
    if let Some((_, rest)) = s.split_once('!') {
        s = rest;
    }
    let mut buf = String::new();
    let mut seen_digit = false;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            seen_digit = true;
            buf.push(ch);
        } else if seen_digit && ch == '.' {
            buf.push(ch);
        } else if seen_digit {
            break;
        }
    }
    buf.trim_matches('.').to_string()
}

fn extract_first_version(raw: &str) -> String {
    VERSION_TOKEN_RE
        .find(raw)
        .map(|m| normalize_for_compare(m.as_str()))
        .unwrap_or_default()
}

impl VersionParser for PythonVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Poetry の Caret
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = normalize_for_compare(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = normalize_for_compare(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~"),
            );
        }

        if let Some(caps) = OP_RE.captures(trimmed) {
            let op = caps.get(1)?.as_str();
            let raw_version = caps.get(2)?.as_str();
            let normalized = normalize_for_compare(raw_version);

            return Some(match op {
                "===" | "==" if !raw_version.ends_with(".*") => {
                    VersionSpec::new(VersionSpecKind::Exact, trimmed, normalized).with_prefix(op)
                }
                "~=" => {
                    VersionSpec::new(VersionSpecKind::Tilde, trimmed, normalized).with_prefix("~=")
                }
                ">=" => VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, normalized)
                    .with_prefix(">="),
                ">" => {
                    VersionSpec::new(VersionSpecKind::Greater, trimmed, normalized).with_prefix(">")
                }
                "<=" => VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, normalized)
                    .with_prefix("<="),
                "<" => {
                    VersionSpec::new(VersionSpecKind::Less, trimmed, normalized).with_prefix("<")
                }
                _ => VersionSpec::new(VersionSpecKind::Range, trimmed, normalized),
            });
        }

        // 空白を含むレンジも許容する
        if RANGE_RE.is_match(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                extract_first_version(trimmed),
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
        assert_eq!(spec.version, "1.2.3");
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
    }

    #[test]
    fn test_parse_compatible_release() {
        let spec = parse("~=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~=".to_string()));
    }

    #[test]
    fn test_parse_arbitrary_equality() {
        let spec = parse("===v1.2-custom").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.prefix, Some("===".to_string()));
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
        let spec = parse(">= 1.0, < 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_not_equal_as_range() {
        let spec = parse("!=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.3");
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
    fn test_format_updated_wildcard_partial() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "2.3.*");
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
    fn test_format_updated_range_keeps_upper_bound() {
        // Range 型は上限制約を残したまま下限だけを更新する
        let spec = parse(">=3.5.0,<4.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert!(spec.prefix.is_none());
        assert!(spec.suffix.is_none());
        assert_eq!(spec.format_updated("3.9.1"), ">=3.9.1,<4.0.0");
    }

    #[test]
    fn test_parse_pep440_epoch() {
        let spec = parse(">=1!2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "2.3");
    }

    #[test]
    fn test_parse_exact_wildcard_as_range() {
        // ==1.2.* は >=1.2.0, <1.3.0 と同値なので Range として扱う
        let spec = parse("==1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_not_equal_wildcard_as_range() {
        // !=1.2.* は Range として扱う
        let spec = parse("!=1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_compatible_release_two_part() {
        // ~=1.2 は >=1.2, <2.0 と同値
        let spec = parse("~=1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.prefix, Some("~=".to_string()));
    }

    #[test]
    fn test_parse_wildcard_partial_minor() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_local_version() {
        // PEP 440 ローカルバージョン指定: '+' 以降はローカルセグメントとして扱われる
        let spec = parse("==1.0+local1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some("==".to_string()));
    }

    #[test]
    fn test_parse_post_release() {
        // PEP 440 ポストリリースバージョン: '.post1' は数値部分のみ抽出される
        let spec = parse("==1.0.post1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some("==".to_string()));
    }

    #[test]
    fn test_parse_not_equal_wildcard() {
        // ワイルドカード除外制約: '!=1.2.*' はレンジとして扱われる
        let spec = parse("!=1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_range_single_segment_version() {
        // 単一セグメントバージョンのレンジ指定: `>=3,<4`
        let spec = parse(">=3,<4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "3");
    }

    #[test]
    fn test_parse_single_segment_gte() {
        // 単一セグメントの以上制約: `>=3`
        let spec = parse(">=3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "3");
    }
}
