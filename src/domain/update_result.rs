//! 更新判定結果の型定義

use super::{ChangeLevel, Dependency};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// install の単位で取り消した更新。復元未完了の候補を未適用と断言しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCancellation {
    pub lock_verification: bool,
    pub recovered: bool,
}

/// 更新候補を OSV で除外した結果。安全性の照会は版の選択条件を緩めない。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsvBlocked {
    pub skipped_versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    pub current_vulnerable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_advisories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_check_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_after_age: Option<AgeWaitingVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_check_error: Option<String>,
}

/// ほかの更新条件と OSV を満たすが、公開後の待機期間が残る版。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgeWaitingVersion {
    pub version: String,
    pub eligible_at: DateTime<Utc>,
}

/// 依存関係の更新がスキップされた理由
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// バージョンがピン留めされている (完全一致バージョン指定)
    Pinned,
    /// 既に最新バージョン
    AlreadyLatest,
    /// 現在版の公開日が age の境界より新しい
    CurrentVersionTooRecent,
    /// --exclude フラグで除外された
    Excluded,
    /// --only リストに含まれていない
    NotInOnlyList,
    /// 実効的な pnpm override がこの宣言を制御している。override 自体は変更しない。
    PnpmOverride(String),
    /// レジストリからのバージョン情報取得に失敗
    FetchFailed(String),
    /// 適切なバージョンが見つからない (例: 経過日数フィルタで全バージョンが除外)
    NoSuitableVersion,
    /// 更新可能な版をすべて既知の脆弱性で除外した。
    KnownVulnerabilities(OsvBlocked),
    /// 更新候補の公開日が不明で、age 制約を確認できない
    ReleaseDateUnknown(String),
    /// 公開日を個別に取得するレジストリで、取得の上限に達し、この版とそれより古い候補の
    /// 公開日を確かめていない (公開日不明とは確かめていない)
    ReleaseDateUnchecked(String),
    /// バージョンのパースに失敗
    ParseError(String),
    /// 言語フィルタで除外された
    LanguageFiltered,
    /// 採用候補が `--max-change` の許容範囲を超えるためスキップ
    /// (内側の値は `max-change` に指定されたレベル)
    ChangeLevelLimited(ChangeLevel),
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Pinned => write!(f, "pinned version"),
            SkipReason::AlreadyLatest => write!(f, "already at latest"),
            SkipReason::CurrentVersionTooRecent => write!(f, "current version is newer than --age"),
            SkipReason::Excluded => write!(f, "excluded by --exclude"),
            SkipReason::NotInOnlyList => write!(f, "not in --only list"),
            SkipReason::PnpmOverride(message) => write!(f, "{message}"),
            SkipReason::FetchFailed(msg) => write!(f, "fetch failed: {}", msg),
            SkipReason::NoSuitableVersion => write!(f, "no suitable version"),
            SkipReason::KnownVulnerabilities(_) => {
                write!(f, "no eligible update without known vulnerabilities")
            }
            SkipReason::ReleaseDateUnknown(version) => {
                write!(f, "release date unavailable for {version}")
            }
            SkipReason::ReleaseDateUnchecked(version) => {
                write!(
                    f,
                    "release date lookup limit reached; {version} and older not checked"
                )
            }
            SkipReason::ParseError(msg) => write!(f, "parse error: {}", msg),
            SkipReason::LanguageFiltered => write!(f, "language filtered"),
            SkipReason::ChangeLevelLimited(level) => {
                write!(f, "limited by --max-change={}", level)
            }
        }
    }
}

