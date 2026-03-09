//! Java プロジェクト向け Gradle マニフェストパーサ
//!
//! 対応内容:
//! - build.gradle (Groovy DSL)
//! - build.gradle.kts (Kotlin DSL)
//! - 変数定義 (def, val, ext block)
//! - map 記法依存: group: 'x', name: 'y', version: 'z'
//! - 文字列記法依存: 'group:name:version'
//! - バージョン内の変数参照

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::get_parser;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

/// build.gradle / build.gradle.kts 用パーサ
pub struct GradleParser;

/// 変数定義情報
#[derive(Debug, Clone)]
struct VariableDefinition {
    /// 変数の値
    value: String,
    /// 行番号 (1-based)
    line_number: usize,
    /// 使用されているクォート文字 (' または ")
    quote_char: char,
}

// Gradle DSL 用の正規表現

// 変数定義 (Groovy): def wicketVersion = '1.2.3' または "1.2.3"
static VAR_DEF_GROOVY_SINGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*def\s+(\w+)\s*=\s*'([^']+)'"#).unwrap());
static VAR_DEF_GROOVY_DOUBLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*def\s+(\w+)\s*=\s*"([^"]+)""#).unwrap());

// 変数定義 (Kotlin): val wicketVersion = "1.2.3"
static VAR_DEF_KOTLIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*val\s+(\w+)\s*=\s*"([^"]+)""#).unwrap());

// ext ブロック内変数: wicketVersion = '1.2.3' または "1.2.3"
static EXT_VAR_SINGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(\w+)\s*=\s*'([^']+)'"#).unwrap());
static EXT_VAR_DOUBLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(\w+)\s*=\s*"([^"]+)""#).unwrap());

// ext ブロック開始
static EXT_BLOCK_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*ext\s*\{").unwrap());

// map 記法依存: implementation group: 'x', name: 'y', version: 'z'
// implementation(group: 'x', name: 'y', version: 'z') も処理
// 非後方参照パターンを使用 (シングル/ダブルクォート両対応)
static DEP_MAP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^\s*(\w+)\s*[\(\s]+group:\s*['"]([^'"]+)['"]\s*,\s*name:\s*['"]([^'"]+)['"]\s*,\s*version:\s*['"]?([^'",\)\s]+)['"]?"#,
    )
    .unwrap()
});

// 文字列記法依存: implementation 'group:name:version'
// 非後方参照パターンを使用 (シングル/ダブルクォート両対応)
static DEP_STRING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(\w+)\s*[\(\s]*['"]([^:'"]+):([^:'"]+):([^'"]+)['"]"#).unwrap()
});

// 変数展開あり文字列記法: implementation "group:name:$version"
static DEP_STRING_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(\w+)\s*[\(\s]*"([^:"]+):([^:"]+):\$\{?(\w+)\}?""#).unwrap()
});

// 開発用 configuration
const DEV_CONFIGURATIONS: [&str; 6] = [
    "testImplementation",
    "testCompileOnly",
    "testRuntimeOnly",
    "testApi",
    "androidTestImplementation",
    "debugImplementation",
];

impl GradleParser {
    /// content から変数定義を抽出
    fn extract_variables(&self, content: &str) -> HashMap<String, VariableDefinition> {
        let mut variables = HashMap::new();
        let mut in_ext_block = false;
        let mut brace_depth = 0;

        for (line_idx, line) in content.lines().enumerate() {
            let line_number = line_idx + 1;
            let trimmed = line.trim();

            // 空行とコメントをスキップ
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }

            // ext ブロックの開始/終了を追跡
            if EXT_BLOCK_START.is_match(trimmed) {
                in_ext_block = true;
                brace_depth = 1;
                // 1 行完結の ext ブロックを判定
                if trimmed.contains('}') {
                    brace_depth = 0;
                    in_ext_block = false;
                }
                continue;
            }

            // ext ブロック内のネスト深さを追跡
            if in_ext_block {
                brace_depth += trimmed.matches('{').count();
                brace_depth = brace_depth.saturating_sub(trimmed.matches('}').count());
                if brace_depth == 0 {
                    in_ext_block = false;
                }
            }

            // Groovy def 変数 (シングルクォート) を判定
            if let Some(caps) = VAR_DEF_GROOVY_SINGLE.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                if !name.is_empty() && !value.is_empty() {
                    variables.insert(
                        name.to_string(),
                        VariableDefinition {
                            value: value.to_string(),
                            line_number,
                            quote_char: '\'',
                        },
                    );
                }
                continue;
            }

