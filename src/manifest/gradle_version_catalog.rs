//! Gradle version catalog (`*.versions.toml`) のパースと更新

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::line_utils::{parse_toml_section_header, split_line_ending};
use crate::parser::get_parser;
use regex::Regex;
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct VersionCatalogVersion {
    spec: VersionSpec,
    update_value: String,
    update_member: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct VersionCatalogEntry {
    alias: String,
    name: String,
    version: VersionCatalogVersion,
    version_ref: Option<String>,
}

fn package_name(group: &str, artifact: &str) -> String {
    format!("{}:{}", group, artifact)
}

fn split_catalog_module(module: &str) -> Option<(&str, &str)> {
    let (group, artifact) = module.split_once(':')?;
    if group.is_empty() || artifact.is_empty() || artifact.contains(':') {
        return None;
    }
    Some((group, artifact))
}

fn split_catalog_coordinate(coordinate: &str) -> Option<(&str, &str, &str)> {
    let mut parts = coordinate.splitn(3, ':');
    let group = parts.next()?;
    let artifact = parts.next()?;
    let version = parts.next()?;
    if group.is_empty() || artifact.is_empty() || version.is_empty() {
        return None;
    }
    Some((group, artifact, version))
}

fn catalog_string(value: &toml::Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn catalog_rejected_versions(value: Option<&toml::Value>) -> Vec<String> {
    match value {
        Some(toml::Value::String(version)) => vec![version.clone()],
        Some(toml::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn replace_toml_string_member(line: &str, key: &str, new_value: &str) -> Option<String> {
    let pattern = format!(r#"(\b{}\s*=\s*)(['"])([^'"]*)(['"])"#, regex::escape(key));
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(line)?;
    let whole = caps.get(0)?;
    let prefix = caps.get(1)?.as_str();
    let quote = caps.get(2)?.as_str();
    let suffix = caps.get(4)?.as_str();
    let replacement = format!("{prefix}{quote}{new_value}{suffix}");

    Some(format!(
        "{}{}{}",
        &line[..whole.start()],
        replacement,
        &line[whole.end()..]
    ))
}

fn replace_toml_assignment_string(line: &str, key: &str, new_value: &str) -> Option<String> {
    let pattern = format!(r#"(^\s*{}\s*=\s*)(['"])([^'"]*)(['"])"#, regex::escape(key));
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(line)?;
    let whole = caps.get(0)?;
    let prefix = caps.get(1)?.as_str();
    let quote = caps.get(2)?.as_str();
    let suffix = caps.get(4)?.as_str();
    let replacement = format!("{prefix}{quote}{new_value}{suffix}");

    Some(format!(
        "{}{}{}",
        &line[..whole.start()],
        replacement,
        &line[whole.end()..]
    ))
}

fn replace_catalog_coordinate_version(
    line: &str,
    group: &str,
    artifact: &str,
    new_version: &str,
) -> Option<String> {
    let pattern = format!(
        r#"(['"]){}:{}:([^'"]+)(['"])"#,
        regex::escape(group),
        regex::escape(artifact)
    );
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(line)?;
    let whole = caps.get(0)?;
    let quote = caps.get(1)?.as_str();
    let suffix = caps.get(3)?.as_str();
    let replacement = format!("{quote}{group}:{artifact}:{new_version}{suffix}");

    Some(format!(
        "{}{}{}",
        &line[..whole.start()],
        replacement,
        &line[whole.end()..]
    ))
}

fn line_starts_toml_assignment(line: &str, key: &str) -> bool {
    let pattern = format!(r#"^\s*{}\s*="#, regex::escape(key));
    Regex::new(&pattern)
        .map(|re| re.is_match(line))
        .unwrap_or(false)
}

/// セクションヘッダ行 (`[versions]` / `[libraries]` 等) からセクション名を取り出す。
/// 字句解析は line_utils の共有実装に委譲する。
fn toml_section_name(line: &str) -> Option<String> {
    parse_toml_section_header(line).map(str::to_string)
}

fn parse_version_catalog_version_table(
    table: &toml::Table,
    parser: &dyn crate::parser::VersionParser,
) -> Option<VersionCatalogVersion> {
    if table
        .get("rejectAll")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    let rejected_versions = catalog_rejected_versions(table.get("reject"));
    let prefer = table
        .get("prefer")
        .and_then(catalog_string)
        .and_then(|version| parser.parse(version).map(|spec| (version, spec)));
    let strong = table
        .get("strictly")
        .and_then(catalog_string)
        .and_then(|version| {
            parser
                .parse(version)
                .map(|spec| ("strictly", version, spec))
        })
        .or_else(|| {
            table
                .get("require")
                .and_then(catalog_string)
                .and_then(|version| parser.parse(version).map(|spec| ("require", version, spec)))
        });

    if let Some((member, update_value, strong_spec)) = strong {
        if strong_spec.kind == VersionSpecKind::Range
            && let Some((prefer_value, prefer_spec)) = prefer
        {
            return Some(VersionCatalogVersion {
                spec: VersionSpec::new(
                    VersionSpecKind::Range,
                    strong_spec.raw,
                    prefer_spec.version,
                )
                .with_rejected_versions(rejected_versions),
                update_value: prefer_value.to_string(),
                update_member: Some("prefer"),
            });
        }

        return Some(VersionCatalogVersion {
            spec: strong_spec.with_rejected_versions(rejected_versions),
            update_value: update_value.to_string(),
            update_member: Some(member),
        });
    }

    let (prefer_value, prefer_spec) = prefer?;
    Some(VersionCatalogVersion {
        spec: prefer_spec.with_rejected_versions(rejected_versions),
        update_value: prefer_value.to_string(),
        update_member: Some("prefer"),
    })
}

fn parse_version_catalog_version_value(
    value: &toml::Value,
    parser: &dyn crate::parser::VersionParser,
) -> Option<VersionCatalogVersion> {
    match value {
        toml::Value::String(version) => {
            let spec = parser.parse(version)?;
            Some(VersionCatalogVersion {
                spec,
                update_value: version.clone(),
                update_member: None,
            })
        }
        toml::Value::Table(table) => parse_version_catalog_version_table(table, parser),
        _ => None,
    }
}

fn version_catalog_ref(table: &toml::Table) -> Option<&str> {
    table
        .get("version.ref")
        .and_then(catalog_string)
        .or_else(|| {
            table
                .get("version")
                .and_then(toml::Value::as_table)
                .and_then(|version| version.get("ref"))
                .and_then(catalog_string)
        })
}

fn parse_version_catalog_library(
    alias: &str,
    value: &toml::Value,
    versions: Option<&toml::Table>,
    parser: &dyn crate::parser::VersionParser,
) -> Option<VersionCatalogEntry> {
    match value {
        toml::Value::String(coordinate) => {
            let (group, artifact, version) = split_catalog_coordinate(coordinate)?;
            let spec = parser.parse(version)?;
            Some(VersionCatalogEntry {
                alias: alias.to_string(),
                name: package_name(group, artifact),
                version: VersionCatalogVersion {
                    spec,
                    update_value: version.to_string(),
                    update_member: None,
                },
                version_ref: None,
            })
        }
        toml::Value::Table(table) => {
            let (group, artifact) =
                if let Some(module) = table.get("module").and_then(catalog_string) {
                    split_catalog_module(module)?
                } else {
                    let group = table.get("group").and_then(catalog_string)?;
                    let artifact = table.get("name").and_then(catalog_string)?;
                    (group, artifact)
                };

            if let Some(version_ref) = version_catalog_ref(table) {
                let version = versions?
                    .get(version_ref)
                    .and_then(|value| parse_version_catalog_version_value(value, parser))?;
                return Some(VersionCatalogEntry {
                    alias: alias.to_string(),
                    name: package_name(group, artifact),
                    version,
                    version_ref: Some(version_ref.to_string()),
                });
            }

            let version = table
                .get("version")
                .and_then(|value| parse_version_catalog_version_value(value, parser))?;

            Some(VersionCatalogEntry {
                alias: alias.to_string(),
                name: package_name(group, artifact),
                version,
                version_ref: None,
            })
        }
        _ => None,
    }
}

fn looks_like_version_catalog(content: &str) -> bool {
    content.lines().any(|line| {
        let section = line.split('#').next().unwrap_or("").trim();
        matches!(
            section,
            "[libraries]" | "[versions]" | "[bundles]" | "[plugins]"
        ) || section.starts_with("[libraries.")
            || section.starts_with("[versions.")
            || section.starts_with("[bundles.")
            || section.starts_with("[plugins.")
    })
}

fn version_catalog_error_path() -> PathBuf {
    PathBuf::from("version catalog")
}

fn parse_version_catalog_entries(
    content: &str,
) -> Result<Option<Vec<VersionCatalogEntry>>, ManifestError> {
    if !looks_like_version_catalog(content) {
        return Ok(None);
    }

    let catalog: toml::Value = toml::from_str(content).map_err(|err| {
        ManifestError::toml_parse_error(version_catalog_error_path(), err.to_string())
    })?;
    let Some(libraries) = catalog.get("libraries").and_then(toml::Value::as_table) else {
        return Ok(Some(Vec::new()));
    };
    let versions = catalog.get("versions").and_then(toml::Value::as_table);
    let parser = get_parser(Language::Java);

    Ok(Some(
        libraries
            .iter()
            .filter_map(|(alias, value)| {
                parse_version_catalog_library(alias, value, versions, parser.as_ref())
            })
            .collect(),
    ))
}

fn formatted_catalog_update(
    entry: &VersionCatalogEntry,
    new_version: &str,
) -> Result<String, ManifestError> {
    let version_parser = get_parser(Language::Java);
    version_parser
        .parse(&entry.version.update_value)
        .and_then(|spec| spec.try_format_updated(new_version))
        .ok_or_else(|| ManifestError::InvalidVersionSpec {
            path: version_catalog_error_path(),
            spec: entry.name.clone(),
            message: "version catalog entry could not be updated safely".to_string(),
        })
}

fn replace_version_alias_line(
    line: &str,
    alias: &str,
    update_member: Option<&str>,
    formatted_version: &str,
    in_target_block: &mut bool,
) -> Option<String> {
    if let Some(member) = update_member {
        if line_starts_toml_assignment(line, alias) {
            *in_target_block = line.contains('{') && !line.contains('}');
            return replace_toml_string_member(line, member, formatted_version);
        }

        if *in_target_block {
            let replaced = replace_toml_string_member(line, member, formatted_version);
            if line.contains('}') {
                *in_target_block = false;
            }
            return replaced;
        }

        return None;
    }

    if line_starts_toml_assignment(line, alias) {
        replace_toml_assignment_string(line, alias, formatted_version)
    } else {
        None
    }
}

/// version catalog の行走査による書き換え共通処理。
/// `toml_section_name` によるセクション追跡と split_inclusive('\n') + split_line_ending
/// による CRLF/LF 保持を行いながら、対象セクション (`primary_section`) の行には
/// `replace_primary` (対象エントリの `{ ... }` ブロック継続を追う `in_target_block` 付き) を、
/// alias 専用テーブル (`alias_section`、例: `[versions.foo]`) の行には
/// `replace_in_alias_section` を適用する。最初の更新以降は残りを素通しし、
/// 1 行も更新できなければ None を返す。
fn rewrite_catalog_lines(
    content: &str,
    primary_section: &str,
    alias_section: &str,
    mut replace_primary: impl FnMut(&str, &mut bool) -> Option<String>,
    mut replace_in_alias_section: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    let mut result = String::new();
    let mut section = String::new();
    let mut in_target_block = false;
    let mut updated = false;

    // CRLF/LF を保持するため split_inclusive で改行込みに走査する
    for raw_line in content.split_inclusive('\n') {
        let (line, line_ending) = split_line_ending(raw_line);
        if let Some(new_section) = toml_section_name(line) {
            section = new_section;
            in_target_block = false;
        }

        let trimmed = line.trim_start();
        let mut next_line = line.to_string();

        if !updated && section == primary_section && !trimmed.starts_with('#') {
            if let Some(replaced) = replace_primary(line, &mut in_target_block) {
                next_line = replaced;
                updated = true;
            }
        } else if !updated
            && section == alias_section
            && !trimmed.starts_with('#')
            && let Some(replaced) = replace_in_alias_section(line)
        {
            next_line = replaced;
            updated = true;
        }

        result.push_str(&next_line);
        result.push_str(line_ending);
    }

    if !updated {
        return None;
    }

    Some(result)
}

fn update_version_catalog_version_alias(
    content: &str,
    alias: &str,
    update_member: Option<&str>,
    formatted_version: &str,
) -> Option<String> {
    let alias_section = format!("versions.{}", alias);
    rewrite_catalog_lines(
        content,
        "versions",
        &alias_section,
        |line, in_target_block| {
            replace_version_alias_line(
                line,
                alias,
                update_member,
                formatted_version,
                in_target_block,
            )
        },
        // `[versions.<alias>]` テーブル内は rich version メンバー指定時のみ書き換える
        |line| {
            update_member
                .and_then(|member| replace_toml_string_member(line, member, formatted_version))
        },
    )
}

fn replace_library_version_value(
    line: &str,
    entry: &VersionCatalogEntry,
    formatted_version: &str,
) -> Option<String> {
    if let Some(member) = entry.version.update_member {
        replace_toml_string_member(line, member, formatted_version)
    } else {
        replace_toml_string_member(line, "version", formatted_version)
    }
}

fn replace_library_dotted_line(
    line: &str,
    entry: &VersionCatalogEntry,
    formatted_version: &str,
) -> Option<String> {
    if let Some(member) = entry.version.update_member {
        let member_key = format!("{}.version.{}", entry.alias, member);
        if line_starts_toml_assignment(line, &member_key) {
            return replace_toml_assignment_string(line, &member_key, formatted_version);
        }
        return None;
    }

    let version_key = format!("{}.version", entry.alias);
    if line_starts_toml_assignment(line, &version_key) {
        replace_toml_assignment_string(line, &version_key, formatted_version)
    } else {
        None
    }
}

fn replace_library_assignment_line(
    line: &str,
    entry: &VersionCatalogEntry,
    group: &str,
    artifact: &str,
    formatted_version: &str,
    in_target_block: &mut bool,
) -> Option<String> {
    if line_starts_toml_assignment(line, &entry.alias) {
        *in_target_block = line.contains('{') && !line.contains('}');

        return replace_catalog_coordinate_version(line, group, artifact, formatted_version)
            .or_else(|| replace_library_version_value(line, entry, formatted_version));
    }

    if *in_target_block {
        let replaced = replace_library_version_value(line, entry, formatted_version);
        if line.contains('}') {
            *in_target_block = false;
        }
        return replaced;
    }

    None
}

fn update_version_catalog_library_alias(
    content: &str,
    entry: &VersionCatalogEntry,
    formatted_version: &str,
) -> Option<String> {
    let (group, artifact) = entry.name.split_once(':')?;
    let alias_section = format!("libraries.{}", entry.alias);
    rewrite_catalog_lines(
        content,
        "libraries",
        &alias_section,
        |line, in_target_block| {
            replace_library_dotted_line(line, entry, formatted_version).or_else(|| {
                replace_library_assignment_line(
                    line,
                    entry,
                    group,
                    artifact,
                    formatted_version,
                    in_target_block,
                )
            })
        },
        |line| replace_library_version_value(line, entry, formatted_version),
    )
}

pub(super) fn parse(content: &str) -> Result<Option<Vec<Dependency>>, ManifestError> {
    Ok(parse_version_catalog_entries(content)?.map(|entries| {
        entries
            .into_iter()
            .map(|entry| {
                let dependency =
                    Dependency::production(entry.name, entry.version.spec, Language::Java);
                match entry.version_ref {
                    Some(version_ref) => dependency.with_variable(version_ref),
                    None => dependency,
                }
            })
            .collect()
    }))
}

pub(super) fn update_version(
    content: &str,
    package: &str,
    new_version: &str,
) -> Result<Option<String>, ManifestError> {
    let Some(entries) = parse_version_catalog_entries(content)? else {
        return Ok(None);
    };
    let Some(entry) = entries.iter().find(|entry| entry.name == package) else {
        return Ok(None);
    };
    let formatted_version = formatted_catalog_update(entry, new_version)?;

    if let Some(version_ref) = &entry.version_ref {
        return update_version_catalog_version_alias(
            content,
            version_ref,
            entry.version.update_member,
            &formatted_version,
        )
        .ok_or_else(|| ManifestError::InvalidVersionSpec {
            path: version_catalog_error_path(),
            spec: package.to_string(),
            message: "version catalog reference could not be updated".to_string(),
        })
        .map(Some);
    }

    update_version_catalog_library_alias(content, entry, &formatted_version)
        .ok_or_else(|| ManifestError::InvalidVersionSpec {
            path: version_catalog_error_path(),
            spec: package.to_string(),
            message: "version catalog dependency could not be updated".to_string(),
        })
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のカタログを組み立てる (`looks_like_version_catalog` を通すため
    /// 必ずセクションヘッダを含める)
    fn deps(content: &str) -> Vec<Dependency> {
        parse(content)
            .expect("パースが成功するべき")
            .expect("version catalog として認識されるべき")
    }

    /// カタログをパースし、指定した Maven 座標の依存を 1 件取り出す
    fn find(content: &str, name: &str) -> Dependency {
        deps(content)
            .into_iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} が抽出されるべき"))
    }

    // --- 判別 -------------------------------------------------------------

    #[test]
    fn test_non_catalog_content_returns_none() {
        // build.gradle は version catalog ではないので None (呼び出し側が別パーサへ回す)
        assert!(
            parse("dependencies {\n  implementation 'a:b:1.0'\n}\n")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_catalog_detected_by_subsection_header() {
        // `[libraries.foo]` のようなサブセクションだけでも catalog と判別する
        let content =
            "[libraries.guava]\nmodule = \"com.google.guava:guava\"\nversion = \"33.0.0\"\n";
        assert_eq!(deps(content).len(), 1);
    }

    #[test]
    fn test_invalid_toml_is_reported_as_error() {
        // 壊れた TOML は黙って読み飛ばさずエラーにする
        let result = parse("[libraries]\nguava = \"unterminated\n");
        assert!(result.is_err(), "壊れた TOML はエラーになるべき");
    }

    #[test]
    fn test_catalog_without_libraries_section_is_empty() {
        // `[versions]` だけのカタログは依存 0 件 (None ではない)
        let parsed = parse("[versions]\nguava = \"33.0.0\"\n").unwrap();
        assert_eq!(parsed.expect("catalog として認識されるべき").len(), 0);
    }

    // --- パース ------------------------------------------------------------

    #[test]
    fn test_parse_coordinate_string_form() {
        let dep = find(
            "[libraries]\njunit = \"junit:junit:4.13.2\"\n",
            "junit:junit",
        );
        assert_eq!(dep.version_spec.version, "4.13.2");
        assert_eq!(dep.variable_name, None);
    }

    #[test]
    fn test_parse_module_with_inline_version() {
        let content = "[libraries]\nguava = { module = \"com.google.guava:guava\", version = \"33.0.0-jre\" }\n";
        let dep = find(content, "com.google.guava:guava");
        assert_eq!(dep.version_spec.version, "33.0.0-jre");
    }

    #[test]
    fn test_parse_group_name_version_form() {
        let content = "[libraries]\ncommons = { group = \"org.apache.commons\", name = \"commons-lang3\", version = \"3.14.0\" }\n";
        let dep = find(content, "org.apache.commons:commons-lang3");
        assert_eq!(dep.version_spec.version, "3.14.0");
    }

    #[test]
    fn test_parse_version_ref_records_variable_name() {
        let content = "[versions]\nguava = \"33.0.0\"\n\n[libraries]\nguava = { module = \"com.google.guava:guava\", version.ref = \"guava\" }\n";
        let dep = find(content, "com.google.guava:guava");
        assert_eq!(dep.version_spec.version, "33.0.0");
        // version.ref 名は共有検出のため variable_name に載せる
        assert_eq!(dep.variable_name.as_deref(), Some("guava"));
    }

    #[test]
    fn test_parse_nested_version_ref_table_form() {
        // `version = { ref = "..." }` は `version.ref = "..."` と同義
        let content = "[versions]\nguava = \"33.0.0\"\n\n[libraries]\nguava = { module = \"com.google.guava:guava\", version = { ref = \"guava\" } }\n";
        let dep = find(content, "com.google.guava:guava");
        assert_eq!(dep.version_spec.version, "33.0.0");
        assert_eq!(dep.variable_name.as_deref(), Some("guava"));
    }

    #[test]
    fn test_parse_dangling_version_ref_is_skipped() {
        // 参照先が `[versions]` に無いエントリは更新先を決められないので surface しない
        let content = "[libraries]\nguava = { module = \"com.google.guava:guava\", version.ref = \"missing\" }\n";
        assert!(deps(content).is_empty());
    }

    #[test]
    fn test_parse_library_without_version_is_skipped() {
        // BOM 管理下でバージョン省略された依存は更新対象にできない
        let content = "[libraries]\nguava = { module = \"com.google.guava:guava\" }\n";
        assert!(deps(content).is_empty());
    }

    #[test]
    fn test_parse_plugins_are_excluded() {
        // plugin ID は Maven 座標と一致しないため対象外
        let content = "[libraries]\njunit = \"junit:junit:4.13.2\"\n\n[plugins]\nspotless = { id = \"com.diffplug.spotless\", version = \"6.25.0\" }\n";
        let d = deps(content);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, "junit:junit");
    }

    #[test]
    fn test_parse_rich_version_strictly() {
        let content = "[versions]\nslf4j = { strictly = \"1.7.25\" }\n\n[libraries]\nslf4j = { module = \"org.slf4j:slf4j-api\", version.ref = \"slf4j\" }\n";
        let dep = find(content, "org.slf4j:slf4j-api");
        assert_eq!(dep.version_spec.version, "1.7.25");
    }

    #[test]
    fn test_parse_rich_version_strictly_range_uses_prefer_as_current() {
        // strictly が範囲なら上限制約として保持し、比較基準は prefer の値
        let content = "[versions]\nslf4j = { strictly = \"[1.7, 1.8[\", prefer = \"1.7.25\" }\n\n[libraries]\nslf4j = { module = \"org.slf4j:slf4j-api\", version.ref = \"slf4j\" }\n";
        let dep = find(content, "org.slf4j:slf4j-api");
        assert_eq!(dep.version_spec.kind, VersionSpecKind::Range);
        assert_eq!(dep.version_spec.version, "1.7.25");
    }

    #[test]
    fn test_parse_reject_all_excludes_entry() {
        // rejectAll = true は全バージョン拒否なので更新対象にしない
        let content = "[versions]\nslf4j = { rejectAll = true }\n\n[libraries]\nslf4j = { module = \"org.slf4j:slf4j-api\", version.ref = \"slf4j\" }\n";
        assert!(deps(content).is_empty());
    }

    #[test]
    fn test_parse_reject_list_is_carried_into_spec() {
        let content = "[versions]\nslf4j = { require = \"1.7.25\", reject = [\"1.7.36\", \"1.7.35\"] }\n\n[libraries]\nslf4j = { module = \"org.slf4j:slf4j-api\", version.ref = \"slf4j\" }\n";
        let dep = find(content, "org.slf4j:slf4j-api");
        assert_eq!(
            dep.version_spec.rejected_versions,
            vec!["1.7.36".to_string(), "1.7.35".to_string()]
        );
    }

    // --- 更新 --------------------------------------------------------------

    #[test]
    fn test_update_coordinate_string_form() {
        let content = "[libraries]\njunit = \"junit:junit:4.13.2\"\n";
        let updated = update_version(content, "junit:junit", "4.13.3")
            .unwrap()
            .unwrap();
        assert_eq!(updated, "[libraries]\njunit = \"junit:junit:4.13.3\"\n");
    }

    #[test]
    fn test_update_inline_version_member() {
        let content =
            "[libraries]\nguava = { module = \"com.google.guava:guava\", version = \"33.0.0\" }\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("version = \"34.0.0\""));
        // module 側の座標は書き換えない
        assert!(updated.contains("module = \"com.google.guava:guava\""));
    }

    #[test]
    fn test_update_version_ref_rewrites_versions_section() {
        let content = "[versions]\nguava = \"33.0.0\"\n\n[libraries]\nguava = { module = \"com.google.guava:guava\", version.ref = \"guava\" }\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("guava = \"34.0.0\""), "{updated}");
        // libraries 側の宣言は触らない
        assert!(updated.contains("version.ref = \"guava\""));
    }

    #[test]
    fn test_update_preserves_single_quotes() {
        let content = "[versions]\nguava = '33.0.0'\n\n[libraries]\nguava = { module = \"com.google.guava:guava\", version.ref = \"guava\" }\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("guava = '34.0.0'"), "{updated}");
    }

    #[test]
    fn test_update_preserves_crlf() {
        let content = "[versions]\r\nguava = \"33.0.0\"\r\n\r\n[libraries]\r\nguava = { module = \"com.google.guava:guava\", version.ref = \"guava\" }\r\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("guava = \"34.0.0\"\r\n"), "{updated:?}");
        assert!(
            !updated.contains("\n\n"),
            "LF 化されていないこと: {updated:?}"
        );
    }

    #[test]
    fn test_update_preserves_missing_trailing_newline() {
        let content = "[libraries]\njunit = \"junit:junit:4.13.2\"";
        let updated = update_version(content, "junit:junit", "4.13.3")
            .unwrap()
            .unwrap();
        assert_eq!(updated, "[libraries]\njunit = \"junit:junit:4.13.3\"");
    }

    #[test]
    fn test_update_dotted_key_form() {
        let content = "[versions]\nguava = \"33.0.0\"\n\n[libraries]\nguava.module = \"com.google.guava:guava\"\nguava.version.ref = \"guava\"\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("guava = \"34.0.0\""), "{updated}");
    }

    #[test]
    fn test_update_library_subsection_table_form() {
        let content =
            "[libraries.guava]\nmodule = \"com.google.guava:guava\"\nversion = \"33.0.0\"\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("version = \"34.0.0\""), "{updated}");
    }

    #[test]
    fn test_update_versions_subsection_table_form() {
        let content = "[versions.slf4j]\nrequire = \"1.7.25\"\n\n[libraries]\nslf4j = { module = \"org.slf4j:slf4j-api\", version.ref = \"slf4j\" }\n";
        let updated = update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap()
            .unwrap();
        assert!(updated.contains("require = \"1.7.36\""), "{updated}");
    }

    #[test]
    fn test_update_rich_version_writes_prefer_member() {
        // strictly が範囲のときに書き換えるのは prefer 側
        let content = "[versions]\nslf4j = { strictly = \"[1.7, 1.8[\", prefer = \"1.7.25\" }\n\n[libraries]\nslf4j = { module = \"org.slf4j:slf4j-api\", version.ref = \"slf4j\" }\n";
        let updated = update_version(content, "org.slf4j:slf4j-api", "1.7.30")
            .unwrap()
            .unwrap();
        assert!(updated.contains("prefer = \"1.7.30\""), "{updated}");
        // 上限制約である strictly の範囲は保持する
        assert!(updated.contains("strictly = \"[1.7, 1.8[\""), "{updated}");
    }

    #[test]
    fn test_update_unknown_package_returns_none() {
        let content = "[libraries]\njunit = \"junit:junit:4.13.2\"\n";
        assert!(
            update_version(content, "org.unknown:missing", "1.0.0")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_update_skips_commented_out_lines() {
        // コメント行の同名宣言を書き換えず、実宣言だけを更新する
        let content = "[versions]\n# guava = \"1.0.0\"\nguava = \"33.0.0\"\n\n[libraries]\nguava = { module = \"com.google.guava:guava\", version.ref = \"guava\" }\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("# guava = \"1.0.0\""), "{updated}");
        assert!(updated.contains("\nguava = \"34.0.0\""), "{updated}");
    }

    #[test]
    fn test_update_does_not_touch_other_sections() {
        // 同名 alias が `[plugins]` にもある場合、`[versions]` 側だけを書き換える
        let content = "[versions]\nguava = \"33.0.0\"\n\n[libraries]\nguava = { module = \"com.google.guava:guava\", version.ref = \"guava\" }\n\n[plugins]\nguava = { id = \"com.example.guava\", version = \"33.0.0\" }\n";
        let updated = update_version(content, "com.google.guava:guava", "34.0.0")
            .unwrap()
            .unwrap();
        assert!(updated.contains("id = \"com.example.guava\", version = \"33.0.0\""));
    }

    #[test]
    fn test_update_wildcard_keeps_shape() {
        // `1.+` のような動的指定は形を保って更新する
        let content = "[libraries]\nguava = \"com.google.guava:guava:33.+\"\n";
        let updated = update_version(content, "com.google.guava:guava", "34.1.0")
            .unwrap()
            .unwrap();
        assert_eq!(
            updated,
            "[libraries]\nguava = \"com.google.guava:guava:34.+\"\n"
        );
    }
}
