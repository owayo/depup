//! Python (pip/poetry) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `==1.2.3`
//! - Caret: `^1.2.3` (Poetry)
//! - Tilde: `~1.2.3`, `~=1.2.3`
//! - 比較演算子: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`, `!=1.2.3`, `===1.2.3`
//! - ワイルドカード: `1.*`
//! - レンジ: `>=1.0,<2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind, range_lower_bound_version};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Python バージョン指定パーサ
pub struct PythonVersionParser;

// Python のバージョン指定用正規表現
// PEP 440 のバージョンは少なくとも 1 つの数字を含む必要があるため、
// version 部の先頭は `[vV]?\d` に限定する。
// 数字を含まない無効入力 (例: `==hello`, `>=foo`) を `version=""` の
// VersionSpec として silent に受理してしまわないための水際チェック。
// epoch (`1!2.3`) は先頭の `\d+!` で許容される。
static CARET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\^\s*([vV]?\d[0-9A-Za-z._!+-]*(?:\*)?)$").unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~\s*([vV]?\d[0-9A-Za-z._!+-]*(?:\*)?)$").unwrap());
static OP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(===|==|!=|~=|>=|<=|>|<)\s*([vV]?\d[0-9A-Za-z._!+-]*(?:\*)?)$").unwrap()
});
static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
            r"^(?:\s*(?:===|==|!=|~=|>=|<=|>|<)\s*[vV]?\d[0-9A-Za-z._!+-]*(?:\*)?\s*,)*\s*(?:===|==|!=|~=|>=|<=|>|<)\s*[vV]?\d[0-9A-Za-z._!+-]*(?:\*)?\s*,?\s*$",
        )
        .unwrap()
});
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*$|^\d+(?:\.\d+)*\.\*$").unwrap());
static PEP440_RELEASE_PREFIX_WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[vV]?(?:\d+!)?\d+(?:\.\d+)*\.\*$").unwrap());
// Poetry の演算子なし完全一致ピン (`requests = "2.28.0"`) 用。
// PEP 440 の epoch (`1!`)・release・pre/post/dev・local (`+local`) を許容するが、
// ワイルドカード (`*`) や演算子・カンマ・空白は含まない (単一の bare バージョンのみ)。
// 書き換え時に `v` を落とす副作用を避けるため v/V 接頭辞は受理しない
// (Poetry の bare ピンで v 接頭辞を使う例は稀)。
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d[0-9A-Za-z._!+-]*$").unwrap());

fn pep440_prefix_wildcard_is_allowed(op: &str, raw_version: &str) -> bool {
    if !raw_version.ends_with(".*") {
        return true;
    }

    match op {
        // PEP 440 の prefix matching は `==` / `!=` かつ release segment のみ有効。
        // pre/post/dev/local を含む `==1.0a1.*` や ordered comparison の `>=1.0.*`
        // を受けると、上限を持たない Range として誤判定されるため parse 時点で弾く。
        "==" | "!=" => PEP440_RELEASE_PREFIX_WILDCARD_RE.is_match(raw_version),
        // `===1.0.*` は arbitrary equality であり prefix matching ではない。
        "===" => true,
        _ => false,
    }
}

/// 比較用バージョン文字列へ正規化する。
///
/// PEP 440 のプレリリース (`2.0.0rc1`)・ポスト (`1.0.post1`)・dev (`1.0.dev1`)・
/// エポック (`1!2.3`) は `compare_versions` が正しく順序付けできるため**保持**する。
/// 以前はこれらを剥ぎ取っていたため、`>=2.0.0rc1` の比較基準が `2.0.0` になり
/// rc 利用者が安定版 `2.0.0` へ昇格できない (AlreadyLatest 判定) 不具合があった。
///
/// 一方で次の正規化は従来どおり維持する:
/// - 先頭の `v` / `V` 接頭辞と数字以前の文字の除去
/// - ワイルドカード (`1.2.*` → `1.2`) と末尾セパレータの除去
/// - ローカルバージョン (`+local` 以降) の除去 (ordered/compatible 指定では許可されないため)
fn normalize_for_compare(version: &str) -> String {
    normalize_for_compare_inner(version, false)
}

/// `==` / `!=` のように PEP 440 が local version を許容する指定用の正規化。
fn normalize_for_compare_preserving_local(version: &str) -> String {
    normalize_for_compare_inner(version, true)
}

