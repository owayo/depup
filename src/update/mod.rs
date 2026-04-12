//! 依存関係の更新判定ロジック。
//!
//! 提供内容:
//! - CLI 引数から組み立てる更新フィルタ
//! - レジストリから取得したリリース日時付きバージョン情報
//! - 更新するかスキップするかを決める判定エンジン

mod filter;
mod version_info;

pub use filter::UpdateFilter;
pub use version_info::{VersionInfo, compare_versions, is_prerelease_version};

use crate::domain::{Dependency, SkipReason, UpdateResult, VersionSpecKind};
use chrono::{DateTime, Utc};
use regex::Regex;
use std::sync::LazyLock;

/// レンジの上限制約抽出で共通利用するバージョントークン。
const VERSION_TOKEN: &str = r"[vV]?\d+(?:\.\d+)*(?:[-+][0-9A-Za-z.-]+)?";
/// Maven レンジ専用のバージョントークン。`2.0.Final` のような qualifier を許容する。
const MAVEN_VERSION_TOKEN: &str = r"[vV]?\d+(?:\.[0-9A-Za-z]+)*(?:[-+][0-9A-Za-z.-]+)?";

/// Range 制約から包含上限 (`<=X`) を抽出する正規表現。
static UPPER_BOUND_LTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"<=\s*({VERSION_TOKEN})")).unwrap());
/// Range 制約から排他的上限 (`<X`) を抽出する正規表現。
static UPPER_BOUND_LT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"<\s*({VERSION_TOKEN})")).unwrap());
/// Swift の閉区間 (`A...B`) から上限を抽出する正規表現。
static UPPER_BOUND_SWIFT_CLOSED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\.\.\.\s*({VERSION_TOKEN})")).unwrap());
/// Swift の半開区間 (`A..<B`) から上限を抽出する正規表現。
static UPPER_BOUND_SWIFT_HALF_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\.\.<\s*({VERSION_TOKEN})")).unwrap());
/// ハイフンレンジ (`A - B`) から上限を抽出する正規表現。
static UPPER_BOUND_HYPHEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"{VERSION_TOKEN}\s*-\s*({VERSION_TOKEN})")).unwrap());
/// Maven 形式レンジ (`[1.0,2.0)`, `(,2.0]`) を解釈する正規表現。
static MAVEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^([\[\(\]])\s*({MAVEN_VERSION_TOKEN})?\s*,\s*({MAVEN_VERSION_TOKEN})?\s*([\]\)\[])$"
    ))
    .unwrap()
});

fn normalize_bound_version(version: &str) -> String {
    version
        .strip_prefix('v')
        .or_else(|| version.strip_prefix('V'))
        .unwrap_or(version)
        .to_string()
}

/// npm / Composer のハイフンレンジ右辺を上限制約へ正規化する。
///
/// 右辺が部分指定 (`2`, `2.3`) の場合はワイルドカード展開後の排他的上限へ進める。
/// 例:
/// - `1 - 2` -> `<3`
/// - `1.2 - 2.3` -> `<2.4`
/// - `1.2.3 - 2.3.4` -> `<=2.3.4`
fn normalize_hyphen_upper_bound(version: &str) -> (String, bool) {
    let normalized = normalize_bound_version(version);

    // 修飾子付きや 3 セグメント以上の指定はその値自体を包含上限として扱う。
    if normalized.contains(['-', '+']) {
        return (normalized, true);
    }

    let segments: Vec<&str> = normalized.split('.').collect();
    if !(1..=2).contains(&segments.len()) {
        return (normalized, true);
    }

    let mut numeric_segments = Vec::with_capacity(segments.len());
    for segment in segments {
        let Ok(value) = segment.parse::<u64>() else {
            return (normalized, true);
        };
        numeric_segments.push(value);
    }

    if let Some(last) = numeric_segments.last_mut()
        && let Some(next) = last.checked_add(1)
    {
        *last = next;
        let upper = numeric_segments
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(".");
        return (upper, false);
    }

    (normalized, true)
}

