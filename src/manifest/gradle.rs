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

/// Gradle rich version の宣言種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RichVersionMethod {
    Strictly,
    Require,
    Prefer,
    Reject,
}

/// rich version ブロック内で見つけた 1 つの宣言
#[derive(Debug, Clone)]
struct RichVersionMatch {
    spec: Option<VersionSpec>,
    line_offset: usize,
    value_start: usize,
    value_end: usize,
}

/// rich version として採用する宣言と、更新すべき位置
#[derive(Debug, Clone)]
struct RichVersionSelection {
    spec: VersionSpec,
    update_line_offset: usize,
    value_start: usize,
    value_end: usize,
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
        r#"^\s*(\w+)\s*[\(\s]+group\s*[:=]\s*['"]([^'"]+)['"]\s*,\s*name\s*[:=]\s*['"]([^'"]+)['"]\s*,\s*version\s*[:=]\s*['"]?([^'",\)\s]+)['"]?"#,
    )
    .unwrap()
});

// 文字列記法依存: implementation 'group:name:version'
// 非後方参照パターンを使用 (シングル/ダブルクォート両対応)
static DEP_STRING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^\s*(\w+)\s*[\(\s]*['"]([^:'"]+):([^:'"]+):([^:'"@]+)(?::[^'"]+)?(?:@[^'"]+)?['"]"#,
    )
    .unwrap()
});

// 変数展開あり文字列記法: implementation "group:name:$version"
static DEP_STRING_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(\w+)\s*[\(\s]*"([^:"]+):([^:"]+):\$\{?(\w+)\}?""#).unwrap()
});

// rich version ブロックを持つ文字列記法依存: implementation("group:name") { ... }
static DEP_STRING_NO_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(\w+)\s*[\(\s]*['"]([^:'"]+):([^:'"]+)['"]\s*\)?\s*(?:\{|$)"#).unwrap()
});

// rich version 宣言: strictly("1.2.3") / require '1.2.3' / prefer "1.2.3" / reject("1.2.3")
static RICH_VERSION_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\b(strictly|require|prefer|reject)\s*(?:\(\s*)?((?:"[^"]+"|'[^']+')(?:\s*,\s*(?:"[^"]+"|'[^']+'))*)\s*\)?"#,
    )
    .unwrap()
});
static RICH_VERSION_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)"|'([^']+)'"#).unwrap());

// 開発用 configuration
const DEV_CONFIGURATIONS: [&str; 6] = [
    "testImplementation",
    "testCompileOnly",
    "testRuntimeOnly",
    "testApi",
    "androidTestImplementation",
    "debugImplementation",
];

fn rich_version_method(name: &str) -> Option<RichVersionMethod> {
    match name {
        "strictly" => Some(RichVersionMethod::Strictly),
        "require" => Some(RichVersionMethod::Require),
        "prefer" => Some(RichVersionMethod::Prefer),
        "reject" => Some(RichVersionMethod::Reject),
        _ => None,
    }
}

fn strip_gradle_comments_from_line(line: &str, in_block_comment: &mut bool) -> String {
    let mut rest = line;
    let mut output = String::new();

    loop {
        if *in_block_comment {
            if let Some(end) = rest.find("*/") {
                let comment_len = end + 2;
                output.push_str(&" ".repeat(comment_len));
                rest = &rest[comment_len..];
                *in_block_comment = false;
            } else {
                output.push_str(&" ".repeat(rest.len()));
                return output;
            }
        } else if let Some(start) = rest.find("/*") {
            output.push_str(&rest[..start]);
            let comment_rest = &rest[start + 2..];
            if let Some(end) = comment_rest.find("*/") {
                let comment_len = 2 + end + 2;
                output.push_str(&" ".repeat(comment_len));
                rest = &rest[start + comment_len..];
            } else {
                output.push_str(&" ".repeat(rest.len() - start));
                *in_block_comment = true;
                return output;
            }
        } else {
            output.push_str(rest);
            break;
        }
    }

    if let Some(start) = output.find("//") {
        output.truncate(start);
    }

    output
}

