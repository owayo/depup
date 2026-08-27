//! `mise ls-remote` を実行してツールの利用可能バージョンを取得する
//!
//! mise は core / aqua / ubi / asdf / npm / cargo / go / gem / pipx など多数の
//! バックエンドを束ねており、それぞれ配布元も API も違う。depup が各バックエンドの
//! レジストリを直接叩くのは非現実的なので、mise 自身に問い合わせる。
//!
//! ```text
//! $ mise ls-remote node --json --minimum-release-age 0
//! [{"version":"26.8.1","created_at":"2026-08-26T13:05:28.0Z"}, ...]
//! ```
//!
//! `--minimum-release-age 0` は必須。mise 側の `minimum_release_age` (既定 24h) が
//! 効いたままだと新しい版が最初から隠れてしまい、depup の age 判定
//! (`--age` / プロジェクト設定 / 既定 1w) と二重にフィルタが掛かる。
//! age の適用は depup 側に一本化する。

use super::RegistryAdapter;
use crate::domain::Language;
use crate::error::RegistryError;
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;

/// レジストリ表示名
const REGISTRY_NAME: &str = "mise";

/// `mise ls-remote` 実行全体のタイムアウト。
///
/// `mise ls-remote java` は 3000 件超を返し、初回はバックエンドへの
/// ネットワークアクセスを伴う。mise 自身が結果をキャッシュする
/// (`fetch_remote_versions_cache`、既定 1h) ため 2 回目以降は即座に返る。
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(60);

/// `mise ls-remote --json` の 1 要素。
///
/// `created_at` / `prerelease` / `release_url` はバックエンド依存で欠落しうるため
/// すべて optional として読む。
#[derive(Debug, Deserialize)]
struct MiseRemoteVersion {
    version: String,
    #[serde(default)]
    created_at: Option<String>,
}

/// mise ツールのバージョン取得アダプタ
pub struct MiseAdapter {
    /// 実行する mise のプログラム名 (テストでは差し替える)
    program: String,
}

impl Default for MiseAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MiseAdapter {
    /// 新しい `MiseAdapter` を作る
    pub fn new() -> Self {
        Self {
            program: "mise".to_string(),
        }
    }

    /// 実行するプログラムを差し替える (テスト用)
    #[cfg(test)]
    fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// `mise` コマンドが PATH 上にあるかを返す。
    ///
    /// mise のバージョン解決は `mise ls-remote` に完全に依存するため、
    /// 未インストールの環境では依存ごとに同じ fetch エラーが並ぶ。
    /// 呼び出し側はこの判定でマニフェストごとスキップし、警告を 1 回に留める。
    pub fn is_available() -> bool {
        which::which("mise").is_ok()
    }
}

/// ツール名として `mise ls-remote` に渡してよい文字列かを検証する。
///
/// 引数は配列で渡す (シェルを経由しない) ため、混入しても任意コマンド実行には
/// ならないが、`--` 始まりの名前はオプションと解釈されて意図しないフラグが立つ。
/// バックエンド接頭辞付きの名前 (`npm:@scope/pkg` / `ubi:owner/repo` /
/// `go:github.com/x/y` / `cargo:ripgrep`) を通すため、記号は控えめに許可する。
fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        // `@` はバージョン指定の区切り (`node@20`) なので、ツール名側には
        // scope 付き npm パッケージ (`npm:@scope/pkg`) の先頭にしか現れない
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | ':' | '@' | '+')
        })
}

/// mise が返す `created_at` を `DateTime<Utc>` に変換する。
///
/// バックエンドによって表記が揺れる:
/// - `2026-08-26T13:05:28.0Z` (RFC 3339)
/// - `2025-03-28T22:04:28.484345` (タイムゾーンなし。UTC とみなす)
///
/// どちらでも読めない場合や欠落時は `UNIX_EPOCH` を返す。「十分に古い」扱いに
/// なるため、公開日を取得できないバックエンドの版が age フィルタで
/// 永久に除外されることはない (GitHub Tags アダプタと同じ方針)。
fn parse_created_at(raw: Option<&str>) -> DateTime<Utc> {
    let epoch = Utc.timestamp_opt(0, 0).single().unwrap_or_default();
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return epoch;
    };

    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return parsed.with_timezone(&Utc);
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Utc.from_utc_datetime(&naive);
        }
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap_or_default());
    }
    epoch
}

