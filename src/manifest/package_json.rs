//! Node.js プロジェクト向けの `package.json` パーサ。
//!
//! 対応対象:
//! - dependencies セクション
//! - devDependencies セクション
//! - peerDependencies セクション
//! - optionalDependencies セクション

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::manifest::json_sections::{
    direct_child_object_section_ranges, replace_string_property_in_ranges,
    replace_string_property_in_top_level_sections, top_level_object_section_ranges,
};
use crate::parser::{VersionParser, get_parser};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// `package.json` 用パーサ
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

        // 通常の依存関係を解釈する
        if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        // 開発依存を解釈する
        if let Some(deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), true, &mut dependencies);
        }

        // peerDependencies は通常依存として扱う
        if let Some(deps) = json.get("peerDependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        // optionalDependencies を解釈する
        if let Some(deps) = json.get("optionalDependencies").and_then(|v| v.as_object()) {
            parse_dependency_object(deps, parser.as_ref(), false, &mut dependencies);
        }

        // Bun Catalogs は root package.json の `catalog` / `catalogs` か
        // `workspaces.catalog` / `workspaces.catalogs` で定義される。
        parse_bun_catalogs(&json, parser.as_ref(), &mut dependencies);

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

        // 元の整形とキー順を保つため、依存セクション内だけをテキスト置換で更新する
        let sections = [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ];
        let (result, updated) = replace_string_property_in_top_level_sections(
            content,
            &sections,
            package,
            |old_version| format_node_update(parser.as_ref(), old_version, new_version),
        )
        .map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("package.json"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let (result, catalog_updated) =
            replace_bun_catalog_versions(&result, package, new_version, parser.as_ref()).map_err(
                |e| ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("package.json"),
                    spec: package.to_string(),
                    message: format!("invalid regex pattern: {}", e),
                },
            )?;
        let updated = updated || catalog_updated;

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

/// npm alias 用に `(解釈対象, alias 接頭辞)` を返す。更新不可な protocol は `None`。
fn normalize_node_constraint(version: &str) -> Option<(&str, Option<String>)> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return None;
    }

    // npm エイリアス: `npm:real-package@^1.2.3`
    if let Some(rest) = trimmed.strip_prefix("npm:") {
        if let Some(at_pos) = rest.rfind('@')
            && at_pos > 0
            && at_pos + 1 < rest.len()
        {
            let prefix = format!("npm:{}@", &rest[..at_pos]);
            return Some((&rest[at_pos + 1..], Some(prefix)));
        }
        return None;
    }

    // レジストリの semver 制約ではない protocol 参照
    const NON_UPDATABLE_PREFIXES: &[&str] = &[
        "workspace:",
        "file:",
        "link:",
        "git+",
        "git://",
        "github:",
        "http://",
        "https://",
        "ssh://",
        "portal:",
        "patch:",
        "catalog:",
    ];
    if NON_UPDATABLE_PREFIXES
        .iter()
        .any(|p| trimmed.starts_with(p))
    {
        return None;
    }

    Some((trimmed, None))
}

fn format_node_update(
    parser: &dyn VersionParser,
    old_version: &str,
    new_version: &str,
) -> Option<String> {
    let (parse_target, alias_prefix) = normalize_node_constraint(old_version)?;
    let spec = parser.parse(parse_target)?;
    let new_ver = spec.try_format_updated(new_version)?;
    Some(if let Some(alias) = alias_prefix {
        format!("{}{}", alias, new_ver)
    } else {
        new_ver
    })
}

/// alias 接頭辞 `npm:<real>@` から実パッケージ名を取り出す
fn real_package_name_from_alias_prefix(prefix: &str) -> Option<&str> {
    prefix.strip_prefix("npm:")?.strip_suffix('@')
}

fn parse_dependency_object(
    deps: &Map<String, Value>,
    parser: &dyn VersionParser,
    is_dev: bool,
    output: &mut Vec<Dependency>,
) {
    for (name, version_value) in deps {
        if let Some(version_str) = version_value.as_str()
            && let Some((parse_target, alias_prefix)) = normalize_node_constraint(version_str)
            && let Some(spec) = parser.parse(parse_target)
        {
            // npm alias (`npm:real-package@^1.2.3`) ではレジストリ照会に実パッケージ名を
            // 使い、書き戻しには JSON キーを使う (Cargo の rename 依存と同じパターン)
            let package_name = alias_prefix
                .as_deref()
                .and_then(real_package_name_from_alias_prefix)
                .unwrap_or(name.as_str());
            let mut dep = if is_dev {
                Dependency::development(package_name, spec, Language::Node)
            } else {
                Dependency::production(package_name, spec, Language::Node)
            }
            .with_manifest_name(name.clone());
            // `npm:<real>@` は制約の一部ではなくマニフェスト上の値の接頭辞。
            // 保持しないと `--diff` が alias を剥がした表示になり、実書き込み
            // (`format_node_update` が接頭辞を復元する) と食い違う
            if let Some(prefix) = alias_prefix {
                dep = dep.with_value_prefix(prefix);
            }
            output.push(dep);
        }
    }
}

