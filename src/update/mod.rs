//! 依存関係の更新判定ロジック。
//!
//! 提供内容:
//! - CLI 引数から組み立てる更新フィルタ
//! - レジストリから取得したリリース日時付きバージョン情報
//! - 更新するかスキップするかを決める判定エンジン

mod filter;
mod version_info;

pub use filter::UpdateFilter;
pub(crate) use version_info::{NumericIdentifier, numeric_core};
pub use version_info::{
    VersionInfo, compare_semver_versions, compare_versions, is_prerelease_version,
};
// Python の PEP 440 (local version / epoch 等) を含む言語別比較を OSV フォールバック等から
// 直接呼び出せるように公開する。
pub use version_info::compare_python_versions;

use crate::domain::{Dependency, Language, SkipReason, UpdateResult, VersionSpecKind};
use chrono::{DateTime, Utc};
use regex::Regex;
use std::sync::LazyLock;

/// レンジの上限制約抽出で共通利用するバージョントークン。
const VERSION_TOKEN: &str = r"[vV]?\d+(?:\.\d+)*(?:[-+][0-9A-Za-z.-]+)?";
/// Maven レンジ専用のバージョントークン。
/// Gradle の順序付け規則に合わせ、`.`, `-`, `_`, `+` 区切りと英数字混在パートを許容する。
const MAVEN_VERSION_TOKEN: &str = r"[vV]?\d[0-9A-Za-z]*(?:[.\-_+][0-9A-Za-z]+)*";

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
/// npm / Composer の仕様どおりハイフンの前後にスペースを必須とし、
/// Maven の qualifier 付き下限 (`[1.0-2,2.0)` の `1.0-2`) への誤マッチを防ぐ。
static UPPER_BOUND_HYPHEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"{VERSION_TOKEN}\s+-\s+({VERSION_TOKEN})")).unwrap());
/// Maven 形式レンジ (`[1.0,2.0)`, `(,2.0]`) を解釈する正規表現。
static MAVEN_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^([\[\(\]])\s*({MAVEN_VERSION_TOKEN})?\s*,\s*({MAVEN_VERSION_TOKEN})?\s*([\]\)\[])$"
    ))
    .unwrap()
});
/// PEP 440 の prefix-match wildcard (`==1.2.*`) を上限制約として解釈する正規表現。
/// `==1.2.*` は `>=1.2.0, <1.3.0` 相当なので、`.*` 直前のリリースセグメントを +1 した
/// 排他的上限を導出する。epoch (`1!2.0.*`) と `v` 接頭辞も許容する。
/// `!=1.2.*` は除外制約であり上限ではないため、ここでは `==` のみを対象にする。
static PEP440_PREFIX_MATCH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^==\s*[vV]?((?:\d+!)?\d+(?:\.\d+)*)\.\*$").unwrap());
/// PEP 440 の compatible release (`~=1.2.3`) を上限制約として解釈する正規表現。
/// `~=1.2.3` は `>=1.2.3, <1.3.0`、`~=1.2` は `>=1.2, <2.0` 相当。最後のリリースセグメントを
/// 落とした prefix を +1 した排他的上限を導出する。epoch (`1!2.3`) と `v` 接頭辞も許容する。
/// 2 セグメント未満 (`~=1`) は PEP 440 上無効なので parser がスキップ済み。
static PEP440_COMPATIBLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^~=\s*[vV]?((?:\d+!)?\d+(?:\.\d+)+)$").unwrap());

