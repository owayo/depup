//! depup - 多言語対応の依存関係アップデーターライブラリ
//!
//! 複数のプログラミング言語の依存関係更新に必要なコア機能を提供する:
//! - Node.js（package.json）対応
//! - Python（pyproject.toml）対応
//! - Rust（Cargo.toml）対応
//! - Go（go.mod）対応
//! - Ruby（Gemfile）対応
//! - PHP（composer.json）対応
//! - Java（build.gradle / build.gradle.kts）対応

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