fn parse_bun_catalogs(json: &Value, parser: &dyn VersionParser, output: &mut Vec<Dependency>) {
    if let Some(root) = json.as_object() {
        parse_bun_catalog_container(root, parser, output);
    }

    if let Some(workspaces) = json.get("workspaces").and_then(|v| v.as_object()) {
        parse_bun_catalog_container(workspaces, parser, output);
    }
}

fn parse_bun_catalog_container(
    container: &Map<String, Value>,
    parser: &dyn VersionParser,
    output: &mut Vec<Dependency>,
) {
    if let Some(catalog) = container.get("catalog").and_then(|v| v.as_object()) {
        parse_dependency_object(catalog, parser, false, output);
    }

    if let Some(catalogs) = container.get("catalogs").and_then(|v| v.as_object()) {
        for catalog in catalogs.values().filter_map(|v| v.as_object()) {
            parse_dependency_object(catalog, parser, false, output);
        }
    }
}

fn bun_catalog_object_ranges(content: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();

    // トップレベルの `catalog`
    ranges.extend(top_level_object_section_ranges(content, &["catalog"]));

    // トップレベルの `catalogs.<name>`
    let top_catalogs = top_level_object_section_ranges(content, &["catalogs"]);
    ranges.extend(direct_child_object_section_ranges(
        content,
        &top_catalogs,
        None,
    ));

    // `workspaces.catalog` を処理する
    let workspaces = top_level_object_section_ranges(content, &["workspaces"]);
    ranges.extend(direct_child_object_section_ranges(
        content,
        &workspaces,
        Some(&["catalog"]),
    ));

    // `workspaces.catalogs.<name>` を処理する
    let workspace_catalogs =
        direct_child_object_section_ranges(content, &workspaces, Some(&["catalogs"]));
    ranges.extend(direct_child_object_section_ranges(
        content,
        &workspace_catalogs,
        None,
    ));

    ranges
}

