//! depup - 多言語対応の依存関係アップデーターライブラリ
//!
//! 複数のプログラミング言語の依存関係更新に必要なコア機能を提供する:
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
pub mod global_config;
pub mod manifest;
pub mod orchestrator;
pub mod osv;
pub mod output;
pub mod package_manager;
pub mod parser;
pub mod progress;
pub mod registry;
pub mod tauri_sync;
pub mod update;