fn normalize_local_label(local: &str) -> String {
    local
        .split(['.', '-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(".")
}

fn normalize_for_compare_inner(version: &str, preserve_local: bool) -> String {
    let s = version.trim();
    let s = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);
    let (public, local) = s
        .split_once('+')
        .map(|(public, local)| (public, Some(local)))
        .unwrap_or((s, None));
    // PEP 440 エポック (`N!`) は比較の最優先キーなので保持する
    let (epoch, rest) = match public.split_once('!') {
        Some((e, r)) if !e.is_empty() && e.chars().all(|c| c.is_ascii_digit()) => (Some(e), r),
        _ => (None, public),
    };
    let mut buf = String::new();
    let mut seen_digit = false;
    for ch in rest.chars() {
        if !seen_digit {
            // 最初の数字までの文字 (v 接頭辞の残りや空白) は読み飛ばす
            if ch.is_ascii_digit() {
                seen_digit = true;
                buf.push(ch);
            }
            continue;
        }
        // PEP 440 バージョン本体: 数字・英字 (rc/post/dev 等)・`.`・`-` を保持する。
        // `+` (ローカルバージョン) や `*` (ワイルドカード) 以降は切り落とす。
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-') {
            buf.push(ch);
        } else {
            break;
        }
    }
    if buf.is_empty() {
        return String::new();
    }
    let core = buf.trim_matches(['.', '-']);
    let mut normalized = match epoch {
        Some(e) => format!("{e}!{core}"),
        None => core.to_string(),
    };
    if preserve_local && let Some(local) = local {
        let local = normalize_local_label(local);
        if !local.is_empty() {
            normalized.push('+');
            normalized.push_str(&local);
        }
    }
    normalized
}

/// レンジ指定の先頭制約からバージョン部分を取り出す (例: `>=1.0,<2.0` → `1.0`)。
/// エポックやプレリリースを含むバージョン (例: `>=2.0.0rc1`) も形を保って抽出する。
fn extract_first_version(raw: &str) -> String {
    let first = raw.split(',').next().unwrap_or("");
    let stripped = first
        .trim()
        .trim_start_matches(['=', '<', '>', '!', '~', ' ', '\t']);
    normalize_for_compare(stripped)
}

