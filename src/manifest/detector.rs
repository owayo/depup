//! Manifest file detection with monorepo and Tauri support
//!
//! Features:
//! - Detects package.json, pyproject.toml, Cargo.toml, go.mod
//! - Supports pnpm-workspace.yaml for monorepo detection
//! - Supports Tauri projects (src-tauri/Cargo.toml)

use crate::domain::Language;
use std::path::{Path, PathBuf};

/// Information about a detected manifest file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInfo {
    /// Path to the manifest file
    pub path: PathBuf,
    /// Language/ecosystem of the manifest
    pub language: Language,
    /// Whether this is a workspace root manifest
    pub is_workspace_root: bool,
    /// Whether this manifest is from a Tauri project's src-tauri directory
    pub is_tauri_rust: bool,
}

impl ManifestInfo {
    /// Create a new ManifestInfo
    pub fn new(path: impl Into<PathBuf>, language: Language) -> Self {
        Self {
            path: path.into(),
            language,
            is_workspace_root: false,
            is_tauri_rust: false,
        }
    }

    /// Mark this manifest as a workspace root
    pub fn with_workspace_root(mut self, is_root: bool) -> Self {
        self.is_workspace_root = is_root;
        self
    }

    /// Mark this manifest as Tauri Rust project
    pub fn with_tauri_rust(mut self, is_tauri: bool) -> Self {
        self.is_tauri_rust = is_tauri;
        self
    }
}

/// Represents a detected manifest file
#[derive(Debug, Clone)]
pub struct ManifestFile {
    /// Path to the manifest file
    pub path: PathBuf,
    /// Content of the manifest file
    pub content: String,
    /// Information about the manifest
    pub info: ManifestInfo,
}

/// Detect all manifest files in the given directory
///
/// This function:
/// 1. Looks for standard manifest files (package.json, pyproject.toml, Cargo.toml, go.mod)
/// 2. Checks for pnpm-workspace.yaml to detect monorepo
/// 3. Checks for src-tauri/Cargo.toml for Tauri projects
pub fn detect_manifests(dir: &Path) -> Vec<ManifestInfo> {
    let mut manifests = Vec::new();

    // Check if this is a pnpm workspace
    let is_pnpm_workspace = dir.join("pnpm-workspace.yaml").exists();

    // Detect each manifest type
    for language in Language::all() {
        let manifest_name = language.manifest_filename();
        let manifest_path = dir.join(manifest_name);

        if manifest_path.exists() {
            let mut info = ManifestInfo::new(&manifest_path, *language);

            // Mark as workspace root if pnpm-workspace.yaml exists and this is package.json
            if *language == Language::Node && is_pnpm_workspace {
                info = info.with_workspace_root(true);
            }

            manifests.push(info);
        }
    }

    // Check for Tauri project (src-tauri/Cargo.toml)
    let tauri_cargo_path = dir.join("src-tauri").join("Cargo.toml");
    if tauri_cargo_path.exists() {
        // Only add if we haven't already added a root Cargo.toml
        let has_root_cargo = manifests
            .iter()
            .any(|m| m.language == Language::Rust && m.path == dir.join("Cargo.toml"));

        // Always add Tauri Cargo.toml
        let tauri_info = ManifestInfo::new(&tauri_cargo_path, Language::Rust).with_tauri_rust(true);
        manifests.push(tauri_info);

        // If there's no root Cargo.toml but src-tauri exists, this is likely a Tauri-only project
        if !has_root_cargo {
            // The Tauri Cargo.toml is already added above
        }
    }

    // Check for pnpm workspace packages if pnpm-workspace.yaml exists
    if is_pnpm_workspace {
        if let Ok(workspace_packages) = detect_pnpm_workspace_packages(dir) {
            for package_path in workspace_packages {
                let package_json_path = package_path.join("package.json");
                if package_json_path.exists() {
                    // Don't add if it's the root package.json
                    if package_json_path != dir.join("package.json") {
                        manifests.push(ManifestInfo::new(&package_json_path, Language::Node));
                    }
                }
            }
        }
    }

    manifests
}

/// Parse pnpm-workspace.yaml and return package directories
fn detect_pnpm_workspace_packages(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let workspace_file = dir.join("pnpm-workspace.yaml");
    let content = std::fs::read_to_string(&workspace_file)?;

    let mut packages = Vec::new();

    // Simple YAML parsing for packages array
    // Format: packages:
    //           - 'packages/*'
    //           - 'apps/*'
    let mut in_packages = false;
    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("packages:") {
            in_packages = true;
            continue;
        }

        if in_packages {
            // Check if we've moved to a new section
            if !trimmed.is_empty() && !trimmed.starts_with('-') && !trimmed.starts_with('#') {
                break;
            }

            // Parse package glob pattern
            if let Some(pattern) = trimmed.strip_prefix('-') {
                let pattern = pattern.trim().trim_matches('\'').trim_matches('"');

                // Handle glob patterns like 'packages/*' or 'apps/**'
                if let Some(base) = pattern.strip_suffix("/*") {
                    // List directories in the base path
                    let base_path = dir.join(base);
                    if let Ok(entries) = std::fs::read_dir(&base_path) {
                        for entry in entries.flatten() {
                            if entry.path().is_dir() {
                                packages.push(entry.path());
                            }
                        }
                    }
                } else if let Some(base) = pattern.strip_suffix("/**") {
                    // For ** patterns, we just use the first level for now
                    let base_path = dir.join(base);
                    if let Ok(entries) = std::fs::read_dir(&base_path) {
                        for entry in entries.flatten() {
                            if entry.path().is_dir() {
                                packages.push(entry.path());
                            }
                        }
                    }
                } else if !pattern.contains('*') {
                    // Direct path without glob
                    let pkg_path = dir.join(pattern);
                    if pkg_path.exists() {
                        packages.push(pkg_path);
                    }
                }
            }
        }
    }

    Ok(packages)
}