            // Groovy def 変数 (ダブルクォート) を判定
            if let Some(caps) = VAR_DEF_GROOVY_DOUBLE.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                if !name.is_empty() && !value.is_empty() {
                    variables.insert(
                        name.to_string(),
                        VariableDefinition {
                            value: value.to_string(),
                            line_number,
                            quote_char: '"',
                        },
                    );
                }
                continue;
            }

            // Kotlin val 変数を判定
            if let Some(caps) = VAR_DEF_KOTLIN.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                if !name.is_empty() && !value.is_empty() {
                    variables.insert(
                        name.to_string(),
                        VariableDefinition {
                            value: value.to_string(),
                            line_number,
                            quote_char: '"',
                        },
                    );
                }
                continue;
            }

            // ext ブロック変数 (シングルクォート) を判定
            if in_ext_block {
                if let Some(caps) = EXT_VAR_SINGLE.captures(line) {
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                    // 一般的な非バージョン変数は除外
                    if !name.is_empty()
                        && !value.is_empty()
                        && !name.starts_with("source")
                        && !name.starts_with("target")
                        && name != "encoding"
                    {
                        variables.insert(
                            name.to_string(),
                            VariableDefinition {
                                value: value.to_string(),
                                line_number,
                                quote_char: '\'',
                            },
                        );
                    }
                    continue;
                }

                // ext ブロック変数 (ダブルクォート) を判定
                if let Some(caps) = EXT_VAR_DOUBLE.captures(line) {
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                    // 一般的な非バージョン変数は除外
                    if !name.is_empty()
                        && !value.is_empty()
                        && !name.starts_with("source")
                        && !name.starts_with("target")
                        && name != "encoding"
                    {
                        variables.insert(
                            name.to_string(),
                            VariableDefinition {
                                value: value.to_string(),
                                line_number,
                                quote_char: '"',
                            },
                        );
                    }
                }
            }
        }

        variables
    }

    /// map 記法依存をパース
    fn parse_map_notation(
        &self,
        line: &str,
        variables: &HashMap<String, VariableDefinition>,
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<(Dependency, Option<String>)> {
        let caps = DEP_MAP.captures(line)?;

        let config = caps.get(1).map(|m| m.as_str())?;
        let group = caps.get(2).map(|m| m.as_str())?;
        let artifact = caps.get(3).map(|m| m.as_str())?;
        let version_raw = caps.get(4).map(|m| m.as_str())?;

        // version が変数参照か判定
        let (version, variable_name) = self.resolve_version(version_raw, variables);

        // バージョン文字列をパース
        let spec = if version.is_empty() {
            VersionSpec::new(VersionSpecKind::Any, "", "")
        } else {
            parser.parse(&version)?
        };

        let is_dev = DEV_CONFIGURATIONS.contains(&config);
        let name = format!("{}:{}", group, artifact);

        let dep = if is_dev {
            Dependency::development(name, spec, Language::Java)
        } else {
            Dependency::production(name, spec, Language::Java)
        };

        Some((dep, variable_name))
    }

    /// 文字列記法依存をパース
    fn parse_string_notation(
        &self,
        line: &str,
        variables: &HashMap<String, VariableDefinition>,
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<(Dependency, Option<String>)> {
        // まず変数展開あり文字列記法を試す
        if let Some(caps) = DEP_STRING_VAR.captures(line) {
            let config = caps.get(1).map(|m| m.as_str())?;
            let group = caps.get(2).map(|m| m.as_str())?;
            let artifact = caps.get(3).map(|m| m.as_str())?;
            let var_name = caps.get(4).map(|m| m.as_str())?;

            // 変数参照を解決
            let version = variables
                .get(var_name)
                .map(|v| v.value.clone())
                .unwrap_or_default();

            let spec = if version.is_empty() {
                VersionSpec::new(VersionSpecKind::Any, "", "")
            } else {
                parser.parse(&version)?
            };

            let is_dev = DEV_CONFIGURATIONS.contains(&config);
            let name = format!("{}:{}", group, artifact);

            let dep = if is_dev {
                Dependency::development(name, spec, Language::Java)
            } else {
                Dependency::production(name, spec, Language::Java)
            };

            return Some((dep, Some(var_name.to_string())));
        }

        // 通常の文字列記法を試す
        let caps = DEP_STRING.captures(line)?;

        let config = caps.get(1).map(|m| m.as_str())?;
        let group = caps.get(2).map(|m| m.as_str())?;
        let artifact = caps.get(3).map(|m| m.as_str())?;
        let version = caps.get(4).map(|m| m.as_str())?;

        let spec = parser.parse(version)?;
        let is_dev = DEV_CONFIGURATIONS.contains(&config);
        let name = format!("{}:{}", group, artifact);

        let dep = if is_dev {
            Dependency::development(name, spec, Language::Java)
        } else {
            Dependency::production(name, spec, Language::Java)
        };

        Some((dep, None))
    }

    /// 変数参照を考慮してバージョン値を解決
    fn resolve_version(
        &self,
        version_raw: &str,
        variables: &HashMap<String, VariableDefinition>,
    ) -> (String, Option<String>) {
        let trimmed = version_raw.trim();

        // 変数参照パターンを判定
        // パターン1: $variableName
        // パターン2: ${variableName}
        // パターン3: variableName (非クォート)

        let var_name =
            if let Some(inner) = trimmed.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
                Some(inner)
            } else if let Some(stripped) = trimmed.strip_prefix('$') {
                Some(stripped)
            } else if !trimmed.starts_with('\'')
                && !trimmed.starts_with('"')
                && !trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
            {
                // 非クォートかつ非数値始まりは変数の可能性がある
                Some(trimmed)
            } else {
                None
            };

        if let Some(var_name) = var_name
            && let Some(var_def) = variables.get(var_name)
        {
            return (var_def.value.clone(), Some(var_name.to_string()));
        }

        // 変数参照でなければそのまま返す (前後のクォートのみ除去)
        let version = trimmed
            .trim_start_matches(['\'', '"'])
            .trim_end_matches(['\'', '"']);
        (version.to_string(), None)
    }
}

