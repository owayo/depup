//! Package manager integration for installing dependencies after updates
//!
//! This module provides:
//! - Detection of installed package managers
//! - Execution of install commands for each language

use crate::domain::Language;
use std::path::Path;
use std::process::{Command, Output};

/// Result of a package manager installation
#[derive(Debug, Clone)]
pub struct InstallResult {
    /// The language/package manager used
    pub language: Language,
    /// The command that was executed
    pub command: String,
    /// Whether the command succeeded
    pub success: bool,
    /// Standard output from the command
    pub stdout: String,
    /// Standard error from the command
    pub stderr: String,
}

impl InstallResult {
    /// Create a successful install result
    pub fn success(language: Language, command: String, stdout: String, stderr: String) -> Self {
        Self {
            language,
            command,
            success: true,
            stdout,
            stderr,
        }
    }

    /// Create a failed install result
    pub fn failure(language: Language, command: String, stdout: String, stderr: String) -> Self {
        Self {
            language,
            command,
            success: false,
            stdout,
            stderr,
        }
    }

    /// Create a skipped result (no package manager found)
    pub fn skipped(language: Language) -> Self {
        Self {
            language,
            command: String::new(),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

/// Trait for running package manager install commands
pub trait PackageManagerRunner {
    /// Run the install command for a language in the specified directory
    fn run_install(&self, language: Language, working_dir: &Path) -> InstallResult;
}

/// Default package manager runner that executes real commands
#[derive(Debug, Default)]
pub struct SystemPackageManager;

impl SystemPackageManager {
    /// Create a new system package manager
    pub fn new() -> Self {
        Self
    }

    /// Detect the Node.js package manager to use
    fn detect_node_pm(&self, working_dir: &Path) -> Option<&'static str> {
        // Check for lockfiles in order of preference
        if working_dir.join("pnpm-lock.yaml").exists() {
            return Some("pnpm");
        }
        if working_dir.join("yarn.lock").exists() {
            return Some("yarn");
        }
        if working_dir.join("bun.lockb").exists() {
            return Some("bun");
        }
        if working_dir.join("package-lock.json").exists() {
            return Some("npm");
        }
        // Default to npm if package.json exists but no lockfile
        if working_dir.join("package.json").exists() {
            return Some("npm");
        }
        None
    }

    /// Detect the Python package manager to use
    fn detect_python_pm(&self, working_dir: &Path) -> Option<&'static str> {
        // Check for lockfiles/configs in order of preference
        if working_dir.join("uv.lock").exists() {
            return Some("uv");
        }
        if working_dir.join("poetry.lock").exists() {
            return Some("poetry");
        }
        if working_dir.join("rye.lock").exists() {
            return Some("rye");
        }
        if working_dir.join("Pipfile.lock").exists() {
            return Some("pipenv");
        }
        // Check for pyproject.toml with specific tool configurations
        if working_dir.join("pyproject.toml").exists() {
            // Default to pip if pyproject.toml exists
            return Some("pip");
        }
        if working_dir.join("requirements.txt").exists() {
            return Some("pip");
        }
        None
    }

    /// Get the install command for a package manager
    fn get_install_command(&self, pm: &str) -> Vec<&'static str> {
        match pm {
            // Node.js package managers
            "npm" => vec!["npm", "install"],
            "yarn" => vec!["yarn", "install"],
            "pnpm" => vec!["pnpm", "install"],
            "bun" => vec!["bun", "install"],
            // Python package managers
            "pip" => vec!["pip", "install", "-e", "."],
            "uv" => vec!["uv", "sync"],
            "poetry" => vec!["poetry", "install"],
            "rye" => vec!["rye", "sync"],
            "pipenv" => vec!["pipenv", "install"],
            // Rust
            "cargo" => vec!["cargo", "build"],
            // Go
            "go" => vec!["go", "mod", "download"],
            // Ruby
            "bundle" => vec!["bundle", "install"],
            // PHP
            "composer" => vec!["composer", "install"],
            _ => vec![],
        }
    }

    /// Run a command and capture output
    fn run_command(&self, command: &[&str], working_dir: &Path) -> std::io::Result<Output> {
        if command.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Empty command",
            ));
        }

        Command::new(command[0])
            .args(&command[1..])
            .current_dir(working_dir)
            .output()
    }
}

impl PackageManagerRunner for SystemPackageManager {
    fn run_install(&self, language: Language, working_dir: &Path) -> InstallResult {
        let pm = match language {
            Language::Node => self.detect_node_pm(working_dir),
            Language::Python => self.detect_python_pm(working_dir),
            Language::Rust => {
                if working_dir.join("Cargo.toml").exists() {
                    Some("cargo")
                } else {
                    None
                }
            }
            Language::Go => {
                if working_dir.join("go.mod").exists() {
                    Some("go")
                } else {
                    None
                }
            }
            Language::Ruby => {
                if working_dir.join("Gemfile").exists() {
                    Some("bundle")
                } else {
                    None
                }
            }
            Language::Php => {
                if working_dir.join("composer.json").exists() {
                    Some("composer")
                } else {
                    None
                }
            }
        };

        let Some(pm) = pm else {
            return InstallResult::skipped(language);
        };

        let command_parts = self.get_install_command(pm);
        if command_parts.is_empty() {
            return InstallResult::skipped(language);
        }

        let command_str = command_parts.join(" ");

        match self.run_command(&command_parts, working_dir) {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if output.status.success() {
                    InstallResult::success(language, command_str, stdout, stderr)
                } else {
                    InstallResult::failure(language, command_str, stdout, stderr)
                }
            }
            Err(e) => InstallResult::failure(
                language,
                command_str,
                String::new(),
                format!("Failed to execute command: {}", e),
            ),
        }
    }
}

