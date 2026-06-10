//! depup の CLI 引数解析モジュール

use crate::domain::ChangeLevel;
use clap::{ArgAction, Parser};
use std::path::PathBuf;
use std::time::Duration;

/// `--max-change` 用に `ChangeLevel` をパースする
pub fn parse_change_level(s: &str) -> Result<ChangeLevel, String> {
    ChangeLevel::parse(s)
}

/// 期間文字列をパースする (形式: Nd (日), Nw (週), Nm (月))
pub fn parse_duration(s: &str) -> Result<Duration, String> {
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

    // 単位ごとの秒数 (checked_mul でオーバーフローを防止)
    let unit_secs: u64 = match unit {
        'd' => 24 * 60 * 60,      // 日
        'w' => 7 * 24 * 60 * 60,  // 週
        'm' => 30 * 24 * 60 * 60, // 月 (30日)
        _ => unreachable!(),
    };
    let seconds = num
        .checked_mul(unit_secs)
        .ok_or_else(|| format!("duration is too large: {}", s))?;

    Ok(Duration::from_secs(seconds))
}

/// 多言語対応の依存関係アップデーター
#[derive(Parser, Debug, Clone)]
#[command(
    name = "depup",
    about = "Multi-language dependency updater",
    disable_version_flag = true
)]
pub struct CliArgs {
    /// Print version
    #[arg(short = 'V', long = "version")]
    pub print_version: bool,

    /// Target directory (default: current directory)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Change to directory before running
    #[arg(short = 'C', long = "cd", value_name = "DIR")]
    pub directory: Option<PathBuf>,

    // 一般オプション
    /// Dry run mode - show what would be updated without making changes
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Enable verbose output
    #[arg(long)]
    pub verbose: bool,

    /// Enable quiet mode - minimal output
    #[arg(short, long)]
    pub quiet: bool,

    // 言語フィルタ
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

    /// Update only Java (build.gradle) dependencies
    #[arg(long)]
    pub java: bool,

    /// Update only Swift (Package.swift) dependencies
    #[arg(long)]
    pub swift: bool,

    // パッケージフィルタ
    /// Exclude specific packages from update (can be specified multiple times)
    #[arg(long, action = ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Update only specific packages (can be specified multiple times)
    #[arg(long, action = ArgAction::Append)]
    pub only: Vec<String>,

    /// Include pinned versions in update
    #[arg(long)]
    pub include_pinned: bool,

    // 経過時間フィルタ
    /// Only update to versions released at least this long ago (e.g., 2w, 10d, 1m)
    #[arg(long, value_parser = parse_duration, conflicts_with = "no_age")]
    pub age: Option<Duration>,

    /// Disable the age filter for this run (overrides global config and default)
    #[arg(long = "no-age")]
    pub no_age: bool,

    // 脆弱性チェック (OSV.dev)
    /// Check candidate versions against the OSV.dev vulnerability database
    /// and skip versions with known vulnerabilities (requires network access)
    #[arg(long, conflicts_with = "no_osv")]
    pub osv: bool,

    /// Disable OSV vulnerability check for this run (overrides global config)
    #[arg(long = "no-osv")]
    pub no_osv: bool,

    // 変更レベル上限
    /// Limit the maximum allowed version change (patch / minor / major).
    /// Default: no limit (= major bumps allowed).
    #[arg(long, value_parser = parse_change_level)]
    pub max_change: Option<ChangeLevel>,

    // 出力オプション
    /// Output results in JSON format
    #[arg(long)]
    pub json: bool,

    /// Show changes in diff format
    #[arg(long)]
    pub diff: bool,

    // インストールオプション
    /// Run package manager install after update
    #[arg(long)]
    pub install: bool,
}

impl CliArgs {
    /// 言語フィルタが指定されているかを確認する
    pub fn has_language_filter(&self) -> bool {
        !self.selected_languages().is_empty()
    }

    /// CLI フラグで選択された言語の一覧を返す (順序: Node, Python, Rust, Go, Ruby, PHP, Java, Swift)
    pub fn selected_languages(&self) -> Vec<crate::domain::Language> {
        use crate::domain::Language;
        let flags: [(bool, Language); 8] = [
            (self.node, Language::Node),
            (self.python, Language::Python),
            (self.rust_lang, Language::Rust),
            (self.go, Language::Go),
            (self.ruby, Language::Ruby),
            (self.php, Language::Php),
            (self.java, Language::Java),
            (self.swift, Language::Swift),
        ];
        flags
            .into_iter()
            .filter_map(|(on, lang)| on.then_some(lang))
            .collect()
    }

    /// 指定された言語を処理すべきかを確認する
    pub fn should_process_language(&self, lang: &str) -> bool {
        if !self.has_language_filter() {
            return true; // フィルタなしの場合は全言語を処理
        }
        match lang {
            "node" | "nodejs" | "javascript" => self.node,
            "python" => self.python,
            "rust" => self.rust_lang,
            "go" | "golang" => self.go,
            "ruby" => self.ruby,
            "php" => self.php,
            "java" => self.java,
            "swift" => self.swift,
            _ => false,
        }
    }