impl ManifestParser for GradleParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Java);
        let variables = self.extract_variables(content);

        for line in content.lines() {
            let trimmed = line.trim();

            // 空行とコメントをスキップ
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }

            // map 記法を先に試す
            if let Some((dep, var_name)) =
                self.parse_map_notation(line, &variables, parser.as_ref())
            {
                let dep = if let Some(ref name) = var_name {
                    dep.with_variable(name)
                } else {
                    dep
                };
                dependencies.push(dep);
                continue;
            }

            // 文字列記法を試す
            if let Some((dep, var_name)) =
                self.parse_string_notation(line, &variables, parser.as_ref())
            {
                let dep = if let Some(ref name) = var_name {
                    dep.with_variable(name)
                } else {
                    dep
                };
                dependencies.push(dep);
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Java
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parser = get_parser(Language::Java);
        let variables = self.extract_variables(content);

        // このパッケージに使われている変数名を特定
        let mut variable_for_package: Option<String> = None;

        for line in content.lines() {
            // map 記法を確認
            if let Some((_dep, var_name)) =
                self.parse_map_notation(line, &variables, parser.as_ref())
                && _dep.name == package
            {
                variable_for_package = var_name;
                break;
            }

            // 文字列記法を確認
            if let Some((_dep, var_name)) =
                self.parse_string_notation(line, &variables, parser.as_ref())
                && _dep.name == package
            {
                variable_for_package = var_name;
                break;
            }
        }

        // 変数経由の場合は変数定義を更新
        if let Some(var_name) = variable_for_package
            && let Some(var_def) = variables.get(&var_name)
        {
            return self.update_variable_definition(content, var_def, new_version);
        }

        // それ以外は依存行の直接バージョンを更新
        self.update_direct_version(content, package, new_version)
    }
}

