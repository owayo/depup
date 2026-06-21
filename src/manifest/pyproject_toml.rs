//! Python プロジェクト向けの `pyproject.toml` パーサ。
//!
//! 対応対象:
//! - `project.dependencies`（PEP 621）
//! - `project.optional-dependencies`（PEP 621）
//! - `dependency-groups`（PEP 735）
//! - `tool.poetry.dependencies`（Poetry）
//! - `tool.poetry.dev-dependencies`（Poetry）
//! - `tool.rye.dev-dependencies`（Rye）

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::{VersionParser, get_parser};
use regex::Regex;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;
use toml::Value;

/// `pyproject.toml` 用パーサ
pub struct PyprojectTomlParser;

// PEP 508 依存指定を解釈する正規表現
// 例: `package-name>=1.0,<2.0`, `package-name==1.0`, `package-name^1.0`
static PEP508_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z0-9][-a-zA-Z0-9._]*)\s*(.*)$").unwrap());
static POETRY_INLINE_SOURCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"source\s*=\s*(?:"([^"]+)"|'([^']+)')"#).unwrap());

impl ManifestParser for PyprojectTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let toml: Value = toml::from_str(content).map_err(|e: toml::de::Error| {
            ManifestError::TomlParseError {
                path: PathBuf::from("pyproject.toml"),
                message: e.to_string(),
            }
        })?;

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Python);
        let non_pypi_source_names = poetry_non_pypi_source_names(&toml);

        // PEP 621 の `project.dependencies` を読む
        if let Some(deps) = toml
            .get("project")
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
        {
            for dep in deps {
                if let Some(dep_str) = dep.as_str()
                    && !pep508_uses_non_pypi_source(dep_str, &non_pypi_source_names)
                    && let Some(parsed) = parse_pep508_dependency(dep_str, parser.as_ref(), false)
                {
                    dependencies.push(parsed);
                }
            }
        }

        // PEP 621 の `project.optional-dependencies` を読む
        if let Some(optional) = toml
            .get("project")
            .and_then(|p| p.get("optional-dependencies"))
            .and_then(|d| d.as_table())
        {
            for (_group, deps) in optional {
                if let Some(deps_array) = deps.as_array() {
                    for dep in deps_array {
                        if let Some(dep_str) = dep.as_str()
                            && !pep508_uses_non_pypi_source(dep_str, &non_pypi_source_names)
                            && let Some(parsed) =
                                parse_pep508_dependency(dep_str, parser.as_ref(), false)
                        {
                            dependencies.push(parsed);
                        }
                    }
                }
            }
        }

        // PEP 735 の `dependency-groups` を読む
        if let Some(groups) = toml.get("dependency-groups").and_then(|d| d.as_table()) {
            for (group_name, deps) in groups {
                let is_dev = group_name == "dev" || group_name == "test" || group_name == "lint";
                if let Some(deps_array) = deps.as_array() {
                    for dep in deps_array {
                        if let Some(dep_str) = dep.as_str()
                            && let Some(parsed) =
                                parse_pep508_dependency(dep_str, parser.as_ref(), is_dev)
                        {
                            dependencies.push(parsed);
                        }
                    }
                }
            }
        }

        // Poetry の依存関係を読む
        if let Some(poetry_deps) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_table())
        {
            for (name, value) in poetry_deps {
                // Python 自体の要求バージョンは依存更新対象にしない
                if name == "python" {
                    continue;
                }
                if let Some(parsed) = parse_poetry_dependency(name, value, parser.as_ref(), false) {
                    dependencies.push(parsed);
                }
            }
        }

        // Poetry の開発依存を読む
        if let Some(dev_deps) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dev-dependencies"))
            .and_then(|d| d.as_table())
        {
            for (name, value) in dev_deps {
                if let Some(parsed) = parse_poetry_dependency(name, value, parser.as_ref(), true) {
                    dependencies.push(parsed);
                }
            }
        }

        // Poetry 1.2+ のグループ依存を読む
        if let Some(groups) = toml
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("group"))
            .and_then(|g| g.as_table())
        {
            for (group_name, group) in groups {
                let is_dev = group_name == "dev" || group_name == "test";
                if let Some(deps) = group.get("dependencies").and_then(|d| d.as_table()) {
                    for (name, value) in deps {
                        if let Some(parsed) =
                            parse_poetry_dependency(name, value, parser.as_ref(), is_dev)
                        {
                            dependencies.push(parsed);
                        }
                    }
                }
            }
        }

        // Rye の開発依存を読む
        if let Some(deps) = toml
            .get("tool")
            .and_then(|t| t.get("rye"))
            .and_then(|r| r.get("dev-dependencies"))
            .and_then(|d| d.as_array())
        {
            for dep in deps {
                if let Some(dep_str) = dep.as_str()
                    && let Some(parsed) = parse_pep508_dependency(dep_str, parser.as_ref(), true)
                {
                    dependencies.push(parsed);
                }
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Python
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        // TOML の整形を壊さないよう、依存セクション内だけを文字列置換で差し替える
        let parser = get_parser(Language::Python);
        let package_has_non_pypi_source = toml::from_str::<Value>(content)
            .map(|toml| poetry_non_pypi_source_names(&toml))
            .unwrap_or_default()
            .contains(&normalize_python_package_name(package));
        if package_has_non_pypi_source {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("pyproject.toml"),
                spec: package.to_string(),
                message: "package uses a non-PyPI Poetry source".to_string(),
            });
        }

        // Poetry 形式: `name = "^1.0.0"` / `name = '^1.0.0'`
        let simple_re = Regex::new(&format!(
            r#"^(\s*{}\s*=\s*)(?:"([^"]+)"|'([^']+)')"#,
            regex::escape(package)
        ))
        .ok();

        // Poetry の inline table 形式: `name = { version = "^1.0.0", ... }`
        let table_re = Regex::new(&format!(
            r#"^(\s*{}\s*=\s*\{{\s*[^}}]*version\s*=\s*)(?:"([^"]+)"|'([^']+)')([^}}]*\}})"#,
            regex::escape(package)
        ))
        .ok();

        // 配列中の PEP 508 形式: `"package>=1.0,<2.0"` / `"package [extras] (>=1.0); marker"`
        let pep508_re = Regex::new(&format!(
            r#""({}(?:(?:\s*\[[^\]]*\])?\s*(?:[<>=!~^]|\()[^"]*)?)"|'({}(?:(?:\s*\[[^\]]*\])?\s*(?:[<>=!~^]|\()[^']*)?)'"#,
            regex::escape(package),
            regex::escape(package)
        ))
        .ok();

        // 行単位で現在の TOML セクションを追跡し、parse が依存として読むセクション内の
        // 全出現だけを更新する (`[build-system]` の requires 等は書き換えない)
        let mut result = String::with_capacity(content.len());
        let mut updated = false;
        let mut section = TomlSectionKind::Other;
        // `[project]` / `[tool.rye]` 内で依存配列 (`dependencies` / `dev-dependencies`) の中にいるか
        let mut in_scoped_dep_array = false;

        for line in content.split_inclusive('\n') {
            if let Some(header) = toml_section_header(line) {
                section = classify_toml_section(header);
                in_scoped_dep_array = false;
                result.push_str(line);
                continue;
            }

            let updated_line = match section {
                TomlSectionKind::PoetryDependencies => update_poetry_dependency_line(
                    line,
                    simple_re.as_ref(),
                    table_re.as_ref(),
                    parser.as_ref(),
                    new_version,
                ),
                TomlSectionKind::Pep508Dependencies => update_pep508_array_line(
                    line,
                    pep508_re.as_ref(),
                    package,
                    parser.as_ref(),
                    new_version,
                ),
                TomlSectionKind::Pep508ScopedArrays => {
                    // 依存配列 (`dependencies = [` / `dev-dependencies = [`) の内側だけ処理する。
                    // `name` / `keywords` / `description` 等の他キーは書き換えない。
                    if in_scoped_dep_array {
                        if line_has_scoped_array_close(line) {
                            in_scoped_dep_array = false;
                        }
                        update_pep508_array_line(
                            line,
                            pep508_re.as_ref(),
                            package,
                            parser.as_ref(),
                            new_version,
                        )
                    } else if is_scoped_dependency_array_start(line) {
                        // 配列開始行。同一行で閉じなければ以降も配列内
                        if !line_has_scoped_array_close(line) {
                            in_scoped_dep_array = true;
                        }
                        update_pep508_array_line(
                            line,
                            pep508_re.as_ref(),
                            package,
                            parser.as_ref(),
                            new_version,
                        )
                    } else {
                        None
                    }
                }
                TomlSectionKind::Other => None,
            };

            match updated_line {
                Some(new_line) => {
                    updated = true;
                    result.push_str(&new_line);
                }
                None => result.push_str(line),
            }
        }

        if updated {
            Ok(result)
        } else {
            Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("pyproject.toml"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })
        }
    }
}

