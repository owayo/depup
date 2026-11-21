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
use std::path::Path;

/// 設定ファイルが属するプロジェクト。複数の末尾に一致する名前は最長のものを使う。
pub(crate) fn mise_project_directory(config: &Path) -> &Path {
    let depth = super::MISE_CONFIG_FILENAMES
        .iter()
        .map(Path::new)
        .filter(|name| config.ends_with(name))
        .map(|name| name.components().count())
        .max()
        .unwrap_or(1);
    config.ancestors().nth(depth).unwrap_or(config)
}

/// mise の Python だけが、同じプロジェクトのインタープリター制約を使う。
pub(crate) fn read_mise_requires_python(
    manifest: &Path,
    dep: &Dependency,
) -> Result<Option<pep440_rs::VersionSpecifiers>, String> {
    if dep.language != Language::Mise
        || !matches!(dep.name.split('[').next(), Some("python" | "core:python"))
    {
        return Ok(None);
    }
    let path = mise_project_directory(manifest).join("pyproject.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read pyproject.toml: {error}")),
    };
    let (_, body) = super::split_utf8_bom(&content);
    let document: toml::Table = toml::from_str(body)
        .map_err(|_| "cannot read project.requires-python from invalid pyproject.toml")?;
    let Some(value) = document
        .get("project")
        .and_then(|project| project.get("requires-python"))
    else {
        return Ok(None);
    };
    let specifiers = value
        .as_str()
        .ok_or("project.requires-python must be a string")?
        .trim();
    // PEP 508 の版指定のリストは末尾にカンマを置ける。
    let specifiers = specifiers
        .strip_suffix(',')
        .filter(|rest| !rest.is_empty())
        .unwrap_or(specifiers);
    specifiers
        .parse()
        .map(Some)
        .map_err(|error| format!("invalid project.requires-python: {error}"))
}

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
    fn python_requirement_uses_only_the_mise_project() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path();
        let member = parent.join("member");
        std::fs::create_dir(&member).unwrap();
        std::fs::write(
            parent.join("pyproject.toml"),
            "[project]\nrequires-python = '<3'",
        )
        .unwrap();
        let config = member.join(".config/mise/config.toml");
        let deps = MiseParser
            .parse("[tools]\n'core:python' = '3.14.10'\n")
            .unwrap();
        let python = &deps[0];
        assert_eq!(read_mise_requires_python(&config, python).unwrap(), None);
        let pyproject = member.join("pyproject.toml");
        std::fs::write(
            &pyproject,
            "\u{feff}project.requires-python = '==3.14.*,!=3.14.11,'",
        )
        .unwrap();
        let requirement = read_mise_requires_python(&config, python).unwrap().unwrap();
        assert!(requirement.contains(&"3.14.10".parse().unwrap()));
        assert!(!requirement.contains(&"3.14.11".parse().unwrap()));
        for invalid in ["'not a version specifier'", "314"] {
            std::fs::write(&pyproject, format!("project.requires-python = {invalid}")).unwrap();
            assert!(read_mise_requires_python(&config, python).is_err());
            let node = MiseParser.parse("[tools]\nnode = '24'\n").unwrap();
            assert_eq!(read_mise_requires_python(&config, &node[0]).unwrap(), None);
        }
    }

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
