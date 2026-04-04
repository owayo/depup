//! Swift Package Manager プロジェクト向けの `Package.swift` パーサ。
//!
//! 対応対象:
//! - `.package(url:, from:)` / `.package(name:, url:, from:)` → Caret
//! - `.package(url:, .upToNextMajor(from:))` → Caret
//! - `.package(url:, .upToNextMinor(from:))` → Tilde
//! - `.package(url:, exact:)` / `.package(url:, .exact())` → Exact (固定)
//! - `.package(url:, "V1"..<"V2")` → Range
//! - `.package(url:, "V1"..."V2")` → Range
//! - `.package(path:)` → スキップ (ローカル依存)
//! - `branch:` / `revision:` / `.branch()` / `.revision()` → スキップ (バージョンなし)
//! - コメント行 (`//`) はスキップ
//! - 複数行の `.package()` 宣言に対応

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// `Package.swift` 用パーサ
pub struct PackageSwiftParser;

/// オプションの `name:` パラメータ接頭辞 (Swift 5.2+ で追加、5.5+ で非推奨)
const NAME_OPT: &str = r#"(?:name:\s*"[^"]+"\s*,\s*)?"#;

// .package([name:,] url: "...", from: "VERSION") にマッチする
static FROM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*from:\s*"([^"]+)"\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", .upToNextMajor(from: "VERSION")) にマッチする
static UP_TO_NEXT_MAJOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*\.upToNextMajor\(\s*from:\s*"([^"]+)"\s*\)\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", .upToNextMinor(from: "VERSION")) にマッチする
static UP_TO_NEXT_MINOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*\.upToNextMinor\(\s*from:\s*"([^"]+)"\s*\)\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", exact: "VERSION") にマッチする
static EXACT_KEYWORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*exact:\s*"([^"]+)"\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", .exact("VERSION")) にマッチする
static EXACT_METHOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*\.exact\(\s*"([^"]+)"\s*\)\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", "V1"..<"V2") にマッチする — 半開区間
static RANGE_HALF_OPEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\.\.<\s*"([^"]+)"\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", "V1"..."V2") にマッチする — 閉区間
static RANGE_CLOSED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\.\.\.\s*"([^"]+)"\s*\)"#,
        NAME_OPT
    ))
    .unwrap()
});

/// GitHub URL から owner/repo を抽出する
///
/// 対応形式:
/// - https://github.com/owner/repo.git
/// - https://github.com/owner/repo
/// - git@github.com:owner/repo.git
fn extract_github_owner_repo(url: &str) -> Option<String> {
    // HTTPS URL パターン
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let path = rest.trim_end_matches(".git");
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Some(format!("{}/{}", parts[0], parts[1]));
        }
    }

    // SSH URL パターン
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let path = rest.trim_end_matches(".git");
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Some(format!("{}/{}", parts[0], parts[1]));
        }
    }

    None
}