/// `update_version` が置換対象とする TOML セクションの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TomlSectionKind {
    /// Poetry の依存セクション (`name = "^1.0"` / inline table 置換の対象)
    PoetryDependencies,
    /// セクション全体が PEP 508 依存配列 (の集合) で全行が置換対象
    /// (`[project.optional-dependencies]` / `[dependency-groups]`)
    Pep508Dependencies,
    /// セクション内の特定の依存配列 (`dependencies` / `dev-dependencies`) だけが置換対象で、
    /// `name` / `keywords` / `description` 等の他キーは書き換えてはいけないセクション
    /// (`[project]` / `[tool.rye]`)
    Pep508ScopedArrays,
    /// 依存セクション以外 (`[build-system]` 等。書き換え対象外)
    Other,
}

/// TOML のセクションヘッダ行 (`[section]` / `[[section]]`) からドット区切りキーを取り出す
fn toml_section_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let inner = trimmed
        .strip_prefix("[[")
        .or_else(|| trimmed.strip_prefix('['))?;
    let close = inner.find(']')?;
    let key = inner[..close].trim();
    let rest = inner[close..].trim_start_matches(']').trim_start();
    if key.is_empty() || !(rest.is_empty() || rest.starts_with('#')) {
        return None;
    }
    Some(key)
}

/// セクションヘッダのドット区切りキーから置換対象種別を判定する
fn classify_toml_section(header: &str) -> TomlSectionKind {
    // parse が読む Poetry の依存セクション
    if header == "tool.poetry.dependencies"
        || header == "tool.poetry.dev-dependencies"
        || (header.starts_with("tool.poetry.group.") && header.ends_with(".dependencies"))
    {
        return TomlSectionKind::PoetryDependencies;
    }
    // セクション全体が PEP 508 依存配列 (の集合) で、全行が置換対象:
    // - [project.optional-dependencies] (PEP 621、各キー = 配列)
    // - [dependency-groups] (PEP 735、各キー = 配列)
    if header == "project.optional-dependencies" || header == "dependency-groups" {
        return TomlSectionKind::Pep508Dependencies;
    }
    // セクション内の特定の依存配列だけが対象 (name/keywords/description 等は書き換えない):
    // - [project] の dependencies 配列 (PEP 621)
    // - [tool.rye] の dev-dependencies 配列 (Rye)
    if header == "project" || header == "tool.rye" {
        return TomlSectionKind::Pep508ScopedArrays;
    }
    TomlSectionKind::Other
}

/// `[project]` / `[tool.rye]` 内で依存配列 (`dependencies = [` / `dev-dependencies = [`) の
/// 開始行かどうかを判定する。`name = "..."` や `keywords = [...]` は対象外。
fn is_scoped_dependency_array_start(line: &str) -> bool {
    let trimmed = line.trim_start();
    let after_key = if let Some(rest) = trimmed.strip_prefix("dependencies") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("dev-dependencies") {
        rest
    } else {
        return false;
    };
    // キー名の直後は空白か `=` のみ (`dependencies-extra` のような別キーに前方一致しない)
    let after_key = after_key.trim_start();
    after_key.starts_with('=') && after_key[1..].trim_start().starts_with('[')
}

/// 行をクォート外でスキャンし、配列の閉じ括弧 `]` を含むか判定する。
/// extras (`foo[extra]`) のようなクォート内の `]` は配列終了とみなさない。
fn line_has_scoped_array_close(line: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    for ch in line.chars() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return false,
            ']' if !in_single && !in_double => return true,
            _ => {}
        }
    }
    false
}

/// TOML 行から `#` 以降の行コメントを取り除く。文字列リテラル内 (`"..."` / `'...'`)
/// の `#` は保持する。改行コードや末尾の空白はそのまま残す。
fn strip_toml_line_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// 正規表現キャプチャから引用符種別と旧バージョン文字列 (グループ 2/3) を取り出す
fn captured_quote_and_version<'a>(caps: &regex::Captures<'a>) -> (&'static str, &'a str) {
    if let Some(m) = caps.get(2) {
        ("\"", m.as_str())
    } else if let Some(m) = caps.get(3) {
        ("'", m.as_str())
    } else {
        ("\"", "")
    }
}

