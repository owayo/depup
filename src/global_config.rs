//! グローバル設定ファイル (`~/.config/depup/config.toml`) のローダ
//!
//! ユーザー単位のデフォルトを定義する。現状は `age` と `osv` をサポート。
//! ファイルが無い場合は初回読み込み時にコメント付きの雛形を自動生成する
//! (生成に失敗してもツールの動作は続行され、組み込みデフォルトが使われる)。

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::parse_duration;
use crate::domain::ChangeLevel;

/// 組み込みデフォルトの age (1週間)。
///
/// グローバル設定ファイルが存在しないとき、または存在しても `age` が
/// 未指定のときに使用される。
pub const DEFAULT_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 組み込みデフォルトの OSV 脆弱性チェック (有効)。
///
/// グローバル設定ファイルが存在しないとき、または存在しても `osv` が
/// 未指定のときに使用される。`--no-osv` またはグローバル設定の
/// `osv = false` で無効化できる。
pub const DEFAULT_OSV: bool = true;

/// 初回起動時に書き出されるデフォルト設定の TOML 内容。
///
/// 組み込みデフォルト (age=1w, osv=true) と一致するキーを生成する。
/// オプトアウト項目 (max_change) はコメントアウトしておき、ユーザーが
/// 必要時にアンコメントできるようにする。
pub const DEFAULT_CONFIG_CONTENT: &str = r#"# depup global configuration
# https://github.com/owayo/depup
#
# This file is auto-generated on first run.
# Edit values below to override depup's built-in defaults.

# Default age filter applied to every depup run.
# Accepts the same format as --age: Nd (days), Nw (weeks), Nm (months).
# Override per-run with --age <DURATION> or disable with --no-age.
age = "1w"

# Check candidate versions against the OSV.dev vulnerability database
# and skip versions with known vulnerabilities (enabled by default).
# Requires network access; on API errors depup keeps the original candidate.
# Override per-run with --osv / --no-osv.
osv = true

# Limit the maximum allowed version change.
# Accepts: "patch" (allow only patch bumps), "minor" (allow patch + minor),
# or "major" (default — all bumps allowed).
# Override per-run with --max-change <LEVEL>.
# max_change = "minor"
"#;

/// `~/.config/depup/config.toml` の内容。
///
/// 全フィールドはオプショナル。未指定時は組み込みデフォルトが使われる。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GlobalConfig {
    /// 経過時間フィルタのデフォルト (例: `"1w"`, `"10d"`, `"2m"`)。
    /// 未指定の場合は組み込みデフォルト [`DEFAULT_AGE`] が使われる。
    #[serde(default)]
    pub age: Option<String>,

    /// OSV.dev による脆弱性チェックをデフォルトで有効にするか。
    /// 未指定の場合は組み込みデフォルト (`true` = OSV チェック有効) が使われる。
    /// 明示的に `false` を書けば無効化できる。
    #[serde(default)]
    pub osv: Option<bool>,

    /// 許容する変更レベルの上限 (`"patch"` / `"minor"` / `"major"`)。
    /// 未指定の場合は制限なし (= major bumps も許可)。
    #[serde(default)]
    pub max_change: Option<String>,
}

