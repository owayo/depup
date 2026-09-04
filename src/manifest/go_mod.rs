//! Go プロジェクト向けの `go.mod` パーサ。
//!
//! 対応対象:
//! - require 文 (単一行およびブロック)
//! - `// pinned` コメントによるバージョン固定
//! - replace ディレクティブ (記述自体はパース・更新ともスキップし、
//!   さらに置換対象の `require` を更新候補から除外する)
//! - exclude ディレクティブ (更新候補から除外し、記述自体は書き換えない)

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::{ManifestParser, line_utils::split_line_ending};
use crate::parser::{VersionParser, get_parser};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::LazyLock;

/// `go.mod` 用パーサ
pub struct GoModParser;

// バージョン部の共通パターン: `v1.2.3` とその後続 (prerelease / build metadata)。
//
// 後続の `[^\s/]*` から `/` を除くのは、go の字句解析が識別子の途中でも `//` で
// 必ずコメントを切り出すため (x/mod modfile/read.go)。`[^\s]*` にすると
// `v1.0.0//indirect` のように `//` の前に空白が無い行でコメントまで飲み込み、
// 行全体が正規表現に一致せず依存が無言で取りこぼされていた
// (`v1.0.0 //indirect` は動くという非対称があった)。
const GO_VERSION_PATTERN: &str = r"v[\d]+\.[\d]+\.[\d]+[^\s/]*";

// 単一 require 文の正規表現: require module/path v1.2.3
// go.mod では ModulePath / Version ともに quoted string も許容される
static SINGLE_REQUIRE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^\s*require\s+("[^"]+"|`[^`]+`|\S+)\s+("[^"]+"|`[^`]+`|{})\s*(//.*)?\s*$"#,
        GO_VERSION_PATTERN
    ))
    .unwrap()
});

// require ブロック内エントリの正規表現: module/path v1.2.3
static BLOCK_ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^\s*("[^"]+"|`[^`]+`|\S+)\s+("[^"]+"|`[^`]+`|{})\s*(//.*)?\s*$"#,
        GO_VERSION_PATTERN
    ))
    .unwrap()
});

// replace の左辺 (置換対象) を読むための正規表現: `<module> [<version>]`
// `=>` より左側だけを渡す前提で、モジュールパスと省略可能なバージョンを取り出す。
static REPLACE_LHS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*("[^"]+"|`[^`]+`|\S+)(?:\s+("[^"]+"|`[^`]+`|\S+))?\s*$"#).unwrap()
});

// pinned コメントの正規表現。
// `// indirect` 判定 (`comment.contains("indirect")`) が語順非依存なのと整合させるため、
// `pinned` がコメント内のどこに現れてもマッチさせる (`// indirect; pinned` のように
// `//` 直後でない場合も拾う)。単語境界 `\b` で `repinned` / `unpinned` への誤マッチは防ぐ。
static PINNED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//.*\bpinned\b").unwrap());

/// `require (` のようなブロック開始行かどうかを判定する。
///
/// キーワードの後ろが `(` だけで終わっている行のみをブロック開始とみなす。
/// `require ()` のように同じ行で閉じている場合は開始として扱わない
/// (単独の `)` 行が現れず、ブロック状態が解除されないため)。
fn is_go_block_start(logical: &str, keyword: &str) -> bool {
    logical
        .strip_prefix(keyword)
        .map(str::trim_start)
        .is_some_and(|rest| rest == "(")
}

/// `<keyword> <残り>` 形式の単一行ディレクティブから、キーワード以降を取り出す。
///
/// go の字句解析はキーワードと引数を空白種別で区別しないため、`strip_prefix("keyword ")`
/// のように半角スペース固定にするとタブ区切りの記述を取りこぼす。
fn strip_go_directive<'a>(logical: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = logical.strip_prefix(keyword)?;
    rest.starts_with(char::is_whitespace).then(|| rest.trim())
}