/// Poetry 依存セクション内の 1 行を更新する。更新が起きた場合のみ `Some` を返す
fn update_poetry_dependency_line(
    line: &str,
    simple_re: Option<&Regex>,
    table_re: Option<&Regex>,
    parser: &dyn VersionParser,
    new_version: &str,
) -> Option<String> {
    // Poetry 形式: `name = "^1.0.0"` / `name = '^1.0.0'`
    if let Some(re) = simple_re
        && let Some(caps) = re.captures(line)
    {
        let (quote, old_version) = captured_quote_and_version(&caps);
        if let Some(spec) = parser.parse(old_version)
            && let Some(new_ver) = spec.try_format_updated(new_version)
        {
            let matched_end = caps.get(0).map(|m| m.end()).unwrap_or(0);
            let mut new_line = String::with_capacity(line.len() + new_ver.len());
            new_line.push_str(&caps[1]);
            new_line.push_str(quote);
            new_line.push_str(&new_ver);
            new_line.push_str(quote);
            new_line.push_str(&line[matched_end..]);
            return Some(new_line);
        }
        return None;
    }

    // Poetry の inline table 形式: `name = { version = "^1.0.0", ... }`
    if let Some(re) = table_re
        && let Some(caps) = re.captures(line)
    {
        let (quote, old_version) = captured_quote_and_version(&caps);
        let table_fragment = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        if !inline_poetry_table_has_non_pypi_source(table_fragment)
            && let Some(spec) = parser.parse(old_version)
            && let Some(new_ver) = spec.try_format_updated(new_version)
        {
            let matched_end = caps.get(0).map(|m| m.end()).unwrap_or(0);
            let mut new_line = String::with_capacity(line.len() + new_ver.len());
            new_line.push_str(&caps[1]);
            new_line.push_str(quote);
            new_line.push_str(&new_ver);
            new_line.push_str(quote);
            new_line.push_str(&caps[4]);
            new_line.push_str(&line[matched_end..]);
            return Some(new_line);
        }
    }

    None
}

/// PEP 508 配列を含む依存セクション内の 1 行を更新する。更新が起きた場合のみ `Some` を返す
fn update_pep508_array_line(
    line: &str,
    pep508_re: Option<&Regex>,
    package: &str,
    parser: &dyn VersionParser,
    new_version: &str,
) -> Option<String> {
    let re = pep508_re?;
    // `# "requests>=1.0",` のようにコメントアウトされた依存指定は parse 側 (TOML パーサ)
    // でも無視されているため、書き換えも見送る。コメント前の部分だけマッチング対象とする。
    // インラインコメント (`"requests>=2.0",  # used in prod`) の左側は引き続き処理する。
    let scan_target = strip_toml_line_comment(line);
    let mut new_line = line.to_string();
    let mut updated = false;

    for caps in re.captures_iter(scan_target) {
        let (quote, full_dep) = if let Some(m) = caps.get(1) {
            ("\"", m.as_str())
        } else if let Some(m) = caps.get(2) {
            ("'", m.as_str())
        } else {
            ("\"", "")
        };
        if let Some(pep_caps) = PEP508_RE.captures(full_dep) {
            let pkg_name = pep_caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let after_name = &full_dep[pkg_name.len()..];
            let (extras_str, version_part) = split_pep508_extras_and_version(after_name);

            if pkg_name == package
                && !version_part.is_empty()
                && let Some(new_ver) =
                    format_pep508_updated_version(version_part, parser, new_version)
            {
                let new_dep = format!("{}{}{}", package, extras_str, new_ver);
                new_line = new_line.replace(
                    &format!("{quote}{full_dep}{quote}"),
                    &format!("{quote}{new_dep}{quote}"),
                );
                updated = true;
            }
        }
    }

    updated.then_some(new_line)
}

fn normalize_python_package_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '_' | '.' => '-',
            _ => ch.to_ascii_lowercase(),
        })
        .collect()
}

fn poetry_source_is_non_pypi(source: &str) -> bool {
    !source.eq_ignore_ascii_case("pypi")
}

fn poetry_table_has_non_pypi_source(table: &toml::map::Map<String, Value>) -> bool {
    table
        .get("source")
        .and_then(|v| v.as_str())
        .is_some_and(poetry_source_is_non_pypi)
}

fn collect_poetry_non_pypi_source_names(
    deps: &toml::map::Map<String, Value>,
    names: &mut HashSet<String>,
) {
    for (name, value) in deps {
        let has_non_pypi_source = match value {
            Value::Table(table) => poetry_table_has_non_pypi_source(table),
            Value::Array(items) => items.iter().any(|item| {
                item.as_table()
                    .is_some_and(poetry_table_has_non_pypi_source)
            }),
            _ => false,
        };
        if has_non_pypi_source {
            names.insert(normalize_python_package_name(name));
        }
    }
}

fn poetry_non_pypi_source_names(toml: &Value) -> HashSet<String> {
    let mut names = HashSet::new();

    if let Some(poetry_deps) = toml
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_table())
    {
        collect_poetry_non_pypi_source_names(poetry_deps, &mut names);
    }

    if let Some(poetry_dev_deps) = toml
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dev-dependencies"))
        .and_then(|d| d.as_table())
    {
        collect_poetry_non_pypi_source_names(poetry_dev_deps, &mut names);
    }

    if let Some(groups) = toml
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("group"))
        .and_then(|g| g.as_table())
    {
        for group in groups.values() {
            if let Some(deps) = group.get("dependencies").and_then(|d| d.as_table()) {
                collect_poetry_non_pypi_source_names(deps, &mut names);
            }
        }
    }

    names
}

fn pep508_uses_non_pypi_source(dep_str: &str, source_names: &HashSet<String>) -> bool {
    PEP508_RE
        .captures(dep_str)
        .and_then(|caps| caps.get(1))
        .map(|name| source_names.contains(&normalize_python_package_name(name.as_str())))
        .unwrap_or(false)
}

fn inline_poetry_table_has_non_pypi_source(table_fragment: &str) -> bool {
    POETRY_INLINE_SOURCE_RE
        .captures(table_fragment)
        .and_then(|caps| caps.get(1).or_else(|| caps.get(2)))
        .map(|source| poetry_source_is_non_pypi(source.as_str()))
        .unwrap_or(false)
}

fn parse_pep508_dependency(
    dep_str: &str,
    parser: &dyn VersionParser,
    is_dev: bool,
) -> Option<Dependency> {
    let caps = PEP508_RE.captures(dep_str)?;
    let name = caps.get(1)?.as_str();
    let mut version_part = caps.get(2).map(|m| m.as_str()).unwrap_or("").trim();

    // `package[extra]>=1.0` の extras を version 部分から取り除く
    if version_part.starts_with('[')
        && let Some(idx) = version_part.find(']')
    {
        version_part = version_part[idx + 1..].trim();
    }

    // 環境マーカー（`;` 以降）は更新判定の対象外にする
    let version_part = version_part
        .split(';')
        .next()
        .unwrap_or(version_part)
        .trim();
    let (version_part, _) = strip_pep508_version_parens(version_part);

    if version_part.is_empty() {
        return None;
    }

    let spec = parser.parse(version_part)?;
    Some(if is_dev {
        Dependency::development(name, spec, Language::Python)
    } else {
        Dependency::production(name, spec, Language::Python)
    })
}

