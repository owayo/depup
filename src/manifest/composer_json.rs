//! PHP プロジェクト向けの `composer.json` パーサ。
//!
//! 対応対象:
//! - `require`（本番依存）
//! - `require-dev`（開発依存）
//! - PHP プラットフォームパッケージ（`php`, `ext-*` など）の除外
//! - 更新時のバージョン制約保持

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::manifest::json_sections::replace_string_property_in_top_level_sections;
use crate::parser::get_parser;
use serde_json::{Map, Value};
use std::path::PathBuf;

/// `composer.json` 用パーサ
pub struct ComposerJsonParser;

impl ManifestParser for ComposerJsonParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let json: Value =
            serde_json::from_str(content).map_err(|e| ManifestError::JsonParseError {
                path: PathBuf::from("composer.json"),
                message: e.to_string(),
            })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Php);

        // `require`（本番依存）を読む
        if let Some(deps) = json.get("require").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        // `require-dev`（開発依存）を読む
        if let Some(deps) = json.get("require-dev").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), true, &mut dependencies);
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Php
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parser = get_parser(Language::Php);

        // 元の整形を保つため、依存セクション内だけをテキスト置換で更新する
        let sections = ["require", "require-dev"];
        let (result, updated) = replace_string_property_in_top_level_sections(
            content,
            &sections,
            package,
            |old_version| {
                if let Some(spec) = parser.parse(old_version)
                    && let Some(new_ver) = spec.try_format_updated(new_version)
                {
                    return Some(new_ver);
                }
                // 解釈できない制約は元の値を維持する
                None
            },
        )
        .map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("composer.json"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("composer.json"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        Ok(result.to_string())
    }
}

/// パッケージ名がプラットフォームパッケージかどうかを返す
fn is_platform_package(name: &str) -> bool {
    name == "php"
        || name == "hhvm"
        || name.starts_with("php-")
        || name.starts_with("ext-")
        || name.starts_with("lib-")
        || name == "composer"
        || name == "composer-plugin-api"
        || name == "composer-runtime-api"
}

