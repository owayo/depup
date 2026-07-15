//! 更新後の依存関係インストールのためのパッケージマネージャ連携
//!
//! このモジュールが提供する機能:
//! - インストール済みパッケージマネージャの検出
//! - 各言語のインストールコマンドの実行
//! - `--age` 指定時の transitive 依存 age 制約注入 (対応 PM のみ)

use crate::domain::Language;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

/// パッケージマネージャの実行ファイルを PATH から解決する。
///
/// Windows では `npm` / `pnpm` / `yarn` / `bun` / `composer` / `gradle` などが
/// `.cmd` / `.bat` シムとして配布される。`Command::new("pnpm")` は内部の
/// `CreateProcessW` 呼び出しで拡張子無しを `.exe` として解決しようとするため、
/// シムしか無い環境では `program not found` で失敗する (Issue #1)。
///
/// `which` クレートは PATHEXT を考慮して実体パス (例: `C:\...\pnpm.cmd`) を
/// 返すため、それを直接 `Command::new` に渡すことで `.cmd` / `.bat` シムを
/// 起動できる。フルパスの `.cmd` / `.bat` を `Command::new` に渡した場合は
/// Rust 標準ライブラリ (1.77+) の引数自動エスケープが効くため、
/// CVE-2024-24576 (BatBadBut) の影響も緩和される。
///
/// 解決に失敗した場合は元のプログラム名をそのまま返し、呼び出し側で
/// 従来どおりの `program not found` エラーを返せるようにする。
fn resolve_program(program: &str) -> PathBuf {
    which::which(program).unwrap_or_else(|_| PathBuf::from(program))
}

/// パッケージマネージャのインストール結果
#[derive(Debug, Clone)]
pub struct InstallResult {
    /// 使用された言語/パッケージマネージャ
    pub language: Language,
    /// 実行されたコマンド
    pub command: String,
    /// コマンドが成功したかどうか
    pub success: bool,
    /// コマンドの標準出力
    pub stdout: String,
    /// コマンドの標準エラー出力
    pub stderr: String,
}

impl InstallResult {
    /// 成功したインストール結果を作成する
    pub fn success(language: Language, command: String, stdout: String, stderr: String) -> Self {
        Self {
            language,
            command,
            success: true,
            stdout,
            stderr,
        }
    }

    /// 失敗したインストール結果を作成する
    pub fn failure(language: Language, command: String, stdout: String, stderr: String) -> Self {
        Self {
            language,
            command,
            success: false,
            stdout,
            stderr,
        }
    }

    /// スキップされた結果を作成する (パッケージマネージャが見つからない場合)
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

/// パッケージマネージャのインストールコマンドを実行するトレイト
pub trait PackageManagerRunner {
    /// 指定されたディレクトリで言語のインストールコマンドを実行する。
    ///
    /// `min_age` が `Some` の場合、対応する PM には transitive 依存にも age 制約が
    /// 効くようなオプションを注入する (例: pnpm の `--config.minimumReleaseAge`)。
    /// 非対応 PM では無視される。
    fn run_install(
        &self,
        language: Language,
        working_dir: &Path,
        min_age: Option<Duration>,
    ) -> InstallResult;
}

/// 実際のコマンドを実行するデフォルトのパッケージマネージャランナー
#[derive(Debug, Default)]
pub struct SystemPackageManager;

impl SystemPackageManager {
    /// 新しいシステムパッケージマネージャを作成する
    pub fn new() -> Self {
        Self
    }

    /// 使用する Node.js パッケージマネージャを検出する
    fn detect_node_pm(&self, working_dir: &Path) -> Option<&'static str> {
        // ロックファイルを優先順位に従ってチェック
        if working_dir.join("pnpm-lock.yaml").exists() {
            return Some("pnpm");
        }
        if working_dir.join("yarn.lock").exists() {
            return Some("yarn");
        }
        // bun.lock はテキスト形式 (Bun 1.2+ のデフォルト)、bun.lockb は旧バイナリ形式
        if working_dir.join("bun.lock").exists() || working_dir.join("bun.lockb").exists() {
            return Some("bun");
        }
        if working_dir.join("package-lock.json").exists() {
            return Some("npm");
        }
        // package.json があるがロックファイルがない場合は npm をデフォルトにする
        if working_dir.join("package.json").exists() {
            return Some("npm");
        }
        None
    }

