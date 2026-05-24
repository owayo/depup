//! Ruby プロジェクト向けの `Gemfile` パーサ。
//!
//! 対応対象:
//! - バージョン制約付き `gem` 宣言
//! - 開発グループ依存関係
//! - ペシミスティック制約 (`~>`)
//! - 複数バージョン制約の解析
//! - シングルクォートとダブルクォート

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::get_parser;
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// `Gemfile` 用パーサ
pub struct GemfileParser;

enum GemfileBlock {
    Group(bool),
    Other,
}

// `gem 'name'` / `gem "name"` / `gem('name')` を解釈する正規表現
static GEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 例:
    // gem 'rails', '~> 7.0'
    // gem "pg", ">= 0.18", "< 2.0"
    // gem("rack", "~> 3.0")
    // gem 'bcrypt'
    Regex::new(
        r#"^\s*gem(?:\s+|\s*\(\s*)['"]([^'"]+)['"](?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,|\s*\)?\s*$|\s*\)?\s*#)"#,
    )
    .unwrap()
});

// `group ... do` 開始行
static GROUP_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 例:
    // group :development do
    // group :development, :test do
    // group :development do # security gems  <- 行末コメントも許容する
    Regex::new(r"^\s*group\s+(.+?)\s+do\s*(?:#.*)?$").unwrap()
});

