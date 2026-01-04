//! Manifest file detection and parsing
//!
//! This module provides functionality to:
//! - Detect manifest files in a directory
//! - Parse dependencies from different manifest formats
//! - Support monorepo structures (pnpm-workspace.yaml)
//! - Support Tauri projects (src-tauri/Cargo.toml)

mod cargo_toml;
mod detector;
mod go_mod;
mod package_json;
mod pnpm_settings;
mod pyproject_toml;
mod writer;

pub use cargo_toml::CargoTomlParser;
pub use detector::{detect_manifests, ManifestFile, ManifestInfo};
pub use go_mod::GoModParser;
pub use package_json::PackageJsonParser;
pub use pnpm_settings::{has_pnpm_workspace, PnpmSettings};
pub use pyproject_toml::PyprojectTomlParser;
pub use writer::{read_manifest, write_manifest, ManifestWriter, WriteResult};

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use std::path::Path;

/// Trait for parsing manifest files
pub trait ManifestParser {
    /// Parse dependencies from a manifest file
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError>;

    /// Returns the language this parser handles
    fn language(&self) -> Language;

    /// Update a dependency version in the manifest content
    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError>;
}

/// Get a manifest parser for the specified language
pub fn get_parser(language: Language) -> Box<dyn ManifestParser> {
    match language {
        Language::Node => Box::new(PackageJsonParser),
        Language::Python => Box::new(PyprojectTomlParser),
        Language::Rust => Box::new(CargoTomlParser),
        Language::Go => Box::new(GoModParser),
    }
}

/// Parse dependencies from a manifest file path
pub fn parse_manifest(path: &Path) -> Result<Vec<Dependency>, ManifestError> {
    let content = std::fs::read_to_string(path).map_err(|e| ManifestError::ReadError {
        path: path.to_path_buf(),
        source: e,
    })?;

    let language = Language::all()
        .iter()
        .find(|lang| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == lang.manifest_filename())
                .unwrap_or(false)
        })
        .ok_or_else(|| ManifestError::UnsupportedFormat {
            path: path.to_path_buf(),
        })?;

    let parser = get_parser(*language);
    parser.parse(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_parser_node() {
        let parser = get_parser(Language::Node);
        assert_eq!(parser.language(), Language::Node);
    }

    #[test]
    fn test_get_parser_python() {
        let parser = get_parser(Language::Python);
        assert_eq!(parser.language(), Language::Python);
    }

    #[test]
    fn test_get_parser_rust() {
        let parser = get_parser(Language::Rust);
        assert_eq!(parser.language(), Language::Rust);
    }

    #[test]
    fn test_get_parser_go() {
        let parser = get_parser(Language::Go);
        assert_eq!(parser.language(), Language::Go);
    }
}