/// Poetry の演算子なしバージョン (`requests = "2.28.0"`) を完全一致ピンとして解釈する。
///
/// Poetry では bare な文字列は完全一致 (公式ドキュメントの "Exact requirements"、
/// `==2.28.0` と同義) を意味する。`==` 版と同じく `VersionSpecKind::Exact` にするが、
/// prefix は付けない (元表記に演算子が無いため、書き換えも演算子なしで行う)。
/// ワイルドカードや演算子付きは `parse` 側で処理済みなので、ここには到達しない。
fn parse_poetry_bare_pin(version_str: &str) -> Option<VersionSpec> {
    let trimmed = version_str.trim();
    if !BARE_VERSION_RE.is_match(trimmed) {
        return None;
    }
    // `==` と同様に local version (`+local`) を保持して比較する。
    let normalized = normalize_for_compare_preserving_local(trimmed);
    if normalized.is_empty() {
        return None;
    }
    Some(VersionSpec::new(
        VersionSpecKind::Exact,
        trimmed,
        normalized,
    ))
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
            if !pep440_prefix_wildcard_is_allowed(op, raw_version) {
                return None;
            }
            let has_local = raw_version.contains('+');
            let normalized = if matches!(op, "==" | "===" | "!=") {
                normalize_for_compare_preserving_local(raw_version)
            } else {
                if has_local {
                    return None;
                }
                normalize_for_compare(raw_version)
            };

            return Some(match op {
                "===" | "==" if op == "===" || !raw_version.ends_with(".*") => {
                    VersionSpec::new(VersionSpecKind::Exact, trimmed, normalized).with_prefix(op)
                }
                "~=" => {
                    // PEP 440 の compatible release (`~=1.2.3` = `>=1.2.3, <1.3.0`、
                    // `~=1.2` = `>=1.2, <2.0`) は明示的な上限を持つレンジ。
                    // Tilde の「最新追従」ではなく Range として扱い、judge で上限を尊重する
                    // (`==1.2.*` を Range 保護しているのと整合させる)。
                    // 単一セグメント (`~=1`) は PEP 440 上無効なのでスキップする。
                    if normalized
                        .split('.')
                        .filter(|part| !part.is_empty())
                        .count()
                        < 2
                    {
                        return None;
                    }
                    VersionSpec::new(VersionSpecKind::Range, trimmed, normalized)
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
            if trimmed.contains('+') {
                return None;
            }
            // 比較基準は記述順に依存せず包含下限を採用する
            // (`<1.5,>=1.2.2` でも下限 `1.2.2` を基準にし、更新の取りこぼしを防ぐ)。
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                range_lower_bound_version(trimmed)
                    .map(|v| normalize_for_compare(&v))
                    .unwrap_or_else(|| extract_first_version(trimmed)),
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

    /// Poetry の `tool.poetry.dependencies` 向けに、演算子なしの bare バージョンを
    /// 完全一致ピンとして受理する。通常の `parse` で解釈できたものはそのまま返し、
    /// None のときだけ bare ピンとしての解釈を試みる。
    fn parse_exact_pin(&self, version_str: &str) -> Option<VersionSpec> {
        self.parse(version_str)
            .or_else(|| parse_poetry_bare_pin(version_str))
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
    fn test_parse_exact_pin_poetry_bare_version() {
        // Poetry の演算子なしバージョンは完全一致ピン (公式: "Exact requirements"、
        // `==2.28.0` と同義)。parse_exact_pin だけが受理し、通常の parse
        // (PEP 508 経路) は演算子必須なので従来どおり None を返す。
        let parser = PythonVersionParser;

        let spec = parser.parse_exact_pin("2.28.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "2.28.0");
        assert_eq!(spec.prefix, None);
        assert!(spec.is_pinned());
        // 演算子なしなので書き換えも演算子なし (`==` 版と違い prefix を付けない)
        assert_eq!(spec.format_updated("2.29.0"), "2.29.0");

        // PEP 508 経路 (通常 parse) は bare を受理しない (`name==ver` が必須)
        assert!(parser.parse("2.28.0").is_none());
        assert!(parser.parse("1").is_none());

        // 他の bare 形式 (segment 数違い / pre / post / epoch) も Exact ピンとして受理
        for v in ["1", "1.2", "4.2.1", "1.2.3rc1", "1!2.3", "1.0.post1"] {
            let s = parser.parse_exact_pin(v).unwrap();
            assert_eq!(s.kind, VersionSpecKind::Exact, "v={v}");
            assert!(!s.version.is_empty(), "v={v}");
        }

        // local version は `==` と同様に保持する
        assert_eq!(
            parser.parse_exact_pin("1.0+cu121").unwrap().version,
            "1.0+cu121"
        );

        // 非バージョン文字列や空は None のまま
        assert!(parser.parse_exact_pin("hello").is_none());
        assert!(parser.parse_exact_pin("").is_none());

        // ワイルドカードや演算子付きは parse 側で処理され、bare ピンにはならない
        assert_eq!(
            parser.parse_exact_pin("1.*").unwrap().kind,
            VersionSpecKind::Wildcard
        );
        assert_eq!(
            parser.parse_exact_pin(">=1.0").unwrap().kind,
            VersionSpecKind::GreaterOrEqual
        );
    }

    #[test]
    fn test_parse_exact_with_prerelease() {
        // 仕様変更: PEP 440 の prerelease 部は比較で意味を持つ ("2.0.0rc1 < 2.0.0")
        // ため保持する。以前は "1.2.3" へ剥ぎ取られ、rc/alpha 利用者が対応する
        // 安定版へ昇格できない (AlreadyLatest 誤判定) 不具合があった。
        let spec = parse("==1.2.3a1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3a1");
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
        // PEP 440 の compatible release (~=) は明示的上限を持つレンジなので Range として扱う
        let spec = parse("~=1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_arbitrary_equality() {
        let spec = parse("===v1.2-custom").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.prefix, Some("===".to_string()));
    }

    #[test]
    fn test_parse_arbitrary_equality_with_star_is_exact() {
        // `===` は arbitrary equality であり、`.*` が付いても prefix matching ではない。
        let spec = parse("===1.0.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.prefix, Some("===".to_string()));
        assert!(spec.is_pinned());
    }

    #[test]
    fn test_parse_rejects_invalid_pep440_prefix_wildcards() {
        // PEP 440 の prefix matching は release segment の `==` / `!=` に限定される。
        assert!(parse(">=1.0.*").is_none());
        assert!(parse("<=1.0.*").is_none());
        assert!(parse("~=1.0.*").is_none());
        assert!(parse(">1.0.*").is_none());
        assert!(parse("<1.0.*").is_none());
        assert!(parse("==1.0a1.*").is_none());
        assert!(parse("==1.0.post1.*").is_none());
        assert!(parse("==1.0.dev1.*").is_none());
        assert!(parse("==1.0+local.*").is_none());
        assert!(parse("!=1.0a1.*").is_none());

        assert_eq!(parse("==1.2.*").unwrap().kind, VersionSpecKind::Range);
        assert_eq!(parse("!=1.2.*").unwrap().kind, VersionSpecKind::Range);
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
    fn test_parse_range_with_trailing_comma() {
        // PyPA の dependency specifier は version_many の末尾カンマを許容する
        let spec = parse(">=1.0,<2.0,").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, ">=1.0,<2.0,");
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_single_constraint_with_trailing_comma() {
        // version_many は単一指定でも末尾カンマを許容する
        let spec = parse(">=1.0,").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
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

    // 回帰テスト: 数字を含まない無効な version 部を持つ入力を None で弾く。
    // 以前は OP_RE / CARET_RE / TILDE_RE の version 部が `[0-9A-Za-z]` 始まりだったため、
    // `==hello` / `>=foo` のような数字なし指定が `version = ""` の VersionSpec として
    // silent に受理されてしまい、後段の比較で意図しない更新候補選択が走る可能性があった。
    #[test]
    fn test_parse_rejects_alpha_only_version() {
        assert!(parse("==hello").is_none());
        assert!(parse(">=foo").is_none());
        assert!(parse(">=abc.def").is_none());
        assert!(parse("^foo").is_none());
        assert!(parse("~foo").is_none());
        assert!(parse(">=local-only").is_none());
        // 制御として: 数字を含む有効入力は引き続き受理する
        assert!(parse(">=1.0").is_some());
        assert!(parse("==1.0a1").is_some());
        // v / V 接頭辞 + 数字は引き続き受理する (PEP 440 互換)
        assert!(parse("==v1.0").is_some());
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
    fn test_parse_range_upper_bound_first_uses_lower_bound() {
        // 回帰: 上限が先に書かれた複合制約 (`<1.5,>=1.2.2`) でも、比較基準 version は
        // 包含下限 `1.2.2` を採用する。PEP 440/508 では comparator の記述順は自由。
        // 以前は split(',').next() が先頭 (=上限 1.5) を採用し更新を取りこぼしていた。
        let a = parse("<1.5,>=1.2.2").unwrap();
        assert_eq!(a.kind, VersionSpecKind::Range);
        assert_eq!(a.version, "1.2.2");
        // 下限が先の従来ケースは回帰しないこと
        let b = parse(">=1.2.2,<1.5").unwrap();
        assert_eq!(b.version, "1.2.2");
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
    fn test_format_updated_range_preserves_trailing_comma() {
        let spec = parse(">=3.5.0,<4.0.0,").unwrap();
        assert_eq!(spec.format_updated("3.9.1"), ">=3.9.1,<4.0.0,");
    }

    #[test]
    fn test_parse_pep440_epoch() {
        // 仕様変更: エポックは比較の最優先キー (`0!9.9 < 1!1.0`) のため保持する。
        // 以前は "2.3" へ剥ぎ取られ、エポック情報が比較で失われていた。
        let spec = parse(">=1!2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1!2.3");
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
        // ~=1.2 は >=1.2, <2.0 と同値。明示的上限を持つレンジなので Range として扱う
        let spec = parse("~=1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_compatible_release_single_segment_is_invalid() {
        // PEP 440 では ~=1 のような単一セグメント指定は無効
        assert!(parse("~=1").is_none());
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
        assert_eq!(spec.version, "1.0+local1");
        assert_eq!(spec.prefix, Some("==".to_string()));
    }

    #[test]
    fn test_parse_local_version_normalizes_separator_and_case() {
        let spec = parse("==1.0+Cu_121").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0+cu.121");
    }

    #[test]
    fn test_parse_ordered_local_version_is_invalid() {
        // PEP 440 では local version は ordered/compatible 指定に書けない
        assert!(parse(">=1.0+local1").is_none());
        assert!(parse("~=1.0+local1").is_none());
        assert!(parse(">=1.0+local1,<2.0").is_none());
    }

    #[test]
    fn test_parse_not_equal_local_version_preserved() {
        // `!=` は version matching と同じ構文を使うため local version を許容する
        let spec = parse("!=1.0+local1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0+local1");
    }

    #[test]
    fn test_parse_post_release() {
        // 仕様変更: PEP 440 ポストリリースは対応する release より新しい
        // (`1.0.post1 > 1.0`) ため、比較用 version に保持する (以前は "1.0" へ剥ぎ取り)。
        let spec = parse("==1.0.post1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.post1");
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

    // --- エッジケース追加テスト ---

    #[test]
    fn test_parse_caret_partial() {
        // Poetry の部分 caret
        let spec = parse("^1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_caret_major_only() {
        // Poetry の caret メジャーのみ
        let spec = parse("^1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_parse_tilde_partial() {
        // Poetry の部分 tilde
        let spec = parse("~1.2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Tilde);
        assert_eq!(spec.version, "1.2");
    }

    #[test]
    fn test_parse_gte_with_space() {
        // 演算子後のスペース
        let spec = parse(">= 1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_dev_release() {
        // 仕様変更: PEP 440 開発リリースは対応する release より古い
        // (`1.0.dev1 < 1.0`) ため、比較用 version に保持する (以前は "1.0" へ剥ぎ取り)。
        let spec = parse("==1.0.dev1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.dev1");
    }

    #[test]
    fn test_parse_range_triple_constraints() {
        // 3つの制約を含むレンジ
        let spec = parse(">=1.0,!=1.5.0,<2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_compatible_release_updates_correctly() {
        // ~=1.2.3 の更新フォーマット
        let spec = parse("~=1.2.3").unwrap();
        assert_eq!(spec.format_updated("1.3.0"), "~=1.3.0");
    }

    /// 回帰テスト (task: prerelease 剥ぎ取り修正): `>=2.0.0rc1` の比較基準は
    /// "2.0.0rc1" を保持する。以前は "2.0.0" になり、`2.0.0rc1 < 2.0.0` の仕様
    /// (CLAUDE.md) と矛盾して rc 利用者が安定版 2.0.0 へ昇格できなかった。
    #[test]
    fn test_parse_gte_keeps_pep440_prerelease() {
        let spec = parse(">=2.0.0rc1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "2.0.0rc1");
        assert_eq!(spec.prefix, Some(">=".to_string()));
        // 更新時は新しいバージョンへ置き換わる
        assert_eq!(spec.format_updated("2.0.0"), ">=2.0.0");
    }

    #[test]
    fn test_parse_caret_keeps_prerelease() {
        // Poetry の caret でも prerelease を保持する
        let spec = parse("^1.2.3rc1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Caret);
        assert_eq!(spec.version, "1.2.3rc1");
    }

    #[test]
    fn test_parse_range_keeps_prerelease_in_first_version() {
        // Range の下限が prerelease でも形を保って抽出する
        let spec = parse(">=2.0.0rc1,<3.0.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "2.0.0rc1");
    }

    #[test]
    fn test_normalize_for_compare_preserves_and_strips() {
        use super::normalize_for_compare;
        // 保持: prerelease / post / dev / epoch
        assert_eq!(normalize_for_compare("2.0.0rc1"), "2.0.0rc1");
        assert_eq!(normalize_for_compare("1.0.post1"), "1.0.post1");
        assert_eq!(normalize_for_compare("1.0.dev1"), "1.0.dev1");
        assert_eq!(normalize_for_compare("1!2.3"), "1!2.3");
        assert_eq!(normalize_for_compare("1!2.3rc1"), "1!2.3rc1");
        // 維持される正規化: v 接頭辞除去 / ワイルドカード除去 / ローカルバージョン除去
        assert_eq!(normalize_for_compare("v1.2.3"), "1.2.3");
        assert_eq!(normalize_for_compare("1.2.*"), "1.2");
        assert_eq!(normalize_for_compare("1.0+local1"), "1.0");
        assert_eq!(normalize_for_compare("  1.2.3  "), "1.2.3");
        assert_eq!(normalize_for_compare(""), "");
        assert_eq!(normalize_for_compare("custom"), "");
    }

    #[test]
    fn test_parse_epoch_with_prerelease() {
        // エポック + PEP 440 プレリリースの組み合わせ
        let spec = parse(">=1!2.0.0rc1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(spec.version, "1!2.0.0rc1");
        assert_eq!(spec.prefix, Some(">=".to_string()));
    }

    #[test]
    fn test_parse_epoch_with_postrelease() {
        // エポック + ポストリリースの組み合わせ
        let spec = parse("==1!2.0.0.post1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1!2.0.0.post1");
    }

    #[test]
    fn test_parse_arbitrary_equality_with_epoch_prerelease() {
        // === は任意のラベルを許容するが、parser では Exact + 演算子保持として扱う
        let spec = parse("===1!2.0.0a1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.prefix, Some("===".to_string()));
    }
}