fn strip_pep508_version_parens(version_part: &str) -> (&str, bool) {
    let trimmed = version_part.trim();
    if let Some(inner) = trimmed
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        (inner.trim(), true)
    } else {
        (trimmed, false)
    }
}

fn split_pep508_extras_and_version(after_name: &str) -> (&str, &str) {
    let leading_ws_len = after_name.len() - after_name.trim_start().len();
    let mut prefix_end = leading_ws_len;

    if let Some(rest) = after_name.get(prefix_end..)
        && rest.starts_with('[')
        && let Some(idx) = rest.find(']')
    {
        prefix_end += idx + 1;
        if let Some(after_extras) = after_name.get(prefix_end..) {
            prefix_end += after_extras.len() - after_extras.trim_start().len();
        }
    }

    (&after_name[..prefix_end], after_name[prefix_end..].trim())
}

fn format_pep508_updated_version(
    version_part: &str,
    parser: &dyn VersionParser,
    new_version: &str,
) -> Option<String> {
    let (constraint, marker) = version_part
        .split_once(';')
        .map(|(constraint, marker)| (constraint.trim(), Some(marker)))
        .unwrap_or((version_part.trim(), None));
    let (parse_target, has_parens) = strip_pep508_version_parens(constraint);
    if parse_target.is_empty() {
        return None;
    }
    let spec = parser.parse(parse_target)?;
    let updated = spec.try_format_updated(new_version)?;
    let mut result = if has_parens {
        format!("({})", updated)
    } else {
        updated
    };
    if let Some(marker) = marker {
        result.push(';');
        result.push_str(marker);
    }
    Some(result)
}

