//! Version specification parsers for different package ecosystems
//!
//! This module provides parsers for version specifications in:
//! - Node.js (npm/yarn/pnpm)
//! - Python (pip/poetry)
//! - Rust (cargo)
//! - Go (go mod)

mod node;
mod python;
mod rust;
mod go;

pub use node::NodeVersionParser;
pub use python::PythonVersionParser;
pub use rust::RustVersionParser;
pub use go::GoVersionParser;

use crate::domain::{Language, VersionSpec};

/// Trait for parsing version specifications
pub trait VersionParser {
    /// Parse a version specification string
    fn parse(&self, version_str: &str) -> Option<VersionSpec>;

    /// Returns the language this parser handles
    fn language(&self) -> Language;
}

/// Get a version parser for the specified language
pub fn get_parser(language: Language) -> Box<dyn VersionParser> {
    match language {
        Language::Node => Box::new(NodeVersionParser),
        Language::Python => Box::new(PythonVersionParser),
        Language::Rust => Box::new(RustVersionParser),
        Language::Go => Box::new(GoVersionParser),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_parser_node() {
        let parser = get_parser(Language::Node);
        assert_eq!(parser.language(), Language::Node);
    }

    #[test]
    fn test_get_parser_python() {
        let parser = get_parser(Language::Python);
        assert_eq!(parser.language(), Language::Python);
    }

    #[test]
    fn test_get_parser_rust() {
        let parser = get_parser(Language::Rust);
        assert_eq!(parser.language(), Language::Rust);
    }

    #[test]
    fn test_get_parser_go() {
        let parser = get_parser(Language::Go);
        assert_eq!(parser.language(), Language::Go);
    }
}
