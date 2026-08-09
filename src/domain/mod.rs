//! depup のコアドメインモデル
//!
//! このモジュールはアプリケーション全体で使用される基本型を含む:
//! - 対応エコシステムの言語型
//! - バージョン制約のパースと保持のためのバージョン指定型
//! - 依存関係情報の構造体
//! - 更新判定結果
//! - サマリと結果の構造体
//! - リリース経過時間 (age) の範囲検証とカットオフ算出

mod age;
mod change_level;
mod dependency;
mod git_source;
mod language;
mod summary;
mod update_result;
mod version_spec;

pub use age::{MAX_AGE_SECS, checked_age, checked_age_from_minutes, cutoff_from, cutoff_now};
pub use change_level::ChangeLevel;
pub use dependency::Dependency;
pub use git_source::{GitReference, GitSource};
pub use language::Language;
pub use summary::{ManifestUpdateResult, UpdateSummary};
pub use update_result::{SkipReason, UpdateResult};
pub use version_spec::{VersionSpec, VersionSpecKind, range_lower_bound_version};