/// 単一依存関係の更新判定結果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateResult {
    /// 依存関係が更新される
    Update {
        /// 更新対象の依存関係
        dependency: Dependency,
        /// 更新先の新バージョン
        new_version: String,
        /// 新バージョンのリリース日時
        #[serde(skip_serializing_if = "Option::is_none")]
        released_at: Option<DateTime<Utc>>,
        /// OSV チェックで脆弱と判定され、採用しなかった候補のリスト
        /// (新しい順)。各要素は `"<version> (<advisory ids>)"` の形式。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        osv_skipped: Vec<String>,
        /// 採用版が OSV で検査され脆弱性なしと判定されたか。
        /// `false` は OSV 未実行 (`--osv` 無効、または Swift 等の未対応エコシステム、
        /// API エラー時)、または脆弱性が残る交換ルールでの採用を意味する。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        osv_checked: bool,
        /// 脆弱性が残るが、解消した問題の重大度と件数を根拠に許容した更新。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        osv_risk_tradeoff: Option<crate::osv::RiskTradeoff>,
        /// age を免除した身元と、レジストリが確認した根拠。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        age_exemption: Option<crate::update::AgeExemption>,
        /// マニフェストへ書き込めなかったときの理由 (dry-run では書き込めない見込みの理由)。
        /// install 後に更新先の解決版を確認できなかった場合も記録する。
        /// 書き込みの段階より前は常に `None`
        #[serde(default, skip_serializing_if = "Option::is_none")]
        write_error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancellation: Option<UpdateCancellation>,
    },
    /// 依存関係の更新がスキップされた
    Skip {
        /// スキップされた依存関係
        dependency: Dependency,
        /// スキップの理由
        reason: SkipReason,
        /// 現在版自身の公開日時 (AlreadyLatest / CurrentVersionTooRecent)。不明なら省略
        #[serde(skip_serializing_if = "Option::is_none")]
        released_at: Option<DateTime<Utc>>,
    },
}

impl UpdateResult {
    pub fn with_age_exemption(mut self, exemption: Option<crate::update::AgeExemption>) -> Self {
        if let Self::Update { age_exemption, .. } = &mut self {
            *age_exemption = exemption;
        }
        self
    }

    /// リリース日なしのUpdate結果を作成する (後方互換性のため)
    pub fn update(dependency: Dependency, new_version: impl Into<String>) -> Self {
        UpdateResult::Update {
            dependency,
            new_version: new_version.into(),
            released_at: None,
            osv_skipped: Vec::new(),
            osv_checked: false,
            osv_risk_tradeoff: None,
            age_exemption: None,
            write_error: None,
            cancellation: None,
        }
    }

    /// リリース日付きのUpdate結果を作成する
    pub fn update_with_date(
        dependency: Dependency,
        new_version: impl Into<String>,
        released_at: DateTime<Utc>,
    ) -> Self {
        Self::update_with_optional_date(dependency, new_version, Some(released_at))
    }

    /// 公開日の有無を保った Update 結果を作成する
    pub fn update_with_optional_date(
        dependency: Dependency,
        new_version: impl Into<String>,
        released_at: Option<DateTime<Utc>>,
    ) -> Self {
        UpdateResult::Update {
            dependency,
            new_version: new_version.into(),
            released_at,
            osv_skipped: Vec::new(),
            osv_checked: false,
            osv_risk_tradeoff: None,
            age_exemption: None,
            write_error: None,
            cancellation: None,
        }
    }

    /// OSV チェックで除外した候補リストを Update 結果に紐付ける。
    /// `Skip` 結果に対しては no-op。
    pub fn with_osv_skipped(mut self, skipped: Vec<String>) -> Self {
        if let UpdateResult::Update {
            ref mut osv_skipped,
            ..
        } = self
        {
            *osv_skipped = skipped;
        }
        self
    }

    /// 採用版が OSV で検査済みであることを示すマーカーをセットする。
    /// `Skip` 結果に対しては no-op。
    pub fn with_osv_checked(mut self, checked: bool) -> Self {
        if let UpdateResult::Update {
            ref mut osv_checked,
            ..
        } = self
        {
            *osv_checked = checked;
        }
        self
    }

    pub fn with_osv_risk_tradeoff(mut self, reduction: crate::osv::RiskTradeoff) -> Self {
        if let Self::Update {
            osv_risk_tradeoff,
            osv_checked,
            ..
        } = &mut self
        {
            *osv_risk_tradeoff = Some(reduction);
            *osv_checked = false;
        }
        self
    }

    /// Skip結果を作成する
    pub fn skip(dependency: Dependency, reason: SkipReason) -> Self {
        UpdateResult::Skip {
            dependency,
            reason,
            released_at: None,
        }
    }