    /// 使用する Python パッケージマネージャを検出する
    fn detect_python_pm(&self, working_dir: &Path) -> Option<&'static str> {
        // ロックファイル/設定を優先順位に従ってチェック
        if working_dir.join("uv.lock").exists() {
            return Some("uv");
        }
        if working_dir.join("poetry.lock").exists() {
            return Some("poetry");
        }
        // Rye が生成するロックファイルは requirements.lock / requirements-dev.lock
        // ("rye.lock" というファイルは存在しない)
        if working_dir.join("requirements.lock").exists()
            || working_dir.join("requirements-dev.lock").exists()
        {
            return Some("rye");
        }
        if working_dir.join("Pipfile.lock").exists() {
            return Some("pipenv");
        }
        // 特定のツール設定を持つ pyproject.toml をチェック
        if working_dir.join("pyproject.toml").exists() {
            // pyproject.toml がある場合は pip をデフォルトにする
            return Some("pip");
        }
        if working_dir.join("requirements.txt").exists() {
            return Some("pip");
        }
        None
    }

    /// Tauri プロジェクトかどうかを検出する (src-tauri/Cargo.toml が存在するか)
    fn detect_tauri_project(&self, working_dir: &Path) -> bool {
        working_dir.join("src-tauri/Cargo.toml").exists()
    }

    /// パッケージマネージャのインストールコマンドの引数を決定する。
    ///
    /// `min_age` が指定されていれば CLI フラグでネイティブに受ける PM のみここで追加する:
    /// - uv: `--exclude-newer <RFC3339 datetime>` (公式 CLI フラグ)
    ///
    /// pnpm は `minimumReleaseAge` 用の公式 CLI フラグが無い (pnpm v10.33 時点で
    /// 未実装: https://github.com/pnpm/pnpm/issues/11224) ため、引数ではなく
    /// 環境変数経由で指定する。`get_install_env` を参照。
    fn get_install_command_args(&self, pm: &str, min_age: Option<Duration>) -> Vec<String> {
        let base: Vec<&'static str> = self.get_install_command(pm);
        let mut out: Vec<String> = base.into_iter().map(String::from).collect();
        if let Some(age) = min_age
            && pm == "uv"
        {
            // uv: `--exclude-newer <RFC3339>` で指定日時以降にリリースされた
            // バージョンを resolve から除外する (transitive 含む)。
            if let Ok(chrono_dur) = chrono::Duration::from_std(age) {
                let cutoff = chrono::Utc::now() - chrono_dur;
                out.push("--exclude-newer".to_string());
                out.push(cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            }
        }
        out
    }

    /// パッケージマネージャ実行時に追加する環境変数を決定する。
    ///
    /// - **pnpm (v10.16+)**: `npm_config_minimum_release_age=<分>` で
    ///   `minimumReleaseAge` 設定を注入する。pnpm は npm の config 規約に
    ///   従うため、この env var は `.npmrc` の `minimum-release-age=<分>` と
    ///   等価になる。pnpm v10.16 未満ではこの env var は未知の設定として
    ///   無視される (graceful no-op)。
    /// - **uv (preview)**: `UV_MALWARE_CHECK=1` を常に注入し、`uv sync`
    ///   実行時に OSV の MAL advisories と locked resolution を照合する
    ///   ライトウェイトなマルウェアチェックを有効化する (Astral の preview
    ///   機能。`uv audit` 機能の一部)。未対応の uv バージョンではこの
    ///   env var は未知の設定として無視される (graceful no-op)。
    /// - 他 PM: 現状追加なし。
    fn get_install_env(&self, pm: &str, min_age: Option<Duration>) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(age) = min_age
            && pm == "pnpm"
        {
            let minutes = age.as_secs() / 60;
            env.push((
                "npm_config_minimum_release_age".to_string(),
                minutes.to_string(),
            ));
        }
        if pm == "uv" {
            env.push(("UV_MALWARE_CHECK".to_string(), "1".to_string()));
        }
        env
    }

    /// パッケージマネージャのインストールコマンドを取得する
    fn get_install_command(&self, pm: &str) -> Vec<&'static str> {
        match pm {
            // Node.js パッケージマネージャ
            "npm" => vec!["npm", "install"],
            "yarn" => vec!["yarn", "install"],
            "pnpm" => vec!["pnpm", "install"],
            "bun" => vec!["bun", "install"],
            // Python パッケージマネージャ
            "pip" => vec!["pip", "install", "-e", "."],
            "uv" => vec!["uv", "sync"],
            "poetry" => vec!["poetry", "install"],
            "rye" => vec!["rye", "sync"],
            "pipenv" => vec!["pipenv", "install"],
            // Rust の処理
            "cargo" => vec!["cargo", "update"],
            // Go
            "go" => vec!["go", "mod", "download"],
            // Ruby の処理
            "bundle" => vec!["bundle", "install"],
            // PHP の処理
            "composer" => vec!["composer", "install"],
            // Java/Gradle の処理
            "gradle" => vec!["gradle", "dependencies"],
            "./gradlew" => vec!["./gradlew", "dependencies"],
            // Swift の処理
            "swift" => vec!["swift", "package", "resolve"],
            _ => vec![],
        }
    }

    /// コマンドを実行して出力をキャプチャする。
    ///
    /// プログラム名 (`command[0]`) は `resolve_program` で PATH 解決してから
    /// `Command::new` に渡す。Windows で `.cmd` / `.bat` シムしか無い PM
    /// (`pnpm`, `composer.bat`, `gradle.bat` 等) を起動するための対応 (Issue #1)。
    fn run_command(
        &self,
        command: &[&str],
        working_dir: &Path,
        env: &[(String, String)],
    ) -> std::io::Result<Output> {
        if command.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Empty command",
            ));
        }

        let program = resolve_program(command[0]);
        let mut cmd = Command::new(&program);
        cmd.args(&command[1..]).current_dir(working_dir);
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.output()
    }
}