/// `replace` の左辺 (置換対象) を集めたもの。
///
/// Go の `modload.replacement()` は `replace[path@version]` → `replace[path]` の順に
/// 引き、版付き左辺は**その版だけ**を置換する。したがって
/// - 版なし replace: そのモジュールの全バージョンがローカルパス / 別モジュールへ差し替わる
/// - 版付き replace: 左辺の版と一致する `require` だけが差し替わる
///
/// のいずれかになる。前者で `require` を更新しても実ビルドは一切変わらず報告ノイズになり、
/// 後者では左辺と一致しなくなって**置換が黙って外れ**、ローカルのパッチ済みコードから
/// 上流コードへエラーなしで切り替わる。どちらもローカル解決される依存なので、
/// Cargo の path 依存 / 非 crates.io registry 依存と同じ方針で更新対象から外す。
#[derive(Debug, Default)]
struct ReplaceTargets {
    /// 版なし replace の対象モジュール (全バージョンが置換対象)
    wildcard: HashSet<String>,
    /// 版付き replace の対象モジュール → 置換される版
    versioned: HashMap<String, HashSet<String>>,
}

impl ReplaceTargets {
    /// `<module> [<version>] => <...>` 形式の 1 エントリを取り込む。
    fn add_entry(&mut self, entry: &str) {
        // `=>` の左側だけが置換対象。go の字句解析では `=>` はトークンなので
        // `a=>b` のように空白が無くても成立する。
        let Some((lhs, _)) = entry.split_once("=>") else {
            return;
        };
        let Some(caps) = REPLACE_LHS_RE.captures(lhs) else {
            return;
        };
        let Some(module) = caps.get(1) else {
            return;
        };
        let module = unquote_go_token(module.as_str()).to_string();

        match caps.get(2) {
            Some(version) => {
                let version = unquote_go_token(version.as_str()).to_string();
                self.versioned.entry(module).or_default().insert(version);
            }
            None => {
                self.wildcard.insert(module);
            }
        }
    }

    /// 指定モジュール・バージョンの `require` が replace で差し替えられるか。
    fn replaces(&self, module: &str, version: &str) -> bool {
        self.wildcard.contains(module)
            || self
                .versioned
                .get(module)
                .is_some_and(|versions| versions.contains(version))
    }
}

/// `go.mod` 全体から `replace` の左辺を収集する (単一行・ブロック形式の両方)。
fn collect_replace_targets(content: &str) -> ReplaceTargets {
    let mut targets = ReplaceTargets::default();
    let mut in_replace_block = false;

    for line in content.lines() {
        let logical = line.split("//").next().unwrap_or("").trim();
        if logical.is_empty() {
            continue;
        }

        if is_go_block_start(logical, "replace") {
            in_replace_block = true;
            continue;
        }
        if in_replace_block && logical == ")" {
            in_replace_block = false;
            continue;
        }

        let entry = if in_replace_block {
            logical
        } else if let Some(entry) = strip_go_directive(logical, "replace") {
            entry
        } else {
            continue;
        };

        targets.add_entry(entry);
    }

    targets
}

impl GoModParser {
    /// go.mod のブロック開始行判定 (`require (` / `retract(`) をクレート内へ公開する。
    ///
    /// `mod go_mod` は `manifest` 内部限定の可視性なので、自由関数のままでは
    /// 他モジュール (Go Proxy の retract 解析) から参照できない。公開済みの
    /// `GoModParser` にぶら下げることで、空白の有無を吸収する判定を 1 箇所に保つ。
    pub(crate) fn is_block_start(logical: &str, keyword: &str) -> bool {
        is_go_block_start(logical, keyword)
    }
}

