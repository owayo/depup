//! Node.js (npm/yarn/pnpm) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `1.2.3`
//! - キャレット: `^1.2.3`, `^1.2`, `^1`
//! - チルダ: `~1.2.3`, `~1.2`, `~1`
//! - 比較演算子: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`
//! - ワイルドカード: `1.x`, `1.2.*`, `^1.x`, `~1.2.x` (caret/tilde + x-range)
//! - レンジ: `>=1.0.0 <2.0.0`, `1.2 <2.0.0`, `1.0.0 - 2.0.0`, `^1 || ^2`

use crate::domain::{Language, VersionSpec, VersionSpecKind, range_lower_bound_version};
use crate::parser::{VersionParser, anchored_op_pattern, is_fully_floating_wildcard};
use regex::Regex;
use semver::Version;
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

fn has_leading_zero_numeric_prerelease_identifier(version: &str) -> bool {
    let Some(prerelease_start) = version.find('-') else {
        return false;
    };
    let prerelease_and_build = &version[prerelease_start + 1..];
    let prerelease = prerelease_and_build
        .split_once('+')
        .map(|(pre, _)| pre)
        .unwrap_or(prerelease_and_build);

    prerelease.split('.').any(|identifier| {
        identifier.len() > 1
            && identifier.starts_with('0')
            && identifier.chars().all(|ch| ch.is_ascii_digit())
    })
}

fn normalize_valid_version(version: &str) -> Option<String> {
    let normalized = normalize_version(version);
    Version::parse(&normalized).ok()?;
    if has_leading_zero_numeric_prerelease_identifier(&normalized) {
        return None;
    }
    Some(normalized)
}

// Node.js のバージョン指定用正規表現
// ^2 や ~2.1 のような部分指定も受け付ける
// node-semver の prerelease (`-...`) と build metadata (`+...`) は同時に出現することがある
// (例: `1.2.3-rc.1+build123`)
const NODE_VERSION_PATTERN: &str = r"v?\d+(?:\.\d+){0,2}(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?";
const NODE_VERSION_OR_X_PATTERN: &str = r"v?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,2}(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?";
static CARET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"\^", NODE_VERSION_PATTERN)).unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"~>?", NODE_VERSION_PATTERN)).unwrap());
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r">=", NODE_VERSION_PATTERN)).unwrap());
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r">", NODE_VERSION_PATTERN)).unwrap());
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"<=", NODE_VERSION_PATTERN)).unwrap());
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"<", NODE_VERSION_PATTERN)).unwrap());
static EQUAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"=", NODE_VERSION_PATTERN)).unwrap());
static EXACT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^({NODE_VERSION_PATTERN})$")).unwrap());
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:v?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,2}|\*)$").unwrap());
// `^1.x` / `~1.2.*` のような caret/tilde + x-range。
// `^1` / `^1.2.3` は先に CARET_RE / TILDE_RE が消費するため、ここに到達するのは
// ワイルドカード文字 (x/X/*) を含むものだけ。
static CARET_TILDE_WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\^~]\s*v?(?:\d+|[xX*])(?:\.(?:\d+|[xX*])){0,2}$").unwrap());
static RANGE_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(NODE_VERSION_PATTERN).unwrap());
// node-semver の hyphen range は両端が裸の version (partial version も可) でなければならない。
// `^1.0 - 2.0` / `~1.0 - 2.0` / `>=1.0 - 2.0` のような演算子付き端点は node-semver 仕様上 invalid。
// 単に `" - "` で contains 判定すると過受理して壊れた制約を Range として書き換える可能性があるため、
// 両端が `vN(.N){0,2}(-pre)?(+meta)?` の数値トークンに限定して全体一致を要求する。
static HYPHEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^\s*{NODE_VERSION_PATTERN}\s+-\s+{NODE_VERSION_PATTERN}\s*$"
    ))
    .unwrap()
});
static SIMPLE_RANGE_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:>=|>|<=|<|=|\^|~)?\s*{NODE_VERSION_OR_X_PATTERN}$"
    ))
    .unwrap()
});