/// `mise ls-remote --json` の出力を `VersionInfo` に変換する
fn parse_ls_remote_json(package: &str, stdout: &str) -> Result<Vec<VersionInfo>, RegistryError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let entries: Vec<MiseRemoteVersion> =
        serde_json::from_str(trimmed).map_err(|e| RegistryError::InvalidResponse {
            package: package.to_string(),
            registry: REGISTRY_NAME.to_string(),
            message: e.to_string(),
        })?;

    Ok(entries
        .into_iter()
        .filter(|entry| !entry.version.trim().is_empty())
        .map(|entry| {
            VersionInfo::new(
                entry.version.trim().to_string(),
                parse_created_at(entry.created_at.as_deref()),
            )
        })
        .collect())
}

#[async_trait]
impl RegistryAdapter for MiseAdapter {
    fn language(&self) -> Language {
        Language::Mise
    }

    fn registry_name(&self) -> &'static str {
        REGISTRY_NAME
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        if !is_valid_tool_name(package) {
            return Err(RegistryError::InvalidPackageName {
                name: package.to_string(),
                registry: REGISTRY_NAME.to_string(),
                reason: "tool name contains characters that are not allowed".to_string(),
            });
        }

        let output_future = Command::new(&self.program)
            .arg("ls-remote")
            .arg("--json")
            // mise 側の minimum_release_age を無効化し、age 判定は depup に一本化する。
            // フラグと env の両方で無効化しておく (どちらか一方しか効かない
            // バージョンでも取りこぼさないための二重防御)。
            .arg("--minimum-release-age")
            .arg("0")
            // 以降を位置引数として扱わせ、`-` 始まりのツール名がオプションに
            // 化けるのを防ぐ
            .arg("--")
            .arg(package)
            .env("MISE_MINIMUM_RELEASE_AGE", "0")
            // 対話プロンプト (未信頼 config の確認など) で止まらないようにする
            .env("MISE_YES", "1")
            .env("MISE_QUIET", "1")
            .kill_on_drop(true)
            .output();

        let output = match tokio::time::timeout(LS_REMOTE_TIMEOUT, output_future).await {
            Ok(result) => result.map_err(|e| RegistryError::NetworkError {
                package: package.to_string(),
                registry: REGISTRY_NAME.to_string(),
                message: format!("failed to run `{} ls-remote`: {}", self.program, e),
            })?,
            Err(_) => {
                return Err(RegistryError::Timeout {
                    package: package.to_string(),
                    registry: REGISTRY_NAME.to_string(),
                });
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = stderr.trim();
            // mise のレジストリに無いツールは "not found in mise tool registry"
            if message.contains("not found in mise tool registry") {
                return Err(RegistryError::PackageNotFound {
                    package: package.to_string(),
                    registry: REGISTRY_NAME.to_string(),
                });
            }
            return Err(RegistryError::NetworkError {
                package: package.to_string(),
                registry: REGISTRY_NAME.to_string(),
                message: format!(
                    "`{} ls-remote {}` exited with {}: {}",
                    self.program,
                    package,
                    output.status,
                    first_line(message)
                ),
            });
        }

        parse_ls_remote_json(package, &String::from_utf8_lossy(&output.stdout))
    }
}

