//! Node.js (npm/yarn/pnpm) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `1.2.3`
//! - Caret: `^1.2.3`, `^1.2`, `^1`
//! - Tilde: `~1.2.3`, `~1.2`, `~1`
//! - 比較演算子: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`
//! - ワイルドカード: `1.x`, `1.2.*`, `^1.x`, `~1.2.x` (caret/tilde + x-range)
//! - レンジ: `>=1.0.0 <2.0.0`, `1.0.0 - 2.0.0`, `^1 || ^2`

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Node.js バージョン指定パーサ
pub struct NodeVersionParser;

/// 部分指定を完全な semver に正規化する。例: `2` -> `2.0.0`
fn normalize_version(version: &str) -> String {
    let version = version.strip_prefix('v').unwrap_or(version);

    // プレリリースやビルドメタデータは後ろに戻す
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

// Node.js のバージョン指定用正規表現
// ^2 や ~2.1 のような部分指定も受け付ける
// node-semver の prerelease (`-...`) と build metadata (`+...`) は同時に出現することがある
// (例: `1.2.3-rc.1+build123`)
static CARET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\^\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static TILDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^~\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static GTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^>=\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static GT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^>\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static LTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<=\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static LT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static EQUAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^=\s*(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap()
});
static EXACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?)$").unwrap());
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:v?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,2}|\*)$").unwrap());
// `^1.x` / `~1.2.*` のような caret/tilde + x-range。
// `^1` / `^1.2.3` は先に CARET_RE / TILDE_RE が消費するため、ここに到達するのは
// ワイルドカード文字 (x/X/*) を含むものだけ。
static CARET_TILDE_WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\^~]\s*v?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,2}$").unwrap());
static RANGE_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v?\d+(?:\.\d+){0,2}(?:-[\w.-]+)?(?:\+[\w.-]+)?").unwrap());

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

fn is_fully_floating_wildcard(raw: &str) -> bool {
    !raw.chars().any(|ch| ch.is_ascii_digit())
}

