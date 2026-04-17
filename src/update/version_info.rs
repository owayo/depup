//! レジストリからのバージョン情報
//!
//! このモジュールはリリース日付を伴うパッケージバージョンを表す
//! VersionInfo 構造体を提供する。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// レジストリから取得したパッケージバージョンの情報
///
/// `Eq`/`Ord` はバージョン文字列のみで比較する（`released_at` は無視）。
/// これにより BTreeSet 等でも一貫した振る舞いになる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// バージョン文字列 (例: "1.2.3")
    pub version: String,
    /// このバージョンがリリースされた日時
    pub released_at: DateTime<Utc>,
}

impl VersionInfo {
    /// 新しいVersionInfoを作成する
    pub fn new(version: impl Into<String>, released_at: DateTime<Utc>) -> Self {
        Self {
            version: version.into(),
            released_at,
        }
    }

    /// リリース日として現在時刻を使用してVersionInfoを作成する
    pub fn now(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            released_at: Utc::now(),
        }
    }

    /// このバージョンがプレリリース (alpha, beta, rc, canary, dev 等) かチェックする
    pub fn is_prerelease(&self) -> bool {
        is_prerelease_version(&self.version)
    }
}

/// チェック対象のプレリリース識別子
///
/// 安定版として扱わない suffix を列挙する。semver の prerelease マーカーに加え、
/// `-deprecated` / `-yanked` のように作者が「更新非推奨」を示すためにリリース末尾へ
/// 付けるマーカーも除外対象に含める (例: `serde_yaml 0.9.34-deprecated`)。
const PRERELEASE_IDENTIFIERS: &[&str] = &[
    "alpha",
    "beta",
    "rc",
    "canary",
    "dev",
    "preview",
    "next",
    "nightly",
    "snapshot",
    "pre",
    "insiders",
    "experimental",
    // 非推奨マーカー (crates.io などで作者が自発的に付与)
    "deprecated",
    "obsolete",
    "retired",
    "yanked",
    "unmaintained",
];

/// セパレータ (`-`, `.`, `+`, 文字列境界) で区切られた単語としてマッチするかチェックする。
/// 部分文字列マッチによる誤検出 ("enterprise" に "pre" がマッチ等) を防止する。
fn contains_identifier_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let needle_len = needle.len();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        // 前方境界: 文字列先頭、またはセパレータ文字
        let before_ok = abs == 0 || matches!(bytes[abs - 1], b'-' | b'.' | b'+' | b'_' | b' ');
        // 後方境界: 文字列末尾、セパレータ文字、または数字 (例: "alpha1" の "alpha" + "1")
        let end = abs + needle_len;
        let after_ok = end >= haystack.len()
            || matches!(bytes[end], b'-' | b'.' | b'+' | b'_' | b' ')
            || bytes[end].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
        if start >= haystack.len() {
            break;
        }
    }
    false
}

/// バージョン文字列がプレリリースバージョンを表すかチェックする
pub fn is_prerelease_version(version: &str) -> bool {
    let lower = version.to_lowercase();

    // 単語境界ベースの識別子をチェック (alpha, beta, canary 等)
    // セパレータ (-._+ またはバージョン境界) で区切られた単語としてマッチする
    if PRERELEASE_IDENTIFIERS
        .iter()
        .any(|id| contains_identifier_word(&lower, id))
    {
        return true;
    }

    // Python/PEP 440 形式の短縮識別子をチェック:
    // - 26.1a1 (alpha), 21.12b0 (beta), 1.0c1 or 1.0rc1 (release candidate)
    // パターン: 数字の後に 'a', 'b', 'c' が続き、さらに数字が続く
    let chars: Vec<char> = lower.chars().collect();
    for i in 0..chars.len().saturating_sub(2) {
        if chars[i].is_ascii_digit() {
            let next = chars[i + 1];
            // 'a' は alpha, 'b' は beta, 'c' は release candidate
            if (next == 'a' || next == 'b' || next == 'c') && chars[i + 2].is_ascii_digit() {
                return true;
            }
        }
    }

    false
}

impl PartialEq for VersionInfo {
    fn eq(&self, other: &Self) -> bool {
        compare_versions(&self.version, &other.version) == std::cmp::Ordering::Equal
    }
}

impl Eq for VersionInfo {}

impl Ord for VersionInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // semver 風の比較でバージョンを比較
        compare_versions(&self.version, &other.version)
    }
}