/// 複数行のエラー出力から先頭行だけを取り出す (メッセージが長くなりすぎるのを防ぐ)
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("(no output)")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_tool_name() {
        assert!(is_valid_tool_name("node"));
        assert!(is_valid_tool_name("python"));
        assert!(is_valid_tool_name("npm:prettier"));
        assert!(is_valid_tool_name("npm:@biomejs/biome"));
        assert!(is_valid_tool_name("ubi:jdx/usage"));
        assert!(is_valid_tool_name("go:github.com/x/y"));
        assert!(is_valid_tool_name("cargo:ripgrep"));
        assert!(is_valid_tool_name("rust-analyzer"));
    }

    #[test]
    fn test_is_valid_tool_name_rejects_option_like_and_shell_chars() {
        assert!(!is_valid_tool_name(""));
        assert!(!is_valid_tool_name("--json"));
        assert!(!is_valid_tool_name("-x"));
        assert!(!is_valid_tool_name("node; rm -rf /"));
        assert!(!is_valid_tool_name("node`whoami`"));
        assert!(!is_valid_tool_name("node $(id)"));
        assert!(!is_valid_tool_name("node\nls"));
    }

    #[test]
    fn test_parse_created_at_rfc3339() {
        let parsed = parse_created_at(Some("2026-08-26T13:05:28.0Z"));
        assert_eq!(parsed.to_rfc3339(), "2026-08-26T13:05:28+00:00");
    }

    /// タイムゾーンなしの表記 (asdf 系プラグインが返す) も UTC として読む
    #[test]
    fn test_parse_created_at_naive() {
        let parsed = parse_created_at(Some("2025-03-28T22:04:28.484345"));
        assert_eq!(
            parsed.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2025-03-28 22:04:28"
        );
    }

    #[test]
    fn test_parse_created_at_missing_falls_back_to_epoch() {
        assert_eq!(parse_created_at(None).timestamp(), 0);
        assert_eq!(parse_created_at(Some("")).timestamp(), 0);
        assert_eq!(parse_created_at(Some("not a date")).timestamp(), 0);
    }

    #[test]
    fn test_parse_ls_remote_json() {
        let json = r#"[
            {"version":"26.7.0","created_at":"2026-08-01T00:00:00.0Z"},
            {"version":"26.8.1","created_at":"2026-08-26T13:05:28.0Z"}
        ]"#;
        let versions = parse_ls_remote_json("node", json).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, "26.7.0");
        assert_eq!(versions[1].version, "26.8.1");
    }

    /// created_at を持たないバックエンド (GitHub tag ベース等) でも取りこぼさない
    #[test]
    fn test_parse_ls_remote_json_without_created_at() {
        let json = r#"[{"version":"1.2.3"},{"version":"1.2.4","prerelease":false}]"#;
        let versions = parse_ls_remote_json("ubi:owner/repo", json).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].released_at.timestamp(), 0);
    }

    #[test]
    fn test_parse_ls_remote_json_empty() {
        assert!(parse_ls_remote_json("node", "").unwrap().is_empty());
        assert!(parse_ls_remote_json("node", "[]").unwrap().is_empty());
    }

    #[test]
    fn test_parse_ls_remote_json_invalid() {
        let err = parse_ls_remote_json("node", "not json").unwrap_err();
        assert!(matches!(err, RegistryError::InvalidResponse { .. }));
    }

    #[tokio::test]
    async fn test_fetch_versions_rejects_invalid_tool_name() {
        let adapter = MiseAdapter::new();
        let err = adapter.fetch_versions("--json").await.unwrap_err();
        assert!(matches!(err, RegistryError::InvalidPackageName { .. }));
    }

    /// mise が見つからない環境でも panic せずエラーとして返す
    #[tokio::test]
    async fn test_fetch_versions_missing_binary_is_error() {
        let adapter = MiseAdapter::with_program("depup-nonexistent-mise-binary");
        let err = adapter.fetch_versions("node").await.unwrap_err();
        assert!(matches!(err, RegistryError::NetworkError { .. }));
    }

    #[test]
    fn test_registry_metadata() {
        let adapter = MiseAdapter::new();
        assert_eq!(adapter.language(), Language::Mise);
        assert_eq!(adapter.registry_name(), "mise");
    }
}
