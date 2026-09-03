//! Rust プロジェクト向けの `Cargo.toml` パーサ。
//!
//! 対応対象:
//! - `dependencies` セクション
//! - `dev-dependencies` セクション
//! - `build-dependencies` セクション
//! - `workspace.dependencies`（ワークスペースルート）
//! - inline table 形式: `{ version = "1.0" }`
//! - workspace 依存関係

use crate::domain::{Dependency, GitReference, GitSource, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::{
    ManifestParser,
    line_utils::{captured_quote_and_version, parse_toml_section_header, split_line_ending},
};
use crate::parser::{VersionParser, get_parser};
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;
use toml::Value;

/// `Cargo.toml` 用パーサ
pub struct CargoTomlParser;

/// セクションヘッダ行 (`[dependencies]` 等) からセクション名を取り出す。
///
/// 字句解析は line_utils の共有実装に委譲する。`[ dependencies ]` のような
/// ヘッダ内空白は TOML 仕様どおりキーの一部にしない (toml クレートによる
/// parse 側の解釈と一致し、依存セクションの照合が食い違わない)。
fn cargo_section_name(line: &str) -> Option<&str> {
    parse_toml_section_header(line)
}

fn is_cargo_dependency_section(section: &str) -> bool {
    matches!(
        section,
        "dependencies" | "dev-dependencies" | "build-dependencies" | "workspace.dependencies"
    ) || (section.starts_with("target.")
        && (section.ends_with(".dependencies")
            || section.ends_with(".dev-dependencies")
            || section.ends_with(".build-dependencies")))
}

/// `[dependencies.<package>]` のような複数行依存テーブルのセクション名から、
/// 末尾のパッケージ名を取り除いた親セクション名を返す。
/// パッケージ名は完全一致のみ許容し、`serde` で `[dependencies.serde_json]` のような
/// 名前プレフィックスを共有するテーブルへ誤マッチしない。
fn cargo_package_table_parent<'a>(section: &'a str, package: &str) -> Option<&'a str> {
    section.trim().strip_suffix(package)?.strip_suffix('.')
}

/// 現在のセクションが `package` の複数行依存テーブルかどうかを返す
fn is_cargo_package_dependency_table(section: &str, package: &str) -> bool {
    cargo_package_table_parent(section, package).is_some_and(is_cargo_dependency_section)
}

/// 現在のセクションが git tag 更新対象の複数行テーブルかどうかを返す。
/// 依存セクションに加えて `[patch.<registry>.<package>]` も対象にする。
fn is_cargo_package_git_tag_table(section: &str, package: &str) -> bool {
    cargo_package_table_parent(section, package).is_some_and(|parent| {
        is_cargo_dependency_section(parent)
            || parent
                .strip_prefix("patch.")
                .is_some_and(|registry| !registry.is_empty())
    })
}

/// inline table 形式の git tag を更新してよい親セクションかどうかを返す。
/// 通常の依存セクションに加えて `[patch.<registry>]` の inline table も対象にする。
fn is_cargo_git_tag_inline_section(section: &str) -> bool {
    is_cargo_dependency_section(section)
        || section
            .strip_prefix("patch.")
            .is_some_and(|registry| !registry.is_empty())
}

// 複数行テーブル内の `version` キー行にマッチする。行頭のキーだけを対象にするため、
// `#` で始まるコメント行や行末コメント内の `version = "..."` には反応しない。
static VERSION_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\s*version\s*=\s*)(?:"([^"]+)"|'([^']+)')"#).unwrap());

// 依存宣言内の `path` キー (inline table 内・複数行テーブル内の両方)
static PATH_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[{,])\s*path\s*=").unwrap());
// 依存宣言内の `registry = "..."` キーとその値
static REGISTRY_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|[{,])\s*registry\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
// 依存宣言内の `registry-index = "..."` キー。
// `registry` が config 上のレジストリ名を指すのに対し `registry-index` はインデックス URL を
// 直接指す別キーで、どちらも crates.io 以外を指す (Cargo は両方の同時指定を拒否する)。
// `registry` 側のパターンは `registry\s*=` を要求するため `registry-index` にはマッチしない
static REGISTRY_INDEX_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[{,])\s*registry-index\s*=").unwrap());

/// 行が crates.io 以外のソースを指す依存宣言かどうかを返す。
///
/// parse 側 (`parse_cargo_dependencies`) は path 依存と `registry` 指定 (crates-io 以外) を
/// 依存として surface しない。writer 側にも同じ除外が無いと、同名クレートが別セクションに
/// 宣言されている場合 (`[dependencies] my-lib = "0.4"` + `[dev-dependencies] my-lib =
/// { path = "../my-lib", version = "0.4" }`) に writer の曖昧判定が「宣言 1 個」と見なして
/// 発火せず、path 依存まで crates.io の最新版で黙って書き換えてしまう。
fn declares_non_crates_io_source(line: &str) -> bool {
    if PATH_KEY_RE.is_match(line) || REGISTRY_INDEX_KEY_RE.is_match(line) {
        return true;
    }
    REGISTRY_KEY_RE.captures(line).is_some_and(|caps| {
        let registry = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");
        registry != "crates-io"
    })
}

/// 対象パッケージが crates.io 以外のソースを指す依存セクションの集合を返す。
///
/// 複数行テーブルと dotted key のどちらも、`version` より後ろに `path` / `registry` が
/// 書かれ得るため、書き換えの前に 1 パス走査する。
fn non_crates_io_dependency_sections(
    content: &str,
    package: &str,
) -> std::collections::HashSet<String> {
    let mut excluded = std::collections::HashSet::new();
    let mut section = String::new();
    let escaped_package = regex::escape(package);
    let dotted_path_re = Regex::new(&format!(r"^\s*{}\s*\.\s*path\s*=", escaped_package)).unwrap();
    let dotted_registry_re = Regex::new(&format!(
        r#"^\s*{}\s*\.\s*registry\s*=\s*(?:"([^"]*)"|'([^']*)')"#,
        escaped_package
    ))
    .unwrap();
    // `registry-index` は値に関わらず crates.io 以外を指すので値は見ない
    let dotted_registry_index_re = Regex::new(&format!(
        r"^\s*{}\s*\.\s*registry-index\s*=",
        escaped_package
    ))
    .unwrap();

    for line in content.lines() {
        if let Some(name) = cargo_section_name(line) {
            section.clear();
            section.push_str(name);
            continue;
        }
        let package_table_is_excluded = is_cargo_package_dependency_table(&section, package)
            && declares_non_crates_io_source(line);
        let dotted_key_is_excluded = is_cargo_dependency_section(&section)
            && (dotted_path_re.is_match(line)
                || dotted_registry_index_re.is_match(line)
                || dotted_registry_re.captures(line).is_some_and(|caps| {
                    caps.get(1)
                        .or_else(|| caps.get(2))
                        .is_none_or(|value| value.as_str() != "crates-io")
                }));
        if package_table_is_excluded || dotted_key_is_excluded {
            excluded.insert(section.clone());
        }
    }

    excluded
}

