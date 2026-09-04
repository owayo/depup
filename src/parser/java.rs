//! Java/Gradle のバージョン指定パーサ
//!
//! 対応する構文:
//! - 固定バージョン: `1.2.3`, `1.2.3-alpha1`
//! - strict 記法: `1.2.3!!`, `1.2.+!!`, `[1.0,2.0[!!`
//! - プレフィックス指定: `1.2.+` (`1.2` 系を許可)
//! - 動的指定: `latest.release`, `latest.integration` (更新対象外)
//! - SNAPSHOT 参照: `1.2.3-SNAPSHOT`, `1.2.3.SNAPSHOT` (動く参照なので更新対象外)
//! - Maven 形式レンジ: `[1.0,2.0]`, `[1.0,)`, `(,2.0]`, `[1.0,2.0)`, `]1.0,2.0[`
//!
//! 備考: 変数参照 (例: `$version`, `${version}`) は
//! マニフェストパーサ側で解決する。

use crate::domain::{Language, VersionSpec, VersionSpecKind};
use crate::parser::VersionParser;
use regex::Regex;
use std::sync::LazyLock;

/// Java/Gradle のバージョン指定パーサ
pub struct JavaVersionParser;

// Gradle バージョン指定の正規表現

// Gradle は `.`, `-`, `_`, `+` を区切りとして扱い、`1a1` のような
// 数字と英字が混ざったパートも解釈する。
const GRADLE_VERSION_TOKEN: &str = r"\d[0-9A-Za-z]*(?:[.\-_+][0-9A-Za-z]+)*";

// 通常バージョン: 1.2.3 / 1.2.3-SNAPSHOT / 1.2.3.RELEASE
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("^({GRADLE_VERSION_TOKEN})$")).unwrap());

// strict 記法: 1.2.3!!
static STRICT_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("^({GRADLE_VERSION_TOKEN})!!$")).unwrap());

// strict range + prefer 短縮記法: [1.7, 1.8[!!1.7.25
static STRICT_RANGE_WITH_PREFER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?P<range>[\[\(\]]\s*(?:{GRADLE_VERSION_TOKEN})?\s*,\s*(?:{GRADLE_VERSION_TOKEN})?\s*[\]\)\[])\s*!!\s*(?P<prefer>{GRADLE_VERSION_TOKEN})$"
    ))
    .unwrap()
});

// プレフィックス指定: 1.2.+ / 1.+（末尾の strict 記法 `!!` も許可）
static PREFIX_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)\.\+(!!)?$").unwrap());

// 動的指定: latest.release / latest.integration / latest.milestone / latest.<custom-status>
// Gradle 公式仕様では任意の status 識別子を取れる (status scheme で定義可能)。
// すべての `latest.*` を更新対象外として一律にスキップする。
static DYNAMIC_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(latest\.[a-zA-Z][A-Za-z0-9_-]*)$").unwrap());

// Maven 形式レンジ: [1.0,2.0], [1.0,), (,2.0], [1.0,2.0)
// 形式: [(] lower , upper [)] (lower/upper は空またはバージョン、末尾の `!!` も許可)
static MAVEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^[\[\(\]]({GRADLE_VERSION_TOKEN})?\s*,\s*({GRADLE_VERSION_TOKEN})?[\]\)\[](?:!!)?$"
    ))
    .unwrap()
});

// Maven 単一指定 (Hard requirement): [1.0]
// Maven Enforcer / Gradle の仕様で「このバージョンに完全一致する」ことを要求する形式。
// [1.0] は = 1.0 と同義として扱う。
static MAVEN_HARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^\[({GRADLE_VERSION_TOKEN})\]$")).unwrap());

