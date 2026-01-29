//! Cargo.toml parser for Rust projects
//!
//! Handles:
//! - dependencies
//! - dev-dependencies
//! - build-dependencies
//! - workspace.dependencies (for Cargo workspace root)
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
        let toml: Value = toml::from_str(content).map_err(|e: toml::de::Error| {
            ManifestError::TomlParseError {
                path: PathBuf::from("Cargo.toml"),
                message: e.to_string(),
            }
        })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Rust);

        // Parse regular dependencies
        if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
        }

        // Parse dev-dependencies
        if let Some(deps) = toml.get("dev-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
        }

        // Parse build-dependencies (treated as dev dependencies)
        if let Some(deps) = toml.get("build-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
        }

        // Parse target-specific dependencies
        if let Some(target) = toml.get("target").and_then(|t| t.as_table()) {
            for (_target_name, target_config) in target {
                if let Some(deps) = target_config.get("dependencies").and_then(|d| d.as_table()) {
                    parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
                }
                if let Some(deps) = target_config
                    .get("dev-dependencies")
                    .and_then(|d| d.as_table())
                {
                    parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
                }
            }
        }

        // Parse workspace.dependencies (for Cargo workspace root Cargo.toml)
        if let Some(workspace) = toml.get("workspace").and_then(|w| w.as_table()) {
            if let Some(deps) = workspace.get("dependencies").and_then(|d| d.as_table()) {
                parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
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
        // Also handles [workspace.dependencies.package]
        let multiline_pattern = format!(
            r#"(?m)(\[(?:dependencies|dev-dependencies|build-dependencies|workspace\.dependencies)\.{}[^\]]*\][^\[]*version\s*=\s*)"([^"]+)""#,
            regex::escape(package)
        );
        if let Ok(re) = Regex::new(&multiline_pattern) {
            if let Some(caps) = re.captures(&result) {
                let old_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                if let Some(spec) = parser.parse(old_version) {
                    let new_ver = spec.format_updated(new_version);
                    let replacement = format!(r#"{}"{}""#, &caps[1], new_ver);
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
    parser: &dyn VersionParser,
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

    #[test]
    fn test_update_multiline_table() {
        let content = r#"[dependencies.tree-sitter]
version = "0.22"

[dependencies.tree-sitter-bash]
version = "0.21"
"#;

        let result = CargoTomlParser
            .update_version(content, "tree-sitter", "0.26.3")
            .unwrap();

        // Check that version is properly quoted
        assert!(result.contains("version = \"0.26.3\""));
        // Ensure closing quote exists
        assert!(!result.contains("\"0.26.3\n"));

        // Update second package
        let result2 = CargoTomlParser
            .update_version(&result, "tree-sitter-bash", "0.25.1")
            .unwrap();

        assert!(result2.contains("version = \"0.25.1\""));
        assert!(result2.contains("version = \"0.26.3\""));
    }

    #[test]
    fn test_update_multiline_table_with_features() {
        let content = r#"[dependencies.serde]
version = "1.0.0"
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("version = \"1.1.0\""));
        assert!(result.contains("features = [\"derive\"]"));
    }

    #[test]
    fn test_update_mixed_dependency_formats() {
        // Real-world Cargo.toml with mixed formats:
        // - Simple format: pkg = "version"
        // - Inline table: pkg = { version = "...", features = [...] }
        // - Multiline table: [dependencies.pkg]
        let content = r#"[package]
name = "example-hooks"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
dirs = "5"
regex = "1"
thiserror = "1"
anyhow = "1"

[dependencies.ts-parser]
version = "0.22"
optional = true

[dependencies.ts-bash]
version = "0.21"
optional = true

[features]
default = ["ast-parser"]
ast-parser = ["ts-parser", "ts-bash"]
"#;

        // Test simple format update
        let result = CargoTomlParser
            .update_version(content, "serde_json", "1.0.140")
            .unwrap();
        assert!(result.contains("serde_json = \"1.0.140\""));

        // Test inline table format update
        let result = CargoTomlParser
            .update_version(&result, "clap", "4.5.0")
            .unwrap();
        assert!(result.contains("version = \"4.5.0\""));
        assert!(result.contains("features = [\"derive\"]"));

        // Test another inline table
        let result = CargoTomlParser
            .update_version(&result, "tracing-subscriber", "0.3.20")
            .unwrap();
        assert!(result.contains("version = \"0.3.20\""));
        assert!(result.contains("features = [\"env-filter\"]"));

        // Test multiline table format - must have proper closing quotes
        let result = CargoTomlParser
            .update_version(&result, "ts-parser", "0.26.3")
            .unwrap();
        assert!(result.contains("version = \"0.26.3\""));
        // Verify closing quote exists (not broken)
        assert!(!result.contains("\"0.26.3\n["));

        let result = CargoTomlParser
            .update_version(&result, "ts-bash", "0.25.1")
            .unwrap();
        assert!(result.contains("version = \"0.25.1\""));
        assert!(!result.contains("\"0.25.1\n["));

        // Verify all updates are preserved
        assert!(result.contains("serde_json = \"1.0.140\""));
        assert!(result.contains("clap = { version = \"4.5.0\""));
        assert!(result.contains("version = \"0.26.3\""));
        assert!(result.contains("version = \"0.25.1\""));

        // Verify unrelated content is preserved
        assert!(result.contains("[features]"));
        assert!(result.contains("ast-parser = [\"ts-parser\", \"ts-bash\"]"));
    }

    #[test]
    fn test_parse_workspace_dependencies() {
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/core", "crates/cli"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
        assert!(!tokio.is_dev);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);

        let serde_json = deps.iter().find(|d| d.name == "serde_json").unwrap();
        assert_eq!(serde_json.version_spec.kind, VersionSpecKind::Caret);

        let thiserror = deps.iter().find(|d| d.name == "thiserror").unwrap();
        assert_eq!(thiserror.version_spec.kind, VersionSpecKind::Caret);

        let clap = deps.iter().find(|d| d.name == "clap").unwrap();
        assert_eq!(clap.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_update_workspace_dependencies_simple() {
        let content = r#"
[workspace.dependencies]
serde_json = "1"
thiserror = "2"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde_json", "1.0.140")
            .unwrap();
        assert!(result.contains("serde_json = \"1.0.140\""));
        // Ensure other dependencies are preserved
        assert!(result.contains("thiserror = \"2\""));
    }

    #[test]
    fn test_update_workspace_dependencies_inline_table() {
        let content = r#"
[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        let result = CargoTomlParser
            .update_version(&result, "serde", "1.0.220")
            .unwrap();
        assert!(result.contains("serde = { version = \"1.0.220\""));
    }

    #[test]
    fn test_update_workspace_dependencies_multiline_table() {
        let content = r#"[workspace.dependencies.tokio]
version = "1"
features = ["full"]

[workspace.dependencies.serde]
version = "1"
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        let result = CargoTomlParser
            .update_version(&result, "serde", "1.0.220")
            .unwrap();
        assert!(result.contains("version = \"1.0.220\""));
    }

    #[test]
    fn test_parse_full_workspace_cargo_toml() {
        // Real-world example from the user
        let content = r#"
[workspace]
resolver = "2"
members = [
    "crates/omni-pty-core",
    "crates/term-ipc",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
authors = ["omni-pty contributors"]
repository = "https://github.com/omni-pty/omni-pty"

[workspace.dependencies]
# Core dependencies
portable-pty = "0.9"
vte = "0.15"
tokio = { version = "1", features = ["full"] }
kdl = "4"

# Utilities
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
parking_lot = "0.12"
log = "0.4"
libc = "0.2"

# CLI
clap = { version = "4", features = ["derive"] }

# Testing
tokio-test = "0.4"

# Swift FFI
swift-bridge = "0.1"
swift-bridge-build = "0.1"
"#;

        let deps = parse(content).unwrap();
        // Should parse all workspace.dependencies (16 total)
        assert_eq!(deps.len(), 16);

        // Verify some specific dependencies
        let portable_pty = deps.iter().find(|d| d.name == "portable-pty").unwrap();
        assert_eq!(portable_pty.version_spec.version, "0.9");

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.version, "1");

        let uuid = deps.iter().find(|d| d.name == "uuid").unwrap();
        assert_eq!(uuid.version_spec.version, "1");

        let swift_bridge = deps.iter().find(|d| d.name == "swift-bridge").unwrap();
        assert_eq!(swift_bridge.version_spec.version, "0.1");
    }

    #[test]
    fn test_update_full_workspace_cargo_toml() {
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/core"]

[workspace.dependencies]
portable-pty = "0.9"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
"#;

        // Update simple format
        let result = CargoTomlParser
            .update_version(content, "portable-pty", "0.10.0")
            .unwrap();
        assert!(result.contains("portable-pty = \"0.10.0\""));

        // Update inline table format
        let result = CargoTomlParser
            .update_version(&result, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        // Verify workspace metadata is preserved
        assert!(result.contains("resolver = \"2\""));
        assert!(result.contains("members = [\"crates/core\"]"));
    }

    #[test]
    fn test_parse_workspace_with_regular_dependencies() {
        // Workspace root with both workspace.dependencies and regular dependencies
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/cli"]

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = "1"

[dependencies]
clap = "4"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        // Workspace dependencies
        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert!(!tokio.is_dev);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde.is_dev);

        // Regular dependencies
        let clap = deps.iter().find(|d| d.name == "clap").unwrap();
        assert!(!clap.is_dev);

        // Dev dependencies
        let criterion = deps.iter().find(|d| d.name == "criterion").unwrap();
        assert!(criterion.is_dev);
    }
}