impl PackageManagerRunner for SystemPackageManager {
    fn run_install(
        &self,
        language: Language,
        working_dir: &Path,
        min_age: Option<Duration>,
    ) -> InstallResult {
        // Tauri プロジェクトの Rust では src-tauri ディレクトリを使用
        let (effective_dir, pm) = match language {
            Language::Node => (working_dir.to_path_buf(), self.detect_node_pm(working_dir)),
            Language::Python => (
                working_dir.to_path_buf(),
                self.detect_python_pm(working_dir),
            ),
            Language::Rust => {
                // まず Tauri プロジェクトかチェック
                if self.detect_tauri_project(working_dir) {
                    (working_dir.join("src-tauri"), Some("cargo"))
                } else if working_dir.join("Cargo.toml").exists() {
                    (working_dir.to_path_buf(), Some("cargo"))
                } else {
                    (working_dir.to_path_buf(), None)
                }
            }
            Language::Go => {
                let pm = if working_dir.join("go.mod").exists() {
                    Some("go")
                } else {
                    None
                };
                (working_dir.to_path_buf(), pm)
            }
            Language::Ruby => {
                let pm = if working_dir.join("Gemfile").exists() {
                    Some("bundle")
                } else {
                    None
                };
                (working_dir.to_path_buf(), pm)
            }
            Language::Php => {
                let pm = if working_dir.join("composer.json").exists() {
                    Some("composer")
                } else {
                    None
                };
                (working_dir.to_path_buf(), pm)
            }
            Language::Java => {
                // gradlew が利用可能ならそちらを優先し、なければ gradle にフォールバック
                let pm = if working_dir.join("gradlew").exists() {
                    Some("./gradlew")
                } else if working_dir.join("build.gradle").exists()
                    || working_dir.join("build.gradle.kts").exists()
                {
                    Some("gradle")
                } else {
                    None
                };
                (working_dir.to_path_buf(), pm)
            }
            Language::Swift => {
                let pm = if working_dir.join("Package.swift").exists() {
                    Some("swift")
                } else {
                    None
                };
                (working_dir.to_path_buf(), pm)
            }
        };

        let Some(pm) = pm else {
            return InstallResult::skipped(language);
        };

        let command_parts = self.get_install_command_args(pm, min_age);
        if command_parts.is_empty() {
            return InstallResult::skipped(language);
        }

        let env = self.get_install_env(pm, min_age);
        let command_refs: Vec<&str> = command_parts.iter().map(|s| s.as_str()).collect();

        // 表示用コマンド文字列: env var 付与時は `KEY=VALUE cmd args...` で表現
        let command_str = if env.is_empty() {
            command_parts.join(" ")
        } else {
            let env_str: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            format!("{} {}", env_str.join(" "), command_parts.join(" "))
        };

        match self.run_command(&command_refs, &effective_dir, &env) {
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

/// 指定された全言語のインストールコマンドを実行する
pub fn run_installs<R: PackageManagerRunner>(
    runner: &R,
    languages: &[Language],
    working_dir: &Path,
    min_age: Option<Duration>,
) -> Vec<InstallResult> {
    languages
        .iter()
        .map(|lang| runner.run_install(*lang, working_dir, min_age))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のモックパッケージマネージャランナー
    struct MockPackageManager {
        should_succeed: bool,
    }

    impl MockPackageManager {
        fn new(should_succeed: bool) -> Self {
            Self { should_succeed }
        }
    }

    impl PackageManagerRunner for MockPackageManager {
        fn run_install(
            &self,
            language: Language,
            _working_dir: &Path,
            _min_age: Option<Duration>,
        ) -> InstallResult {
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
        let result = runner.run_install(Language::Node, Path::new("."), None);
        assert!(result.success);
    }

    #[test]
    fn test_mock_package_manager_failure() {
        let runner = MockPackageManager::new(false);
        let result = runner.run_install(Language::Node, Path::new("."), None);
        assert!(!result.success);
    }

    #[test]
    fn test_run_installs() {
        let runner = MockPackageManager::new(true);
        let languages = vec![Language::Node, Language::Python];
        let results = run_installs(&runner, &languages, Path::new("."), None);

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
    }

    #[test]
    fn test_system_package_manager_new() {
        let _pm = SystemPackageManager::new();
        // パニックせずに作成できることを確認
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
        assert_eq!(cmd, vec!["cargo", "update"]);
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
    fn test_get_install_command_gradle() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("gradle");
        assert_eq!(cmd, vec!["gradle", "dependencies"]);
    }

    #[test]
    fn test_get_install_command_gradlew() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("./gradlew");
        assert_eq!(cmd, vec!["./gradlew", "dependencies"]);
    }

    #[test]
    fn test_get_install_command_unknown() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("unknown");
        assert!(cmd.is_empty());
    }

    #[test]
    fn test_get_install_command_args_pnpm_without_age() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command_args("pnpm", None);
        assert_eq!(cmd, vec!["pnpm".to_string(), "install".to_string()]);
    }

    #[test]
    fn test_get_install_command_args_pnpm_age_does_not_inject_cli_flag() {
        // pnpm は CLI フラグでの minimumReleaseAge をサポートしないため、
        // age 指定時でも CLI 引数は変化しない (env 変数経由で渡す)。
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let cmd = pm.get_install_command_args("pnpm", Some(age));
        assert_eq!(cmd, vec!["pnpm".to_string(), "install".to_string()]);
    }

    #[test]
    fn test_get_install_env_pnpm_with_age() {
        // pnpm への age 指定は npm_config_minimum_release_age env 変数で渡す。
        // 2 週間 = 20160 分。
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let env = pm.get_install_env("pnpm", Some(age));
        assert_eq!(
            env,
            vec![(
                "npm_config_minimum_release_age".to_string(),
                "20160".to_string()
            )]
        );
    }

    #[test]
    fn test_get_install_env_pnpm_without_age() {
        let pm = SystemPackageManager::new();
        let env = pm.get_install_env("pnpm", None);
        assert!(env.is_empty());
    }

    #[test]
    fn test_get_install_env_npm_does_not_set_age() {
        // npm 本体は別の age 機能 (`--min-release-age`) を使うため、ここでは設定しない。
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let env = pm.get_install_env("npm", Some(age));
        assert!(env.is_empty());
    }

    #[test]
    fn test_get_install_env_uv_does_not_set_age_via_env() {
        // uv は CLI フラグで age を受けるため age 関連の env 変数は設定しない。
        // ただし `UV_MALWARE_CHECK=1` は age とは独立に常時付与されるため、
        // env 全体が空にはならない。age 関連 env のみが含まれないことを確認する。
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let env = pm.get_install_env("uv", Some(age));
        assert!(
            env.iter()
                .all(|(k, _)| k != "npm_config_minimum_release_age"),
            "uv に age を渡しても pnpm 用の env が混入してはならない: {:?}",
            env
        );
    }

    #[test]
    fn test_get_install_env_uv_sets_malware_check_without_age() {
        // age 未指定でも `uv sync` 実行時の OSV マルウェアチェックを常時有効化する。
        let pm = SystemPackageManager::new();
        let env = pm.get_install_env("uv", None);
        assert!(
            env.contains(&("UV_MALWARE_CHECK".to_string(), "1".to_string())),
            "uv では UV_MALWARE_CHECK=1 が常時付与されるべき: {:?}",
            env
        );
    }

    #[test]
    fn test_get_install_env_uv_sets_malware_check_with_age() {
        // age 指定時でも UV_MALWARE_CHECK=1 は付与される。
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let env = pm.get_install_env("uv", Some(age));
        assert!(
            env.contains(&("UV_MALWARE_CHECK".to_string(), "1".to_string())),
            "uv では age 指定の有無に関わらず UV_MALWARE_CHECK=1 が付与されるべき: {:?}",
            env
        );
    }

    #[test]
    fn test_get_install_env_non_uv_does_not_set_malware_check() {
        // uv 以外の PM には UV_MALWARE_CHECK を漏らさない。
        let pm = SystemPackageManager::new();
        for other in &["pip", "poetry", "rye", "pipenv", "npm", "pnpm", "cargo"] {
            let env = pm.get_install_env(other, None);
            assert!(
                env.iter().all(|(k, _)| k != "UV_MALWARE_CHECK"),
                "{} に UV_MALWARE_CHECK は付与してはならない: {:?}",
                other,
                env
            );
        }
    }

    #[test]
    fn test_get_install_command_args_uv_with_age_has_exclude_newer() {
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let cmd = pm.get_install_command_args("uv", Some(age));
        assert!(cmd.contains(&"--exclude-newer".to_string()));
        // RFC3339 形式: 末尾に `Z` が付く
        assert!(cmd.last().unwrap().ends_with('Z'));
    }

    #[test]
    fn test_get_install_command_args_npm_ignores_age() {
        // npm はネイティブ age サポートがないため CLI 引数は変化しない
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let cmd = pm.get_install_command_args("npm", Some(age));
        assert_eq!(cmd, vec!["npm".to_string(), "install".to_string()]);
    }

    #[test]
    fn test_get_install_command_args_cargo_ignores_age() {
        // cargo はネイティブ age サポートがない (post-install audit で対応)
        let pm = SystemPackageManager::new();
        let age = Duration::from_secs(14 * 24 * 60 * 60);
        let cmd = pm.get_install_command_args("cargo", Some(age));
        assert_eq!(cmd, vec!["cargo".to_string(), "update".to_string()]);
    }

    #[test]
    fn test_detect_node_pm_npm() {
        // package-lock.json がある一時ディレクトリを作成
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
    fn test_detect_node_pm_bun() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("bun.lockb"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), Some("bun"));
    }