/// パース用にコンテンツからコメント行を除去する
fn strip_comment_lines(content: &str) -> String {
    content
        .lines()
        .filter(|line| !line.trim().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl ManifestParser for PackageSwiftParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        // コメント行を除去し、複数行対応のため全体に対してマッチする
        let clean = strip_comment_lines(content);
        let mut found: Vec<(usize, Dependency)> = Vec::new();

        // より具体的なパターンを先に、汎用の FROM_RE を最後に試す
        for caps in UP_TO_NEXT_MAJOR_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let spec = VersionSpec::new(VersionSpecKind::Caret, version, version);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        for caps in UP_TO_NEXT_MINOR_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let spec = VersionSpec::new(VersionSpecKind::Tilde, version, version);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        for caps in EXACT_KEYWORD_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let spec = VersionSpec::new(VersionSpecKind::Exact, version, version);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        for caps in EXACT_METHOD_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let spec = VersionSpec::new(VersionSpecKind::Exact, version, version);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        for caps in RANGE_HALF_OPEN_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let lower = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let raw = format!("{}..<{}", lower, upper);
                let spec = VersionSpec::new(VersionSpecKind::Range, raw, lower);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        for caps in RANGE_CLOSED_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let lower = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let raw = format!("{}...{}", lower, upper);
                let spec = VersionSpec::new(VersionSpecKind::Range, raw, lower);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        // FROM_RE を最後に (最も汎用的なパターン)
        for caps in FROM_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(name) = extract_github_owner_repo(url) {
                let spec = VersionSpec::new(VersionSpecKind::Caret, version, version);
                found.push((pos, Dependency::production(name, spec, Language::Swift)));
            }
        }

        // 元の順序を保つため位置でソートする
        found.sort_by_key(|(pos, _)| *pos);

        // パッケージ名で重複を除去する
        let mut seen = std::collections::HashSet::new();
        let dependencies = found
            .into_iter()
            .filter(|(_, dep)| seen.insert(dep.name.clone()))
            .map(|(_, dep)| dep)
            .collect();

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Swift
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let escaped_package = regex::escape(package);
        let url_pattern = format!(r#"github\.com[/:]{}(?:\.git)?"#, escaped_package);

        let url_re = Regex::new(&url_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Package.swift"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let version_re =
            Regex::new(r#""(\d+(?:\.\d+)*)""#).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        // 全体から URL を検索する (複数行宣言に対応)
        let url_match = url_re
            .find(content)
            .ok_or_else(|| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })?;

        // URL がコメント行にあるかどうかを確認する
        let line_start = content[..url_match.start()]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let line_prefix = content[line_start..url_match.start()].trim_start();
        if line_prefix.starts_with("//") {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        // 囲む .package() 宣言を見つける
        let prefix = &content[..url_match.start()];
        let pkg_start =
            prefix
                .rfind(".package(")
                .ok_or_else(|| ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("Package.swift"),
                    spec: package.to_string(),
                    message: "package not found or version could not be updated".to_string(),
                })?;

        // .package( から URL 末尾までの括弧深度を数える
        let mut depth: i32 = 0;
        for c in content[pkg_start..url_match.end()].chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }

        if depth <= 0 {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        // 対応する閉じ括弧を見つける
        let mut end_pos = content.len();
        for (i, c) in content[url_match.end()..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end_pos = url_match.end() + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        // URL 末尾からパッケージ宣言末尾までのバージョン文字列を置換する
        let before = &content[..url_match.end()];
        let version_section = &content[url_match.end()..end_pos];
        let after = &content[end_pos..];

        let mut updated = false;
        // replace (replace_all ではなく) を使い、最初のバージョンのみ置換する。
        // レンジ構文 ("1.0.0"..<"2.0.0") で上限まで置換されるのを防ぐ。
        let new_section = version_re.replace(version_section, |_caps: &regex::Captures| {
            updated = true;
            format!("\"{}\"", new_version)
        });

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        Ok(format!("{}{}{}", before, new_section, after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        PackageSwiftParser.parse(content)
    }

    #[test]
    fn test_parse_from_version() {
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_up_to_next_major() {
        let content =
            r#".package(url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.0.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "4.0.0");
    }

    #[test]
    fn test_parse_up_to_next_minor() {
        let content =
            r#".package(url: "https://github.com/vapor/vapor.git", .upToNextMinor(from: "4.5.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "4.5.0");
    }

    #[test]
    fn test_parse_exact_keyword() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_exact_method() {
        let content =
            r#".package(url: "https://github.com/apple/swift-nio.git", .exact("2.40.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_range_half_open() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", "4.0.0"..<"5.0.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.version, "4.0.0");
    }

    #[test]
    fn test_parse_range_closed() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", "4.0.0"..."4.9.9")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_skip_branch() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", branch: "main")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_revision() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", revision: "abc123")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_comments() {
        let content = r#"
        // .package(url: "https://github.com/vapor/vapor.git", from: "4.0.0")
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.0.0")
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
    }

    #[test]
    fn test_skip_non_github_url() {
        let content = r#".package(url: "https://gitlab.com/some/repo.git", from: "1.0.0")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_multiple_dependencies() {
        let content = r#"
let package = Package(
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
        .package(url: "https://github.com/vapor/vapor.git", from: "4.0.0"),
        .package(url: "https://github.com/apple/swift-nio.git", .upToNextMinor(from: "2.40.0")),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[1].name, "vapor/vapor");
        assert_eq!(deps[2].name, "apple/swift-nio");
    }

    #[test]
    fn test_parse_empty() {
        let deps = parse("").unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_extract_github_owner_repo_https() {
        assert_eq!(
            extract_github_owner_repo("https://github.com/apple/swift-argument-parser.git"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_https_no_git() {
        assert_eq!(
            extract_github_owner_repo("https://github.com/apple/swift-argument-parser"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_ssh() {
        assert_eq!(
            extract_github_owner_repo("git@github.com:apple/swift-argument-parser.git"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_non_github() {
        assert_eq!(
            extract_github_owner_repo("https://gitlab.com/some/repo.git"),
            None
        );
    }

    #[test]
    fn test_update_version_from() {
        let content = r#"
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
        .package(url: "https://github.com/vapor/vapor.git", from: "4.0.0"),
"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-argument-parser", "1.3.0")
            .unwrap();
        assert!(result.contains(r#"from: "1.3.0""#));
        // 他のパッケージは変更されない
        assert!(result.contains(r#"from: "4.0.0""#));
    }

    #[test]
    fn test_update_version_exact() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.41.0")
            .unwrap();
        assert!(result.contains(r#"exact: "2.41.0""#));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#;
        let result = PackageSwiftParser.update_version(content, "nonexistent/repo", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_language() {
        assert_eq!(PackageSwiftParser.language(), Language::Swift);
    }

    #[test]
    fn test_parse_url_without_git_extension() {
        let content = r#".package(url: "https://github.com/apple/swift-log", from: "1.0.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-log");
    }

    // --- name: パラメータのサポート ---

    #[test]
    fn test_parse_with_name_parameter_from() {
        let content = r#".package(name: "ArgumentParser", url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_with_name_up_to_next_major() {
        let content = r#".package(name: "Vapor", url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.0.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_with_name_exact() {
        let content = r#".package(name: "SwiftNIO", url: "https://github.com/apple/swift-nio.git", exact: "2.40.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    // --- path: 依存 (スキップされるべき) ---

    #[test]
    fn test_skip_path_dependency() {
        let content = r#".package(path: "../some-local-package")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_path_dependency_with_name() {
        let content = r#".package(name: "LocalLib", path: "../local-lib")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    // --- .branch() / .revision() メソッド構文 ---

    #[test]
    fn test_skip_branch_method_syntax() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", .branch("main"))"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_revision_method_syntax() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", .revision("abc123"))"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    // --- 複数行宣言 ---

    #[test]
    fn test_parse_multiline_from() {
        let content = ".package(\n    url: \"https://github.com/apple/swift-argument-parser.git\",\n    from: \"1.2.0\"\n)";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_multiline_up_to_next_major() {
        let content = ".package(\n    url: \"https://github.com/vapor/vapor.git\",\n    .upToNextMajor(from: \"4.0.0\")\n)";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_multiline_with_name() {
        let content = ".package(\n    name: \"ArgumentParser\",\n    url: \"https://github.com/apple/swift-argument-parser.git\",\n    from: \"1.2.0\"\n)";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_update_version_multiline() {
        let content = ".package(\n    url: \"https://github.com/apple/swift-argument-parser.git\",\n    from: \"1.2.0\"\n)";
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-argument-parser", "1.3.0")
            .unwrap();
        assert!(result.contains("from: \"1.3.0\""));
        assert!(result.contains(".package(\n"));
    }

    #[test]
    fn test_update_version_with_name_parameter() {
        let content = r#".package(name: "ArgumentParser", url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-argument-parser", "1.3.0")
            .unwrap();
        assert!(result.contains(r#"from: "1.3.0""#));
        assert!(result.contains(r#"name: "ArgumentParser""#));
    }

    // --- 実運用に近い Package.swift ---

    #[test]
    fn test_update_version_range_preserves_upper_bound() {
        // レンジ構文で上限が誤って置換されないことを確認
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio.git", "1.0.0"..<"2.0.0"),
    ]
)
"#;

        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "1.5.0")
            .unwrap();
        assert!(result.contains(r#""1.5.0"..<"2.0.0""#));
    }

    #[test]
    fn test_parse_realistic_package_swift() {
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    platforms: [
        .macOS(.v13),
        .iOS(.v16)
    ],
    dependencies: [
        .package(
            url: "https://github.com/apple/swift-argument-parser.git",
            from: "1.2.0"
        ),
        .package(url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.89.0")),
        .package(name: "SwiftNIO", url: "https://github.com/apple/swift-nio.git", exact: "2.40.0"),
        // .package(url: "https://github.com/old/dep.git", from: "0.1.0"),
        .package(url: "https://github.com/grpc/grpc-swift.git", branch: "main"),
        .package(path: "../my-local-lib"),
    ],
    targets: [
        .target(name: "MyApp", dependencies: [
            .product(name: "ArgumentParser", package: "swift-argument-parser"),
            .product(name: "Vapor", package: "vapor"),
        ]),
    ]
)"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
        assert_eq!(deps[1].name, "vapor/vapor");
        assert_eq!(deps[1].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[1].version_spec.version, "4.89.0");
        assert_eq!(deps[2].name, "apple/swift-nio");
        assert_eq!(deps[2].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[2].version_spec.version, "2.40.0");
    }

    // --- 追加エッジケーステスト ---

    #[test]
    fn test_parse_url_without_dot_git_from() {
        // .git 拡張なしの URL でも正しくパースされること
        let content = r#".package(url: "https://github.com/owner/repo", from: "1.0.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "owner/repo");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_v_prefix_in_version() {
        // v プレフィックス付きバージョン。from: に指定された文字列がそのまま保持される
        let content = r#".package(url: "https://github.com/owner/repo.git", from: "v1.0.0")"#;
        let deps = parse(content).unwrap();
        // v プレフィックスはバージョン正規表現 (\d+...) にマッチしないため、
        // パースはされるがバージョン文字列は "v1.0.0" として保持される
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "owner/repo");
        assert_eq!(deps[0].version_spec.version, "v1.0.0");
    }

    #[test]
    fn test_parse_multiple_mixed_constraint_types() {
        // 複数の依存関係が異なる制約タイプを使用する Package.swift
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MixedDeps",
    dependencies: [
        .package(url: "https://github.com/apple/swift-log", from: "1.5.0"),
        .package(url: "https://github.com/vapor/vapor.git", .upToNextMinor(from: "4.89.0")),
        .package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0"),
        .package(url: "https://github.com/swift-server/async-http-client.git", "1.0.0"..<"2.0.0"),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        // from: → Caret
        let swift_log = deps.iter().find(|d| d.name == "apple/swift-log").unwrap();
        assert_eq!(swift_log.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(swift_log.version_spec.version, "1.5.0");

        // upToNextMinor → Tilde
        let vapor = deps.iter().find(|d| d.name == "vapor/vapor").unwrap();
        assert_eq!(vapor.version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(vapor.version_spec.version, "4.89.0");

        // exact: → Exact
        let nio = deps.iter().find(|d| d.name == "apple/swift-nio").unwrap();
        assert_eq!(nio.version_spec.kind, VersionSpecKind::Exact);

        // ..<  → Range
        let http_client = deps
            .iter()
            .find(|d| d.name == "swift-server/async-http-client")
            .unwrap();
        assert_eq!(http_client.version_spec.kind, VersionSpecKind::Range);
        assert_eq!(http_client.version_spec.version, "1.0.0");
    }
}
