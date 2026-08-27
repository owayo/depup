//! mise マニフェストの形式振り分け
//!
//! mise は 2 つの形式を読む:
//! - TOML 形式 (`mise.toml` / `.mise.toml` / `.config/mise/config.toml` …)
//! - asdf 互換の空白区切り形式 (`.tool-versions`)
//!
//! `ManifestParser` には内容しか渡らないため、Gradle が build.gradle と
//! version catalog (`*.versions.toml`) を内容で見分けているのと同じ方式で
//! 形式を判別してから各実装へ委譲する。

use super::ManifestParser;
use super::mise_toml::MiseTomlParser;
use super::tool_versions::{ToolVersionsParser, looks_like_tool_versions};
use crate::domain::{Dependency, Language};
use crate::error::ManifestError;

/// mise 設定ファイルのパーサ (TOML 形式 / `.tool-versions` 形式を振り分ける)
pub struct MiseParser;

impl MiseParser {
    /// 内容に応じた実体パーサを返す
    fn delegate(content: &str) -> Box<dyn ManifestParser> {
        if looks_like_tool_versions(content) {
            Box::new(ToolVersionsParser)
        } else {
            Box::new(MiseTomlParser)
        }
    }
}

impl ManifestParser for MiseParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        Self::delegate(content).parse(content)
    }

    fn language(&self) -> Language {
        Language::Mise
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        Self::delegate(content).update_version(content, package, new_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatches_to_toml_parser() {
        let deps = MiseParser.parse("[tools]\nnode = \"26.7.0\"\n").unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "node");
    }

    #[test]
    fn test_dispatches_to_tool_versions_parser() {
        let deps = MiseParser.parse("node 26.7.0\nruby 3.4.2\n").unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_update_dispatches_by_format() {
        let toml_updated = MiseParser
            .update_version("[tools]\nnode = \"26.7.0\"\n", "node", "26.8.1")
            .unwrap();
        assert_eq!(toml_updated, "[tools]\nnode = \"26.8.1\"\n");

        let plain_updated = MiseParser
            .update_version("node 26.7.0\n", "node", "26.8.1")
            .unwrap();
        assert_eq!(plain_updated, "node 26.8.1\n");
    }

    /// 壊れた TOML を `.tool-versions` と誤認して黙って読み飛ばさない
    #[test]
    fn test_broken_toml_is_reported_as_toml_error() {
        let result = MiseParser.parse("[tools]\nnode = \"26.7.0\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_language() {
        assert_eq!(MiseParser.language(), Language::Mise);
    }
}