    #[test]
    fn test_detect_node_pm_bun_text_lockfile() {
        // Bun 1.2+ のデフォルトであるテキスト形式 bun.lock を検出する
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(temp_dir.path().join("bun.lock"), "{}").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_node_pm(temp_dir.path()), Some("bun"));
    }

    #[test]
    fn test_detect_python_pm_rye_requirements_lock() {
        // Rye は rye.lock ではなく requirements.lock を生成する
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("pyproject.toml"), "").unwrap();
        std::fs::write(temp_dir.path().join("requirements.lock"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("rye"));
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
    fn test_resolve_program_existing_command_returns_absolute_path() {
        // `cargo` は cargo test を実行している時点で PATH 上に存在する前提。
        // which 経由で絶対パスに解決され、`Command::new(full_path)` で起動できる。
        // Windows ではこれが `.cmd`/`.bat` シムのフルパス、Unix では実体 (`.../cargo`)。
        let resolved = resolve_program("cargo");
        assert!(
            resolved.is_absolute(),
            "PATH 上に存在する cargo は which で絶対パスに解決されるべき: {:?}",
            resolved
        );
    }

    #[test]
    fn test_resolve_program_nonexistent_command_falls_back_to_name() {
        // 存在しないコマンドは which が失敗するため、元の program 名を
        // そのまま PathBuf として返し、後段の Command::new で従来通り
        // `program not found` 系の `io::ErrorKind::NotFound` が返るようにする。
        let bogus = "__depup_definitely_nonexistent_command_xyz__";
        let resolved = resolve_program(bogus);
        assert_eq!(
            resolved,
            PathBuf::from(bogus),
            "解決失敗時は元の program 名をそのまま返してフォールバックさせるべき"
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_resolve_program_finds_cmd_shim_on_windows() {
        // Windows で `.cmd` シムのみ存在するパッケージマネージャ (pnpm.cmd 等)
        // を which が PATHEXT 経由で解決できることを確認する。
        // tempdir に `pnpm_test_shim.cmd` を置き、PATH の先頭にそのディレクトリを
        // 追加した状態で which を呼ぶ。
        let temp_dir = tempfile::tempdir().unwrap();
        let shim_path = temp_dir.path().join("pnpm_test_shim.cmd");
        std::fs::write(&shim_path, "@echo off\r\necho ok\r\n").unwrap();

        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let mut new_paths = vec![temp_dir.path().to_path_buf()];
        new_paths.extend(std::env::split_paths(&original_path));
        let joined = std::env::join_paths(new_paths).unwrap();

        // SAFETY: テストはシングルスレッドで PATH を一時的に書き換える。
        // テスト終了前に元に戻す。
        unsafe { std::env::set_var("PATH", &joined) };
        let resolved = resolve_program("pnpm_test_shim");
        unsafe { std::env::set_var("PATH", &original_path) };

        assert!(
            resolved.is_absolute(),
            "Windows では .cmd シムが which で絶対パスに解決されるべき: {:?}",
            resolved
        );
        assert_eq!(
            resolved.extension().and_then(|s| s.to_str()),
            Some("cmd"),
            "解決結果は .cmd 拡張子を持つはず: {:?}",
            resolved
        );
    }

    #[test]
    fn test_run_install_skipped_no_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pm = SystemPackageManager::new();

        let result = pm.run_install(Language::Node, temp_dir.path(), None);
        assert!(result.success);
        assert!(result.command.is_empty());
    }

    #[test]
    fn test_detect_tauri_project_true() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp_dir.path().join("src-tauri")).unwrap();
        std::fs::write(temp_dir.path().join("src-tauri/Cargo.toml"), "[package]").unwrap();

        let pm = SystemPackageManager::new();
        assert!(pm.detect_tauri_project(temp_dir.path()));
    }

    #[test]
    fn test_detect_tauri_project_false() {
        let temp_dir = tempfile::tempdir().unwrap();

        let pm = SystemPackageManager::new();
        assert!(!pm.detect_tauri_project(temp_dir.path()));
    }

    #[test]
    fn test_detect_tauri_project_no_cargo_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp_dir.path().join("src-tauri")).unwrap();

        let pm = SystemPackageManager::new();
        assert!(!pm.detect_tauri_project(temp_dir.path()));
    }

    #[test]
    fn test_get_install_command_bun() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("bun");
        assert_eq!(cmd, vec!["bun", "install"]);
    }

    #[test]
    fn test_get_install_command_bundle() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("bundle");
        assert_eq!(cmd, vec!["bundle", "install"]);
    }

    #[test]
    fn test_get_install_command_composer() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("composer");
        assert_eq!(cmd, vec!["composer", "install"]);
    }

    #[test]
    fn test_get_install_command_rye() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("rye");
        assert_eq!(cmd, vec!["rye", "sync"]);
    }

    #[test]
    fn test_get_install_command_pipenv() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("pipenv");
        assert_eq!(cmd, vec!["pipenv", "install"]);
    }

    #[test]
    fn test_get_install_command_pip() {
        let pm = SystemPackageManager::new();
        let cmd = pm.get_install_command("pip");
        assert_eq!(cmd, vec!["pip", "install", "-e", "."]);
    }

    #[test]
    fn test_detect_python_pm_rye() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("requirements-dev.lock"), "").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("rye"));
    }

    #[test]
    fn test_detect_python_pm_pipenv() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("Pipfile.lock"), "{}").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("pipenv"));
    }

    #[test]
    fn test_detect_python_pm_requirements_txt() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("requirements.txt"), "requests>=2.0").unwrap();

        let pm = SystemPackageManager::new();
        assert_eq!(pm.detect_python_pm(temp_dir.path()), Some("pip"));
    }

    #[test]
    fn test_run_install_ruby_skipped_no_gemfile() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pm = SystemPackageManager::new();

        let result = pm.run_install(Language::Ruby, temp_dir.path(), None);
        assert!(result.success);
        assert!(result.command.is_empty()); // スキップ
    }

    #[test]
    fn test_run_install_php_skipped_no_composer() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pm = SystemPackageManager::new();

        let result = pm.run_install(Language::Php, temp_dir.path(), None);
        assert!(result.success);
        assert!(result.command.is_empty()); // スキップ
    }

    #[test]
    fn test_run_install_java_skipped_no_gradle() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pm = SystemPackageManager::new();

        let result = pm.run_install(Language::Java, temp_dir.path(), None);
        assert!(result.success);
        assert!(result.command.is_empty()); // スキップ
    }

    #[test]
    fn test_run_install_go_skipped_no_gomod() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pm = SystemPackageManager::new();

        let result = pm.run_install(Language::Go, temp_dir.path(), None);
        assert!(result.success);
        assert!(result.command.is_empty()); // スキップ
    }
}