impl GradleParser {
    /// 新しいバージョンで変数定義を更新
    fn update_variable_definition(
        &self,
        content: &str,
        var_def: &VariableDefinition,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let lines: Vec<&str> = content.lines().collect();
        let mut result = Vec::new();
        let quote = var_def.quote_char;
        let version_parser = get_parser(Language::Java);
        let formatted_version = version_parser
            .parse(&var_def.value)
            .map(|spec| spec.format_updated(new_version))
            .unwrap_or_else(|| new_version.to_string());

        for (idx, line) in lines.iter().enumerate() {
            let line_number = idx + 1;

            if line_number == var_def.line_number {
                // 更新対象行。元の構造を保持して置換する

                // def variable = 'value' (シングルクォート)
                if let Some(caps) = VAR_DEF_GROOVY_SINGLE.captures(line) {
                    let prefix = &line[..caps.get(0).unwrap().start()];
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    result.push(format!(
                        "{}def {} = {}{}{}",
                        prefix, name, quote, formatted_version, quote
                    ));
                    continue;
                }

                // def variable = "value" (ダブルクォート)
                if let Some(caps) = VAR_DEF_GROOVY_DOUBLE.captures(line) {
                    let prefix = &line[..caps.get(0).unwrap().start()];
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    result.push(format!(
                        "{}def {} = {}{}{}",
                        prefix, name, quote, formatted_version, quote
                    ));
                    continue;
                }

                // val variable = "value"
                if let Some(caps) = VAR_DEF_KOTLIN.captures(line) {
                    let prefix = &line[..caps.get(0).unwrap().start()];
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    result.push(format!(
                        "{}val {} = \"{}\"",
                        prefix, name, formatted_version
                    ));
                    continue;
                }

                // ext ブロック変数 = 'value' (シングルクォート)
                if let Some(caps) = EXT_VAR_SINGLE.captures(line) {
                    let prefix = &line[..caps.get(0).unwrap().start()];
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    result.push(format!(
                        "{}{} = {}{}{}",
                        prefix, name, quote, formatted_version, quote
                    ));
                    continue;
                }

                // ext ブロック変数 = "value" (ダブルクォート)
                if let Some(caps) = EXT_VAR_DOUBLE.captures(line) {
                    let prefix = &line[..caps.get(0).unwrap().start()];
                    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    result.push(format!(
                        "{}{} = {}{}{}",
                        prefix, name, quote, formatted_version, quote
                    ));
                    continue;
                }
            }

            result.push(line.to_string());
        }

        Ok(result.join("\n"))
    }