fn normalize_bound_version(version: &str) -> String {
    version_info::strip_ascii_v_prefix(version).to_string()
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

/// PEP 440 prefix-match wildcard (`==1.2.*`) のリリース部を +1 して排他的上限を作る。
///
/// 例:
/// - `1.2` -> `1.3`
/// - `1` -> `2`
/// - epoch 付き `1!2.3` -> `1!2.4`
fn increment_release_prefix(prefix: &str) -> Option<String> {
    let (epoch, release) = prefix
        .split_once('!')
        .map_or((None, prefix), |(e, r)| (Some(e), r));
    let mut parts = release
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let last = parts.last_mut()?;
    *last = last.checked_add(1)?;
    let upper = parts
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(".");
    Some(epoch.map_or_else(|| upper.clone(), |e| format!("{e}!{upper}")))
}

/// PEP 440 compatible release (`~=A.B.C`) の排他的上限を導出する。
/// 最後のリリースセグメントを落とした prefix に `increment_release_prefix` を適用する。
/// `~=1.2.3` -> `1.3`、`~=1.2` -> `2`、epoch 付き `~=1!2.3.4` -> `1!2.4`。
fn compatible_release_upper_bound(release: &str) -> Option<String> {
    let (epoch, core) = release
        .split_once('!')
        .map_or((None, release), |(e, r)| (Some(e), r));
    let segments: Vec<&str> = core.split('.').collect();
    if segments.len() < 2 {
        return None;
    }
    let prefix_core = segments[..segments.len() - 1].join(".");
    let prefix = epoch.map_or_else(|| prefix_core.clone(), |e| format!("{e}!{prefix_core}"));
    increment_release_prefix(&prefix)
}

/// 2 つの上限制約候補からより厳しい方を選ぶ。
/// バージョンが小さい方が厳しい。同値なら排他的 (`<`) を包含 (`<=`) より優先する。
fn stricter_upper_bound(
    best: Option<(String, bool)>,
    candidate: (String, bool),
) -> Option<(String, bool)> {
    match best {
        None => Some(candidate),
        Some(current) => match version_info::compare_versions(&candidate.0, &current.0) {
            std::cmp::Ordering::Less => Some(candidate),
            std::cmp::Ordering::Equal if current.1 && !candidate.1 => Some(candidate),
            _ => Some(current),
        },
    }
}

/// Range 制約文字列から上限バージョンと包含可否を取り出す。
///
/// 戻り値は `(upper_bound, inclusive)`:
/// - `<X` と `A..<B` は `(X, false)`
/// - `<=X` と `A...B` は `(X, true)`
///
/// `<` / `<=` が複数並ぶ場合 (例: `>=1,<2,<=3`) は最も厳しい上限を採用する。
fn extract_upper_bound(raw: &str) -> Option<(String, bool)> {
    let trimmed = raw.trim();
    let trimmed = trimmed
        .split_once("!!")
        .map(|(range, _)| range.trim())
        .unwrap_or(trimmed);

    // Maven 形式レンジは完全アンカー付きなので最初に評価する。
    // `[1.0-2,2.0)` のような qualifier 付き下限がハイフンレンジ等へ誤マッチして
    // 誤った上限 (充足不能レンジ) を返すのを防ぐ。
    if let Some(caps) = MAVEN_RANGE_RE.captures(trimmed) {
        let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("").trim();
        if !upper.is_empty() {
            let inclusive = caps.get(4).map(|m| m.as_str()) == Some("]");
            return Some((normalize_bound_version(upper), inclusive));
        }
        // 上限なしの Maven レンジ (`[1.0,)`) を他形式として再解釈しない
        return None;
    }

    // PEP 440 の prefix-match wildcard (`==1.2.*`) は `<次セグメント` 相当の排他的上限を持つ。
    // 完全アンカー (`^==...\.\*$`) なので他言語の Range 文字列には誤マッチしない。
    if let Some(caps) = PEP440_PREFIX_MATCH_RE.captures(trimmed)
        && let Some(prefix) = caps.get(1)
        && let Some(upper) = increment_release_prefix(prefix.as_str())
    {
        return Some((upper, false));
    }

    // PEP 440 の compatible release (`~=1.2.3`) は `<次セグメント` 相当の排他的上限を持つ。
    // 完全アンカー (`^~=...$`) なので他言語の Range 文字列には誤マッチしない。
    if let Some(caps) = PEP440_COMPATIBLE_RE.captures(trimmed)
        && let Some(release) = caps.get(1)
        && let Some(upper) = compatible_release_upper_bound(release.as_str())
    {
        return Some((upper, false));
    }

    if let Some(caps) = UPPER_BOUND_SWIFT_HALF_OPEN_RE.captures(trimmed)
        && let Some(m) = caps.get(1)
    {
        return Some((normalize_bound_version(m.as_str()), false));
    }

    // `<` / `<=` は複数並びうる (例: `>=1,<2,<=3`)。全マッチを収集し、
    // 最も厳しい上限 (最小バージョン、同値なら排他的) を採用する。
    let mut best: Option<(String, bool)> = None;
    for caps in UPPER_BOUND_LTE_RE.captures_iter(trimmed) {
        if let Some(m) = caps.get(1) {
            best = stricter_upper_bound(best, (normalize_bound_version(m.as_str()), true));
        }
    }
    for caps in UPPER_BOUND_LT_RE.captures_iter(trimmed) {
        if let Some(m) = caps.get(1) {
            best = stricter_upper_bound(best, (normalize_bound_version(m.as_str()), false));
        }
    }
    if best.is_some() {
        return best;
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

        // パッケージフィルタ（exclude/only）を確認する。
        // Cargo のリネーム依存 (manifest_name) を考慮する判定は UpdateFilter 側へ集約している。
        if let Some(reason) = self.filter.package_filter_skip_reason(dependency) {
            return Some(reason);
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

    /// 利用可能バージョンをもとに更新要否を判定する。
    ///
    /// 内部では候補を段階的に絞り込む:
    ///   1. `should_skip` (前段の言語/パッケージ/pinned フィルタ)
    ///   2. プレリリース除外 (`stable_candidates`)
    ///   3. age 制約 (`apply_age_filter`)
    ///   4. Range 上限 (`apply_range_upper_bound`)
    ///   5. `--max-change` 上限 (`apply_max_change_filter`)
    ///   6. ダウングレード防止と更新可否確定 (`select_latest_candidate`)
    pub fn judge(
        &self,
        dependency: &Dependency,
        available_versions: &[VersionInfo],
    ) -> UpdateResult {
        if let Some(reason) = self.should_skip(dependency) {
            return UpdateResult::skip(dependency.clone(), reason);
        }

        if available_versions.is_empty() {
            return UpdateResult::skip(
                dependency.clone(),
                SkipReason::FetchFailed("no versions available".to_string()),
            );
        }

        let stable = self.stable_candidates(dependency, available_versions);
        let age_filtered = self.apply_age_filter(stable);
        let range_filtered = apply_range_upper_bound(dependency, age_filtered);
        let eligible = apply_rejected_versions(dependency, range_filtered);

        if eligible.is_empty() {
            return UpdateResult::skip(dependency.clone(), SkipReason::NoSuitableVersion);
        }

        let allowed = apply_max_change_filter(dependency, &eligible, self.filter.max_change);
        if allowed.is_empty() {
            // 全候補が max_change で除外された。除外された候補の中に現在版より新しい
            // ものが存在する場合のみ ChangeLevelLimited とし、新しい候補がそもそも
            // 無い場合は AlreadyLatest として扱う (max_change が原因ではないため)。
            let current = dependency.version();
            let has_newer_excluded = eligible.iter().any(|v| {
                compare_dependency_versions(dependency, &v.version, current)
                    == std::cmp::Ordering::Greater
            });
            if let (true, Some(max)) = (has_newer_excluded, self.filter.max_change) {
                return UpdateResult::skip(dependency.clone(), SkipReason::ChangeLevelLimited(max));
            }
            // eligible は非空が保証されているため最新候補のリリース日時を添える
            let latest = eligible
                .iter()
                .max_by(|a, b| compare_dependency_versions(dependency, &a.version, &b.version))
                .unwrap();
            return UpdateResult::skip_already_latest_with_date(
                dependency.clone(),
                latest.released_at,
            );
        }

        select_latest_candidate(dependency, &eligible, &allowed, self.filter.max_change)
    }

    /// 既定ではプレリリースを除外する。現在版がプレリリースなら全候補を残す。
    fn stable_candidates<'a>(
        &self,
        dependency: &Dependency,
        available_versions: &'a [VersionInfo],
    ) -> Vec<&'a VersionInfo> {
        let is_prerelease = |version: &str| match dependency.language {
            Language::Node | Language::Rust | Language::Go | Language::Swift => {
                version_info::is_semver_prerelease_version(version)
            }
            Language::Python => version_info::is_python_prerelease_version(version),
            Language::Ruby => version_info::is_ruby_prerelease_version(version),
            _ => is_prerelease_version(version),
        };

        if is_prerelease(dependency.version()) {
            available_versions.iter().collect()
        } else {
            available_versions
                .iter()
                .filter(|v| !is_prerelease(&v.version))
                .collect()
        }
    }

    /// `min_age` が設定されていれば、現在時刻から逆算したリリース時刻以前のものだけを残す。
    fn apply_age_filter<'a>(&self, candidates: Vec<&'a VersionInfo>) -> Vec<&'a VersionInfo> {
        let Some(min_age) = self.filter.min_age else {
            return candidates;
        };
        // chrono::Duration は i64 ナノ秒 (約292年) が上限。変換失敗時は age 制約を無視して全候補を通す。
        let Ok(chrono_duration) = chrono::Duration::from_std(min_age) else {
            return candidates;
        };
        let min_release_time = self.now - chrono_duration;
        candidates
            .into_iter()
            .filter(|v| v.released_at <= min_release_time)
            .collect()
    }
}

