//! Core domain models for depup
//!
//! This module contains the fundamental types used throughout the application:
//! - Language types for supported ecosystems
//! - Version specification types for parsing and maintaining version constraints
//! - Dependency information structures
//! - Update decision results
//! - Summary and result structures

mod dependency;
mod language;
mod summary;
mod update_result;
mod version_spec;

pub use dependency::Dependency;
pub use language::Language;
pub use summary::{ManifestUpdateResult, UpdateSummary};
pub use update_result::{SkipReason, UpdateResult};
pub use version_spec::{VersionSpec, VersionSpecKind};