/// Range 制約文字列から上限バージョンと包含可否を取り出す。
///
/// 戻り値は `(upper_bound, inclusive)`:
/// - `<X` と `A..<B` は `(X, false)`
/// - `<=X` と `A...B` は `(X, true)`
fn extract_upper_bound(raw: &str) -> Option<(String, bool)> {
    let trimmed = raw.trim();

    if let Some(caps) = UPPER_BOUND_SWIFT_HALF_OPEN_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), false));
    }

    if let Some(caps) = UPPER_BOUND_LTE_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), true));
    }

    if let Some(caps) = UPPER_BOUND_LT_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), false));
    }

    if let Some(caps) = UPPER_BOUND_SWIFT_CLOSED_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), true));
    }

    if let Some(caps) = UPPER_BOUND_HYPHEN_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some(normalize_hyphen_upper_bound(m.as_str()));
    }

    if let Some(caps) = MAVEN_RANGE_RE.captures(trimmed) {
        let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("").trim();
        if !upper.is_empty() {
            let inclusive = caps.get(4).map(|m| m.as_str()) == Some("]");
            return Some((normalize_bound_version(upper), inclusive));
        }
    }

    None
}

/// 依存関係を更新するかどうかを判断するエンジン
pub struct UpdateJudge {
    /// フィルタ設定
    filter: UpdateFilter,
    /// age 計算に使う現在時刻
    now: DateTime<Utc>,
}

impl UpdateJudge {
    /// 指定されたフィルタで `UpdateJudge` を作る
    pub fn new(filter: UpdateFilter) -> Self {
        Self {
            filter,
            now: Utc::now(),
        }
    }

    /// テスト用に現在時刻を差し替えて `UpdateJudge` を作る
    pub fn with_time(filter: UpdateFilter, now: DateTime<Utc>) -> Self {
        Self { filter, now }
    }

    /// 依存関係を処理対象にすべきか確認する
    /// スキップする場合は `Some(SkipReason)`、処理する場合は `None` を返す
    pub fn should_skip(&self, dependency: &Dependency) -> Option<SkipReason> {
        // 言語フィルタを確認する
        if !self.filter.should_process_language(dependency.language) {
            return Some(SkipReason::LanguageFiltered);
        }

        // パッケージフィルタ（exclude/only）を確認する
        if !self.filter.should_process_package(&dependency.name) {
            if !self.filter.only.is_empty() {
                return Some(SkipReason::NotInOnlyList);
            } else {
                return Some(SkipReason::Excluded);
            }
        }

        // GoPinned (// pinned コメント付き) は always_pinned に関係なくスキップ
        if dependency.version_spec.kind == VersionSpecKind::GoPinned && !self.filter.include_pinned
        {
            return Some(SkipReason::Pinned);
        }

        // pinned 扱いのバージョンを確認する
        // Go や Java のように常に固定指定として扱う言語は `--include-pinned` なしでも処理する
        if dependency.is_pinned()
            && !self.filter.include_pinned
            && !dependency.language.always_pinned()
        {
            return Some(SkipReason::Pinned);
        }

        None
    }

