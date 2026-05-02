//! Go プロジェクト向けの `go.mod` パーサ。
//!
//! 対応対象:
//! - require 文 (単一行およびブロック)
//! - `// pinned` コメントによるバージョン固定
//! - replace ディレクティブ (パースと更新の両方でスキップ)
//! - exclude ディレクティブ (パースと更新の両方でスキップ)

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::{VersionParser, get_parser};
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// `go.mod` 用パーサ
pub struct GoModParser;

// 単一 require 文の正規表現: require module/path v1.2.3
static SINGLE_REQUIRE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*require\s+(\S+)\s+(v[\d]+\.[\d]+\.[\d]+[^\s]*)\s*(//.*)?\s*$").unwrap()
});

// require ブロック内エントリの正規表現: module/path v1.2.3
static BLOCK_ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(\S+)\s+(v[\d]+\.[\d]+\.[\d]+[^\s]*)\s*(//.*)?\s*$").unwrap()
});

// pinned コメントの正規表現
static PINNED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//\s*pinned").unwrap());

impl ManifestParser for GoModParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Go);

        let mut in_require_block = false;
        let mut in_replace_block = false;

        for line in content.lines() {
            let trimmed = line.trim();
            let logical = trimmed.split("//").next().unwrap_or("").trim();

            // 空行とコメントをスキップする
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }

            // ブロックの開始/終了を確認する
            if logical.starts_with("require (") || logical == "require (" {
                in_require_block = true;
                continue;
            }

            if logical.starts_with("replace (") || logical == "replace (" {
                in_replace_block = true;
                continue;
            }

            if logical == ")" {
                in_require_block = false;
                in_replace_block = false;
                continue;
            }

            // replace ブロックはローカルオーバーライドなのでスキップする
            if in_replace_block || trimmed.starts_with("replace ") {
                continue;
            }

            // pinned コメントを確認する
            let is_pinned = PINNED_RE.is_match(line);

            // 単一 require 文をパースする
            if let Some(caps) = SINGLE_REQUIRE_RE.captures(trimmed) {
                if let Some(dep) = parse_go_dependency(&caps, parser.as_ref(), is_pinned) {
                    dependencies.push(dep);
                }
                continue;
            }

            // require ブロック内エントリをパースする
            if in_require_block
                && let Some(caps) = BLOCK_ENTRY_RE.captures(trimmed)
                && let Some(dep) = parse_go_dependency(&caps, parser.as_ref(), is_pinned)
            {
                dependencies.push(dep);
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Go
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let mut result = String::new();
        let mut updated = false;
        let mut in_replace_block = false;
        let mut in_exclude_block = false;

        // バージョンに v プレフィックスを付ける
        let new_ver = if new_version.starts_with('v') {
            new_version.to_string()
        } else {
            format!("v{}", new_version)
        };

        for line in content.lines() {
            let trimmed = line.trim();
            let logical = trimmed.split("//").next().unwrap_or("").trim();

            // replace ブロックの開始/終了を追跡する
            if logical.starts_with("replace (") || logical == "replace (" {
                in_replace_block = true;
            } else if logical.starts_with("exclude (") || logical == "exclude (" {
                in_exclude_block = true;
            } else if (in_replace_block || in_exclude_block) && logical == ")" {
                in_replace_block = false;
                in_exclude_block = false;
            }

            // replace/exclude ブロック内および単一行 replace/exclude は更新対象外
            let in_replace = in_replace_block || trimmed.starts_with("replace ");
            let in_exclude = in_exclude_block || trimmed.starts_with("exclude ");

            // この行に対象パッケージが含まれているか確認する
            let updated_line = if !in_replace && !in_exclude && trimmed.contains(package) {
                // 単一 require 文とのマッチを試みる
                if let Some(caps) = SINGLE_REQUIRE_RE.captures(trimmed) {
                    let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    if module == package {
                        let comment = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                        let new_line = if comment.is_empty() {
                            format!("require {} {}", package, new_ver)
                        } else {
                            format!("require {} {} {}", package, new_ver, comment)
                        };
                        updated = true;
                        Some(new_line)
                    } else {
                        None
                    }
                } else if let Some(caps) = BLOCK_ENTRY_RE.captures(trimmed) {
                    // ブロックエントリとのマッチを試みる
                    let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    if module == package {
                        let comment = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                        // 先頭の空白を保持する
                        let leading_ws = line.len() - line.trim_start().len();
                        let indent = &line[..leading_ws];
                        let new_line = if comment.is_empty() {
                            format!("{}{} {}", indent, package, new_ver)
                        } else {
                            format!("{}{} {} {}", indent, package, new_ver, comment)
                        };
                        updated = true;
                        Some(new_line)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(new_line) = updated_line {
                result.push_str(&new_line);
            } else {
                result.push_str(line);
            }
            result.push('\n');
        }

        // 元のファイルに末尾改行がなければ除去する
        if !content.ends_with('\n') && result.ends_with('\n') {
            result.pop();
        }

        if updated {
            Ok(result)
        } else {
            Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("go.mod"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })
        }
    }
}

fn parse_go_dependency(
    caps: &regex::Captures,
    parser: &dyn VersionParser,
    is_pinned: bool,
) -> Option<Dependency> {
    let module = caps.get(1)?.as_str();
    let version = caps.get(2)?.as_str();

    // 間接依存をスキップする (通常 // indirect コメントが付いている)
    let comment = caps.get(3).map(|m| m.as_str()).unwrap_or("");
    let is_indirect = comment.contains("indirect");

    let mut spec = parser.parse(version)?;

    // // pinned コメント付きの場合、GoPinned として扱い更新対象から除外する
    if is_pinned {
        use crate::domain::{VersionSpec, VersionSpecKind};
        let mut pinned_spec = VersionSpec::new(
            VersionSpecKind::GoPinned,
            spec.raw.clone(),
            spec.version.clone(),
        );
        if let Some(ref prefix) = spec.prefix {
            pinned_spec = pinned_spec.with_prefix(prefix.clone());
        }
        if let Some(ref suffix) = spec.suffix {
            pinned_spec = pinned_spec.with_suffix(suffix.clone());
        }
        spec = pinned_spec;
    }

    let dep = if is_indirect {
        Dependency::development(module, spec, Language::Go)
    } else {
        Dependency::production(module, spec, Language::Go)
    };

    Some(dep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        GoModParser.parse(content)
    }

    #[test]
    fn test_parse_single_require() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
        assert_eq!(deps[0].version_spec.version, "1.9.1");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_require_block() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/stretchr/testify v1.8.4
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let gin = deps
            .iter()
            .find(|d| d.name == "github.com/gin-gonic/gin")
            .unwrap();
        assert_eq!(gin.version_spec.version, "1.9.1");

        let testify = deps
            .iter()
            .find(|d| d.name == "github.com/stretchr/testify")
            .unwrap();
        assert_eq!(testify.version_spec.version, "1.8.4");
    }

    #[test]
    fn test_parse_block_close_with_comment() {
        let content = r#"
module example.com/myproject

replace (
	github.com/example/old => ../old
) // local replacements

require (
	github.com/gin-gonic/gin v1.9.1
) // direct deps
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
    }

    #[test]
    fn test_parse_indirect_dependencies() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	golang.org/x/text v0.14.0 // indirect
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let gin = deps
            .iter()
            .find(|d| d.name == "github.com/gin-gonic/gin")
            .unwrap();
        assert!(!gin.is_dev);

        let text = deps.iter().find(|d| d.name == "golang.org/x/text").unwrap();
        assert!(text.is_dev); // indirect は開発依存として扱う
    }

    #[test]
    fn test_parse_pinned() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/critical/lib v1.0.0 // pinned
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        // // pinned コメント付きは GoPinned として扱われる
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GoPinned);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_with_replace() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

