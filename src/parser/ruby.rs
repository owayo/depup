//! Ruby version specification parser
//!
//! Handles:
//! - Fixed versions: `= 1.2.3`, `1.2.3`
//! - Pessimistic constraints: `~> 1.2`
//! - Comparison operators: `>=`, `<`, `>`
//! - Compound constraints: `>= 1.0, < 2.0`

use crate::domain::{Language, VersionSpec};
use crate::parser::VersionParser;

/// Parser for Ruby version specifications
pub struct RubyVersionParser;

impl VersionParser for RubyVersionParser {
    fn parse(&self, _version_str: &str) -> Option<VersionSpec> {
        // TODO: Implement Ruby version parsing in Task 2.1
        None
    }

    fn language(&self) -> Language {
        Language::Ruby
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ruby_parser_language() {
        let parser = RubyVersionParser;
        assert_eq!(parser.language(), Language::Ruby);
    }
}
