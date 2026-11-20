//! Git サブモジュールの作業ツリーを、親リポジトリの gitlink を基準に更新する。

use crate::cli::CliArgs;
use crate::domain::{SubmoduleOutcome, SubmoduleResult, UpdateSummary};
use crate::orchestrator::{OrchestratorError, OrchestratorResult};
use crate::update::UpdateFilter;
use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

pub(crate) async fn run(args: &CliArgs) -> OrchestratorResult {
    let mut summary = UpdateSummary::new(args.dry_run);
    let mut errors = Vec::new();
    let mut results = Vec::new();
    if let Err(error) = update_submodules(args, &mut results).await {
        errors.push(OrchestratorError::ManifestDetectionError(format!(
            "{}: {error:#}",
            args.path.display()
        )));
    }
    summary.submodules = Some(results);
    OrchestratorResult {
        summary,
        write_results: Vec::new(),
        errors,
    }
}

async fn update_submodules(args: &CliArgs, results: &mut Vec<SubmoduleResult>) -> Result<()> {
    let scope = args.path.canonicalize()?;
    let root = git_worktree_root(&scope).await?;
    ensure!(
        scope.starts_with(&root),
        "target is outside the Git worktree"
    );

    let config = root.join(".gitmodules");
    match config.symlink_metadata() {
        Ok(metadata) => ensure!(metadata.is_file(), ".gitmodules must be a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    let paths = submodule_paths(&root).await?;
    let filter = UpdateFilter::new()
        .with_only(args.only.clone())
        .with_exclude(args.exclude.clone());
    for (path, key) in paths {
        if !root.join(&path).starts_with(&scope) || !filter.should_process_package(&path) {
            continue;
        }
        let outcome = match update_one(&root, &key, &path, args.dry_run).await {
            Ok(outcome) => outcome,
            Err(error) => SubmoduleOutcome::Failed {
                error: format!("{error:#}"),
            },
        };
        results.push(SubmoduleResult { path, outcome });
    }
    Ok(())
}

async fn submodule_paths(root: &Path) -> Result<BTreeMap<String, String>> {
    let args = [
        "config",
        "--file",
        ".gitmodules",
        "--null",
        "--get-regexp",
        r"^submodule\..+\.path$",
    ];
    let output = git_output(root, &args).await?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(BTreeMap::new());
    }
    let entries = git_stdout(output, &args)?;
    let mut paths = BTreeMap::new();
    for entry in entries.split_terminator('\0') {
        let (key, path) = entry
            .split_once('\n')
            .context("invalid .gitmodules path record")?;
        let key = key
            .strip_suffix(".path")
            .context("invalid .gitmodules path key")?;
        ensure!(
            !path.is_empty()
                && Path::new(path)
                    .components()
                    .all(|p| matches!(p, Component::Normal(_))),
            "invalid submodule path {path:?}"
        );
        ensure!(
            paths.insert(path.to_owned(), key.to_owned()).is_none(),
            "duplicate submodule path {path:?}"
        );
    }
    Ok(paths)
}

async fn update_one(root: &Path, key: &str, path: &str, dry_run: bool) -> Result<SubmoduleOutcome> {
    let pathspec = format!(":(literal){path}");
    let Some(from) = index_commit(root, &pathspec, path).await? else {
        return skip("path is not a tracked submodule or has unresolved conflicts");
    };
    if submodule_setting(root, key, "update").await?.as_deref() == Some("none") {
        return skip("submodule update is disabled in Git config");
    }
    if has_staged_change(root, &pathspec).await? {
        return skip("submodule pointer has staged changes");
    }

    let dir = root.join(path);
    if !dir.join(".git").exists() {
        return skip("submodule is uninitialized; run git submodule update --init first");
    }
    let canonical = dir.canonicalize()?;
    ensure!(
        canonical == dir,
        "submodule path resolves to a different directory"
    );
    ensure!(
        git_worktree_root(&canonical).await? == canonical,
        "path is not the root of a submodule worktree"
    );
    let superproject = git(&dir, &["rev-parse", "--show-superproject-working-tree"]).await?;
    let superproject = superproject.trim_end_matches(['\n', '\r']);
    ensure!(
        !superproject.is_empty() && Path::new(superproject).canonicalize()? == root,
        "checkout does not belong to the parent repository"
    );
    if let Some(reason) = local_changes(&dir, &from).await? {
        return skip(reason);
    }

    let reference = match submodule_setting(root, key, "branch").await? {
        Some(branch) if branch == "." => symbolic_head(root)
            .await?
            .context("submodule branch is '.', but the parent HEAD is detached")?,
        Some(branch) => format!("refs/heads/{branch}"),
        None => "HEAD".into(),
    };
    let remote = tracking_remote(&dir).await?;
    let refs = git(
        &dir,
        &["ls-remote", "--exit-code", "--", &remote, &reference],
    )
    .await
    .with_context(|| format!("could not resolve {remote}/{reference}"))?;
    let target = refs
        .lines()
        .find_map(|line| {
            let (commit, name) = line.split_once('\t')?;
            (name == reference && commit.len() == from.len() && is_object_id(commit))
                .then_some(commit)
        })
        .context("Git did not return a commit ID for the requested ref")?;
    if target == from {
        return skip("already at latest commit");
    }

    let object = format!("{target}^{{commit}}");
    let has_target = git_output(&dir, &["cat-file", "-e", &object])
        .await?
        .status
        .success();
    if !dry_run && !has_target {
        let args = [
            "fetch",
            "--no-tags",
            "--no-prune",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--refmap=",
            "--",
            &remote,
            target,
        ];
        check_git_status(&git_output(&dir, &args).await?, &args)?;
    }
    if (has_target || !dry_run)
        && !git_predicate(&dir, &["merge-base", "--is-ancestor", &from, target]).await?
    {
        return skip("remote commit is not a verified fast-forward");
    }
    if !dry_run {
        // ネットワークへの問い合わせ中に親の index や作業ツリーが変わっていないか再確認する。
        if index_commit(root, &pathspec, path).await?.as_deref() != Some(from.as_str())
            || has_staged_change(root, &pathspec).await?
        {
            return skip("submodule pointer changed during update");
        }
        if let Some(reason) = local_changes(&dir, &from).await? {
            return skip(reason);
        }
        let args = [
            "checkout",
            "--detach",
            "--no-overwrite-ignore",
            "--no-recurse-submodules",
            target,
            "--",
        ];
        let output = git_output(&dir, &args).await?;
        let head = git(&dir, &["rev-parse", "--verify", "HEAD"]).await;
        let reached_target = head
            .as_ref()
            .is_ok_and(|head| head.trim_ascii_end() == target);
        if let Err(error) = check_git_status(&output, &args) {
            if reached_target {
                return Err(
                    error.context(format!("HEAD reached {target} despite a checkout error"))
                );
            }
            return Err(error);
        }
        ensure!(
            reached_target,
            "could not verify that checkout reached {target}"
        );
    }
    Ok(SubmoduleOutcome::Updated {
        from,
        to: target.into(),
        reference,
    })
}

async fn git_worktree_root(dir: &Path) -> Result<PathBuf> {
    let output = git(dir, &["rev-parse", "--show-toplevel"]).await?;
    Ok(Path::new(output.trim_end_matches(['\n', '\r'])).canonicalize()?)
}

async fn index_commit(root: &Path, pathspec: &str, path: &str) -> Result<Option<String>> {
    let output = git(
        root,
        &["ls-files", "--stage", "--full-name", "-z", "--", pathspec],
    )
    .await?;
    Ok(output.split_terminator('\0').find_map(|entry| {
        let (fields, file) = entry.split_once('\t')?;
        let (commit, stage) = fields.strip_prefix("160000 ")?.split_once(' ')?;
        (file == path && stage == "0" && is_object_id(commit)).then(|| commit.to_owned())
    }))
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn has_staged_change(root: &Path, pathspec: &str) -> Result<bool> {
    Ok(!git_predicate(
        root,
        &[
            "diff",
            "--cached",
            "--quiet",
            "--no-ext-diff",
            "--ignore-submodules=none",
            "--",
            pathspec,
        ],
    )
    .await?)
}

async fn local_changes(dir: &Path, recorded_commit: &str) -> Result<Option<&'static str>> {
    let head = git(dir, &["rev-parse", "--verify", "HEAD"]).await?;
    if head.trim_ascii_end() != recorded_commit {
        return Ok(Some("checked-out commit differs from the recorded pointer"));
    }
    let status = git(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    Ok((!status.is_empty()).then_some("working tree has local changes"))
}

async fn symbolic_head(dir: &Path) -> Result<Option<String>> {
    let args = ["symbolic-ref", "--quiet", "HEAD"];
    let output = git_output(dir, &args).await?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    Ok(Some(git_stdout(output, &args)?.trim_ascii_end().to_owned()))
}

async fn tracking_remote(dir: &Path) -> Result<String> {
    let mut remote = None;
    if let Some(branch) = symbolic_head(dir).await? {
        let branch = branch
            .strip_prefix("refs/heads/")
            .context("invalid submodule HEAD")?;
        remote = config_value(dir, &["--local"], &format!("branch.{branch}.remote")).await?;
    }
    let remote = remote.unwrap_or_else(|| "origin".into());
    if remote != "." {
        let output = git_output(dir, &["remote", "get-url", "--", &remote]).await?;
        ensure!(
            output.status.success(),
            "submodule tracking remote is not configured"
        );
    }
    Ok(remote)
}

async fn submodule_setting(root: &Path, key: &str, setting: &str) -> Result<Option<String>> {
    let key = format!("{key}.{setting}");
    if let Some(value) = config_value(root, &["--local"], &key).await? {
        return Ok(Some(value));
    }
    config_value(root, &["--file", ".gitmodules"], &key).await
}

async fn config_value(dir: &Path, scope: &[&str], key: &str) -> Result<Option<String>> {
    let mut args = vec!["config"];
    args.extend_from_slice(scope);
    args.extend(["--null", "--get", key]);
    let output = git_output(dir, &args).await?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(None);
    }
    let value = git_stdout(output, &args)?;
    Ok(Some(value.trim_end_matches('\0').to_owned()))
}

fn skip(reason: &str) -> Result<SubmoduleOutcome> {
    Ok(SubmoduleOutcome::Skipped {
        reason: reason.into(),
    })
}

async fn git_predicate(dir: &Path, args: &[&str]) -> Result<bool> {
    let output = git_output(dir, args).await?;
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    check_git_status(&output, args)?;
    Ok(true)
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_stdout(git_output(dir, args).await?, args)
}

fn git_stdout(output: Output, args: &[&str]) -> Result<String> {
    check_git_status(&output, args)?;
    String::from_utf8(output.stdout).context("Git output is not UTF-8")
}

fn check_git_status(output: &Output, args: &[&str]) -> Result<()> {
    if !output.status.success() {
        // リモートエラーは設定や接続先に含まれる資格情報を報告に載せない。
        let message = format!("git {} failed ({})", args[0], output.status);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if matches!(args[0], "ls-remote" | "fetch") || stderr.trim().is_empty() {
            bail!("{message}");
        }
        bail!("{message}: {}", stderr.trim());
    }
    Ok(())
}

async fn git_output(dir: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .args([
            "--no-optional-locks",
            "--no-replace-objects",
            "-c",
            "protocol.ext.allow=never",
        ])
        .args(args)
        // Git hook などから呼ばれても、親の index やオブジェクトを子のコマンドで使わない。
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_SHALLOW_FILE")
        .env_remove("GIT_GRAFT_FILE")
        .env_remove("GIT_PREFIX")
        .env_remove("GIT_CONFIG")
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    if !matches!(args[0], "checkout" | "fetch") {
        command.env("GIT_NO_LAZY_FETCH", "1");
    }
    let operation = command.output();
    let output = match args[0] {
        "ls-remote" => tokio::time::timeout(Duration::from_secs(30), operation).await?,
        "fetch" => tokio::time::timeout(Duration::from_secs(300), operation).await?,
        _ => operation.await,
    };
    output.with_context(|| format!("could not run git {}", args[0]))
}