// 複数行テーブル内の `tag` キー行にマッチする (git 依存のタグ更新用)
static TAG_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\s*tag\s*=\s*)(?:"([^"]+)"|'([^']+)')"#).unwrap());

impl ManifestParser for CargoTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let toml: Value = toml::from_str(content).map_err(|e: toml::de::Error| {
            ManifestError::TomlParseError {
                path: PathBuf::from("Cargo.toml"),
                message: e.to_string(),
            }
        })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Rust);

        // 通常の依存関係を読む
        if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
        }

        // 開発依存を読む
        if let Some(deps) = toml.get("dev-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
        }

        // build-dependencies は開発依存として扱う
        if let Some(deps) = toml.get("build-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
        }

        // target 固有依存を読む
        if let Some(target) = toml.get("target").and_then(|t| t.as_table()) {
            for (_target_name, target_config) in target {
                if let Some(deps) = target_config.get("dependencies").and_then(|d| d.as_table()) {
                    parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
                }
                if let Some(deps) = target_config
                    .get("dev-dependencies")
                    .and_then(|d| d.as_table())
                {
                    parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
                }
                if let Some(deps) = target_config
                    .get("build-dependencies")
                    .and_then(|d| d.as_table())
                {
                    parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
                }
            }
        }

        // ワークスペースルートの `workspace.dependencies` を読む
        if let Some(workspace) = toml.get("workspace").and_then(|w| w.as_table())
            && let Some(deps) = workspace.get("dependencies").and_then(|d| d.as_table())
        {
            parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Rust
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        // Cargo 警告を避けるため semver の build metadata (`+...`) は落とす
        let new_version = new_version.split('+').next().unwrap_or(new_version);

        let parser = get_parser(Language::Rust);
        let mut updated = false;

        // 単純な依存宣言と inline table は、現在の TOML セクションが依存セクションの時だけ更新する。
        // 同名クレートが複数の依存セクションにある場合に備えて最初の一致では打ち切らず、
        // 全依存セクションの全出現を置換する (各出現は自身の旧値から整形するため、
        // 出現ごとに制約形式が異なっていても正しく更新される)。
        let simple_pattern = format!(
            r#"^(\s*{})\s*=\s*(?:"([^"]+)"|'([^']+)')"#,
            regex::escape(package)
        );
        let table_pattern = format!(
            r#"^(\s*{}\s*=\s*\{{[^\n}}]*?\bversion\s*=\s*)(?:"([^"]+)"|'([^']+)')"#,
            regex::escape(package)
        );
        // TOML の dotted key 形式: `tokio.version = "1.38"`
        // toml クレートは dotted key を inline table と同じ構造へ畳むため parse は
        // 依存として surface する。ここを書き換えないと「更新あり」と報告した後に
        // 書き込みが失敗し、report/apply が矛盾する。
        let dotted_pattern = format!(
            r#"^(\s*{}\s*\.\s*version\s*=\s*)(?:"([^"]+)"|'([^']+)')"#,
            regex::escape(package)
        );
        let simple_re =
            Regex::new(&simple_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;
        let table_re =
            Regex::new(&table_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;
        let dotted_re =
            Regex::new(&dotted_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;
        // path / 別レジストリを指す複数行テーブルと dotted key は
        // 書き換え対象から外す (parse と同じ範囲)
        let excluded_sections = non_crates_io_dependency_sections(content, package);
        let mut section = String::new();
        let mut rebuilt = String::new();
        for segment in content.split_inclusive('\n') {
            let (line, newline) = split_line_ending(segment);

            if let Some(name) = cargo_section_name(line) {
                section.clear();
                section.push_str(name);
                rebuilt.push_str(segment);
                continue;
            }

            if is_cargo_dependency_section(&section) {
                if excluded_sections.contains(&section) {
                    rebuilt.push_str(segment);
                    continue;
                }
                // inline table が path / 別レジストリを指す場合は parse も依存として
                // 拾わないため、writer も触らない
                if declares_non_crates_io_source(line) {
                    rebuilt.push_str(segment);
                    continue;
                }
                let replacement = if let Some(caps) = simple_re.captures(line) {
                    let (quote, old_version) = captured_quote_and_version(&caps);
                    if !old_version.contains('/')
                        && let Some(spec) = parser.parse(old_version)
                        && let Some(new_ver) = spec.try_format_updated(new_version)
                    {
                        let replacement = format!("{} = {}{}{}", &caps[1], quote, new_ver, quote);
                        Some(simple_re.replace(line, replacement.as_str()).to_string())
                    } else {
                        None
                    }
                } else if let Some(caps) = table_re.captures(line) {
                    let (quote, old_version) = captured_quote_and_version(&caps);
                    if let Some(spec) = parser.parse(old_version)
                        && let Some(new_ver) = spec.try_format_updated(new_version)
                    {
                        let replacement = format!("{}{}{}{}", &caps[1], quote, new_ver, quote);
                        Some(table_re.replace(line, replacement.as_str()).to_string())
                    } else {
                        None
                    }
                } else if let Some(caps) = dotted_re.captures(line) {
                    let (quote, old_version) = captured_quote_and_version(&caps);
                    if let Some(spec) = parser.parse(old_version)
                        && let Some(new_ver) = spec.try_format_updated(new_version)
                    {
                        let replacement = format!("{}{}{}{}", &caps[1], quote, new_ver, quote);
                        Some(dotted_re.replace(line, replacement.as_str()).to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(new_line) = replacement {
                    rebuilt.push_str(&new_line);
                    rebuilt.push_str(newline);
                    updated = true;
                    continue;
                }
            } else if is_cargo_package_dependency_table(&section, package)
                && !excluded_sections.contains(&section)
            {
                // 複数行テーブル:
                // テーブル形式の例: [dependencies.package]
                // バージョン指定の例: version = "1.0.0"
                // [workspace.dependencies.package] や target 固有テーブルも、セクション名と
                // パッケージ名の完全一致で追跡しながら `version` キー行だけを置換する。
                // `features = [...]` が `version` より前にあっても更新でき、コメント行や
                // 行末コメント内の `version = "..."` は書き換えない。
                if let Some(caps) = VERSION_LINE_RE.captures(line) {
                    let (quote, old_version) = captured_quote_and_version(&caps);
                    if let Some(spec) = parser.parse(old_version)
                        && let Some(new_ver) = spec.try_format_updated(new_version)
                    {
                        let replacement = format!("{}{}{}{}", &caps[1], quote, new_ver, quote);
                        rebuilt.push_str(&VERSION_LINE_RE.replace(line, replacement.as_str()));
                        rebuilt.push_str(newline);
                        updated = true;
                        continue;
                    }
                }
            }

            rebuilt.push_str(segment);
        }

        if updated {
            Ok(rebuilt)
        } else {
            Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })
        }
    }

    fn update_git_tag(
        &self,
        content: &str,
        package: &str,
        new_tag: &str,
    ) -> Result<String, ManifestError> {
        let mut updated = false;

        let inline_pattern = format!(
            r#"^(\s*{}\s*=\s*\{{[^\n}}]*?\btag\s*=\s*)(?:"([^"]+)"|'([^']+)')"#,
            regex::escape(package)
        );
        let inline_re =
            Regex::new(&inline_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        // inline table と複数行テーブルの両方をセクション追跡しながら更新する。
        // 依存セクション外の同名キーを誤って書き換えず、TOML の単一引用符も保持する。
        let mut section = String::new();
        let mut rebuilt = String::new();
        for segment in content.split_inclusive('\n') {
            let (line, newline) = split_line_ending(segment);

            if let Some(name) = cargo_section_name(line) {
                section.clear();
                section.push_str(name);
                rebuilt.push_str(segment);
                continue;
            }

            if is_cargo_git_tag_inline_section(&section)
                && let Some(caps) = inline_re.captures(line)
            {
                let quote = if caps.get(3).is_some() { "'" } else { "\"" };
                let replacement = format!("{}{}{}{}", &caps[1], quote, new_tag, quote);
                rebuilt.push_str(&inline_re.replace(line, replacement.as_str()));
                rebuilt.push_str(newline);
                updated = true;
                continue;
            }

            if is_cargo_package_git_tag_table(&section, package)
                && let Some(caps) = TAG_LINE_RE.captures(line)
            {
                let quote = if caps.get(3).is_some() { "'" } else { "\"" };
                let replacement = format!("{}{}{}{}", &caps[1], quote, new_tag, quote);
                rebuilt.push_str(&TAG_LINE_RE.replace(line, replacement.as_str()));
                rebuilt.push_str(newline);
                updated = true;
                continue;
            }

            rebuilt.push_str(segment);
        }

        if updated {
            Ok(rebuilt)
        } else {
            Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Cargo.toml"),
                spec: package.to_string(),
                message: "git tag not found or could not be updated".to_string(),
            })
        }
    }
}

fn parse_cargo_dependencies(
    deps: &toml::map::Map<String, Value>,
    parser: &dyn VersionParser,
    is_dev: bool,
    output: &mut Vec<Dependency>,
) {
    for (name, value) in deps {
        match value {
            // 単純な文字列: `package = "1.0.0"`
            Value::String(s) => {
                if let Some(spec) = parser.parse(s) {
                    push_dependency(output, name, spec, is_dev, None, None);
                }
            }
            // inline table 形式: `package = { version = "1.0.0", features = [...] }`
            Value::Table(t) => {
                let package_name = t.get("package").and_then(|v| v.as_str()).unwrap_or(name);
                let manifest_name = (package_name != name).then_some(name.as_str());

                // git 依存の検出を先に試みる
                if let Some(git_source) = try_parse_git_source(t) {
                    let spec = git_reference_spec(&git_source.reference);
                    push_dependency(
                        output,
                        package_name,
                        spec,
                        is_dev,
                        manifest_name,
                        Some(git_source),
                    );
                    continue;
                }
                if t.contains_key("path") {
                    // path 依存はローカルのクレートで解決されるため crates.io の候補で
                    // 書き換えてはいけない。publish 用に `version` を併記していても、
                    // 公開済み最新版へ引き上げるとローカルクレートの実バージョンが
                    // 要求を満たさなくなり `cargo build` が壊れる
                    // (Cargo.lock 側が source 無しエントリを path 依存として除外するのと同じ方針)。
                    continue;
                }
                if t.get("registry")
                    .and_then(|v| v.as_str())
                    .is_some_and(|registry| registry != "crates-io")
                    || t.contains_key("registry-index")
                {
                    // crates.io API の候補で別レジストリの依存を書き換えると誤更新になる。
                    // `registry-index` はインデックス URL の直接指定なので、値に関わらず
                    // crates.io 以外を指す (社内 private registry で使われる)
                    continue;
                }
                if let Some(version_str) = t.get("version").and_then(|v| v.as_str())
                    && let Some(spec) = parser.parse(version_str)
                {
                    push_dependency(output, package_name, spec, is_dev, manifest_name, None);
                }
            }
            _ => {}
        }
    }
}

fn push_dependency(
    output: &mut Vec<Dependency>,
    name: &str,
    spec: VersionSpec,
    is_dev: bool,
    manifest_name: Option<&str>,
    git_source: Option<GitSource>,
) {
    let mut dep = if is_dev {
        Dependency::development(name.to_string(), spec, Language::Rust)
    } else {
        Dependency::production(name.to_string(), spec, Language::Rust)
    };
    if let Some(manifest_name) = manifest_name {
        dep = dep.with_manifest_name(manifest_name);
    }
    if let Some(gs) = git_source {
        dep = dep.with_git_source(gs);
    }
    output.push(dep);
}

/// inline table から git 依存を検出して `GitSource` を組み立てる
fn try_parse_git_source(table: &toml::map::Map<String, Value>) -> Option<GitSource> {
    let url = table.get("git").and_then(|v| v.as_str())?;
    let reference = if let Some(branch) = table.get("branch").and_then(|v| v.as_str()) {
        GitReference::Branch(branch.to_string())
    } else if let Some(tag) = table.get("tag").and_then(|v| v.as_str()) {
        GitReference::Tag(tag.to_string())
    } else if let Some(rev) = table.get("rev").and_then(|v| v.as_str()) {
        GitReference::Rev(rev.to_string())
    } else {
        GitReference::DefaultBranch
    };
    Some(GitSource::new(url, reference))
}

/// git 依存に対応する VersionSpec を組み立てる。
/// raw/version は branch/tag/rev の表示名を保持する (更新判定・出力表示で利用)。
fn git_reference_spec(reference: &GitReference) -> VersionSpec {
    let display = match reference {
        GitReference::Branch(_) | GitReference::Tag(_) | GitReference::Rev(_) => {
            reference.raw_value().unwrap_or("").to_string()
        }
        GitReference::DefaultBranch => "HEAD".to_string(),
    };
    VersionSpec::new(VersionSpecKind::Exact, display.clone(), display)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        CargoTomlParser.parse(content)
    }

    #[test]
    fn test_parse_simple_dependencies() {
        let content = r#"
[dependencies]
serde = "1.0"
tokio = "^1.28.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);
        assert!(!serde.is_dev);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_pinned_version() {
        let content = r#"
[dependencies]
exact = "=1.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_inline_table() {
        let content = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "^1.28.0", features = ["full"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_dev_dependencies() {
        let content = r#"
[dev-dependencies]
criterion = "0.5"
tempfile = "3.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().all(|d| d.is_dev));
    }

    #[test]
    fn test_parse_build_dependencies() {
        let content = r#"
[build-dependencies]
cc = "1.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_mixed_dependencies() {
        let content = r#"
[dependencies]
serde = "1.0"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde.is_dev);

        let criterion = deps.iter().find(|d| d.name == "criterion").unwrap();
        assert!(criterion.is_dev);
    }

    #[test]
    fn test_parse_tilde_version() {
        let content = r#"
[dependencies]
regex = "~1.9"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"
[package]
name = "test"
version = "0.1.0"
"#;

        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_invalid_toml() {
        let content = "not valid toml";
        let result = parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_git_dependency_default_branch() {
        let content = r#"
[dependencies]
my-crate = { git = "https://github.com/example/my-crate" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let dep = &deps[0];
        assert_eq!(dep.name, "my-crate");
        let git = dep.git_source.as_ref().unwrap();
        assert_eq!(git.url, "https://github.com/example/my-crate");
        assert_eq!(git.reference, GitReference::DefaultBranch);
    }

    #[test]
    fn test_parse_git_dependency_branch() {
        let content = r#"
[dependencies]
foo = { git = "https://github.com/owner/foo.git", branch = "main" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.url, "https://github.com/owner/foo.git");
        assert_eq!(git.reference, GitReference::Branch("main".to_string()));
    }

    #[test]
    fn test_parse_git_dependency_tag() {
        let content = r#"
[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.reference, GitReference::Tag("v1.2.3".to_string()));
        // tag 指定は is_pinned() = false (更新対象)
        assert!(!deps[0].is_pinned());
    }

    #[test]
    fn test_parse_git_dependency_rev_is_pinned() {
        let content = r#"
[dependencies]
baz = { git = "https://github.com/owner/baz.git", rev = "abc1234" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.reference, GitReference::Rev("abc1234".to_string()));
        // rev 指定は is_pinned() = true
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_update_git_tag_inline_table() {
        let content = r#"[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3" }
serde = "1.0.0"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.3.0""#));
        assert!(!result.contains(r#"tag = "v1.2.3""#));
        // 他の行が保持される
        assert!(result.contains(r#"serde = "1.0.0""#));
    }

    #[test]
    fn test_update_git_tag_inline_table_single_quoted_tag() {
        let content = r#"[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = 'v1.2.3' }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains("tag = 'v1.3.0'"));
        assert!(!result.contains("tag = 'v1.2.3'"));
    }

    #[test]
    fn test_update_git_tag_inline_table_scoped_to_dependency_sections() {
        let content = r#"[package.metadata]
bar = { tag = "keep" }

[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3" }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();

        assert!(result.contains(
            r#"[package.metadata]
bar = { tag = "keep" }"#
        ));
        assert!(
            result
                .contains(r#"bar = { git = "https://github.com/owner/bar.git", tag = "v1.3.0" }"#)
        );
    }

    #[test]
    fn test_update_git_tag_patch_inline_table() {
        let content = r#"[patch.crates-io]
foo = { git = "https://example.com/foo", tag = "v1.0.0" }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v1.1.0")
            .unwrap();
        assert!(result.contains(r#"foo = { git = "https://example.com/foo", tag = "v1.1.0" }"#));
    }

    #[test]
    fn test_update_git_tag_multiline_table() {
        let content = r#"[dependencies.bar]
git = "https://github.com/owner/bar.git"
tag = "v1.2.3"
features = ["async"]
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.3.0""#));
        assert!(result.contains(r#"features = ["async"]"#));
    }

    #[test]
    fn test_update_git_tag_not_found() {
        let content = r#"[dependencies]
serde = "1.0.0"
"#;
        let result = CargoTomlParser.update_git_tag(content, "serde", "v1.3.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_ignores_git_dependency() {
        // update_version は git 依存 (version 無し) を見つけられずエラーを返す
        let content = r#"[dependencies]
my-crate = { git = "https://github.com/example/my-crate", branch = "main" }
"#;
        let result = CargoTomlParser.update_version(content, "my-crate", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_path_dependency_skipped() {
        let content = r#"
[dependencies]
local-crate = { path = "../local-crate" }
"#;

        let deps = parse(content).unwrap();
        // バージョンを持たない path 依存はスキップする
        assert!(deps.is_empty());
    }

    /// 回帰テスト: publish 用に `version` を併記した path 依存も crates.io の候補で
    /// 書き換えてはいけない。ローカルクレートの実バージョンが要求を満たさなくなり
    /// `cargo build` が壊れるため (crates.io に同名クレートが実在する場合は
    /// まったく無関係なクレートの版で上書きされる)。
    #[test]
    fn test_parse_path_dependency_with_version_skipped() {
        let content = r#"
[dependencies]
common = { path = "../common", version = "0.1.0" }
tokio = { version = "1.0" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "tokio");
    }

    /// 回帰テスト: TOML の dotted key (`tokio.version = "1.38"`) は parse が依存として
    /// 読むため、update も同じ範囲を書き換えられること (report/apply の整合)。
    #[test]
    fn test_dotted_key_dependency_parse_and_update() {
        let content = r#"
[dependencies]
tokio.version = "1.38"
tokio.features = ["full"]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "tokio");
        assert_eq!(deps[0].version_spec.version, "1.38");

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.52.4")
            .unwrap();
        assert!(result.contains(r#"tokio.version = "1.52.4""#), "{}", result);
        // 他の dotted key は触らない
        assert!(result.contains(r#"tokio.features = ["full"]"#));
    }

    /// 依存セクション外の dotted key は書き換えない。
    #[test]
    fn test_dotted_key_path_dependency_is_not_updated() {
        let content = r#"
[dependencies]
tokio.version = "1.38"

[dev-dependencies]
tokio.path = "../tokio"
tokio.version = "1.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.52.4")
            .unwrap();

        assert!(result.contains(r#"tokio.version = "1.52.4""#));
        assert!(result.contains(r#"tokio.path = "../tokio""#));
        assert!(result.contains(r#"tokio.version = "1.0""#));
    }

    #[test]
    fn test_dotted_key_outside_dependency_section_not_updated() {
        let content = r#"
[package.metadata.custom]
tokio.version = "1.38"
"#;

        let result = CargoTomlParser.update_version(content, "tokio", "1.52.4");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_workspace_dependency() {
        let content = r#"
[dependencies]
serde = { workspace = true }
"#;

        let deps = parse(content).unwrap();
        // 明示バージョンのない workspace 依存はスキップする
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_custom_registry_dependency_skipped() {
        let content = r#"
[dependencies]
private-crate = { version = "1.0", registry = "internal" }
public-crate = { version = "2.0", registry = "crates-io" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "public-crate");
        assert_eq!(deps[0].version_spec.version, "2.0");
    }

    /// `registry-index` はインデックス URL の直接指定で、`registry` と同じく
    /// crates.io 以外を指す。crates.io の候補で書き換えると、社内 private registry の
    /// 依存が同名の公開クレート (typosquat を含む) の版で上書きされる
    #[test]
    fn test_parse_registry_index_dependency_skipped() {
        let content = r#"
[dependencies]
internal-crate = { version = "1.0", registry-index = "https://intranet.example/index" }
public-crate = { version = "2.0" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(
            deps.len(),
            1,
            "registry-index 依存は surface しない: {deps:?}"
        );
        assert_eq!(deps[0].name, "public-crate");
    }

    /// 複数行テーブル / dotted key 形式の `registry-index` も writer が書き換えない
    #[test]
    fn test_update_does_not_touch_registry_index_dependency() {
        for content in [
            // inline table
            "[dependencies]\ninternal-crate = { version = \"1.0\", registry-index = \"https://intranet.example/index\" }\n",
            // 複数行テーブル (version より後ろに registry-index)
            "[dependencies.internal-crate]\nversion = \"1.0\"\nregistry-index = \"https://intranet.example/index\"\n",
            // dotted key
            "[dependencies]\ninternal-crate.version = \"1.0\"\ninternal-crate.registry-index = \"https://intranet.example/index\"\n",
        ] {
            let result = CargoTomlParser.update_version(content, "internal-crate", "9.9.9");
            match result {
                // 書き換え対象が見つからずエラーになるのが期待動作 (誤更新しない)
                Err(_) => {}
                Ok(updated) => assert_eq!(
                    updated, content,
                    "registry-index 依存を書き換えてはいけない: {content}"
                ),
            }
        }
    }

    #[test]
    fn test_parse_target_specific() {
        let content = r#"
[target.'cfg(windows)'.dependencies]
winapi = "0.3"

[target.'cfg(unix)'.dependencies]
libc = "0.2"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let winapi = deps.iter().find(|d| d.name == "winapi").unwrap();
        assert!(!winapi.is_dev);

        let libc = deps.iter().find(|d| d.name == "libc").unwrap();
        assert!(!libc.is_dev);
    }

    #[test]
    fn test_parse_target_specific_build_dependencies() {
        let content = r#"
[target.'cfg(unix)'.build-dependencies]
cc = "1.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "cc");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_renamed_package_dependency() {
        let content = r#"
[dependencies]
tokio_v1 = { package = "tokio", version = "1.0", features = ["rt"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "tokio");
        assert_eq!(deps[0].manifest_name(), "tokio_v1");
        assert_eq!(deps[0].version_spec.version, "1.0");
    }

    #[test]
    fn test_parse_renamed_git_dependency() {
        let content = r#"
[dependencies]
regex_alias = { package = "regex", git = "https://github.com/rust-lang/regex.git", tag = "1.10.3" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "regex");
        assert_eq!(deps[0].manifest_name(), "regex_alias");
        assert!(deps[0].is_git());
    }

    #[test]
    fn test_update_simple_version() {
        let content = r#"
[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(result.contains("\"1.1.0\""));
    }

    #[test]
    fn test_update_simple_version_single_quotes() {
        // TOML のリテラル文字列でも、引用符の種類を維持して更新する
        let content = r#"
[dependencies]
serde = '1.0.0'
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(
            result.contains("serde = '1.1.0'"),
            "単一引用符の依存を更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_simple_version_ignores_non_dependency_section() {
        // 同名キーが依存セクション外にあっても、実際の依存だけを更新する
        let content = r#"
[package.metadata]
serde = "metadata-value"

[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(r#"serde = "metadata-value""#));
        assert!(result.contains(r#"serde = "1.1.0""#));
    }

    #[test]
    fn test_update_simple_version_under_commented_section_header() {
        let content = r#"
[dependencies] # direct deps
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(r#"serde = "1.1.0""#));
    }

    /// 回帰テスト: TOML 仕様が許容するヘッダ内空白 (`[ dependencies ]`) でも
    /// 依存セクションとして認識して更新できる。以前はセクション名に空白が
    /// 残って照合に失敗し、parse (toml クレート) は依存として読むのに
    /// writer は書き換えないという parse/write 不整合があった。
    #[test]
    fn test_update_simple_version_under_whitespace_padded_section_header() {
        let content = r#"
[ dependencies ]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(r#"serde = "1.1.0""#));
    }

    #[test]
    fn test_update_renamed_package_dependency_uses_manifest_key() {
        let content = r#"
[dependencies]
tokio_v1 = { package = "tokio", version = "1.0", features = ["rt"] }
"#;

        let deps = parse(content).unwrap();
        let result = CargoTomlParser
            .update_version(content, deps[0].manifest_name(), "1.45.0")
            .unwrap();
        assert!(result.contains(r#"tokio_v1 = { package = "tokio", version = "1.45.0""#));
    }

    #[test]
    fn test_update_target_specific_multiline_table() {
        let content = r#"
[target.'cfg(unix)'.dependencies.openssl]
version = "0.10"
features = ["vendored"]
"#;

        let result = CargoTomlParser
            .update_version(content, "openssl", "0.10.72")
            .unwrap();
        assert!(result.contains(r#"version = "0.10.72""#));
        assert!(result.contains(r#"features = ["vendored"]"#));
    }

    #[test]
    fn test_update_target_specific_git_tag_multiline_table() {
        let content = r#"
[target.'cfg(unix)'.dependencies.regex]
git = "https://github.com/rust-lang/regex.git"
tag = "1.10.3"
"#;

        let result = CargoTomlParser
            .update_git_tag(content, "regex", "1.11.0")
            .unwrap();
        assert!(result.contains(r#"tag = "1.11.0""#));
    }

    #[test]
    fn test_update_wildcard_version_preserves_shape() {
        let content = r#"
[dependencies]
serde = "1.*"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "2.3.4")
            .unwrap();
        assert!(result.contains("\"2.*\""));
    }

    #[test]
    fn test_update_wildcard_x_version_preserves_shape() {
        // 回帰テスト: cargo (semver crate) が受理する `1.x` 形式も形を保って更新する
        let content = r#"
[dependencies]
serde = "1.x"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Wildcard);

        let result = CargoTomlParser
            .update_version(content, "serde", "2.3.4")
            .unwrap();
        assert!(result.contains(r#"serde = "2.x""#));
    }

    #[test]
    fn test_update_caret_version() {
        let content = r#"
[dependencies]
tokio = "^1.28.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.35.0")
            .unwrap();
        assert!(result.contains("\"^1.35.0\""));
    }

    #[test]
    fn test_update_range_version_keeps_upper_bound() {
        let content = r#"
[dependencies]
serde = ">=1.0, <2.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.9.3")
            .unwrap();
        assert!(result.contains("\">=1.9.3, <2.0\""));
    }

    #[test]
    fn test_update_inline_table() {
        let content = r#"
[dependencies]
serde = { version = "1.0.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(result.contains("\"1.1.0\""));
        assert!(result.contains("features"));
    }

    #[test]
    fn test_update_inline_table_single_quotes() {
        // inline table の version 値でも単一引用符を維持する
        let content = r#"
[dependencies]
serde = { version = '1.0.0', features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(
            result.contains("version = '1.1.0'"),
            "inline table の単一引用符バージョンを更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_inline_table_ignores_non_dependency_section() {
        // metadata の inline table ではなく、依存セクションの inline table だけを更新する
        let content = r#"
[package.metadata]
serde = { version = "metadata-value" }

[dependencies]
serde = { version = "1.0.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(
            r#"[package.metadata]
serde = { version = "metadata-value" }"#
        ));
        assert!(result.contains(r#"serde = { version = "1.1.0", features = ["derive"] }"#));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"
[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_language() {
        assert_eq!(CargoTomlParser.language(), Language::Rust);
    }

    #[test]
    fn test_parse_comparison_operators() {
        let content = r#"
[dependencies]
pkg1 = ">=1.0.0"
pkg2 = ">1.0.0"
pkg3 = "<=2.0.0"
pkg4 = "<2.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        let pkg1 = deps.iter().find(|d| d.name == "pkg1").unwrap();
        assert_eq!(pkg1.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pkg2 = deps.iter().find(|d| d.name == "pkg2").unwrap();
        assert_eq!(pkg2.version_spec.kind, VersionSpecKind::Greater);

        let pkg3 = deps.iter().find(|d| d.name == "pkg3").unwrap();
        assert_eq!(pkg3.version_spec.kind, VersionSpecKind::LessOrEqual);

        let pkg4 = deps.iter().find(|d| d.name == "pkg4").unwrap();
        assert_eq!(pkg4.version_spec.kind, VersionSpecKind::Less);
    }

    #[test]
    fn test_update_multiline_table() {
        let content = r#"[dependencies.tree-sitter]
version = "0.22"

[dependencies.tree-sitter-bash]
version = "0.21"
"#;

        let result = CargoTomlParser
            .update_version(content, "tree-sitter", "0.26.3")
            .unwrap();

        // version が正しく引用符で囲まれていることを確認する
        assert!(result.contains("version = \"0.26.3\""));
        // 閉じ引用符が欠けていないことを確認する
        assert!(!result.contains("\"0.26.3\n"));

        // 2つ目のパッケージも更新する
        let result2 = CargoTomlParser
            .update_version(&result, "tree-sitter-bash", "0.25.1")
            .unwrap();

        assert!(result2.contains("version = \"0.25.1\""));
        assert!(result2.contains("version = \"0.26.3\""));
    }

    #[test]
    fn test_update_multiline_table_single_quotes() {
        // 複数行テーブルの version 値でも単一引用符を維持する
        let content = r#"[dependencies.tree-sitter]
version = '0.22'
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "tree-sitter", "0.26.3")
            .unwrap();

        assert!(
            result.contains("version = '0.26.3'"),
            "複数行テーブルの単一引用符バージョンを更新できていません: {}",
            result
        );
        assert!(result.contains(r#"features = ["derive"]"#));
    }

    #[test]
    fn test_update_multiline_table_prefix_collision() {
        // 回帰テスト: パッケージ名が他のパッケージのプレフィックスと一致する場合に
        // 誤って prefix-only のパッケージ更新で suffix が長いパッケージが書き換わらない
        // 例: `[dependencies.serde_json]` が先に出現する状況で `serde` を更新する
        let content = r#"[dependencies.serde_json]
version = "1.0.0"

[dependencies.serde]
version = "1.0.2"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        // serde_json は変更されていないこと
        assert!(
            result.contains("[dependencies.serde_json]\nversion = \"1.0.0\""),
            "serde_json should not be modified, but got:\n{}",
            result
        );
        // serde が更新されていること
        assert!(
            result.contains("[dependencies.serde]\nversion = \"1.1.0\""),
            "serde should be updated, but got:\n{}",
            result
        );
    }

    #[test]
    fn test_update_git_tag_prefix_collision() {
        // 回帰テスト: プレフィックスが一致する別パッケージの git tag が誤って書き換わらない
        let content = r#"[dependencies.foo-extra]
git = "https://example.com/foo-extra"
tag = "v1.0.0"

[dependencies.foo]
git = "https://example.com/foo"
tag = "v0.5.0"
"#;

        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v0.6.0")
            .unwrap();

        // foo-extra の tag は変わらない
        assert!(
            result.contains("[dependencies.foo-extra]\ngit = \"https://example.com/foo-extra\"\ntag = \"v1.0.0\""),
            "foo-extra tag should not be modified, but got:\n{}",
            result
        );
        // foo の tag は更新される
        assert!(
            result.contains(
                "[dependencies.foo]\ngit = \"https://example.com/foo\"\ntag = \"v0.6.0\""
            ),
            "foo tag should be updated, but got:\n{}",
            result
        );
    }

    #[test]
    fn test_update_git_tag_multiline_table_tag_after_features() {
        // 回帰テスト: `features = [...]` が `tag` より前にあるテーブルでも更新できる
        let content = r#"[dependencies.bar]
git = "https://github.com/owner/bar.git"
features = ["async"]
tag = "v1.2.3"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(
            result.contains(r#"tag = "v1.3.0""#),
            "features が先行するテーブルの tag を更新できていません: {}",
            result
        );
        assert!(result.contains(r#"features = ["async"]"#));
    }

    #[test]
    fn test_update_git_tag_multiline_table_ignores_commented_tag() {
        // 回帰テスト: テーブル内コメントの `tag = "..."` は書き換えない
        let content = r#"[dependencies.bar]
git = "https://github.com/owner/bar.git"
# tag = "v0.9.0" 以前のタグ
tag = "v1.2.3"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.3.0""#));
        assert!(result.contains(r#"# tag = "v0.9.0" 以前のタグ"#));
        assert!(!result.contains(r#"tag = "v1.2.3""#));
    }

    #[test]
    fn test_update_git_tag_patch_section_multiline_table() {
        // `[patch.<registry>.<package>]` 形式の複数行テーブルも更新対象
        let content = r#"[patch.crates-io.foo]
git = "https://example.com/foo"
tag = "v1.0.0"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v1.1.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.1.0""#));
        assert!(result.contains(r#"git = "https://example.com/foo""#));
    }

    #[test]
    fn test_update_multiline_table_with_features() {
        let content = r#"[dependencies.serde]
version = "1.0.0"
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("version = \"1.1.0\""));
        assert!(result.contains("features = [\"derive\"]"));
    }

    #[test]
    fn test_update_multiline_table_version_after_features() {
        // 回帰テスト: `features = [...]` が `version` より前にあるテーブルでも更新できる
        // (旧実装は `[^\[]*` が features の `[` で止まり、常に更新失敗していた)
        let content = r#"[dependencies.serde]
features = ["derive"]
version = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(
            result.contains("version = \"1.1.0\""),
            "features が先行するテーブルの version を更新できていません: {}",
            result
        );
        assert!(result.contains("features = [\"derive\"]"));
    }

    #[test]
    fn test_update_multiline_table_ignores_commented_version() {
        // 回帰テスト: テーブル内コメントの `version = "..."` は書き換えず、
        // 実際の version キー行だけを更新する (旧実装は greedy マッチで
        // テーブル末尾コメント内の version を誤置換していた)
        let content = r#"[dependencies.serde]
# version = "0.9" 以前の固定値
version = "1.0.0"
# version = "0.8" まで利用していた

[features]
default = []
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("version = \"1.1.0\""));
        assert!(result.contains("# version = \"0.9\" 以前の固定値"));
        assert!(result.contains("# version = \"0.8\" まで利用していた"));
        assert!(!result.contains("version = \"1.0.0\""));
    }

    #[test]
    fn test_update_version_updates_all_dependency_sections() {
        // 回帰テスト: 同名クレートが複数の依存セクションにある場合、
        // 1回の update_version で全出現が更新される (旧実装は最初の1箇所のみ)
        let content = r#"
[dependencies]
tokio = { version = "1.28.0", features = ["full"] }

[dev-dependencies]
tokio = "~1.28.0"

[package.metadata]
tokio = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();

        // dependencies 側 (inline table) は自身の制約形式で更新される
        assert!(result.contains(r#"tokio = { version = "1.45.0", features = ["full"] }"#));
        // dev-dependencies 側 (tilde) も同じ呼び出しで更新される
        assert!(
            result.contains(r#"tokio = "~1.45.0""#),
            "dev-dependencies 側の同名依存が更新されていません: {}",
            result
        );
        // 依存セクション外の同名キーは書き換えない
        assert!(result.contains(r#"tokio = "1.0.0""#));
    }

    #[test]
    fn test_update_version_updates_all_multiline_tables() {
        // 同名クレートの複数行テーブルが複数の依存セクションにある場合も全て更新する
        let content = r#"[dependencies.serde]
version = "1.0.0"
features = ["derive"]

[dev-dependencies.serde]
version = "1.0.1"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("[dependencies.serde]\nversion = \"1.1.0\""));
        assert!(
            result.contains("[dev-dependencies.serde]\nversion = \"1.1.0\""),
            "dev-dependencies 側の複数行テーブルが更新されていません: {}",
            result
        );
    }

    #[test]
    fn test_update_mixed_dependency_formats() {
        // 実運用に近い混在形式の Cargo.toml:
        // - 単純形式: `pkg = "version"`
        // - inline table 形式: `pkg = { version = "...", features = [...] }`
        // - 複数行テーブル: `[dependencies.pkg]`
        let content = r#"[package]
name = "example-hooks"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
dirs = "5"
regex = "1"
thiserror = "1"
anyhow = "1"

[dependencies.ts-parser]
version = "0.22"
optional = true

[dependencies.ts-bash]
version = "0.21"
optional = true

[features]
default = ["ast-parser"]
ast-parser = ["ts-parser", "ts-bash"]
"#;

        // 単純形式の更新
        let result = CargoTomlParser
            .update_version(content, "serde_json", "1.0.140")
            .unwrap();
        assert!(result.contains("serde_json = \"1.0.140\""));

        // inline table 形式の更新
        let result = CargoTomlParser
            .update_version(&result, "clap", "4.5.0")
            .unwrap();
        assert!(result.contains("version = \"4.5.0\""));
        assert!(result.contains("features = [\"derive\"]"));

        // 別の inline table も更新する
        let result = CargoTomlParser
            .update_version(&result, "tracing-subscriber", "0.3.20")
            .unwrap();
        assert!(result.contains("version = \"0.3.20\""));
        assert!(result.contains("features = [\"env-filter\"]"));

        // 複数行テーブル形式の更新でも引用符が壊れないことを確認する
        let result = CargoTomlParser
            .update_version(&result, "ts-parser", "0.26.3")
            .unwrap();
        assert!(result.contains("version = \"0.26.3\""));
        // 閉じ引用符が壊れていないことを確認する
        assert!(!result.contains("\"0.26.3\n["));

        let result = CargoTomlParser
            .update_version(&result, "ts-bash", "0.25.1")
            .unwrap();
        assert!(result.contains("version = \"0.25.1\""));
        assert!(!result.contains("\"0.25.1\n["));

        // すべての更新結果が保持されていることを確認する
        assert!(result.contains("serde_json = \"1.0.140\""));
        assert!(result.contains("clap = { version = \"4.5.0\""));
        assert!(result.contains("version = \"0.26.3\""));
        assert!(result.contains("version = \"0.25.1\""));

        // 関係ない内容が保持されていることを確認する
        assert!(result.contains("[features]"));
        assert!(result.contains("ast-parser = [\"ts-parser\", \"ts-bash\"]"));
    }

    #[test]
    fn test_parse_workspace_dependencies() {
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/core", "crates/cli"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
        assert!(!tokio.is_dev);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);

        let serde_json = deps.iter().find(|d| d.name == "serde_json").unwrap();
        assert_eq!(serde_json.version_spec.kind, VersionSpecKind::Caret);

        let thiserror = deps.iter().find(|d| d.name == "thiserror").unwrap();
        assert_eq!(thiserror.version_spec.kind, VersionSpecKind::Caret);

        let clap = deps.iter().find(|d| d.name == "clap").unwrap();
        assert_eq!(clap.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_update_workspace_dependencies_simple() {
        let content = r#"
[workspace.dependencies]
serde_json = "1"
thiserror = "2"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde_json", "1.0.140")
            .unwrap();
        assert!(result.contains("serde_json = \"1.0.140\""));
        // 他の依存関係が保持されていることを確認する
        assert!(result.contains("thiserror = \"2\""));
    }

    #[test]
    fn test_update_workspace_dependencies_inline_table() {
        let content = r#"
[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        let result = CargoTomlParser
            .update_version(&result, "serde", "1.0.220")
            .unwrap();
        assert!(result.contains("serde = { version = \"1.0.220\""));
    }

    #[test]
    fn test_update_workspace_dependencies_multiline_table() {
        let content = r#"[workspace.dependencies.tokio]
version = "1"
features = ["full"]

[workspace.dependencies.serde]
version = "1"
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        let result = CargoTomlParser
            .update_version(&result, "serde", "1.0.220")
            .unwrap();
        assert!(result.contains("version = \"1.0.220\""));
    }

    #[test]
    fn test_parse_full_workspace_cargo_toml() {
        // ユーザーの実例に近いワークスペース構成
        let content = r#"
[workspace]
resolver = "2"
members = [
    "crates/omni-pty-core",
    "crates/term-ipc",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
authors = ["omni-pty contributors"]
repository = "https://github.com/omni-pty/omni-pty"

[workspace.dependencies]
# Core dependencies
portable-pty = "0.9"
vte = "0.15"
tokio = { version = "1", features = ["full"] }
kdl = "4"

# Utilities
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
parking_lot = "0.12"
log = "0.4"
libc = "0.2"

# CLI
clap = { version = "4", features = ["derive"] }

# Testing
tokio-test = "0.4"

# Swift FFI
swift-bridge = "0.1"
swift-bridge-build = "0.1"
"#;

        let deps = parse(content).unwrap();
        // `workspace.dependencies` をすべて解釈できることを確認する（合計16件）
        assert_eq!(deps.len(), 16);

        // 代表的な依存関係の内容を確認する
        let portable_pty = deps.iter().find(|d| d.name == "portable-pty").unwrap();
        assert_eq!(portable_pty.version_spec.version, "0.9");

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.version, "1");

        let uuid = deps.iter().find(|d| d.name == "uuid").unwrap();
        assert_eq!(uuid.version_spec.version, "1");

        let swift_bridge = deps.iter().find(|d| d.name == "swift-bridge").unwrap();
        assert_eq!(swift_bridge.version_spec.version, "0.1");
    }

    #[test]
    fn test_update_full_workspace_cargo_toml() {
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/core"]

[workspace.dependencies]
portable-pty = "0.9"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
"#;

        // 単純形式を更新する
        let result = CargoTomlParser
            .update_version(content, "portable-pty", "0.10.0")
            .unwrap();
        assert!(result.contains("portable-pty = \"0.10.0\""));

        // inline table 形式を更新する
        let result = CargoTomlParser
            .update_version(&result, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        // workspace メタデータが保持されていることを確認する
        assert!(result.contains("resolver = \"2\""));
        assert!(result.contains("members = [\"crates/core\"]"));
    }

    #[test]
    fn test_parse_workspace_with_regular_dependencies() {
        // `workspace.dependencies` と通常依存が共存する workspace ルート
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/cli"]

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = "1"

[dependencies]
clap = "4"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        // workspace 依存
        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert!(!tokio.is_dev);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde.is_dev);

        // 通常依存
        let clap = deps.iter().find(|d| d.name == "clap").unwrap();
        assert!(!clap.is_dev);

        // 開発依存
        let criterion = deps.iter().find(|d| d.name == "criterion").unwrap();
        assert!(criterion.is_dev);
    }

    #[test]
    fn test_update_version_strips_build_metadata() {
        let content = r#"
[dependencies]
toml = "0.8.0"
"#;

        // crates.io は `"1.0.0+spec-1.1.0"` のような build metadata 付きで返す場合がある
        let result = CargoTomlParser
            .update_version(content, "toml", "1.0.0+spec-1.1.0")
            .unwrap();
        // build metadata は書き込み前に除去する
        assert!(result.contains("\"1.0.0\""));
        assert!(!result.contains("+spec-1.1.0"));
    }

    #[test]
    fn test_update_version_strips_build_metadata_inline_table() {
        let content = r#"
[dependencies]
toml = { version = "0.8.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "toml", "1.0.0+spec-1.1.0")
            .unwrap();
        assert!(result.contains("version = \"1.0.0\""));
        assert!(!result.contains("+spec-1.1.0"));
        assert!(result.contains("features"));
    }

    #[test]
    fn test_update_version_preserves_crlf() {
        // 回帰: CRLF の Cargo.toml を update_version しても改行コードを保持し、
        // 通常依存・inline table・複数行テーブルのいずれの経路でも正しく更新する。
        let content = "[dependencies]\r\nserde = \"1.0.0\"\r\ntokio = { version = \"1.0.0\", features = [\"full\"] }\r\n\r\n[dependencies.reqwest]\r\nversion = \"0.11.0\"\r\n";

        let simple = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(simple.contains("serde = \"1.1.0\""));
        assert_eq!(
            simple.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!simple.replace("\r\n", "").contains('\n'));

        let inline = CargoTomlParser
            .update_version(content, "tokio", "1.1.0")
            .unwrap();
        assert!(inline.contains("version = \"1.1.0\""));
        assert!(inline.contains("features = [\"full\"]"));
        assert_eq!(
            inline.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!inline.replace("\r\n", "").contains('\n'));

        let multiline = CargoTomlParser
            .update_version(content, "reqwest", "0.11.20")
            .unwrap();
        assert!(multiline.contains("version = \"0.11.20\""));
        assert_eq!(
            multiline.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!multiline.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_update_git_tag_preserves_crlf() {
        // 回帰: CRLF の Cargo.toml を update_git_tag しても改行コードを保持し、
        // inline table・複数行テーブルの両タグ書き換え経路でも正しく更新する。
        let inline_content = "[dependencies]\r\nbar = { git = \"https://github.com/owner/bar.git\", tag = \"v1.2.3\" }\r\n";
        let inline = CargoTomlParser
            .update_git_tag(inline_content, "bar", "v1.3.0")
            .unwrap();
        assert!(inline.contains("tag = \"v1.3.0\""));
        assert_eq!(
            inline.matches("\r\n").count(),
            inline_content.matches("\r\n").count()
        );
        assert!(!inline.replace("\r\n", "").contains('\n'));

        let table_content = "[dependencies.bar]\r\ngit = \"https://github.com/owner/bar.git\"\r\ntag = \"v1.2.3\"\r\n";
        let table = CargoTomlParser
            .update_git_tag(table_content, "bar", "v1.3.0")
            .unwrap();
        assert!(table.contains("tag = \"v1.3.0\""));
        assert_eq!(
            table.matches("\r\n").count(),
            table_content.matches("\r\n").count()
        );
        assert!(!table.replace("\r\n", "").contains('\n'));
    }
}
