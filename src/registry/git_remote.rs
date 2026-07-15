//! `git ls-remote` を実行してリモートリポジトリの refs を取得する。
//!
//! レジストリアダプタのような HTTP ではなく、`git` コマンドを介して
//! ブランチ・タグ・HEAD のコミットハッシュを取得する。
//!
//! 認証は `GIT_ASKPASS` / SSH 鍵 / `.netrc` などの非対話的手段に任せる (passthrough)。
//! `GIT_TERMINAL_PROMPT=0` を付与するため、端末での対話プロンプトは発生せず、
//! 認証が必要な場合は即エラーになる。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

/// `git ls-remote` 実行全体のタイムアウト
///
/// 認証待ちや応答しないリモートで無期限にブロックしないための安全弁。
/// タイムアウト時はエラーとして扱い、そのバージョンチェックはスキップされる。
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

/// git ls-remote の取得結果
#[derive(Debug, Clone, Default)]
pub struct GitRemoteRefs {
    /// `refs/heads/<name>` からコミットハッシュへの対応
    pub heads: HashMap<String, String>,
    /// `refs/tags/<name>` からコミットハッシュへの対応
    pub tags: HashMap<String, String>,
    /// `HEAD` に対応するコミットハッシュ
    pub head: Option<String>,
}

impl GitRemoteRefs {
    /// 指定されたブランチのコミットハッシュを取得する
    pub fn branch_commit(&self, name: &str) -> Option<&str> {
        self.heads.get(name).map(|s| s.as_str())
    }

    /// 指定されたタグのコミットハッシュを取得する。`^{}` (peeled) を優先する
    pub fn tag_commit(&self, name: &str) -> Option<&str> {
        let peeled = format!("{}^{{}}", name);
        self.tags
            .get(&peeled)
            .or_else(|| self.tags.get(name))
            .map(|s| s.as_str())
    }

    /// 利用可能な全タグ名を返す (`^{}` 接尾辞を除去済み)
    pub fn all_tag_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tags
            .keys()
            .map(|k| k.trim_end_matches("^{}").to_string())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// HEAD のコミットハッシュを取得する
    pub fn head_commit(&self) -> Option<&str> {
        self.head.as_deref()
    }
}

/// git ls-remote のエラー種別
#[derive(Debug, Clone, thiserror::Error)]
pub enum GitRemoteError {
    #[error("failed to spawn `git ls-remote {url}`: {message}")]
    SpawnFailed { url: String, message: String },
    #[error("`git ls-remote {url}` exited with status {status}: {stderr}")]
    CommandFailed {
        url: String,
        status: String,
        stderr: String,
    },
    #[error("invalid output from `git ls-remote {url}`: {message}")]
    InvalidOutput { url: String, message: String },
    #[error("`git ls-remote {url}` timed out after {seconds}s")]
    Timeout { url: String, seconds: u64 },
    #[error(
        "unsupported git URL scheme '{url}' (allowed: https://, http://, ssh://, git://, git@, file://)"
    )]
    UnsupportedUrlScheme { url: String },
}

/// URL 単位でキャッシュする `git ls-remote` クライアント
#[derive(Clone, Default)]
pub struct GitRemote {
    cache: Arc<Mutex<HashMap<String, Result<GitRemoteRefs, GitRemoteError>>>>,
}

impl GitRemote {
    /// 新しい `GitRemote` を作る
    pub fn new() -> Self {
        Self::default()
    }

    /// `git ls-remote <url>` を実行して refs を取得する。
    /// 同一 URL に対する以降の呼び出しはキャッシュ結果を返す。
    pub async fn fetch(&self, url: &str) -> Result<GitRemoteRefs, GitRemoteError> {
        {
            let cache = self.cache.lock().await;
            if let Some(cached) = cache.get(url) {
                return cached.clone();
            }
        }

        let result = run_ls_remote(url).await;

        {
            let mut cache = self.cache.lock().await;
            cache.insert(url.to_string(), result.clone());
        }

        result
    }
}

