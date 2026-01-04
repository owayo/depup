//! package.json parser for Node.js projects
//!
//! Handles:
//! - dependencies
//! - devDependencies
//! - peerDependencies
//! - optionalDependencies

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::{get_parser, VersionParser};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// Parser for package.json files
pub struct PackageJsonParser;

impl ManifestParser for PackageJsonParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let json: Value =
            serde_json::from_str(content).map_err(|e| ManifestError::JsonParseError {
                path: PathBuf::from("package.json"),
                message: e.to_string(),
            })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Node);

        // Parse regular dependencies
        if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        // Parse devDependencies
        if let Some(deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), true, &mut dependencies);
        }

        // Parse peerDependencies (treated as regular dependencies)
        if let Some(deps) = json.get("peerDependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        // Parse optionalDependencies
        if let Some(deps) = json.get("optionalDependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Node
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let mut json: Value =
            serde_json::from_str(content).map_err(|e| ManifestError::JsonParseError {
                path: PathBuf::from("package.json"),
                message: e.to_string(),
            })?;

        let mut updated = false;

        // Update in all dependency sections
        for section in &[
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            if let Some(deps) = json.get_mut(section).and_then(|v| v.as_object_mut()) {
                if let Some(version_value) = deps.get_mut(package) {
                    if let Some(old_version_str) = version_value.as_str() {
                        let parser = get_parser(Language::Node);
                        if let Some(spec) = parser.parse(old_version_str) {
                            let updated_version = spec.format_updated(new_version);
                            *version_value = Value::String(updated_version);
                            updated = true;
                        }
                    }
                }
            }
        }

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("package.json"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        // Serialize with pretty printing to maintain formatting
        serde_json::to_string_pretty(&json).map_err(|e| ManifestError::JsonParseError {
            path: PathBuf::from("package.json"),
            message: e.to_string(),
        })
    }
}

fn parse_dependency_object(
    deps: &Map<String, Value>,
    parser: &dyn VersionParser,
    is_dev: bool,
    output: &mut Vec<Dependency>,
) {
    for (name, version_value) in deps {
        if let Some(version_str) = version_value.as_str() {
            if let Some(spec) = parser.parse(version_str) {
                let dep = if is_dev {
                    Dependency::development(name.clone(), spec, Language::Node)
                } else {
                    Dependency::production(name.clone(), spec, Language::Node)
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
        PackageJsonParser.parse(content)
    }

    #[test]
    fn test_parse_simple_dependencies() {
        let content = r#"{
            "dependencies": {
                "lodash": "^4.17.21",
                "express": "~4.18.2"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let lodash = deps.iter().find(|d| d.name == "lodash").unwrap();
        assert_eq!(lodash.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(lodash.version_spec.version, "4.17.21");
        assert!(!lodash.is_dev);

        let express = deps.iter().find(|d| d.name == "express").unwrap();
        assert_eq!(express.version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_dev_dependencies() {
        let content = r#"{
            "devDependencies": {
                "typescript": "^5.0.0",
                "jest": "^29.0.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().all(|d| d.is_dev));
    }

    #[test]
    fn test_parse_mixed_dependencies() {
        let content = r#"{
            "dependencies": {
                "react": "^18.2.0"
            },
            "devDependencies": {
                "typescript": "^5.0.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let react = deps.iter().find(|d| d.name == "react").unwrap();
        assert!(!react.is_dev);

        let ts = deps.iter().find(|d| d.name == "typescript").unwrap();
        assert!(ts.is_dev);
    }

    #[test]
    fn test_parse_exact_version() {
        let content = r#"{
            "dependencies": {
                "pinned": "1.0.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        let pinned = deps.first().unwrap();
        assert_eq!(pinned.version_spec.kind, VersionSpecKind::Exact);
        assert!(pinned.is_pinned());
    }

    #[test]
    fn test_parse_empty_object() {
        let content = "{}";
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_peer_dependencies() {
        let content = r#"{
            "peerDependencies": {
                "react": "^18.0.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_invalid_json() {
        let content = "not json";
        let result = parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version() {
        let content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "lodash", "4.18.0")
            .unwrap();
        assert!(result.contains("^4.18.0"));
    }

    #[test]
    fn test_update_version_maintains_prefix() {
        let content = r#"{
  "dependencies": {
    "express": "~4.18.2"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "express", "4.19.0")
            .unwrap();
        assert!(result.contains("~4.19.0"));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"{
  "dependencies": {}
}"#;

        let result = PackageJsonParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_language() {
        assert_eq!(PackageJsonParser.language(), Language::Node);
    }

    #[test]
    fn test_parse_with_prerelease() {
        let content = r#"{
            "dependencies": {
                "next": "^14.0.0-canary.1"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "14.0.0-canary.1");
    }

    #[test]
    fn test_parse_wildcard() {
        let content = r#"{
            "dependencies": {
                "pkg": "*"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Wildcard);
    }
}