/// Run install commands for all specified languages
pub fn run_installs<R: PackageManagerRunner>(
    runner: &R,
    languages: &[Language],
    working_dir: &Path,
) -> Vec<InstallResult> {
    languages
        .iter()
        .map(|lang| runner.run_install(*lang, working_dir))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock package manager runner for testing
    struct MockPackageManager {
        should_succeed: bool,
    }

    impl MockPackageManager {
        fn new(should_succeed: bool) -> Self {
            Self { should_succeed }
        }
    }

    impl PackageManagerRunner for MockPackageManager {
        fn run_install(&self, language: Language, _working_dir: &Path) -> InstallResult {
            if self.should_succeed {
                InstallResult::success(
                    language,
                    "mock install".to_string(),
                    "Install successful".to_string(),
                    String::new(),
                )
            } else {
                InstallResult::failure(
                    language,
                    "mock install".to_string(),
                    String::new(),
                    "Install failed".to_string(),
                )
            }
        }
    }

    #[test]
    fn test_install_result_success() {
        let result = InstallResult::success(
            Language::Node,
            "npm install".to_string(),
            "done".to_string(),
            String::new(),
        );
        assert!(result.success);
        assert_eq!(result.language, Language::Node);
        assert_eq!(result.command, "npm install");
    }

    #[test]
    fn test_install_result_failure() {
        let result = InstallResult::failure(
            Language::Python,
            "pip install".to_string(),
            String::new(),
            "error".to_string(),
        );
        assert!(!result.success);
        assert_eq!(result.language, Language::Python);
    }

    #[test]
    fn test_install_result_skipped() {
        let result = InstallResult::skipped(Language::Rust);
        assert!(result.success);
        assert!(result.command.is_empty());
    }

    #[test]
    fn test_mock_package_manager_success() {
        let runner = MockPackageManager::new(true);
        let result = runner.run_install(Language::Node, Path::new("."));
        assert!(result.success);
    }

    #[test]
    fn test_mock_package_manager_failure() {
        let runner = MockPackageManager::new(false);
        let result = runner.run_install(Language::Node, Path::new("."));
        assert!(!result.success);
    }

    #[test]
    fn test_run_installs() {
        let runner = MockPackageManager::new(true);
        let languages = vec![Language::Node, Language::Python];
        let results = run_installs(&runner, &languages, Path::new("."));

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
    }

    #[test]
    fn test_system_package_manager_new() {
        let _pm = SystemPackageManager::new();
        // Just verify it can be created without panic
    }

    #[test]
    fn test_get_install_command_npm() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("npm");
        assert_eq!(cmd, vec!["npm", "install"]);
    }

    #[test]
    fn test_get_install_command_yarn() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("yarn");
        assert_eq!(cmd, vec!["yarn", "install"]);
    }

    #[test]
    fn test_get_install_command_pnpm() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("pnpm");
        assert_eq!(cmd, vec!["pnpm", "install"]);
    }

    #[test]
    fn test_get_install_command_cargo() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("cargo");
        assert_eq!(cmd, vec!["cargo", "build"]);
    }

    #[test]
    fn test_get_install_command_go() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("go");
        assert_eq!(cmd, vec!["go", "mod", "download"]);
    }

    #[test]
    fn test_get_install_command_uv() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("uv");
        assert_eq!(cmd, vec!["uv", "sync"]);
    }

    #[test]
    fn test_get_install_command_poetry() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("poetry");
        assert_eq!(cmd, vec!["poetry", "install"]);
    }

    #[test]
    fn test_get_install_command_unknown() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("unknown");
        assert!(cmd.is_empty());
    }

    #[test]
    fn test_detect_node_pm_npm() {
        // Create a temp directory with package-lock.json
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("package-lock.json"), "{}").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), Some("npm"));
    }

    #[test]
    fn test_detect_node_pm_yarn() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("yarn.lock"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), Some("yarn"));
    }

    #[test]
    fn test_detect_node_pm_pnpm() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("pnpm-lock.yaml"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), Some("pnpm"));
    }

    #[test]
    fn test_detect_node_pm_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), Some("npm"));
    }

    #[test]
    fn test_detect_node_pm_none() {
        let temp_dir = tempfile::tempdir().unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), None);
    }

    #[test]
    fn test_detect_python_pm_uv() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("uv.lock"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("uv"));
    }

    #[test]
    fn test_detect_python_pm_poetry() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("poetry.lock"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("poetry"));
    }

    #[test]
    fn test_detect_python_pm_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("pyproject.toml"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("pip"));
    }

    #[test]
    fn test_detect_python_pm_none() {
        let temp_dir = tempfile::tempdir().unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), None);
    }

    #[test]
    fn test_run_install_skipped_no_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pm = SystemPackageManager::new();

        let result = pm.run_install(Language::Node, temp_dir.path());
        assert!(result.success);
        assert!(result.command.is_empty());
    }
}
