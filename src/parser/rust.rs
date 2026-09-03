//! Rust (Cargo) のバージョン指定パーサ。
//!
//! 対応する形式:
//! - 固定: `=1.2.3`
//! - キャレット: `1.2.3`, `^1.2.3`
//! - チルダ: `~1.2.3`
//! - 比較演算子: `>=1.2.3`, `>1.2.3`, `<=1.2.3`, `<1.2.3`
//! - ワイルドカード: `1.*`, `1.x`, `1.X`
//! - レンジ: `>=1.0, <2.0`

use crate::domain::{Language, VersionSpec, VersionSpecKind, range_lower_bound_version};
use crate::parser::{VersionParser, anchored_op_pattern};
use regex::Regex;
use std::sync::LazyLock;

/// Rust/Cargo バージョン指定パーサ
pub struct RustVersionParser;

// Rust のバージョン指定用正規表現
// 演算子後の空白を許容する（Cargo は `>= 1.2.3` のようなスペース付き指定を受け入れる）
// SemVer の prerelease (`-...`) と build metadata (`+...`) は同時指定を許容する
// (例: `1.2.3-rc.1+build123`)。ビルドメタデータはバージョン比較時には無視されるが、
// マニフェスト上の表記はそのまま保持する。
//
// バージョンコアのパターンは 1 箇所に集約する。Node の NODE_VERSION_PATTERN /
// PHP の PHP_VERSION_CORE と同様、全演算子の正規表現がこの定数を共有することで
// 定義間の不整合を防ぐ。コア断片は非キャプチャグループのみのため `(...)` で包んでも
// キャプチャ番号はずれない (末尾構造が異なる WILDCARD_RE は共有しない)。
const RUST_VERSION_CORE: &str = r"[\d]+(?:\.[\d]+)*(?:-[\w.-]+)?(?:\+[\w.-]+)?";
static EXACT_PINNED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"=", RUST_VERSION_CORE)).unwrap());
static CARET_EXPLICIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"\^", RUST_VERSION_CORE)).unwrap());
static TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"~", RUST_VERSION_CORE)).unwrap());
static GTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r">=", RUST_VERSION_CORE)).unwrap());
static GT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r">", RUST_VERSION_CORE)).unwrap());
static LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"<=", RUST_VERSION_CORE)).unwrap());
static LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&anchored_op_pattern(r"<", RUST_VERSION_CORE)).unwrap());
static BARE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^({RUST_VERSION_CORE})$")).unwrap());
static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:(?:>=|<=|>|<|=)\s*{RUST_VERSION_CORE}\s*,\s*)+(?:>=|<=|>|<|=)\s*{RUST_VERSION_CORE}$"
    ))
    .unwrap()
});
// semver crate は `*` に加えて `x` / `X` もワイルドカード文字として受理する。
// `1.x.x` のように minor/patch 連続のワイルドカードも有効 (`1.x.3` は無効)。
// 先頭の `=` / `^` / `~` 演算子付き (`=1.*` / `^1.*` / `~1.x`) も Cargo (semver crate) では
// valid なので許容し、演算子を保持したまま形を保って更新する (npm の `^1.x` と対称)。
static WILDCARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[=^~]?[\d]+(?:\.[\d]+)*(?:\.[*xX]){1,2}$").unwrap());
// Range の先頭 comparison requirement からバージョン部を抽出する。
// プレリリース (`-...`) とビルドメタデータ (`+...`) も比較基準として保持する。
static RANGE_FIRST_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?:>=|<=|>|<|=)\s*({RUST_VERSION_CORE})$")).unwrap());
// caret/tilde/wildcard 混在の複数要件から先頭のバージョントークンを抽出する。
// (`^1.2.2, <1.5` -> `1.2.2`、`>=1.2, <1.5` -> `1.2`)
static FIRST_VERSION_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(RUST_VERSION_CORE).unwrap());

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

        // チルダ
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
            // 比較基準は記述順に依存せず包含下限を採用する
            // (`<1.5, >=1.2.2` でも下限 `1.2.2` を基準にし、AlreadyLatest 誤判定を防ぐ)。
            // 包含下限が取れない場合 (厳密下限のみ等) は従来どおり先頭コンマ区切りの
            // バージョンにフォールバックする。`>=1.0.0-rc.1, <2.0` のようなプレリリース付き
            // 下限は `-` で分割せず保持する。
            let first_version = range_lower_bound_version(trimmed)
                .or_else(|| {
                    trimmed
                        .split(',')
                        .next()
                        .and_then(|s| RANGE_FIRST_VERSION_RE.captures(s.trim()))
                        .and_then(|caps| caps.get(1))
                        .map(|m| m.as_str().to_string())
                })
                .unwrap_or_default();
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                first_version,
            ));
        }

        // caret/tilde/wildcard を comparator と混在させたカンマ区切りの複数要件
        // (`^1.2.2, <1.5` など) は comparator のみの RANGE_RE では拾えないが、
        // Cargo (semver crate) では valid。parse 漏れによる無言スキップを防ぐため、
        // semver::VersionReq で valid 性を確認して Range として検出する。
        // (`<` 上限のない複数下限などは format 側で安全に Skip される)
        if trimmed.contains(',') && semver::VersionReq::parse(trimmed).is_ok() {
            // ここも包含下限を優先し、取れなければ先頭バージョントークンにフォールバックする。
            let first_version = range_lower_bound_version(trimmed)
                .or_else(|| {
                    FIRST_VERSION_TOKEN_RE
                        .find(trimmed)
                        .map(|m| m.as_str().to_string())
                })
                .unwrap_or_default();
            return Some(VersionSpec::new(
                VersionSpecKind::Range,
                trimmed,
                first_version,
            ));
        }

        // `*` / `x` / `X` 単独は完全な浮動指定なので更新対象にしない
        if matches!(trimmed, "*" | "x" | "X") {
            return None;
        }

        // `1.*` / `1.x` / `1.X` は形を保ったまま更新する。
        // `=1.*` / `^1.*` / `~1.x` の演算子付きワイルドカードも演算子を保持して更新する
        // (比較用 version からは演算子を除き、raw 側に残して format で復元する)。
        if WILDCARD_RE.is_match(trimmed) {
            return Some(VersionSpec::new(
                VersionSpecKind::Wildcard,
                trimmed,
                trimmed.trim_start_matches(['=', '^', '~']),
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
    fn test_parse_range_three_comparators() {
        // Cargo はカンマ区切りの複数 requirement を2個に限定していない
        let spec = parse(">=1.0, <2.0, >=1.0.100").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
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
    fn test_parse_range_preserves_prerelease_lower_bound() {
        // 回帰テスト: Range の下限がプレリリース付きでも version に保持される
        // (旧実装は `-` で分割して "1.0.0" に落としていた)
        let spec = parse(">=1.0.0-rc.1, <2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0-rc.1");
    }

    #[test]
    fn test_parse_range_preserves_build_metadata_lower_bound() {
        // 単一演算子 (GTE_RE) と同様に、ビルドメタデータも version に保持する
        let spec = parse(">=1.0.0+sha.abc123, <2.0").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0+sha.abc123");
    }

    #[test]
    fn test_parse_range_upper_bound_written_first_uses_lower_bound() {
        // 回帰: 上限が先に書かれたレンジでも、比較基準 version は包含下限を採用する。
        // 以前は先頭トークン (=上限) を採用し、judge が AlreadyLatest と誤判定して
        // 有効な更新を取りこぼしていた。comparator の記述順は Cargo(semver)で自由。
        // comparator のみ (RANGE_RE 経路)
        let a = parse("<1.5, >=1.2.2").unwrap();
        assert_eq!(a.kind, VersionSpecKind::Range);
        assert_eq!(a.version, "1.2.2");
        // caret と comparator の混在 (VersionReq 経路)
        let b = parse("<1.5, ^1.2.2").unwrap();
        assert_eq!(b.kind, VersionSpecKind::Range);
        assert_eq!(b.version, "1.2.2");
        // 正しい順序 (下限が先) は従来どおり下限を採用する (回帰しないこと)
        let c = parse(">=1.2.2, <1.5").unwrap();
        assert_eq!(c.version, "1.2.2");
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
    fn test_parse_wildcard_x_lower() {
        // 回帰テスト: cargo (semver crate) が受理する `1.x` を Wildcard として扱う
        let spec = parse("1.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "2.x");
    }

    #[test]
    fn test_parse_wildcard_x_upper() {
        // `1.X` も形を保って更新される
        let spec = parse("1.X").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "2.X");
    }

    #[test]
    fn test_parse_wildcard_patch_x() {
        let spec = parse("1.2.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "2.3.x");
    }

    #[test]
    fn test_parse_wildcard_double_x() {
        // semver crate は minor/patch 連続のワイルドカード `1.x.x` も受理する
        let spec = parse("1.x.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "2.x.x");
    }

    #[test]
    fn test_parse_wildcard_with_operator_prefix() {
        // 回帰テスト: cargo (semver crate) が受理する演算子付きワイルドカードを
        // 演算子を保持したまま Wildcard として扱う (`=1.*` / `^1.*` / `~1.x`)。
        // 以前は parse が None を返し、該当依存が黙ってスキップされていた。
        let spec = parse("=1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1.*"); // 比較用 version は演算子を除く
        assert_eq!(spec.format_updated("2.3.4"), "=2.*");

        let spec = parse("^1.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "^2.*");

        let spec = parse("~1.x").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "~2.x");

        // `^1.2.*` も minor 位置のワイルドカードを保って更新する
        let spec = parse("^1.2.*").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.format_updated("2.3.4"), "^2.3.*");
    }

    #[test]
    fn test_parse_bare_x_is_floating() {
        // `x` / `X` 単独は `*` と同じ完全浮動指定なので更新対象にしない
        assert!(parse("x").is_none());
        assert!(parse("X").is_none());
        // 演算子付きの完全浮動指定 (`^*` / `=x`) も数値アンカーが無いので更新対象にしない
        assert!(parse("^*").is_none());
        assert!(parse("=x").is_none());
    }

    #[test]
    fn test_parse_wildcard_rejects_digit_after_x() {
        // semver crate はワイルドカードの後の数値セグメント (`1.x.3`) を受理しない
        assert!(parse("1.x.3").is_none());
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

    #[test]
    fn test_parse_mixed_caret_comparator_range() {
        // caret/tilde と comparator を混在させた複数要件も Cargo (semver) で valid。
        // comparator のみの RANGE_RE では拾えないが Range として検出する。
        let spec = parse("^1.2.2, <1.5").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.2.2");

        let tilde = parse("~1.2, <1.5").unwrap();
        assert_eq!(tilde.kind, VersionSpecKind::Range);
        assert_eq!(tilde.version, "1.2");
    }

    /// カンマ区切り複数要件に埋め込まれた tilde もセグメント数を保つ。
    /// Cargo の `~1` は `>=1.0.0, <2.0.0` だが `~1.4.2` にすると `<1.5.0` へ縮む
    #[test]
    fn test_format_updated_tilde_in_multi_requirement_keeps_segment_count() {
        for (input, new_version, expected) in [
            ("~1, <5.0", "4.9.0", "~4, <5.0"),
            ("~1.2, <1.5", "1.4.2", "~1.4, <1.5"),
            ("~1.2.3, <1.5", "1.4.2", "~1.4.2, <1.5"),
        ] {
            let spec = parse(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(spec.kind, VersionSpecKind::Range, "input={input}");
            assert_eq!(
                spec.format_updated(new_version),
                expected,
                "input={input} new={new_version}"
            );
        }
    }

    #[test]
    fn test_parse_mixed_lower_bounds_detected_as_range() {
        // 上限のない複数下限の混在も検出はされる (無言スキップしない)。
        // 安全な書き換え可否は format / judge 側で判定される。
        let spec = parse(">=1.2.3, ^1.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_exact_pinned_with_prerelease() {
        // 回帰防止: `=` 演算子 + プレリリースのみ (build metadata 無し)
        // (test_parse_exact_pinned_with_build_metadata は build metadata 付きを確認している)
        let spec = parse("=1.0.0-rc.1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0.0-rc.1");
        assert_eq!(spec.prefix, Some("=".to_string()));
        assert!(spec.is_pinned());
        assert_eq!(spec.format_updated("1.0.0"), "=1.0.0");
    }

    #[test]
    fn test_parse_exact_pinned_with_prerelease_and_build() {
        // 回帰防止: `=` 演算子 + プレリリース + ビルドメタデータ
        let spec = parse("=1.2.3-rc.1+build123").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-rc.1+build123");
        assert_eq!(spec.format_updated("1.2.3"), "=1.2.3");
    }

    /// 回帰テスト: Tilde のセグメント数保持を「実パーサ経由」で検証する。
    ///
    /// Cargo の `~1` は `>=1.0.0, <2.0.0` (major 幅) なので、完全版へ展開すると
    /// minor 幅へ狭まる。`~1.2` / `~1.2.3` はどちらも minor 幅だが、著者が
    /// 表明した粒度を勝手に増やさない。
    #[test]
    fn test_format_updated_tilde_preserves_segment_count_via_parser() {
        for (input, new_version, expected) in [
            ("~1", "2.5.3", "~2"),
            ("~1.0", "1.9.7", "~1.9"),
            ("~1.2.3", "1.8.9", "~1.8.9"),
        ] {
            let spec = parse(input).expect(input);
            assert_eq!(spec.kind, VersionSpecKind::Tilde, "input={}", input);
            assert_eq!(
                spec.format_updated(new_version),
                expected,
                "input={} new={}",
                input,
                new_version
            );
        }
    }
}