replace github.com/gin-gonic/gin => ../local-gin
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
    }

    #[test]
    fn test_parse_replace_block() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

replace (
	github.com/gin-gonic/gin => ../local-gin
	github.com/other/lib => ../other-lib
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_parse_prerelease_version() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/pkg/errors v0.9.1-beta.1
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].version_spec.version.contains("beta"));
    }

    #[test]
    fn test_parse_incompatible() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/old/module v2.0.0+incompatible
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].version_spec.raw.contains("+incompatible"));
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"
module example.com/myproject

go 1.21
"#;

        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_update_single_require() {
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(result.contains("v1.10.0"));
        assert!(!result.contains("v1.9.1"));
    }

    #[test]
    fn test_update_require_block() {
        let content = r#"module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/stretchr/testify v1.8.4
)
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(result.contains("v1.10.0"));
        assert!(result.contains("v1.8.4")); // 他の依存は変更されない
    }

    #[test]
    fn test_update_preserves_comment() {
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1 // some comment
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(result.contains("v1.10.0"));
        assert!(result.contains("// some comment"));
    }

    #[test]
    fn test_update_adds_v_prefix() {
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "1.10.0")
            .unwrap();
        assert!(result.contains("v1.10.0"));
    }

    #[test]
    fn test_update_not_found() {
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1
"#;

        let result = GoModParser.update_version(content, "github.com/nonexistent", "v1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_language() {
        assert_eq!(GoModParser.language(), Language::Go);
    }

    #[test]
    fn test_parse_ignores_exclude() {
        // exclude ディレクティブは依存関係としてパースされないこと
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

exclude github.com/bad/pkg v1.2.3
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
    }

    #[test]
    fn test_parse_ignores_exclude_block() {
        // exclude ブロックは依存関係としてパースされないこと
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

exclude (
	github.com/bad/pkg v1.2.3
	github.com/old/module v0.1.0
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
    }

    #[test]
    fn test_parse_ignores_retract() {
        // retract ディレクティブは依存関係としてパースされないこと
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

retract v1.0.0
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
    }

    #[test]
    fn test_parse_mixed_directives() {
        // require 以外のディレクティブはすべて無視されるべき
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/stretchr/testify v1.8.4
)

replace github.com/foo/bar => ../bar

exclude github.com/bad/pkg v1.2.3

retract (
	v1.0.0
	[v1.1.0, v1.2.0]
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_parse_pinned_in_require_block() {
        // require ブロック内の // pinned コメントも GoPinned として扱われる
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/critical/lib v1.0.0 // pinned
	github.com/stretchr/testify v1.8.4
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let critical = deps
            .iter()
            .find(|d| d.name == "github.com/critical/lib")
            .unwrap();
        assert_eq!(critical.version_spec.kind, VersionSpecKind::GoPinned);
        assert!(critical.is_pinned());

        // 他の依存は通常の Exact
        let gin = deps
            .iter()
            .find(|d| d.name == "github.com/gin-gonic/gin")
            .unwrap();
        assert_eq!(gin.version_spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_update_preserves_pinned_comment() {
        // update_version は // pinned コメントを保持する
        let content = r#"module example.com/myproject

go 1.21

require github.com/critical/lib v1.0.0 // pinned
"#;

        let result = GoModParser
            .update_version(content, "github.com/critical/lib", "v2.0.0")
            .unwrap();
        assert!(result.contains("v2.0.0"));
        assert!(result.contains("// pinned"));
    }

    #[test]
    fn test_update_preserves_trailing_newline() {
        // 末尾改行を持つファイルは更新後も保持する
        let content =
            "module example.com/myproject\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.1\n";

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(result.ends_with('\n'));
        assert!(result.contains("v1.10.0"));
    }

    #[test]
    fn test_update_no_trailing_newline_when_original_lacks_it() {
        // 末尾改行がないファイルは更新後も付けない
        let content =
            "module example.com/myproject\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.1";

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(!result.ends_with('\n'));
        assert!(result.contains("v1.10.0"));
    }

    #[test]
    fn test_parse_require_with_tabs_and_spaces() {
        // タブとスペースが混在しても処理できること
        let content = "module example.com/myproject\n\ngo 1.21\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.1\n    github.com/pkg/errors v0.9.1\n)";

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_parse_indirect_dependency() {
        // 単一 require 文の // indirect コメント付き依存が dev=true として分類されること
        let content = r#"
module example.com/myproject

go 1.21

require golang.org/x/sys v0.15.0 // indirect
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "golang.org/x/sys");
        assert_eq!(deps[0].version_spec.version, "0.15.0");
        assert!(
            deps[0].is_dev,
            "// indirect 付き依存は開発依存として扱われるべき"
        );
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_incompatible_suffix() {
        // +incompatible サフィックスがパース後も suffix フィールドに保持されること
        let content = r#"
module example.com/myproject

go 1.21

require github.com/old/module v2.0.0+incompatible
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/old/module");
        assert_eq!(deps[0].version_spec.version, "2.0.0");
        assert_eq!(
            deps[0].version_spec.suffix,
            Some("+incompatible".to_string()),
            "+incompatible サフィックスが suffix フィールドに保持されるべき"
        );
        assert_eq!(deps[0].version_spec.prefix, Some("v".to_string()));
    }

    #[test]
    fn test_parse_pseudo_version() {
        // pseudo-version (v0.0.0-YYYYMMDDHHmmss-hash) が正しくパースされること
        let content = r#"
module example.com/myproject

go 1.21

require golang.org/x/tools v0.0.0-20210101120000-abcdef123456
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "golang.org/x/tools");
        assert_eq!(
            deps[0].version_spec.version,
            "0.0.0-20210101120000-abcdef123456"
        );
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.prefix, Some("v".to_string()));
    }

    #[test]
    fn test_update_skips_replace_block() {
        // replace ブロック内のパッケージは更新されないこと
        let content = r#"module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/other/pkg v1.0.0
)

replace (
	github.com/gin-gonic/gin => github.com/fork/gin v1.9.1
	github.com/other/pkg => ../local-pkg
)
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        // require 内は更新される
        assert!(result.contains("github.com/gin-gonic/gin v1.10.0"));
        // replace 内は変更されない
        assert!(result.contains("github.com/fork/gin v1.9.1"));
    }

    #[test]
    fn test_update_skips_single_replace() {
        // 単一行 replace は更新されないこと
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

replace github.com/gin-gonic/gin => github.com/fork/gin v1.9.1
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        // require は更新される
        assert!(result.contains("require github.com/gin-gonic/gin v1.10.0"));
        // replace は変更されない
        assert!(result.contains("replace github.com/gin-gonic/gin => github.com/fork/gin v1.9.1"));
    }

    #[test]
    fn test_update_does_not_modify_exclude_line() {
        // exclude 行に同じパッケージ名が含まれていても更新されないこと
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

exclude github.com/gin-gonic/gin v1.9.0
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        // require は更新される
        assert!(result.contains("require github.com/gin-gonic/gin v1.10.0"));
        // exclude 行は変更されない
        assert!(result.contains("exclude github.com/gin-gonic/gin v1.9.0"));
    }

    #[test]
    fn test_update_does_not_modify_retract_line() {
        // retract 行がバージョンを含んでいても更新されないこと
        let content = r#"module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

retract v1.0.0
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(result.contains("require github.com/gin-gonic/gin v1.10.0"));
        assert!(result.contains("retract v1.0.0"));
    }

    #[test]
    fn test_update_does_not_modify_exclude_block() {
        // exclude ブロック内の同名パッケージも更新されないこと
        let content = r#"module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
)

exclude (
	github.com/gin-gonic/gin v1.8.0
	github.com/gin-gonic/gin v1.8.1
)
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        // require ブロック内は更新される
        assert!(result.contains("github.com/gin-gonic/gin v1.10.0"));
        // exclude ブロック内は変更されない
        assert!(result.contains("github.com/gin-gonic/gin v1.8.0"));
        assert!(result.contains("github.com/gin-gonic/gin v1.8.1"));
    }

    #[test]
    fn test_update_after_exclude_block_close_with_comment() {
        let content = r#"module example.com/myproject

exclude (
	github.com/gin-gonic/gin v1.8.0
) // excluded versions

require github.com/gin-gonic/gin v1.9.1
"#;

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(result.contains("require github.com/gin-gonic/gin v1.10.0"));
        assert!(result.contains("github.com/gin-gonic/gin v1.8.0"));
    }

    #[test]
    fn test_update_preserves_pinned_comment_in_block() {
        // require ブロック内の // pinned コメントが更新後も保持されること
        let content = r#"module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/critical/lib v1.0.0 // pinned
)
"#;

        let result = GoModParser
            .update_version(content, "github.com/critical/lib", "v2.0.0")
            .unwrap();
        assert!(result.contains("v2.0.0"), "バージョンが更新されるべき");
        assert!(
            result.contains("// pinned"),
            "// pinned コメントが保持されるべき"
        );
        // 他の依存は変更されないこと
        assert!(result.contains("v1.9.1"));
    }
}