    /// ピン留めバージョンのSkip結果を作成する
    pub fn skip_pinned(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::Pinned)
    }

    /// 既に最新のSkip結果を作成する
    pub fn skip_already_latest(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::AlreadyLatest)
    }

    /// リリース日付きの既に最新のSkip結果を作成する
    pub fn skip_already_latest_with_date(
        dependency: Dependency,
        released_at: DateTime<Utc>,
    ) -> Self {
        Self::skip_already_latest_with_optional_date(dependency, Some(released_at))
    }

    /// 公開日の有無を保った AlreadyLatest 結果を作成する
    pub fn skip_already_latest_with_optional_date(
        dependency: Dependency,
        released_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self::skip_with_optional_date(dependency, SkipReason::AlreadyLatest, released_at)
    }

    /// 現在版の公開日の有無を保った Skip 結果を作成する
    pub fn skip_with_optional_date(
        dependency: Dependency,
        reason: SkipReason,
        released_at: Option<DateTime<Utc>>,
    ) -> Self {
        UpdateResult::Skip {
            dependency,
            reason,
            released_at,
        }
    }

    /// 除外パッケージのSkip結果を作成する
    pub fn skip_excluded(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::Excluded)
    }

    /// onlyリスト外のSkip結果を作成する
    pub fn skip_not_in_only_list(dependency: Dependency) -> Self {
        Self::skip(dependency, SkipReason::NotInOnlyList)
    }

    /// フェッチ失敗のSkip結果を作成する
    pub fn skip_fetch_failed(dependency: Dependency, message: impl Into<String>) -> Self {
        Self::skip(dependency, SkipReason::FetchFailed(message.into()))
    }

    /// Update結果かどうかを返す。
    ///
    /// judge が更新すると決めたかどうかで、書き込みの成否は見ない。書き込めた更新だけを
    /// 数えるときは `write_error` も見る (`ManifestUpdateResult::updates` を参照)
    pub fn is_update(&self) -> bool {
        matches!(self, UpdateResult::Update { .. })
    }

    /// マニフェストへ書き込めなかった更新の理由を返す
    pub fn write_error(&self) -> Option<&str> {
        match self {
            UpdateResult::Update { write_error, .. } => write_error.as_deref(),
            UpdateResult::Skip { .. } => None,
        }
    }

    pub fn cancellation(&self) -> Option<UpdateCancellation> {
        match self {
            Self::Update { cancellation, .. } => *cancellation,
            Self::Skip { .. } => None,
        }
    }

    pub(crate) fn record_cancellation(&mut self, reason: String, cancellation: UpdateCancellation) {
        if let Self::Update {
            write_error,
            cancellation: state,
            ..
        } = self
        {
            *write_error = Some(reason);
            *state = Some(cancellation);
        }
    }

    /// マニフェストへ書き込めなかったことを記録する。`Skip` 結果に対しては no-op
    pub fn record_write_error(&mut self, reason: impl Into<String>) {
        if let UpdateResult::Update { write_error, .. } = self {
            *write_error = Some(reason.into());
        }
    }

    /// Skip結果かどうかを返す
    pub fn is_skip(&self) -> bool {
        matches!(self, UpdateResult::Skip { .. })
    }

    /// 依存関係の参照を返す
    pub fn dependency(&self) -> &Dependency {
        match self {
            UpdateResult::Update { dependency, .. } => dependency,
            UpdateResult::Skip { dependency, .. } => dependency,
        }
    }

    /// パッケージ名を返す
    pub fn package_name(&self) -> &str {
        &self.dependency().name
    }
}