    /// 依存行の直接バージョンを更新
    fn update_direct_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parts: Vec<&str> = package.split(':').collect();
        if parts.len() != 2 {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: package.to_string(),
                message: "invalid package format, expected 'group:artifact'".to_string(),
            });
        }
        let (group, artifact) = (parts[0], parts[1]);
        let escaped_group = regex::escape(group);
        let escaped_artifact = regex::escape(artifact);
        let version_parser = get_parser(Language::Java);
        let format_updated = |current_version: &str| -> String {
            version_parser
                .parse(current_version)
                .map(|spec| spec.format_updated(new_version))
                .unwrap_or_else(|| new_version.to_string())
        };

        // map 記法を更新: group: 'x', name: 'y', version: 'z'
        // 非後方参照パターンでシングル/ダブルクォート両対応
        let map_pattern = format!(
            r#"(group:\s*['"]{}['"]\s*,\s*name:\s*['"]{}['"]\s*,\s*version:\s*)(['"])([^'"]+)['"]"#,
            escaped_group, escaped_artifact
        );
        let map_re = Regex::new(&map_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let mut updated = false;
        let result = map_re.replace(content, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let quote = &caps[2];
            let current_version = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let updated_version = format_updated(current_version);
            updated = true;
            format!("{}{}{}{}", prefix, quote, updated_version, quote)
        });

        if updated {
            return Ok(result.to_string());
        }

        // 文字列記法を更新: 'group:artifact:version'
        // 非後方参照パターンでシングル/ダブルクォート両対応
        let string_pattern = format!(
            r#"(['"]){}:{}:([^'"]+)['"]"#,
            escaped_group, escaped_artifact
        );
        let string_re =
            Regex::new(&string_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let result = string_re.replace(content, |caps: &regex::Captures| {
            let quote = &caps[1];
            let current_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let updated_version = format_updated(current_version);
            updated = true;
            format!(
                "{}{}:{}:{}{}",
                quote, group, artifact, updated_version, quote
            )
        });

        if updated {
            return Ok(result.to_string());
        }

        Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: "dependency not found or version could not be updated".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        GradleParser.parse(content)
    }

    // 基本的な依存関係パースのテスト

    #[test]
    fn test_parse_string_notation() {
        let content = r#"
dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
        assert_eq!(deps[0].version_spec.version, "9.12.0");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_string_notation_double_quotes() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:5.3.23"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.springframework:spring-core");
        assert_eq!(deps[0].version_spec.version, "5.3.23");
    }

    #[test]
    fn test_parse_string_notation_maven_alt_range() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:]5.2.0,5.3.8["
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.springframework:spring-core");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.version, "5.2.0");
    }

    #[test]
    fn test_parse_map_notation() {
        let content = r#"
dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: '9.12.0'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    #[test]
    fn test_parse_map_notation_with_parens() {
        let content = r#"
dependencies {
    implementation(group: 'org.apache.wicket', name: 'wicket-core', version: '9.12.0')
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
    }

    #[test]
    fn test_parse_test_implementation() {
        let content = r#"
dependencies {
    testImplementation 'junit:junit:4.13.2'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_multiple_dependencies() {
        let content = r#"
dependencies {
    implementation 'org.springframework:spring-core:5.3.23'
    implementation 'org.springframework:spring-web:5.3.23'
    testImplementation 'junit:junit:4.13.2'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let prod_deps: Vec<_> = deps.iter().filter(|d| !d.is_dev).collect();
        let dev_deps: Vec<_> = deps.iter().filter(|d| d.is_dev).collect();

        assert_eq!(prod_deps.len(), 2);
        assert_eq!(dev_deps.len(), 1);
    }

    // 変数定義のテスト

    #[test]
    fn test_parse_groovy_variable() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    #[test]
    fn test_parse_kotlin_variable() {
        let content = r#"
val wicketVersion = "9.12.0"

dependencies {
    implementation(group = "org.apache.wicket", name = "wicket-core", version = wicketVersion)
}
"#;
        // Kotlin DSL は構文が異なるが、変数抽出は機能する必要がある
        let parser = GradleParser;
        let vars = parser.extract_variables(content);
        assert_eq!(
            vars.get("wicketVersion").map(|v| v.value.as_str()),
            Some("9.12.0")
        );
    }

    #[test]
    fn test_parse_ext_block_variable() {
        let content = r#"
ext {
    springVersion = '5.3.23'
}

dependencies {
    implementation group: 'org.springframework', name: 'spring-core', version: springVersion
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "5.3.23");
    }

    #[test]
    fn test_parse_string_interpolation_variable() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation "org.apache.wicket:wicket-core:$wicketVersion"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    #[test]
    fn test_parse_string_interpolation_braces() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation "org.apache.wicket:wicket-core:${wicketVersion}"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    // バージョン更新のテスト

    #[test]
    fn test_update_version_string_notation() {
        let content = r#"
dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("'org.apache.wicket:wicket-core:10.0.0'"));
    }

    #[test]
    fn test_update_version_map_notation() {
        let content = r#"
dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: '9.12.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("version: '10.0.0'"));
    }

    #[test]
    fn test_update_version_variable() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0'"));
        // 元の変数参照は保持される
        assert!(result.contains("version: wicketVersion"));
    }

    #[test]
    fn test_update_version_ext_variable() {
        let content = r#"
ext {
    springVersion = '5.3.23'
}

dependencies {
    implementation group: 'org.springframework', name: 'spring-core', version: springVersion
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("springVersion = '6.0.0'"));
    }

    #[test]
    fn test_update_version_preserves_quote_style() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:5.3.23"
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("\"org.springframework:spring-core:6.0.0\""));
    }

    #[test]
    fn test_update_version_preserves_strict_notation() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:5.3.23!!"
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("\"org.springframework:spring-core:6.0.0!!\""));
    }

    #[test]
    fn test_update_variable_preserves_strict_notation() {
        let content = r#"
def springVersion = '5.3.23!!'

dependencies {
    implementation "org.springframework:spring-core:$springVersion"
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("def springVersion = '6.0.0!!'"));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"
dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let result = GradleParser.update_version(content, "nonexistent:package", "1.0.0");
        assert!(result.is_err());
    }

    // エッジケースのテスト

    #[test]
    fn test_parse_empty() {
        let deps = parse("").unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_comments_only() {
        let content = r#"
// これはコメントです
// implementation 'commented:out:1.0.0'
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_version_with_suffix() {
        let content = r#"
dependencies {
    implementation 'org.springframework:spring-core:5.3.23.RELEASE'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "5.3.23.RELEASE");
    }

    #[test]
    fn test_parse_snapshot_version() {
        let content = r#"
dependencies {
    implementation 'com.example:my-lib:1.0.0-SNAPSHOT'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "1.0.0-SNAPSHOT");
    }

    #[test]
    fn test_gradle_parser_language() {
        let parser = GradleParser;
        assert_eq!(parser.language(), Language::Java);
    }

    // 実運用に近い例のテスト
    #[test]
    fn test_parse_realistic_build_gradle() {
        let content = r#"
plugins {
    id 'java'
    id 'org.springframework.boot' version '3.0.0'
}

def lombokVersion = '1.18.24'
def junitVersion = '5.9.0'

ext {
    springVersion = '6.0.0'
}

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter-web:3.0.0'
    implementation group: 'org.projectlombok', name: 'lombok', version: lombokVersion
    implementation "org.springframework:spring-core:$springVersion"

    testImplementation 'org.junit.jupiter:junit-jupiter-api:5.9.0'
    testImplementation "org.junit.jupiter:junit-jupiter-engine:${junitVersion}"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        // 主要な依存関係を確認
        let spring_boot = deps
            .iter()
            .find(|d| d.name.contains("spring-boot-starter-web"));
        assert!(spring_boot.is_some());
        assert_eq!(spring_boot.unwrap().version_spec.version, "3.0.0");

        let lombok = deps.iter().find(|d| d.name.contains("lombok"));
        assert!(lombok.is_some());
        assert_eq!(lombok.unwrap().version_spec.version, "1.18.24");

        let spring_core = deps.iter().find(|d| d.name.contains("spring-core"));
        assert!(spring_core.is_some());
        assert_eq!(spring_core.unwrap().version_spec.version, "6.0.0");

        let test_deps: Vec<_> = deps.iter().filter(|d| d.is_dev).collect();
        assert_eq!(test_deps.len(), 2);
    }
}
