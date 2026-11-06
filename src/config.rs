//! `.depup` 設定ファイルパーサ
//!
//! モノレポ対応のための `.depup` ファイルを解析する。
//! 各行はマニフェストファイルを含むサブディレクトリへの相対パス。
//! `#` で始まる行はコメントとして扱い、インラインの `#` コメントもサポートする。

use std::path::{Component, Path, PathBuf};

/// `.depup` ファイルから解析された設定
#[derive(Debug, Clone)]
pub struct DepupConfig {
    /// 処理対象のサブディレクトリ一覧
    pub directories: Vec<PathBuf>,
}

impl DepupConfig {
    /// 指定されたディレクトリ内の `.depup` ファイルを探して解析する
    pub fn from_dir(dir: &Path) -> Option<Self> {
        let config_path = dir.join(".depup");
        if !config_path.is_file() {
            return None;
        }

        match Self::load(&config_path, dir) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!("Warning: failed to parse .depup: {}", e);
                None
            }
        }
    }

    /// `.depup` を読んで解析する。読めない・UTF-8 でない内容も、解析できない内容と同じく
    /// エラーにする (呼び出し側が警告して `.depup` 全体を無視する)。以前は
    /// `read_to_string(...).ok()?` で読み、Windows PowerShell 5.1 の `>` が書く UTF-16 などを
    /// 「`.depup` がない」と同じに扱って、警告なしにルートだけを処理していた (#104)。
    fn load(path: &Path, base_dir: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("failed to read: {error}"))?;
        let content = String::from_utf8(bytes)
            .map_err(|_| "the file is not UTF-8 text; save it as UTF-8".to_string())?;
        Self::parse(&content, base_dir)
    }

    /// スキャン対象のディレクトリ一覧を構築する (ルートディレクトリを常に含む)
    ///
    /// ルートディレクトリが先頭に配置され、その後に `.depup` に記載されたディレクトリが
    /// ルート自身でない場合のみ追加される (重複を防ぐため)。
    pub fn directories_with_root(&self, root: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![root.to_path_buf()];
        for dir in &self.directories {
            if *dir != root {
                dirs.push(dir.clone());
            }
        }
        dirs
    }

    /// `.depup` ファイルの内容を解析する
    ///
    /// 各行は相対パスとして扱われる。`#` はコメント開始文字 (行頭・インライン)。
    /// 空行やコメントのみの行は無視される。
    /// 存在しないディレクトリは警告を出してスキップする。
    pub fn parse(content: &str, base_dir: &Path) -> Result<Self, String> {
        let mut directories = Vec::new();
        let canonical_base = base_dir.canonicalize().map_err(|error| {
            format!(
                "failed to resolve .depup base directory '{}': {}",
                base_dir.display(),
                error
            )
        })?;

        // 先頭の UTF-8 の BOM は文字コードの印で、1 行目のパスの一部ではない (#104)
        let (_, content) = crate::manifest::split_utf8_bom(content);
        for line in content.lines() {
            // インラインコメントを除去
            let stripped = match line.find('#') {
                Some(pos) => &line[..pos],
                None => line,
            };

            let trimmed = stripped.trim();
            if trimmed.is_empty() {
                continue;
            }

            let configured_path = Path::new(trimmed);
            if configured_path.is_absolute()
                || configured_path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
            {
                return Err(format!(
                    "directory '{}' must be a relative path below the .depup directory",
                    trimmed
                ));
            }

            let dir_path = base_dir.join(trimmed);
            if !dir_path.is_dir() {
                eprintln!(
                    "Warning: directory '{}' not found, skipping",
                    dir_path.display()
                );
                continue;
            }

            let canonical_dir = dir_path.canonicalize().map_err(|error| {
                format!(
                    "failed to resolve directory '{}': {}",
                    dir_path.display(),
                    error
                )
            })?;
            if !canonical_dir.starts_with(&canonical_base) {
                return Err(format!(
                    "directory '{}' resolves outside the .depup directory",
                    trimmed
                ));
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

    /// Issue #104: 先頭の UTF-8 の BOM を 1 行目のパスの一部として読まない。以前は
    /// `\u{feff}gui` を探して見つからず、そのディレクトリを処理から外していた。
    #[test]
    fn test_parse_skips_a_leading_utf8_bom() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("web")).unwrap();
        let config = DepupConfig::parse("\u{feff}gui\nweb\n", dir.path()).unwrap();
        assert_eq!(
            config.directories,
            [dir.path().join("gui"), dir.path().join("web")]
        );
    }

    /// Issue #104: UTF-8 として読めない `.depup` (PowerShell 5.1 の `>` が書く UTF-16 など) は
    /// `.depup` がないのと同じに扱わず、解析できない `.depup` と同じくエラーにする
    /// (呼び出し側が警告して無視する)。
    #[test]
    fn test_load_rejects_a_file_that_is_not_utf8() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        let path = dir.path().join(".depup");
        let utf16: Vec<u8> = [0xFEFF_u16]
            .into_iter()
            .chain("gui\r\n".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        fs::write(&path, utf16).unwrap();
        let error = DepupConfig::load(&path, dir.path()).unwrap_err();
        assert!(error.contains("not UTF-8"), "{error}");
        // UTF-8 なら同じ内容を読める
        fs::write(&path, "\u{feff}gui\r\n").unwrap();
        assert_eq!(
            DepupConfig::load(&path, dir.path()).unwrap().directories,
            [dir.path().join("gui")]
        );
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

        // 末尾スラッシュは Path::join が処理するため動作する
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
        assert_eq!(dirs[0], dir.path().to_path_buf(), "ルートが先頭であること");
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
        assert_eq!(dirs[0], dir.path().to_path_buf(), "ルートのみ");
    }

    #[test]
    fn test_parse_windows_line_endings() {
        // Windows 形式の改行コード (CRLF) でもパースできる
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("web")).unwrap();

        let content = "gui\r\nweb\r\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 2);
    }

    #[test]
    fn test_parse_whitespace_only_lines() {
        // 空白のみの行はスキップされる
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("app")).unwrap();

        let content = "   \n\t\n  app  \n  \n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 1);
        assert_eq!(config.directories[0], dir.path().join("app"));
    }

    #[test]
    fn test_parse_hash_in_directory_name() {
        // ディレクトリ名に # が含まれる場合、# 以降はコメントとして切り捨てられる
        // これは仕様通り（# はコメント開始文字）
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("app")).unwrap();

        let content = "app#comment\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        // "app#comment" は # でコメントが切り取られ "app" になる
        assert_eq!(config.directories.len(), 1);
        assert_eq!(config.directories[0], dir.path().join("app"));
    }

    #[test]
    fn test_parse_all_nonexistent_directories() {
        // 全てのディレクトリが存在しない場合、空のリストが返る
        let dir = create_test_dir();
        let content = "missing1\nmissing2\nmissing3\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert!(config.directories.is_empty());
    }

    #[test]
    fn test_parse_duplicate_directories() {
        // 同じディレクトリが複数回指定された場合、重複して含まれる
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("app")).unwrap();

        let content = "app\napp\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 2);
    }

    #[test]
    fn test_parse_nested_directories() {
        // ネストされたディレクトリパスもサポート
        let dir = create_test_dir();
        fs::create_dir_all(dir.path().join("packages/frontend")).unwrap();
        fs::create_dir_all(dir.path().join("packages/backend")).unwrap();

        let content = "packages/frontend\npackages/backend\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 2);
        assert_eq!(config.directories[0], dir.path().join("packages/frontend"));
        assert_eq!(config.directories[1], dir.path().join("packages/backend"));
    }

    #[test]
    fn test_parse_rejects_parent_directory_escape() {
        let parent = create_test_dir();
        let base = parent.path().join("project");
        fs::create_dir(&base).unwrap();
        fs::create_dir(parent.path().join("outside")).unwrap();

        let error = DepupConfig::parse("../outside\n", &base).unwrap_err();
        assert!(error.contains("relative path below"));
    }

    #[test]
    fn test_parse_rejects_absolute_directory() {
        let base = create_test_dir();
        let outside = create_test_dir();

        let error = DepupConfig::parse(&format!("{}\n", outside.path().display()), base.path())
            .unwrap_err();
        assert!(error.contains("relative path below"));
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_rejects_symlink_resolving_outside_base() {
        use std::os::unix::fs::symlink;

        let base = create_test_dir();
        let outside = create_test_dir();
        symlink(outside.path(), base.path().join("outside-link")).unwrap();

        let error = DepupConfig::parse("outside-link\n", base.path()).unwrap_err();
        assert!(error.contains("resolves outside"));
    }
}
