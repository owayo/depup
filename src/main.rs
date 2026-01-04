//! depup - Multi-language dependency updater CLI tool
//!
//! This tool updates dependencies across multiple programming languages:
//! - Node.js (package.json)
//! - Python (pyproject.toml)
//! - Rust (Cargo.toml)
//! - Go (go.mod)

use clap::Parser;
use depup::cli::CliArgs;
use depup::domain::Language;
use depup::orchestrator::Orchestrator;
use depup::output::{create_formatter, OutputConfig};
use depup::package_manager::{run_installs, SystemPackageManager};
use std::io::{self, Write};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = CliArgs::parse();

    // Handle version flag
    if args.print_version {
        println!("depup {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
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

    // Create and run the orchestrator
    let orchestrator = Orchestrator::new(args.clone())?;
    let result = orchestrator.run().await;

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

    // Determine which languages had updates
    let mut updated_languages: Vec<Language> = Vec::new();
    for manifest in &result.summary.manifests {
        if manifest.has_updates() && !updated_languages.contains(&manifest.language) {
            updated_languages.push(manifest.language);
        }
    }

    // Run package manager install if requested and not dry-run
    if args.install && !args.dry_run && !updated_languages.is_empty() {
        if args.verbose {
            eprintln!();
            eprintln!("Running package manager install...");
        }

        let pm_runner = SystemPackageManager::new();
        let install_results = run_installs(&pm_runner, &updated_languages, &args.path);

        for install_result in &install_results {
            if install_result.command.is_empty() {
                // Skipped - no package manager found
                continue;
            }

            if install_result.success {
                if args.verbose {
                    eprintln!(
                        "  {} install completed: {}",
                        install_result.language.display_name(),
                        install_result.command
                    );
                }
            } else {
                eprintln!(
                    "  {} install failed: {}",
                    install_result.language.display_name(),
                    install_result.command
                );
                if !install_result.stderr.is_empty() {
                    eprintln!("    {}", install_result.stderr);
                }
            }
        }

        // Check if any install failed
        let any_install_failed = install_results
            .iter()
            .any(|r| !r.command.is_empty() && !r.success);
        if any_install_failed {
            return Ok(ExitCode::FAILURE);
        }
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