fn extract_first_version(raw: &str) -> String {
    RANGE_TOKEN_RE
        .find(raw)
        .and_then(|m| normalize_valid_version(m.as_str()))
        .unwrap_or_default()
}

fn range_versions_are_valid(raw: &str) -> bool {
    RANGE_TOKEN_RE
        .find_iter(raw)
        .all(|m| normalize_valid_version(m.as_str()).is_some())
}

fn has_hyphen_range(raw: &str) -> bool {
    HYPHEN_RANGE_RE.is_match(raw) && range_versions_are_valid(raw)
}

fn normalize_comparator_tokens(raw: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut iter = raw.split_whitespace();

    while let Some(token) = iter.next() {
        if matches!(token, ">=" | ">" | "<=" | "<" | "=" | "^" | "~") {
            let version = iter.next()?;
            tokens.push(format!("{token}{version}"));
        } else {
            tokens.push(token.to_string());
        }
    }

    Some(tokens)
}

fn comparator_set_is_valid(raw: &str) -> bool {
    let Some(tokens) = normalize_comparator_tokens(raw) else {
        return false;
    };

    !tokens.is_empty() && tokens.iter().all(|token| is_simple_range_token(token))
}

fn has_or_range(raw: &str) -> bool {
    raw.contains("||")
        && raw
            .split("||")
            .all(|part| comparator_set_is_valid(part.trim()))
}

fn has_compound_range(raw: &str) -> bool {
    has_or_range(raw) || has_hyphen_range(raw)
}

fn is_simple_range_token(token: &str) -> bool {
    let token = token.trim();
    if !SIMPLE_RANGE_TOKEN_RE.is_match(token) {
        return false;
    }

    let body = token
        .strip_prefix(">=")
        .or_else(|| token.strip_prefix("<="))
        .or_else(|| token.strip_prefix('>'))
        .or_else(|| token.strip_prefix('<'))
        .or_else(|| token.strip_prefix('='))
        .or_else(|| token.strip_prefix('^'))
        .or_else(|| token.strip_prefix('~'))
        .unwrap_or(token)
        .trim();
    let body = body.strip_prefix(['v', 'V']).unwrap_or(body);

    if body.contains(['x', 'X', '*']) {
        !is_fully_floating_wildcard(body) && !has_digit_after_wildcard(body)
    } else {
        normalize_valid_version(body).is_some()
    }
}

fn has_multi_comparator(raw: &str) -> bool {
    let Some(tokens) = normalize_comparator_tokens(raw) else {
        return false;
    };

    if !tokens.iter().all(|token| is_simple_range_token(token)) {
        return false;
    }

    let mut comparator_count = 0usize;
    let mut simple_count = 0usize;
    let mut has_bound_operator = false;

    for token in tokens {
        if token.starts_with(">=")
            || token.starts_with('>')
            || token.starts_with("<=")
            || token.starts_with('<')
            || token.starts_with('^')
            || token.starts_with('~')
        {
            comparator_count += 1;
        }
        if token.starts_with(">=")
            || token.starts_with('>')
            || token.starts_with("<=")
            || token.starts_with('<')
        {
            has_bound_operator = true;
        }
        if is_simple_range_token(&token) {
            simple_count += 1;
        }
    }

    comparator_count >= 2 || (has_bound_operator && simple_count >= 2)
}

