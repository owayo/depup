//! Gemfile parser for Ruby projects
//!
//! Handles:
//! - gem declarations with version constraints
//! - Development group dependencies
//! - Pessimistic version constraints (~>)
//! - Multiple version constraints
//! - Both single and double quotes

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::get_parser;
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Parser for Gemfile files
pub struct GemfileParser;

// Regex for gem declaration: gem 'name' or gem "name", with optional version(s)
// Captures: name, and optionally version constraints
static GEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Match gem 'name' or gem "name" with optional version constraints
    // gem 'rails', '~> 7.0'
    // gem "pg", ">= 0.18", "< 2.0"
    // gem 'bcrypt'
    Regex::new(
        r#"^\s*gem\s+['"]([^'"]+)['"](?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,|\s*$|\s*#)"#,
    )
    .unwrap()
});

// Regex for group block start
static GROUP_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    // group :development do
    // group :development, :test do
    Regex::new(r"^\s*group\s+(.+?)\s+do\s*$").unwrap()
});

// Regex for group block end
static GROUP_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*end\s*(?:#.*)?$").unwrap());

// Check if a group is development-only
fn is_dev_group(group_line: &str) -> bool {
    let lowered = group_line.to_lowercase();
    // Check for :development, :test symbols
    lowered.contains(":development") || lowered.contains(":test")
}

impl ManifestParser for GemfileParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Ruby);
        let mut in_dev_group = false;
        let mut group_depth = 0;

        for line in content.lines() {
            let trimmed = line.trim();

            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Check for group start
            if let Some(caps) = GROUP_START_RE.captures(trimmed) {
                group_depth += 1;
                let group_spec = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if is_dev_group(group_spec) {
                    in_dev_group = true;
                }
                continue;
            }

            // Check for group end
            if GROUP_END_RE.is_match(trimmed) && group_depth > 0 {
                group_depth -= 1;
                if group_depth == 0 {
                    in_dev_group = false;
                }
                continue;
            }

            // Check for gem declaration
            if let Some(caps) = GEM_RE.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

                if name.is_empty() {
                    continue;
                }

                // Collect version constraints (up to 3)
                let mut version_parts = Vec::new();
                for i in 2..=4 {
                    if let Some(v) = caps.get(i) {
                        version_parts.push(v.as_str().to_string());
                    }
                }

                // If no version specified, skip this gem (can't update what's not pinned)
                if version_parts.is_empty() {
                    continue;
                }

                // Parse the version constraint(s)
                let version_str = version_parts.join(", ");
                if let Some(spec) = parser.parse(&version_str) {
                    let dep = if in_dev_group {
                        Dependency::development(name, spec, Language::Ruby)
                    } else {
                        Dependency::production(name, spec, Language::Ruby)
                    };
                    dependencies.push(dep);
                }
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Ruby
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parser = get_parser(Language::Ruby);

        // Build pattern for the specific gem
        // gem 'package' or gem "package" followed by version
        let escaped_name = regex::escape(package);
        let pattern = format!(r#"(gem\s+['"]{escaped_name}['"]\s*,\s*['"])([^'"]+)(['"])"#);

        let re = Regex::new(&pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Gemfile"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let mut updated = false;
        let result = re.replace(content, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let old_version = &caps[2];
            let suffix = &caps[3];

            if let Some(spec) = parser.parse(old_version) {
                updated = true;
                let new_ver = spec.format_updated(new_version);
                format!("{}{}{}", prefix, new_ver, suffix)
            } else {
                caps[0].to_string()
            }
        });

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Gemfile"),
                spec: package.to_string(),
                message: "gem not found or version could not be updated".to_string(),
            });
        }

        Ok(result.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        GemfileParser.parse(content)
    }

    #[test]
    fn test_parse_simple_gem() {
        let content = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "7.0");
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_exact_version() {
        let content = r#"gem 'bcrypt', '3.1.7'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "bcrypt");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_pessimistic_constraint() {
        let content = r#"gem 'puma', '~> 5.0'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "5.0");
    }

    #[test]
    fn test_parse_compound_constraints() {
        let content = r#"gem 'pg', '>= 0.18', '< 2.0'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pg");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_no_version() {
        let content = r#"gem 'some_gem'"#;
        let deps = parse(content).unwrap();
        // Gems without version should be skipped
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_double_quotes() {
        let content = r#"gem "rails", "~> 7.0""#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
    }

    #[test]
    fn test_parse_development_group() {
        let content = r#"
group :development do
  gem 'web-console', '>= 4.1.0'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "web-console");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_development_test_group() {
        let content = r#"
group :development, :test do
  gem 'rspec-rails', '~> 5.0'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_mixed_groups() {
        let content = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem 'pg', '~> 1.1'

group :development, :test do
  gem 'rspec-rails', '~> 5.0'
  gem 'factory_bot_rails', '~> 6.0'
end

group :development do
  gem 'web-console', '>= 4.1.0'
end

gem 'bcrypt', '~> 3.1.7'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 6);

        let prod_deps: Vec<_> = deps.iter().filter(|d| !d.is_dev).collect();
        let dev_deps: Vec<_> = deps.iter().filter(|d| d.is_dev).collect();

        assert_eq!(prod_deps.len(), 3); // rails, pg, bcrypt
        assert_eq!(dev_deps.len(), 3); // rspec-rails, factory_bot_rails, web-console
    }

    #[test]
    fn test_parse_with_comments() {
        let content = r#"
# This is a comment
gem 'rails', '~> 7.0' # inline comment
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_parse_empty() {
        let deps = parse("").unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_source_only() {
        let content = r#"source 'https://rubygems.org'"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_update_version() {
        let content = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem 'pg', '~> 1.1'
"#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert!(result.contains("'~> 7.1.0'"));
        assert!(result.contains("gem 'pg'")); // Other gems unchanged
    }

    #[test]
    fn test_update_version_exact() {
        let content = r#"gem 'bcrypt', '3.1.7'"#;
        let result = GemfileParser
            .update_version(content, "bcrypt", "3.1.18")
            .unwrap();
        assert!(result.contains("'3.1.18'"));
    }

    #[test]
    fn test_update_version_maintains_format() {
        let content = r#"gem 'puma', '>= 5.0'"#;
        let result = GemfileParser
            .update_version(content, "puma", "6.0")
            .unwrap();
        assert!(result.contains("'>= 6.0'"));
    }

    #[test]
    fn test_update_version_double_quotes() {
        let content = r#"gem "rails", "~> 7.0""#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert!(result.contains("\"~> 7.1.0\""));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"gem 'rails', '~> 7.0'"#;
        let result = GemfileParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_gemfile_parser_language() {
        let parser = GemfileParser;
        assert_eq!(parser.language(), Language::Ruby);
    }

    #[test]
    fn test_parse_gem_with_options() {
        let content = r#"gem 'rails', '~> 7.0', require: false"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.version, "7.0");
    }

    #[test]
    fn test_parse_gem_with_git_source() {
        // Gems with git source should be parsed if they have a version
        let content = r#"gem 'rails', '~> 7.0', git: 'https://github.com/rails/rails'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_parse_gte_constraint() {
        let content = r#"gem 'web-console', '>= 4.1.0'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GreaterOrEqual);
    }

    #[test]
    fn test_format_updated_maintains_prefix() {
        let content = r#"gem 'rails', '~> 7.0'"#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1")
            .unwrap();
        assert!(result.contains("'~> 7.1'"));
    }
}
