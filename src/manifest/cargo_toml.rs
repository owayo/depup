//! Cargo.toml parser for Rust projects
//!
//! Handles:
//! - dependencies
//! - dev-dependencies
//! - build-dependencies
//! - Inline table format: { version = "1.0" }
//! - Workspace dependencies

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::{get_parser, VersionParser};
use regex::Regex;
use std::path::PathBuf;
use toml::Value;

/// Parser for Cargo.toml files
pub struct CargoTomlParser;

impl ManifestParser for CargoTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let toml: Value =
            content
                .parse()
                .map_err(|e: toml::de::Error| ManifestError::TomlParseError {
                    path: PathBuf::from("Cargo.toml"),
                    message: e.to_string(),
                })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Rust);

        // Parse regular dependencies
        if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, &parser, false, &mut dependencies);
        }

        // Parse dev-dependencies
        if let Some(deps) = toml.get("dev-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, &parser, true, &mut dependencies);
        }

        // Parse build-dependencies (treated as dev dependencies)
        if let Some(deps) = toml.get("build-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, &parser, true, &mut dependencies);
        }

        // Parse target-specific dependencies
        if let Some(target) = toml.get("target").and_then(|t| t.as_table()) {
            for (_target_name, target_config) in target {
                if let Some(deps) = target_config.get("dependencies").and_then(|d| d.as_table()) {
                    parse_cargo_dependencies(deps, &parser, false, &mut dependencies);
                }
                if let Some(deps) = target_config
                    .get("dev-dependencies")
                    .and_then(|d| d.as_table())
                {
                    parse_cargo_dependencies(deps, &parser, true, &mut dependencies);
                }
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Rust
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parser = get_parser(Language::Rust);
        let mut result = content.to_string();
        let mut updated = false;

        // Pattern for simple version: package = "1.0.0" or package = "^1.0.0"
        let simple_pattern = format!(r#"(?m)^(\s*{})\s*=\s*"([^"]+)""#, regex::escape(package));
        if let Ok(re) = Regex::new(&simple_pattern) {
            if let Some(caps) = re.captures(&result) {
                let old_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                // Check if this is a simple version string (not a path or git dependency)
                if !old_version.contains('/') && !old_version.starts_with('{') {
                    if let Some(spec) = parser.parse(old_version) {
                        let new_ver = spec.format_updated(new_version);
                        let replacement = format!(r#"{} = "{}""#, &caps[1], new_ver);
                        result = re.replace(&result, replacement.as_str()).to_string();
                        updated = true;
                    }
                }
            }
        }

        // Pattern for inline table: package = { version = "1.0.0", ... }
        // Match only the version value part to preserve the rest of the line
        let table_pattern = format!(
            r#"(?m)({})\s*=\s*\{{\s*version\s*=\s*"([^"]+)""#,
            regex::escape(package)
        );
        if let Ok(re) = Regex::new(&table_pattern) {
            if let Some(caps) = re.captures(&result) {
                let old_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                if let Some(spec) = parser.parse(old_version) {
                    let new_ver = spec.format_updated(new_version);
                    let replacement = format!(r#"{} = {{ version = "{}""#, &caps[1], new_ver);
                    result = re.replace(&result, replacement.as_str()).to_string();
                    updated = true;
                }
            }
        }

        // Pattern for multi-line table format:
        // [dependencies.package]
        // version = "1.0.0"
        let multiline_pattern = format!(
            r#"(?m)(\[(?:dependencies|dev-dependencies|build-dependencies)\.{}[^\]]*\][^\[]*version\s*=\s*)"([^"]+)""#,
            regex::escape(package)
        );
        if let Ok(re) = Regex::new(&multiline_pattern) {
            if let Some(caps) = re.captures(&result) {
                let old_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                if let Some(spec) = parser.parse(old_version) {
                    let new_ver = spec.format_updated(new_version);
                    let replacement = format!(r#"{}"{}"#, &caps[1], new_ver);
                    result = re.replace(&result, replacement.as_str()).to_string();
                    updated = true;
                }
            }
        }

        if updated {
            Ok(result)
        } else {
            Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })
        }
    }
}

fn parse_cargo_dependencies(
    deps: &toml::map::Map<String, Value>,
    parser: &Box<dyn VersionParser>,
    is_dev: bool,
    output: &mut Vec<Dependency>,
) {
    for (name, value) in deps {
        let version_str = match value {
            // Simple string: package = "1.0.0"
            Value::String(s) => Some(s.clone()),
            // Inline table: package = { version = "1.0.0", features = [...] }
            Value::Table(t) => t.get("version").and_then(|v| v.as_str()).map(String::from),
            _ => None,
        };

        if let Some(version_str) = version_str {
            if let Some(spec) = parser.parse(&version_str) {
                let dep = if is_dev {
                    Dependency::development(name.clone(), spec, Language::Rust)
                } else {
                    Dependency::production(name.clone(), spec, Language::Rust)
                };
                output.push(dep);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        CargoTomlParser.parse(content)
    }

    #[test]
    fn test_parse_simple_dependencies() {
        let content = r#"
[dependencies]
serde = "1.0"
tokio = "^1.28.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);
        assert!(!serde.is_dev);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_pinned_version() {
        let content = r#"
[dependencies]
exact = "=1.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_inline_table() {
        let content = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "^1.28.0", features = ["full"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_dev_dependencies() {
        let content = r#"
[dev-dependencies]
criterion = "0.5"
tempfile = "3.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().all(|d| d.is_dev));
    }

    #[test]
    fn test_parse_build_dependencies() {
        let content = r#"
[build-dependencies]
cc = "1.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_mixed_dependencies() {
        let content = r#"
[dependencies]
serde = "1.0"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde.is_dev);

        let criterion = deps.iter().find(|d| d.name == "criterion").unwrap();
        assert!(criterion.is_dev);
    }

    #[test]
    fn test_parse_tilde_version() {
        let content = r#"
[dependencies]
regex = "~1.9"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"
[package]
name = "test"
version = "0.1.0"
"#;

        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_invalid_toml() {
        let content = "not valid toml";
        let result = parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_git_dependency_skipped() {
        let content = r#"
[dependencies]
my-crate = { git = "https://github.com/example/my-crate" }
"#;

        let deps = parse(content).unwrap();
        // Git dependencies without version should be skipped
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_path_dependency_skipped() {
        let content = r#"
[dependencies]
local-crate = { path = "../local-crate" }
"#;

        let deps = parse(content).unwrap();
        // Path dependencies without version should be skipped
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_workspace_dependency() {
        let content = r#"
[dependencies]
serde = { workspace = true }
"#;

        let deps = parse(content).unwrap();
        // Workspace dependencies without explicit version should be skipped
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_target_specific() {
        let content = r#"
[target.'cfg(windows)'.dependencies]
winapi = "0.3"

[target.'cfg(unix)'.dependencies]
libc = "0.2"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let winapi = deps.iter().find(|d| d.name == "winapi").unwrap();
        assert!(!winapi.is_dev);

        let libc = deps.iter().find(|d| d.name == "libc").unwrap();
        assert!(!libc.is_dev);
    }

    #[test]
    fn test_update_simple_version() {
        let content = r#"
[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(result.contains("\"1.1.0\""));
    }

    #[test]
    fn test_update_caret_version() {
        let content = r#"
[dependencies]
tokio = "^1.28.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.35.0")
            .unwrap();
        assert!(result.contains("\"^1.35.0\""));
    }

    #[test]
    fn test_update_inline_table() {
        let content = r#"
[dependencies]
serde = { version = "1.0.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(result.contains("\"1.1.0\""));
        assert!(result.contains("features"));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"
[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_language() {
        assert_eq!(CargoTomlParser.language(), Language::Rust);
    }

    #[test]
    fn test_parse_comparison_operators() {
        let content = r#"
[dependencies]
pkg1 = ">=1.0.0"
pkg2 = ">1.0.0"
pkg3 = "<=2.0.0"
pkg4 = "<2.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        let pkg1 = deps.iter().find(|d| d.name == "pkg1").unwrap();
        assert_eq!(pkg1.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pkg2 = deps.iter().find(|d| d.name == "pkg2").unwrap();
        assert_eq!(pkg2.version_spec.kind, VersionSpecKind::Greater);

        let pkg3 = deps.iter().find(|d| d.name == "pkg3").unwrap();
        assert_eq!(pkg3.version_spec.kind, VersionSpecKind::LessOrEqual);

        let pkg4 = deps.iter().find(|d| d.name == "pkg4").unwrap();
        assert_eq!(pkg4.version_spec.kind, VersionSpecKind::Less);
    }
}
