//! マニフェストファイルの検出とパース
//!
//! このモジュールが提供する機能:
//! - ディレクトリ内のマニフェストファイルを検出
//! - 各種マニフェスト形式から依存関係をパース
//! - モノレポ構造のサポート (pnpm-workspace.yaml)
//! - Tauri プロジェクトのサポート (src-tauri/Cargo.toml)

mod bun_settings;
mod cargo_lock;
mod cargo_toml;
mod composer_json;
mod detector;
mod gemfile;
mod go_mod;
mod gradle;
mod gradle_version_catalog;
mod json_sections;
mod package_json;
mod package_swift;
mod pnpm_settings;
mod pyproject_toml;
mod writer;

pub use bun_settings::{BunSettings, has_bunfig};
pub use cargo_lock::{
    GitLockEntry, RegistryLockEntries, parse_git_entries, parse_registry_entries, read_git_entries,
    read_registry_entries,
};
pub use cargo_toml::CargoTomlParser;
pub use composer_json::ComposerJsonParser;
pub use detector::{ManifestFile, ManifestInfo, detect_manifests};
pub use gemfile::GemfileParser;
pub use go_mod::GoModParser;
pub use gradle::GradleParser;
pub use package_json::PackageJsonParser;
pub use package_swift::PackageSwiftParser;
pub use pnpm_settings::{PnpmSettings, has_pnpm_workspace};
pub use pyproject_toml::PyprojectTomlParser;
pub use writer::{ManifestWriter, WriteResult, read_manifest, write_manifest};

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;

/// マニフェストファイルをパースするためのトレイト
pub trait ManifestParser {
    /// マニフェストファイルから依存関係をパースする
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError>;

    /// このパーサが対応する言語を返す
    fn language(&self) -> Language;

    /// マニフェスト内容の依存バージョンを更新する
    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError>;

    /// git 依存の tag 値を更新する。デフォルトは no-op (対応しない言語向け)。
    /// tag 以外の参照 (branch/rev/default) はマニフェストを書き換えないため呼び出し不要。
    fn update_git_tag(
        &self,
        content: &str,
        _package: &str,
        _new_tag: &str,
    ) -> Result<String, ManifestError> {
        Ok(content.to_string())
    }
}

/// 指定された言語に対応するマニフェストパーサを取得する
pub fn get_parser(language: Language) -> Box<dyn ManifestParser> {
    match language {
        Language::Node => Box::new(PackageJsonParser),
        Language::Python => Box::new(PyprojectTomlParser),
        Language::Rust => Box::new(CargoTomlParser),
        Language::Go => Box::new(GoModParser),
        Language::Ruby => Box::new(GemfileParser),
        Language::Php => Box::new(ComposerJsonParser),
        Language::Java => Box::new(GradleParser),
        Language::Swift => Box::new(PackageSwiftParser),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_parser_node() {
        let parser = get_parser(Language::Node);
        assert_eq!(parser.language(), Language::Node);
    }

    #[test]
    fn test_get_parser_python() {
        let parser = get_parser(Language::Python);
        assert_eq!(parser.language(), Language::Python);
    }

    #[test]
    fn test_get_parser_rust() {
        let parser = get_parser(Language::Rust);
        assert_eq!(parser.language(), Language::Rust);
    }

    #[test]
    fn test_get_parser_go() {
        let parser = get_parser(Language::Go);
        assert_eq!(parser.language(), Language::Go);
    }

    #[test]
    fn test_get_parser_ruby() {
        let parser = get_parser(Language::Ruby);
        assert_eq!(parser.language(), Language::Ruby);
    }

    #[test]
    fn test_get_parser_php() {
        let parser = get_parser(Language::Php);
        assert_eq!(parser.language(), Language::Php);
    }

    #[test]
    fn test_get_parser_java() {
        let parser = get_parser(Language::Java);
        assert_eq!(parser.language(), Language::Java);
    }
}
