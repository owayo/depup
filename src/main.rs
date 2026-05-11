//! depup - 多言語対応の依存関係アップデーター CLI ツール
//!
//! 複数のプログラミング言語の依存関係を更新するツール:
//! - Node.js (package.json)
//! - Python (pyproject.toml)
//! - Rust (Cargo.toml)
//! - Go (go.mod)
//! - Ruby (Gemfile)
//! - PHP (composer.json)
//! - Java (build.gradle / build.gradle.kts)

use clap::Parser;
use depup::cli::CliArgs;
use depup::config::DepupConfig;
use depup::domain::Language;
use depup::orchestrator::{Orchestrator, OrchestratorResult};
use depup::output::{OutputConfig, create_formatter};
use depup::package_manager::{SystemPackageManager, run_installs};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = CliArgs::parse();

    // バージョンフラグの処理
    if args.print_version {
        println!("depup {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // --cd が指定されている場合はディレクトリを変更
    if let Some(ref dir) = args.directory
        && let Err(e) = std::env::set_current_dir(dir)
    {
        eprintln!(
            "Error: cannot change to directory '{}': {}",
            dir.display(),
            e
        );
        return ExitCode::FAILURE;
    }

    // メインロジックを実行してエラーを処理
    match run(args).await {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// アプリケーションのメインロジック
async fn run(args: CliArgs) -> anyhow::Result<ExitCode> {
    // verbose モードではバージョン情報を表示
    if args.verbose {
        eprintln!("depup v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Target: {}", args.path.display());
        if args.dry_run {
            eprintln!("Mode: dry-run");
        }
    }

    // .depup モノレポ設定を確認
    let monorepo_config = DepupConfig::from_dir(&args.path);

    // オーケストレーターを作成して実行
    let orchestrator = Orchestrator::new(args.clone())?;

    let (result, monorepo_dirs) = if let Some(config) = monorepo_config {
        let dirs = config.directories_with_root(&args.path);
        if args.verbose {
            eprintln!("Monorepo mode: {} directories", dirs.len());
            for dir in &dirs {
                eprintln!("  - {}", dir.display());
            }
        }
        let r = orchestrator.run_directories(&dirs).await;
        (r, Some(dirs))
    } else {
        let r = orchestrator.run().await;
        (r, None)
    };

    // CLI オプションに基づいて出力フォーマッターを作成
    let output_config =
        OutputConfig::from_cli(args.json, args.diff, args.verbose, args.quiet, args.dry_run);
    let formatter = create_formatter(output_config);

    // 結果を出力
    let mut stdout = io::stdout().lock();
    formatter.format(&result, &mut stdout)?;
    stdout.flush()?;

    // verbose モードではエラーを表示
    if args.verbose && !result.errors.is_empty() {
        eprintln!();
        eprintln!("Errors encountered:");
        for error in &result.errors {
            eprintln!("  - {}", error);
        }
    }

    // dry-run でない場合、要求があればパッケージマネージャの install を実行
    if args.install && !args.dry_run {
        run_package_installs(&args, &result, &monorepo_dirs)?;

        // --age と --install が同時指定された場合、Rust の transitive 依存も
        // age 制約を満たすよう Cargo.lock を整える。
        if let Some(age) = args.age {
            enforce_rust_lock_age(&args, &orchestrator, &result, &monorepo_dirs, age).await;
        }
    }

    // 適切な終了コードを返す
    let has_errors = !result.errors.is_empty();
    let has_updates = result.summary.total_updates() > 0;

    if has_errors {
        // 部分的な成功 - 一部エラーが発生
        Ok(ExitCode::from(2))
    } else if has_updates || args.dry_run {
        // 成功 - 更新が行われた (dry-run では更新予定)
        Ok(ExitCode::SUCCESS)
    } else {
        // 更新不要
        Ok(ExitCode::SUCCESS)
    }
}

/// パッケージマネージャの install を実行する (単一ディレクトリとモノレポの両方に対応)
fn run_package_installs(
    args: &CliArgs,
    result: &OrchestratorResult,
    monorepo_dirs: &Option<Vec<PathBuf>>,
) -> anyhow::Result<()> {
    // ディレクトリ -> install が必要な言語のマップを構築
    let install_map = build_install_map(result, monorepo_dirs, &args.path);

    if install_map.is_empty() {
        return Ok(());
    }

    if args.verbose {
        eprintln!();
        eprintln!("Running package manager install...");
        if args.age.is_some() {
            // age が指定されている場合、ネイティブ対応 PM とそうでないものを通知する
            let mut unsupported: Vec<&str> = Vec::new();
            for (_dir, langs) in &install_map {
                for lang in langs {
                    match lang {
                        Language::Node => {
                            // Node.js は pnpm のみネイティブ対応 (install 時に判定)
                        }
                        Language::Python => {
                            // Python は uv のみネイティブ対応
                        }
                        Language::Rust => {
                            // Rust は post-install audit (enforce_lock_age_rust) で対応
                        }
                        Language::Go => unsupported.push("Go"),
                        Language::Ruby => unsupported.push("Ruby"),
                        Language::Php => unsupported.push("PHP"),
                        Language::Java => unsupported.push("Java"),
                        Language::Swift => unsupported.push("Swift"),
                    }
                }
            }
            unsupported.sort();
            unsupported.dedup();
            if !unsupported.is_empty() {
                eprintln!(
                    "  Note: --age applies to direct deps only for: {} (no native transitive-age support)",
                    unsupported.join(", ")
                );
            }
        }
    }

    let pm_runner = SystemPackageManager::new();
    let mut any_install_failed = false;

    for (dir, languages) in &install_map {
        let install_results = run_installs(&pm_runner, languages, dir, args.age);

        for install_result in &install_results {
            if install_result.command.is_empty() {
                continue;
            }

            if install_result.success {
                if args.verbose {
                    eprintln!(
                        "  {} install completed: {} ({})",
                        install_result.language.display_name(),
                        install_result.command,
                        dir.display()
                    );
                }
            } else {
                eprintln!(
                    "  {} install failed: {} ({})",
                    install_result.language.display_name(),
                    install_result.command,
                    dir.display()
                );
                if !install_result.stderr.is_empty() {
                    eprintln!("    {}", install_result.stderr);
                }
                any_install_failed = true;
            }
        }
    }

    if any_install_failed {
        anyhow::bail!("Some package manager installs failed");
    }

    Ok(())
}

/// Rust プロジェクト (Cargo.toml を含む) ディレクトリに対し、
/// `--age` を transitive 依存にも適用する。install 済み Cargo.lock を走査し、
/// age 違反の依存を `cargo update -p --precise` で古いバージョンへ差し戻す。
async fn enforce_rust_lock_age(
    args: &CliArgs,
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
    monorepo_dirs: &Option<Vec<PathBuf>>,
    age: std::time::Duration,
) {
    use depup::orchestrator::LockAgeStatus;

    // 対象となる Rust プロジェクトディレクトリを収集
    let mut rust_dirs: Vec<PathBuf> = Vec::new();
    for manifest in &result.summary.manifests {
        if manifest.language != Language::Rust {
            continue;
        }
        let Some(parent) = manifest.path.parent() else {
            continue;
        };
        let working_dir = if let Some(dirs) = monorepo_dirs {
            nearest_monorepo_dir(&manifest.path, dirs, parent)
        } else {
            parent.to_path_buf()
        };
        if !rust_dirs.contains(&working_dir) {
            rust_dirs.push(working_dir);
        }
    }

    if rust_dirs.is_empty() {
        return;
    }

    if args.verbose {
        eprintln!();
        eprintln!("Enforcing --age on transitive Rust dependencies...");
    }

    for dir in &rust_dirs {
        let adjustments = orchestrator.enforce_lock_age_rust(dir, age).await;
        if adjustments.is_empty() {
            if args.verbose {
                eprintln!("  {} — all transitive deps within --age", dir.display());
            }
            continue;
        }

        let downgraded: Vec<_> = adjustments
            .iter()
            .filter(|a| matches!(a.status, LockAgeStatus::Downgraded))
            .collect();
        let failures: Vec<_> = adjustments
            .iter()
            .filter(|a| !matches!(a.status, LockAgeStatus::Downgraded))
            .collect();

        if !downgraded.is_empty() {
            eprintln!(
                "  {} — {} transitive dep(s) rolled back to satisfy --age:",
                dir.display(),
                downgraded.len()
            );
            for adj in &downgraded {
                eprintln!(
                    "    {} {} → {}",
                    adj.name,
                    adj.from,
                    adj.to.as_deref().unwrap_or("?")
                );
            }
        }

        if !failures.is_empty() && args.verbose {
            eprintln!(
                "  {} — {} transitive dep(s) could not be rolled back:",
                dir.display(),
                failures.len()
            );
            for adj in &failures {
                let detail = match &adj.status {
                    LockAgeStatus::NoOlderCandidate => "no older candidate".to_string(),
                    LockAgeStatus::ReleaseDateUnavailable => "release date unavailable".to_string(),
                    LockAgeStatus::UpdateCommandFailed(msg) => {
                        format!("cargo update failed: {msg}")
                    }
                    LockAgeStatus::Downgraded => unreachable!(),
                };
                eprintln!("    {} ({}): {}", adj.name, adj.from, detail);
            }
        }
    }
}

/// 結果からディレクトリ -> install が必要な言語のマップを構築する
fn nearest_monorepo_dir(
    manifest_path: &Path,
    monorepo_dirs: &[PathBuf],
    fallback: &Path,
) -> PathBuf {
    monorepo_dirs
        .iter()
        .filter(|dir| manifest_path.starts_with(dir))
        .max_by_key(|dir| dir.components().count())
        .cloned()
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn build_install_map(
    result: &OrchestratorResult,
    monorepo_dirs: &Option<Vec<PathBuf>>,
    default_path: &Path,
) -> Vec<(PathBuf, Vec<Language>)> {
    let mut dir_langs: HashMap<PathBuf, Vec<Language>> = HashMap::new();

    for manifest in &result.summary.manifests {
        if !manifest.has_updates() {
            continue;
        }

        // このマニフェストが属するディレクトリを特定
        let working_dir = if let Some(dirs) = monorepo_dirs {
            let manifest_path = &manifest.path;
            nearest_monorepo_dir(manifest_path, dirs, default_path)
        } else {
            default_path.to_path_buf()
        };

        let entry = dir_langs.entry(working_dir).or_default();
        if !entry.contains(&manifest.language) {
            entry.push(manifest.language);
        }
    }

    dir_langs.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use depup::domain::{
        Dependency, ManifestUpdateResult, UpdateResult, UpdateSummary, VersionSpec, VersionSpecKind,
    };

    fn result_with_update(path: PathBuf, language: Language) -> OrchestratorResult {
        let spec = VersionSpec::new(VersionSpecKind::Caret, "1.0.0", "1.0.0");
        let dep = Dependency::production("pkg", spec, language);
        let mut manifest = ManifestUpdateResult::new(path, language);
        manifest.add_result(UpdateResult::update(dep, "2.0.0"));

        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_nearest_monorepo_dir_uses_deepest_match() {
        let root = PathBuf::from("/repo");
        let app = PathBuf::from("/repo/apps/web");
        let manifest = app.join("package.json");
        let dirs = vec![root.clone(), app.clone()];

        assert_eq!(nearest_monorepo_dir(&manifest, &dirs, &root), app);
    }

    #[test]
    fn test_build_install_map_uses_nested_monorepo_dir() {
        let root = PathBuf::from("/repo");
        let app = PathBuf::from("/repo/apps/web");
        let result = result_with_update(app.join("package.json"), Language::Node);
        let dirs = Some(vec![root.clone(), app.clone()]);

        let install_map = build_install_map(&result, &dirs, &root);

        assert_eq!(install_map.len(), 1);
        assert_eq!(install_map[0].0, app);
        assert_eq!(install_map[0].1, vec![Language::Node]);
    }
}
