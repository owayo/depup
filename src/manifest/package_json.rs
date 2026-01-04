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
use regex::Regex;
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
        let parser = get_parser(Language::Node);

        // Use regex-based text replacement to preserve original formatting and key order
        // Pattern matches: "package-name": "version" with flexible whitespace
        // Escape special characters in package name (e.g., @scope/package)
        let escaped_package = regex::escape(package);
        let pattern = format!(r#"("{}"\s*:\s*)"([^"]+)""#, escaped_package);

        let re = Regex::new(&pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("package.json"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let mut updated = false;
        let result = re.replace(content, |caps: &regex::Captures| {
            let prefix = &caps[1]; // "package": or "package" :
            let old_version = &caps[2];

            if let Some(spec) = parser.parse(old_version) {
                updated = true;
                let new_ver = spec.format_updated(new_version);
                format!(r#"{}"{}""#, prefix, new_ver)
            } else {
                // If we can't parse the version, keep the original
                caps[0].to_string()
            }
        });

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("package.json"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        Ok(result.to_string())
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

    #[test]
    fn test_update_version_preserves_key_order() {
        // Keys are intentionally NOT in alphabetical order
        let content = r#"{
  "name": "test-package",
  "version": "1.0.0",
  "dependencies": {
    "zod": "^3.0.0",
    "axios": "^1.0.0",
    "lodash": "^4.17.21"
  },
  "devDependencies": {
    "typescript": "^5.0.0"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "axios", "1.5.0")
            .unwrap();

        // Verify the original key order is preserved
        assert_eq!(result, content.replace("^1.0.0", "^1.5.0"));

        // Double-check by finding positions - zod should come before axios
        let zod_pos = result.find("\"zod\"").unwrap();
        let axios_pos = result.find("\"axios\"").unwrap();
        let lodash_pos = result.find("\"lodash\"").unwrap();
        assert!(zod_pos < axios_pos, "zod should come before axios");
        assert!(axios_pos < lodash_pos, "axios should come before lodash");
    }

    #[test]
    fn test_update_version_scoped_package() {
        let content = r#"{
  "dependencies": {
    "@types/node": "^20.0.0",
    "@scope/package": "^1.0.0"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "@types/node", "20.10.0")
            .unwrap();
        assert!(result.contains("\"@types/node\": \"^20.10.0\""));

        let result2 = PackageJsonParser
            .update_version(content, "@scope/package", "2.0.0")
            .unwrap();
        assert!(result2.contains("\"@scope/package\": \"^2.0.0\""));
    }

    #[test]
    fn test_update_version_preserves_formatting() {
        // Test various formatting styles
        let content_with_spaces = r#"{"dependencies": { "lodash" : "^4.17.21" }}"#;
        let result = PackageJsonParser
            .update_version(content_with_spaces, "lodash", "4.18.0")
            .unwrap();
        // Should preserve the original spacing around the colon
        assert!(result.contains("\"lodash\" : \"^4.18.0\""));
    }
}
