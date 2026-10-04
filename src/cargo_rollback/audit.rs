//! 監査用の lock 読み込み。読めない lock を空の解決結果として扱わない。

use crate::manifest::{RegistryLockEntries, parse_registry_entries};
use std::path::Path;

pub(crate) fn read_audit_lock(path: &Path) -> Result<(String, RegistryLockEntries), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read Cargo.lock for age audit: {error}"))?;
    let document: toml::Value = toml::from_str(&content)
        .map_err(|error| format!("cannot parse Cargo.lock for age audit: {error}"))?;
    if let Some(packages) = document.get("package") {
        let packages = packages
            .as_array()
            .ok_or("invalid Cargo.lock package list")?;
        if packages.iter().any(|package| {
            package.get("name").and_then(toml::Value::as_str).is_none()
                || package
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .is_none()
                || package
                    .get("source")
                    .is_some_and(|source| source.as_str().is_none())
        }) {
            return Err("invalid Cargo.lock package entry".into());
        }
    } else if document
        .get("version")
        .and_then(toml::Value::as_integer)
        .is_none()
    {
        return Err("invalid Cargo.lock: missing version and package list".into());
    }
    let entries = parse_registry_entries(&content);
    Ok((content, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_or_invalid_lock_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Cargo.lock");
        assert!(read_audit_lock(&path).is_err());
        for content in [
            "",
            "broken {{{",
            "package = 1",
            "[[package]]\nname = 'library'\n",
        ] {
            std::fs::write(&path, content).unwrap();
            assert!(read_audit_lock(&path).is_err(), "{content}");
        }
        std::fs::write(&path, "version = 4\n").unwrap();
        assert!(read_audit_lock(&path).unwrap().1.is_empty());
    }
}