fn replace_bun_catalog_versions(
    content: &str,
    package: &str,
    new_version: &str,
    parser: &dyn VersionParser,
) -> Result<(String, bool), regex::Error> {
    let ranges = bun_catalog_object_ranges(content);
    let mut transform = |old_version: &str| format_node_update(parser, old_version, new_version);
    replace_string_property_in_ranges(content, ranges, package, &mut transform)
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
    fn test_update_version_ignores_overrides_section() {
        let content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  },
  "overrides": {
    "lodash": "4.17.20"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "lodash", "4.18.0")
            .unwrap();
        assert!(result.contains(r#""lodash": "^4.18.0""#));
        assert!(result.contains(r#""lodash": "4.17.20""#));
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
                "pkg": "1.x"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Wildcard);
    }

    #[test]
    fn test_parse_skips_bare_wildcard() {
        let content = r#"{
            "dependencies": {
                "pkg": "*"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_update_version_preserves_wildcard_shape() {
        let content = r#"{
  "dependencies": {
    "pkg": "1.x"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "pkg", "2.3.4")
            .unwrap();
        assert!(result.contains("\"pkg\": \"2.x\""));
    }

    #[test]
    fn test_update_version_preserves_equal_wildcard_shape() {
        let content = r#"{
  "dependencies": {
    "pkg": "=1.x"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "pkg", "2.3.4")
            .unwrap();
        assert!(result.contains("\"pkg\": \"=2.x\""));
    }

    #[test]
    fn test_update_version_preserves_full_tuple_wildcard_shape() {
        let content = r#"{
  "dependencies": {
    "pkg": "1.x.x"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "pkg", "2.3.4")
            .unwrap();
        assert!(result.contains("\"pkg\": \"2.x.x\""));
    }

    #[test]
    fn test_update_version_preserves_v_prefix_wildcard_shape() {
        let content = r#"{
  "dependencies": {
    "pkg": "v1.*"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "pkg", "2.3.4")
            .unwrap();
        assert!(result.contains("\"pkg\": \"v2.*\""));
    }

    #[test]
    fn test_update_version_range_keeps_upper_bound() {
        let content = r#"{
  "dependencies": {
    "pkg": ">=1.0.0 <2.0.0"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "pkg", "1.9.3")
            .unwrap();
        assert!(result.contains("\"pkg\": \">=1.9.3 <2.0.0\""));
    }

    #[test]
    fn test_update_version_bare_partial_range_keeps_upper_bound() {
        let content = r#"{
  "dependencies": {
    "pkg": "1.2 <2.0.0"
  }
}"#;

        let deps = parse(content).unwrap();
        let pkg = deps.iter().find(|d| d.name == "pkg").unwrap();
        assert_eq!(pkg.version_spec.kind, VersionSpecKind::Range);
        assert_eq!(pkg.version_spec.version, "1.2.0");

        let result = PackageJsonParser
            .update_version(content, "pkg", "1.9.3")
            .unwrap();
        assert!(result.contains("\"pkg\": \"1.9 <2.0.0\""));
    }

    #[test]
    fn test_update_version_or_constraint_returns_err() {
        let content = r#"{
  "dependencies": {
    "pkg": "^1 || ^2"
  }
}"#;

        let result = PackageJsonParser.update_version(content, "pkg", "2.5.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_preserves_key_order() {
        // キー順保持を確認するため、あえてアルファベット順にしていない
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

        // 元のキー順が保たれることを確認する
        assert_eq!(result, content.replace("^1.0.0", "^1.5.0"));

        // 位置でも再確認し、`zod` が `axios` より前にあることを確かめる
        let zod_pos = result.find("\"zod\"").unwrap();
        let axios_pos = result.find("\"axios\"").unwrap();
        let lodash_pos = result.find("\"lodash\"").unwrap();
        assert!(zod_pos < axios_pos, "zod は axios より前にあるべき");
        assert!(axios_pos < lodash_pos, "axios は lodash より前にあるべき");
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
        // さまざまな空白パターンでも書式を保つ
        let content_with_spaces = r#"{"dependencies": { "lodash" : "^4.17.21" }}"#;
        let result = PackageJsonParser
            .update_version(content_with_spaces, "lodash", "4.18.0")
            .unwrap();
        // コロン前後の空白を維持する
        assert!(result.contains("\"lodash\" : \"^4.18.0\""));
    }

    #[test]
    fn test_parse_ignores_non_string_versions() {
        // 文字列以外のバージョン値は無視する
        let content = r#"{
            "dependencies": {
                "lodash": "^4.17.21",
                "local": { "path": "../local" },
                "num": 1,
                "flag": true,
                "nil": null
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "lodash");
    }

    #[test]
    fn test_parse_ignores_link_protocol() {
        // `link:` 依存は無視する
        let content = r#"{
            "dependencies": {
                "lodash": "^4.17.21",
                "local-pkg": "link:../packages/local"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "lodash");
    }

    #[test]
    fn test_parse_ignores_file_protocol() {
        // `file:` 依存は無視する
        let content = r#"{
            "dependencies": {
                "express": "^4.18.0",
                "my-local": "file:../my-local"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "express");
    }

    #[test]
    fn test_parse_ignores_git_url() {
        // `git://` と `github:` URL は無視する
        let content = r#"{
            "dependencies": {
                "axios": "^1.0.0",
                "git-dep": "git://github.com/user/repo.git",
                "github-dep": "github:user/repo"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "axios");
    }

    #[test]
    fn test_parse_workspace_protocol() {
        // `workspace:` protocol は検出されても更新対象にはしない
        let content = r#"{
            "dependencies": {
                "lodash": "^4.17.21",
                "shared": "workspace:*"
            }
        }"#;

        let deps = parse(content).unwrap();
        // `workspace:` 依存は更新対象バージョンを持たないので無視する
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "lodash");
    }

    #[test]
    fn test_parse_empty_version_string() {
        // 空文字のバージョンは無視する
        let content = r#"{
            "dependencies": {
                "valid": "^1.0.0",
                "empty": ""
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "valid");
    }

    #[test]
    fn test_parse_npm_alias_dependency() {
        let content = r#"{
            "dependencies": {
                "ui": "npm:@mui/lab@^7.0.0"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        // レジストリ照会には実パッケージ名、書き戻しには JSON キーを使う
        assert_eq!(deps[0].name, "@mui/lab");
        assert_eq!(deps[0].manifest_name(), "ui");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "7.0.0");
    }

    #[test]
    fn test_parse_npm_alias_uses_real_package_name_for_registry() {
        // (回帰) `"react": "npm:@preact/compat@^17.1.2"` で alias キー (`react`) の
        // レジストリ情報により別パッケージとして判定されるバグの修正確認
        let content = r#"{
            "dependencies": {
                "react": "npm:@preact/compat@^17.1.2",
                "lodash": "^4.17.21"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let aliased = deps.iter().find(|d| d.manifest_name() == "react").unwrap();
        assert_eq!(aliased.name, "@preact/compat");
        assert_eq!(aliased.version_spec.version, "17.1.2");
        assert_eq!(aliased.manifest_name, Some("react".to_string()));

        // alias でない依存は manifest_name を持たない (name と同一のため)
        let lodash = deps.iter().find(|d| d.name == "lodash").unwrap();
        assert!(lodash.manifest_name.is_none());
        assert_eq!(lodash.manifest_name(), "lodash");
    }

    #[test]
    fn test_parse_npm_alias_unscoped_package() {
        let content = r#"{
            "devDependencies": {
                "my-lodash": "npm:lodash@^4.17.21"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "lodash");
        assert_eq!(deps[0].manifest_name(), "my-lodash");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_update_npm_alias_dependency() {
        let content = r#"{
  "dependencies": {
    "ui": "npm:@mui/lab@^7.0.0"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "ui", "7.1.0")
            .unwrap();
        assert!(result.contains(r#""ui": "npm:@mui/lab@^7.1.0""#));
    }

    #[test]
    fn test_parse_bun_top_level_catalogs() {
        let content = r#"{
            "catalog": {
                "react": "^19.0.0"
            },
            "catalogs": {
                "testing": {
                    "jest": "30.0.0"
                }
            },
            "dependencies": {
                "react": "catalog:",
                "jest": "catalog:testing"
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let react = deps.iter().find(|d| d.name == "react").unwrap();
        assert_eq!(react.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(react.version_spec.version, "19.0.0");

        let jest = deps.iter().find(|d| d.name == "jest").unwrap();
        assert_eq!(jest.version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(jest.version_spec.version, "30.0.0");
    }

    #[test]
    fn test_parse_bun_workspaces_catalogs() {
        let content = r#"{
            "workspaces": {
                "packages": ["packages/*"],
                "catalog": {
                    "react": "^19.0.0"
                },
                "catalogs": {
                    "build": {
                        "webpack": "5.88.2"
                    }
                }
            }
        }"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.name == "react"));
        assert!(deps.iter().any(|d| d.name == "webpack"));
    }

    #[test]
    fn test_update_bun_catalogs_preserves_catalog_references() {
        let content = r#"{
  "workspaces": {
    "packages": ["packages/*"],
    "catalog": {
      "react": "^19.0.0"
    },
    "catalogs": {
      "testing": {
        "jest": "30.0.0"
      }
    }
  },
  "dependencies": {
    "react": "catalog:",
    "jest": "catalog:testing"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "react", "19.1.0")
            .unwrap();
        assert!(result.contains(r#""react": "^19.1.0""#));
        assert!(result.contains(r#""react": "catalog:""#));

        let result = PackageJsonParser
            .update_version(&result, "jest", "30.1.0")
            .unwrap();
        assert!(result.contains(r#""jest": "30.1.0""#));
        assert!(result.contains(r#""jest": "catalog:testing""#));
    }

    #[test]
    fn test_update_bun_top_level_catalogs() {
        let content = r#"{
  "catalog": {
    "react": "^19.0.0"
  },
  "catalogs": {
    "testing": {
      "jest": "30.0.0"
    }
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "react", "19.1.0")
            .unwrap();
        assert!(result.contains(r#""react": "^19.1.0""#));

        let result = PackageJsonParser
            .update_version(&result, "jest", "30.1.0")
            .unwrap();
        assert!(result.contains(r#""jest": "30.1.0""#));
    }

    #[test]
    fn test_update_bun_catalogs_when_ranges_are_not_discovered_in_file_order() {
        let content = r#"{
  "workspaces": {
    "catalog": {
      "react": "^19.0.0"
    }
  },
  "catalog": {
    "react": "^19.0.0"
  }
}"#;

        let result = PackageJsonParser
            .update_version(content, "react", "19.1.0")
            .unwrap();
        assert_eq!(result.matches(r#""react": "^19.1.0""#).count(), 2);
    }
}