/// ワイルドカードトークン列 (`1.x.3` のような形) で、いったんワイルドカード文字 (`x`/`X`/`*`)
/// が現れた後に数値セグメントが続いていないかを検査する。node-semver / semver の規約では、
/// あるセグメントがワイルドカードなら以降の minor/patch もワイルドカードでなければならない
/// (`1.x.3` は invalid)。先頭の `^` / `~` / `=` / `v` / `V` は事前に剥がしてから呼ぶ。
fn has_digit_after_wildcard(body: &str) -> bool {
    let mut seen_wildcard = false;
    for segment in body.split('.') {
        if segment.contains(['x', 'X', '*']) {
            seen_wildcard = true;
        } else if seen_wildcard && segment.chars().all(|ch| ch.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// ワイルドカード指定の共通末尾処理。caret/tilde 付き (`^1.x`) と裸 (`1.x`) の
/// 両経路が共用する。`body` は呼び出し側で `^` / `~` などの演算子を剥がした後の
/// 文字列を渡す (先頭の `v` / `V` はここで剥がす)。
fn build_wildcard_spec(trimmed: &str, body: &str) -> Option<VersionSpec> {
    // `^x` / `~*` / `x.x` のような完全浮動指定は意味を変えないため更新対象にしない
    // (version が空の Wildcard を作ると phantom update の原因になる)
    if is_fully_floating_wildcard(trimmed) {
        return None;
    }
    // `1.x.3` / `^x.0.0` のように一度ワイルドカードが出た後に数値セグメントが続く形は
    // semver / node-semver の x-range 規約上 invalid (Rust の semver crate も同様に拒否)。
    // 誤って受理して `version="0.0.0"` のような捏造値で比較されるのを防ぐためここで弾く。
    let body = body.strip_prefix(['v', 'V']).unwrap_or(body);
    if has_digit_after_wildcard(body) {
        return None;
    }
    Some(VersionSpec::new(
        VersionSpecKind::Wildcard,
        trimmed,
        extract_first_version(trimmed),
    ))
}

impl VersionParser for NodeVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // ` - ` を含むが両端が裸 version になっていない `^1.0 - 2.0` / `>=1.0 - 2.0` 等は
        // node-semver 仕様上 invalid。has_multi_comparator が `-` を独立 token として
        // 数えないため、`>=1.0 - 2.0` のような形式は comparator set として誤受理する。
        // 先んじてここで拒否することで過受理を防ぐ。
        if trimmed.contains(" - ") && !HYPHEN_RANGE_RE.is_match(trimmed) {
            return None;
        }

        // 比較演算子を複数含む複合レンジを先に判定する
        if has_compound_range(trimmed) || has_multi_comparator(trimmed) {
            // 比較基準は記述順に依存せず包含下限を採用する
            // (`<1.5 >=1.2.2` でも下限 `1.2.2` を基準にし、更新の取りこぼしを防ぐ)。
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                range_lower_bound_version(trimmed)
                    .and_then(|v| normalize_valid_version(&v))
                    .unwrap_or_else(|| extract_first_version(trimmed)),
            ));
        }

        // Caret レンジ
        if let Some(caps) = CARET_RE.captures(trimmed) {
            let version = normalize_valid_version(caps.get(1)?.as_str())?;
            return Some(
                VersionSpec::new(VersionSpecKind::Caret, trimmed, version).with_prefix("^"),
            );
        }

        // Tilde レンジ
        if let Some(caps) = TILDE_RE.captures(trimmed) {
            let version = normalize_valid_version(caps.get(1)?.as_str())?;
            let prefix = if trimmed.starts_with("~>") { "~>" } else { "~" };
            return Some(
                VersionSpec::new(VersionSpecKind::Tilde, trimmed, version).with_prefix(prefix),
            );
        }

        // 以上
        if let Some(caps) = GTE_RE.captures(trimmed) {
            let version = normalize_valid_version(caps.get(1)?.as_str())?;
            return Some(
                VersionSpec::new(VersionSpecKind::GreaterOrEqual, trimmed, version)
                    .with_prefix(">="),
            );
        }

        // より大きい
        if let Some(caps) = GT_RE.captures(trimmed) {
            let version = normalize_valid_version(caps.get(1)?.as_str())?;
            return Some(
                VersionSpec::new(VersionSpecKind::Greater, trimmed, version).with_prefix(">"),
            );
        }

        // 以下
        if let Some(caps) = LTE_RE.captures(trimmed) {
            let version = normalize_valid_version(caps.get(1)?.as_str())?;
            return Some(
                VersionSpec::new(VersionSpecKind::LessOrEqual, trimmed, version).with_prefix("<="),
            );
        }

        // より小さい
        if let Some(caps) = LT_RE.captures(trimmed) {
            let version = normalize_valid_version(caps.get(1)?.as_str())?;
            return Some(
                VersionSpec::new(VersionSpecKind::Less, trimmed, version).with_prefix("<"),
            );
        }

        if let Some(caps) = EQUAL_RE.captures(trimmed) {
            let raw_version = caps.get(1)?.as_str();
            let version = normalize_valid_version(raw_version)?;
            if raw_version.matches('.').count() >= 2 {
                return Some(
                    VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_prefix("="),
                );
            }
            return Some(VersionSpec::new(VersionSpecKind::Range, trimmed, version));
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
            // ワイルドカード位置の検査は `^` / `~` を剥がした本体に対して行う
            let body = trimmed.trim_start_matches(['^', '~']).trim_start();
            return build_wildcard_spec(trimmed, body);
        }

        // `1.x` や `1.2.*` は形を保ったまま更新する
        if WILDCARD_RE.is_match(trimmed)
            && (trimmed.contains('x') || trimmed.contains('X') || trimmed.contains('*'))
        {
            return build_wildcard_spec(trimmed, trimmed);
        }

        // 固定バージョンまたは部分指定
        if let Some(caps) = EXACT_RE.captures(trimmed) {
            let raw_version = caps.get(1)?.as_str();
            let normalized = normalize_valid_version(raw_version)?;
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
    fn test_parse_range_upper_bound_first_uses_lower_bound() {
        // 回帰: 上限が先に書かれた comparator set (`<1.5 >=1.2.2`) でも、
        // 比較基準 version は包含下限 `1.2.2` を採用する。以前は先頭トークン (=上限 1.5)
        // を基準にして judge が AlreadyLatest と誤判定し、有効な更新を取りこぼしていた。
        let a = parse("<1.5 >=1.2.2").unwrap();
        assert_eq!(a.kind, VersionSpecKind::Range);
        assert_eq!(a.version, "1.2.2");
        // 下限が先の従来ケースは回帰しないこと
        let b = parse(">=1.2.2 <1.5").unwrap();
        assert_eq!(b.version, "1.2.2");
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
    fn test_parse_equal_partial_as_range() {
        // node-semver では `=1.2` / `=1` も partial comparator であり、
        // `1.2.x` / `1.x` と同じレンジとして扱われる。
        let spec = parse("=1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.0");
        assert_eq!(spec.format_updated("2.3.4"), "=2.3");

        let major = parse("=1").unwrap();
        assert_eq!(major.kind, VersionSpecKind::Range);
        assert_eq!(major.version, "1.0.0");
        assert_eq!(major.format_updated("2.3.4"), "=2");
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
    fn test_parse_rejects_invalid_semver_identifiers() {
        // SemVer の prerelease/build 識別子は ASCII 英数字とハイフンのみ。
        // 空識別子や prerelease の数値識別子の先頭ゼロも invalid として弾く。
        assert!(parse("1.2.3-rc_1").is_none());
        assert!(parse("^1.2.3-alpha..1").is_none());
        assert!(parse("~1.2.3+build_1").is_none());
        assert!(parse(">=1.2.3-01").is_none());
        assert!(parse(">=1.2.3-alpha.01").is_none());
        assert!(parse("1.2.3 - 2.0.0-rc_1").is_none());
        assert!(parse(">= 1.2.3-alpha..1 < 2.0.0").is_none());
        assert!(parse("^1.0.0 || ^2.0.0-01").is_none());
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
        // node-semver では `!=` comparator は無効なので更新対象にしない
        assert!(parse(">=1.0.0 <2.0.0 !=1.5.0").is_none());
    }

    #[test]
    fn test_parse_compound_extra_whitespace() {
        // 演算子の間に複数の空白が含まれた compound range も Range として認識する
        let spec = parse(">=1.0.0    <2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_compound_with_spaced_operators() {
        // node-semver の comparator set は演算子とバージョンの間に空白を置ける。
        let spec = parse(">= 1.0.0 < 2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_compound_with_bare_partial_lower_bound() {
        // npm の comparator set は bare partial と comparator を空白で結合できる
        // (`1.2 <2.0.0` は `>=1.2.0 <1.3.0 <2.0.0` 相当)。
        let spec = parse("1.2 <2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_compound_with_bare_exact_lower_bound() {
        let spec = parse("1.2.3 <2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_caret_zero_zero_partial() {
        // ^0.0 は >=0.0.0 <0.1.0 と同値
        let spec = parse("^0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "0.0.0");
    }

    /// 回帰テスト: ワイルドカード文字 (`x`/`X`/`*`) の後ろに数値セグメントが続く形式は
    /// node-semver / semver の x-range 規約上 invalid。受理するとフォーマット時に
    /// `1.x.3` → `2.x.4` のような不正出力や、捏造 version での比較が起こる。
    #[test]
    fn test_parse_wildcard_rejects_digit_after_wildcard() {
        assert!(parse("1.x.3").is_none());
        assert!(parse("1.*.3").is_none());
        assert!(parse("v1.X.3").is_none());
    }

    /// 回帰テスト: caret/tilde + ワイルドカードでも、ワイルドカード後の数値は弾く。
    /// `^x.0.0` は major がワイルドカードなのに minor/patch が数値という invalid 形式。
    #[test]
    fn test_parse_caret_tilde_wildcard_rejects_digit_after_wildcard() {
        assert!(parse("^x.0.0").is_none());
        assert!(parse("~x.1.0").is_none());
        assert!(parse("^1.x.5").is_none());
    }

    /// 回帰テスト: node-semver の hyphen range は両端が裸の version でなければならない。
    /// `^1.0 - 2.0` / `~1.0 - 2.0` / `>=1.0 - 2.0` のような演算子付き端点は node-semver 仕様上
    /// invalid。以前は `has_compound_range` が単純な `contains(" - ")` で判定していたため、
    /// 過受理して `^1.0 - 2.0` を Range として受理してしまい、`format_updated` でレジストリ
    /// 最新版を `^X.Y.Z - 2.0` のような壊れた制約として書き戻す可能性があった
    /// (npm install で構文エラーになる)。
    #[test]
    fn test_parse_rejects_operator_prefixed_hyphen_range() {
        assert!(parse("^1.0 - 2.0").is_none());
        assert!(parse("~1.0 - 2.0").is_none());
        assert!(parse(">=1.0 - 2.0").is_none());
        assert!(parse("<=1.0 - 2.0").is_none());
        assert!(parse("1.0 - ^2.0").is_none());
        assert!(parse("1.0 - ~2.0").is_none());
        assert!(parse("1.0 - >=2.0").is_none());
    }

    /// 制御テスト: 通常の hyphen range は引き続き Range として受理される
    #[test]
    fn test_parse_hyphen_range_basic_forms_still_work() {
        // 完全 semver 両端
        let spec = parse("1.0.0 - 2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        // partial 両端
        let spec = parse("1.0 - 2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        // 右辺 partial (test_parse_hyphen_range_partial_upper との重複だが明示)
        let spec = parse("1.2.3 - 2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        // v 接頭辞
        let spec = parse("v1.0.0 - v2.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        // プレリリース付き
        let spec = parse("1.0.0-beta - 2.0.0-rc").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_legacy_tilde_greater_operator() {
        let spec = parse("~>1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.prefix.as_deref(), Some("~>"));
        assert_eq!(spec.format_updated("1.3.0"), "~>1.3.0");
    }
}