    /// 利用可能バージョンをもとに更新要否を判定する
    pub fn judge(
        &self,
        dependency: &Dependency,
        available_versions: &[VersionInfo],
    ) -> UpdateResult {
        // 先に前段のスキップ条件を評価する
        if let Some(reason) = self.should_skip(dependency) {
            return UpdateResult::skip(dependency.clone(), reason);
        }

        // 候補バージョンがなければスキップする
        if available_versions.is_empty() {
            return UpdateResult::skip(
                dependency.clone(),
                SkipReason::FetchFailed("no versions available".to_string()),
            );
        }

        // 既定ではプレリリース版を除外し、現在版がプレリリースのときだけ候補に含める
        let current_is_prerelease = is_prerelease_version(dependency.version());
        let stable_versions: Vec<&VersionInfo> = if current_is_prerelease {
            // 現在がプレリリースならプレリリース更新も許可する
            available_versions.iter().collect()
        } else {
            // それ以外は安定版だけを候補にする
            available_versions
                .iter()
                .filter(|v| !v.is_prerelease())
                .collect()
        };

        // age 制約があれば候補を絞る
        let age_filtered: Vec<&VersionInfo> = if let Some(min_age) = self.filter.min_age {
            // chrono::Duration は i64 ナノ秒 (約292年) が上限。
            // 変換失敗時は age 制約を無視して全候補を通す。
            if let Ok(chrono_duration) = chrono::Duration::from_std(min_age) {
                let min_release_time = self.now - chrono_duration;
                stable_versions
                    .into_iter()
                    .filter(|v| v.released_at <= min_release_time)
                    .collect()
            } else {
                stable_versions
            }
        } else {
            stable_versions
        };

        // Range 制約の上限がある場合は、その上限を超える候補を除外する
        // 例: ">=3.5.0,<4.0.0" なら 4.0.0 以上を除外する
        let eligible_versions: Vec<&VersionInfo> =
            if dependency.version_spec.kind == VersionSpecKind::Range {
                if let Some((upper_bound, inclusive)) =
                    extract_upper_bound(&dependency.version_spec.raw)
                {
                    age_filtered
                        .into_iter()
                        .filter(|v| {
                            match version_info::compare_versions(&v.version, &upper_bound) {
                                std::cmp::Ordering::Less => true,
                                std::cmp::Ordering::Equal => inclusive,
                                std::cmp::Ordering::Greater => false,
                            }
                        })
                        .collect()
                } else {
                    age_filtered
                }
            } else {
                age_filtered
            };

        if eligible_versions.is_empty() {
            return UpdateResult::skip(dependency.clone(), SkipReason::NoSuitableVersion);
        }

        // semver 比較で最新の更新候補を選ぶ
        let latest = eligible_versions.iter().max().unwrap();

        // 現在版が最新以上ならダウングレードを防いでスキップする
        if version_info::compare_versions(dependency.version(), &latest.version)
            != std::cmp::Ordering::Less
        {
            return UpdateResult::skip_already_latest_with_date(
                dependency.clone(),
                latest.released_at,
            );
        }

        // 更新先の文字列表現を安全に組み立てられない制約は更新対象にしない
        if dependency
            .version_spec
            .try_format_updated(&latest.version)
            .is_none()
        {
            return UpdateResult::skip(
                dependency.clone(),
                SkipReason::ParseError("constraint cannot be updated safely".to_string()),
            );
        }

        // リリース日時付きで更新結果を返す
        UpdateResult::update_with_date(dependency.clone(), &latest.version, latest.released_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Language, VersionSpec, VersionSpecKind};
    use chrono::TimeZone;
    use std::time::Duration;

    fn make_dependency(name: &str, version: &str, language: Language, pinned: bool) -> Dependency {
        let kind = if pinned {
            VersionSpecKind::Exact
        } else {
            VersionSpecKind::Caret
        };
        let spec = if pinned {
            VersionSpec::new(kind, version, version)
        } else {
            VersionSpec::new(kind, format!("^{}", version), version).with_prefix("^")
        };
        Dependency::new(name, spec, false, language)
    }

    fn make_version_info(version: &str, days_ago: i64) -> VersionInfo {
        let released_at = Utc::now() - chrono::Duration::days(days_ago);
        VersionInfo::new(version, released_at)
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap()
    }

    fn make_version_info_at(version: &str, date: DateTime<Utc>) -> VersionInfo {
        VersionInfo::new(version, date)
    }

    #[test]
    fn test_judge_simple_update() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("1.1.0", 50),
            make_version_info("2.0.0", 10),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_judge_already_latest() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "2.0.0", Language::Node, false);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("2.0.0", 10),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip {
            reason,
            released_at,
            ..
        } = result
        {
            assert_eq!(reason, SkipReason::AlreadyLatest);
            // `released_at` には最新候補のリリース日時が入る
            assert!(released_at.is_some());
        }
    }

    #[test]
    fn test_judge_skip_pinned() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, true);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        }
    }

    #[test]
    fn test_judge_include_pinned() {
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, true);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
    }

    #[test]
    fn test_judge_exclude_package() {
        let filter = UpdateFilter::new().with_exclude(vec!["lodash".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Excluded);
        }
    }

    #[test]
    fn test_judge_only_list() {
        let filter = UpdateFilter::new().with_only(vec!["express".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::NotInOnlyList);
        }
    }

    #[test]
    fn test_judge_only_list_match() {
        let filter = UpdateFilter::new().with_only(vec!["lodash".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
    }

    #[test]
    fn test_judge_language_filter() {
        let filter = UpdateFilter::new().with_languages(vec![Language::Python]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::LanguageFiltered);
        }
    }

    #[test]
    fn test_judge_language_filter_match() {
        let filter = UpdateFilter::new().with_languages(vec![Language::Node]);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
    }

    #[test]
    fn test_judge_age_filter() {
        let now = fixed_time();
        let filter = UpdateFilter::new().with_min_age(Duration::from_secs(7 * 24 * 60 * 60)); // 7 days
        let judge = UpdateJudge::with_time(filter, now);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);

        // 3日前の版は新しすぎる
        let recent = make_version_info_at("2.0.0", now - chrono::Duration::days(3));
        // 10日前の版は候補に含まれる
        let old = make_version_info_at("1.5.0", now - chrono::Duration::days(10));

        let versions = vec![old, recent];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // 2.0.0 は新しすぎるため 1.5.0 に更新される
            assert_eq!(new_version, "1.5.0");
        }
    }

    #[test]
    fn test_judge_age_filter_no_suitable() {
        let now = fixed_time();
        let filter = UpdateFilter::new().with_min_age(Duration::from_secs(30 * 24 * 60 * 60)); // 30日
        let judge = UpdateJudge::with_time(filter, now);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);

        // すべて新しすぎて候補外
        let versions = vec![
            make_version_info_at("2.0.0", now - chrono::Duration::days(3)),
            make_version_info_at("1.5.0", now - chrono::Duration::days(10)),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::NoSuitableVersion);
        }
    }

    #[test]
    fn test_judge_no_versions() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        let versions: Vec<VersionInfo> = vec![];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert!(matches!(reason, SkipReason::FetchFailed(_)));
        }
    }

    #[test]
    fn test_should_skip_returns_none_for_normal() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, false);
        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_should_skip_returns_reason_for_pinned() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "1.0.0", Language::Node, true);
        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Pinned));
    }

    #[test]
    fn test_should_skip_go_always_pinned_language() {
        // Go は固定版しか扱わないため pinned でもスキップしない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // Go の依存は常に pinned 扱い（VersionSpecKind::Exact）
        let dep = make_dependency("github.com/gin-gonic/gin", "1.9.0", Language::Go, true);

        // Go は always_pinned 言語なのでスキップしない
        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_should_skip_java_pinned_dependency() {
        // Java/Gradle はレンジ指定を扱えるため、
        // `--include-pinned` なしでは pinned 依存をスキップする
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // 固定バージョンの Java 依存
        let dep = make_dependency(
            "org.springframework:spring-core",
            "6.0.0",
            Language::Java,
            true,
        );

        // Java は always_pinned 言語ではないためスキップされる
        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Pinned));
    }

    #[test]
    fn test_judge_go_pinned_without_include_pinned_flag() {
        // Go 依存は `--include-pinned` なしでも更新対象にする
        let filter = UpdateFilter::new(); // include_pinned = false
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("github.com/gin-gonic/gin", "1.9.0", Language::Go, true);
        let versions = vec![make_version_info("1.10.0", 10)];

        let result = judge.judge(&dep, &versions);
        // Go は always_pinned 言語なので更新される
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.10.0");
        }
    }

    #[test]
    fn test_judge_java_pinned_without_include_pinned_flag() {
        // Java/Gradle はレンジ指定を扱えるため、
        // `--include-pinned` なしでは pinned 依存をスキップする
        let filter = UpdateFilter::new(); // include_pinned = false
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency(
            "org.springframework:spring-core",
            "6.0.0",
            Language::Java,
            true,
        );
        let versions = vec![make_version_info("6.1.0", 10)];

        let result = judge.judge(&dep, &versions);
        // Java は always_pinned ではなく include_pinned も false なのでスキップされる
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        }
    }

    #[test]
    fn test_judge_prevents_downgrade() {
        // 回帰テスト: 0.13 が 0.9.1 に「ダウングレード」されないことを確認する
        // 以前は文字列比較していたため semver 比較になっていなかった
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("mockall", "0.13.0", Language::Rust, false);
        let versions = vec![
            make_version_info("0.9.1", 100),
            make_version_info("0.10.0", 80),
            make_version_info("0.11.0", 60),
            make_version_info("0.12.0", 40),
            make_version_info("0.13.0", 20), // 現在のバージョン
        ];

        let result = judge.judge(&dep, &versions);
        // すでに最新なのでスキップされる
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_multi_digit_version_comparison() {
        // `1.10.0 > 1.9.0` を正しく semver 比較できることを確認する
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("serde", "1.9.0", Language::Rust, false);
        let versions = vec![
            make_version_info("1.8.0", 100),
            make_version_info("1.9.0", 80),
            make_version_info("1.10.0", 60),
            make_version_info("1.11.0", 40),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // 1.9.0 のままでもダウングレードでもなく 1.11.0 に更新される
            assert_eq!(new_version, "1.11.0");
        }
    }

    #[test]
    fn test_judge_no_downgrade_when_current_is_newer() {
        // 現在版が公開済み候補より新しい場合はダウングレードせずスキップする
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("cocoa", "0.26.0", Language::Rust, false);
        let versions = vec![
            make_version_info("0.9.2", 200),
            make_version_info("0.20.0", 100),
            make_version_info("0.25.0", 50),
        ];

        let result = judge.judge(&dep, &versions);
        // 現在の 0.26.0 が候補の最新 0.25.0 より新しいのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_filters_prerelease_versions() {
        // 回帰テスト: stable 版から prerelease 版へは更新しない
        // 例: `react 19.2.1` は `19.3.0-canary-*` に更新しない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("react", "19.2.1", Language::Node, false);
        let versions = vec![
            make_version_info("19.2.0", 30),
            make_version_info("19.2.1", 20),
            make_version_info("19.3.0-canary-52684925-20251110", 5), // prerelease なので無視する
        ];

        let result = judge.judge(&dep, &versions);
        // stable 版としてはすでに最新なのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_filters_various_prerelease_types() {
        // alpha, beta, rc, dev, canary がすべて除外されることを確認する
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("typescript", "5.9.0", Language::Node, false);
        let versions = vec![
            make_version_info("5.8.0", 100),
            make_version_info("5.9.0", 50),
            make_version_info("6.0.0-dev.20260103", 10), // dev なので無視する
            make_version_info("6.0.0-beta.1", 8),        // beta なので無視する
            make_version_info("6.0.0-alpha.5", 6),       // alpha なので無視する
        ];

        let result = judge.judge(&dep, &versions);
        // stable 版としては 5.9.0 が最新なのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_updates_to_stable_not_prerelease() {
        // stable と prerelease の両方がある場合は stable を選ぶ
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("vite", "7.0.0", Language::Node, false);
        let versions = vec![
            make_version_info("7.0.0", 50),
            make_version_info("7.1.0", 20), // stable なので選ばれる
            make_version_info("8.0.0-beta.5", 10), // prerelease なので無視する
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // 8.0.0-beta.5 ではなく 7.1.0 に更新される
            assert_eq!(new_version, "7.1.0");
        }
    }

    #[test]
    fn test_judge_prerelease_current_allows_prerelease_update() {
        // 現在版が prerelease なら、より新しい prerelease への更新を許可する
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // canary を使っている利用者は canary 更新も望んでいるとみなす
        let spec = VersionSpec::new(
            VersionSpecKind::Caret,
            "^19.3.0-canary-123",
            "19.3.0-canary-123",
        );
        let dep = Dependency::new("react", spec, false, Language::Node);

        let versions = vec![
            make_version_info("19.2.1", 30),
            make_version_info("19.3.0-canary-123", 20),
            make_version_info("19.3.0-canary-456", 10), // より新しい canary
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // より新しい canary に更新される
            assert_eq!(new_version, "19.3.0-canary-456");
        }
    }

    #[test]
    fn test_judge_no_suitable_stable_version() {
        // 現在版が stable で、新しい候補が prerelease しかないなら更新対象なし
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("prettier", "3.7.0", Language::Node, false);
        let versions = vec![
            make_version_info("3.6.0", 50),
            make_version_info("3.7.0", 30),
            make_version_info("4.0.0-alpha.13", 10), // 新しい候補は alpha のみ
        ];

        let result = judge.judge(&dep, &versions);
        // stable 版としてはすでに最新なのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_extract_upper_bound() {
        // Range 制約から上限を抽出する補助関数の確認
        assert_eq!(
            super::extract_upper_bound(">=3.5.0,<4.0.0"),
            Some(("4.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0,<2.0"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0, <2.0"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0,<=2.0"),
            Some(("2.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("4.0.0...4.9.9"),
            Some(("4.9.9".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("4.0.0..<5.0.0"),
            Some(("5.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("1.2.0 - 2.0.0"),
            Some(("2.0.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0)"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("(,2.0]"),
            Some(("2.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("]1.0,2.0["),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0["),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(">=1.0.0,<v2.0.0"),
            Some(("2.0.0".to_string(), false))
        );
        // 上限なし
        assert_eq!(super::extract_upper_bound(">=1.0"), None);
        // `>` のみで上限なし
        assert_eq!(super::extract_upper_bound(">1.0"), None);
    }

    #[test]
    fn test_extract_upper_bound_whitespace_handling() {
        // 回帰テスト: 前後の空白を除去して判定できることを確認する
        assert_eq!(
            super::extract_upper_bound("  >=1.0,<2.0  "),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("  >=1.0,<=2.0  "),
            Some(("2.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound(" 4.0.0..<5.0.0 "),
            Some(("5.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound(" 4.0.0...4.9.9 "),
            Some(("4.9.9".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("  1.2.0 - 2.0.0  "),
            Some(("2.0.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("  [1.0,2.0)  "),
            Some(("2.0".to_string(), false))
        );
    }

    #[test]
    fn test_extract_upper_bound_maven_inclusive_bracket() {
        // Maven 形式の閉区間: `[1.0,2.0]`
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0]"),
            Some(("2.0".to_string(), true))
        );
        // Maven 形式の開区間: `(1.0,2.0)`
        assert_eq!(
            super::extract_upper_bound("(1.0,2.0)"),
            Some(("2.0".to_string(), false))
        );
        // Maven 形式で下限なし: `(,2.0)`
        assert_eq!(
            super::extract_upper_bound("(,2.0)"),
            Some(("2.0".to_string(), false))
        );
        // Maven 形式の単一指定: `[1.0]`
        assert_eq!(super::extract_upper_bound("[1.0]"), None);
        // Maven qualifier 付き上限: `[1.0,2.0.Final)`
        assert_eq!(
            super::extract_upper_bound("[1.0,2.0.Final)"),
            Some(("2.0.Final".to_string(), false))
        );
    }

    #[test]
    fn test_extract_upper_bound_v_prefix_normalization() {
        // 返却される上限値から `v` / `V` 接頭辞を除去する
        assert_eq!(
            super::extract_upper_bound(">=v1.0.0,<V2.0.0"),
            Some(("2.0.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("v1.0.0...v2.0.0"),
            Some(("2.0.0".to_string(), true))
        );
        assert_eq!(
            super::extract_upper_bound("v1.0.0..<v3.0.0"),
            Some(("3.0.0".to_string(), false))
        );
    }

    #[test]
    fn test_judge_hyphen_range_respects_upper_bound() {
        // npm のハイフンレンジ `1.0.0 - 2.0.0` は `>=1.0.0 <=2.0.0` と同義
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("lodash", "1.0.0 - 2.0.0", "1.0.0", Language::Node);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("1.5.0", 50),
            make_version_info("2.0.0", 30), // ハイフンレンジは包含なので候補に入る
            make_version_info("2.1.0", 10), // 上限超過で除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_judge_hyphen_range_partial_upper_bound_allows_patch_updates() {
        // npm の `1.2.3 - 2.3` は `>=1.2.3 <2.4` と同義
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("lodash", "1.2.3 - 2.3", "1.2.3", Language::Node);
        let versions = vec![
            make_version_info("1.2.3", 100),
            make_version_info("2.3.5", 50), // `2.3.*` は上限内
            make_version_info("2.4.0", 10), // `2.4` は排他的上限なので除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.3.5");
        }
    }

    #[test]
    fn test_judge_composer_hyphen_range_partial_upper_bound_allows_patch_updates() {
        // Composer の `1.0 - 2.0` は `>=1.0.0 <2.1` と同義
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("vendor/package", "1.0 - 2.0", "1.0", Language::Php);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("2.0.9", 50), // `2.0.*` は上限内
            make_version_info("2.1.0", 10), // `2.1` は排他的上限なので除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.9");
        }
    }

    #[test]
    fn test_judge_maven_range_exclusive() {
        // Maven レンジ `[1.0,2.0)` は `>=1.0 && <2.0`
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "[1.0,2.0)", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 10), // 排他的上限なので除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_maven_range_exclusive_alt_brackets() {
        // Maven の代替記法 `]1.0,2.0[` は `>1.0 && <2.0`
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "]1.0,2.0[", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 10), // 排他的上限なので除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_maven_range_with_qualifier_upper_bound() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep =
            make_range_dependency("org.example:lib", "[1.0,2.0.Final)", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.1", 10), // 上限より大きいので除外されるべき
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_maven_range_inclusive() {
        // Maven レンジ `[1.0,2.0]` は `>=1.0 && <=2.0`
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "[1.0,2.0]", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 30), // 包含上限なので候補に入る
            make_version_info("2.1", 10), // 上限超過で除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0");
        }
    }

    #[test]
    fn test_judge_swift_half_open_range_respects_upper_bound() {
        // Swift の半開レンジ `4.0.0..<5.0.0` は `>=4.0.0 && <5.0.0`
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("vapor/vapor", "4.0.0..<5.0.0", "4.0.0", Language::Swift);
        let versions = vec![
            make_version_info("4.0.0", 100),
            make_version_info("4.99.0", 50),
            make_version_info("5.0.0", 10), // 半開レンジの上限なので除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "4.99.0");
        }
    }

    fn make_range_dependency(
        name: &str,
        raw: &str,
        version: &str,
        language: Language,
    ) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Range, raw, version);
        Dependency::new(name, spec, false, language)
    }

    #[test]
    fn test_judge_range_respects_upper_bound() {
        // 回帰テスト: `paramiko>=3.5.0,<4.0.0` は 4.0.0 に更新されない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("paramiko", ">=3.5.0,<4.0.0", "3.5.0", Language::Python);
        let versions = vec![
            make_version_info("3.5.0", 100),
            make_version_info("3.6.0", 50),
            make_version_info("3.9.0", 20),
            make_version_info("4.0.0", 10), // 上限制約により除外される
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // 4.0.0 ではなく 3.9.0 に更新される
            assert_eq!(new_version, "3.9.0");
        }
    }

    #[test]
    fn test_judge_range_already_at_max_within_bound() {
        // 上限内で既に最新ならスキップする
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("paramiko", ">=3.5.0,<4.0.0", "3.9.0", Language::Python);
        let versions = vec![
            make_version_info("3.5.0", 100),
            make_version_info("3.9.0", 20), // 現在のバージョン
            make_version_info("4.0.0", 10), // 上限制約により除外
            make_version_info("4.1.0", 5),  // 上限制約により除外
        ];

        let result = judge.judge(&dep, &versions);
        // 上限内では 3.9.0 が最新なのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_range_respects_inclusive_upper_bound() {
        // 包含上限なら境界値そのものを許可する
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("package", ">=1.0,<=2.0", "1.0", Language::Python);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("2.0", 50), // 上限内なので候補に入る
            make_version_info("2.1", 20), // 上限超過で除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0");
        }
    }

    #[test]
    fn test_judge_swift_closed_range_respects_upper_bound() {
        // Swift の閉レンジ (`...`) は包含上限を持つ
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("vapor/vapor", "4.0.0...4.9.9", "4.0.0", Language::Swift);
        let versions = vec![
            make_version_info("4.0.0", 100),
            make_version_info("4.9.9", 50), // 上限内なので候補に入る
            make_version_info("5.0.0", 20), // 上限超過で除外
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "4.9.9");
        }
    }

    #[test]
    fn test_judge_range_no_suitable_version_all_above_bound() {
        // 新しい候補がすべて上限超過なら更新候補は存在しない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("package", ">=1.0,<2.0", "1.0", Language::Python);
        let versions = vec![
            make_version_info("1.0", 100), // 現在のバージョン
            make_version_info("2.0", 50),  // 上限超過で除外
            make_version_info("3.0", 20),  // 上限超過で除外
        ];

        let result = judge.judge(&dep, &versions);
        // 上限内に現在版より新しい候補がないのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_range_without_upper_bound_updates_normally() {
        // 上限なし Range（`>=` のみ）は最新へ更新される
        // パーサ上は本来この分類で来ないが、防御的な挙動を確認する
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // 上限なし Range を直接作る境界ケース
        let spec = VersionSpec::new(VersionSpecKind::Range, ">=1.0", "1.0");
        let dep = Dependency::new("package", spec, false, Language::Python);

        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("2.0", 50),
            make_version_info("3.0", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            // 上限がないので最新へ更新される
            assert_eq!(new_version, "3.0");
        }
    }

    #[test]
    fn test_judge_unsupported_range_skips_with_parse_error() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("vendor/package", "^1 || ^2", "1", Language::Node);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("2.5.0", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(
                reason,
                SkipReason::ParseError("constraint cannot be updated safely".to_string())
            );
        }
    }

    #[test]
    fn test_judge_short_version_equivalent_to_full() {
        // 回帰テスト: `"0.15"` は `"0.15.0"` と同値として扱う
        // そのため現在版が `0.15` で最新候補が `0.15.0` なら `AlreadyLatest` になる
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // `vte = "0.15"` で最新候補が `0.15.0` のケースを再現する
        let dep = make_dependency("vte", "0.15", Language::Rust, false);
        let versions = vec![
            make_version_info("0.14.0", 100),
            make_version_info("0.15.0", 50), // latest だが current の `0.15` と同値
        ];

        let result = judge.judge(&dep, &versions);
        // `0.15 == 0.15.0` なのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_extract_upper_bound_maven_exclusive() {
        // Maven 半開区間の上限抽出
        let result = extract_upper_bound("[1.0,2.0)");
        assert_eq!(result, Some(("2.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_maven_inclusive() {
        // Maven 閉区間の上限抽出
        let result = extract_upper_bound("[1.0,2.0]");
        assert_eq!(result, Some(("2.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_maven_alt_brackets() {
        // Maven 代替記法 `]1.0,2.0[` は両端排他
        let result = extract_upper_bound("]1.0,2.0[");
        assert_eq!(result, Some(("2.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_swift_half_open() {
        // Swift 半開レンジ（引用符なしの内部表現）
        let result = extract_upper_bound("1.0.0..<2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_swift_closed() {
        // Swift 閉レンジ（引用符なしの内部表現）
        let result = extract_upper_bound("1.0.0...2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_hyphen_range() {
        let result = extract_upper_bound("1.0.0 - 2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_hyphen_range_partial_upper() {
        let result = extract_upper_bound("1.2.3 - 2.3");
        assert_eq!(result, Some(("2.4".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_hyphen_range_single_segment_upper() {
        let result = extract_upper_bound("1 - 2");
        assert_eq!(result, Some(("3".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_lte() {
        let result = extract_upper_bound(">=1.0.0 <=2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_lt() {
        let result = extract_upper_bound(">=1.0.0 <2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_v_prefix() {
        // v接頭辞は除去される
        let result = extract_upper_bound(">=v1.0.0 <v2.0.0");
        assert_eq!(result, Some(("2.0.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_maven_qualifier() {
        // Maven qualifier 付き上限
        let result = extract_upper_bound("[1.0,2.0.Final)");
        assert_eq!(result, Some(("2.0.Final".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_no_upper() {
        // 上限なし
        let result = extract_upper_bound(">=1.0.0");
        assert_eq!(result, None);
    }

    #[test]
    fn test_should_skip_go_pinned_without_include_pinned() {
        // `// pinned` 付き GoPinned は `include_pinned` なしならスキップされる
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);

        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Pinned));
    }

    #[test]
    fn test_should_skip_go_pinned_with_include_pinned() {
        // GoPinned でも `--include-pinned` を付ければ更新対象になる
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);

        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_judge_go_pinned_skips_update() {
        // GoPinned 依存は `judge` でもスキップされる
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        }
    }

    #[test]
    fn test_judge_go_pinned_with_include_pinned_updates() {
        // GoPinned でも `--include-pinned` なら更新される
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.0.0", "1.0.0");
        let dep = Dependency::new("github.com/critical/lib", spec, false, Language::Go);
        let versions = vec![make_version_info("2.0.0", 10)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_extract_upper_bound_maven_alt_inclusive() {
        // Maven 代替記法 `]1.0,2.0]` は下限排他・上限包含
        let result = extract_upper_bound("]1.0,2.0]");
        assert_eq!(result, Some(("2.0".to_string(), true)));
    }

    #[test]
    fn test_judge_short_version_can_still_update() {
        // 短縮バージョン指定でも、より新しい候補があれば更新される
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // `tokio = "1"` で最新候補が `1.49.0` のケースを再現する
        let dep = make_dependency("tokio", "1", Language::Rust, false);
        let versions = vec![
            make_version_info("1.0.0", 200),
            make_version_info("1.48.0", 50),
            make_version_info("1.49.0", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.49.0");
        }
    }

    #[test]
    fn test_extract_upper_bound_maven_lower_open_exclusive() {
        // Maven 下限なし排他的上限 `(,2.0)` は上限 2.0 で排他
        let result = extract_upper_bound("(,2.0)");
        assert_eq!(result, Some(("2.0".to_string(), false)));
    }

    #[test]
    fn test_extract_upper_bound_maven_lower_open_inclusive() {
        // Maven 下限なし包含上限 `(,2.0]` は上限 2.0 で包含
        let result = extract_upper_bound("(,2.0]");
        assert_eq!(result, Some(("2.0".to_string(), true)));
    }

    #[test]
    fn test_extract_upper_bound_maven_upper_open() {
        // Maven 上限なし `[1.0,)` は上限なし → None
        let result = extract_upper_bound("[1.0,)");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_upper_bound_maven_single_exact() {
        // Maven 単一指定 `[1.5]` はカンマなしなので正規表現にマッチしない → None
        let result = extract_upper_bound("[1.5]");
        assert_eq!(result, None);
    }

    #[test]
    fn test_judge_maven_lower_open_range() {
        // Maven 下限なし `(,2.0]` はフォーマット更新不可で ParseError スキップになる
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "(,2.0]", "0.0", Language::Java);
        let versions = vec![make_version_info("1.0", 100), make_version_info("1.9", 50)];

        let result = judge.judge(&dep, &versions);
        // 下限なし Maven 形式は安全に書き換えられないため ParseError でスキップ
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(
                reason,
                SkipReason::ParseError("constraint cannot be updated safely".to_string())
            );
        }
    }
}
