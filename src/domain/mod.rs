//! depup のコアドメインモデル
//!
//! このモジュールはアプリケーション全体で使用される基本型を含む:
//! - 対応エコシステムの言語型
//! - バージョン制約のパースと保持のためのバージョン指定型
//! - 依存関係情報の構造体
//! - 更新判定結果
//! - サマリと結果の構造体

mod dependency;
mod git_source;
mod language;
mod summary;
mod update_result;
mod version_spec;

pub use dependency::Dependency;
pub use git_source::{GitReference, GitSource};
pub use language::Language;
pub use summary::{ManifestUpdateResult, UpdateSummary};
pub use update_result::{SkipReason, UpdateResult};
pub use version_spec::{VersionSpec, VersionSpecKind};
