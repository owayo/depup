//! CLI argument parsing module for depup

use clap::{ArgAction, Parser};
use std::path::PathBuf;
use std::time::Duration;

/// Parse duration string in format: Nd (days), Nw (weeks), Nm (months)
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }

    let (num_str, unit) = if let Some(n) = s.strip_suffix('d') {
        (n, 'd')
    } else if let Some(n) = s.strip_suffix('w') {
        (n, 'w')
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 'm')
    } else {
        return Err(format!("invalid duration format: {}", s));
    };

    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number in duration: {}", num_str))?;

    let seconds = match unit {
        'd' => num * 24 * 60 * 60,      // days
        'w' => num * 7 * 24 * 60 * 60,  // weeks
        'm' => num * 30 * 24 * 60 * 60, // months (30 days)
        _ => unreachable!(),
    };

    Ok(Duration::from_secs(seconds))
}

/// Multi-language dependency updater
#[derive(Parser, Debug, Clone)]
#[command(name = "depup", version, about = "Multi-language dependency updater")]
pub struct CliArgs {
    /// Target directory (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    // General options
    /// Dry run mode - show what would be updated without making changes
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Enable verbose output
    #[arg(long)]
    pub verbose: bool,

    /// Enable quiet mode - minimal output
    #[arg(short, long)]
    pub quiet: bool,

    // Language filters
    /// Update only Node.js (package.json) dependencies
    #[arg(long)]
    pub node: bool,

    /// Update only Python (pyproject.toml) dependencies
    #[arg(long)]
    pub python: bool,

    /// Update only Rust (Cargo.toml) dependencies
    #[arg(long = "rust")]
    pub rust_lang: bool,

    /// Update only Go (go.mod) dependencies
    #[arg(long)]
    pub go: bool,

    /// Update only Ruby (Gemfile) dependencies
    #[arg(long)]
    pub ruby: bool,

    /// Update only PHP (composer.json) dependencies
    #[arg(long)]
    pub php: bool,

    // Package filters
    /// Exclude specific packages from update (can be specified multiple times)
    #[arg(long, action = ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Update only specific packages (can be specified multiple times)
    #[arg(long, action = ArgAction::Append)]
    pub only: Vec<String>,

    /// Include pinned versions in update
    #[arg(long)]
    pub include_pinned: bool,

    // Age filter
    /// Only update to versions released at least this long ago (e.g., 2w, 10d, 1m)
    #[arg(long, value_parser = parse_duration)]
    pub age: Option<Duration>,

    // Output options
    /// Output results in JSON format
    #[arg(long)]
    pub json: bool,

    /// Show changes in diff format
    #[arg(long)]
    pub diff: bool,

    // Install option
    /// Run package manager install after update
    #[arg(long)]
    pub install: bool,
}

impl CliArgs {
    /// Check if any language filter is specified
    pub fn has_language_filter(&self) -> bool {
        self.node || self.python || self.rust_lang || self.go || self.ruby || self.php
    }

    /// Check if a specific language should be processed
    pub fn should_process_language(&self, lang: &str) -> bool {
        if !self.has_language_filter() {
            return true; // No filter means process all
        }
        match lang {
            "node" | "nodejs" | "javascript" => self.node,
            "python" => self.python,
            "rust" => self.rust_lang,
            "go" | "golang" => self.go,
            "ruby" => self.ruby,
            "php" => self.php,
            _ => false,
        }
    }