fn parse_dependency_object(
    deps: &Map<String, Value>,
    parser: &dyn crate::parser::VersionParser,
    is_dev: bool,
    output: &mut Vec<Dependency>,
) {
    for (name, version_value) in deps {
        // プラットフォームパッケージは更新対象から除外する
        if is_platform_package(name) {
            continue;
        }

        if let Some(version_str) = version_value.as_str()
            && let Some(spec) = parser.parse(version_str)
        {
            let dep = if is_dev {
                Dependency::development(name.clone(), spec, Language::Php)
            } else {
                Dependency::production(name.clone(), spec, Language::Php)
            };
            output.push(dep);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        ComposerJsonParser.parse(content)
    }

    #[test]
    fn test_parse_simple_require() {
        let content = r#"{
            "require": {
                "monolog/monolog": "^3.0",
                "symfony/console": "~6.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let monolog = deps.iter().find(|d| d.name == "monolog/monolog").unwrap();
        assert_eq!(monolog.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(monolog.version_spec.version, "3.0");
        assert!(!monolog.is_dev);

        let console = deps.iter().find(|d| d.name == "symfony/console").unwrap();
        assert_eq!(console.version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_require_dev() {
        let content = r#"{
            "require-dev": {
                "phpunit/phpunit": "^10.0",
                "phpstan/phpstan": "^1.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().all(|d| d.is_dev));
    }

    #[test]
    fn test_parse_mixed_dependencies() {
        let content = r#"{
            "require": {
                "laravel/framework": "^10.0"
            },
            "require-dev": {
                "phpunit/phpunit": "^10.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let laravel = deps.iter().find(|d| d.name == "laravel/framework").unwrap();
        assert!(!laravel.is_dev);

        let phpunit = deps.iter().find(|d| d.name == "phpunit/phpunit").unwrap();
        assert!(phpunit.is_dev);
    }

    #[test]
    fn test_parse_exact_version() {
        let content = r#"{
            "require": {
                "vendor/package": "1.0.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        let pkg = deps.first().unwrap();
        assert_eq!(pkg.version_spec.kind, VersionSpecKind::Exact);
        assert!(pkg.is_pinned());
    }

    #[test]
    fn test_parse_wildcard_version() {
        let content = r#"{
            "require": {
                "vendor/package": "1.2.*"
            }
        }"#;

        let deps = parse(content).unwrap();
        let pkg = deps.first().unwrap();
        assert_eq!(pkg.version_spec.kind, VersionSpecKind::Wildcard);
        assert!(!pkg.is_pinned());
    }

    #[test]
    fn test_parse_range_version() {
        let content = r#"{
            "require": {
                "vendor/package": ">=1.0 <2.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        let pkg = deps.first().unwrap();
        assert_eq!(pkg.version_spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_skip_platform_packages() {
        let content = r#"{
            "require": {
                "php": ">=8.1",
                "hhvm": "^4.0",
                "ext-json": "*",
                "ext-mbstring": "*",
                "lib-curl": ">=7.0",
                "monolog/monolog": "^3.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "monolog/monolog");
    }

    #[test]
    fn test_skip_composer_packages() {
        let content = r#"{
            "require": {
                "composer-plugin-api": "^2.0",
                "composer-runtime-api": "^2.0",
                "vendor/package": "^1.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vendor/package");
    }

    #[test]
    fn test_parse_empty_object() {
        let content = "{}";
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_invalid_json() {
        let content = "not json";
        let result = parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_caret() {
        let content = r#"{
  "require": {
    "monolog/monolog": "^3.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "monolog/monolog", "3.5.0")
            .unwrap();
        assert!(result.contains("^3.5.0"));
    }

    #[test]
    fn test_update_version_tilde() {
        let content = r#"{
  "require": {
    "symfony/console": "~6.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "symfony/console", "6.4.0")
            .unwrap();
        assert!(result.contains("~6.4.0"));
    }

    #[test]
    fn test_update_version_exact() {
        let content = r#"{
  "require": {
    "vendor/package": "1.0.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "vendor/package", "2.0.0")
            .unwrap();
        assert!(result.contains("\"2.0.0\""));
    }

    #[test]
    fn test_update_version_ignores_replace_section() {
        let content = r#"{
  "require": {
    "vendor/package": "^1.0"
  },
  "replace": {
    "vendor/package": "1.0.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "vendor/package", "1.2.0")
            .unwrap();
        assert!(result.contains(r#""vendor/package": "^1.2.0""#));
        assert!(result.contains(r#""vendor/package": "1.0.0""#));
    }

    #[test]
    fn test_update_version_wildcard() {
        let content = r#"{
  "require": {
    "vendor/package": "1.2.*"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "vendor/package", "1.3.4")
            .unwrap();
        assert!(result.contains("\"1.3.*\""));
    }

    #[test]
    fn test_update_version_gte() {
        let content = r#"{
  "require": {
    "vendor/package": ">=1.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "vendor/package", "2.0")
            .unwrap();
        assert!(result.contains("\">=2.0\""));
    }

    #[test]
    fn test_update_version_range_keeps_upper_bound() {
        let content = r#"{
  "require": {
    "vendor/package": ">=1.0 <2.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "vendor/package", "1.9.3")
            .unwrap();
        assert!(result.contains("\">=1.9.3 <2.0\""));
    }

    #[test]
    fn test_update_version_or_constraint_returns_err() {
        let content = r#"{
  "require": {
    "vendor/package": "^1.0 || ^2.0"
  }
}"#;

        let result = ComposerJsonParser.update_version(content, "vendor/package", "2.5.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_compound_not_equal_constraint_returns_err() {
        let content = r#"{
  "require": {
    "vendor/package": ">=1.0 !=1.5.0 <2.0"
  }
}"#;

        let result = ComposerJsonParser.update_version(content, "vendor/package", "1.9.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"{
  "require": {}
}"#;

        let result = ComposerJsonParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_preserves_key_order() {
        let content = r#"{
  "require": {
    "symfony/console": "^6.0",
    "laravel/framework": "^10.0",
    "monolog/monolog": "^3.0"
  }
}"#;

        let result = ComposerJsonParser
            .update_version(content, "laravel/framework", "10.5.0")
            .unwrap();

        // 元のキー順が維持されることを確認する
        let symfony_pos = result.find("\"symfony/console\"").unwrap();
        let laravel_pos = result.find("\"laravel/framework\"").unwrap();
        let monolog_pos = result.find("\"monolog/monolog\"").unwrap();

        assert!(
            symfony_pos < laravel_pos,
            "symfony should come before laravel"
        );
        assert!(
            laravel_pos < monolog_pos,
            "laravel should come before monolog"
        );
    }

    #[test]
    fn test_update_version_preserves_formatting() {
        let content = r#"{"require": { "vendor/package" : "^1.0" }}"#;
        let result = ComposerJsonParser
            .update_version(content, "vendor/package", "2.0")
            .unwrap();
        assert!(result.contains("\"vendor/package\" : \"^2.0\""));
    }

    #[test]
    fn test_language() {
        assert_eq!(ComposerJsonParser.language(), Language::Php);
    }

    #[test]
    fn test_parse_or_constraint() {
        let content = r#"{
            "require": {
                "vendor/package": "^1.0 || ^2.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_gte_version() {
        let content = r#"{
            "require": {
                "vendor/package": ">=1.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        let pkg = deps.first().unwrap();
        assert_eq!(pkg.version_spec.kind, VersionSpecKind::GreaterOrEqual);
    }
}
