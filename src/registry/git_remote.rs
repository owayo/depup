//! `git ls-remote` を実行してリモートリポジトリの refs を取得する。
//!
//! レジストリアダプタのような HTTP ではなく、`git` コマンドを介して
//! ブランチ・タグ・HEAD のコミットハッシュを取得する。
//!
//! 認証は `GIT_ASKPASS` / SSH 鍵 / `.netrc` などに任せる (passthrough)。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

/// git ls-remote の取得結果
#[derive(Debug, Clone, Default)]
pub struct GitRemoteRefs {
    /// `refs/heads/<name>` -> commit hash
    pub heads: HashMap<String, String>,
    /// `refs/tags/<name>` -> commit hash
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

async fn run_ls_remote(url: &str) -> Result<GitRemoteRefs, GitRemoteError> {
    let output = Command::new("git")
        .arg("ls-remote")
        .arg("--")
        .arg(url)
        .output()
        .await
        .map_err(|e| GitRemoteError::SpawnFailed {
            url: url.to_string(),
            message: e.to_string(),
        })?;

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
}
