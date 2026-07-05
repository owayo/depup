//! Gradle version catalog (`*.versions.toml`) のパースと更新

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::line_utils::split_line_ending;
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

fn toml_section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let trimmed = trimmed.split('#').next().unwrap_or("").trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }

    let section = trimmed.trim_start_matches('[').trim_end_matches(']');
    if section.starts_with('[') || section.ends_with(']') {
        return None;
    }

    Some(section.trim().to_string())
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

fn update_version_catalog_version_alias(
    content: &str,
    alias: &str,
    update_member: Option<&str>,
    formatted_version: &str,
) -> Option<String> {
    let mut result = String::new();
    let mut section = String::new();
    let alias_section = format!("versions.{}", alias);
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

        if !updated && section == "versions" && !trimmed.starts_with('#') {
            if let Some(replaced) = replace_version_alias_line(
                line,
                alias,
                update_member,
                formatted_version,
                &mut in_target_block,
            ) {
                next_line = replaced;
                updated = true;
            }
        } else if !updated
            && section == alias_section
            && !trimmed.starts_with('#')
            && let Some(member) = update_member
            && let Some(replaced) = replace_toml_string_member(line, member, formatted_version)
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
    let mut result = String::new();
    let mut section = String::new();
    let alias_section = format!("libraries.{}", entry.alias);
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

        if !updated && section == "libraries" && !trimmed.starts_with('#') {
            if let Some(replaced) = replace_library_dotted_line(line, entry, formatted_version)
                .or_else(|| {
                    replace_library_assignment_line(
                        line,
                        entry,
                        group,
                        artifact,
                        formatted_version,
                        &mut in_target_block,
                    )
                })
            {
                next_line = replaced;
                updated = true;
            }
        } else if !updated
            && section == alias_section
            && !trimmed.starts_with('#')
            && let Some(replaced) = replace_library_version_value(line, entry, formatted_version)
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

pub(super) fn parse(content: &str) -> Result<Option<Vec<Dependency>>, ManifestError> {
    Ok(parse_version_catalog_entries(content)?.map(|entries| {
        entries
            .into_iter()
            .map(|entry| Dependency::production(entry.name, entry.version.spec, Language::Java))
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
