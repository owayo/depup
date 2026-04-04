//! 更新判定結果の型定義

use super::Dependency;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// 依存関係の更新がスキップされた理由
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// バージョンがピン留めされている (完全一致バージョン指定)
    Pinned,
    /// 既に最新バージョン
    AlreadyLatest,
    /// --exclude フラグで除外された
    Excluded,
    /// --only リストに含まれていない
    NotInOnlyList,
    /// レジストリからのバージョン情報取得に失敗
    FetchFailed(String),
    /// 適切なバージョンが見つからない (例: 経過日数フィルタで全バージョンが除外)
    NoSuitableVersion,
    /// バージョンのパースに失敗
    ParseError(String),
    /// 言語フィルタで除外された
    LanguageFiltered,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Pinned => write!(f, "pinned version"),
            SkipReason::AlreadyLatest => write!(f, "already at latest"),
            SkipReason::Excluded => write!(f, "excluded by --exclude"),
            SkipReason::NotInOnlyList => write!(f, "not in --only list"),
            SkipReason::FetchFailed(msg) => write!(f, "fetch failed: {}", msg),
            SkipReason::NoSuitableVersion => write!(f, "no suitable version"),
            SkipReason::ParseError(msg) => write!(f, "parse error: {}", msg),
            SkipReason::LanguageFiltered => write!(f, "language filtered"),
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
    },
    /// 依存関係の更新がスキップされた
    Skip {
        /// スキップされた依存関係
        dependency: Dependency,
        /// スキップの理由
        reason: SkipReason,
        /// 現在のバージョンのリリース日時 (AlreadyLatest の場合)
        #[serde(skip_serializing_if = "Option::is_none")]
        released_at: Option<DateTime<Utc>>,
    },
}

impl UpdateResult {
    /// リリース日なしのUpdate結果を作成する (後方互換性のため)
    pub fn update(dependency: Dependency, new_version: impl Into<String>) -> Self {
        UpdateResult::Update {
            dependency,
            new_version: new_version.into(),
            released_at: None,
        }
    }

    /// リリース日付きのUpdate結果を作成する
    pub fn update_with_date(
        dependency: Dependency,
        new_version: impl Into<String>,
        released_at: DateTime<Utc>,
    ) -> Self {
        UpdateResult::Update {
            dependency,
            new_version: new_version.into(),
            released_at: Some(released_at),
        }
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
        UpdateResult::Skip {
            dependency,
            reason: SkipReason::AlreadyLatest,
            released_at: Some(released_at),
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

    /// Update結果かどうかを返す
    pub fn is_update(&self) -> bool {
        matches!(self, UpdateResult::Update { .. })
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
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(new_version, "2.0.0");
            assert!(released_at.is_none()); // update() 使用時はリリース日なし
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
        } = result
        {
            assert_eq!(dependency, dep);
            assert_eq!(new_version, "2.0.0");
            assert_eq!(released_at, Some(date));
        } else {
            panic!("Expected Update variant");
        }
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
}
