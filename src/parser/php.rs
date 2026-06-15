//! PHP のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `1.2.3`
//! - Caret: `^1.2.3`
//! - Tilde: `~1.2.3`
//! - 比較演算子: `>=`, `<`, `>`, `<=`
//! - 複合制約: `>=1.0 <2.0`, `^1 || ^2`, `1.0 - 2.0`
//! - ワイルドカード: `1.2.*`, `1.x`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// PHP バージョン指定パーサ
pub struct PhpVersionParser;

// PHP のバージョン指定用正規表現
// Composer は semver の prerelease (`-...`) と build metadata (`+...`) の同時指定を許可する
// (例: `1.2.3-rc.1+build123`)
// また Composer/Packagist は 1〜4 セグメントの数値バージョン (`1.0.0.0` 等) を valid 扱いするため
// `(?:\.\d+){0,3}` で 4 セグメントまで許容する (composer/semver の VersionParser に準拠)

// Caret: ^1.2.3 / ^1.2.3.4
static CARET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\^\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});

// Tilde: ~1.2.3 / ~1.2.3.4
static TILDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^~\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});

// 以上: >=1.2.3 / >=1.2.3.4
static GTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^>=\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});

// より大きい: >1.2.3 / >1.2.3.4
static GT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^>\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});

// 以下: <=1.2.3 / <=1.2.3.4
static LTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<=\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});

// より小さい: <1.2.3 / <1.2.3.4
static LT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static NOT_EQUAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^!=\s*(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});

// ワイルドカード: 1.2.*, 1.x, 1.2.3.*, *, V1.* (composer/semver は v/V を大小問わず許容)
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[vV]?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,3}|\*)$").unwrap());

// 固定バージョン: 1.2.3 / 1.2.3.4
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());

// 複合制約用パターン
static COMPOUND_OR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\|\|?").unwrap());
static HYPHEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s-\s").unwrap());

// 空白区切りの複合制約。
// 例: ">=1.0 <2.0", "^1.0 !=1.5"
// 単一制約の ">=1.0" や ">= 1.0.0" は含めない
static COMPOUND_SPACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 先頭の制約に続いて、空白区切りで別の演算子付き制約が続く形だけを拾う
    Regex::new(r"^[<>=^~!].*\s+[<>=^~!]").unwrap()
});
static COMPOUND_COMMA_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",").unwrap());
static VERSION_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v?\d+(?:\.\d+){0,3}(?:-[\w.-]+)?(?:\+[\w.-]+)?").unwrap());

fn normalize_version(version: &str) -> String {
    version.strip_prefix('v').unwrap_or(version).to_string()
}

fn extract_first_version(raw: &str) -> String {
    VERSION_TOKEN_RE
        .find(raw)
        .map(|m| normalize_version(m.as_str()))
        .unwrap_or_default()
}

impl PhpVersionParser {
    /// 単一制約を解釈する
    fn parse_single(&self, version_str: &str) -> Option<VersionSpec> {
        // インラインエイリアス (`1.0.0 as 1.1.0` / `1.0.0@dev as 1.1.0`) は別バージョンへの
        // エイリアス宣言。レジストリ最新版で上書きすると宣言が壊れるためスキップする。
        if version_str.contains(" as ") {
            return None;
        }
        let trimmed = version_str.trim().split('@').next().unwrap_or("").trim();

        if trimmed.is_empty() {
            return None;
        }

        // Caret
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Tilde
        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix("~"),
            );
        }

        // 以上
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">="),
            );
        }

        // より大きい
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix(">"),
            );
        }

        // 以下
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<="),
            );
        }

        // より小さい
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("<"),
            );
        }

        if let Some(caps) = NOT_EQUAL_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(VersionSpec::new(VersionSpecKind::Range, trimmed, version));
        }

        // `*` は完全な浮動指定なので更新対象にしない
        if matches!(trimmed, "*" | "x" | "X") {
            return None;
        }

        // `1.2.*` や `1.x` は形を保ったまま更新する
        if WILDCARD_RE.is_match(trimmed)
            && (trimmed.contains('x') || trimmed.contains('X') || trimmed.contains('*'))
        {
            let version = extract_first_version(trimmed);
            let mut spec = VersionSpec::new(VersionSpecKind::Wildcard, trimmed, version);
            if trimmed.ends_with(".*") {
                spec = spec.with_suffix(".*");
            }
            return Some(spec);
        }

        // 固定バージョン
        if let Some(caps) = BARE_VERSION_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(VersionSpec::new(VersionSpecKind::Exact, trimmed, version));
        }

        None
    }
}

