//! `.depup` configuration file parser
//!
//! Parses `.depup` files that list subdirectories for monorepo support.
//! Each line is a relative path to a subdirectory containing manifest files.
//! Lines starting with `#` are comments, and inline `#` comments are supported.

use std::path::{Path, PathBuf};

/// Configuration parsed from a `.depup` file
#[derive(Debug, Clone)]
pub struct DepupConfig {
    /// List of subdirectories to process
    pub directories: Vec<PathBuf>,
}

impl DepupConfig {
    /// Look for a `.depup` file in the given directory and parse it
    pub fn from_dir(dir: &Path) -> Option<Self> {
        let config_path = dir.join(".depup");
        if !config_path.is_file() {
            return None;
        }

        let content = std::fs::read_to_string(&config_path).ok()?;
        match Self::parse(&content, dir) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("Warning: failed to parse .depup: {}", e);
                None
            }
        }
    }

    /// Build the list of directories to scan, always including the root directory.
    ///
    /// The root directory is placed first, followed by any `.depup`-listed directories
    /// that are not the root itself (to avoid duplicates).
    pub fn directories_with_root(&self, root: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![root.to_path_buf()];
        for dir in &self.directories {
            if *dir != root {
                dirs.push(dir.clone());
            }
        }
        dirs
    }

    /// Parse the content of a `.depup` file
    ///
    /// Each line is treated as a relative path. `#` starts a comment (line or inline).
    /// Empty lines and comment-only lines are ignored.
    /// Non-existent directories are warned and skipped.
    pub fn parse(content: &str, base_dir: &Path) -> Result<Self, String> {
        let mut directories = Vec::new();

        for line in content.lines() {
            // Strip inline comments
            let stripped = match line.find('#') {
                Some(pos) => &line[..pos],
                None => line,
            };

            let trimmed = stripped.trim();
            if trimmed.is_empty() {
                continue;
            }

            let dir_path = base_dir.join(trimmed);
            if !dir_path.is_dir() {
                eprintln!(
                    "Warning: directory '{}' not found, skipping",
                    dir_path.display()
                );
                continue;
            }

            directories.push(dir_path);
        }

        Ok(DepupConfig { directories })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        tempfile::tempdir().expect("Failed to create temp directory")
    }

    #[test]
    fn test_parse_empty_file() {
        let dir = create_test_dir();
        let config = DepupConfig::parse("", dir.path()).unwrap();
        assert!(config.directories.is_empty());
    }

    #[test]
    fn test_parse_comments_only() {
        let dir = create_test_dir();
        let content = "# This is a comment\n# Another comment\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert!(config.directories.is_empty());
    }

    #[test]
    fn test_parse_blank_lines_only() {
        let dir = create_test_dir();
        let content = "\n\n  \n\t\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert!(config.directories.is_empty());
    }

    #[test]
    fn test_parse_valid_directories() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("web")).unwrap();
        fs::create_dir(dir.path().join("cli")).unwrap();

        let content = "gui\nweb\ncli\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 3);
        assert_eq!(config.directories[0], dir.path().join("gui"));
        assert_eq!(config.directories[1], dir.path().join("web"));
        assert_eq!(config.directories[2], dir.path().join("cli"));
    }

    #[test]
    fn test_parse_inline_comments() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("web")).unwrap();

        let content = "gui  # frontend app\nweb # web server\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 2);
        assert_eq!(config.directories[0], dir.path().join("gui"));
        assert_eq!(config.directories[1], dir.path().join("web"));
    }

    #[test]
    fn test_parse_trailing_slashes() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();

        // Trailing slash should work since Path::join handles it
        let content = "gui/\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 1);
    }

    #[test]
    fn test_parse_nonexistent_directory_skipped() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();

        let content = "gui\nnonexistent\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 1);
        assert_eq!(config.directories[0], dir.path().join("gui"));
    }

    #[test]
    fn test_parse_mixed_content() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("cli")).unwrap();

        let content = "\
# depup monorepo config
gui  # frontend

# backend services
cli

# this dir doesn't exist
missing
";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 2);
        assert_eq!(config.directories[0], dir.path().join("gui"));
        assert_eq!(config.directories[1], dir.path().join("cli"));
    }

    #[test]
    fn test_from_dir_no_config_file() {
        let dir = create_test_dir();
        assert!(DepupConfig::from_dir(dir.path()).is_none());
    }

    #[test]
    fn test_from_dir_with_config_file() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("app")).unwrap();
        fs::write(dir.path().join(".depup"), "app\n").unwrap();

        let config = DepupConfig::from_dir(dir.path()).unwrap();
        assert_eq!(config.directories.len(), 1);
        assert_eq!(config.directories[0], dir.path().join("app"));
    }

    #[test]
    fn test_from_dir_empty_config() {
        let dir = create_test_dir();
        fs::write(dir.path().join(".depup"), "").unwrap();

        let config = DepupConfig::from_dir(dir.path()).unwrap();
        assert!(config.directories.is_empty());
    }

    #[test]
    fn test_directories_with_root_always_includes_root() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("api")).unwrap();

        let config = DepupConfig::parse("gui\napi\n", dir.path()).unwrap();
        let dirs = config.directories_with_root(dir.path());

        assert_eq!(dirs.len(), 3);
        assert_eq!(dirs[0], dir.path().to_path_buf(), "root should be first");
        assert_eq!(dirs[1], dir.path().join("gui"));
        assert_eq!(dirs[2], dir.path().join("api"));
    }

    #[test]
    fn test_directories_with_root_no_duplicate_when_root_in_depup() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();

        // .depup にルート自身のパスが含まれていても重複しない
        let directories = vec![dir.path().to_path_buf(), dir.path().join("gui")];
        let config = DepupConfig { directories };
        let dirs = config.directories_with_root(dir.path());

        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0], dir.path().to_path_buf());
        assert_eq!(dirs[1], dir.path().join("gui"));
    }

    #[test]
    fn test_directories_with_root_empty_depup() {
        let dir = create_test_dir();
        fs::write(dir.path().join(".depup"), "").unwrap();

        let config = DepupConfig::from_dir(dir.path()).unwrap();
        let dirs = config.directories_with_root(dir.path());

        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], dir.path().to_path_buf(), "root only");
    }
}
