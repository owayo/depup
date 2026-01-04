//! Version specification parsers for different package ecosystems
//!
//! This module provides parsers for version specifications in:
//! - Node.js (npm/yarn/pnpm)
//! - Python (pip/poetry)
//! - Rust (cargo)
//! - Go (go mod)
//! - Ruby (bundler)
//! - PHP (composer)
//! - Java (gradle)

mod go;
mod java;
mod node;
mod php;
mod python;
mod ruby;
mod rust;

pub use go::GoVersionParser;
pub use java::JavaVersionParser;
pub use node::NodeVersionParser;
pub use php::PhpVersionParser;
pub use python::PythonVersionParser;
pub use ruby::RubyVersionParser;
pub use rust::RustVersionParser;

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
        Language::Ruby => Box::new(RubyVersionParser),
        Language::Php => Box::new(PhpVersionParser),
        Language::Java => Box::new(JavaVersionParser),
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