/// `git ls-remote` に渡してよい URL スキームかどうかを検証する
///
/// git の `ext::` トランスポート (例: `ext::sh -c <cmd>`) はリモートヘルパー経由で
/// 任意コマンドを実行できるため、マニフェスト由来の URL をそのまま渡すのは危険。
/// 既知の安全なトランスポートのみ許可リストで通す:
/// `https://` / `http://` / `ssh://` / `git://` / `file://` / `git@` (scp 形式)
fn is_allowed_git_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
        || lower.starts_with("file://")
        || lower.starts_with("git@")
}

async fn run_ls_remote(url: &str) -> Result<GitRemoteRefs, GitRemoteError> {
    // 許可リスト外のスキーム (ext:: 等) は実行せず警告してスキップ
    // (呼び出し側がこのエラーを fetch failed として警告付きスキップ扱いにする)
    if !is_allowed_git_url(url) {
        eprintln!(
            "⚠ skipping git dependency with unsupported URL scheme: {} (allowed: https://, http://, ssh://, git://, git@, file://)",
            url
        );
        return Err(GitRemoteError::UnsupportedUrlScheme {
            url: url.to_string(),
        });
    }

    let output_future = Command::new("git")
        .arg("ls-remote")
        .arg("--")
        .arg(url)
        // 端末での認証プロンプトを禁止する。プロンプトが必要な場合は即エラーになり、
        // 認証情報の入力待ちで無期限にブロックしない (非対話的な認証手段は引き続き有効)。
        .env("GIT_TERMINAL_PROMPT", "0")
        // タイムアウトで future が drop された際に子プロセスを残さない
        .kill_on_drop(true)
        .output();

    let output = match tokio::time::timeout(LS_REMOTE_TIMEOUT, output_future).await {
        Ok(result) => result.map_err(|e| GitRemoteError::SpawnFailed {
            url: url.to_string(),
            message: e.to_string(),
        })?,
        Err(_) => {
            return Err(GitRemoteError::Timeout {
                url: url.to_string(),
                seconds: LS_REMOTE_TIMEOUT.as_secs(),
            });
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitRemoteError::CommandFailed {
            url: url.to_string(),
            status: output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            stderr,
        });
    }

    let stdout =
        std::str::from_utf8(&output.stdout).map_err(|e| GitRemoteError::InvalidOutput {
            url: url.to_string(),
            message: e.to_string(),
        })?;

    Ok(parse_ls_remote_output(stdout))
}

