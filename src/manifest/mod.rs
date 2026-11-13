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
mod gradle_wrapper;
mod json_sections;
mod line_utils;
mod mise;
mod mise_settings;
mod mise_toml;
pub(crate) mod native_age;
mod package_json;
mod package_swift;
mod pnpm_settings;
mod pyproject_toml;
mod tool_versions;
mod writer;

pub use bun_settings::{BunSettings, has_bunfig};
pub(crate) use cargo_lock::parse_dependency_ref;
pub use cargo_lock::{
    CRATES_IO_LOCK_SOURCE, CRATES_IO_SPARSE_SOURCE, GitLockEntry, LockDependencyRef, LockPackage,
    RegistryLockEntries, find_cargo_lock_upward, is_crates_io_source, parse_git_entries,
    parse_lock_packages, parse_registry_entries, read_git_entries, read_lock_packages,
    read_registry_entries,
};
pub use cargo_toml::CargoTomlParser;
pub use composer_json::ComposerJsonParser;
pub(crate) use detector::strip_gradle_comments;
pub use detector::{ManifestFile, ManifestInfo, detect_manifests};
pub use gemfile::GemfileParser;
pub use go_mod::GoModParser;
pub use gradle::{GradleFile, GradleParser, GradleVersionCatalogParser, GradleWrapperParser};
pub use gradle_wrapper::GRADLE_WRAPPER_PACKAGE;
pub(crate) use gradle_wrapper::{GRADLE_WRAPPER_FILES, GRADLE_WRAPPER_PROPERTIES};
pub use mise::MiseParser;
pub use mise_settings::{MISE_CONFIG_FILENAMES, MiseSettings, has_mise_config, mise_config_paths};
pub use package_json::PackageJsonParser;
pub use package_swift::PackageSwiftParser;
pub use pnpm_settings::{PnpmSettings, has_pnpm_workspace};
pub use pyproject_toml::PyprojectTomlParser;
pub use tool_versions::TOOL_VERSIONS_FILENAME;
pub use writer::{ManifestWriter, WriteFailure, WriteResult, read_manifest, write_manifest};

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use std::path::Path;

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

/// 指定された言語の主なマニフェスト (Gradle は build スクリプト) のパーサを取得する。
/// 検出したマニフェストを読むときは、ファイルの種類で選ぶ [`parser_for`] を使う。
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
        Language::Mise => Box::new(MiseParser),
    }
}

/// マニフェストのパスと言語に対応するパーサを取得する。Gradle はファイルの種類
/// (build スクリプト・version catalog・Wrapper の properties) をファイル名で決める。
pub fn parser_for(path: &Path, language: Language) -> Box<dyn ManifestParser> {
    match language {
        Language::Java => GradleFile::of(path).parser(),
        _ => get_parser(language),
    }
}

/// UTF-8 の BOM (`U+FEFF`)
const UTF8_BOM: &str = "\u{feff}";

/// ファイルの内容を、先頭の UTF-8 の BOM (なければ空) と、それを除いた本文に分ける。
///
/// BOM は文字コードの印で、TOML や JSON の構文ではない。パーサは本文を受け取り、書き戻す
/// ときに BOM を付け直す。行ごとに読むパーサは 1 行目の `\u{feff}[tools]` をセクションと
/// 見ないので、BOM のまま渡すと mise.toml のツールを検出せず、Cargo.toml は更新を報告した
/// 後に書き換えに失敗していた (#103)。
pub(crate) fn split_utf8_bom(content: &str) -> (&'static str, &str) {
    match content.strip_prefix(UTF8_BOM) {
        Some(body) => (UTF8_BOM, body),
        None => ("", content),
    }
}

/// バイト列の先頭の UTF-8 の BOM を除く。package.json をバイト列のまま `serde_json` へ渡す
/// 箇所 (pnpm の override・install 後の監査・OSV の基準) で使う。`serde_json` は BOM を
/// 読み飛ばさないので、BOM 付きの package.json で Node の処理が止まっていた (#159)。
pub(crate) fn strip_utf8_bom_bytes(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM.as_bytes()).unwrap_or(bytes)
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

    /// Gradle はファイル名でパーサを選ぶ。build スクリプトの `distributionUrl` を Wrapper の
    /// 宣言と取り違えず、依存を検出する (#92)。
    #[test]
    fn gradle_manifests_are_parsed_by_their_file_kind() {
        let script = "dependencies {\n    implementation 'com.google.guava:guava:31.0-jre'\n}\n\nwrapper {\n    distributionUrl = 'https://services.gradle.org/distributions/gradle-8.5-bin.zip'\n}\n";
        for file in ["build.gradle", "app/build.gradle.kts"] {
            let dependencies = parser_for(Path::new(file), Language::Java)
                .parse(script)
                .unwrap();
            let names: Vec<_> = dependencies.iter().map(|dep| dep.name.as_str()).collect();
            assert_eq!(names, ["com.google.guava:guava"], "{file}");
        }
        let wrapper =
            "distributionUrl=https\\://services.gradle.org/distributions/gradle-8.5-bin.zip\n";
        let dependencies = parser_for(
            Path::new("gradle/wrapper/gradle-wrapper.properties"),
            Language::Java,
        )
        .parse(wrapper)
        .unwrap();
        assert_eq!(dependencies[0].name, GRADLE_WRAPPER_PACKAGE);
        let catalog = "[libraries]\nguava = \"com.google.guava:guava:31.0-jre\"\n";
        let dependencies = parser_for(Path::new("gradle/libs.versions.toml"), Language::Java)
            .parse(catalog)
            .unwrap();
        assert_eq!(dependencies[0].name, "com.google.guava:guava");
    }
}