/// Check if a directory is a Tauri project
#[allow(dead_code)]
pub fn is_tauri_project(dir: &Path) -> bool {
    dir.join("src-tauri").exists() && dir.join("src-tauri").join("Cargo.toml").exists()
}

/// Check if a directory is a pnpm workspace
#[allow(dead_code)]
pub fn is_pnpm_workspace(dir: &Path) -> bool {
    dir.join("pnpm-workspace.yaml").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_detect_package_json() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Node);
        assert!(!manifests[0].is_workspace_root);
    }

    #[test]
    fn test_detect_multiple_manifests() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        fs::write(dir.path().join("go.mod"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 4);

        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(languages.contains(&Language::Node));
        assert!(languages.contains(&Language::Rust));
        assert!(languages.contains(&Language::Python));
        assert!(languages.contains(&Language::Go));
    }

    #[test]
    fn test_detect_pnpm_workspace() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let root = manifests
            .iter()
            .find(|m| m.path == dir.path().join("package.json"))
            .unwrap();
        assert!(root.is_workspace_root);
    }

    #[test]
    fn test_detect_tauri_project() {
        let dir = create_temp_dir();
        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        fs::write(dir.path().join("src-tauri").join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 2);

        let tauri_manifest = manifests.iter().find(|m| m.is_tauri_rust).unwrap();
        assert_eq!(tauri_manifest.language, Language::Rust);
        assert!(tauri_manifest.path.ends_with("src-tauri/Cargo.toml"));
    }

    #[test]
    fn test_detect_tauri_with_root_cargo() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        fs::write(dir.path().join("src-tauri").join("Cargo.toml"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        // Should have both root Cargo.toml and src-tauri/Cargo.toml
        let rust_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Rust)
            .collect();
        assert_eq!(rust_manifests.len(), 2);

        // One should be tauri, one should not
        assert!(rust_manifests.iter().any(|m| m.is_tauri_rust));
        assert!(rust_manifests.iter().any(|m| !m.is_tauri_rust));
    }

    #[test]
    fn test_detect_empty_directory() {
        let dir = create_temp_dir();
        let manifests = detect_manifests(dir.path());
        assert!(manifests.is_empty());
    }

    #[test]
    fn test_is_tauri_project() {
        let dir = create_temp_dir();
        assert!(!is_tauri_project(dir.path()));

        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        assert!(!is_tauri_project(dir.path()));

        fs::write(dir.path().join("src-tauri").join("Cargo.toml"), "").unwrap();
        assert!(is_tauri_project(dir.path()));
    }

    #[test]
    fn test_is_pnpm_workspace() {
        let dir = create_temp_dir();
        assert!(!is_pnpm_workspace(dir.path()));

        fs::write(dir.path().join("pnpm-workspace.yaml"), "").unwrap();
        assert!(is_pnpm_workspace(dir.path()));
    }

    #[test]
    fn test_manifest_info_builder() {
        let info = ManifestInfo::new("/test/package.json", Language::Node)
            .with_workspace_root(true)
            .with_tauri_rust(false);

        assert_eq!(info.path, PathBuf::from("/test/package.json"));
        assert_eq!(info.language, Language::Node);
        assert!(info.is_workspace_root);
        assert!(!info.is_tauri_rust);
    }

    #[test]
    fn test_pnpm_workspace_packages_detection() {
        let dir = create_temp_dir();

        // Create pnpm-workspace.yaml
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n  - 'apps/*'\n",
        )
        .unwrap();

        // Create root package.json
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        // Create packages directory with sub-packages
        fs::create_dir(dir.path().join("packages")).unwrap();
        fs::create_dir(dir.path().join("packages").join("pkg-a")).unwrap();
        fs::write(
            dir.path()
                .join("packages")
                .join("pkg-a")
                .join("package.json"),
            "{}",
        )
        .unwrap();
        fs::create_dir(dir.path().join("packages").join("pkg-b")).unwrap();
        fs::write(
            dir.path()
                .join("packages")
                .join("pkg-b")
                .join("package.json"),
            "{}",
        )
        .unwrap();

        // Create apps directory
        fs::create_dir(dir.path().join("apps")).unwrap();
        fs::create_dir(dir.path().join("apps").join("web")).unwrap();
        fs::write(
            dir.path().join("apps").join("web").join("package.json"),
            "{}",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());

        // Should find: root package.json + pkg-a + pkg-b + apps/web
        let node_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Node)
            .collect();
        assert_eq!(node_manifests.len(), 4);

        // Root should be marked as workspace root
        let root = node_manifests
            .iter()
            .find(|m| m.path == dir.path().join("package.json"))
            .unwrap();
        assert!(root.is_workspace_root);
    }
}