impl PartialOrd for VersionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// semver 風ルールでバージョン文字列を比較する
/// 不足パートは 0 として扱う (例: "1.0" == "1.0.0")
/// ビルドメタデータ ('+' 以降) は semver 仕様に従い無視する
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_parts = |s: &str| -> Vec<u64> {
        // 先頭の 'v' または 'V' を除去
        let s = s
            .strip_prefix('v')
            .or_else(|| s.strip_prefix('V'))
            .unwrap_or(s);
        // ビルドメタデータ (+...) を除去
        let s = s.split('+').next().unwrap_or(s);
        // '.' と '-' で分割し、数値部分のみ取得
        s.split(['.', '-']).filter_map(|p| p.parse().ok()).collect()
    };

    let parts_a = parse_parts(a);
    let parts_b = parse_parts(b);

    let max_len = parts_a.len().max(parts_b.len());

    // 各パートを比較 (不足パートは 0 として扱う)
    for i in 0..max_len {
        let pa = parts_a.get(i).copied().unwrap_or(0);
        let pb = parts_b.get(i).copied().unwrap_or(0);
        match pa.cmp(&pb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }

    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_version_info_new() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.released_at, date);
    }

    #[test]
    fn test_version_info_now() {
        let before = Utc::now();
        let info = VersionInfo::now("1.0.0");
        let after = Utc::now();

        assert_eq!(info.version, "1.0.0");
        assert!(info.released_at >= before);
        assert!(info.released_at <= after);
    }

    #[test]
    fn test_version_info_eq_consistent_with_ord() {
        // Eq と Ord がバージョン文字列のみで比較されること
        let date1 = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let date2 = Utc.with_ymd_and_hms(2025, 6, 15, 0, 0, 0).unwrap();
        let a = VersionInfo::new("1.0.0", date1);
        let b = VersionInfo::new("1.0.0", date2);

        // 同じバージョン文字列は released_at が異なっても等しい
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_simple() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("2.0.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_minor() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.1.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_patch() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.1");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_equal() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.0");
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_with_v_prefix() {
        let v1 = VersionInfo::now("v1.0.0");
        let v2 = VersionInfo::now("v2.0.0");
        assert!(v1 < v2);
    }

    #[test]
    fn test_version_comparison_mixed_prefix() {
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("v1.0.0");
        // 等しいはず (v接頭辞は除去される)
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_different_lengths() {
        let v1 = VersionInfo::now("1.0");
        let v2 = VersionInfo::now("1.0.0");
        // 1.0 は 1.0.0 と等価 (不足パートは0として扱う)
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_comparison_semver_equivalence() {
        // 様々なsemver等価バージョンのテスト
        assert_eq!(
            compare_versions("0.15", "0.15.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(compare_versions("1", "1.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("1", "1.0.0"), std::cmp::Ordering::Equal);
        assert_eq!(
            compare_versions("2.0", "2.0.0.0"),
            std::cmp::Ordering::Equal
        );

        // 異なるバージョンは異なるままであるべき
        assert_eq!(compare_versions("0.15", "0.15.1"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("0.16", "0.15.1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("1", "1.0.1"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("2", "1.9.9"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_version_comparison_prerelease() {
        // 簡略化された比較 - プレリリースパートを数値として扱う
        let v1 = VersionInfo::now("1.0.0-alpha");
        let v2 = VersionInfo::now("1.0.0-beta");
        // alpha/beta は数値でないため無視される
        // つまり簡略比較では 1.0.0-alpha == 1.0.0-beta
        // 本番用途では完全なsemverパースが必要
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_versions_basic() {
        assert_eq!(
            compare_versions("1.0.0", "1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(compare_versions("1.0.0", "2.0.0"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("2.0.0", "1.0.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_versions_multi_digit() {
        assert!(compare_versions("1.9.0", "1.10.0") == std::cmp::Ordering::Less);
        assert!(compare_versions("10.0.0", "9.0.0") == std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_serde_version_info() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);

        let json = serde_json::to_string(&info).unwrap();
        let parsed: VersionInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, "1.2.3");
        assert_eq!(parsed.released_at, date);
    }

    #[test]
    fn test_version_info_clone() {
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        let info = VersionInfo::new("1.2.3", date);
        let cloned = info.clone();

        assert_eq!(info, cloned);
    }

    #[test]
    fn test_version_sorting() {
        let mut versions = [
            VersionInfo::now("2.0.0"),
            VersionInfo::now("1.0.0"),
            VersionInfo::now("1.5.0"),
            VersionInfo::now("1.0.1"),
        ];

        versions.sort();

        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[1].version, "1.0.1");
        assert_eq!(versions[2].version, "1.5.0");
        assert_eq!(versions[3].version, "2.0.0");
    }

    #[test]
    fn test_find_max_version() {
        let versions = [
            VersionInfo::now("1.0.0"),
            VersionInfo::now("2.5.0"),
            VersionInfo::now("2.0.0"),
            VersionInfo::now("1.9.9"),
        ];

        let max = versions.iter().max().unwrap();
        assert_eq!(max.version, "2.5.0");
    }

    #[test]
    fn test_is_prerelease_stable_versions() {
        // 安定版バージョンはプレリリースとして検出されてはいけない
        assert!(!is_prerelease_version("1.0.0"));
        assert!(!is_prerelease_version("2.5.3"));
        assert!(!is_prerelease_version("v1.0.0"));
        assert!(!is_prerelease_version("10.20.30"));
    }

    #[test]
    fn test_is_prerelease_alpha_beta_rc() {
        assert!(is_prerelease_version("1.0.0-alpha"));
        assert!(is_prerelease_version("1.0.0-alpha.1"));
        assert!(is_prerelease_version("1.0.0-beta"));
        assert!(is_prerelease_version("1.0.0-beta.2"));
        assert!(is_prerelease_version("1.0.0-rc.1"));
        assert!(is_prerelease_version("2.0.0-RC1"));
    }

    #[test]
    fn test_is_prerelease_canary_dev() {
        // React風のcanaryバージョン
        assert!(is_prerelease_version("19.3.0-canary-52684925-20251110"));
        // TypeScript風のdevバージョン
        assert!(is_prerelease_version("6.0.0-dev.20260103"));
        // Vite風のbetaバージョン
        assert!(is_prerelease_version("8.0.0-beta.5"));
        // Prettier風のalpha
        assert!(is_prerelease_version("4.0.0-alpha.13"));
    }

    #[test]
    fn test_is_prerelease_other_identifiers() {
        assert!(is_prerelease_version("1.0.0-preview"));
        assert!(is_prerelease_version("1.0.0-next"));
        assert!(is_prerelease_version("1.0.0-nightly"));
        assert!(is_prerelease_version("1.0.0-snapshot"));
        assert!(is_prerelease_version("1.0.0-pre.1"));
        assert!(is_prerelease_version("1.0.0-insiders"));
        assert!(is_prerelease_version("1.0.0-experimental"));
    }

    #[test]
    fn test_is_prerelease_python_pep440_style() {
        // Python/PEP 440 形式: 数字 + a/b/c + 数字
        // アルファリリース
        assert!(is_prerelease_version("26.1a1"));
        assert!(is_prerelease_version("18.3a0"));
        assert!(is_prerelease_version("1.0a1"));
        // ベータリリース
        assert!(is_prerelease_version("21.12b0"));
        assert!(is_prerelease_version("21.11b1"));
        assert!(is_prerelease_version("1.0b2"));
        // リリース候補 ('c' 使用)
        assert!(is_prerelease_version("1.0c1"));
        assert!(is_prerelease_version("2.5c0"));
        // 安定版バージョンはマッチしないべき
        assert!(!is_prerelease_version("25.12.0"));
        assert!(!is_prerelease_version("1.2.3"));
        assert!(!is_prerelease_version("2024.1.1"));
    }

    #[test]
    fn test_version_info_is_prerelease() {
        let stable = VersionInfo::now("1.0.0");
        assert!(!stable.is_prerelease());

        let canary = VersionInfo::now("19.3.0-canary-52684925-20251110");
        assert!(canary.is_prerelease());

        let beta = VersionInfo::now("8.0.0-beta.5");
        assert!(beta.is_prerelease());
    }

    #[test]
    fn test_compare_versions_ignores_build_metadata() {
        // semver ビルドメタデータ (+...) はバージョン優先度に影響しないべき
        assert_eq!(
            compare_versions("1.0.0", "1.0.0+spec-1.1.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.0.0+spec-1.1.0", "1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.0.0+build.1", "1.0.0+build.2"),
            std::cmp::Ordering::Equal
        );
        // 実際のバージョン差は引き続き機能するべき
        assert_eq!(
            compare_versions("1.0.0+build", "1.0.1"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_compare_versions_four_part_versions() {
        // 一部のエコシステムは4パートバージョンを使用 (例: Java SNAPSHOT, .NET)
        assert_eq!(
            compare_versions("1.0.0.0", "1.0.0.1"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0.1", "1.0.0.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0.0", "1.0.0.0"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_compare_versions_large_numbers() {
        // CalVer形式の大きなバージョン番号
        assert_eq!(
            compare_versions("2024.1.1", "2025.1.1"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("2025.12.31", "2025.12.31"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("20260226", "20260227"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_is_prerelease_false_positives_avoided() {
        // プレリリースに似た部分文字列を含むがプレリリースではないバージョン
        // "1.0.0" にはプレリリース識別子が含まれない
        assert!(!is_prerelease_version("1.0.0"));
        // ハイフン後が数値のみのバージョン
        assert!(!is_prerelease_version("1.0.0-1"));
        // CalVer日付はプレリリースをトリガーしないべき
        assert!(!is_prerelease_version("2024.1.15"));
        assert!(!is_prerelease_version("25.12.0"));
    }

    #[test]
    fn test_is_prerelease_pep440_edge_cases() {
        // ポストリリース (PEP 440) - プレリリースではない
        // 注: 現在の実装はポストリリースを数字+文字+数字パターンで特別処理する;
        // 'p' は a/b/c ではないためマッチしないべき
        assert!(!is_prerelease_version("1.0.0.post1"));
        // dev0 はプレリリース ("dev" を含む)
        assert!(is_prerelease_version("1.0.0.dev0"));
        // 複合: dev + rc
        assert!(is_prerelease_version("1.0.0.dev1rc1"));
    }

    #[test]
    fn test_compare_versions_single_component() {
        // 単一コンポーネントバージョン (例: Rust crate "1")
        assert_eq!(compare_versions("1", "2"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("10", "9"), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions("1", "1"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_version_info_ordering_consistency() {
        // Ord/PartialOrd の一貫性検証
        let v1 = VersionInfo::now("1.0.0");
        let v2 = VersionInfo::now("1.0.0");
        let v3 = VersionInfo::now("2.0.0");

        // 反射的
        assert_eq!(v1.cmp(&v2), std::cmp::Ordering::Equal);
        // 反対称
        assert_eq!(v1.cmp(&v3), std::cmp::Ordering::Less);
        assert_eq!(v3.cmp(&v1), std::cmp::Ordering::Greater);
        // PartialOrd は Ord と一致する
        assert_eq!(v1.partial_cmp(&v3), Some(std::cmp::Ordering::Less));
    }

    #[test]
    fn test_compare_versions_empty_string() {
        // 空文字列は 0 として扱われる
        assert_eq!(compare_versions("", ""), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("", "1.0.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_compare_versions_qualifier_suffix() {
        // Java 風の qualifier (RELEASE, Final) は非数値部で終了する
        assert_eq!(
            compare_versions("5.0.0", "5.0.0.RELEASE"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn test_is_prerelease_java_snapshot() {
        assert!(is_prerelease_version("1.0.0-SNAPSHOT"));
    }

    #[test]
    fn test_is_prerelease_release_suffix_not_prerelease() {
        // RELEASE サフィックスはプレリリースではない
        assert!(!is_prerelease_version("5.0.0.RELEASE"));
        assert!(!is_prerelease_version("4.0.0.Final"));
    }

    #[test]
    fn test_is_prerelease_case_insensitive() {
        // 大文字小文字混在でもプレリリースとして検出される
        assert!(is_prerelease_version("1.0.0-ALPHA"));
        assert!(is_prerelease_version("1.0.0-Beta.1"));
        assert!(is_prerelease_version("1.0.0-RC1"));
        assert!(is_prerelease_version("1.0.0-CANARY"));
    }

    #[test]
    fn test_is_prerelease_non_version_strings() {
        // バージョン表記ではない文字列の処理
        assert!(!is_prerelease_version("hello"));
        assert!(!is_prerelease_version(""));
        assert!(!is_prerelease_version("abc"));
        // "development" は "dev" を部分文字列として含むが、
        // 単語境界チェックにより誤検出されない
        assert!(!is_prerelease_version("development"));
    }

    #[test]
    fn test_compare_versions_v_prefix_mixed() {
        // v/V 接頭辞が混在していても正しく比較できる
        assert_eq!(
            compare_versions("v1.0.0", "V1.0.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("v2.0.0", "1.0.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0", "V2.0.0"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn test_contains_identifier_word_basic() {
        // 基本的な単語境界マッチ
        assert!(contains_identifier_word("1.0.0-alpha", "alpha"));
        assert!(contains_identifier_word("1.0.0-dev.1", "dev"));
        assert!(contains_identifier_word("1.0.0+pre.1", "pre"));
        assert!(contains_identifier_word("alpha-1.0.0", "alpha"));
    }

    #[test]
    fn test_contains_identifier_word_false_positives_prevented() {
        // 部分文字列マッチによる誤検出が防止される
        assert!(!contains_identifier_word("1.0.0-enterprise", "pre"));
        assert!(!contains_identifier_word("1.0.0-deprecated", "pre"));
        assert!(!contains_identifier_word("1.0.0-spread", "pre"));
        assert!(!contains_identifier_word("development", "dev"));
        assert!(!contains_identifier_word("1.0.0-nextcloud", "next"));
        assert!(!contains_identifier_word("salpha", "alpha"));
        assert!(!contains_identifier_word("preemptive", "pre"));
    }

    #[test]
    fn test_contains_identifier_word_separators() {
        // 各種セパレータで区切られた場合にマッチする
        assert!(contains_identifier_word("1.0.0-dev", "dev")); // ハイフン
        assert!(contains_identifier_word("1.0.0.dev", "dev")); // ドット
        assert!(contains_identifier_word("1.0.0+dev", "dev")); // プラス
        assert!(contains_identifier_word("1.0.0_dev", "dev")); // アンダースコア
        assert!(contains_identifier_word("dev", "dev")); // 文字列全体
    }

    #[test]
    fn test_contains_identifier_word_digit_boundary() {
        // 識別子の後に数字が続く場合もマッチする (例: alpha1)
        assert!(contains_identifier_word("1.0.0-alpha1", "alpha"));
        assert!(contains_identifier_word("1.0.0-beta2", "beta"));
        assert!(contains_identifier_word("1.0.0-rc1", "rc"));
        assert!(contains_identifier_word("1.0.0-dev0", "dev"));
    }

    #[test]
    fn test_contains_identifier_word_edge_cases() {
        // 空文字列や境界ケース
        assert!(!contains_identifier_word("", "dev"));
        assert!(!contains_identifier_word("abc", "development"));
        assert!(contains_identifier_word("dev", "dev"));
        assert!(!contains_identifier_word("d", "dev"));
    }

    #[test]
    fn test_is_prerelease_word_boundary_regression() {
        // Bug回帰テスト: 部分文字列マッチによる誤検出が修正されている
        // "enterprise" は "pre" を部分文字列として含むがプレリリースではない
        assert!(!is_prerelease_version("1.0.0-enterprise"));
        // これらは正しくプレリリースと判定される
        assert!(is_prerelease_version("1.0.0-pre"));
        assert!(is_prerelease_version("1.0.0-pre.1"));
        assert!(is_prerelease_version("1.0.0-pre1"));
        // "dev" の境界チェック
        assert!(!is_prerelease_version("1.0.0-devtools"));
        assert!(is_prerelease_version("1.0.0-dev"));
        assert!(is_prerelease_version("1.0.0-dev.1"));
        assert!(is_prerelease_version("1.0.0-dev0"));
    }

    #[test]
    fn test_is_prerelease_deprecation_markers() {
        // 作者が「更新非推奨」を示すために付けたマーカーは prerelease 扱いで
        // デフォルト更新対象から外す (例: `serde_yaml 0.9.34-deprecated`)
        assert!(is_prerelease_version("0.9.34-deprecated"));
        assert!(is_prerelease_version("1.0.0-DEPRECATED"));
        assert!(is_prerelease_version("1.0.0-obsolete"));
        assert!(is_prerelease_version("1.0.0-retired"));
        assert!(is_prerelease_version("1.0.0-yanked"));
        assert!(is_prerelease_version("1.0.0-unmaintained"));
        // 単語境界チェック: "deprecated" を部分文字列として含む別の語は除外しない
        assert!(!is_prerelease_version("1.0.0-undeprecated"));
    }
}