    /// フィルタ条件に基づいてパッケージを処理すべきかを確認する
    pub fn should_process_package(&self, name: &str) -> bool {
        // --only が指定されている場合、そのパッケージのみ処理
        if !self.only.is_empty() {
            return self.only.iter().any(|p| p == name);
        }
        // --exclude が指定されている場合、そのパッケージをスキップ
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
        assert!(args.directory.is_none());
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
        assert!(!args.no_age);
        assert!(!args.osv);
        assert!(!args.no_osv);
        assert!(args.max_change.is_none());
        assert!(!args.json);
        assert!(!args.diff);
        assert!(!args.install);
    }

    #[test]
    fn test_cd_short_flag() {
        let args = CliArgs::parse_from(["depup", "-C", "/some/path"]);
        assert_eq!(args.directory, Some(PathBuf::from("/some/path")));
        assert_eq!(args.path, PathBuf::from("."));
    }

    #[test]
    fn test_cd_long_flag() {
        let args = CliArgs::parse_from(["depup", "--cd", "./hoge/fuga"]);
        assert_eq!(args.directory, Some(PathBuf::from("./hoge/fuga")));
        assert_eq!(args.path, PathBuf::from("."));
    }

    #[test]
    fn test_cd_with_path() {
        let args = CliArgs::parse_from(["depup", "--cd", "/work", "./subdir"]);
        assert_eq!(args.directory, Some(PathBuf::from("/work")));
        assert_eq!(args.path, PathBuf::from("./subdir"));
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

        let args = CliArgs::parse_from(["depup", "--java"]);
        assert!(args.java);
        assert!(!args.node);
    }

    #[test]
    fn test_multiple_language_filters() {
        let args = CliArgs::parse_from(["depup", "--node", "--python"]);
        assert!(args.node);
        assert!(args.python);
        assert!(!args.rust_lang);
        assert!(!args.go);
        assert!(!args.java);

        // Java と他の言語の組み合わせテスト
        let args = CliArgs::parse_from(["depup", "--java", "--node"]);
        assert!(args.java);
        assert!(args.node);
        assert!(!args.python);
        assert!(!args.rust_lang);
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
    fn test_no_age_flag() {
        let args = CliArgs::parse_from(["depup", "--no-age"]);
        assert!(args.no_age);
        assert!(args.age.is_none());
    }

    #[test]
    fn test_age_and_no_age_conflict() {
        // --age と --no-age は同時指定できない
        let result = CliArgs::try_parse_from(["depup", "--age", "1w", "--no-age"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_osv_flag() {
        let args = CliArgs::parse_from(["depup", "--osv"]);
        assert!(args.osv);
        assert!(!args.no_osv);
    }

    #[test]
    fn test_no_osv_flag() {
        let args = CliArgs::parse_from(["depup", "--no-osv"]);
        assert!(args.no_osv);
        assert!(!args.osv);
    }

    #[test]
    fn test_osv_and_no_osv_conflict() {
        // --osv と --no-osv は同時指定できない
        let result = CliArgs::try_parse_from(["depup", "--osv", "--no-osv"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_change_values() {
        let args = CliArgs::parse_from(["depup", "--max-change", "patch"]);
        assert_eq!(args.max_change, Some(ChangeLevel::Patch));

        let args = CliArgs::parse_from(["depup", "--max-change", "minor"]);
        assert_eq!(args.max_change, Some(ChangeLevel::Minor));

        let args = CliArgs::parse_from(["depup", "--max-change", "major"]);
        assert_eq!(args.max_change, Some(ChangeLevel::Major));
    }

    #[test]
    fn test_max_change_invalid_rejected() {
        let result = CliArgs::try_parse_from(["depup", "--max-change", "foo"]);
        assert!(result.is_err());
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

        let args = CliArgs::parse_from(["depup", "--java"]);
        assert!(args.has_language_filter());
    }

    #[test]
    fn test_should_process_language() {
        let args = CliArgs::parse_from(["depup"]);
        assert!(args.should_process_language("node"));
        assert!(args.should_process_language("python"));
        assert!(args.should_process_language("rust"));
        assert!(args.should_process_language("go"));
        assert!(args.should_process_language("java"));

        let args = CliArgs::parse_from(["depup", "--node", "--python"]);
        assert!(args.should_process_language("node"));
        assert!(args.should_process_language("python"));
        assert!(!args.should_process_language("rust"));
        assert!(!args.should_process_language("go"));
        assert!(!args.should_process_language("java"));

        // Javaのみのフィルタテスト
        let args = CliArgs::parse_from(["depup", "--java"]);
        assert!(args.should_process_language("java"));
        assert!(!args.should_process_language("node"));
        assert!(!args.should_process_language("python"));
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
    fn test_parse_duration_overflow() {
        // 乗算オーバーフローは panic せずエラーになる
        assert!(parse_duration("213503982334602d").is_err());
        assert!(parse_duration(&format!("{}w", u64::MAX)).is_err());
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
