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
    }

    let pm_runner = SystemPackageManager::new();
    let mut any_install_failed = false;

    for (dir, languages) in &install_map {
        let install_results = run_installs(&pm_runner, languages, dir);

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

/// 結果からディレクトリ -> install が必要な言語のマップを構築する
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
            dirs.iter()
                .find(|d| manifest_path.starts_with(d))
                .cloned()
                .unwrap_or_else(|| default_path.to_path_buf())
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