impl ManifestParser for GoModParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Go);
        let replace_targets = collect_replace_targets(content);

        let mut in_require_block = false;
        let mut in_replace_block = false;

        for line in content.lines() {
            let trimmed = line.trim();
            let logical = trimmed.split("//").next().unwrap_or("").trim();

            // 空行とコメントをスキップする
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }

            // ブロックの開始/終了を確認する。
            // `require ()` (空ブロック) は開いて即閉じるため、開始として扱うと
            // 単独の `)` 行が現れず in_require_block が立ちっぱなしになり、
            // 以降の `exclude` / `retract` ブロックの行を依存として誤検出する。
            if is_go_block_start(logical, "require") {
                in_require_block = true;
                continue;
            }

            if is_go_block_start(logical, "replace") {
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
                if let Some(dep) =
                    parse_go_dependency(&caps, parser.as_ref(), is_pinned, &replace_targets)
                {
                    dependencies.push(dep);
                }
                continue;
            }

            // require ブロック内エントリをパースする
            if in_require_block
                && let Some(caps) = BLOCK_ENTRY_RE.captures(trimmed)
                && let Some(dep) =
                    parse_go_dependency(&caps, parser.as_ref(), is_pinned, &replace_targets)
            {
                dependencies.push(dep);
            }
        }

        let excluded_versions = collect_excluded_versions(content);
        for dependency in &mut dependencies {
            let Some(versions) = excluded_versions.get(&dependency.name) else {
                continue;
            };
            for version in versions {
                if !dependency.version_spec.rejected_versions.contains(version) {
                    dependency
                        .version_spec
                        .rejected_versions
                        .push(version.clone());
                }
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

        for raw_line in content.split_inclusive('\n') {
            let (line, line_ending) = split_line_ending(raw_line);
            let trimmed = line.trim();
            let logical = trimmed.split("//").next().unwrap_or("").trim();

            // replace ブロックの開始/終了を追跡する
            if is_go_block_start(logical, "replace") {
                in_replace_block = true;
            } else if is_go_block_start(logical, "exclude") {
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
                    let module_token = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let module = unquote_go_token(module_token);
                    if module == package {
                        let comment = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                        let version_token = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                        let formatted_version = quote_go_version_like(version_token, &new_ver);
                        let new_line = if comment.is_empty() {
                            format!("require {} {}", module_token, formatted_version)
                        } else {
                            format!("require {} {} {}", module_token, formatted_version, comment)
                        };
                        updated = true;
                        Some(new_line)
                    } else {
                        None
                    }
                } else if let Some(caps) = BLOCK_ENTRY_RE.captures(trimmed) {
                    // ブロックエントリとのマッチを試みる
                    let module_token = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let module = unquote_go_token(module_token);
                    if module == package {
                        let comment = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                        let version_token = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                        let formatted_version = quote_go_version_like(version_token, &new_ver);
                        // 先頭の空白を保持する
                        let leading_ws = line.len() - line.trim_start().len();
                        let indent = &line[..leading_ws];
                        let new_line = if comment.is_empty() {
                            format!("{}{} {}", indent, module_token, formatted_version)
                        } else {
                            format!(
                                "{}{} {} {}",
                                indent, module_token, formatted_version, comment
                            )
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
            result.push_str(line_ending);
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

fn unquote_go_token(token: &str) -> &str {
    if token.len() >= 2
        && ((token.starts_with('"') && token.ends_with('"'))
            || (token.starts_with('`') && token.ends_with('`')))
    {
        &token[1..token.len() - 1]
    } else {
        token
    }
}

fn quote_go_version_like(original: &str, new_version: &str) -> String {
    if original.starts_with('"') && original.ends_with('"') {
        format!("\"{}\"", new_version)
    } else if original.starts_with('`') && original.ends_with('`') {
        format!("`{}`", new_version)
    } else {
        new_version.to_string()
    }
}

/// `exclude` 指示をモジュール名ごとの除外バージョンへ変換する。
fn collect_excluded_versions(content: &str) -> HashMap<String, Vec<String>> {
    let mut excluded_versions: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_exclude_block = false;

    for line in content.lines() {
        let logical = line.split("//").next().unwrap_or("").trim();
        if logical.is_empty() {
            continue;
        }

        if is_go_block_start(logical, "exclude") {
            in_exclude_block = true;
            continue;
        }
        if in_exclude_block && logical == ")" {
            in_exclude_block = false;
            continue;
        }

        let entry = if in_exclude_block {
            logical
        } else if let Some(entry) = logical.strip_prefix("exclude ") {
            entry.trim()
        } else {
            continue;
        };
        let Some(caps) = BLOCK_ENTRY_RE.captures(entry) else {
            continue;
        };
        let Some(module) = caps.get(1) else {
            continue;
        };
        let Some(version) = caps.get(2) else {
            continue;
        };

        let module = unquote_go_token(module.as_str()).to_string();
        let version = unquote_go_token(version.as_str()).to_string();
        let versions = excluded_versions.entry(module).or_default();
        if !versions.contains(&version) {
            versions.push(version);
        }
    }

    excluded_versions
}

fn parse_go_dependency(
    caps: &regex::Captures,
    parser: &dyn VersionParser,
    is_pinned: bool,
    replace_targets: &ReplaceTargets,
) -> Option<Dependency> {
    let module = unquote_go_token(caps.get(1)?.as_str());
    let version = unquote_go_token(caps.get(2)?.as_str());

    // replace で差し替えられる require は更新対象から外す。
    // 版付き replace はバージョンを進めた瞬間に左辺と一致しなくなり、置換が
    // 黙って外れてローカルのパッチ済みコードから上流コードへ切り替わる。
    // 版なし replace は更新しても実ビルドが変わらないので報告ノイズにしかならない。
    if replace_targets.replaces(module, version) {
        return None;
    }

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
    fn test_parse_quoted_single_require() {
        let content = r#"
module example.com/myproject

go 1.21

require "golang.org/x/text" "v0.14.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "golang.org/x/text");
        assert_eq!(deps[0].version_spec.version, "0.14.0");
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
    fn test_parse_quoted_require_block_entry() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	"golang.org/x/text" "v0.14.0"
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "golang.org/x/text");
        assert_eq!(deps[0].version_spec.version, "0.14.0");
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
    fn test_parse_pinned_order_independent() {
        // `// pinned` は `//` の直後に限らず、コメント内のどこにあっても認識される。
        // `is_indirect` (comment.contains("indirect")) と語順非依存性を揃えることで、
        // `// indirect; pinned` のように pinned が後置されても GoPinned 扱いになる。
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/a/lib v1.0.0 // pinned; indirect
	github.com/b/lib v1.0.0 // indirect; pinned
	github.com/c/lib v1.0.0 // pinned
	github.com/d/lib v1.0.0 // indirect
)
"#;

        let deps = parse(content).unwrap();
        let find = |name: &str| deps.iter().find(|d| d.name == name).unwrap();

        // pinned を含む 3 件はすべて GoPinned (語順を問わない)
        assert_eq!(
            find("github.com/a/lib").version_spec.kind,
            VersionSpecKind::GoPinned
        );
        assert_eq!(
            find("github.com/b/lib").version_spec.kind,
            VersionSpecKind::GoPinned
        );
        assert_eq!(
            find("github.com/c/lib").version_spec.kind,
            VersionSpecKind::GoPinned
        );
        // pinned を含まない indirect 依存は GoPinned ではない
        assert_ne!(
            find("github.com/d/lib").version_spec.kind,
            VersionSpecKind::GoPinned
        );
    }

    #[test]
    fn test_parse_pinned_word_boundary() {
        // `unpinned` / `repinned` のような単語は pinned 指定として誤認しない。
        let content = r#"
module example.com/myproject

go 1.21

require github.com/x/lib v1.0.0 // unpinned for now
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_ne!(deps[0].version_spec.kind, VersionSpecKind::GoPinned);
    }

    /// バグ回帰テスト: 版なし replace の対象モジュールは全バージョンがローカルへ
    /// 差し替わるため、`require` を更新しても実ビルドは一切変わらない (報告ノイズ)。
    #[test]
    fn test_parse_skips_module_with_wildcard_replace() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/other/lib v1.0.0
)

replace github.com/gin-gonic/gin => ../local-gin
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/other/lib");
    }

    /// バグ回帰テスト: ブロック形式の replace も対象モジュールを更新候補から外す。
    #[test]
    fn test_parse_skips_modules_with_replace_block() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	github.com/gin-gonic/gin v1.9.1
	github.com/other/lib v1.0.0
	github.com/kept/lib v2.3.4
)

replace (
	github.com/gin-gonic/gin => ../local-gin
	github.com/other/lib => ../other-lib
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/kept/lib");
    }

    /// バグ回帰テスト: 版付き replace の版と一致する `require` を更新すると、
    /// 左辺と一致しなくなって置換が黙って外れ、ローカルのパッチ済みコードから
    /// 上流コードへエラーなしで切り替わる。
    #[test]
    fn test_parse_skips_require_matching_versioned_replace() {
        let content = r#"
module example.com/myproject

go 1.21

require example.com/lib v1.0.0

replace example.com/lib v1.0.0 => ./vendor/lib-patched
"#;

        let deps = parse(content).unwrap();
        assert!(
            deps.is_empty(),
            "版付き replace と一致する require は更新対象外にすべき: {:?}",
            deps
        );
    }

    /// 版付き replace の版が現在の `require` と一致しなければ置換は効いていないため、
    /// 通常どおり更新対象にする (Go の `replace[path@version]` は完全一致)。
    #[test]
    fn test_parse_keeps_require_when_versioned_replace_does_not_match() {
        let content = r#"
module example.com/myproject

go 1.21

require example.com/lib v1.0.0

replace example.com/lib v0.9.0 => ./vendor/lib-patched
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "example.com/lib");
        assert_eq!(deps[0].version_spec.version, "1.0.0");
    }

    /// 引用符付き・空白なし (`a=>b`) の replace 記述も左辺として解釈する。
    #[test]
    fn test_parse_skips_quoted_and_compact_replace() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	"example.com/quoted" "v1.0.0"
	example.com/compact v2.0.0
)

replace "example.com/quoted" "v1.0.0" => ./vendor/quoted
replace example.com/compact=>./vendor/compact
"#;

        let deps = parse(content).unwrap();
        assert!(
            deps.is_empty(),
            "引用符付き / 空白なしの replace も置換対象として扱うべき: {:?}",
            deps
        );
    }

    /// replace の右辺に現れるモジュールは置換対象ではないので更新候補から外さない。
    #[test]
    fn test_parse_keeps_module_appearing_only_on_replace_rhs() {
        let content = r#"
module example.com/myproject

go 1.21

require github.com/fork/gin v1.9.1

replace github.com/gin-gonic/gin => github.com/fork/gin v1.9.1
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/fork/gin");
    }

    /// バグ回帰テスト: `//` の前に空白が無い行末コメント。
    /// go の字句解析は識別子の途中でも `//` で必ず切る (x/mod modfile/read.go) ため
    /// `v1.0.0//indirect` は正当な構文だが、バージョン部のパターンが貪欲で
    /// コメントまで飲み込み、依存が無言で取りこぼされていた
    /// (`v1.0.0 //indirect` は動くという非対称があった)。
    #[test]
    fn test_parse_comment_without_space_before_slashes() {
        let content = r#"
module example.com/myproject

go 1.21

require (
	example.com/indirect v1.0.0//indirect
	example.com/pinned v2.0.0//pinned
	example.com/plain v3.0.0//some note
)

require example.com/single v4.0.0//indirect
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4, "取りこぼしがあります: {:?}", deps);

        let find = |name: &str| deps.iter().find(|d| d.name == name).unwrap();

        // 空白なしでも `// indirect` 判定が働く
        let indirect = find("example.com/indirect");
        assert_eq!(indirect.version_spec.version, "1.0.0");
        assert!(indirect.is_dev);

        // 空白なしでも `// pinned` 判定が働く
        let pinned = find("example.com/pinned");
        assert_eq!(pinned.version_spec.version, "2.0.0");
        assert_eq!(pinned.version_spec.kind, VersionSpecKind::GoPinned);

        // コメント内容はバージョンに混入しない
        let plain = find("example.com/plain");
        assert_eq!(plain.version_spec.version, "3.0.0");
        assert_eq!(plain.version_spec.raw, "v3.0.0");
        assert!(!plain.is_dev);

        let single = find("example.com/single");
        assert_eq!(single.version_spec.version, "4.0.0");
        assert!(single.is_dev);
    }

    /// 空白なしコメント付きの行も更新でき、コメントを保持する。
    #[test]
    fn test_update_comment_without_space_before_slashes() {
        let content =
            "module example.com/myproject\n\nrequire example.com/single v1.0.0//indirect\n";

        let result = GoModParser
            .update_version(content, "example.com/single", "v1.5.0")
            .unwrap();

        assert_eq!(
            result,
            "module example.com/myproject\n\nrequire example.com/single v1.5.0 //indirect\n"
        );
    }

    /// 空白なしコメントでも `+incompatible` のような build metadata は
    /// バージョン側に残す (`/` だけをコメント境界にする)。
    #[test]
    fn test_parse_incompatible_with_comment_without_space() {
        let content = r#"
module example.com/myproject

require example.com/legacy v2.0.0+incompatible//indirect
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "2.0.0");
        assert_eq!(
            deps[0].version_spec.suffix,
            Some("+incompatible".to_string())
        );
        assert!(deps[0].is_dev);
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
    fn test_update_quoted_single_require_preserves_quotes() {
        let content = r#"module example.com/myproject

go 1.21

require "golang.org/x/text" "v0.14.0"
"#;

        let result = GoModParser
            .update_version(content, "golang.org/x/text", "v0.15.0")
            .unwrap();

        assert!(
            result.contains(r#"require "golang.org/x/text" "v0.15.0""#),
            "quoted require の引用符を維持できていません: {}",
            result
        );
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
    fn test_parse_applies_single_exclude_to_matching_dependency() {
        // exclude の対象を依存関係の除外候補へ反映すること
        let content = r#"
module example.com/myproject

go 1.21

require github.com/gin-gonic/gin v1.9.1

exclude github.com/gin-gonic/gin v1.10.0
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
        assert_eq!(deps[0].version_spec.rejected_versions, vec!["v1.10.0"]);
    }

    #[test]
    fn test_parse_applies_exclude_block_to_matching_dependencies() {
        // exclude ブロックの複数指定を対応する依存関係へ反映すること
        let content = r#"
module example.com/myproject

go 1.21

require (
 github.com/gin-gonic/gin v1.9.1
 github.com/old/module v0.1.0
)

exclude (
 github.com/gin-gonic/gin v1.10.0
 github.com/gin-gonic/gin v1.11.0
 github.com/old/module v0.2.0
)
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");
        assert_eq!(
            deps[0].version_spec.rejected_versions,
            vec!["v1.10.0", "v1.11.0"]
        );
        assert_eq!(deps[1].version_spec.rejected_versions, vec!["v0.2.0"]);
    }

    #[test]
    fn test_parse_applies_quoted_exclude_before_require_without_duplicates() {
        // require より前の引用符付き exclude も収集し、重複は一度だけ反映すること
        let content = r#"
module example.com/myproject

go 1.21

exclude "github.com/gin-gonic/gin" "v1.10.0"
exclude github.com/gin-gonic/gin v1.10.0

require "github.com/gin-gonic/gin" "v1.9.1"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.rejected_versions, vec!["v1.10.0"]);
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
    fn test_update_single_require_preserves_crlf() {
        let content = "module example.com/myproject\r\n\r\ngo 1.21\r\n\r\nrequire github.com/gin-gonic/gin v1.9.1\r\n";

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();

        assert_eq!(
            result,
            "module example.com/myproject\r\n\r\ngo 1.21\r\n\r\nrequire github.com/gin-gonic/gin v1.10.0\r\n"
        );
    }

    #[test]
    fn test_update_require_block_preserves_crlf() {
        let content = "module example.com/myproject\r\n\r\ngo 1.21\r\n\r\nrequire (\r\n\tgithub.com/gin-gonic/gin v1.9.1 // indirect\r\n\tgithub.com/pkg/errors v0.9.1\r\n)\r\n";

        let result = GoModParser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();

        assert_eq!(
            result,
            "module example.com/myproject\r\n\r\ngo 1.21\r\n\r\nrequire (\r\n\tgithub.com/gin-gonic/gin v1.10.0 // indirect\r\n\tgithub.com/pkg/errors v0.9.1\r\n)\r\n"
        );
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