fn parse_poetry_dependency(
    name: &str,
    value: &Value,
    parser: &dyn VersionParser,
    is_dev: bool,
) -> Option<Dependency> {
    let version_str = match value {
        Value::String(s) => s.clone(),
        Value::Table(t) => {
            if poetry_table_has_non_pypi_source(t) {
                return None;
            }
            t.get("version")?.as_str()?.to_string()
        }
        _ => return None,
    };

    let spec = parser.parse(&version_str)?;
    Some(if is_dev {
        Dependency::development(name, spec, Language::Python)
    } else {
        Dependency::production(name, spec, Language::Python)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        PyprojectTomlParser.parse(content)
    }

    #[test]
    fn test_parse_pep621_dependencies() {
        let content = r#"
[project]
dependencies = [
    "requests>=2.28.0",
    "pydantic==2.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let requests = deps.iter().find(|d| d.name == "requests").unwrap();
        assert_eq!(requests.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pydantic = deps.iter().find(|d| d.name == "pydantic").unwrap();
        assert_eq!(pydantic.version_spec.kind, VersionSpecKind::Exact);
        assert!(pydantic.is_pinned());
    }

    #[test]
    fn test_parse_pep621_optional_dependencies() {
        let content = r#"
[project.optional-dependencies]
dev = [
    "pytest>=7.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pytest");
    }

    #[test]
    fn test_parse_poetry_dependencies() {
        let content = r#"
[tool.poetry.dependencies]
python = "^3.8"
requests = "^2.28.0"
pydantic = "~2.0"
"#;

        let deps = parse(content).unwrap();
        // `python` 自体の指定はスキップされる
        assert_eq!(deps.len(), 2);

        let requests = deps.iter().find(|d| d.name == "requests").unwrap();
        assert_eq!(requests.version_spec.kind, VersionSpecKind::Caret);
        assert!(!requests.is_dev);

        let pydantic = deps.iter().find(|d| d.name == "pydantic").unwrap();
        assert_eq!(pydantic.version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_poetry_dev_dependencies() {
        let content = r#"
[tool.poetry.dev-dependencies]
pytest = "^7.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_poetry_group_dependencies() {
        let content = r#"
[tool.poetry.group.dev.dependencies]
pytest = "^7.0.0"

[tool.poetry.group.docs.dependencies]
sphinx = "^6.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let pytest = deps.iter().find(|d| d.name == "pytest").unwrap();
        assert!(pytest.is_dev);

        let sphinx = deps.iter().find(|d| d.name == "sphinx").unwrap();
        assert!(!sphinx.is_dev); // docs グループは開発依存ではない
    }

    #[test]
    fn test_parse_pep735_dependency_groups() {
        let content = r#"
[project]
name = "test"

[dependency-groups]
dev = [
    "ruff>=0.11.8",
    "pytest>=7.0.0",
]
lint = [
    "mypy>=1.0.0",
]
docs = [
    "sphinx>=6.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        let ruff = deps.iter().find(|d| d.name == "ruff").unwrap();
        assert!(ruff.is_dev); // dev グループは開発依存
        assert_eq!(ruff.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pytest = deps.iter().find(|d| d.name == "pytest").unwrap();
        assert!(pytest.is_dev); // dev グループは開発依存

        let mypy = deps.iter().find(|d| d.name == "mypy").unwrap();
        assert!(mypy.is_dev); // lint グループは開発依存

        let sphinx = deps.iter().find(|d| d.name == "sphinx").unwrap();
        assert!(!sphinx.is_dev); // docs グループは開発依存ではない
    }

    #[test]
    fn test_parse_poetry_inline_table() {
        let content = r#"
[tool.poetry.dependencies]
requests = { version = "^2.28.0", extras = ["security"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"
[project]
name = "test"
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
    fn test_parse_with_extras() {
        let content = r#"
[project]
dependencies = [
    "httpx[http2]>=0.24.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "httpx");
    }

    #[test]
    fn test_parse_with_environment_markers() {
        let content = r#"
[project]
dependencies = [
    "pywin32>=300; sys_platform == 'win32'",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pywin32");
    }

    #[test]
    fn test_update_poetry_version() {
        let content = r#"
[tool.poetry.dependencies]
requests = "^2.28.0"
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains("^2.31.0"));
    }

    #[test]
    fn test_update_poetry_single_quoted_version() {
        // Poetry 形式でも TOML の単一引用符を維持して更新する
        let content = r#"
[tool.poetry.dependencies]
requests = '^2.28.0'
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("requests = '^2.31.0'"),
            "単一引用符の Poetry バージョンを更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_poetry_inline_table() {
        let content = r#"
[tool.poetry.dependencies]
requests = { version = "^2.28.0", extras = ["security"] }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains("^2.31.0"));
    }

    #[test]
    fn test_update_poetry_inline_table_single_quoted_version() {
        // inline table の version 値でも TOML の単一引用符を維持する
        let content = r#"
[tool.poetry.dependencies]
requests = { version = '^2.28.0', extras = ["security"] }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("version = '^2.31.0'"),
            "inline table の単一引用符バージョンを更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_parse_rye_dev_dependencies() {
        let content = r#"
[project]
name = "ci-watcher"
dependencies = [
    "requests>=2.32.5",
]

[tool.rye]
managed = true
dev-dependencies = [
    "ruff>=0.11.3",
    "pytest>=7.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let requests = deps.iter().find(|d| d.name == "requests").unwrap();
        assert!(!requests.is_dev);

        let ruff = deps.iter().find(|d| d.name == "ruff").unwrap();
        assert!(ruff.is_dev); // Rye の dev-dependencies は開発依存になる
        assert_eq!(ruff.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pytest = deps.iter().find(|d| d.name == "pytest").unwrap();
        assert!(pytest.is_dev);
    }

    #[test]
    fn test_language() {
        assert_eq!(PyprojectTomlParser.language(), Language::Python);
    }

    #[test]
    fn test_parse_pep508_range_version() {
        let content = r#"
[project]
dependencies = [
    "paramiko>=3.5.0,<4.0.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "paramiko");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, ">=3.5.0,<4.0.0");
    }

    #[test]
    fn test_parse_pep508_range_version_with_trailing_comma() {
        let content = r#"
[project]
dependencies = [
    "paramiko>=3.5.0,<4.0.0,",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "paramiko");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, ">=3.5.0,<4.0.0,");
    }

    #[test]
    fn test_parse_pep508_parenthesized_version_spec() {
        let content = r#"
[project]
dependencies = [
    "requests (>=2.28,<3)",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, ">=2.28,<3");
    }

    #[test]
    fn test_update_pep508_range_preserves_constraint() {
        // 下限つき Range は上限制約を保ったまま下限だけ更新する
        let content = r#"
[project]
dependencies = [
    "paramiko>=3.5.0,<4.0.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "paramiko", "3.9.1")
            .unwrap();

        assert!(
            result.contains("paramiko>=3.9.1,<4.0.0"),
            "Range constraint should keep the upper bound, got: {}",
            result
        );
        assert!(
            !result.contains("paramiko3.9.1"),
            "Version should not be concatenated with package name"
        );
    }

    #[test]
    fn test_update_pep508_range_preserves_trailing_comma() {
        let content = r#"
[project]
dependencies = [
    "paramiko>=3.5.0,<4.0.0,",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "paramiko", "3.9.1")
            .unwrap();

        assert!(
            result.contains("paramiko>=3.9.1,<4.0.0,"),
            "末尾カンマ付きの PEP 508 range を維持できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_single_quoted_dependency_preserves_quote() {
        // TOML のリテラル文字列でも、依存指定を更新して引用符を維持する
        let content = r#"
[project]
dependencies = [
    'paramiko>=3.5.0,<4.0.0',
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "paramiko", "3.9.1")
            .unwrap();

        assert!(
            result.contains("'paramiko>=3.9.1,<4.0.0'"),
            "単一引用符の PEP 508 依存を更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_parenthesized_version_spec() {
        let content = r#"
[project]
dependencies = [
    "requests (>=2.28,<3)",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("requests (>=2.31.0,<3)"),
            "Parenthesized versionspec should keep parentheses, got: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_parenthesized_version_spec_with_marker() {
        let content = r#"
[project]
dependencies = [
    "requests (>=2.28,<3); python_version < '3.12'",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("requests (>=2.31.0,<3); python_version < '3.12'"),
            "括弧付き versionspec と環境マーカーを維持できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_single_quoted_dependency_with_double_quoted_marker() {
        // 単一引用符の TOML 文字列では、PEP 508 マーカー側の二重引用符も維持する
        let content = r#"
[project]
dependencies = [
    'requests (>=2.28,<3); python_version < "3.12"',
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("'requests (>=2.31.0,<3); python_version < \"3.12\"'"),
            "単一引用符の依存と二重引用符の環境マーカーを維持できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_range_with_space() {
        let content = r#"
[project]
dependencies = [
    "requests>=2.0, <3.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("requests>=2.31.0, <3.0"),
            "Range constraint with space should update the lower bound, got: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_extras_with_spaces() {
        let content = r#"
[project]
dependencies = [
    "coverage [toml] >=7,<8",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "coverage", "7.6.0")
            .unwrap();

        assert!(
            result.contains("coverage [toml] >=7.6.0,<8"),
            "Extras spacing should be preserved, got: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_simple_gte_still_works() {
        // 単純な>=指定子は通常通り更新されるべき
        let content = r#"
[project]
dependencies = [
    "requests>=2.28.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains("requests>=2.31.0"),
            "Simple >= constraint should be updated, got: {}",
            result
        );
    }

    #[test]
    fn test_parse_pep508_without_version_is_skipped() {
        // バージョン指定のない依存はスキップする
        let content = r#"
[project]
dependencies = [
    "requests",
    "flask>=2.0"
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "flask");
    }

    #[test]
    fn test_parse_poetry_path_dependency_skipped() {
        // Poetry 形式の path 依存はスキップする
        let content = r#"
[tool.poetry.dependencies]
python = "^3.8"
requests = "^2.28.0"
local-pkg = { path = "../local-pkg" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
    }

    #[test]
    fn test_parse_poetry_git_dependency_skipped() {
        // Poetry 形式の git 依存はスキップする
        let content = r#"
[tool.poetry.dependencies]
python = "^3.8"
requests = "^2.28.0"
my-pkg = { git = "https://github.com/user/my-pkg.git", branch = "main" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
    }

    #[test]
    fn test_parse_poetry_multiple_constraints_array_skipped() {
        // Poetry のマルチプル制約配列形式 (Python バージョン別に異なる制約) は、
        // depup の「1依存=1バージョン=1書き換え」モデルでは配列要素の位置を特定して
        // 安全に更新できないため、意図的にスキップする (誤更新を防ぐ安全側の挙動)。
        // 各要素の python マーカーごとの requires_python 互換性も judge しないため、
        // 自動更新の対象から外す。
        let content = r#"
[tool.poetry.dependencies]
python = "^3.8"
requests = "^2.28.0"
foo = [
    { version = "<=1.9", python = ">=3.6,<3.8" },
    { version = "^2.0", python = ">=3.8" },
]
"#;

        let deps = parse(content).unwrap();
        // 配列形式の foo はスキップされ、通常依存の requests だけがパースされる
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
    }

    #[test]
    fn test_parse_poetry_non_pypi_source_dependency_skipped() {
        // PyPI 以外の Poetry source は PyPI API の候補で更新できないためスキップする
        let content = r#"
[tool.poetry.dependencies]
python = "^3.8"
private-pkg = { version = "^1.0", source = "internal" }
public-pkg = { version = "^2.0", source = "pypi" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "public-pkg");
        assert_eq!(deps[0].version_spec.version, "2.0");
    }

    #[test]
    fn test_parse_pep508_enriched_by_non_pypi_source_skipped() {
        // Poetry 2 の source 補足がある PEP 621 依存も PyPI 候補では更新しない
        let content = r#"
[project]
dependencies = [
    "private_pkg>=1.0,<2.0",
    "requests>=2.0,<3.0",
]

[tool.poetry.dependencies]
private-pkg = { source = "internal" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
    }

    #[test]
    fn test_update_poetry_non_pypi_source_dependency_returns_err() {
        // source が PyPI 以外なら inline table の version も書き換えない
        let content = r#"
[tool.poetry.dependencies]
private-pkg = { version = "^1.0", source = "internal" }
"#;

        let result = PyprojectTomlParser.update_version(content, "private-pkg", "1.2.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_pep508_with_url_skipped() {
        // URL 依存はスキップする
        let content = r#"
[project]
dependencies = [
    "flask>=2.0",
    "my-pkg @ https://example.com/my-pkg-1.0.tar.gz",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "flask");
    }

    #[test]
    fn test_parse_pep508_with_spaces_in_version() {
        // PEP 508 では演算子前後の空白を許容する
        let content = r#"
[project]
dependencies = [
    "flask>=2.0",
    "requests >= 2.28.0",
]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.name == "flask"));
        assert!(deps.iter().any(|d| d.name == "requests"));
    }

    #[test]
    fn test_update_pep508_with_extras() {
        // PEP 508 の extras 付き依存も正しく更新する
        let content = r#"
[project]
dependencies = [
    "coverage[toml]>=6.5",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "coverage", "7.6.0")
            .unwrap();

        assert!(
            result.contains("coverage[toml]>=7.6.0"),
            "Extras should be preserved during update, got: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_with_multiple_extras() {
        let content = r#"
[project]
dependencies = [
    "httpx[http2,brotli]>=0.24.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "httpx", "0.28.0")
            .unwrap();

        assert!(
            result.contains("httpx[http2,brotli]>=0.28.0"),
            "Multiple extras should be preserved, got: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_extras_with_range() {
        // Range 型 + extras の組み合わせ
        let content = r#"
[project]
dependencies = [
    "coverage[toml]>=6.5,<8.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "coverage", "7.6.0")
            .unwrap();

        assert!(
            result.contains("coverage[toml]>=7.6.0,<8.0"),
            "Range constraint with extras should update the lower bound, got: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_not_equal_constraint_returns_err() {
        let content = r#"
[project]
dependencies = [
    "requests!=2.31.0",
]
"#;

        let result = PyprojectTomlParser.update_version(content, "requests", "2.32.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_pep508_compound_not_equal_constraint_returns_err() {
        let content = r#"
[project]
dependencies = [
    "requests>=2.0, !=2.31.0, <3.0",
]
"#;

        let result = PyprojectTomlParser.update_version(content, "requests", "2.32.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dependency_group_with_extras_and_bare() {
        // extras付き・バージョンなし・通常のパッケージが混在するグループ
        let content = r#"
[project]
name = "test"

[project.optional-dependencies]
dev = [
    "coverage[toml]>=6.5",
    "pytest",
    "scipy",
    "ruff",
]
"#;

        let deps = parse(content).unwrap();
        // pytest, scipy, ruff はバージョン指定なしなのでスキップされる
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "coverage");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(deps[0].version_spec.version, "6.5");
    }

    #[test]
    fn test_update_dependency_group_with_extras_and_bare() {
        // extras付きパッケージがバージョンなしパッケージと共存する場合の更新
        let content = r#"
[project]
name = "test"

[project.optional-dependencies]
dev = [
    "coverage[toml]>=6.5",
    "pytest",
    "scipy",
    "ruff",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "coverage", "7.6.0")
            .unwrap();

        // extras が保持され、バージョンが更新される
        assert!(
            result.contains(r#""coverage[toml]>=7.6.0""#),
            "Extras should be preserved and version updated, got: {}",
            result
        );
        // 他のパッケージは変更されない
        assert!(result.contains(r#""pytest""#));
        assert!(result.contains(r#""scipy""#));
        assert!(result.contains(r#""ruff""#));
    }

    #[test]
    fn test_parse_real_world_pyproject_with_extras() {
        // 実際の pyproject.toml に近い構造: self-reference extras, extras付き依存, バージョンなし依存が混在
        let content = r#"
[project]
name = "style-bert-vits2"
dependencies = [
    "numba>=0.64.0",
    "numpy>=2.4.2",
    "pydantic>=2.12.5",
]

[project.optional-dependencies]
train = [
    "style-bert-vits2[torch]",
    "faster-whisper>=1.2.1",
    "onnx>=1.20.1",
    "protobuf>=6.33.5",
    "pyannote.audio>=4.0.4",
]
infer = [
    "style-bert-vits2[torch]",
    "onnx>=1.20.1",
    "pyannote.audio>=4.0.4",
]

[dependency-groups]
dev = [
    "coverage[toml]>=6.5",
    "pytest",
    "scipy",
    "ruff",
]
"#;

        let deps = parse(content).unwrap();

        // `dependencies` から通常依存を取得できることを確認する
        assert!(deps.iter().any(|d| d.name == "numba"));
        assert!(deps.iter().any(|d| d.name == "numpy"));
        assert!(deps.iter().any(|d| d.name == "pydantic"));

        // `optional-dependencies` 内の自己参照はバージョンなしなのでスキップする
        assert!(!deps.iter().any(|d| d.name == "style-bert-vits2"));

        // onnx は train と infer 両方に出現するので2つ
        assert_eq!(deps.iter().filter(|d| d.name == "onnx").count(), 2);

        // `dependency-groups` では `coverage` のみが対象になる
        let coverage = deps.iter().find(|d| d.name == "coverage").unwrap();
        assert!(coverage.is_dev);
        assert_eq!(coverage.version_spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(coverage.version_spec.version, "6.5");

        // pytest, scipy, ruff はバージョン指定なしなのでスキップ
        assert!(!deps.iter().any(|d| d.name == "pytest"));
        assert!(!deps.iter().any(|d| d.name == "scipy"));
        assert!(!deps.iter().any(|d| d.name == "ruff"));
    }

    #[test]
    fn test_update_real_world_pyproject_coverage_with_extras() {
        // 修正前のバグ再現: coverage[toml]>=6.5 の更新で
        // "package not found or version could not be updated" エラーが発生していた
        let content = r#"
[project]
name = "style-bert-vits2"
dependencies = [
    "numba>=0.64.0",
    "numpy>=2.4.2",
    "pydantic>=2.12.5",
]

[project.optional-dependencies]
train = [
    "style-bert-vits2[torch]",
    "onnx>=1.20.1",
]
infer = [
    "style-bert-vits2[torch]",
    "onnx>=1.20.1",
]

[dependency-groups]
dev = [
    "coverage[toml]>=6.5",
    "pytest",
    "scipy",
    "ruff",
]
"#;

        // coverage の更新
        let result = PyprojectTomlParser
            .update_version(content, "coverage", "7.13.4")
            .unwrap();
        assert!(
            result.contains(r#""coverage[toml]>=7.13.4""#),
            "coverage[toml] extras should be preserved, got: {}",
            result
        );
        // 他のエントリは変更されない
        assert!(result.contains(r#""numba>=0.64.0""#));
        assert!(result.contains(r#""numpy>=2.4.2""#));
        assert!(result.contains(r#""onnx>=1.20.1""#));
        assert!(result.contains(r#""pytest""#));

        // onnx の更新（optional-dependencies 内の2箇所が同時に更新される）
        let result2 = PyprojectTomlParser
            .update_version(content, "onnx", "1.20.1")
            .unwrap();
        assert!(result2.contains(r#""onnx>=1.20.1""#));

        // numpy の更新
        let result3 = PyprojectTomlParser
            .update_version(content, "numpy", "2.4.2")
            .unwrap();
        assert!(result3.contains(r#""numpy>=2.4.2""#));
    }

    #[test]
    fn test_update_poetry_inline_table_does_not_touch_prefixed_package() {
        // (回帰) `requests` の更新が、行頭アンカーのない inline table 正規表現によって
        // `types-requests = { version = ... }` の内部を破壊しないことを確認する
        let content = r#"
[tool.poetry.dependencies]
requests = "^2.28.0"
types-requests = { version = "^2.28.0" }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains(r#"requests = "^2.31.0""#),
            "requests 本体が更新されるべき: {}",
            result
        );
        assert!(
            result.contains(r#"types-requests = { version = "^2.28.0" }"#),
            "types-requests は書き換えられないべき: {}",
            result
        );
    }

    #[test]
    fn test_update_poetry_same_package_in_main_and_group_updates_both() {
        // (回帰) 同名依存が main と dev group の両方にある場合、両方更新される
        // (replace-first だった旧実装では 2 箇所目が更新されなかった)
        let content = r#"
[tool.poetry.dependencies]
requests = "^2.28.0"

[tool.poetry.group.dev.dependencies]
requests = "~2.20"
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(
            result.contains(r#"requests = "^2.31.0""#),
            "main の requests が更新されるべき: {}",
            result
        );
        assert!(
            result.contains(r#"requests = "~2.31.0""#),
            "dev group の requests も更新されるべき: {}",
            result
        );
        assert!(!result.contains("2.28.0"));
        assert!(!result.contains("~2.20"));
    }

    #[test]
    fn test_update_poetry_same_package_in_dev_dependencies_updates_both() {
        // tool.poetry.dev-dependencies 側の同名依存も併せて更新される
        let content = r#"
[tool.poetry.dependencies]
requests = "^2.28.0"

[tool.poetry.dev-dependencies]
requests = { version = "^2.27.0", extras = ["security"] }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert!(result.contains(r#"requests = "^2.31.0""#));
        assert!(
            result.contains(r#"requests = { version = "^2.31.0", extras = ["security"] }"#),
            "dev-dependencies の inline table も更新されるべき: {}",
            result
        );
    }

    #[test]
    fn test_update_does_not_rewrite_build_system_requires() {
        // (回帰) parse が読まない [build-system] requires は書き換えない
        // (旧実装では pep508 置換が依存セクション外にも適用されていた)
        let content = r#"
[build-system]
requires = ["setuptools>=61.0"]
build-backend = "setuptools.build_meta"

[project]
dependencies = [
    "setuptools>=68.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "setuptools", "70.0.0")
            .unwrap();

        assert!(
            result.contains(r#""setuptools>=70.0.0""#),
            "project.dependencies 側は更新されるべき: {}",
            result
        );
        assert!(
            result.contains(r#"requires = ["setuptools>=61.0"]"#),
            "[build-system] requires は書き換えられないべき: {}",
            result
        );
    }

    #[test]
    fn test_update_poetry_simple_only_in_dependency_sections() {
        // Poetry 形式の単純置換は依存セクション内に限定され、無関係なセクションの
        // 同名キーは書き換えない
        let content = r#"
[tool.poetry.dependencies]
requests = "^2.28.0"

[tool.mytool]
requests = "^2.28.0"
"#;

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();

        assert_eq!(
            result.matches(r#"requests = "^2.31.0""#).count(),
            1,
            "依存セクション内の 1 箇所だけが更新されるべき: {}",
            result
        );
        assert!(
            result.contains("[tool.mytool]\nrequests = \"^2.28.0\""),
            "[tool.mytool] の同名キーは書き換えられないべき: {}",
            result
        );
    }

    #[test]
    fn test_update_pep508_same_package_in_project_and_dependency_groups() {
        // PEP 508 配列の同名依存が複数セクションにある場合、すべて更新される
        let content = r#"
[project]
dependencies = [
    "ruff>=0.11.0",
]

[dependency-groups]
dev = [
    "ruff>=0.10.0",
]

[tool.rye]
dev-dependencies = [
    "ruff>=0.9.0",
]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "ruff", "0.12.0")
            .unwrap();

        assert!(result.contains(r#""ruff>=0.12.0""#));
        assert!(
            !result.contains("0.11.0") && !result.contains("0.10.0") && !result.contains("0.9.0"),
            "全セクションの ruff が更新されるべき: {}",
            result
        );
    }

    #[test]
    fn test_update_version_skips_project_metadata_string() {
        // [project] 内の非依存メタデータ文字列 (description) は依存として書き換えない。
        // dependencies 配列内の同名依存だけが更新される。
        let content = r#"
[project]
name = "mypkg"
description = "requests>=2 is required for this"
dependencies = [
    "requests>=2.28.0",
]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(
            result.contains(r#"description = "requests>=2 is required for this""#),
            "description メタデータが書き換わってはいけない:\n{}",
            result
        );
        assert!(
            result.contains("requests>=2.31.0"),
            "dependencies 配列内の requests は更新されるべき:\n{}",
            result
        );
    }

    #[test]
    fn test_update_version_skips_project_keywords_array() {
        // [project] 配下の別の配列キー (keywords) は依存配列ではないので書き換えない。
        // flask は keywords にしかなく dependencies に無いので Err になる。
        let content = r#"
[project]
keywords = ["flask>=2.0"]
dependencies = ["requests>=2.28.0"]
"#;
        let result = PyprojectTomlParser.update_version(content, "flask", "9.9.9");
        assert!(
            result.is_err(),
            "keywords 配列の文字列を依存として書き換えてはいけない: {:?}",
            result
        );
    }

    #[test]
    fn test_update_version_skips_exact_metadata_string() {
        // メタデータ文字列が「ちょうど有効な依存指定」と一致しても依存として書き換えない。
        let content = r#"
[project]
description = "flask>=2.0"
dependencies = ["requests>=2.28.0"]
"#;
        let result = PyprojectTomlParser.update_version(content, "flask", "9.9.9");
        assert!(
            result.is_err(),
            "メタデータ文字列を依存として書き換えてはいけない: {:?}",
            result
        );
    }

    #[test]
    fn test_parse_full_optional_dependencies_groups() {
        // `optional-dependencies` の全グループが解釈されるか確認する
        let content = r#"
[project]
name = "style-bert-vits2"
dependencies = [
    "numba>=0.64.0",
    "numpy>=2.4.2",
    "pydantic>=2.12.5",
]

[project.optional-dependencies]
torch = [
    "accelerate",
    "torch>=2.10.0",
    "torchaudio>=2.10.0",
]
train = [
    "style-bert-vits2[torch]",
    "faster-whisper>=1.2.1",
    "GPUtil",
    "gradio>=6.6.0",
    "librosa>=0.11.0",
    "onnx>=1.20.1",
    "protobuf>=6.33.5",
    "pyannote.audio>=4.0.4",
]
infer = [
    "style-bert-vits2[torch]",
    "GPUtil",
    "gradio>=6.6.0",
    "onnx>=1.20.1",
    "pyannote.audio>=4.0.4",
]
colab = [
    "style-bert-vits2[train]",
    "onnxruntime-gpu",
    "torchvision",
]

[dependency-groups]
dev = [
    "coverage[toml]>=7.13.4",
    "pytest",
    "scipy",
    "ruff",
]
"#;

        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        println!("Parsed deps: {:?}", names);

        // `project.dependencies` の 3 件
        assert!(deps.iter().any(|d| d.name == "numba"), "numba missing");
        assert!(deps.iter().any(|d| d.name == "numpy"), "numpy missing");
        assert!(
            deps.iter().any(|d| d.name == "pydantic"),
            "pydantic missing"
        );

        // `torch` グループのうちバージョン付き 2 件
        assert!(deps.iter().any(|d| d.name == "torch"), "torch missing");
        assert!(
            deps.iter().any(|d| d.name == "torchaudio"),
            "torchaudio missing"
        );

        // `train` グループ
        assert!(
            deps.iter().any(|d| d.name == "faster-whisper"),
            "faster-whisper missing"
        );
        assert!(
            deps.iter().any(|d| d.name == "gradio"),
            "gradio missing from train"
        );
        assert!(deps.iter().any(|d| d.name == "librosa"), "librosa missing");
        assert!(deps.iter().any(|d| d.name == "onnx"), "onnx missing");
        assert!(
            deps.iter().any(|d| d.name == "protobuf"),
            "protobuf missing"
        );

        // `infer` グループでは `gradio` と `onnx` が重複して現れる
        assert!(
            deps.iter().filter(|d| d.name == "gradio").count() >= 2,
            "gradio should appear in both train and infer"
        );
        assert!(
            deps.iter().filter(|d| d.name == "onnx").count() >= 2,
            "onnx should appear in both train and infer"
        );

        // `dependency-groups` では `coverage[toml]` のみが対象になる
        assert!(
            deps.iter().any(|d| d.name == "coverage"),
            "coverage missing"
        );

        // 自己参照 (`style-bert-vits2[torch]`) やバージョンなし依存はスキップする
        assert!(!deps.iter().any(|d| d.name == "style-bert-vits2"));
        assert!(!deps.iter().any(|d| d.name == "accelerate"));
        assert!(
            !deps
                .iter()
                .any(|d| d.name == "GPUtil" || d.name == "gputil")
        );
    }

    /// 回帰テスト: PEP 508 配列内でコメントアウトされた依存指定は書き換えない。
    /// parse 側 (TOML パーサ) もコメントを無視するため、parse/write を整合させる。
    #[test]
    fn test_update_pep508_skips_commented_dependency() {
        let content = "[project]\n\
            name = \"test\"\n\
            dependencies = [\n    \
            \"requests>=2.0\",\n    \
            # \"requests>=1.0\",  # 旧版コメントアウト\n    \
            \"flask>=2.0\",\n\
            ]\n";
        let parser = PyprojectTomlParser;
        let updated = parser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(updated.contains("\"requests>=2.31.0\""));
        assert!(
            updated.contains("# \"requests>=1.0\""),
            "コメント内のバージョンは書き換えないこと: {updated}"
        );
    }

    /// 行末コメントの右側に依存風文字列がある場合も、左側だけ更新する。
    #[test]
    fn test_update_pep508_preserves_trailing_comment() {
        let content = "[project.optional-dependencies]\n\
            extra = [\n    \
            \"requests>=2.0\",  # 本番で使用 (旧: requests>=1.0)\n\
            ]\n";
        let parser = PyprojectTomlParser;
        let updated = parser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(updated.contains("\"requests>=2.31.0\""));
        // コメント内の "requests>=1.0" は単なる説明文字列で引用符もないので書き換えられない
        assert!(updated.contains("旧: requests>=1.0"));
    }

    #[test]
    fn test_strip_toml_line_comment_respects_strings() {
        // 文字列リテラル内の `#` は保持する
        assert_eq!(
            super::strip_toml_line_comment("a = \"x#y\"  # comment\n"),
            "a = \"x#y\"  "
        );
        assert_eq!(
            super::strip_toml_line_comment("a = 'x#y'  # comment"),
            "a = 'x#y'  "
        );
        // 文字列外の `#` 以降を切り捨てる
        assert_eq!(
            super::strip_toml_line_comment("a = 1  # comment"),
            "a = 1  "
        );
        // コメントなしはそのまま返る
        assert_eq!(super::strip_toml_line_comment("a = 1\n"), "a = 1\n");
    }
}
