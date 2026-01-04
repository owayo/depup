//! pyproject.toml parser for Python projects
//!
//! Handles:
//! - project.dependencies (PEP 621)
//! - project.optional-dependencies (PEP 621)
//! - tool.poetry.dependencies (Poetry)
//! - tool.poetry.dev-dependencies (Poetry)

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::{get_parser, VersionParser};
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;
use toml::Value;

/// Parser for pyproject.toml files
pub struct PyprojectTomlParser;

// Regex to parse PEP 508 dependency specifiers
// Matches: package-name>=1.0,<2.0 or package-name==1.0 or package-name^1.0, etc.
static PEP508_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z0-9][-a-zA-Z0-9._]*)\s*(.*)$").unwrap());

impl ManifestParser for PyprojectTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let toml: Value =
            content
                .parse()
                .map_err(|e: toml::de::Error| ManifestError::TomlParseError {
                    path: PathBuf::from("pyproject.toml"),
                    message: e.to_string(),
                })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Python);

        // Parse PEP 621 project.dependencies
        if let Some(deps) = toml
            .get("project")
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
        {
            for dep in deps {
                if let Some(dep_str) = dep.as_str() {
                    if let Some(parsed) = parse_pep508_dependency(dep_str, parser.as_ref(), false) {
                        dependencies.push(parsed);
                    }
                }
            }
        }

        // Parse PEP 621 project.optional-dependencies
        if let Some(optional) = toml
            .get("project")
            .and_then(|p| p.get("optional-dependencies"))
            .and_then(|d| d.as_table())
        {
            for (_group, deps) in optional {
                if let Some(deps_array) = deps.as_array() {
                    for dep in deps_array {
                        if let Some(dep_str) = dep.as_str() {
                            if let Some(parsed) =
                                parse_pep508_dependency(dep_str, parser.as_ref(), false)
                            {
                                dependencies.push(parsed);
                            }
                        }
                    }
                }
            }
        }

        // Parse Poetry dependencies
        if let Some(poetry_deps) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_table())
        {
            for (name, value) in poetry_deps {
                // Skip python version requirement
                if name == "python" {
                    continue;
                }
                if let Some(parsed) = parse_poetry_dependency(name, value, parser.as_ref(), false) {
                    dependencies.push(parsed);
                }
            }
        }

        // Parse Poetry dev-dependencies
        if let Some(dev_deps) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dev-dependencies"))
            .and_then(|d| d.as_table())
        {
            for (name, value) in dev_deps {
                if let Some(parsed) = parse_poetry_dependency(name, value, parser.as_ref(), true) {
                    dependencies.push(parsed);
                }
            }
        }

        // Parse Poetry group dependencies (Poetry 1.2+)
        if let Some(groups) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("group"))
            .and_then(|g| g.as_table())
        {
            for (group_name, group) in groups {
                let is_dev = group_name == "dev" || group_name == "test";
                if let Some(deps) = group.get("dependencies").and_then(|d| d.as_table()) {
                    for (name, value) in deps {
                        if let Some(parsed) =
                            parse_poetry_dependency(name, value, parser.as_ref(), is_dev)
                        {
                            dependencies.push(parsed);
                        }
                    }
                }
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Python
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        // For TOML, we need to be careful to preserve formatting
        // We'll do a simple string replacement approach
        let parser = get_parser(Language::Python);

        // Try to find and update the version in the content
        let mut result = content.to_string();
        let mut updated = false;

        // Pattern for Poetry-style dependencies: name = "^1.0.0" or name = { version = "^1.0.0" }
        let simple_pattern = format!(r#"(?m)^(\s*{}\s*=\s*)"([^"]+)"#, regex::escape(package));
        if let Ok(re) = Regex::new(&simple_pattern) {
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

        // Pattern for Poetry inline table: name = { version = "^1.0.0", ... }
        let table_pattern = format!(
            r#"(?m)({}\s*=\s*\{{\s*[^}}]*version\s*=\s*)"([^"]+)""#,
            regex::escape(package)
        );
        if let Ok(re) = Regex::new(&table_pattern) {
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

        // Pattern for PEP 508 in array: "package>=1.0,<2.0"
        let pep508_pattern = format!(r#""({}(?:\s*[<>=!~^]+[^"]+)?)""#, regex::escape(package));
        if let Ok(re) = Regex::new(&pep508_pattern) {
            let result_clone = result.clone();
            for caps in re.captures_iter(&result_clone) {
                let full_dep = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if let Some(pep_caps) = PEP508_RE.captures(full_dep) {
                    let pkg_name = pep_caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let version_part = pep_caps.get(2).map(|m| m.as_str()).unwrap_or("").trim();

                    if pkg_name == package && !version_part.is_empty() {
                        if let Some(spec) = parser.parse(version_part) {
                            let new_ver = spec.format_updated(new_version);
                            let new_dep = format!("{}{}", package, new_ver);
                            result = result
                                .replace(&format!(r#""{full_dep}""#), &format!(r#""{new_dep}""#));
                            updated = true;
                        }
                    }
                }
            }
        }

        if updated {
            Ok(result)
        } else {
            Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("pyproject.toml"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })
        }
    }
}

fn parse_pep508_dependency(
    dep_str: &str,
    parser: &dyn VersionParser,
    is_dev: bool,
) -> Option<Dependency> {
    let caps = PEP508_RE.captures(dep_str)?;
    let name = caps.get(1)?.as_str();
    let mut version_part = caps.get(2).map(|m| m.as_str()).unwrap_or("").trim();

    // Handle extras like package[extra]>=1.0 - strip [extra] from version_part
    if version_part.starts_with('[') {
        if let Some(idx) = version_part.find(']') {
            version_part = version_part[idx + 1..].trim();
        }
    }

    // Remove any environment markers (after ;)
    let version_part = version_part
        .split(';')
        .next()
        .unwrap_or(version_part)
        .trim();

    if version_part.is_empty() {
        return None;
    }

    let spec = parser.parse(version_part)?;
    Some(if is_dev {
        Dependency::development(name, spec, Language::Python)
    } else {
        Dependency::production(name, spec, Language::Python)
    })
}

fn parse_poetry_dependency(
    name: &str,
    value: &Value,
    parser: &dyn VersionParser,
    is_dev: bool,
) -> Option<Dependency> {
    let version_str = match value {
        Value::String(s) => s.clone(),
        Value::Table(t) => t.get("version")?.as_str()?.to_string(),
        _ => return None,
    };

    let spec = parser.parse(&version_str)?;
    Some(if is_dev {
        Dependency::development(name, spec, Language::Python)
    } else {
        Dependency::production(name, spec, Language::Python)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        PyprojectTomlParser.parse(content)
    }

    #[test]
    fn test_parse_pep621_dependencies() {
        let content = r#"
[project]
dependencies = [
    "requests>=2.28.0",
    "pydantic==2.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let requests = deps.iter().find(|d| d.name == "requests").unwrap();
        assert_eq!(requests.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pydantic = deps.iter().find(|d| d.name == "pydantic").unwrap();
        assert_eq!(pydantic.version_spec.kind, VersionSpecKind::Exact);
        assert!(pydantic.is_pinned());
    }

    #[test]
    fn test_parse_pep621_optional_dependencies() {
        let content = r#"
[project.optional-dependencies]
dev = [
    "pytest>=7.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pytest");
    }

    #[test]
    fn test_parse_poetry_dependencies() {
        let content = r#"
[tool.poetry.dependencies]
python = "^3.8"
requests = "^2.28.0"
pydantic = "~2.0"
"#;

        let deps = parse(content).unwrap();
        // python should be skipped
        assert_eq!(deps.len(), 2);

        let requests = deps.iter().find(|d| d.name == "requests").unwrap();
        assert_eq!(requests.version_spec.kind, VersionSpecKind::Caret);
        assert!(!requests.is_dev);

        let pydantic = deps.iter().find(|d| d.name == "pydantic").unwrap();
        assert_eq!(pydantic.version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_poetry_dev_dependencies() {
        let content = r#"
[tool.poetry.dev-dependencies]
pytest = "^7.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_poetry_group_dependencies() {
        let content = r#"
[tool.poetry.group.dev.dependencies]
pytest = "^7.0.0"

[tool.poetry.group.docs.dependencies]
sphinx = "^6.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let pytest = deps.iter().find(|d| d.name == "pytest").unwrap();
        assert!(pytest.is_dev);

        let sphinx = deps.iter().find(|d| d.name == "sphinx").unwrap();
        assert!(!sphinx.is_dev); // docs group is not dev
    }

    #[test]
    fn test_parse_poetry_inline_table() {
        let content = r#"
[tool.poetry.dependencies]
requests = { version = "^2.28.0", extras = ["security"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"
[project]
name = "test"
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
    fn test_parse_with_extras() {
        let content = r#"
[project]
dependencies = [
    "httpx[http2]>=0.24.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "httpx");
    }

    #[test]
    fn test_parse_with_environment_markers() {
        let content = r#"
[project]
dependencies = [
    "pywin32>=300; sys_platform == 'win32'",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pywin32");
    }

    #[test]
    fn test_update_poetry_version() {
        let content = r#"
[tool.poetry.dependencies]
requests = "^2.28.0"
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains("^2.31.0"));
    }

    #[test]
    fn test_update_poetry_inline_table() {
        let content = r#"
[tool.poetry.dependencies]
requests = { version = "^2.28.0", extras = ["security"] }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains("^2.31.0"));
    }

    #[test]
    fn test_language() {
        assert_eq!(PyprojectTomlParser.language(), Language::Python);
    }
}
