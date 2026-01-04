//! PHP version specification parser
//!
//! Handles:
//! - Fixed versions: `1.2.3`
//! - Caret ranges: `^1.2.3`
//! - Tilde ranges: `~1.2.3`
//! - Comparison operators: `>=`, `<`, `>`
//! - Compound constraints: `>=1.0 <2.0`
//! - Wildcards: `1.2.*`

use crate::domain::{Language, VersionSpec};
use crate::parser::VersionParser;

/// Parser for PHP version specifications
pub struct PhpVersionParser;

impl VersionParser for PhpVersionParser {
    fn parse(&self, _version_str: &str) -> Option<VersionSpec> {
        // TODO: Implement PHP version parsing in Task 3.1
        None
    }

    fn language(&self) -> Language {
        Language::Php
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_php_parser_language() {
        let parser = PhpVersionParser;
        assert_eq!(parser.language(), Language::Php);
    }
}