/// Range 制約の上限を超える候補を除外する。
/// 例: `">=3.5.0,<4.0.0"` なら 4.0.0 以上を除外する。
fn apply_range_upper_bound<'a>(
    dependency: &Dependency,
    candidates: Vec<&'a VersionInfo>,
) -> Vec<&'a VersionInfo> {
    if dependency.version_spec.kind != VersionSpecKind::Range {
        return candidates;
    }
    let Some((upper_bound, inclusive)) = extract_upper_bound(&dependency.version_spec.raw) else {
        return candidates;
    };
    candidates
        .into_iter()
        .filter(
            |v| match compare_dependency_versions(dependency, &v.version, &upper_bound) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => inclusive,
                std::cmp::Ordering::Greater => false,
            },
        )
        .collect()
}

fn matches_rejected_version(dependency: &Dependency, candidate: &str, rejected: &str) -> bool {
    let rejected = rejected.trim();
    if rejected.is_empty() {
        return false;
    }

    if rejected == "+" {
        return true;
    }

    if let Some(prefix) = rejected.strip_suffix('+') {
        return candidate.starts_with(prefix);
    }

    if let Some(captures) = MAVEN_RANGE_RE.captures(rejected) {
        let lower = captures.get(2).map(|value| value.as_str());
        let upper = captures.get(3).map(|value| value.as_str());
        let lower_inclusive = captures.get(1).map(|value| value.as_str()) == Some("[");
        let upper_inclusive = captures.get(4).map(|value| value.as_str()) == Some("]");

        let above_lower = lower.is_none_or(|bound| {
            match compare_dependency_versions(dependency, candidate, bound) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => lower_inclusive,
                std::cmp::Ordering::Less => false,
            }
        });
        let below_upper = upper.is_none_or(|bound| {
            match compare_dependency_versions(dependency, candidate, bound) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => upper_inclusive,
                std::cmp::Ordering::Greater => false,
            }
        });
        return above_lower && below_upper;
    }

    if dependency.language == Language::Go {
        return compare_dependency_versions(dependency, candidate, rejected)
            == std::cmp::Ordering::Equal;
    }

    candidate == rejected
}

/// Gradle rich version の `reject` で指定された候補を除外する。
fn apply_rejected_versions<'a>(
    dependency: &Dependency,
    candidates: Vec<&'a VersionInfo>,
) -> Vec<&'a VersionInfo> {
    if dependency.version_spec.rejected_versions.is_empty() {
        return candidates;
    }

    candidates
        .into_iter()
        .filter(|candidate| {
            !dependency
                .version_spec
                .rejected_versions
                .iter()
                .any(|rejected| matches_rejected_version(dependency, &candidate.version, rejected))
        })
        .collect()
}