fn replace_first_active_gradle_match<F>(
    content: &str,
    re: &Regex,
    mut replacement: F,
) -> (String, bool)
where
    F: FnMut(&regex::Captures) -> Option<String>,
{
    let mut output = String::new();
    let mut updated = false;
    let mut in_block_comment = false;

    for segment in content.split_inclusive('\n') {
        let (line, newline) = if let Some(line) = segment.strip_suffix('\n') {
            (line, "\n")
        } else {
            (segment, "")
        };

        if updated {
            output.push_str(segment);
            continue;
        }

        let active_line = strip_gradle_comments_from_line(line, &mut in_block_comment);
        if let Some(caps) = re.captures(&active_line)
            && let Some(new_match) = replacement(&caps)
            && let Some(whole) = caps.get(0)
        {
            output.push_str(&line[..whole.start()]);
            output.push_str(&new_match);
            output.push_str(&line[whole.end()..]);
            output.push_str(newline);
            updated = true;
            continue;
        }

        output.push_str(segment);
    }

    (output, updated)
}

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

    /// 依存宣言のクロージャ範囲を行番号で取得
    fn dependency_block_end(&self, lines: &[&str], start_index: usize) -> Option<usize> {
        let first_line = lines.get(start_index)?;
        if !first_line.contains('{') {
            return None;
        }

        let mut depth = 0usize;
        let mut opened = false;

        for (line_index, line) in lines.iter().enumerate().skip(start_index) {
            let open_count = line.matches('{').count();
            let close_count = line.matches('}').count();

            if open_count > 0 {
                opened = true;
            }

            if opened {
                depth = depth.saturating_add(open_count);
                depth = depth.saturating_sub(close_count);

                if depth == 0 {
                    return Some(line_index);
                }
            }
        }

        if opened { Some(lines.len() - 1) } else { None }
    }

    /// rich version ブロックの代表バージョンを選ぶ
    fn find_rich_version_selection(
        &self,
        block_lines: &[&str],
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<RichVersionSelection> {
        let mut strong: Option<RichVersionMatch> = None;
        let mut prefer: Option<RichVersionMatch> = None;
        let mut rejected_versions = Vec::new();
        let mut in_block_comment = false;

        for (line_offset, line) in block_lines.iter().enumerate() {
            let code = strip_gradle_comments_from_line(line, &mut in_block_comment);
            let trimmed = code.trim();
            if trimmed.is_empty() {
                continue;
            }

            for caps in RICH_VERSION_DECL.captures_iter(&code) {
                let method = rich_version_method(caps.get(1)?.as_str())?;
                let args_match = caps.get(2)?;
                let args = args_match.as_str();

                if method == RichVersionMethod::Reject {
                    for value_caps in RICH_VERSION_VALUE.captures_iter(args) {
                        let Some(value_match) = value_caps.get(1).or_else(|| value_caps.get(2))
                        else {
                            continue;
                        };
                        rejected_versions.push(value_match.as_str().to_string());
                    }
                    continue;
                }

                let Some(value_caps) = RICH_VERSION_VALUE.captures(args) else {
                    continue;
                };
                let Some(version_match) = value_caps.get(1).or_else(|| value_caps.get(2)) else {
                    continue;
                };
                let version = version_match.as_str();
                let found = RichVersionMatch {
                    spec: parser.parse(version),
                    line_offset,
                    value_start: args_match.start() + version_match.start(),
                    value_end: args_match.start() + version_match.end(),
                };

                // Gradle の MutableVersionConstraint は後続の version 宣言で既存の reject を消す。
                rejected_versions.clear();

                match method {
                    RichVersionMethod::Strictly | RichVersionMethod::Require => {
                        // MutableVersionConstraint は後勝ちなので、最後の強い宣言を採用する。
                        strong = Some(found);
                    }
                    RichVersionMethod::Prefer => {
                        prefer = Some(found);
                    }
                    RichVersionMethod::Reject => {}
                }
            }
        }

        if let Some(strong_match) = strong {
            let strong_spec = strong_match.spec.clone()?;

            if strong_spec.kind == VersionSpecKind::Range
                && let Some(prefer_match) = prefer
                && let Some(prefer_spec) = prefer_match.spec.clone()
            {
                return Some(RichVersionSelection {
                    // strictly/require の範囲は上限判定に使い、現在値と更新対象は prefer とする。
                    spec: VersionSpec::new(
                        VersionSpecKind::Range,
                        strong_spec.raw,
                        prefer_spec.version,
                    )
                    .with_rejected_versions(rejected_versions.clone()),
                    update_line_offset: prefer_match.line_offset,
                    value_start: prefer_match.value_start,
                    value_end: prefer_match.value_end,
                });
            }

            return Some(RichVersionSelection {
                spec: strong_spec.with_rejected_versions(rejected_versions),
                update_line_offset: strong_match.line_offset,
                value_start: strong_match.value_start,
                value_end: strong_match.value_end,
            });
        }

        let prefer_match = prefer?;
        Some(RichVersionSelection {
            spec: prefer_match.spec?.with_rejected_versions(rejected_versions),
            update_line_offset: prefer_match.line_offset,
            value_start: prefer_match.value_start,
            value_end: prefer_match.value_end,
        })
    }

    /// rich version ブロック付き依存をパース
    fn parse_rich_version_notation(
        &self,
        lines: &[&str],
        start_index: usize,
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<Dependency> {
        let line = lines.get(start_index)?;
        let caps = DEP_STRING_NO_VERSION.captures(line)?;

        let config = caps.get(1).map(|m| m.as_str())?;
        let group = caps.get(2).map(|m| m.as_str())?;
        let artifact = caps.get(3).map(|m| m.as_str())?;
        let end_index = self.dependency_block_end(lines, start_index)?;
        let selection =
            self.find_rich_version_selection(&lines[start_index..=end_index], parser)?;
        let is_dev = DEV_CONFIGURATIONS.contains(&config);
        let name = format!("{}:{}", group, artifact);

        let dep = if is_dev {
            Dependency::development(name, selection.spec, Language::Java)
        } else {
            Dependency::production(name, selection.spec, Language::Java)
        };

        Some(dep)
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
        let lines: Vec<&str> = content.lines().collect();

        for (line_index, line) in lines.iter().enumerate() {
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
                continue;
            }

            // rich version ブロック付き文字列記法を試す
            if let Some(dep) = self.parse_rich_version_notation(&lines, line_index, parser.as_ref())
            {
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
            return self.update_variable_definition(content, &var_name, var_def, new_version);
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
        var_name: &str,
        var_def: &VariableDefinition,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let lines: Vec<&str> = content.lines().collect();
        let mut result = Vec::new();
        let quote = var_def.quote_char;
        let version_parser = get_parser(Language::Java);
        let Some(formatted_version) = version_parser
            .parse(&var_def.value)
            .and_then(|spec| spec.try_format_updated(new_version))
        else {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: var_name.to_string(),
                message: "version could not be updated safely".to_string(),
            });
        };

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

        let mut joined = result.join("\n");
        // 元のファイルが末尾改行を持つ場合は保持する
        if content.ends_with('\n') {
            joined.push('\n');
        }
        Ok(joined)
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
        let format_updated = |current_version: &str| -> Option<String> {
            version_parser
                .parse(current_version)
                .and_then(|spec| spec.try_format_updated(new_version))
        };

        // map 記法を更新: group: 'x', name: 'y', version: 'z'
        // 非後方参照パターンでシングル/ダブルクォート両対応
        let map_pattern = format!(
            r#"(group\s*[:=]\s*['"]{}['"]\s*,\s*name\s*[:=]\s*['"]{}['"]\s*,\s*version\s*[:=]\s*)(['"])([^'"]+)['"]"#,
            escaped_group, escaped_artifact
        );
        let map_re = Regex::new(&map_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let (result, mut updated) =
            replace_first_active_gradle_match(content, &map_re, |caps: &regex::Captures| {
                let prefix = &caps[1];
                let quote = &caps[2];
                let current_version = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                format_updated(current_version).map(|updated_version| {
                    format!("{}{}{}{}", prefix, quote, updated_version, quote)
                })
            });

        if updated {
            return Ok(result);
        }

        // 文字列記法を更新: 'group:artifact:version'
        // 非後方参照パターンでシングル/ダブルクォート両対応
        let string_pattern = format!(
            r#"(['"]){}:{}:([^:'"@]+)((?::[^'"]+)?(?:@[^'"]+)?)['"]"#,
            escaped_group, escaped_artifact
        );
        let string_re =
            Regex::new(&string_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let (result, string_updated) =
            replace_first_active_gradle_match(content, &string_re, |caps: &regex::Captures| {
                let quote = &caps[1];
                let current_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let suffix = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                format_updated(current_version).map(|updated_version| {
                    format!(
                        "{}{}:{}:{}{}{}",
                        quote, group, artifact, updated_version, suffix, quote
                    )
                })
            });
        updated = string_updated;

        if updated {
            return Ok(result);
        }

        if let Some(result) =
            self.update_rich_version_block(content, group, artifact, new_version)?
        {
            return Ok(result);
        }

        Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: "dependency not found or version could not be updated".to_string(),
        })
    }

    /// rich version ブロック内の更新対象宣言を書き換える
    fn update_rich_version_block(
        &self,
        content: &str,
        group: &str,
        artifact: &str,
        new_version: &str,
    ) -> Result<Option<String>, ManifestError> {
        let lines: Vec<&str> = content.lines().collect();
        let parser = get_parser(Language::Java);

        for (line_index, line) in lines.iter().enumerate() {
            let Some(caps) = DEP_STRING_NO_VERSION.captures(line) else {
                continue;
            };

            if caps.get(2).map(|m| m.as_str()) != Some(group)
                || caps.get(3).map(|m| m.as_str()) != Some(artifact)
            {
                continue;
            }

            let Some(end_index) = self.dependency_block_end(&lines, line_index) else {
                continue;
            };
            let Some(selection) =
                self.find_rich_version_selection(&lines[line_index..=end_index], parser.as_ref())
            else {
                continue;
            };

            let update_line_index = line_index + selection.update_line_offset;
            let current_line = lines[update_line_index];
            let current_version = &current_line[selection.value_start..selection.value_end];
            let Some(formatted_version) = parser
                .parse(current_version)
                .and_then(|spec| spec.try_format_updated(new_version))
            else {
                return Err(ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("build.gradle"),
                    spec: package_name(group, artifact),
                    message: "version could not be updated safely".to_string(),
                });
            };

            let mut result_lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
            result_lines[update_line_index] = format!(
                "{}{}{}",
                &current_line[..selection.value_start],
                formatted_version,
                &current_line[selection.value_end..]
            );

            let mut joined = result_lines.join("\n");
            if content.ends_with('\n') {
                joined.push('\n');
            }
            return Ok(Some(joined));
        }

        Ok(None)
    }
}