// `group` ブロック終端
static GROUP_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*end\s*(?:#.*)?$").unwrap());

// `do` で終わるブロック開始行（group 以外）
static DO_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bdo\s*(?:#.*)?$").unwrap());

// 開発用グループかどうかを判定する
fn is_dev_group(group_line: &str) -> bool {
    let lowered = group_line.to_lowercase();
    // `:development` と `:test` を開発系として扱う
    lowered.contains(":development") || lowered.contains(":test")
}

fn has_dev_group_option(line: &str) -> bool {
    let lowered = line.to_lowercase();
    let has_group_option = lowered.contains("group:")
        || lowered.contains("groups:")
        || lowered.contains(":group =>")
        || lowered.contains(":groups =>");
    has_group_option
        && (lowered.contains(":development")
            || lowered.contains(":test")
            || lowered.contains("\"development\"")
            || lowered.contains("\"test\"")
            || lowered.contains("'development'")
            || lowered.contains("'test'"))
}

fn has_non_registry_source(line: &str) -> bool {
    let lowered = line.to_lowercase();
    lowered.contains("git:")
        || lowered.contains("github:")
        || lowered.contains("bitbucket:")
        || lowered.contains("gist:")
        || lowered.contains("path:")
        || lowered.contains("source:")
}

impl ManifestParser for GemfileParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Ruby);
        let mut block_stack = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();

            // 空行とコメントは無視する
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // `group ... do` を積む
            if let Some(caps) = GROUP_START_RE.captures(trimmed) {
                let group_spec = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                block_stack.push(GemfileBlock::Group(is_dev_group(group_spec)));
                continue;
            }

            // group 以外の `do` ブロック（platforms, source 等）を追跡する
            if DO_BLOCK_RE.is_match(trimmed) {
                block_stack.push(GemfileBlock::Other);
                continue;
            }

            // 対応する `end` で適切なスタック/カウンタを戻す
            if GROUP_END_RE.is_match(trimmed) {
                block_stack.pop();
                continue;
            }

            // `gem` 宣言を解釈する
            if let Some(caps) = GEM_RE.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

                if name.is_empty() {
                    continue;
                }

                // 最大 3 個までのバージョン制約を回収する
                let mut version_parts = Vec::new();
                for i in 2..=4 {
                    if let Some(v) = caps.get(i) {
                        version_parts.push(v.as_str().to_string());
                    }
                }

                // バージョン指定から `VersionSpec` を組み立てる
                let spec = if version_parts.is_empty() {
                    if has_non_registry_source(line) {
                        continue;
                    }
                    // バージョン指定がなければ `Any`
                    VersionSpec::new(VersionSpecKind::Any, "", "")
                } else {
                    let version_str = version_parts.join(", ");
                    match parser.parse(&version_str) {
                        Some(s) => s,
                        None => continue,
                    }
                };

                let dep = if block_stack
                    .iter()
                    .any(|block| matches!(block, GemfileBlock::Group(true)))
                    || has_dev_group_option(line)
                {
                    Dependency::development(name, spec, Language::Ruby)
                } else {
                    Dependency::production(name, spec, Language::Ruby)
                };
                dependencies.push(dep);
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
        let escaped_name = regex::escape(package);
        let mut updated = false;
        let no_version_pattern = format!(
            r#"(gem(?:\s+|\s*\(\s*))(['"])({escaped_name})(['"])(\s*(?:(?:\)\s*)?(?:#|$)|,\s*(?:require|group|git|path|branch|ref|tag|source|platforms?)\s*:))"#
        );

        let no_version_re =
            Regex::new(&no_version_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Gemfile"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let simple_pattern =
            format!(r#"(gem(?:\s+|\s*\(\s*))(['"])({escaped_name})(['"])(\s*\)?\s*)$"#);

        let simple_re =
            Regex::new(&simple_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Gemfile"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let mut lines = Vec::new();
        for line in content.lines() {
            if !updated
                && let Some(caps) = GEM_RE.captures(line)
                && caps.get(1).map(|m| m.as_str()) == Some(package)
            {
                let mut version_parts = Vec::new();
                for i in 2..=4 {
                    if let Some(m) = caps.get(i) {
                        version_parts.push(m);
                    }
                }

                match version_parts.len() {
                    0 => {
                        if has_non_registry_source(line) {
                            lines.push(line.to_string());
                            continue;
                        }

                        if let Some(caps) = no_version_re.captures(line) {
                            let gem_keyword = &caps[1];
                            let quote_start = &caps[2];
                            let name = &caps[3];
                            let quote_end = &caps[4];
                            let suffix = &caps[5];
                            let matched_range = caps.get(0).unwrap().range();
                            updated = true;
                            let inserted = format!(
                                "{}{}{}{}, {}{}{}{}",
                                gem_keyword,
                                quote_start,
                                name,
                                quote_end,
                                quote_start,
                                new_version,
                                quote_end,
                                suffix
                            );
                            let mut updated_line =
                                String::with_capacity(line.len() + inserted.len() + 8);
                            updated_line.push_str(&line[..matched_range.start]);
                            updated_line.push_str(&inserted);
                            updated_line.push_str(&line[matched_range.end..]);
                            lines.push(updated_line);
                            continue;
                        }

                        if let Some(caps) = simple_re.captures(line) {
                            let gem_keyword = &caps[1];
                            let quote_start = &caps[2];
                            let name = &caps[3];
                            let quote_end = &caps[4];
                            let trailing = &caps[5];
                            updated = true;
                            lines.push(format!(
                                "{}{}{}{}, {}{}{}{}",
                                gem_keyword,
                                quote_start,
                                name,
                                quote_end,
                                quote_start,
                                new_version,
                                quote_end,
                                trailing
                            ));
                            continue;
                        }
                    }
                    1 => {
                        let old_version = version_parts[0].as_str();
                        if let Some(spec) = parser.parse(old_version) {
                            if spec.kind == VersionSpecKind::Range {
                                return Err(ManifestError::InvalidVersionSpec {
                                    path: PathBuf::from("Gemfile"),
                                    spec: package.to_string(),
                                    message: "複合制約や除外制約は安全に書き換えられません"
                                        .to_string(),
                                });
                            }

                            let Some(new_ver) = spec.try_format_updated(new_version) else {
                                return Err(ManifestError::InvalidVersionSpec {
                                    path: PathBuf::from("Gemfile"),
                                    spec: package.to_string(),
                                    message: "この制約は安全に書き換えられません".to_string(),
                                });
                            };
                            let version_range = version_parts[0].range();
                            let mut updated_line = String::with_capacity(
                                line.len() - old_version.len() + new_ver.len(),
                            );
                            updated_line.push_str(&line[..version_range.start]);
                            updated_line.push_str(&new_ver);
                            updated_line.push_str(&line[version_range.end..]);
                            updated = true;
                            lines.push(updated_line);
                            continue;
                        }
                    }
                    _ => {
                        return Err(ManifestError::InvalidVersionSpec {
                            path: PathBuf::from("Gemfile"),
                            spec: package.to_string(),
                            message: "複合バージョン制約は安全に書き換えられません".to_string(),
                        });
                    }
                }
            }

            lines.push(line.to_string());
        }

        if updated {
            let mut joined = lines.join("\n");
            // 元のファイルが末尾改行を持つ場合は保持する
            if content.ends_with('\n') {
                joined.push('\n');
            }
            return Ok(joined);
        }

        Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Gemfile"),
            spec: package.to_string(),
            message: "gem not found or version could not be updated".to_string(),
        })
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
    fn test_parse_parenthesized_gem() {
        let content = r#"gem("rack", "~> 3.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rack");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "3.0");
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
        // バージョンなしの gem は `Any` として扱う
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "some_gem");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_multiple_no_version() {
        let content = r#"
gem 'rmagick'
gem 'nokogiri'
gem 'playwright-ruby-client', '1.57.1'
gem 'websocket-driver'
gem 'rtesseract'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        // バージョンなしの gem を確認する
        assert_eq!(deps[0].name, "rmagick");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);

        assert_eq!(deps[1].name, "nokogiri");
        assert_eq!(deps[1].version_spec.kind, VersionSpecKind::Any);

        // バージョン付きの gem を確認する
        assert_eq!(deps[2].name, "playwright-ruby-client");
        assert_eq!(deps[2].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[2].version_spec.version, "1.57.1");

        assert_eq!(deps[3].name, "websocket-driver");
        assert_eq!(deps[3].version_spec.kind, VersionSpecKind::Any);

        assert_eq!(deps[4].name, "rtesseract");
        assert_eq!(deps[4].version_spec.kind, VersionSpecKind::Any);
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
    fn test_parse_nested_dev_group_does_not_leak() {
        let content = r#"
group :production do
  group :development do
    gem 'rubocop', '~> 1.0'
  end
  gem 'pg', '~> 1.1'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let rubocop = deps.iter().find(|dep| dep.name == "rubocop").unwrap();
        let pg = deps.iter().find(|dep| dep.name == "pg").unwrap();

        assert!(rubocop.is_dev);
        assert!(!pg.is_dev);
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

        assert_eq!(prod_deps.len(), 3); // rails, pg, bcrypt の 3 件
        assert_eq!(dev_deps.len(), 3); // 開発系 3 件
    }

    #[test]
    fn test_parse_with_comments() {
        let content = r#"
# コメント
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
        assert!(result.contains("gem 'pg'")); // 他の gem は変更しない
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
    fn test_update_version_parenthesized_gem() {
        let content = r#"gem("rack", "~> 3.0")"#;
        let result = GemfileParser
            .update_version(content, "rack", "3.1.0")
            .unwrap();
        assert_eq!(result, r#"gem("rack", "~> 3.1.0")"#);
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
        // git ソースでもバージョンがあれば解釈する
        let content = r#"gem 'rails', '~> 7.0', git: 'https://github.com/rails/rails'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_parse_unversioned_gem_with_git_source_skipped() {
        let content = r#"
gem 'rails', git: 'https://github.com/rails/rails'
gem 'pg', '~> 1.5'
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pg");
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

    #[test]
    fn test_update_version_add_to_unversioned_gem() {
        let content = r#"gem 'rmagick'"#;
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_double_quotes() {
        let content = r#"gem "nokogiri""#;
        let result = GemfileParser
            .update_version(content, "nokogiri", "1.16.0")
            .unwrap();
        assert!(result.contains("gem \"nokogiri\", \"1.16.0\""));
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_with_options() {
        let content = r#"gem 'my_gem', require: false"#;
        let result = GemfileParser
            .update_version(content, "my_gem", "1.0.0")
            .unwrap();
        assert!(result.contains("gem 'my_gem', '1.0.0', require: false"));
    }

    #[test]
    fn test_update_version_add_to_parenthesized_unversioned_gem() {
        let content = r#"gem("my_gem", require: false)"#;
        let result = GemfileParser
            .update_version(content, "my_gem", "1.0.0")
            .unwrap();
        assert_eq!(result, r#"gem("my_gem", "1.0.0", require: false)"#);
    }

    #[test]
    fn test_update_version_add_to_parenthesized_unversioned_gem_with_comment() {
        let content = r#"gem("my_gem") # comment"#;
        let result = GemfileParser
            .update_version(content, "my_gem", "1.0.0")
            .unwrap();
        assert_eq!(result, r#"gem("my_gem", "1.0.0") # comment"#);
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_in_multiline() {
        let content = r#"
gem 'rmagick'
gem 'nokogiri'
gem 'playwright-ruby-client', '1.57.1'
"#;
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
        // 他の gem は変更しない
        assert!(result.contains("gem 'nokogiri'"));
        assert!(result.contains("gem 'playwright-ruby-client', '1.57.1'"));
    }

    #[test]
    fn test_update_versionless_gem_preserves_trailing_newline() {
        // バージョンなし gem の更新で末尾改行を保持する
        let content = "gem 'rmagick'\ngem 'nokogiri'\n";
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_update_versionless_gem_no_trailing_newline() {
        // 末尾改行がないファイルは付けない
        let content = "gem 'rmagick'";
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
        assert!(!result.ends_with('\n'));
    }

    #[test]
    fn test_update_version_mixed_versioned_and_unversioned() {
        let content = r#"
gem 'rmagick'
gem 'rails', '~> 7.0'
gem 'nokogiri'
"#;
        // バージョン付き gem の更新
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert!(result.contains("'~> 7.1.0'"));

        // バージョンなし gem の更新
        let result2 = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result2.contains("gem 'rmagick', '5.3.0'"));
    }

    #[test]
    fn test_update_version_not_equal_constraint_returns_err() {
        let content = r#"gem 'rack', '!= 2.2.4'"#;
        let result = GemfileParser.update_version(content, "rack", "3.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_compound_constraint_returns_err() {
        let content = r#"gem 'pg', '>= 0.18', '< 2.0'"#;
        let result = GemfileParser.update_version(content, "pg", "1.5.0");
        assert!(result.is_err());
    }

    // --- 追加エッジケーステスト ---

    #[test]
    fn test_parse_four_segment_version() {
        // 4セグメントバージョン（例: loofah のパッチリリース）
        let content = r#"gem 'loofah', '2.22.0.1'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "loofah");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "2.22.0.1");
    }

    #[test]
    fn test_parse_prerelease_gem() {
        // プレリリースバージョンの gem をパースできること
        let content = r#"gem 'rails', '7.0.0.alpha'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.version, "7.0.0.alpha");
    }

    #[test]
    fn test_parse_double_quoted_pessimistic() {
        // ダブルクォート + ペシミスティック制約の組み合わせ
        let content = r#"gem "nokogiri", "~> 1.15""#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "nokogiri");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "1.15");
    }

    #[test]
    fn test_parse_gem_with_require_false_option() {
        // require: false オプション付きの gem がバージョン制約だけ解釈されること
        let content = r#"gem 'webpacker', '~> 5.0', require: false"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "webpacker");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "5.0");
    }

    #[test]
    fn test_parse_multiple_groups_on_one_line() {
        // 1行に複数グループを指定した group ブロック
        let content = r#"
group :development, :test do
  gem 'pry', '~> 0.14'
  gem 'faker', '~> 3.0'
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let pry = deps.iter().find(|d| d.name == "pry").unwrap();
        let faker = deps.iter().find(|d| d.name == "faker").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        // development, test グループ内の gem は開発依存
        assert!(pry.is_dev);
        assert!(faker.is_dev);
        // グループ外の gem は本番依存
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_nested_do_end_in_group() {
        // group 内の platforms do...end がグループスタックを壊さないこと
        let content = r#"
group :development do
  platforms :jruby do
    gem 'activerecord-jdbc-adapter', '~> 1.3'
  end
  gem 'web-console', '>= 4.1.0'
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let jdbc = deps
            .iter()
            .find(|d| d.name == "activerecord-jdbc-adapter")
            .unwrap();
        let console = deps.iter().find(|d| d.name == "web-console").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        // platforms ブロック内の gem も group :development 内なので開発依存
        assert!(jdbc.is_dev);
        // platforms の end でグループが外れてはいけない
        assert!(console.is_dev);
        // グループ外は本番依存
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_source_do_end_in_group() {
        // group 内の source do...end がグループスタックを壊さないこと
        let content = r#"
group :development do
  source 'https://gems.example.com' do
    gem 'private-gem', '~> 1.0'
  end
  gem 'debug', '~> 1.0'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let private = deps.iter().find(|d| d.name == "private-gem").unwrap();
        let debug = deps.iter().find(|d| d.name == "debug").unwrap();

        assert!(private.is_dev);
        assert!(debug.is_dev);
    }

    #[test]
    fn test_parse_group_inside_source_does_not_leak() {
        // source do の内側で閉じた group が、後続の gem に漏れないこと
        let content = r#"
source 'https://gems.example.com' do
  group :development do
    gem 'debug', '~> 1.0'
  end
  gem 'pg', '~> 1.5'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let debug = deps.iter().find(|d| d.name == "debug").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        assert!(debug.is_dev);
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_group_with_inline_comment() {
        // 回帰テスト: `group :development do # comment` のように
        // インラインコメントが付いていても開発グループとして認識されること
        let content = r#"
group :development do # security gems
  gem 'brakeman', '~> 6.0'
end

group :test, :development do  # bundler comment
  gem 'rspec', '~> 3.13'
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let brakeman = deps.iter().find(|d| d.name == "brakeman").unwrap();
        let rspec = deps.iter().find(|d| d.name == "rspec").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        // インラインコメント付きの group も開発依存と認識されること
        assert!(
            brakeman.is_dev,
            "brakeman should be dev (group has inline comment)"
        );
        assert!(
            rspec.is_dev,
            "rspec should be dev (group has trailing space + comment)"
        );
        // グループ外の gem は本番依存
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_gem_group_option_as_dev() {
        let content = r#"
gem 'rspec', '~> 3.0', group: :test
gem 'rubocop', '~> 1.0', groups: [:development, :test]
gem 'rails', '~> 7.0'
"#;

        let deps = parse(content).unwrap();
        let rspec = deps.iter().find(|d| d.name == "rspec").unwrap();
        let rubocop = deps.iter().find(|d| d.name == "rubocop").unwrap();
        let rails = deps.iter().find(|d| d.name == "rails").unwrap();
        assert!(rspec.is_dev);
        assert!(rubocop.is_dev);
        assert!(!rails.is_dev);
    }

    #[test]
    fn test_update_unversioned_git_source_is_skipped() {
        let content = r#"gem 'rails', git: 'https://github.com/rails/rails'"#;
        let result = GemfileParser.update_version(content, "rails", "7.1.0");
        assert!(result.is_err());
    }
}