impl fmt::Display for UpdateResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateResult::Update {
                dependency,
                new_version,
                released_at,
                ..
            } => {
                write!(
                    f,
                    "{}: {} → {}",
                    dependency.name,
                    dependency.version(),
                    new_version
                )?;
                if let Some(date) = released_at {
                    write!(f, " ({})", date.format("%Y/%m/%d %H:%M"))?;
                }
                Ok(())
            }
            UpdateResult::Skip {
                dependency, reason, ..
            } => {
                write!(f, "{}: skipped ({})", dependency.name, reason)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Language, VersionSpec, VersionSpecKind};

    fn sample_dependency() -> Dependency {
        Dependency::new(
            "lodash",
            VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^"),
            false,
            Language::Node,
        )
    }

    fn pinned_dependency() -> Dependency {
        Dependency::new(
            "lodash",
            VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3"),
            false,
            Language::Node,
        )
    }

    #[test]
    fn test_skip_reason_display() {
        assert_eq!(format!("{}", SkipReason::Pinned), "pinned version");
        assert_eq!(
            format!("{}", SkipReason::AlreadyLatest),
            "already at latest"
        );
        assert_eq!(format!("{}", SkipReason::Excluded), "excluded by --exclude");
        assert_eq!(
            format!("{}", SkipReason::NotInOnlyList),
            "not in --only list"
        );
        assert_eq!(
            format!("{}", SkipReason::FetchFailed("timeout".to_string())),
            "fetch failed: timeout"
        );
        assert_eq!(
            format!("{}", SkipReason::NoSuitableVersion),
            "no suitable version"
        );
        assert_eq!(
            format!("{}", SkipReason::ParseError("invalid".to_string())),
            "parse error: invalid"
        );
        assert_eq!(
            format!("{}", SkipReason::LanguageFiltered),
            "language filtered"
        );
    }

    #[test]
    fn current_release_age_skip_preserves_date_and_round_trips() {
        use chrono::TimeZone;
        let date = Utc.with_ymd_and_hms(2024, 5, 30, 12, 0, 0).unwrap();
        let reason = SkipReason::CurrentVersionTooRecent;
        assert_eq!(reason.to_string(), "current version is newer than --age");
        assert_eq!(
            serde_json::to_string(&reason).unwrap(),
            "\"current_version_too_recent\""
        );
        let result = UpdateResult::skip_with_optional_date(sample_dependency(), reason, Some(date));
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(serde_json::from_str::<UpdateResult>(&json).unwrap(), result);
        assert!(json.contains("2024-05-30T12:00:00Z"));
    }

    #[test]
    fn test_update_result_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep.clone(), "2.0.0");

        assert!(result.is_update());
        assert!(!result.is_skip());
        assert_eq!(result.package_name(), "lodash");

        if let UpdateResult::Update {
            dependency,
            new_version,
            released_at,
            osv_skipped,
            ..
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(new_version, "2.0.0");
            assert!(released_at.is_none()); // update() 使用時はリリース日なし
            assert!(osv_skipped.is_empty());
        } else {
            panic!("Expected Update variant");
        }
    }

    #[test]
    fn test_update_result_update_with_date() {
        use chrono::TimeZone;
        let dep = sample_dependency();
        let date = Utc.with_ymd_and_hms(2024, 6, 15, 10, 30, 0).unwrap();
        let result = UpdateResult::update_with_date(dep.clone(), "2.0.0", date);

        assert!(result.is_update());

        if let UpdateResult::Update {
            dependency,
            new_version,
            released_at,
            osv_skipped,
            ..
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(new_version, "2.0.0");
            assert_eq!(released_at, Some(date));
            assert!(osv_skipped.is_empty());
        } else {
            panic!("Expected Update variant");
        }
    }

    #[test]
    fn test_with_osv_skipped_attaches_to_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0").with_osv_skipped(vec![
            "2.0.1 (GHSA-xxxx)".to_string(),
            "2.0.0-rc1 (GHSA-yyyy)".to_string(),
        ]);
        match result {
            UpdateResult::Update { osv_skipped, .. } => {
                assert_eq!(osv_skipped.len(), 2);
                assert_eq!(osv_skipped[0], "2.0.1 (GHSA-xxxx)");
            }
            _ => panic!("Expected Update variant"),
        }
    }

    #[test]
    fn test_with_osv_skipped_noop_on_skip() {
        let dep = pinned_dependency();
        let result = UpdateResult::skip_pinned(dep).with_osv_skipped(vec!["1.0.0".to_string()]);
        // Skip 結果は変更されない
        assert!(result.is_skip());
    }

    #[test]
    fn test_update_result_skip() {
        let dep = sample_dependency();
        let result = UpdateResult::skip(dep.clone(), SkipReason::Excluded);

        assert!(!result.is_update());
        assert!(result.is_skip());
        assert_eq!(result.package_name(), "lodash");

        if let UpdateResult::Skip {
            dependency,
            reason,
            released_at,
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(reason, SkipReason::Excluded);
            assert!(released_at.is_none());
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_pinned() {
        let dep = pinned_dependency();
        let result = UpdateResult::skip_pinned(dep.clone());

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Pinned);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_already_latest() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_already_latest(dep);

        if let UpdateResult::Skip {
            reason,
            released_at,
            ..
        } = result
        {
            assert_eq!(reason, SkipReason::AlreadyLatest);
            assert!(released_at.is_none());
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_already_latest_with_date() {
        use chrono::TimeZone;
        let dep = sample_dependency();
        let date = Utc.with_ymd_and_hms(2025, 1, 15, 12, 30, 0).unwrap();
        let result = UpdateResult::skip_already_latest_with_date(dep.clone(), date);

        assert!(result.is_skip());
        if let UpdateResult::Skip {
            dependency,
            reason,
            released_at,
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(reason, SkipReason::AlreadyLatest);
            assert_eq!(released_at, Some(date));
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_excluded() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_excluded(dep);

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::Excluded);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_not_in_only_list() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_not_in_only_list(dep);

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::NotInOnlyList);
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_skip_fetch_failed() {
        let dep = sample_dependency();
        let result = UpdateResult::skip_fetch_failed(dep, "network error");

        if let UpdateResult::Skip { reason, .. } = result {
            assert_eq!(reason, SkipReason::FetchFailed("network error".to_string()));
        } else {
            panic!("Expected Skip variant");
        }
    }

    #[test]
    fn test_update_result_dependency() {
        let dep = sample_dependency();
        let update = UpdateResult::update(dep.clone(), "2.0.0");
        assert_eq!(update.dependency(), &dep);

        let skip = UpdateResult::skip(dep.clone(), SkipReason::Pinned);
        assert_eq!(skip.dependency(), &dep);
    }

    #[test]
    fn test_update_result_display_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0");
        assert_eq!(format!("{}", result), "lodash: 1.2.3 → 2.0.0");
    }

    #[test]
    fn test_update_result_display_update_with_date() {
        use chrono::TimeZone;
        let dep = sample_dependency();
        let date = Utc.with_ymd_and_hms(2024, 6, 15, 10, 30, 0).unwrap();
        let result = UpdateResult::update_with_date(dep, "2.0.0", date);
        assert_eq!(
            format!("{}", result),
            "lodash: 1.2.3 → 2.0.0 (2024/06/15 10:30)"
        );
    }

    #[test]
    fn test_update_result_display_skip() {
        let dep = sample_dependency();
        let result = UpdateResult::skip(dep, SkipReason::Pinned);
        assert_eq!(format!("{}", result), "lodash: skipped (pinned version)");
    }

    #[test]
    fn test_skip_reason_equality() {
        assert_eq!(SkipReason::Pinned, SkipReason::Pinned);
        assert_ne!(SkipReason::Pinned, SkipReason::Excluded);
    }

    #[test]
    fn test_skip_reason_clone() {
        let reason = SkipReason::FetchFailed("error".to_string());
        let cloned = reason.clone();
        assert_eq!(reason, cloned);
    }

    #[test]
    fn test_update_result_clone() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0");
        let cloned = result.clone();
        assert_eq!(result, cloned);
    }

    #[test]
    fn test_serde_skip_reason() {
        let reason = SkipReason::FetchFailed("timeout".to_string());
        let json = serde_json::to_string(&reason).unwrap();
        let parsed: SkipReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }

    #[test]
    fn test_serde_update_result_update() {
        let dep = sample_dependency();
        let result = UpdateResult::update(dep, "2.0.0");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"type\":\"update\""));
        let parsed: UpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn test_serde_update_result_skip() {
        let dep = sample_dependency();
        let result = UpdateResult::skip(dep, SkipReason::Excluded);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"type\":\"skip\""));
        let parsed: UpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
    }

    #[test]
    fn test_record_write_error() {
        // 書き込めなかった理由は Update にだけ記録し、Skip では何もしない
        let mut update = UpdateResult::update(sample_dependency(), "2.0.0");
        assert_eq!(update.write_error(), None);
        update.record_write_error("Failed to update lodash");
        assert_eq!(update.write_error(), Some("Failed to update lodash"));
        assert!(update.is_update());

        let mut skip = UpdateResult::skip(sample_dependency(), SkipReason::Excluded);
        skip.record_write_error("ignored");
        assert_eq!(skip.write_error(), None);
    }

    #[test]
    fn test_serde_update_result_write_error() {
        // 書き込めなかった理由が無いときは出力せず、無いデータも読み込める
        let result = UpdateResult::update(sample_dependency(), "2.0.0");
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("write_error"));
        let parsed: UpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.write_error(), None);

        let mut failed = result;
        failed.record_write_error("Failed to update lodash");
        let json = serde_json::to_string(&failed).unwrap();
        let parsed: UpdateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, failed);
    }
}