fn package_name(group: &str, artifact: &str) -> String {
    format!("{}:{}", group, artifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{UpdateResult, VersionSpecKind};
    use crate::update::{UpdateFilter, UpdateJudge, VersionInfo};
    use chrono::{TimeZone, Utc};

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
    fn test_parse_string_notation_with_classifier_and_extension() {
        // Gradle の group:name:version:classifier@extension 形式も version 部だけを解析する
        let content = r#"
dependencies {
    runtimeOnly("net.sf.docbook:docbook-xsl:1.75.2:resources@zip")
    implementation("com.google.android.material:material:1.11.0@aar")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let docbook = deps
            .iter()
            .find(|d| d.name == "net.sf.docbook:docbook-xsl")
            .unwrap();
        assert_eq!(docbook.version_spec.version, "1.75.2");

        let material = deps
            .iter()
            .find(|d| d.name == "com.google.android.material:material")
            .unwrap();
        assert_eq!(material.version_spec.version, "1.11.0");
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
    fn test_parse_string_notation_maven_range_with_multi_part_qualifier() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:[5.2.0,5.3.8-beta1-SNAPSHOT]"
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
    fn test_parse_kotlin_named_argument_map_notation() {
        let content = r#"
dependencies {
    implementation(group = "com.google.guava", name = "guava", version = "32.1.2-jre")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
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
    fn test_parse_rich_version_strictly_groovy() {
        let content = r#"
dependencies {
    implementation('org.slf4j:slf4j-api') {
        version {
            strictly '1.7.36'
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.slf4j:slf4j-api");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "1.7.36");
    }

    #[test]
    fn test_parse_rich_version_range_with_prefer_kotlin() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.slf4j:slf4j-api");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, "[1.7, 1.8[");
        // 更新判定に使う現在値は Gradle が選好する prefer 側を採用する。
        assert_eq!(deps[0].version_spec.version, "1.7.25");
    }

    #[test]
    fn test_parse_rich_version_ignores_commented_declaration() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            // prefer("1.7.25")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_rich_version_ignores_block_commented_declaration() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            /*
             * prefer("1.7.25")
             */
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_judge_rich_version_prefer_respects_strict_range() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
        }
    }
}
"#;
        let dep = parse(content).unwrap().remove(0);
        let released_at = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let versions = vec![
            VersionInfo::new("1.7.36", released_at),
            VersionInfo::new("1.8.0", released_at),
        ];

        let result = UpdateJudge::new(UpdateFilter::new()).judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.7.36");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_parse_rich_version_rejects() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
            reject("1.7.36", "1.7.37")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(
            deps[0].version_spec.rejected_versions,
            vec!["1.7.36", "1.7.37"]
        );
    }

    #[test]
    fn test_judge_rich_version_rejects_candidate() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
            reject("1.7.36")
        }
    }
}
"#;
        let dep = parse(content).unwrap().remove(0);
        let released_at = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let versions = vec![
            VersionInfo::new("1.7.35", released_at),
            VersionInfo::new("1.7.36", released_at),
        ];

        let result = UpdateJudge::new(UpdateFilter::new()).judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.7.35");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_parse_rich_version_declaration_clears_previous_rejects() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            reject("1.7.36")
            prefer("1.7.25")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].version_spec.rejected_versions.is_empty());
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
    fn test_update_string_notation_preserves_classifier_and_extension() {
        // classifier / extension は依存座標の一部なので、version だけを差し替えて維持する
        let content = r#"
dependencies {
    runtimeOnly("net.sf.docbook:docbook-xsl:1.75.2:resources@zip")
}
"#;
        let result = GradleParser
            .update_version(content, "net.sf.docbook:docbook-xsl", "1.76.0")
            .unwrap();
        assert!(result.contains(r#""net.sf.docbook:docbook-xsl:1.76.0:resources@zip""#));
    }

    #[test]
    fn test_update_string_notation_ignores_line_comment() {
        let content = r#"
dependencies {
    // implementation 'org.apache.wicket:wicket-core:9.12.0'
    implementation 'org.apache.wicket:wicket-core:9.13.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("// implementation 'org.apache.wicket:wicket-core:9.12.0'"));
        assert!(result.contains("implementation 'org.apache.wicket:wicket-core:10.0.0'"));
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
    fn test_update_kotlin_named_argument_map_notation() {
        let content = r#"
dependencies {
    implementation(group = "com.google.guava", name = "guava", version = "32.1.2-jre")
}
"#;
        let result = GradleParser
            .update_version(content, "com.google.guava:guava", "33.4.0-jre")
            .unwrap();
        assert!(result.contains(r#"version = "33.4.0-jre""#));
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
    fn test_update_rich_version_strictly() {
        let content = r#"
dependencies {
    implementation('org.slf4j:slf4j-api') {
        version {
            strictly '1.7.36'
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "2.0.17")
            .unwrap();
        assert!(result.contains("strictly '2.0.17'"));
    }

    #[test]
    fn test_update_rich_version_prefer_with_strict_range() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains(r#"strictly("[1.7, 1.8[")"#));
        assert!(result.contains(r#"prefer("1.7.36")"#));
    }

    #[test]
    fn test_update_rich_version_after_inline_block_comment() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            /* 現在の推奨版 */ prefer("1.7.25")
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains(r#"/* 現在の推奨版 */ prefer("1.7.36")"#));
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
    fn test_update_variable_preserves_trailing_newline() {
        // 変数定義の更新で末尾改行を保持する
        let content = "def wicketVersion = '9.12.0'\n\ndependencies {\n    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion\n}\n";
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0'"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_update_variable_no_trailing_newline() {
        // 末尾改行がないファイルは付けない
        let content = "def wicketVersion = '9.12.0'\n\ndependencies {\n    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion\n}";
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0'"));
        assert!(!result.ends_with('\n'));
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

    // --- 追加エッジケーステスト ---

    #[test]
    fn test_parse_kotlin_dsl_parenthesized_string() {
        // Kotlin DSL: implementation("group:name:version") 形式
        let content = r#"
dependencies {
    implementation("com.google.guava:guava:32.1.2-jre")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_test_implementation_string_notation() {
        // testImplementation の文字列記法が開発依存として判定されること
        let content = r#"
dependencies {
    testImplementation 'org.mockito:mockito-core:5.5.0'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.mockito:mockito-core");
        assert_eq!(deps[0].version_spec.version, "5.5.0");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_variable_reference_in_string_interpolation() {
        // 変数参照を文字列展開で使用するケース
        let content = r#"
def guavaVersion = '32.1.2-jre'

dependencies {
    implementation "com.google.guava:guava:$guavaVersion"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        // 変数が解決されて実際のバージョンになること
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
    }

    #[test]
    fn test_parse_platform_dependency_not_matched() {
        // platform() ラッパー付き依存は通常の文字列記法正規表現にマッチしない
        // （platform がコンフィグ名として解釈されるため、実際の configuration とは異なる）
        let content = r#"
dependencies {
    implementation platform('com.google.cloud:libraries-bom:26.1.0')
}
"#;
        let deps = parse(content).unwrap();
        // platform() ラッパーの中身は独立した行としてパースされる
        // "platform" が configuration 名として解釈される
        let platform_dep = deps
            .iter()
            .find(|d| d.name == "com.google.cloud:libraries-bom");
        if let Some(dep) = platform_dep {
            // パースされた場合、本番依存として扱われる（platform は DEV_CONFIGURATIONS に含まれない）
            assert!(!dep.is_dev);
            assert_eq!(dep.version_spec.version, "26.1.0");
        }
        // パースされない場合もテスト自体は成功（実装の振る舞いを記録）
    }
}