impl VersionParser for NodeVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // 比較演算子を複数含む複合レンジを先に判定する
        if has_compound_range(trimmed) || has_multi_comparator(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // Caret レンジ
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Tilde レンジ
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

        if let Some(caps) = EQUAL_RE.captures(trimmed) {
            let version = normalize_version(caps.get(1)?.as_str());
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("="),
            );
        }

        // `*` は完全な浮動指定なので更新対象にしない
        if matches!(trimmed, "*" | "x" | "X") {
            return None;
        }

        // `^1.x` / `~1.2.*` のような caret/tilde + x-range は、演算子を保持しつつ
        // ワイルドカードとして形を保って更新する (例: `^1.x` → `^2.x`)。
        // ワイルドカード文字を含む場合のみ対象とし、`^1`/`^1.2.3` は手前の
        // CARET_RE/TILDE_RE で既に処理されている。
        if CARET_TILDE_WILDCARD_RE.is_match(trimmed)
            && (trimmed.contains('x') || trimmed.contains('X') || trimmed.contains('*'))
        {
            // `^x` / `~*` のような完全浮動指定は意味を変えないため更新対象にしない
            // (version が空の Wildcard を作ると phantom update の原因になる)
            if is_fully_floating_wildcard(trimmed) {
                return None;
            }
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // `1.x` や `1.2.*` は形を保ったまま更新する
        if WILDCARD_RE.is_match(trimmed)
            && (trimmed.contains('x') || trimmed.contains('X') || trimmed.contains('*'))
        {
            if is_fully_floating_wildcard(trimmed) {
                return None;
            }
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                extract_first_version(trimmed),
            ));
        }

        // 固定バージョンまたは部分指定
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

        // npm dist-tag (`latest` / `beta` 等) やその他解釈できない文字列は更新対象にしない
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
        assert!(parse("*").is_none());
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
    fn test_parse_wildcard_full_tuple() {
        let spec = parse("1.x.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "2.x.x");
    }

    #[test]
    fn test_parse_fully_floating_multi_segment_wildcard() {
        assert!(parse("x.x").is_none());
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
        assert!(parse("latest").is_none());
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
    fn test_format_updated_wildcard_x() {
        let spec = parse("1.x").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "2.x");
    }

    #[test]
    fn test_format_updated_wildcard_minor() {
        let spec = parse("1.2.*").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "2.3.*");
    }

    #[test]
    fn test_format_updated_wildcard_v_prefix() {
        let spec = parse("v1.*").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "v2.*");
    }

    #[test]
    fn test_language() {
        assert_eq!(NodeVersionParser.language(), Language::Node);
    }

    /// v接頭辞付き完全バージョンが Exact として分類される
    #[test]
    fn test_parse_v_prefix_exact() {
        let spec = parse("v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.raw, "v1.2.3");
    }

    /// v接頭辞付き部分指定が Range として分類される（npm の部分指定扱い）
    #[test]
    fn test_parse_v_prefix_partial() {
        let spec = parse("v1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.0");
        assert_eq!(spec.raw, "v1.2");
    }

    /// npm エイリアス形式 ("npm:@scope/pkg@^1.0") はパース対象外で None を返す
    #[test]
    fn test_parse_npm_alias() {
        assert!(parse("npm:@scope/pkg@^1.0").is_none());
    }

    /// プロトコル参照 (git://, file:, workspace:*) はパース対象外で None を返す
    #[test]
    fn test_parse_protocol_refs_skipped() {
        assert!(parse("git://github.com/user/repo.git").is_none());
        assert!(parse("file:../local-pkg").is_none());
        assert!(parse("workspace:*").is_none());
    }

    /// v接頭辞付き Exact バージョンの更新フォーマット（v は正規化時に除去される）
    #[test]
    fn test_format_updated_v_prefix_exact() {
        let spec = parse("v1.2.3").unwrap();
        // v接頭辞は normalize_version で除去されるため、更新後は v なしになる
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    // --- エッジケース追加テスト ---

    #[test]
    fn test_parse_equal_with_space() {
        // = 接頭辞とスペース付き
        let spec = parse("= 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("=".to_string()));
    }

    #[test]
    fn test_parse_caret_zero_zero_three() {
        // ^0.0.3 は >=0.0.3 <0.0.4 と同値（パッチ固定）
        let spec = parse("^0.0.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "0.0.3");
    }

    #[test]
    fn test_parse_caret_zero() {
        // ^0 は >=0.0.0 <1.0.0 と同値
        let spec = parse("^0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "0.0.0");
    }

    #[test]
    fn test_parse_tilde_zero() {
        // ~0.0.3 は >=0.0.3 <0.1.0 と同値
        let spec = parse("~0.0.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "0.0.3");
    }

    #[test]
    fn test_parse_exact_with_build_metadata() {
        // ビルドメタデータ付きバージョンは Exact として分類
        let spec = parse("1.0.0+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0+build");
    }

    #[test]
    fn test_parse_caret_with_build_metadata() {
        let spec = parse("^1.0.0+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.0.0+build");
    }

    #[test]
    fn test_parse_caret_with_prerelease_and_build() {
        // node-semver では prerelease + build metadata の組み合わせも有効
        let spec = parse("^1.2.3-rc.1+build123").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3-rc.1+build123");
    }

    #[test]
    fn test_parse_exact_with_prerelease_and_build() {
        // prerelease + build metadata 同時指定の固定バージョン
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
    fn test_parse_tilde_with_prerelease_and_build() {
        let spec = parse("~1.2.3-rc.1+build").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3-rc.1+build");
    }

    #[test]
    fn test_format_updated_caret_prerelease_and_build_to_stable() {
        // prerelease+build から安定版への更新が正しく書き出される
        let spec = parse("^1.2.3-rc.1+build123").unwrap();
        assert_eq!(spec.format_updated("1.2.3"), "^1.2.3");
    }

    #[test]
    fn test_parse_partial_major_only_as_range() {
        // 単一セグメント（例: "2"）は部分指定として Range に分類
        let spec = parse("2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "2.0.0");
    }

    #[test]
    fn test_parse_gte_with_space() {
        let spec = parse(">= 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_tag_next() {
        // dist-tag は更新対象外
        assert!(parse("next").is_none());
    }

    #[test]
    fn test_parse_tag_beta() {
        assert!(parse("beta").is_none());
    }

    #[test]
    fn test_parse_tag_canary() {
        assert!(parse("canary").is_none());
    }

    #[test]
    fn test_parse_equal_v_prefix() {
        // `=v1.2.3` のような `=` + `v` 接頭辞の組み合わせ
        let spec = parse("=v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("=".to_string()));
    }

    #[test]
    fn test_parse_gte_v_prefix() {
        // `>=v1.2.3` のような `>=` + `v` 接頭辞の組み合わせ
        let spec = parse(">=v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some(">=".to_string()));
    }

    #[test]
    fn test_parse_caret_v_prefix() {
        // `^v1.2.3` のような `^` + `v` 接頭辞の組み合わせ
        let spec = parse("^v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("^".to_string()));
    }

    #[test]
    fn test_parse_tilde_v_prefix() {
        // `~v1.2.3` のような `~` + `v` 接頭辞の組み合わせ
        let spec = parse("~v1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix, Some("~".to_string()));
    }

    #[test]
    fn test_parse_hyphen_range_partial_upper() {
        // npm の hyphen range で右辺が部分指定の場合
        // (`1.2.3 - 2.3` は `>=1.2.3 <2.4.0-0` と同値)
        let spec = parse("1.2.3 - 2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_caret_wildcard() {
        // caret + x-range は Wildcard として認識する (以前は None で無言ドロップしていた)
        let spec = parse("^1.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.raw, "^1.x");
    }

    #[test]
    fn test_parse_tilde_wildcard() {
        let spec = parse("~1.2.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.raw, "~1.2.x");
    }

    #[test]
    fn test_parse_caret_wildcard_star() {
        let spec = parse("^1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.raw, "^1.2.*");
    }

    #[test]
    fn test_format_updated_caret_wildcard() {
        // caret + x-range は演算子とワイルドカードの形を保って更新する
        let spec = parse("^1.x").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "^2.x");
    }

    #[test]
    fn test_format_updated_tilde_wildcard_minor() {
        let spec = parse("~1.2.x").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "~2.3.x");
    }

    #[test]
    fn test_format_updated_caret_wildcard_star() {
        let spec = parse("^1.2.*").unwrap();
        assert_eq!(spec.format_updated("2.3.4"), "^2.3.*");
    }

    #[test]
    fn test_parse_caret_plain_still_caret() {
        // ワイルドカードを含まない `^1` は従来どおり Caret のまま (Wildcard にしない)
        let spec = parse("^1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_caret_fully_floating_wildcard_is_none() {
        // (回帰) `^x` は完全浮動指定なので version="" の Wildcard にせず None を返す
        // (phantom update 防止)
        assert!(parse("^x").is_none());
    }

    #[test]
    fn test_parse_tilde_fully_floating_wildcard_is_none() {
        // (回帰) `~*` も完全浮動指定として更新対象にしない
        assert!(parse("~*").is_none());
        assert!(parse("^x.x").is_none());
    }

    #[test]
    fn test_parse_compound_with_three_comparators() {
        // 3つの演算子を含む compound range
        let spec = parse(">=1.0.0 <2.0.0 !=1.5.0");
        // node-semver では != は無効だがパース上は Range として扱われる
        if let Some(s) = spec {
            assert_eq!(s.kind, VersionSpecKind::Range);
        }
    }

    #[test]
    fn test_parse_compound_extra_whitespace() {
        // 演算子の間に複数の空白が含まれた compound range も Range として認識する
        let spec = parse(">=1.0.0    <2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_caret_zero_zero_partial() {
        // ^0.0 は >=0.0.0 <0.1.0 と同値
        let spec = parse("^0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "0.0.0");
    }
}
