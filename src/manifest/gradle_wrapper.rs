//! Gradle Wrapper distribution URL parsing and format-preserving updates.

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::line_utils::split_line_ending;
use crate::parser::get_parser;
use std::path::PathBuf;

pub const GRADLE_WRAPPER_PACKAGE: &str = "gradle:wrapper";

fn is_distribution_url_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with(['#', '!']) {
        return false;
    }
    line.split_once('=')
        .is_some_and(|(key, _)| key.trim() == "distributionUrl")
}

fn distribution_version(line: &str) -> Option<(usize, usize, &str)> {
    if !is_distribution_url_line(line) {
        return None;
    }
    let (key, _) = line.split_once('=')?;
    let value_start = key.len() + 1;
    let value = &line[value_start..];
    let leading = value.len() - value.trim_start().len();
    let raw = value.trim();
    let prefixes = [
        "https\\://services.gradle.org/distributions/gradle-",
        "https://services.gradle.org/distributions/gradle-",
        "https\\://downloads.gradle.org/distributions/gradle-",
        "https://downloads.gradle.org/distributions/gradle-",
    ];
    let prefix = prefixes.iter().find(|prefix| raw.starts_with(**prefix))?;
    let remainder = raw.strip_prefix(prefix)?;
    let version = remainder
        .strip_suffix("-bin.zip")
        .or_else(|| remainder.strip_suffix("-all.zip"))?;
    if version.is_empty()
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
    {
        return None;
    }
    let start = value_start + leading + prefix.len();
    Some((start, start + version.len(), version))
}

pub(super) fn parse(content: &str) -> Result<Option<Vec<Dependency>>, ManifestError> {
    let declaration_count = content
        .lines()
        .filter(|line| is_distribution_url_line(line))
        .count();
    if declaration_count == 0 {
        return Ok(None);
    }
    if declaration_count > 1 {
        return Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("gradle-wrapper.properties"),
            spec: GRADLE_WRAPPER_PACKAGE.to_string(),
            message: "multiple distributionUrl entries".to_string(),
        });
    }
    let versions: Vec<_> = content.lines().filter_map(distribution_version).collect();
    if versions.len() != 1 {
        return Ok(Some(Vec::new()));
    }
    if content
        .lines()
        .any(|line| line.trim_start().starts_with("distributionSha256Sum"))
    {
        return Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("gradle-wrapper.properties"),
            spec: GRADLE_WRAPPER_PACKAGE.to_string(),
            message: "distributionSha256Sum must be updated together with distributionUrl"
                .to_string(),
        });
    }
    let version = versions[0].2;
    let Some(spec) = get_parser(Language::Java).parse(version) else {
        return Ok(Some(Vec::new()));
    };
    Ok(Some(vec![Dependency::production(
        GRADLE_WRAPPER_PACKAGE,
        VersionSpec::new(VersionSpecKind::Exact, spec.version, version),
        Language::Java,
    )]))
}

pub(super) fn update_version(
    content: &str,
    package: &str,
    new_version: &str,
) -> Result<Option<String>, ManifestError> {
    if package != GRADLE_WRAPPER_PACKAGE || parse(content)?.is_none() {
        return Ok(None);
    }
    if get_parser(Language::Java).parse(new_version).is_none()
        || !new_version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
    {
        return Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("gradle-wrapper.properties"),
            spec: new_version.to_string(),
            message: "invalid Gradle distribution version".to_string(),
        });
    }
    let mut result = String::with_capacity(content.len() + new_version.len());
    let mut changed = false;
    for raw_line in content.split_inclusive('\n') {
        let (line, ending) = split_line_ending(raw_line);
        if let Some((start, end, _)) = distribution_version(line) {
            if changed {
                return Err(ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("gradle-wrapper.properties"),
                    spec: package.to_string(),
                    message: "multiple distributionUrl entries".to_string(),
                });
            }
            result.push_str(&line[..start]);
            result.push_str(new_version);
            result.push_str(&line[end..]);
            changed = true;
        } else {
            result.push_str(line);
        }
        result.push_str(ending);
    }
    Ok(changed.then_some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_url_is_parsed_and_updated_without_changing_format() {
        let content = "distributionBase=GRADLE_USER_HOME\r\ndistributionUrl=https\\://services.gradle.org/distributions/gradle-9.7.0-all.zip\r\nzipStoreBase=GRADLE_USER_HOME\r\n";
        let dep = parse(content).unwrap().unwrap().pop().unwrap();
        assert_eq!(dep.name, GRADLE_WRAPPER_PACKAGE);
        assert_eq!(dep.version_spec.version, "9.7.0");
        let updated = update_version(content, GRADLE_WRAPPER_PACKAGE, "9.8.0")
            .unwrap()
            .unwrap();
        assert_eq!(
            updated,
            content.replace("gradle-9.7.0-all.zip", "gradle-9.8.0-all.zip")
        );
    }

    #[test]
    fn nonstandard_url_and_pinned_checksum_are_not_rewritten() {
        assert!(
            parse("distributionUrl=https\\://example.com/gradle-9.7-bin.zip\n")
                .unwrap()
                .unwrap()
                .is_empty()
        );
        let content = "distributionUrl=https\\://services.gradle.org/distributions/gradle-9.7-bin.zip\ndistributionSha256Sum=abc\n";
        assert!(parse(content).is_err());
        assert!(update_version(content, GRADLE_WRAPPER_PACKAGE, "9.8").is_err());
        let duplicate = "distributionUrl=https\\://services.gradle.org/distributions/gradle-9.7-bin.zip\ndistributionUrl=https\\://example.com/gradle-9.6-bin.zip\n";
        assert!(parse(duplicate).is_err());
    }
}
