//! depup - Multi-language dependency updater CLI tool
//!
//! This tool updates dependencies across multiple programming languages:
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
use depup::output::{create_formatter, OutputConfig};
use depup::package_manager::{run_installs, SystemPackageManager};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = CliArgs::parse();

    // Handle version flag
    if args.print_version {
        println!("depup {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    // Change directory if --cd is specified
    if let Some(ref dir) = args.directory {
        if let Err(e) = std::env::set_current_dir(dir) {
            eprintln!(
                "Error: cannot change to directory '{}': {}",
                dir.display(),
                e
            );
            return ExitCode::FAILURE;
        }
    }

    // Run the main logic and handle errors
    match run(args).await {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Main application logic
async fn run(args: CliArgs) -> anyhow::Result<ExitCode> {
    // Print version info in verbose mode
    if args.verbose {
        eprintln!("depup v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Target: {}", args.path.display());
        if args.dry_run {
            eprintln!("Mode: dry-run");
        }
    }

    // Check for .depup monorepo config
    let monorepo_config = DepupConfig::from_dir(&args.path);

    // Create and run the orchestrator
    let orchestrator = Orchestrator::new(args.clone())?;

    let (result, monorepo_dirs) = if let Some(config) = monorepo_config {
        if args.verbose {
            eprintln!("Monorepo mode: {} directories", config.directories.len());
            for dir in &config.directories {
                eprintln!("  - {}", dir.display());
            }
        }
        let dirs = config.directories.clone();
        let r = orchestrator.run_directories(&dirs).await;
        (r, Some(dirs))
    } else {
        let r = orchestrator.run().await;
        (r, None)
    };

    // Create output formatter based on CLI options
    let output_config =
        OutputConfig::from_cli(args.json, args.diff, args.verbose, args.quiet, args.dry_run);
    let formatter = create_formatter(output_config);

    // Output results
    let mut stdout = io::stdout().lock();
    formatter.format(&result, &mut stdout)?;
    stdout.flush()?;

    // Print errors in verbose mode
    if args.verbose && !result.errors.is_empty() {
        eprintln!();
        eprintln!("Errors encountered:");
        for error in &result.errors {
            eprintln!("  - {}", error);
        }
    }

    // Run package manager install if requested and not dry-run
    if args.install && !args.dry_run {
        run_package_installs(&args, &result, &monorepo_dirs)?;
    }

    // Return appropriate exit code
    let has_errors = !result.errors.is_empty();
    let has_updates = result.summary.total_updates() > 0;

    if has_errors {
        // Partial success - some errors occurred
        Ok(ExitCode::from(2))
    } else if has_updates || args.dry_run {
        // Success - updates were made (or would be in dry-run)
        Ok(ExitCode::SUCCESS)
    } else {
        // No updates needed
        Ok(ExitCode::SUCCESS)
    }
}

/// Run package manager installs, handling both single-dir and monorepo modes
fn run_package_installs(
    args: &CliArgs,
    result: &OrchestratorResult,
    monorepo_dirs: &Option<Vec<PathBuf>>,
) -> anyhow::Result<()> {
    // Build a map of directory -> languages that need install
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

/// Build a map of directory -> languages needing install from the result
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

        // Determine which directory this manifest belongs to
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