/// `--max-change` で許容レベルを超える候補を除外する。
/// 比較不能 / 同一バージョンの候補は通す。
fn apply_max_change_filter<'a>(
    dependency: &Dependency,
    eligible: &[&'a VersionInfo],
    max_change: Option<crate::domain::ChangeLevel>,
) -> Vec<&'a VersionInfo> {
    let Some(max) = max_change else {
        return eligible.to_vec();
    };
    let current = dependency.version();
    eligible
        .iter()
        .copied()
        .filter(|v| {
            crate::domain::ChangeLevel::from_versions(current, &v.version)
                .is_none_or(|level| level <= max)
        })
        .collect()
}

pub fn compare_dependency_versions(
    dependency: &Dependency,
    a: &str,
    b: &str,
) -> std::cmp::Ordering {
    match dependency.language {
        Language::Node | Language::Rust | Language::Go | Language::Swift => {
            version_info::compare_semver_versions(a, b)
        }
        Language::Python => version_info::compare_python_versions(a, b),
        Language::Ruby => version_info::compare_ruby_versions(a, b),
        Language::Php => version_info::compare_composer_versions(a, b),
        Language::Java => version_info::compare_gradle_versions(a, b),
    }
}

/// 最新候補を選び、ダウングレード防止・更新先制約フォーマット可否を判定して結果を返す。
fn select_latest_candidate(
    dependency: &Dependency,
    eligible: &[&VersionInfo],
    allowed: &[&VersionInfo],
    max_change: Option<crate::domain::ChangeLevel>,
) -> UpdateResult {
    // 言語ごとの比較規則で最新の更新候補を選ぶ
    let latest = allowed
        .iter()
        .max_by(|a, b| compare_dependency_versions(dependency, &a.version, &b.version))
        .unwrap();

    // 現在版が最新以上ならダウングレードを防いでスキップする
    if compare_dependency_versions(dependency, dependency.version(), &latest.version)
        != std::cmp::Ordering::Less
    {
        // max_change で除外された「より新しい候補」が存在すれば ChangeLevelLimited
        if let Some(max) = max_change {
            let current = dependency.version();
            let has_newer_excluded = eligible.iter().any(|v| {
                compare_dependency_versions(dependency, &v.version, current)
                    == std::cmp::Ordering::Greater
                    && crate::domain::ChangeLevel::from_versions(current, &v.version)
                        .is_some_and(|level| level > max)
            });
            if has_newer_excluded {
                return UpdateResult::skip(dependency.clone(), SkipReason::ChangeLevelLimited(max));
            }
        }
        return UpdateResult::skip_already_latest_with_date(dependency.clone(), latest.released_at);
    }

    // 更新先の文字列表現を安全に組み立てられない制約は更新対象にしない
    let Some(formatted) = dependency.version_spec.try_format_updated(&latest.version) else {
        return UpdateResult::skip(
            dependency.clone(),
            SkipReason::ParseError("constraint cannot be updated safely".to_string()),
        );
    };

    // 書き換え結果が現在の raw と同一なら、マニフェスト上は何も変わらない
    // phantom update (例: Wildcard `1.x` の範囲内に最新版がある場合)。
    // writer が no-op なのに毎回「更新あり」と報告し続けないよう AlreadyLatest にする。
    if formatted == dependency.version_spec.raw {
        return UpdateResult::skip_already_latest_with_date(dependency.clone(), latest.released_at);
    }

    UpdateResult::update_with_date(dependency.clone(), &latest.version, latest.released_at)
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
        let filter = UpdateFilter::new().with_min_age(Duration::from_secs(7 * 24 * 60 * 60)); // 7日
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
    fn test_should_skip_only_matches_manifest_name() {
        let filter = UpdateFilter::new().with_only(vec!["tokio_v1".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep =
            make_dependency("tokio", "1.0.0", Language::Rust, false).with_manifest_name("tokio_v1");
        assert!(judge.should_skip(&dep).is_none());
    }

    #[test]
    fn test_should_skip_exclude_matches_manifest_name() {
        let filter = UpdateFilter::new().with_exclude(vec!["tokio_v1".to_string()]);
        let judge = UpdateJudge::new(filter);

        let dep =
            make_dependency("tokio", "1.0.0", Language::Rust, false).with_manifest_name("tokio_v1");
        assert_eq!(judge.should_skip(&dep), Some(SkipReason::Excluded));
    }

    #[test]
    fn test_should_skip_only_takes_precedence_over_exclude() {
        let filter = UpdateFilter::new()
            .with_only(vec!["lodash".to_string()])
            .with_exclude(vec!["lodash".to_string()]);
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
        let filter = UpdateFilter::new(); // include_pinned は false
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
        let filter = UpdateFilter::new(); // include_pinned は false
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
    fn test_judge_python_pep440_rc_without_separator_not_chosen() {
        // 回帰テスト: PyPI が返すセパレータなし rc (例: 2.0.0rc1) を
        // 安定版ユーザーへ誤って更新候補として選ばない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("urllib3", "1.9.0", Language::Python, false);
        let versions = vec![
            make_version_info("1.9.0", 100),
            make_version_info("2.0.0rc1", 10), // PEP 440 セパレータなし rc は prerelease
        ];

        let result = judge.judge(&dep, &versions);
        // 安定版候補は現状版だけなので更新されない (rc へ誤更新しない)
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_python_pep440_preview_spelling_not_chosen_for_stable_user() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("django", "1.0", Language::Python, false);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("2.0preview1", 10),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_python_pep440_prefers_stable_over_rc() {
        // 安定版と rc が両方ある場合は安定版を選ぶ
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("urllib3", "1.9.0", Language::Python, false);
        let versions = vec![
            make_version_info("1.9.0", 100),
            make_version_info("2.0.0rc1", 20), // prerelease なので無視する
            make_version_info("2.0.0", 10),    // 安定版なので選ばれる
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
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
    fn test_extract_upper_bound_pep440_prefix_match() {
        // PEP 440 の prefix-match wildcard (`==1.2.*`) は `<1.3` 相当の排他的上限を持つ
        assert_eq!(
            super::extract_upper_bound("==1.2.*"),
            Some(("1.3".to_string(), false))
        );
        // `==1.*` は `<2` 相当
        assert_eq!(
            super::extract_upper_bound("==1.*"),
            Some(("2".to_string(), false))
        );
        // `==1.2.3.*` は `<1.2.4` 相当 (最後のリリースセグメントを +1)
        assert_eq!(
            super::extract_upper_bound("==1.2.3.*"),
            Some(("1.2.4".to_string(), false))
        );
        // 空白付き / `v` 接頭辞付きも許容する
        assert_eq!(
            super::extract_upper_bound("== 1.2.*"),
            Some(("1.3".to_string(), false))
        );
        // epoch 付き `1!2.3.*` は epoch を保持して上限を導出する
        assert_eq!(
            super::extract_upper_bound("==1!2.3.*"),
            Some(("1!2.4".to_string(), false))
        );
        // `!=1.2.*` は除外制約であり上限ではないため None
        assert_eq!(super::extract_upper_bound("!=1.2.*"), None);
        // 末尾が `.*` でない通常の `==1.2.3` は上限を持たない
        assert_eq!(super::extract_upper_bound("==1.2.3"), None);
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
    fn test_judge_pep440_prefix_match_respects_upper_bound() {
        // PEP 440 の `==1.2.*` は `>=1.2.0, <1.3.0` 相当。
        // 上限が効かないと judge が 2.5.0 を選び `==2.5.*` へ誤更新してしまうため、
        // 1.3 以上の候補が除外され major/minor を跨がないことを確認する。
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("requests", "==1.2.*", "1.2", Language::Python);
        let versions = vec![
            make_version_info("1.2.0", 100),
            make_version_info("1.2.5", 80), // `1.2.*` の範囲内 (書き換え後も `==1.2.*` なので phantom)
            make_version_info("1.3.0", 50), // 排他的上限 1.3 以上なので除外
            make_version_info("2.5.0", 10), // major 跨ぎ、除外されるべき
        ];

        let result = judge.judge(&dep, &versions);
        // `==1.2.*` の文字列は 1.2 系内では変化しないため AlreadyLatest (更新なし) になる。
        // 上限が無いと 2.5.0 への Update になってしまうので、ここは更新なしが正しい。
        assert!(!result.is_update());
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
        // Maven の代替記法 `]1.0,2.0[` は下限排他なので安全に書き換えられない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("org.example:lib", "]1.0,2.0[", "1.0", Language::Java);
        let versions = vec![
            make_version_info("1.0", 100),
            make_version_info("1.9", 50),
            make_version_info("2.0", 10), // 排他的上限なので除外
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
    fn test_judge_gradle_strict_range_prefer_respects_upper_bound() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency(
            "org.slf4j:slf4j-api",
            "[1.7, 1.8[!!1.7.25",
            "1.7.25",
            Language::Java,
        );
        let versions = vec![
            make_version_info("1.7.36", 50),
            make_version_info("1.8.0", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.7.36");
        }
    }

    #[test]
    fn test_judge_rejected_versions_are_excluded() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0[", "1.5")
            .with_rejected_versions(["1.7"]);
        let dep = Dependency::new("org.example:demo", spec, false, Language::Java);
        let versions = vec![make_version_info("1.6", 50), make_version_info("1.7", 20)];

        let result = judge.judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.6");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_judge_rejected_dynamic_versions_are_excluded() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,3.0[", "1.5")
            .with_rejected_versions(["2.+"]);
        let dep = Dependency::new("org.example:demo", spec, false, Language::Java);
        let versions = vec![make_version_info("1.9", 50), make_version_info("2.1", 20)];

        let result = judge.judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.9");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_judge_rejected_version_range_is_excluded() {
        let judge = UpdateJudge::new(UpdateFilter::new());
        let spec = VersionSpec::new(VersionSpecKind::Range, "[1.0,2.0)", "1.4")
            .with_rejected_versions(["[1.5,1.9)"]);
        let dep = Dependency::new("org.example:demo", spec, false, Language::Java);
        let versions = vec![
            make_version_info("1.5", 50),
            make_version_info("1.8", 30),
            make_version_info("1.9", 20),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(matches!(
            result,
            UpdateResult::Update { ref new_version, .. } if new_version == "1.9"
        ));
        assert!(matches_rejected_version(&dep, "1.5", "[1.5,1.9)"));
        assert!(matches_rejected_version(&dep, "1.8", "[1.5,1.9)"));
        assert!(!matches_rejected_version(&dep, "1.9", "[1.5,1.9)"));
    }

    #[test]
    fn test_judge_range_upper_bound_first_updates_lower_bound() {
        // 上限制約が先に書かれていても、更新するのは下限側だけ
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("paramiko", "<4.0.0,>=3.5.0", "3.5.0", Language::Python);
        let versions = vec![
            make_version_info("3.5.0", 100),
            make_version_info("3.9.0", 20),
            make_version_info("4.0.0", 10), // 上限制約により除外される
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "3.9.0");
        }
    }

    #[test]
    fn test_judge_range_exclusive_lower_bound_skips_with_parse_error() {
        // `>最新候補` に書き換えると最新候補自身が制約を満たさない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let dep = make_range_dependency("package", ">1.0,<2.0", "1.0", Language::Python);
        let versions = vec![make_version_info("1.9", 20), make_version_info("2.0", 10)];

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
    fn test_judge_strict_greater_skips_with_parse_error() {
        // 単独の `>` は最新候補へ更新すると `>最新候補` になり、解決不能になり得る
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::Greater, ">1.0", "1.0").with_prefix(">");
        let dep = Dependency::new("package", spec, false, Language::Python);
        let versions = vec![make_version_info("2.0", 20)];

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
    fn test_judge_upper_bound_only_skips_with_parse_error() {
        // 単独の `<` / `<=` は上限だけの制約なので、自動更新で上限を広げない
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::LessOrEqual, "<=2.0", "2.0").with_prefix("<=");
        let dep = Dependency::new("package", spec, false, Language::Python);
        let versions = vec![make_version_info("3.0", 20)];

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
    fn test_judge_pep440_compatible_release_respects_upper_bound() {
        // PEP 440 の `~=1.2.3` は `>=1.2.3, <1.3.0` 相当。1.3.0 以上は除外される
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);
        let dep = make_range_dependency("requests", "~=1.2.3", "1.2.3", Language::Python);
        let versions = vec![
            make_version_info("1.2.3", 100),
            make_version_info("1.2.9", 80), // 上限内の最新
            make_version_info("1.3.0", 50), // 排他的上限なので除外
            make_version_info("2.0.0", 10), // major 跨ぎで除外
        ];
        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.2.9");
        }
    }

    #[test]
    fn test_judge_pep440_compatible_release_two_part_respects_upper_bound() {
        // PEP 440 の `~=1.2` は `>=1.2, <2.0` 相当。2.0 以上は除外される
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);
        let dep = make_range_dependency("requests", "~=1.2", "1.2", Language::Python);
        let versions = vec![
            make_version_info("1.2", 100),
            make_version_info("1.9", 50), // 上限内の最新
            make_version_info("2.0", 10), // 排他的上限なので除外
        ];
        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.9");
        }
    }

    #[test]
    fn test_judge_mixed_lower_bounds_without_upper_skips_safely() {
        // `>=1.2.3, ^1.3` のように上限 (`<`) のない複数下限の混在は、
        // 下限だけ進めると充足不能になり得るため安全に書き換えられず Skip する
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);
        let dep = make_range_dependency("some_crate", ">=1.2.3, ^1.3", "1.3", Language::Rust);
        let versions = vec![
            make_version_info("1.3.0", 50),
            make_version_info("1.5.0", 20),
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
    fn test_extract_upper_bound_maven_multi_part_qualifier() {
        // 複数区切りの qualifier 付き上限
        let result = extract_upper_bound("[1.0,2.0-beta1-SNAPSHOT)");
        assert_eq!(result, Some(("2.0-beta1-SNAPSHOT".to_string(), false)));
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
    fn test_judge_go_excluded_version_is_not_selected() {
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GoPinned, "v1.9.1", "1.9.1")
            .with_rejected_versions(["v1.10.0"]);
        let dep = Dependency::new("example.com/module", spec, false, Language::Go);
        let versions = vec![
            make_version_info("1.9.2", 20),
            make_version_info("1.10.0", 10),
        ];

        let result = judge.judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.9.2");
            }
            other => panic!("予期しない更新判定: {other:?}"),
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

    /// 回帰テスト: `<=` regex が先に勝って緩い上限を返さない。
    /// `>=1,<2,<=3` では最も厳しい `<2` (排他) を採用する。
    #[test]
    fn test_extract_upper_bound_multiple_bounds_picks_strictest() {
        assert_eq!(
            super::extract_upper_bound(">=1,<2,<=3"),
            Some(("2".to_string(), false))
        );
        // 順序を入れ替えても同じ結果になる
        assert_eq!(
            super::extract_upper_bound(">=1,<=3,<2"),
            Some(("2".to_string(), false))
        );
        // 同値の上限が `<` と `<=` で並ぶ場合は排他的 (`<`) を優先する
        assert_eq!(
            super::extract_upper_bound(">=1,<2,<=2"),
            Some(("2".to_string(), false))
        );
        // `<=` のみ複数なら最小の包含上限
        assert_eq!(
            super::extract_upper_bound(">=1,<=3,<=2.5"),
            Some(("2.5".to_string(), true))
        );
    }

    /// 回帰テスト: Maven の qualifier 付き下限 `[1.0-2,2.0)` がハイフンレンジに
    /// 誤マッチして誤った上限 (充足不能レンジ) を返さない。
    /// Maven 形式 (完全アンカー付き) を最初に評価する。
    #[test]
    fn test_extract_upper_bound_maven_qualifier_lower_bound_not_hyphen_range() {
        assert_eq!(
            super::extract_upper_bound("[1.0-2,2.0)"),
            Some(("2.0".to_string(), false))
        );
        assert_eq!(
            super::extract_upper_bound("[1.0-rc1,2.0]"),
            Some(("2.0".to_string(), true))
        );
    }

    /// 回帰テスト: ハイフンレンジは npm/Composer 仕様どおりスペース必須。
    /// スペースなしハイフン (`1.0-2`) はハイフンレンジとして解釈しない。
    #[test]
    fn test_extract_upper_bound_hyphen_requires_spaces() {
        // スペースなしハイフンは prerelease/qualifier 付きバージョンであり、
        // 上限制約を持たない
        assert_eq!(super::extract_upper_bound("1.0-2"), None);
        assert_eq!(super::extract_upper_bound(">=1.0-beta"), None);
        // スペース付きは従来どおりハイフンレンジ
        assert_eq!(
            super::extract_upper_bound("1.0 - 2.0"),
            Some(("2.1".to_string(), false))
        );
    }

    /// 回帰テスト (phantom update): 書き換え結果が raw と同値になる場合は
    /// 「更新あり」と報告し続けず AlreadyLatest として扱う。
    /// 例: Wildcard `1.x` の範囲内に最新版 (1.9.3) がある場合、writer は no-op。
    #[test]
    fn test_judge_wildcard_phantom_update_reports_already_latest() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // node パーサ同様、Wildcard spec の version には先頭の数値部分が入る
        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.x", "1");
        let dep = Dependency::new("lodash", spec, false, Language::Node);
        let versions = vec![
            make_version_info("1.0.0", 100),
            make_version_info("1.9.3", 10), // `1.x` 範囲内の最新 → 書き換え不要
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
            // 最新候補のリリース日時が添えられる
            assert!(released_at.is_some());
        }
    }

    #[test]
    fn test_judge_wildcard_updates_when_shape_changes() {
        // `1.x` → `2.x` のように文字列が変わる場合は通常どおり更新される
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::Wildcard, "1.x", "1");
        let dep = Dependency::new("lodash", spec, false, Language::Node);
        let versions = vec![
            make_version_info("1.9.3", 50),
            make_version_info("2.4.1", 10), // `2.x` へ形が変わる
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.4.1");
        }
    }

    /// 回帰テスト: 「現在版より新しい候補がそもそも無い」場合は
    /// `--max-change` 指定があっても ChangeLevelLimited ではなく AlreadyLatest。
    #[test]
    fn test_judge_max_change_all_older_candidates_already_latest() {
        use crate::domain::ChangeLevel;
        let filter = UpdateFilter::new().with_max_change(ChangeLevel::Patch);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "2.0.0", Language::Node, false);
        // 候補は全部古い。major 差のため max_change=patch で allowed は空になるが、
        // 新しい候補が無いだけなので AlreadyLatest が正しい
        let versions = vec![make_version_info("1.0.0", 100)];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip {
            reason,
            released_at,
            ..
        } = result
        {
            assert_eq!(reason, SkipReason::AlreadyLatest);
            assert!(released_at.is_some());
        }
    }

    #[test]
    fn test_judge_max_change_newer_excluded_reports_change_level_limited() {
        use crate::domain::ChangeLevel;
        // 新しい候補が max_change で除外された場合は従来どおり ChangeLevelLimited
        let filter = UpdateFilter::new().with_max_change(ChangeLevel::Patch);
        let judge = UpdateJudge::new(filter);

        let dep = make_dependency("lodash", "2.0.0", Language::Node, false);
        let versions = vec![make_version_info("3.0.0", 10)]; // major 差で除外

        let result = judge.judge(&dep, &versions);
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::ChangeLevelLimited(ChangeLevel::Patch));
        }
    }

    /// 回帰テスト (Python rc 利用者の安定版昇格): 現在版が `2.0.0rc1` のとき、
    /// 安定版 2.0.0 への更新が AlreadyLatest にならず Update と判定される。
    /// parser/python.rs が prerelease 部を保持するようになったことと対になるテスト。
    #[test]
    fn test_judge_python_rc_user_upgrades_to_stable() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        // `>=2.0.0rc1` 相当: 比較基準は prerelease を保持した "2.0.0rc1"
        let spec = VersionSpec::new(VersionSpecKind::GreaterOrEqual, ">=2.0.0rc1", "2.0.0rc1")
            .with_prefix(">=");
        let dep = Dependency::new("django", spec, false, Language::Python);
        let versions = vec![
            make_version_info("2.0.0rc1", 30),
            make_version_info("2.0.0", 10), // 安定版が出ている
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update(), "rc 利用者は安定版へ昇格できるべき");
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0");
        }
    }

    #[test]
    fn test_judge_python_rc_user_stays_when_no_stable() {
        // 安定版が無ければ rc のまま (より新しい rc があればそちらへ)
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::GreaterOrEqual, ">=2.0.0rc1", "2.0.0rc1")
            .with_prefix(">=");
        let dep = Dependency::new("django", spec, false, Language::Python);
        let versions = vec![
            make_version_info("2.0.0rc1", 30),
            make_version_info("2.0.0rc2", 10),
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "2.0.0rc2");
        }
    }

    #[test]
    fn test_judge_python_local_version_uses_pep440_ordering() {
        let filter = UpdateFilter::new().with_include_pinned(true);
        let judge = UpdateJudge::new(filter);

        let spec =
            VersionSpec::new(VersionSpecKind::Exact, "==1.0+abc", "1.0+abc").with_prefix("==");
        let dep = Dependency::new("torch", spec, false, Language::Python);
        let versions = vec![
            make_version_info("1.0+abc.1", 30),
            make_version_info("1.0+1", 10), // 数値 local セグメントは英字より新しい
        ];

        let result = judge.judge(&dep, &versions);
        assert!(result.is_update());
        if let UpdateResult::Update { new_version, .. } = result {
            assert_eq!(new_version, "1.0+1");
        }
    }

    /// 回帰テスト (semver 11.4): プレリリース利用者が beta → alpha へ
    /// 実質ダウングレードされない (alpha.24 < beta.2)。
    #[test]
    fn test_judge_prerelease_user_not_downgraded_to_alpha() {
        let filter = UpdateFilter::new();
        let judge = UpdateJudge::new(filter);

        let spec = VersionSpec::new(VersionSpecKind::Caret, "^6.0.0-beta.2", "6.0.0-beta.2")
            .with_prefix("^");
        let dep = Dependency::new("some-lib", spec, false, Language::Node);
        let versions = vec![
            make_version_info("6.0.0-alpha.24", 5), // beta より古い段階
            make_version_info("6.0.0-beta.2", 10),
        ];

        let result = judge.judge(&dep, &versions);
        // beta.2 が最新 (alpha.24 < beta.2) なのでスキップする
        assert!(result.is_skip());
        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::AlreadyLatest);
        }
    }

    #[test]
    fn test_judge_ruby_unknown_prerelease_is_filtered_for_stable_user() {
        let judge = UpdateJudge::new(UpdateFilter::new());
        let dependency = make_dependency("rack", "0.9.0", Language::Ruby, false);
        let versions = vec![
            make_version_info("0.9.1", 10),
            make_version_info("1.0.zeta", 10),
        ];

        let result = judge.judge(&dependency, &versions);
        assert!(matches!(
            result,
            UpdateResult::Update { ref new_version, .. } if new_version == "0.9.1"
        ));
    }

    #[test]
    fn test_compare_dependency_versions_uses_ecosystem_rules() {
        use std::cmp::Ordering;

        let node = make_dependency("node", "1.0.0-1", Language::Node, false);
        assert_eq!(
            compare_dependency_versions(&node, "1.0.0-1", "1.0.0"),
            Ordering::Less
        );

        let ruby = make_dependency("ruby", "1.0.zeta", Language::Ruby, false);
        assert_eq!(
            compare_dependency_versions(&ruby, "1.0.zeta", "1.0"),
            Ordering::Less
        );

        let php = make_dependency("php", "1.0.0-p1", Language::Php, false);
        assert_eq!(
            compare_dependency_versions(&php, "1.0.0-p1", "1.0.0"),
            Ordering::Greater
        );

        let java = make_dependency("java", "1.0-rc", Language::Java, false);
        assert_eq!(
            compare_dependency_versions(&java, "1.0-zeta", "1.0-rc"),
            Ordering::Less
        );
    }

    #[test]
    fn test_judge_semver_numeric_prerelease_is_filtered_for_stable_user() {
        let judge = UpdateJudge::new(UpdateFilter::new());
        let dep = make_dependency("demo", "0.9.0", Language::Node, false);
        let result = judge.judge(&dep, &[make_version_info("1.0.0-1", 10)]);

        assert!(matches!(
            result,
            UpdateResult::Skip {
                reason: SkipReason::NoSuitableVersion,
                ..
            }
        ));
    }

    #[test]
    fn test_judge_semver_numeric_prerelease_upgrades_to_stable() {
        let judge = UpdateJudge::new(UpdateFilter::new());
        let dep = make_dependency("demo", "1.0.0-1", Language::Rust, false);
        let versions = vec![
            make_version_info("1.0.0-1", 20),
            make_version_info("1.0.0", 10),
        ];
        let result = judge.judge(&dep, &versions);

        assert!(matches!(
            result,
            UpdateResult::Update { ref new_version, .. } if new_version == "1.0.0"
        ));
    }

    #[test]
    fn test_judge_gradle_does_not_downgrade_rc_to_ordinary_qualifier() {
        let judge = UpdateJudge::new(UpdateFilter::new());
        let dep = make_dependency("org.example:demo", "1.0-rc", Language::Java, false);
        let versions = vec![
            make_version_info("1.0-zeta", 10),
            make_version_info("1.0-rc", 20),
        ];
        let result = judge.judge(&dep, &versions);

        assert!(matches!(
            result,
            UpdateResult::Skip {
                reason: SkipReason::AlreadyLatest,
                ..
            }
        ));
    }

    #[test]
    fn test_judge_composer_patch_alias_is_not_downgraded() {
        let judge = UpdateJudge::new(UpdateFilter::new());
        let dep = make_dependency("vendor/demo", "1.0.0-p1", Language::Php, false);
        let versions = vec![make_version_info("1.0.0", 10)];
        let result = judge.judge(&dep, &versions);

        assert!(matches!(
            result,
            UpdateResult::Skip {
                reason: SkipReason::AlreadyLatest,
                ..
            }
        ));
    }
}
