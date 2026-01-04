//! Gemfile parser for Ruby projects
//!
//! Handles:
//! - gem declarations with version constraints
//! - Development group dependencies
//! - Pessimistic version constraints (~>)

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;

/// Parser for Gemfile files
pub struct GemfileParser;

impl ManifestParser for GemfileParser {
    fn parse(&self, _content: &str) -> Result<Vec<Dependency>, ManifestError> {
        // TODO: Implement Gemfile parsing in Task 4.1
        Ok(Vec::new())
    }

    fn language(&self) -> Language {
        Language::Ruby
    }

    fn update_version(
        &self,
        content: &str,
        _package: &str,
        _new_version: &str,
    ) -> Result<String, ManifestError> {
        // TODO: Implement version update in Task 4.1
        Ok(content.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemfile_parser_language() {
        let parser = GemfileParser;
        assert_eq!(parser.language(), Language::Ruby);
    }
}