/// `git ls-remote` 形式の文字列をパースする (テスト用に分離)
pub fn parse_ls_remote_output(text: &str) -> GitRemoteRefs {
    let mut refs = GitRemoteRefs::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 形式: `<sha>\t<ref>`
        let mut parts = line.splitn(2, '\t');
        let Some(sha) = parts.next() else { continue };
        let Some(ref_name) = parts.next() else {
            continue;
        };
        if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if ref_name == "HEAD" {
            refs.head = Some(sha.to_string());
        } else if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
            refs.heads.insert(branch.to_string(), sha.to_string());
        } else if let Some(tag) = ref_name.strip_prefix("refs/tags/") {
            refs.tags.insert(tag.to_string(), sha.to_string());
        }
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let out = "\
0123456789abcdef0123456789abcdef01234567\tHEAD
0123456789abcdef0123456789abcdef01234567\trefs/heads/main
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/heads/dev
1111111111111111111111111111111111111111\trefs/tags/v1.0.0
2222222222222222222222222222222222222222\trefs/tags/v1.0.0^{}
";
        let refs = parse_ls_remote_output(out);
        assert_eq!(
            refs.head.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            refs.branch_commit("main"),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            refs.branch_commit("dev"),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        // peeled が優先される
        assert_eq!(
            refs.tag_commit("v1.0.0"),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn test_parse_invalid_lines_skipped() {
        let out = "\
not-a-sha\trefs/heads/bogus
\t
0123456789abcdef0123456789abcdef01234567\trefs/heads/main
";
        let refs = parse_ls_remote_output(out);
        assert_eq!(refs.heads.len(), 1);
        assert!(refs.branch_commit("bogus").is_none());
        assert!(refs.branch_commit("main").is_some());
    }

    #[test]
    fn test_all_tag_names_dedupes_peeled() {
        let out = "\
1111111111111111111111111111111111111111\trefs/tags/v1.0.0
2222222222222222222222222222222222222222\trefs/tags/v1.0.0^{}
3333333333333333333333333333333333333333\trefs/tags/v1.1.0
";
        let refs = parse_ls_remote_output(out);
        let names = refs.all_tag_names();
        assert_eq!(names, vec!["v1.0.0".to_string(), "v1.1.0".to_string()]);
    }

    #[test]
    fn test_tag_commit_without_peeled() {
        let out = "\
1111111111111111111111111111111111111111\trefs/tags/v1.0.0
";
        let refs = parse_ls_remote_output(out);
        assert_eq!(
            refs.tag_commit("v1.0.0"),
            Some("1111111111111111111111111111111111111111")
        );
    }

    #[test]
    fn test_head_commit_accessor() {
        let out = "0123456789abcdef0123456789abcdef01234567\tHEAD\n";
        let refs = parse_ls_remote_output(out);
        assert_eq!(
            refs.head_commit(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    /// バグ回帰テスト: 既知の安全なトランスポートのみ許可する。
    /// 以前はマニフェスト由来の URL を無検証で `git ls-remote` に渡していたため、
    /// `ext::sh -c <cmd>` のようなリモートヘルパー構文で任意コマンド実行が可能だった。
    #[test]
    fn test_allowed_git_url_schemes() {
        assert!(is_allowed_git_url("https://github.com/owner/repo.git"));
        assert!(is_allowed_git_url("http://internal.example.com/repo.git"));
        assert!(is_allowed_git_url("ssh://git@github.com/owner/repo.git"));
        assert!(is_allowed_git_url("git://github.com/owner/repo.git"));
        assert!(is_allowed_git_url("file:///path/to/repo"));
        // scp 形式
        assert!(is_allowed_git_url("git@github.com:owner/repo.git"));
        // スキームは大文字小文字を区別しない
        assert!(is_allowed_git_url("HTTPS://github.com/owner/repo.git"));
    }

    #[test]
    fn test_disallowed_git_url_schemes() {
        // ext:: トランスポートは任意コマンド実行が可能なため拒否する
        assert!(!is_allowed_git_url("ext::sh -c whoami"));
        assert!(!is_allowed_git_url("ext::git --namespace=foo %s /repo"));
        // その他のリモートヘルパー / 不明スキームも拒否
        assert!(!is_allowed_git_url("transport::address"));
        assert!(!is_allowed_git_url("ftp://example.com/repo.git"));
        // スキームなしの相対/絶対パスも拒否 (file:// を明示させる)
        assert!(!is_allowed_git_url("/path/to/repo"));
        assert!(!is_allowed_git_url("../relative/repo"));
        assert!(!is_allowed_git_url(""));
    }

    /// 許可リスト外 URL は fetch がコマンド実行せずエラーを返す
    #[tokio::test]
    async fn test_fetch_rejects_unsupported_scheme() {
        let remote = GitRemote::new();
        let result = remote.fetch("ext::sh -c whoami").await;
        assert!(matches!(
            result,
            Err(GitRemoteError::UnsupportedUrlScheme { .. })
        ));
    }

    #[test]
    fn test_timeout_error_message() {
        let err = GitRemoteError::Timeout {
            url: "https://example.com/repo.git".to_string(),
            seconds: 30,
        };
        let msg = err.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("30"));
    }

    #[test]
    fn test_ls_remote_timeout_constant() {
        // 実行全体の安全弁は 30 秒
        assert_eq!(LS_REMOTE_TIMEOUT, Duration::from_secs(30));
    }
}