/// バージョンが SNAPSHOT 参照かどうかを返す。
///
/// Maven/Gradle の `-SNAPSHOT` は rc/beta のような固定されたプレリリースではなく、
/// 解決のたびに最新のタイムスタンプ版を取り直す「動く参照」(Gradle の changing module)。
/// これをレジストリの最新 release へ書き換えると、開発ビルドへの参照が黙って固定 release に
/// 変わってしまうため、`latest.release` 等と同じく更新対象から外す。
///
/// 判定は末尾セグメントのみで行う。区切りは Maven 標準の `-` に加え、Spring Boot 系で
/// 使われる `.SNAPSHOT` も同じ意味なので両方を見る。`5.0.0.RELEASE` / `4.0.0.Final` /
/// `3.0.0-jre` のような安定版 qualifier は末尾が SNAPSHOT ではないため巻き込まない。
fn is_snapshot_version(version: &str) -> bool {
    version
        .rsplit(['-', '.'])
        .next()
        .is_some_and(|last_segment| last_segment.eq_ignore_ascii_case("SNAPSHOT"))
}

impl VersionParser for JavaVersionParser {
    fn parse(&self, version_str: &str) -> Option<VersionSpec> {
        let trimmed = version_str.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Maven 単一指定 (Hard requirement) を判定: [1.0]
        // 範囲レンジより先に判定する (両者ともブラケットで始まるため、
        // カンマを含まないこちらを優先するとマッチが安定する)。
        if let Some(caps) = MAVEN_HARD_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            // SNAPSHOT への固定指定は動く参照なので更新しない
            if is_snapshot_version(version) {
                return None;
            }
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version)
                    .with_prefix("[")
                    .with_suffix("]"),
            );
        }

        // strict range + prefer 短縮記法を判定: [1.7, 1.8[!!1.7.25
        if let Some(caps) = STRICT_RANGE_WITH_PREFER_RE.captures(trimmed) {
            let prefer = caps.name("prefer")?.as_str();
            return Some(VersionSpec::new(VersionSpecKind::Range, trimmed, prefer));
        }

        // Maven 形式レンジを判定: [1.0,2.0], [1.0,), (,2.0]
        if MAVEN_RANGE_RE.is_match(trimmed) {
            // 下限があれば下限を基準バージョンとして採用
            if let Some(caps) = MAVEN_RANGE_RE.captures(trimmed) {
                let lower = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let upper = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                // 下限がなければ上限を採用
                let version = if !lower.is_empty() {
                    lower
                } else if !upper.is_empty() {
                    upper
                } else {
                    // `[,]` / `(,)` のように下限・上限が両方空のレンジは意味を持たない。
                    // version="" で受理すると compare_versions が "0.0.0" 相当として扱い、
                    // 「常に古い」と誤判定する原因になるためスキップする。
                    return None;
                };
                return Some(VersionSpec::new(VersionSpecKind::Range, trimmed, version));
            }
        }

        // strict 記法を判定: 1.2.3!!
        if let Some(caps) = STRICT_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            // `!!` は競合解決の強さを指定するだけで、SNAPSHOT が動く参照であることは変わらない
            if is_snapshot_version(version) {
                return None;
            }
            return Some(
                VersionSpec::new(VersionSpecKind::Exact, trimmed, version).with_suffix("!!"),
            );
        }

        // プレフィックス指定を判定: 1.2.+ / 1.2.+!!
        if let Some(caps) = PREFIX_VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            let mut spec = VersionSpec::new(VersionSpecKind::Wildcard, trimmed, version);
            if caps.get(2).is_some() {
                spec = spec.with_suffix("!!");
            }
            return Some(spec);
        }

        // `latest.release` は常に移動する参照なので更新対象にしない
        if DYNAMIC_VERSION_RE.is_match(trimmed) {
            return None;
        }

        // 通常バージョンを判定 (プレリリース識別子含む)
        if let Some(caps) = VERSION_RE.captures(trimmed) {
            let version = caps.get(1)?.as_str();
            // `1.2.3-SNAPSHOT` / `1.2.3.SNAPSHOT` は最新の開発ビルドを指す動く参照なので、
            // レジストリの最新 release で固定してしまわないよう更新対象から外す。
            // judge 側は「現在版がプレリリースなら安定版も候補に残す」仕様のため、
            // parse で弾かないと SNAPSHOT が release へ書き換えられてしまう。
            if is_snapshot_version(version) {
                return None;
            }
            return Some(VersionSpec::new(VersionSpecKind::Exact, trimmed, version));
        }

        None
    }

    fn language(&self) -> Language {
        Language::Java
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(version: &str) -> Option<VersionSpec> {
        JavaVersionParser.parse(version)
    }

    // 基本バージョンのテスト
    #[test]
    fn test_parse_simple_version() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.is_pinned());
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
    fn test_parse_four_segments() {
        let spec = parse("1.2.3.4").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.4");
    }

    // プレリリース系バージョンのテスト
    /// 回帰テスト: SNAPSHOT は「毎回の解決で最新タイムスタンプ版を取り直す動く参照」であり、
    /// rc/beta のような固定プレリリースではない。以前は Exact として surface した結果、
    /// judge の「現在版がプレリリースなら安定版も候補に残す」規則で Maven Central の最新
    /// release へ書き換えられ、開発ビルドへの参照が黙って固定 release に変わっていた。
    #[test]
    fn test_parse_snapshot_is_skipped() {
        assert!(parse("1.2.3-SNAPSHOT").is_none());
    }

    /// Spring Boot 系で使われるドット区切りの SNAPSHOT も同じ扱い
    #[test]
    fn test_parse_dot_separated_snapshot_is_skipped() {
        assert!(parse("1.2.3.SNAPSHOT").is_none());
    }

    /// 小文字綴りも Maven/Gradle の慣用外だが、安全側 (更新しない) に倒す
    #[test]
    fn test_parse_lowercase_snapshot_is_skipped() {
        assert!(parse("1.2.3-snapshot").is_none());
    }

    /// SNAPSHOT を弾く判定が JVM の安定版 qualifier を巻き込まないことを確認する
    /// (ここを巻き込むと Java の依存が全滅する)
    #[test]
    fn test_parse_stable_qualifiers_are_still_updatable() {
        for version in [
            "1.2.3-RELEASE",
            "1.2.3.RELEASE",
            "1.2.3.Final",
            "1.2.3-jre",
            "1.2.3.GA",
            "1.2.3-SP1",
            "1.2.3-android",
        ] {
            let spec = parse(version).unwrap_or_else(|| panic!("{version} should parse"));
            assert_eq!(spec.kind, VersionSpecKind::Exact);
            assert_eq!(spec.version, version);
        }
    }

    #[test]
    fn test_parse_alpha() {
        let spec = parse("1.2.3-alpha1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3-alpha1");
    }

    #[test]
    fn test_parse_beta() {
        let spec = parse("2.0.0-beta2").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "2.0.0-beta2");
    }

    #[test]
    fn test_parse_rc() {
        let spec = parse("3.0.0-RC1").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "3.0.0-RC1");
    }

    #[test]
    fn test_parse_release() {
        let spec = parse("5.0.0.RELEASE").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "5.0.0.RELEASE");
    }

    #[test]
    fn test_parse_final() {
        let spec = parse("4.0.0.Final").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "4.0.0.Final");
    }

    #[test]
    fn test_parse_gradle_mixed_separator_versions() {
        for version in ["1a1", "1.0_final", "1-a+1"] {
            let spec = parse(version).unwrap();
            assert_eq!(spec.kind, VersionSpecKind::Exact);
            assert_eq!(spec.version, version);
        }
    }

    // エッジケースのテスト
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
    fn test_parse_variable_reference() {
        // 変数参照はここでは解釈しない (マニフェストパーサで処理)
        assert!(parse("$wicketVersion").is_none());
        assert!(parse("${wicketVersion}").is_none());
    }

    #[test]
    fn test_parse_with_leading_trailing_whitespace() {
        let spec = parse("  1.2.3  ").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
    }

    // 更新時フォーマット保持のテスト
    #[test]
    fn test_format_updated_simple() {
        let spec = parse("1.2.3").unwrap();
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0");
    }

    /// 旧 `test_format_updated_snapshot` の置き換え。
    /// `1.2.3-SNAPSHOT` → `2.0.0` の書き換えは「動く参照を固定 release にする」変更なので、
    /// 更新の形式を検証するのではなく parse 時点で弾く方針へ変えた。
    #[test]
    fn test_format_updated_snapshot_is_not_reachable() {
        assert!(parse("1.2.3-SNAPSHOT").is_none());
    }

    // 対応言語のテスト
    #[test]
    fn test_java_parser_language() {
        let parser = JavaVersionParser;
        assert_eq!(parser.language(), Language::Java);
    }

    // Gradle バージョン指定のテスト
    // Gradle 文字列記法: implementation("org.springframework:spring-core:5.3.8")
    #[test]
    fn test_parse_gradle_exact_version() {
        let spec = parse("5.3.8").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "5.3.8");
        assert!(spec.is_pinned());
    }

    // Gradle プレフィックス指定: implementation("org.springframework:spring-core:5.3.+")
    #[test]
    fn test_parse_gradle_prefix_version() {
        let spec = parse("5.3.+").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "5.3");
        assert!(!spec.is_pinned());
    }

    // Gradle 動的指定: implementation("org.springframework:spring-core:latest.release")
    #[test]
    fn test_parse_gradle_latest_release() {
        assert!(parse("latest.release").is_none());
    }

    #[test]
    fn test_parse_gradle_latest_integration() {
        assert!(parse("latest.integration").is_none());
    }

    // Gradle の Maven 形式レンジ: implementation("org.springframework:spring-core:[5.2.0, 5.3.8]")
    #[test]
    fn test_parse_gradle_maven_range_closed() {
        let spec = parse("[5.2.0, 5.3.8]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "5.2.0"); // 下限
        assert!(!spec.is_pinned());
    }

    // Gradle の上限なし Maven 形式レンジ: implementation("org.springframework:spring-core:[5.2.0,)")
    #[test]
    fn test_parse_gradle_maven_range_open_upper() {
        let spec = parse("[5.2.0,)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "5.2.0"); // 下限
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_maven_range_open_lower() {
        let spec = parse("(,2.0.0]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "2.0.0"); // 下限が空の場合は上限
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_maven_range_exclusive() {
        let spec = parse("(1.0.0,2.0.0)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0"); // 下限
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_maven_range_alt_brackets() {
        let spec = parse("]1.0.0,2.0.0[").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0"); // 下限
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_gradle_maven_range_alt_upper_exclusive() {
        let spec = parse("[1.0.0,2.0.0[").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0.0"); // 下限
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_prefix_version_single_segment() {
        let spec = parse("1.+").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1");
        assert!(!spec.is_pinned());
    }

    #[test]
    fn test_parse_maven_range_with_qualifier() {
        // qualifier 付き Maven レンジ
        let spec = parse("[1.0,2.0.Final)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_plus_alone_not_supported() {
        // + 単独はサポートしない
        assert!(parse("+").is_none());
    }

    #[test]
    fn test_parse_strict_version() {
        let spec = parse("1.2.3!!").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert!(spec.is_pinned());
        assert_eq!(spec.format_updated("2.0.0"), "2.0.0!!");
    }

    #[test]
    fn test_parse_strict_range_with_prefer() {
        let spec = parse("[1.7, 1.8[!!1.7.25").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.raw, "[1.7, 1.8[!!1.7.25");
        assert_eq!(spec.version, "1.7.25");
        assert_eq!(spec.format_updated("1.7.36"), "[1.7, 1.8[!!1.7.36");
    }

    #[test]
    fn test_parse_strict_version_prefix_not_supported() {
        // 先頭 !! の形式はサポートしない
        assert!(parse("!!1.2.3").is_none());
    }

    #[test]
    fn test_parse_strict_prefix_version() {
        let spec = parse("1.2.+!!").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1.2");
        assert_eq!(spec.suffix.as_deref(), Some("!!"));
        assert_eq!(spec.format_updated("2.3.4"), "2.3.+!!");
    }

    #[test]
    fn test_parse_strict_maven_range_without_prefer() {
        let spec = parse("[1.7, 1.8[!!").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.7");
        assert_eq!(spec.format_updated("1.7.36"), "[1.7.36, 1.8[!!");
    }

    #[test]
    fn test_format_updated_prefix_version() {
        // プレフィックス指定の更新
        let spec = parse("5.3.+").unwrap();
        assert_eq!(spec.format_updated("5.4.2"), "5.4.+");
    }

    #[test]
    fn test_format_updated_prefix_version_single_segment() {
        let spec = parse("5.+").unwrap();
        assert_eq!(spec.format_updated("6.1.0"), "6.+");
    }

    #[test]
    fn test_parse_release_suffix_format_updated() {
        // RELEASE サフィックス付きバージョンの更新
        let spec = parse("5.0.0.RELEASE").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.format_updated("5.1.0"), "5.1.0");
    }

    #[test]
    fn test_parse_final_suffix_format_updated() {
        // Final サフィックス付きバージョンの更新
        let spec = parse("4.0.0.Final").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.format_updated("4.1.0"), "4.1.0");
    }

    #[test]
    fn test_parse_maven_range_alt_lower_exclusive() {
        // ]A,B] 形式 — 下限排他、上限包含
        let spec = parse("]1.0,2.0]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_maven_range_upper_only_parenthesis() {
        // (,2.0) 形式 — 上限排他、下限なし
        let spec = parse("(,2.0)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "2.0");
    }

    #[test]
    fn test_parse_strict_range_with_prefer_no_space() {
        // Gradle の strict range + prefer 短縮記法は空白なしでも解析できる
        let spec = parse("[1.7,1.8[!!1.7.25").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.7.25");
        assert_eq!(spec.format_updated("1.7.36"), "[1.7,1.8[!!1.7.36");
    }

    #[test]
    fn test_parse_strict_version_with_qualifier() {
        // qualifier 付き strict 記法
        let spec = parse("1.2.3.Final!!").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.Final");
        assert_eq!(spec.format_updated("1.3.0"), "1.3.0!!");
    }

    #[test]
    fn test_parse_maven_range_half_open_lower_bracket() {
        // ]1.0,2.0) 形式 — 下限排他上限排他
        let spec = parse("]1.0,2.0)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    // --- エッジケース追加テスト ---

    #[test]
    fn test_parse_maven_range_with_spaces() {
        // Maven レンジ内のスペース
        let spec = parse("[1.0, 2.0)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_maven_range_with_multi_part_qualifier() {
        // Gradle の Maven 形式レンジは SNAPSHOT など複数区切りの qualifier も境界にできる
        let spec = parse("[1.0,1.4.9-beta1-SNAPSHOT]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
    }

    #[test]
    fn test_parse_strict_with_snapshot_is_skipped() {
        // `!!` は競合解決の強さを指定するだけで、SNAPSHOT が動く参照であることは変わらない。
        // 更新すると `1.2.3-SNAPSHOT!!` が固定 release の strict 指定に化けるため弾く。
        assert!(parse("1.2.3-SNAPSHOT!!").is_none());
    }

    #[test]
    fn test_parse_prefix_three_segments() {
        // 3セグメント+プラスのプレフィックス指定
        let spec = parse("1.2.3.+").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_parse_maven_hard_requirement_single_version() {
        // Maven の単一バージョン指定 [1.0] は Hard requirement (= 1.0 と同義)
        let spec = parse("[1.0]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.prefix, Some("[".to_string()));
        assert_eq!(spec.suffix, Some("]".to_string()));
        assert!(spec.is_pinned());
        assert_eq!(spec.format_updated("1.5"), "[1.5]");
    }

    #[test]
    fn test_parse_maven_hard_requirement_three_segments() {
        // 3 セグメントの Hard requirement
        let spec = parse("[1.2.3]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.format_updated("2.0.0"), "[2.0.0]");
    }

    #[test]
    fn test_parse_maven_hard_requirement_with_qualifier() {
        // qualifier 付きの Hard requirement
        let spec = parse("[1.2.3.Final]").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Exact);
        assert_eq!(spec.version, "1.2.3.Final");
        assert_eq!(spec.format_updated("1.3.0"), "[1.3.0]");
    }

    #[test]
    fn test_format_updated_strict_preserves_suffix() {
        // strict 記法の !! が更新後も保持される
        let spec = parse("5.3.8!!").unwrap();
        assert_eq!(spec.format_updated("5.4.0"), "5.4.0!!");
    }

    // Gradle 公式仕様で定義される追加の dynamic status は更新対象外として扱う
    #[test]
    fn test_parse_gradle_latest_milestone() {
        // Gradle のビルトイン status (release < milestone < integration)
        assert!(parse("latest.milestone").is_none());
    }

    #[test]
    fn test_parse_gradle_latest_custom_status() {
        // ユーザ定義 status スキーム (例: latest.snapshot, latest.beta)
        assert!(parse("latest.snapshot").is_none());
        assert!(parse("latest.beta").is_none());
    }

    #[test]
    fn test_parse_gradle_latest_status_with_dash() {
        // ハイフン/アンダースコア入り status 名も許容する
        assert!(parse("latest.pre-release").is_none());
        assert!(parse("latest.dev_build").is_none());
    }

    #[test]
    fn test_parse_gradle_latest_invalid_status() {
        // status 名が空・数字始まり・記号始まりの場合は通常バージョンとして扱われ得る
        assert!(parse("latest.").is_none());
        assert!(parse("latest.123").is_none());
    }

    #[test]
    fn test_parse_maven_hard_requirement_with_snapshot_is_skipped() {
        // Hard requirement (`[1.2.3-SNAPSHOT]`) も SNAPSHOT を指す固定指定なので、
        // release へ書き換えると動く参照が失われる。更新対象から外す。
        assert!(parse("[1.2.3-SNAPSHOT]").is_none());
    }

    /// レンジの境界に現れる SNAPSHOT は「動く参照そのもの」ではなく上限/下限の指定なので、
    /// 従来どおり解析・更新できる (下限側だけが進み、境界の綴りは保持される)。
    #[test]
    fn test_parse_maven_range_with_snapshot_bound_is_still_parsed() {
        let spec = parse("[1.0,2.0-SNAPSHOT)").unwrap();
        assert_eq!(spec.kind, VersionSpecKind::Range);
        assert_eq!(spec.version, "1.0");
        assert_eq!(spec.format_updated("1.9"), "[1.9,2.0-SNAPSHOT)");
    }

    /// 回帰テスト: `[,]` / `(,)` のように下限・上限が両方空の Maven レンジは意味を持たない。
    /// version="" で受理すると compare_versions が "0.0.0" 相当として扱い、
    /// 「常に古い」と誤判定するため None で弾く。
    #[test]
    fn test_parse_rejects_empty_maven_range() {
        assert!(parse("[,]").is_none());
        assert!(parse("(,)").is_none());
        assert!(parse("[,)").is_none());
        assert!(parse("(,]").is_none());
        assert!(parse("[,]!!").is_none());
        assert!(parse("(,)!!").is_none());
    }
}