impl VersionParser for PhpVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // OR を含む複合制約
        if COMPOUND_OR_RE.is_match(trimmed)
            || HYPHEN_RANGE_RE.is_match(trimmed)
            || COMPOUND_COMMA_RE.is_match(trimmed)
        {
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // 空白区切りの複合制約
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
        Language::Php
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        PhpVersionParser.parse(version)
    }

    // Caret のテスト
    #[test]
    fn test_parse_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("^".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_caret_minor() {
        let spec = parse("^1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_caret_major() {
        let spec = parse("^1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1");
    }

    // Tilde のテスト
    #[test]
    fn test_parse_tilde() {
        let spec = parse("~1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_tilde_minor() {
        let spec = parse("~1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2");
    }

    // 比較演算子のテスト
    #[test]
    fn test_parse_greater_or_equal() {
        let spec = parse(">=1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some(">=".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_greater_or_equal_with_space() {
        let spec = parse(">= 1.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_greater() {
        let spec = parse(">1.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Greater);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some(">".to_string()));
    }

    #[test]
    fn test_parse_less_or_equal() {
        let spec = parse("<=2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::LessOrEqual);
        assert_eq!(spec.version, "2.0");
        assert_eq!(spec.prefix, Some("<=".to_string()));
    }

    #[test]
    fn test_parse_less() {
        let spec = parse("<2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Less);
        assert_eq!(spec.version, "2.0");
        assert_eq!(spec.prefix, Some("<".to_string()));
    }

    // ワイルドカードのテスト
    #[test]
    fn test_parse_wildcard() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.suffix, Some(".*".to_string()));
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_wildcard_major() {
        let spec = parse("1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_wildcard_x() {
        let spec = parse("1.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_bare_wildcard_is_skipped() {
        assert!(parse("*").is_none());
    }

    // 固定バージョンのテスト
    #[test]
    fn test_parse_exact() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.prefix.is_none());
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_exact_major_minor() {
        let spec = parse("1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2");
    }

    // 複合制約のテスト
    #[test]
    fn test_parse_compound_space() {
        let spec = parse(">=1.0 <2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.raw, ">=1.0 <2.0");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_compound_space_multiple() {
        let spec = parse(">=1.0 <2.0 !=1.5.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_compound_or() {
        let spec = parse("^1.0 || ^2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.raw, "^1.0 || ^2.0");
    }

    #[test]
    fn test_parse_compound_pipe() {
        let spec = parse("^1.0 | ^2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_hyphen_range() {
        let spec = parse("1.0 - 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_not_equal() {
        let spec = parse("!=1.5.0").unwrap();
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
        let spec = parse("  ^1.2.3  ").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
    }

    // 更新書式のテスト
    #[test]
    fn test_format_updated_caret() {
        let spec = parse("^1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.3.0"), "^1.3.0");
    }

    #[test]
    fn test_format_updated_tilde() {
        let spec = parse("~1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.2.5"), "~1.2.5");
    }

    #[test]
    fn test_format_updated_exact() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    #[test]
    fn test_format_updated_wildcard() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.format_updated("1.3.4"), "1.3.*");
    }

    #[test]
    fn test_format_updated_gte() {
        let spec = parse(">=1.0").unwrap();
        assert_eq!(spec.format_updated("2.0"), ">=2.0");
    }

    // language のテスト
    #[test]
    fn test_php_parser_language() {
        let parser = PhpVersionParser;
        assert_eq!(parser.language(), Language::Php);
    }

    #[test]
    fn test_parse_stability_flag_stripped() {
        // @dev, @alpha 等の安定性フラグは除去される
        let spec = parse("^1.0@dev").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_dev_branch_not_parseable() {
        // dev-main ブランチ参照はパースされない
        assert!(parse("dev-main").is_none());
    }

    #[test]
    fn test_parse_v_prefix_stripped() {
        // v接頭辞は正規化で除去される
        let spec = parse("v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_x_notation_uppercase() {
        let spec = parse("1.X").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_compound_comma() {
        // カンマ区切りの複合制約
        let spec = parse(">=1.0,<2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_dev_branch_with_hash() {
        // dev-main#abc1234 はパースされない
        assert!(parse("dev-main#abc1234").is_none());
    }

    #[test]
    fn test_parse_x_dev_branch() {
        // 1.x-dev はバージョン風ブランチ名
        assert!(parse("1.x-dev").is_none());
    }

    #[test]
    fn test_parse_inline_alias() {
        // 1.0.0 as 1.1.0 はインラインエイリアス — 複合とみなしてRange
        let spec = parse("1.0.0 as 1.1.0");
        // "as" は演算子として認識されないため、パースできなくてよい
        assert!(spec.is_none() || spec.unwrap().kind == VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_stability_flag_alpha() {
        let spec = parse("^1.0@alpha").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_caret_with_prerelease() {
        let spec = parse("^1.2.3-beta.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3-beta.1");
    }

    #[test]
    fn test_parse_caret_with_prerelease_and_build() {
        // semver の prerelease + build metadata 両方を含むケース
        let spec = parse("^1.2.3-rc.1+build123").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3-rc.1+build123");
    }

    #[test]
    fn test_parse_exact_with_prerelease_and_build() {
        let spec = parse("1.2.3-beta.1+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-beta.1+build");
    }

    #[test]
    fn test_parse_gte_with_prerelease_and_build() {
        let spec = parse(">=1.2.3-alpha.1+meta").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3-alpha.1+meta");
    }

    #[test]
    fn test_format_updated_wildcard_x() {
        let spec = parse("1.x").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "2.x");
    }

    #[test]
    fn test_parse_v_prefix_with_caret() {
        // v接頭辞付きの caret 指定でもバージョンが正規化される
        let spec = parse("^v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_stability_flag_dev_bare() {
        // 裸のバージョンに @dev が付いた場合もフラグが除去される
        let spec = parse("1.0@dev").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_wildcard_x_minor() {
        // 1.2.x 形式のワイルドカード
        let spec = parse("1.2.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_hyphen_range_with_patch() {
        // パッチバージョン付きのハイフンレンジ
        let spec = parse("1.0.0 - 2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_single_pipe_or() {
        // シングルパイプの OR 制約
        let spec = parse("^1 | ^2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_double_pipe_or_with_tilde() {
        // ダブルパイプの OR 制約 (tilde)
        let spec = parse("~1.0 || ~2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_stability_flag_stable() {
        // @stable フラグも除去される
        let spec = parse("^2.0@stable").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "2.0");
    }

    #[test]
    fn test_parse_compound_caret_and_not_equal() {
        // Composer の compound: caret と != の組み合わせ
        let spec = parse("^1.0 !=1.5.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_caret_with_v_prefix_preserves_v_in_format() {
        // ^v1.2.3 → format_updated は v 接頭辞を保持しないが、version は正規化される
        let spec = parse("^v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
        // 更新後は元の v 接頭辞は失われる（Caret は prefix のみ保持）
        assert_eq!(spec.format_updated("2.0.0"), "^2.0.0");
    }

    #[test]
    fn test_format_updated_wildcard_with_v_prefix_preserves_v() {
        // v1.* の wildcard 更新では v 接頭辞が保持される
        let spec = parse("v1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "v2.*");
    }

    // --- Composer 4セグメントバージョン対応テスト ---

    // Composer/Packagist は composer/semver の VersionParser に従って
    // 4 セグメントまでの数値バージョン (`1.0.0.0` 等) を valid 扱いする

    #[test]
    fn test_parse_exact_four_segments() {
        // 4セグメント完全バージョン
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.4");
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_caret_four_segments() {
        // 4セグメント caret
        let spec = parse("^1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3.4");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    #[test]
    fn test_parse_tilde_four_segments() {
        // 4セグメント tilde
        let spec = parse("~1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3.4");
    }

    #[test]
    fn test_parse_gte_four_segments() {
        // 4セグメント以上
        let spec = parse(">=1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3.4");
    }

    #[test]
    fn test_parse_wildcard_four_segments() {
        // 4セグメント末尾ワイルドカード
        let spec = parse("1.2.3.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_four_segments_with_prerelease() {
        // 4セグメント + プレリリース
        let spec = parse("1.0.0.0-beta1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0.0-beta1");
    }

    #[test]
    fn test_format_updated_exact_four_segments() {
        // 4セグメントバージョンの更新
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.format_updated("1.2.3.5"), "1.2.3.5");
    }

    #[test]
    fn test_format_updated_caret_four_segments() {
        // 4セグメント caret の更新
        let spec = parse("^1.0.0.0").unwrap();
        assert_eq!(spec.format_updated("1.0.0.1"), "^1.0.0.1");
    }

    #[test]
    fn test_parse_rejects_five_segments() {
        // 5セグメントは composer/semver の仕様上 invalid
        assert!(parse("1.2.3.4.5").is_none());
    }

    #[test]
    fn test_parse_wildcard_uppercase_v() {
        // composer/semver は v/V を大小問わず許容するため V1.* も Wildcard として解釈する
        let spec = parse("V1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.5.0"), "V2.*");

        let spec_x = parse("V1.x").unwrap();
        assert_eq!(spec_x.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_inline_alias_skipped() {
        // インラインエイリアス (`as`) はエイリアス宣言を壊さないようスキップする
        assert!(parse("1.0.0@dev as 1.1.0").is_none());
        assert!(parse("1.0.0 as 1.1.0").is_none());
    }
}