    /// Check if a package should be processed based on filters
    pub fn should_process_package(&self, name: &str) -> bool {
        // If --only is specified, only process those packages
        if !self.only.is_empty() {
            return self.only.iter().any(|p| p == name);
        }
        // If --exclude is specified, skip those packages
        if self.exclude.iter().any(|p| p == name) {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_default_args() {
        let args = CliArgs::parse_from(["depup"]);
        assert_eq!(args.path, PathBuf::from("."));
        assert!(!args.dry_run);
        assert!(!args.verbose);
        assert!(!args.quiet);
        assert!(!args.node);
        assert!(!args.python);
        assert!(!args.rust_lang);
        assert!(!args.go);
        assert!(args.exclude.is_empty());
        assert!(args.only.is_empty());
        assert!(!args.include_pinned);
        assert!(args.age.is_none());
        assert!(!args.json);
        assert!(!args.diff);
        assert!(!args.install);
    }

    #[test]
    fn test_path_argument() {
        let args = CliArgs::parse_from(["depup", "/some/path"]);
        assert_eq!(args.path, PathBuf::from("/some/path"));
    }

    #[test]
    fn test_dry_run_short_flag() {
        let args = CliArgs::parse_from(["depup", "-n"]);
        assert!(args.dry_run);
    }

    #[test]
    fn test_dry_run_long_flag() {
        let args = CliArgs::parse_from(["depup", "--dry-run"]);
        assert!(args.dry_run);
    }

    #[test]
    fn test_verbose_flags() {
        let args = CliArgs::parse_from(["depup", "--verbose"]);
        assert!(args.verbose);
    }

    #[test]
    fn test_quiet_flags() {
        let args = CliArgs::parse_from(["depup", "-q"]);
        assert!(args.quiet);

        let args = CliArgs::parse_from(["depup", "--quiet"]);
        assert!(args.quiet);
    }

    #[test]
    fn test_language_filters() {
        let args = CliArgs::parse_from(["depup", "--node"]);
        assert!(args.node);
        assert!(!args.python);

        let args = CliArgs::parse_from(["depup", "--python"]);
        assert!(args.python);

        let args = CliArgs::parse_from(["depup", "--rust"]);
        assert!(args.rust_lang);

        let args = CliArgs::parse_from(["depup", "--go"]);
        assert!(args.go);
    }

    #[test]
    fn test_multiple_language_filters() {
        let args = CliArgs::parse_from(["depup", "--node", "--python"]);
        assert!(args.node);
        assert!(args.python);
        assert!(!args.rust_lang);
        assert!(!args.go);
    }

    #[test]
    fn test_exclude_multiple() {
        let args = CliArgs::parse_from(["depup", "--exclude", "foo", "--exclude", "bar"]);
        assert_eq!(args.exclude, vec!["foo", "bar"]);
    }

    #[test]
    fn test_only_multiple() {
        let args = CliArgs::parse_from(["depup", "--only", "foo", "--only", "bar"]);
        assert_eq!(args.only, vec!["foo", "bar"]);
    }

    #[test]
    fn test_include_pinned() {
        let args = CliArgs::parse_from(["depup", "--include-pinned"]);
        assert!(args.include_pinned);
    }

    #[test]
    fn test_age_days() {
        let args = CliArgs::parse_from(["depup", "--age", "10d"]);
        assert_eq!(args.age, Some(Duration::from_secs(10 * 24 * 60 * 60)));
    }

    #[test]
    fn test_age_weeks() {
        let args = CliArgs::parse_from(["depup", "--age", "2w"]);
        assert_eq!(args.age, Some(Duration::from_secs(2 * 7 * 24 * 60 * 60)));
    }

    #[test]
    fn test_age_months() {
        let args = CliArgs::parse_from(["depup", "--age", "1m"]);
        assert_eq!(args.age, Some(Duration::from_secs(30 * 24 * 60 * 60)));
    }

    #[test]
    fn test_json_output() {
        let args = CliArgs::parse_from(["depup", "--json"]);
        assert!(args.json);
    }

    #[test]
    fn test_diff_output() {
        let args = CliArgs::parse_from(["depup", "--diff"]);
        assert!(args.diff);
    }

    #[test]
    fn test_install_flag() {
        let args = CliArgs::parse_from(["depup", "--install"]);
        assert!(args.install);
    }

    #[test]
    fn test_has_language_filter() {
        let args = CliArgs::parse_from(["depup"]);
        assert!(!args.has_language_filter());

        let args = CliArgs::parse_from(["depup", "--node"]);
        assert!(args.has_language_filter());
    }

    #[test]
    fn test_should_process_language() {
        let args = CliArgs::parse_from(["depup"]);
        assert!(args.should_process_language("node"));
        assert!(args.should_process_language("python"));
        assert!(args.should_process_language("rust"));
        assert!(args.should_process_language("go"));

        let args = CliArgs::parse_from(["depup", "--node", "--python"]);
        assert!(args.should_process_language("node"));
        assert!(args.should_process_language("python"));
        assert!(!args.should_process_language("rust"));
        assert!(!args.should_process_language("go"));
    }

    #[test]
    fn test_should_process_package() {
        let args = CliArgs::parse_from(["depup"]);
        assert!(args.should_process_package("any-package"));

        let args = CliArgs::parse_from(["depup", "--exclude", "foo"]);
        assert!(!args.should_process_package("foo"));
        assert!(args.should_process_package("bar"));

        let args = CliArgs::parse_from(["depup", "--only", "foo"]);
        assert!(args.should_process_package("foo"));
        assert!(!args.should_process_package("bar"));
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(
            parse_duration("7d").unwrap(),
            Duration::from_secs(7 * 86400)
        );
        assert_eq!(
            parse_duration("1w").unwrap(),
            Duration::from_secs(7 * 86400)
        );
        assert_eq!(
            parse_duration("2w").unwrap(),
            Duration::from_secs(14 * 86400)
        );
        assert_eq!(
            parse_duration("1m").unwrap(),
            Duration::from_secs(30 * 86400)
        );
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("10x").is_err());
    }

    #[test]
    fn test_combined_flags() {
        let args = CliArgs::parse_from([
            "depup",
            "/path/to/project",
            "-n",
            "--verbose",
            "--node",
            "--python",
            "--exclude",
            "lodash",
            "--age",
            "2w",
            "--json",
        ]);
        assert_eq!(args.path, PathBuf::from("/path/to/project"));
        assert!(args.dry_run);
        assert!(args.verbose);
        assert!(args.node);
        assert!(args.python);
        assert!(!args.rust_lang);
        assert!(!args.go);
        assert_eq!(args.exclude, vec!["lodash"]);
        assert_eq!(args.age, Some(Duration::from_secs(14 * 86400)));
        assert!(args.json);
    }
}