impl GlobalConfig {
    /// 既定の設定ファイルパス: `~/.config/depup/config.toml`。
    ///
    /// クロスプラットフォームでの一貫性のため、macOS でも `~/.config/...` を
    /// 使用する (`dirs::config_dir()` ではない)。
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("depup")
            .join("config.toml")
    }

    /// 既定パスから設定を読み込む。
    ///
    /// ファイルが存在しなければコメント付きの雛形を自動生成してから読み込む
    /// (生成に失敗した場合は警告を出して `None`)。
    /// パース失敗時も警告を出して `None` を返す。
    pub fn load() -> Option<Self> {
        Self::load_from(&Self::default_path())
    }

    /// 指定パスから設定を読み込む。存在しなければ自動生成する。
    pub fn load_from(path: &Path) -> Option<Self> {
        if !path.exists()
            && let Err(e) = generate_default_at(path)
        {
            eprintln!(
                "Warning: failed to create default global config at '{}': {}",
                path.display(),
                e
            );
            return None;
        }

        let content = std::fs::read_to_string(path).ok()?;
        match toml::from_str::<GlobalConfig>(&content) {
            Ok(config) => Some(config),
            Err(e) => {
                eprintln!(
                    "Warning: failed to parse global config '{}': {}",
                    path.display(),
                    e
                );
                None
            }
        }
    }

    /// 設定の `age` を [`Duration`] に変換する。
    ///
    /// 未指定 or パース失敗の場合は `None`。パース失敗時は警告を出す。
    pub fn age_duration(&self) -> Option<Duration> {
        let raw = self.age.as_deref()?;
        match parse_duration(raw) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!(
                    "Warning: invalid 'age' value in global config: {} ({})",
                    raw, e
                );
                None
            }
        }
    }

    /// 設定の `max_change` を [`ChangeLevel`] に変換する。
    ///
    /// 未指定 or パース失敗の場合は `None`。パース失敗時は警告を出す。
    pub fn max_change_level(&self) -> Option<ChangeLevel> {
        let raw = self.max_change.as_deref()?;
        match ChangeLevel::parse(raw) {
            Ok(level) => Some(level),
            Err(e) => {
                eprintln!(
                    "Warning: invalid 'max_change' value in global config: {} ({})",
                    raw, e
                );
                None
            }
        }
    }
}

/// 指定パスにデフォルト設定の雛形を書き出す。
///
/// 親ディレクトリが無ければ作成する。既に同名ファイルが存在する場合は
/// 上書きしない (呼び出し側で `path.exists()` を確認すること)。
pub fn generate_default_at(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_CONFIG_CONTENT)
}

/// グローバル設定と CLI フラグから、許容する変更レベルの上限を決定する。
///
/// 優先順位 (高い順):
/// 1. `--max-change <LEVEL>` が指定されていればその値
/// 2. グローバル設定の `max_change` がパースできればその値
/// 3. それ以外は `None` (= 制限なし、major bumps も許可)
pub fn resolve_max_change(
    cli: Option<ChangeLevel>,
    config: Option<&GlobalConfig>,
) -> Option<ChangeLevel> {
    if let Some(level) = cli {
        return Some(level);
    }
    config.and_then(|cfg| cfg.max_change_level())
}

