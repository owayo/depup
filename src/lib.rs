//! depup - Multi-language dependency updater library
//!
//! This library provides the core functionality for updating dependencies
//! across multiple programming languages:
//! - Node.js (package.json)
//! - Python (pyproject.toml)
//! - Rust (Cargo.toml)
//! - Go (go.mod)
//! - Ruby (Gemfile)
//! - PHP (composer.json)
//! - Java (build.gradle / build.gradle.kts)

pub mod cli;
pub mod config;
pub mod domain;
pub mod error;
pub mod manifest;
pub mod orchestrator;
pub mod output;
pub mod package_manager;
pub mod parser;
pub mod progress;
pub mod registry;
pub mod tauri_sync;
pub mod update;