/// グローバル設定と CLI フラグから、OSV チェックを有効にするか決定する。
///
/// 優先順位 (高い順):
/// 1. `--no-osv` が指定されていれば `false`
/// 2. `--osv` が指定されていれば `true`
/// 3. グローバル設定の `osv` が指定されていればその値
/// 4. それ以外は `true` (組み込みデフォルト = OSV チェック有効)
pub fn resolve_osv(cli_osv: bool, no_osv: bool, config: Option<&GlobalConfig>) -> bool {
    if no_osv {
        return false;
    }
    if cli_osv {
        return true;
    }
    if let Some(cfg) = config
        && let Some(v) = cfg.osv
    {
        return v;
    }
    DEFAULT_OSV
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_path_under_home_dot_config() {
        let path = GlobalConfig::default_path();
        let s = path.to_string_lossy();
        assert!(s.contains(".config"), "path should contain .config: {}", s);
        assert!(s.ends_with("depup/config.toml"), "unexpected path: {}", s);
    }

    #[test]
    fn test_load_from_missing_file_auto_generates() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nope.toml");
        assert!(!path.exists());

        let cfg = GlobalConfig::load_from(&path).expect("missing file should be auto-created");
        // 雛形のデフォルトでは age=1w / osv=true が書かれる
        assert_eq!(cfg.age.as_deref(), Some("1w"));
        assert_eq!(cfg.osv, Some(true));
        assert!(path.exists(), "file should be created");
    }

    #[test]
    fn test_load_from_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("dir").join("config.toml");
        assert!(!path.parent().unwrap().exists());

        let _ = GlobalConfig::load_from(&path).expect("nested path should be created");
        assert!(path.exists());
    }

    #[test]
    fn test_load_from_does_not_overwrite_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "age = \"3w\"\n").unwrap();

        let cfg = GlobalConfig::load_from(&path).unwrap();
        assert_eq!(cfg.age.as_deref(), Some("3w"));

        // 中身が上書きされていないことを確認
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "age = \"3w\"\n");
    }

    #[test]
    fn test_generate_default_at_writes_template() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        generate_default_at(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("depup global configuration"));
        assert!(content.contains("age = \"1w\""));
        assert!(content.contains("\nosv = true"));
    }

    #[test]
    fn test_default_template_parses_cleanly() {
        // 自動生成テンプレートが GlobalConfig としてパース可能であることを保証
        let cfg: GlobalConfig = toml::from_str(DEFAULT_CONFIG_CONTENT).unwrap();
        assert_eq!(cfg.age.as_deref(), Some("1w"));
        assert_eq!(
            cfg.osv,
            Some(DEFAULT_OSV),
            "雛形の osv は組み込みデフォルトと一致する"
        );
    }

    #[test]
    fn test_load_from_empty_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        let cfg = GlobalConfig::load_from(&path).expect("empty file should parse as default");
        assert!(cfg.age.is_none());
    }

    #[test]
    fn test_load_from_valid_age() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "age = \"2w\"\n").unwrap();
        let cfg = GlobalConfig::load_from(&path).unwrap();
        assert_eq!(cfg.age.as_deref(), Some("2w"));
        assert_eq!(
            cfg.age_duration(),
            Some(Duration::from_secs(14 * 24 * 60 * 60))
        );
    }

    #[test]
    fn test_load_from_invalid_toml_returns_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "age = =[[[invalid").unwrap();
        assert!(GlobalConfig::load_from(&path).is_none());
    }

    #[test]
    fn test_age_duration_invalid_value_returns_none() {
        let cfg = GlobalConfig {
            age: Some("nonsense".to_string()),
            ..Default::default()
        };
        assert!(cfg.age_duration().is_none());
    }

    #[test]
    fn test_age_duration_none_when_unset() {
        let cfg = GlobalConfig::default();
        assert!(cfg.age_duration().is_none());
    }

    #[test]
    fn test_default_age_is_one_week() {
        assert_eq!(DEFAULT_AGE, Duration::from_secs(7 * 24 * 60 * 60));
    }

    #[test]
    fn test_resolve_osv_no_osv_wins() {
        let cfg = GlobalConfig {
            osv: Some(true),
            ..Default::default()
        };
        assert!(!resolve_osv(true, true, Some(&cfg)));
    }

    #[test]
    fn test_resolve_osv_cli_wins_over_config() {
        let cfg = GlobalConfig {
            osv: Some(false),
            ..Default::default()
        };
        assert!(resolve_osv(true, false, Some(&cfg)));
    }

    #[test]
    fn test_resolve_osv_config_when_cli_absent() {
        let cfg = GlobalConfig {
            osv: Some(true),
            ..Default::default()
        };
        assert!(resolve_osv(false, false, Some(&cfg)));
    }

    #[test]
    fn test_resolve_osv_config_false_disables_default() {
        // 組み込みデフォルトが true でも、設定の明示的な false は尊重される
        let cfg = GlobalConfig {
            osv: Some(false),
            ..Default::default()
        };
        assert!(!resolve_osv(false, false, Some(&cfg)));
    }

    #[test]
    fn test_resolve_osv_no_osv_wins_over_default() {
        // 設定なし (= 組み込みデフォルト true) でも --no-osv で無効化できる
        assert!(!resolve_osv(false, true, None));
    }

    #[test]
    fn test_resolve_osv_default_true() {
        assert!(resolve_osv(false, false, None));
        let cfg = GlobalConfig::default();
        assert!(resolve_osv(false, false, Some(&cfg)));
    }

    #[test]
    fn test_load_from_with_both_age_and_osv() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "age = \"2w\"\nosv = true\n").unwrap();
        let cfg = GlobalConfig::load_from(&path).unwrap();
        assert_eq!(cfg.age.as_deref(), Some("2w"));
        assert_eq!(cfg.osv, Some(true));
    }
}
