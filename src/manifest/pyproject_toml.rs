//! Python プロジェクト向けの `pyproject.toml` パーサ。
//!
//! 対応対象:
//! - PEP 621 の `project.dependencies`
//! - PEP 621 の `project.optional-dependencies`
//! - PEP 735 の `dependency-groups`
//! - Poetry の `tool.poetry.dependencies`
//! - Poetry の `tool.poetry.dev-dependencies`
//! - Poetry の多行依存テーブル (`[tool.poetry.dependencies.<name>]`)
//! - Rye の `tool.rye.dev-dependencies`
//! - uv の `tool.uv.dev-dependencies` (旧形式) と `tool.uv.sources` によるソース除外
//! - PDM の `tool.pdm.dev-dependencies`
//!
//! 書き換え (`update_version`) は「セクションヘッダのキー + 行頭のドットキー」を連結した
//! 論理パスで対象を決めるため、同じ値へ解決される dotted key
//! (`optional-dependencies.dev = [...]`) と inline table
//! (`optional-dependencies = { dev = [...] }`) も parse と同じ範囲を扱う。
//!
//! プロジェクト全体の既定インデックスが PyPI 以外 (Poetry の primary source /
//! uv の default index・index-url / PDM の `pypi` source 上書き) の場合は、
//! マニフェストごと更新対象から外す。

use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::manifest::{
    ManifestParser,
    line_utils::{HashCommentMode, parse_toml_section_header, strip_hash_line_comment},
};
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

impl ManifestParser for PyprojectTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let toml: Value = toml::from_str(content).map_err(|e: toml::de::Error| {
            ManifestError::TomlParseError {
                path: PathBuf::from("pyproject.toml"),
                message: e.to_string(),
            }
        })?;

        let mut dependencies = Vec::new();

        // プロジェクト全体の既定インデックスが PyPI 以外なら、依存名が同じでも
        // 別物のパッケージを指しているため、マニフェストごと更新対象から外す
        // (社内 private index の依存を PyPI 上の同名パッケージ (typosquat を含む) の
        // 版で書き換える誤更新を防ぐ安全側のスキップ)。
        //
        // 依存が 0 件になるだけだと利用者には「更新なし」と区別がつかないため、
        // 理由を 1 度だけ通知する。`mise` コマンド不在時にマニフェストを外して
        // 警告するのと同じ方針。
        if uses_non_pypi_default_index(&toml) {
            use colored::Colorize as _;
            eprintln!(
                "{}",
                "⚠ pyproject.toml: a non-PyPI default index is configured; \
                 all dependencies are skipped (depup only queries PyPI)"
                    .yellow()
            );
            return Ok(dependencies);
        }

        let parser = get_parser(Language::Python);
        let parser = parser.as_ref();
        let excluded = non_pypi_source_names(&toml);
        let mut ctx = Pep508Collector {
            parser,
            excluded: &excluded,
            output: &mut dependencies,
        };

        // PEP 621 の `project.dependencies` / `project.optional-dependencies`
        let project = toml.get("project");
        ctx.collect_array(project.and_then(|p| p.get("dependencies")), false);
        ctx.collect_group_table(project.and_then(|p| p.get("optional-dependencies")), |_| {
            false
        });

        // PEP 735 の `dependency-groups`
        ctx.collect_group_table(toml.get("dependency-groups"), |group| {
            matches!(group, "dev" | "test" | "lint")
        });

        let tool = toml.get("tool");

        // PDM の開発依存 (`[tool.pdm.dev-dependencies]`、グループ名 → 配列)
        ctx.collect_group_table(
            tool.and_then(|t| t.get("pdm"))
                .and_then(|p| p.get("dev-dependencies")),
            |_| true,
        );

        // Rye / uv (旧形式) の開発依存。uv の `[tool.uv] dev-dependencies` は
        // `[dependency-groups]` へ移行済みだが、既存リポジトリには広く残っている。
        for name in ["rye", "uv"] {
            ctx.collect_array(
                tool.and_then(|t| t.get(name))
                    .and_then(|r| r.get("dev-dependencies")),
                true,
            );
        }

        let poetry = tool.and_then(|t| t.get("poetry"));

        // Poetry の依存関係 / 開発依存 (名前 → 制約 のテーブル)
        collect_poetry_table(
            poetry.and_then(|p| p.get("dependencies")),
            parser,
            false,
            &mut dependencies,
        );
        collect_poetry_table(
            poetry.and_then(|p| p.get("dev-dependencies")),
            parser,
            true,
            &mut dependencies,
        );

        // Poetry 1.2+ のグループ依存
        if let Some(groups) = poetry
            .and_then(|p| p.get("group"))
            .and_then(|g| g.as_table())
        {
            for (group_name, group) in groups {
                let is_dev = group_name == "dev" || group_name == "test";
                collect_poetry_table(group.get("dependencies"), parser, is_dev, &mut dependencies);
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
        let parser = parser.as_ref();
        let manifest = toml::from_str::<Value>(content).ok();

        if let Some(toml) = &manifest {
            if non_pypi_source_names(toml).contains(&normalize_python_package_name(package)) {
                return Err(ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("pyproject.toml"),
                    spec: package.to_string(),
                    message: "package uses a non-PyPI source".to_string(),
                });
            }
            // parse 側と同じ範囲を拒否する (report/apply の整合)
            if uses_non_pypi_default_index(toml) {
                return Err(ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("pyproject.toml"),
                    spec: package.to_string(),
                    message: "manifest configures a non-PyPI default index".to_string(),
                });
            }
        }

        // 配列中の PEP 508 形式: `"package>=1.0,<2.0"` / `"package [extras] (>=1.0); marker"`
        let pep508_re = Regex::new(&format!(
            r#""({}(?:(?:\s*\[[^\]]*\])?\s*(?:[<>=!~^]|\()[^"]*)?)"|'({}(?:(?:\s*\[[^\]]*\])?\s*(?:[<>=!~^]|\()[^']*)?)'"#,
            regex::escape(package),
            regex::escape(package)
        ))
        .ok();

        // 行ごとに「セクションヘッダのキー + 行頭のドットキー」を連結した論理パスを組み立て、
        // parse (toml クレートの値解決) が依存として読む論理パスと同じ範囲だけを更新する。
        // セクションヘッダ文字列の完全一致で判定していた頃は、同じ値へ解決される
        // dotted key (`optional-dependencies.dev = [...]`) や inline table
        // (`optional-dependencies = { dev = [...] }`) を parse だけが読み、writer が
        // 書けずに report/apply が矛盾していた。
        let mut result = String::with_capacity(content.len());
        let mut updated = false;
        // 現在のセクションヘッダのキー列 (ルート直下は空)
        let mut section: Vec<String> = Vec::new();
        // マルチライン文字列 (`"""` / `'''`) の内側かどうか (Some の場合はその区切り)。
        // description 等の docstring 内側では TOML 構文を解釈せず素通しする。
        let mut multiline_delim: Option<&'static str> = None;
        // 依存配列 / inline table が次行以降へ続いている深さ
        let mut open_dependency_depth: i32 = 0;

        for line in content.split_inclusive('\n') {
            // マルチライン文字列の内側は素通しする。docstring に依存配列風のテキスト
            // (`dependencies = [ "requests>=2.0" ]`) があっても誤書き換え・状態汚染しない。
            if let Some(delim) = multiline_delim {
                if line.contains(delim) {
                    multiline_delim = None;
                }
                result.push_str(line);
                continue;
            }

            // セクションヘッダ。依存配列の内側には現れないので、配列内の `["a"]` のような
            // 行をヘッダと誤認しないよう深さ 0 のときだけ判定する。
            if open_dependency_depth == 0
                && let Some(header) = toml_section_header(line)
            {
                section = split_toml_key_path(header).unwrap_or_default();
                result.push_str(line);
                continue;
            }

            // この行でマルチライン文字列が開いて同一行で閉じないなら、以降を素通しする。
            if let Some(delim) = opens_unclosed_multiline_string(line) {
                multiline_delim = Some(delim);
                result.push_str(line);
                continue;
            }

            let (updated_line, is_dependency_line) = if open_dependency_depth > 0 {
                // 複数行に跨る依存配列の内側
                let updated_line = update_pep508_array_line(
                    line,
                    pep508_re.as_ref(),
                    package,
                    parser,
                    new_version,
                );
                (updated_line, true)
            } else if let Some((keys, value_start)) = parse_assignment_key_path(line) {
                let path = logical_key_path(&section, keys);
                if is_pep508_dependency_path(&path) {
                    let updated_line = update_pep508_array_line(
                        line,
                        pep508_re.as_ref(),
                        package,
                        parser,
                        new_version,
                    );
                    (updated_line, true)
                } else {
                    let updated_line = match poetry_target(&path, package) {
                        Some(PoetryTarget::Table) => update_poetry_table_value(
                            line,
                            value_start,
                            package,
                            parser,
                            new_version,
                        ),
                        Some(PoetryTarget::Entry) => {
                            update_poetry_entry_value(line, value_start, parser, new_version)
                        }
                        Some(PoetryTarget::EntryVersion) => quoted_value_span(line, value_start)
                            .and_then(|span| {
                                replace_quoted_version(line, span, parser, new_version)
                            }),
                        None => None,
                    };
                    (updated_line, false)
                }
            } else {
                (None, false)
            };

            // 依存配列 / inline table が次行へ続くかを、文字列・コメント外の括弧で追跡する
            if open_dependency_depth > 0 {
                open_dependency_depth =
                    (open_dependency_depth + line_bracket_depth_delta(line)).max(0);
            } else if is_dependency_line {
                open_dependency_depth = line_bracket_depth_delta(line).max(0);
            }

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

/// TOML のセクションヘッダ行 (`[section]` / `[[section]]`) からドット区切りキーを取り出す。
/// 字句解析は line_utils の共有実装に委譲する (この実装が共有元になった)。
fn toml_section_header(line: &str) -> Option<&str> {
    parse_toml_section_header(line)
}

/// セクションヘッダのキー列と行頭のドットキーを連結し、その行が書いている値の論理パスにする。
/// `[project]` + `optional-dependencies.dev` → `project.optional-dependencies.dev`。
fn logical_key_path(section: &[String], keys: Vec<String>) -> Vec<String> {
    let mut path = section.to_vec();
    path.extend(keys);
    path
}

/// PEP 508 依存指定の配列 (またはそれを直接内包するテーブル) を指す論理パスか判定する。
///
/// 配列そのもの (`project.dependencies`) に加えて、その親テーブル
/// (`project.optional-dependencies` / `dependency-groups` / `tool.pdm.dev-dependencies`)
/// も対象にする。inline table 形式 (`optional-dependencies = { dev = [...] }`) では
/// 1 行に親テーブルごと書かれるため、親を対象にしないと parse は読むのに writer が
/// 書けない (report/apply の矛盾) 状態になる。
///
/// `project` / `tool.uv` のようにメタデータと同居するテーブル自体は対象にしない
/// (テーブルごと inline で書かれた場合に `name` / `description` を巻き込むため。
/// parse は読むので取りこぼしになるが、メタデータ破壊よりは安全側に倒す)。
fn is_pep508_dependency_path(path: &[String]) -> bool {
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    matches!(
        segments.as_slice(),
        ["project", "dependencies"]
            | ["project", "optional-dependencies"]
            | ["project", "optional-dependencies", _]
            | ["dependency-groups"]
            | ["dependency-groups", _]
            | ["tool", "pdm", "dev-dependencies"]
            | ["tool", "pdm", "dev-dependencies", _]
            | ["tool", "rye", "dev-dependencies"]
            | ["tool", "uv", "dev-dependencies"]
    )
}

/// Poetry の依存テーブル (「名前 → 制約」のテーブル) を指す論理パスか判定する。
fn is_poetry_dependency_table_path(path: &[String]) -> bool {
    let segments: Vec<&str> = path.iter().map(String::as_str).collect();
    matches!(
        segments.as_slice(),
        ["tool", "poetry", "dependencies"]
            | ["tool", "poetry", "dev-dependencies"]
            | ["tool", "poetry", "group", _, "dependencies"]
    )
}

/// 行が Poetry 依存のどこを書いているか
enum PoetryTarget {
    /// 依存テーブルそのもの (`dependencies = { requests = "^2.28" }`)
    Table,
    /// 対象パッケージのエントリ (`requests = "^2.28"` / `requests = { version = ... }`)
    Entry,
    /// 対象パッケージの version キー (`version = "^2.28"`)
    EntryVersion,
}

/// 論理パスから、その行が対象パッケージの Poetry 依存を書いているかを判定する。
fn poetry_target(path: &[String], package: &str) -> Option<PoetryTarget> {
    if is_poetry_dependency_table_path(path) {
        return Some(PoetryTarget::Table);
    }
    if let Some((last, head)) = path.split_last()
        && last == package
        && is_poetry_dependency_table_path(head)
    {
        return Some(PoetryTarget::Entry);
    }
    if path.len() >= 2
        && path[path.len() - 1] == "version"
        && path[path.len() - 2] == package
        && is_poetry_dependency_table_path(&path[..path.len() - 2])
    {
        return Some(PoetryTarget::EntryVersion);
    }
    None
}

/// TOML の 1 行を走査し、文字列リテラルの外側にある文字だけを `visit` へ渡す。
///
/// - 単一行の基本文字列 (`"..."`、バックスラッシュエスケープを解釈) とリテラル文字列
///   (`'...'`) の内側は訪問しない。
/// - 文字列外の `#` に達した時点で走査を打ち切る (行コメント)。
/// - 行内で閉じないマルチライン文字列 (`"""` / `'''`) を開いた場合はその区切りを返す。
///
/// マルチライン区切りを単一行クォートより**先に**判定するのが要点。逆にすると
/// `"""` の 1 文字目で単一行の基本文字列に入ったと誤認する。
fn scan_toml_line_outside_strings(
    line: &str,
    mut visit: impl FnMut(usize, char),
) -> Option<&'static str> {
    let mut idx = 0;
    'scan: while idx < line.len() {
        let rest = &line[idx..];

        for delim in ["\"\"\"", "'''"] {
            if let Some(after_open) = rest.strip_prefix(delim) {
                match after_open.find(delim) {
                    // 同一行で閉じるので読み飛ばして走査を続ける
                    Some(close) => {
                        idx += delim.len() + close + delim.len();
                        continue 'scan;
                    }
                    // 開いたまま行が終わる = 以降の行はマルチライン文字列の内側
                    None => return Some(delim),
                }
            }
        }

        let Some(ch) = rest.chars().next() else {
            break;
        };
        match ch {
            '"' | '\'' => idx += skip_single_line_string(rest, ch == '"'),
            // 行コメント以降は TOML 構文として解釈しない
            '#' => return None,
            _ => {
                visit(idx, ch);
                idx += ch.len_utf8();
            }
        }
    }
    None
}

/// 単一行の TOML 文字列を読み飛ばし、消費したバイト数を返す。
/// `basic` が true なら基本文字列 (`"..."`) としてバックスラッシュエスケープを解釈する。
/// 閉じクォートが無ければ行末まで消費する。
fn skip_single_line_string(rest: &str, basic: bool) -> usize {
    let quote = if basic { '"' } else { '\'' };
    let start = quote.len_utf8();
    let mut escaped = false;
    for (offset, ch) in rest[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if basic && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return start + offset + ch.len_utf8();
        }
    }
    rest.len()
}

/// 行がマルチライン文字列 (`"""` / `'''`) を開いて同一行内で閉じない場合、その区切りを返す。
///
/// 行コメント内 (`# ... """`) や別種クォートの内側 (`description = "Ain't got '''"`) の
/// 区切りは無視する。以前は行全体を `find("\"\"\"")` するだけだったため、コメントや
/// 文字列内の区切りで docstring 状態が立ち、以降のファイル全体が更新不能になっていた。
fn opens_unclosed_multiline_string(line: &str) -> Option<&'static str> {
    scan_toml_line_outside_strings(line, |_, _| {})
}

/// 文字列リテラル・行コメントの外側で数えた括弧の増減を返す。
/// 依存配列 (`dependencies = [`) や inline table が次行以降へ続くかの判定に使う。
/// extras (`"foo[extra]"`) のようなクォート内の括弧は数えない。
fn line_bracket_depth_delta(line: &str) -> i32 {
    let mut depth = 0;
    scan_toml_line_outside_strings(line, |_, ch| match ch {
        '[' | '{' => depth += 1,
        ']' | '}' => depth -= 1,
        _ => {}
    });
    depth
}

/// TOML のキーセグメント 1 個 (bare キー / クォート付きキー) を読み、(値, 消費バイト数)
/// を返す。クォート内のエスケープは展開しない (依存名でエスケープを使う例は無い)。
fn read_toml_key_segment(rest: &str) -> Option<(String, usize)> {
    for quote in ['"', '\''] {
        if rest.starts_with(quote) {
            let len = skip_single_line_string(rest, quote == '"');
            let literal = &rest[..len];
            // 閉じクォートが無い断片はキーとして扱わない
            let inner = literal
                .strip_prefix(quote)
                .and_then(|value| value.strip_suffix(quote))?;
            return Some((inner.to_string(), len));
        }
    }
    let len = rest
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .unwrap_or(rest.len());
    (len > 0).then(|| (rest[..len].to_string(), len))
}

/// 文字列の先頭から TOML のドット区切りキーを読み、(セグメント列, 消費バイト数) を返す。
/// 消費バイト数には最後のセグメント直後の空白も含む。
fn read_toml_key_path(text: &str) -> Option<(Vec<String>, usize)> {
    let mut segments = Vec::new();
    let mut idx = 0;
    loop {
        idx += leading_whitespace_len(&text[idx..]);
        let (segment, len) = read_toml_key_segment(text.get(idx..)?)?;
        segments.push(segment);
        idx += len + leading_whitespace_len(&text[idx + len..]);
        match text[idx..].starts_with('.') {
            true => idx += 1,
            false => return Some((segments, idx)),
        }
    }
}

fn leading_whitespace_len(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// ドット区切りキー全体 (セクションヘッダ等) をセグメント列へ分解する。
/// 末尾に余分な文字が残る場合は解釈できないものとして `None` を返す。
fn split_toml_key_path(key: &str) -> Option<Vec<String>> {
    let (segments, consumed) = read_toml_key_path(key)?;
    key[consumed..].trim().is_empty().then_some(segments)
}

/// `text[idx..]` の先頭から `key = ` を読み、(キーのセグメント列, 値の開始位置) を返す。
/// 行頭の代入と inline table のエントリで共有する。
fn read_key_assignment(text: &str, idx: usize) -> Option<(Vec<String>, usize)> {
    let (segments, consumed) = read_toml_key_path(text.get(idx..)?)?;
    let after_key = idx + consumed;
    if !text.get(after_key..)?.starts_with('=') {
        return None;
    }
    let value_start = after_key + 1;
    Some((
        segments,
        value_start + leading_whitespace_len(text.get(value_start..)?),
    ))
}

/// 行頭が `key = ...` (ドット区切りキー可) なら、キーのセグメント列と値の開始位置を返す。
/// 配列要素の行 (`"requests>=2.0",`)・コメント行・継続行は `None`。
fn parse_assignment_key_path(line: &str) -> Option<(Vec<String>, usize)> {
    let indent = leading_whitespace_len(line);
    if line[indent..].starts_with('#') {
        return None;
    }
    read_key_assignment(line, indent)
}

/// inline table (`{ ... }`) の直下エントリを (キーのセグメント列, 値の開始位置) で列挙する。
///
/// ネストした inline table / 配列 / 文字列リテラルの内側は読み飛ばす。文字列を読み飛ばすのが
/// 要点で、素朴に `version\s*=` を探すと `postinstall = 'version = "x"'` のような
/// 別フィールドの中身を書き換えてしまう。
fn inline_table_entries(line: &str, brace_start: usize) -> Vec<(Vec<String>, usize)> {
    let mut entries = Vec::new();
    let mut idx = brace_start + 1;
    let mut depth: i32 = 0;
    let mut at_entry_start = true;

    while idx < line.len() {
        let rest = &line[idx..];
        let ws = leading_whitespace_len(rest);
        if ws > 0 {
            idx += ws;
            continue;
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };

        if at_entry_start
            && depth == 0
            && ch != '}'
            && let Some((keys, value_start)) = read_key_assignment(line, idx)
        {
            entries.push((keys, value_start));
            idx = value_start;
            at_entry_start = false;
            continue;
        }

        match ch {
            '"' | '\'' => idx += skip_single_line_string(rest, ch == '"'),
            '{' | '[' => {
                depth += 1;
                idx += 1;
            }
            '}' | ']' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                idx += 1;
            }
            ',' if depth == 0 => {
                at_entry_start = true;
                idx += 1;
            }
            '#' => break,
            _ => idx += ch.len_utf8(),
        }
    }

    entries
}

/// 行の指定位置にあるクォート値の「中身」のバイト範囲を返す。
/// 値がクォート文字列でない (inline table / 配列 / 真偽値等) 場合は `None`。
fn quoted_value_span(line: &str, value_start: usize) -> Option<(usize, usize)> {
    let rest = line.get(value_start..)?;
    let quote = rest.chars().next().filter(|ch| matches!(ch, '"' | '\''))?;
    let len = skip_single_line_string(rest, quote == '"');
    // 閉じクォートが行内に無い値は書き換えない
    if len <= quote.len_utf8() || !rest[..len].ends_with(quote) {
        return None;
    }
    Some((
        value_start + quote.len_utf8(),
        value_start + len - quote.len_utf8(),
    ))
}

/// クォート値の中身を、更新後のバージョン制約へ差し替えた行を返す。
/// 引用符の種別 (`"` / `'`) は中身だけを置換するので自動的に保たれる。
fn replace_quoted_version(
    line: &str,
    span: (usize, usize),
    parser: &dyn VersionParser,
    new_version: &str,
) -> Option<String> {
    let (start, end) = span;
    // Poetry コンテキストでは演算子なしの bare バージョンも完全一致ピンとして扱う
    let spec = parser.parse_exact_pin(&line[start..end])?;
    let updated = spec.try_format_updated(new_version)?;
    Some(format!("{}{}{}", &line[..start], updated, &line[end..]))
}

/// Poetry の依存エントリの値 (`"^1.0"` / `{ version = "^1.0", ... }`) を更新する。
fn update_poetry_entry_value(
    line: &str,
    value_start: usize,
    parser: &dyn VersionParser,
    new_version: &str,
) -> Option<String> {
    // `name = "^1.0.0"` (演算子付き / bare の文字列指定)
    if let Some(span) = quoted_value_span(line, value_start) {
        return replace_quoted_version(line, span, parser, new_version);
    }

    // `name = { version = "^1.0.0", ... }`
    if !line[value_start..].starts_with('{') {
        // 配列 (Poetry のマルチプル制約) 等は安全に更新できないので触らない
        return None;
    }
    let entries = inline_table_entries(line, value_start);
    // PyPI 以外の source を持つ inline table は PyPI の候補で書き換えない
    if entries.iter().any(|(keys, start)| {
        keys.len() == 1
            && keys[0] == "source"
            && quoted_value_span(line, *start)
                .is_some_and(|(s, e)| source_name_is_non_pypi(&line[s..e]))
    }) {
        return None;
    }
    let version_start = entries
        .iter()
        .find(|(keys, _)| keys.len() == 1 && keys[0] == "version")
        .map(|(_, start)| *start)?;
    let span = quoted_value_span(line, version_start)?;
    replace_quoted_version(line, span, parser, new_version)
}

/// Poetry の依存テーブルが inline table で書かれている場合
/// (`dependencies = { requests = "^2.28", ... }`) に、対象パッケージのエントリを更新する。
fn update_poetry_table_value(
    line: &str,
    value_start: usize,
    package: &str,
    parser: &dyn VersionParser,
    new_version: &str,
) -> Option<String> {
    if !line[value_start..].starts_with('{') {
        return None;
    }
    for (keys, start) in inline_table_entries(line, value_start) {
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        match keys.as_slice() {
            // `{ requests = "^2.28" }` / `{ requests = { version = "^2.28" } }`
            [name] if *name == package => {
                return update_poetry_entry_value(line, start, parser, new_version);
            }
            // dotted key の `{ requests.version = "^2.28" }`
            [name, "version"] if *name == package => {
                let span = quoted_value_span(line, start)?;
                return replace_quoted_version(line, span, parser, new_version);
            }
            _ => {}
        }
    }
    None
}

/// TOML 行から `#` 以降の行コメントを取り除く。文字列リテラル内 (`"..."` / `'...'`)
/// の `#` は保持する。改行コードや末尾の空白はそのまま残す。
fn strip_toml_line_comment(line: &str) -> &str {
    // TOML ではバックスラッシュをリテラル扱いする (Plain モード)
    strip_hash_line_comment(line, HashCommentMode::Plain)
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

fn source_name_is_non_pypi(source: &str) -> bool {
    !source.eq_ignore_ascii_case("pypi")
}

fn poetry_table_has_non_pypi_source(table: &toml::map::Map<String, Value>) -> bool {
    table
        .get("source")
        .and_then(|v| v.as_str())
        .is_some_and(source_name_is_non_pypi)
}

/// uv の `[tool.uv.sources]` エントリが PyPI 以外のソースを指しているか判定する。
///
/// `workspace = true` (ローカル workspace メンバー) / `git` / `path` / `url` は
/// PyPI 上の同名パッケージとは別物であり、`index = "..."` はカスタムインデックス
/// 指定なので、いずれも PyPI の候補で書き換えてはいけない
/// (Poetry の `source = "..."` と同じ安全側の扱い)。
fn uv_source_is_non_pypi(table: &toml::map::Map<String, Value>) -> bool {
    if table.contains_key("git") || table.contains_key("path") || table.contains_key("url") {
        return true;
    }
    if table.get("workspace").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    table
        .get("index")
        .and_then(|v| v.as_str())
        .is_some_and(source_name_is_non_pypi)
}

/// `[tool.uv.sources]` で PyPI 以外のソースを指定された依存名を集める。
/// 値はテーブル、または環境マーカー別のテーブル配列 (`[[tool.uv.sources.foo]]`) を取りうる。
fn uv_non_pypi_source_names(toml: &Value) -> HashSet<String> {
    let mut names = HashSet::new();
    let Some(sources) = toml
        .get("tool")
        .and_then(|t| t.get("uv"))
        .and_then(|u| u.get("sources"))
        .and_then(|s| s.as_table())
    else {
        return names;
    };

    for (name, value) in sources {
        let is_non_pypi = match value {
            Value::Table(table) => uv_source_is_non_pypi(table),
            Value::Array(items) => items
                .iter()
                .any(|item| item.as_table().is_some_and(uv_source_is_non_pypi)),
            _ => false,
        };
        if is_non_pypi {
            names.insert(normalize_python_package_name(name));
        }
    }
    names
}

/// PyPI の候補で書き換えてはいけない依存名を、Poetry と uv の両方から集める。
fn non_pypi_source_names(toml: &Value) -> HashSet<String> {
    let mut names = poetry_non_pypi_source_names(toml);
    names.extend(uv_non_pypi_source_names(toml));
    names
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

/// インデックス URL が PyPI 本体を指しているか判定する。
///
/// ホスト部だけを見て `pypi.org` / `*.pypi.org` を PyPI とみなす。
/// `pypi.org.evil.example` のような類似ホストは、`.pypi.org` で終わらないので弾かれる。
fn index_url_is_pypi(url: &str) -> bool {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // userinfo (`user:pass@host`) とポート番号を落とす
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "pypi.org" || host.ends_with(".pypi.org")
}

/// Poetry の `[[tool.poetry.source]]` が暗黙の PyPI を無効化しているか判定する。
///
/// Poetry は primary / default 優先度のソースを 1 つでも宣言すると暗黙の PyPI を
/// 無効化する。`priority` 省略時は primary 扱い、legacy の `default = true` も
/// PyPI を置き換える。`name = "pypi"` が併記されていれば PyPI は生きているので対象外。
/// `secondary = true` (legacy) / `supplemental` / `explicit` は PyPI を残すので対象外。
fn poetry_disables_default_pypi(toml: &Value) -> bool {
    let Some(sources) = toml
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("source"))
        .and_then(|s| s.as_array())
    else {
        return false;
    };

    let pypi_declared = sources.iter().any(|source| {
        source
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("pypi"))
    });
    if pypi_declared {
        return false;
    }

    sources.iter().any(|source| {
        match source.get("priority").and_then(Value::as_str) {
            Some(priority) => {
                priority.eq_ignore_ascii_case("default") || priority.eq_ignore_ascii_case("primary")
            }
            // priority 省略は primary 扱い。ただし legacy の `secondary = true` は
            // 補助ソースなので PyPI を無効化しない。
            None => source.get("secondary").and_then(Value::as_bool) != Some(true),
        }
    })
}

/// uv の既定インデックスが PyPI 以外を指しているか判定する。
/// `[[tool.uv.index]]` の `default = true` と `[tool.uv] index-url` の両方を見る。
fn uv_disables_default_pypi(toml: &Value) -> bool {
    let Some(uv) = toml.get("tool").and_then(|t| t.get("uv")) else {
        return false;
    };

    let default_index_is_non_pypi =
        uv.get("index")
            .and_then(|i| i.as_array())
            .is_some_and(|indexes| {
                indexes.iter().any(|index| {
                    index.get("default").and_then(Value::as_bool) == Some(true)
                        && index
                            .get("url")
                            .and_then(Value::as_str)
                            .is_some_and(|url| !index_url_is_pypi(url))
                })
            });
    if default_index_is_non_pypi {
        return true;
    }

    uv.get("index-url")
        .and_then(Value::as_str)
        .is_some_and(|url| !index_url_is_pypi(url))
}

/// PDM の `[[tool.pdm.source]]` が既定の PyPI を別 URL で上書きしているか判定する。
/// PDM では `name = "pypi"` のエントリが既定インデックスを置き換える。
fn pdm_overrides_default_pypi(toml: &Value) -> bool {
    toml.get("tool")
        .and_then(|t| t.get("pdm"))
        .and_then(|p| p.get("source"))
        .and_then(|s| s.as_array())
        .is_some_and(|sources| {
            sources.iter().any(|source| {
                source
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case("pypi"))
                    && source
                        .get("url")
                        .and_then(Value::as_str)
                        .is_some_and(|url| !index_url_is_pypi(url))
            })
        })
}

/// マニフェスト全体の既定インデックスが PyPI 以外かを判定する。
///
/// 依存単位の `source` / `[tool.uv.sources]` 指定 (`non_pypi_source_names`) と違い、
/// これらの設定は**すべての依存**の解決先を差し替える。`internal-auth = "^1.2.0"` の
/// ように依存側には何の印も無いまま private index から取得されるため、PyPI 上の
/// 同名パッケージ (typosquat を含む) の版で書き換えると別物へ差し替わる。
/// Cargo の `registry = "..."` 付き依存を除外しているのと同型の防御。
fn uses_non_pypi_default_index(toml: &Value) -> bool {
    poetry_disables_default_pypi(toml)
        || uv_disables_default_pypi(toml)
        || pdm_overrides_default_pypi(toml)
}

/// PEP 508 依存配列を読み取るコンテキスト。
///
/// `[project]` / `[dependency-groups]` / `[tool.pdm.dev-dependencies]` / `[tool.rye]` /
/// `[tool.uv]` はいずれも「PEP 508 文字列の配列」または「グループ名 → 配列」の形なので、
/// 走査を 1 箇所へ集約する (非 PyPI ソース除外の適用漏れも構造的に防ぐ)。
struct Pep508Collector<'a> {
    parser: &'a dyn VersionParser,
    excluded: &'a HashSet<String>,
    output: &'a mut Vec<Dependency>,
}

impl Pep508Collector<'_> {
    /// PEP 508 文字列の配列を読む
    fn collect_array(&mut self, value: Option<&Value>, is_dev: bool) {
        let Some(items) = value.and_then(|v| v.as_array()) else {
            return;
        };
        for item in items {
            if let Some(dep_str) = item.as_str()
                && !pep508_uses_non_pypi_source(dep_str, self.excluded)
                && let Some(parsed) = parse_pep508_dependency(dep_str, self.parser, is_dev)
            {
                self.output.push(parsed);
            }
        }
    }

    /// 「グループ名 → PEP 508 配列」のテーブルを読む。
    /// 開発依存かどうかはグループ名から判定する。
    fn collect_group_table(&mut self, value: Option<&Value>, is_dev: impl Fn(&str) -> bool) {
        let Some(groups) = value.and_then(|v| v.as_table()) else {
            return;
        };
        for (group_name, deps) in groups {
            self.collect_array(Some(deps), is_dev(group_name));
        }
    }
}

/// Poetry の「名前 → 制約」テーブルを読む。
/// Python 自体の要求バージョンは依存更新の対象にしない。
fn collect_poetry_table(
    value: Option<&Value>,
    parser: &dyn VersionParser,
    is_dev: bool,
    output: &mut Vec<Dependency>,
) {
    let Some(deps) = value.and_then(|v| v.as_table()) else {
        return;
    };
    for (name, value) in deps {
        if name == "python" {
            continue;
        }
        if let Some(parsed) = parse_poetry_dependency(name, value, parser, is_dev) {
            output.push(parsed);
        }
    }
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

    // Poetry の bare バージョン (`requests = "2.28.0"`) は完全一致ピンなので、
    // parse_exact_pin で拾って更新チェック対象に含める (明示 `==2.28.0` と挙動を揃える)。
    let spec = parser.parse_exact_pin(&version_str)?;
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
    fn test_parse_poetry_bare_version_is_exact_pin() {
        // Poetry の演算子なしバージョン (`django = "4.2.1"`) は完全一致ピン。
        // 明示 `== ` 版と同様に依存として surface し (以前は無言でドロップされ、
        // `--include-pinned` を付けても更新チェック対象にすらならなかった)、
        // Exact/pinned として扱われる。inline table 形式も同様。
        let content = r#"
[tool.poetry.dependencies]
python = "^3.11"
requests = "^2.28.0"
django = "4.2.1"
numpy = { version = "1.26.0", optional = true }
"#;

        let deps = parse(content).unwrap();

        let django = deps
            .iter()
            .find(|d| d.name == "django")
            .expect("bare 版の django が依存として拾われていない");
        assert_eq!(django.version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(django.version_spec.version, "4.2.1");
        assert!(django.is_pinned());

        let numpy = deps
            .iter()
            .find(|d| d.name == "numpy")
            .expect("inline table の bare 版 numpy が拾われていない");
        assert_eq!(numpy.version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(numpy.version_spec.version, "1.26.0");
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
    fn test_update_poetry_bare_version_pin() {
        // Poetry の演算子なし完全一致ピンを更新すると、演算子を付けずに
        // 新バージョンへ書き換える (`4.2.1` → `5.0.0`、`==` は付かない)。
        // simple 形式と inline table 形式の両方で動くこと。
        let content = r#"
[tool.poetry.dependencies]
django = "4.2.1"
numpy = { version = '1.26.0', optional = true }
"#;

        let django = PyprojectTomlParser
            .update_version(content, "django", "5.0.0")
            .expect("bare 版 django の更新に失敗");
        assert!(
            django.contains("django = \"5.0.0\""),
            "bare 版を演算子なしで更新できていない: {django}"
        );

        let numpy = PyprojectTomlParser
            .update_version(content, "numpy", "2.0.0")
            .expect("inline table の bare 版 numpy の更新に失敗");
        assert!(
            numpy.contains("version = '2.0.0'"),
            "inline table の bare 版 (単一引用符) を更新できていない: {numpy}"
        );
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
    fn test_update_version_ignores_multiline_string_content() {
        // 回帰: [project] の description 等のマルチライン文字列 (""") の中に依存配列風の
        // テキストがあっても書き換えない。以前はマルチライン文字列を追跡せず、docstring 内の
        // `dependencies = [ "requests>=2.0" ]` を本物の配列と誤認して版を破壊していた。
        let content = r#"[project]
name = "mypkg"
description = """
Install example:
dependencies = [
  "requests>=2.0",
]
"""
dependencies = [
    "requests>=2.0",
]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        // 本物の依存だけが更新され、docstring 内の擬似依存は保持される
        assert_eq!(
            result.matches("requests>=2.0").count(),
            1,
            "docstring 内の擬似依存が破壊された: {result}"
        );
        assert_eq!(
            result.matches("requests>=2.31.0").count(),
            1,
            "本物の依存が更新されていない: {result}"
        );
    }

    #[test]
    fn test_update_version_multiline_string_unclosed_array_no_leak() {
        // 回帰: docstring 内に閉じない `dependencies = [` があっても、状態が漏れて
        // 後続の本物のメタデータ配列 (`keywords` 等) を破壊しないこと。
        let content = r#"[project]
name = "mypkg"
description = """
dependencies = [
"""
keywords = ["evil>=9.9"]
dependencies = [
    "evil>=1.0",
]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "evil", "2.0.0")
            .unwrap();
        // keywords 内の文字列は依存ではないので触らない
        assert!(
            result.contains(r#"keywords = ["evil>=9.9"]"#),
            "keywords が破壊された: {result}"
        );
        assert!(
            result.contains("\"evil>=2.0.0\""),
            "本物の依存未更新: {result}"
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
        // Poetry の `~2.20` は `>=2.20 <2.21`。セグメント数を保って `~2.31` にする
        assert!(
            result.contains(r#"requests = "~2.31""#),
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

    // --- uv / PDM / 多行 Poetry テーブル / クォート付きキーの回帰テスト ---

    /// 回帰テスト: `[tool.uv.sources]` で PyPI 以外を指す依存は更新対象にしない。
    /// workspace メンバーやカスタムインデックス指定を PyPI の同名パッケージ
    /// (typosquat を含む) の版で書き換える誤更新を防ぐ。
    #[test]
    fn test_uv_sources_non_pypi_dependencies_skipped() {
        let content = r#"
[project]
dependencies = ["mylib>=0.1.0", "torch>=2.4.0", "requests>=2.28.0"]

[tool.uv.sources]
mylib = { workspace = true }
torch = { index = "pytorch-cu124" }
"#;

        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["requests"]);

        // 書き換え側も同じ範囲を拒否する (parse と write の整合)
        let err = PyprojectTomlParser.update_version(content, "mylib", "9.9.9");
        assert!(err.is_err());
    }

    /// `[tool.uv.sources]` の git / path / url 指定も同様に除外する。
    #[test]
    fn test_uv_sources_git_path_url_skipped() {
        let content = r#"
[project]
dependencies = ["a>=1.0", "b>=1.0", "c>=1.0", "d>=1.0"]

[tool.uv.sources]
a = { git = "https://github.com/example/a" }
b = { path = "../b" }
c = { url = "https://example.com/c.whl" }
"#;

        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["d"]);
    }

    /// `index = "pypi"` は PyPI そのものなので除外しない。
    #[test]
    fn test_uv_sources_pypi_index_not_skipped() {
        let content = r#"
[project]
dependencies = ["requests>=2.28.0"]

[tool.uv.sources]
requests = { index = "pypi" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
    }

    /// 回帰テスト: uv 旧形式の `[tool.uv] dev-dependencies` を parse / update とも扱う。
    #[test]
    fn test_uv_dev_dependencies_parse_and_update() {
        let content = r#"
[tool.uv]
dev-dependencies = ["ruff>=0.5.0", "pytest>=8.0"]
constraint-dependencies = ["ruff>=0.1.0"]
"#;

        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["ruff", "pytest"]);
        assert!(deps.iter().all(|d| d.is_dev));

        let result = PyprojectTomlParser
            .update_version(content, "ruff", "0.9.2")
            .unwrap();
        assert!(result.contains(r#""ruff>=0.9.2""#), "{}", result);
        // constraint-dependencies は parse 対象外なので書き換えない
        assert!(result.contains(r#"constraint-dependencies = ["ruff>=0.1.0"]"#));
    }

    /// 回帰テスト: PDM の `[tool.pdm.dev-dependencies]` を parse / update とも扱う。
    #[test]
    fn test_pdm_dev_dependencies_parse_and_update() {
        let content = r#"
[tool.pdm.dev-dependencies]
test = ["pytest>=8.0"]
lint = ["ruff>=0.5.0"]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().all(|d| d.is_dev));

        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#""pytest>=8.4.0""#), "{}", result);
    }

    /// 回帰テスト: Poetry の多行依存テーブルは parse が依存として読むため、
    /// update も同じ範囲を書き換えられること (以前は報告後に書き込みが失敗していた)。
    #[test]
    fn test_poetry_multiline_dependency_table_parse_and_update() {
        let content = r#"
[tool.poetry.dependencies.requests]
version = "^2.28.0"
extras = ["security"]

[tool.poetry.group.dev.dependencies.pytest]
version = "^7.0"
"#;

        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"requests"), "{:?}", names);
        assert!(names.contains(&"pytest"), "{:?}", names);

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains(r#"version = "^2.31.0""#), "{}", result);
        // 別パッケージのテーブルは書き換えない
        assert!(result.contains(r#"version = "^7.0""#), "{}", result);

        // Caret はセグメント数を保たない (許容幅が常に次メジャーで変わらないため)
        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#"version = "^8.4.0""#), "{}", result);
    }

    /// 回帰テスト: ドットを含む名前は TOML でクォートが必須なので、
    /// クォート付きキーも更新できること (`zope.interface` / `ruamel.yaml` 等)。
    #[test]
    fn test_poetry_quoted_key_update() {
        let content = r#"
[tool.poetry.dependencies]
"zope.interface" = "^5.4.0"
"ruamel.yaml" = { version = "^0.18.5", optional = true }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "zope.interface", "6.1.0")
            .unwrap();
        assert!(
            result.contains(r#""zope.interface" = "^6.1.0""#),
            "{}",
            result
        );

        let result = PyprojectTomlParser
            .update_version(content, "ruamel.yaml", "0.19.0")
            .unwrap();
        assert!(result.contains(r#"version = "^0.19.0""#), "{}", result);
    }

    /// クォートなしでドットを含むヘッダはネストしたテーブルなので、単一パッケージ名として扱わない。
    /// `[tool.poetry.dependencies.foo.bar]` は toml クレートも `foo` の下の `bar` テーブルと
    /// 読むため parse が依存として surface せず、writer も書き換えてはいけない。
    #[test]
    fn test_poetry_dependency_table_rejects_bare_dotted_tail() {
        let content = r#"
[tool.poetry.dependencies.foo.bar]
version = "^1.0.0"
"#;
        // parse も update もこの形を単一パッケージとして扱わない (parse/write の整合)
        assert!(parse(content).unwrap().is_empty());
        for package in ["foo", "bar", "foo.bar"] {
            assert!(
                PyprojectTomlParser
                    .update_version(content, package, "2.0.0")
                    .is_err(),
                "package={package}"
            );
        }

        // クォート付きの単一セグメントは従来どおり対象
        let quoted = r#"
[tool.poetry.dependencies."zope.interface"]
version = "^5.4.0"
"#;
        let result = PyprojectTomlParser
            .update_version(quoted, "zope.interface", "6.1.0")
            .unwrap();
        assert!(result.contains(r#"version = "^6.1.0""#), "{result}");
    }

    // --- 修正: dotted key / inline table 形式の依存宣言 (report/apply の整合) ---

    /// 回帰テスト: `[project]` 内の dotted key で書かれた optional-dependencies も
    /// parse / update の両方で扱えること。
    /// 以前は parse (toml クレートの値解決) だけが読み、writer はセクションヘッダと
    /// キー名の完全一致しか見ていなかったため
    /// 「更新あり」と報告した後に必ず書き込みが失敗していた。
    #[test]
    fn test_dotted_key_optional_dependencies_parse_and_update() {
        let content = r#"
[project]
name = "x"
optional-dependencies.dev = ["pytest>=7.0.0"]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pytest");

        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#""pytest>=8.4.0""#), "{result}");
    }

    /// 回帰テスト: inline table で書かれた optional-dependencies も更新できること。
    #[test]
    fn test_inline_table_optional_dependencies_parse_and_update() {
        let content = r#"
[project]
name = "x"
optional-dependencies = { dev = ["pytest>=7.0.0"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pytest");

        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#""pytest>=8.4.0""#), "{result}");
    }

    /// 回帰テスト: 複数行に跨る dotted key の依存配列も配列の内側として追跡すること。
    #[test]
    fn test_dotted_key_multiline_dependency_array() {
        let content = r#"
[project]
name = "x"
optional-dependencies.dev = [
    "pytest>=7.0.0",
    "ruff>=0.5.0",
]
keywords = ["pytest>=9.9.9"]
"#;

        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#""pytest>=8.4.0""#), "{result}");
        assert!(result.contains(r#""ruff>=0.5.0""#), "{result}");
        // 配列を抜けた後のメタデータ配列は書き換えない
        assert!(
            result.contains(r#"keywords = ["pytest>=9.9.9"]"#),
            "{result}"
        );
    }

    /// 回帰テスト: ルート直下に書かれた `project.dependencies` /
    /// `dependency-groups` (inline table) も更新できること。
    #[test]
    fn test_root_level_dotted_and_inline_dependency_tables() {
        let content = r#"
project.dependencies = ["requests>=2.28.0"]
dependency-groups = { dev = ["pytest>=7.0.0"] }
"#;

        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"requests"), "{names:?}");
        assert!(names.contains(&"pytest"), "{names:?}");

        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains(r#""requests>=2.31.0""#), "{result}");

        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#""pytest>=8.4.0""#), "{result}");
    }

    /// 回帰テスト: PDM の dev-dependencies を inline table で書いた場合も更新できること。
    #[test]
    fn test_pdm_inline_dev_dependencies_update() {
        let content = r#"
[tool.pdm]
dev-dependencies = { test = ["pytest>=8.0"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pytest");

        let result = PyprojectTomlParser
            .update_version(content, "pytest", "8.4.0")
            .unwrap();
        assert!(result.contains(r#""pytest>=8.4.0""#), "{result}");
    }

    /// 回帰テスト: Poetry の依存テーブルを inline table / dotted key で書いた場合も
    /// 更新でき、同名プレフィックスの別パッケージを壊さないこと。
    #[test]
    fn test_poetry_inline_and_dotted_dependency_table_update() {
        let inline = r#"
[tool.poetry]
dependencies = { requests = "^2.28", types-requests = "^2.28", numpy = { version = "1.26.0" } }
"#;

        let deps = parse(inline).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"requests"), "{names:?}");

        // Caret はセグメント数を保たない (許容幅が常に次メジャーで変わらないため)
        let result = PyprojectTomlParser
            .update_version(inline, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains(r#"requests = "^2.31.0""#), "{result}");
        assert!(
            result.contains(r#"types-requests = "^2.28""#),
            "同名プレフィックスの別パッケージを壊してはいけない: {result}"
        );

        // inline table 内のネストした inline table (`numpy = { version = ... }`)
        let result = PyprojectTomlParser
            .update_version(inline, "numpy", "2.0.0")
            .unwrap();
        assert!(result.contains(r#"{ version = "2.0.0" }"#), "{result}");

        // dotted key 形式 (`dependencies.requests = "^2.28"`)
        let dotted = r#"
[tool.poetry]
dependencies.requests = "^2.28"
dependencies.numpy = { version = "1.26.0" }
"#;
        let deps = parse(dotted).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"requests"), "{names:?}");

        let result = PyprojectTomlParser
            .update_version(dotted, "requests", "2.31.0")
            .unwrap();
        assert!(
            result.contains(r#"dependencies.requests = "^2.31.0""#),
            "{result}"
        );

        let result = PyprojectTomlParser
            .update_version(dotted, "numpy", "2.0.0")
            .unwrap();
        assert!(
            result.contains(r#"dependencies.numpy = { version = "2.0.0" }"#),
            "{result}"
        );
    }

    /// inline table 形式でも、PyPI 以外の source を持つ依存は書き換えない。
    #[test]
    fn test_poetry_inline_table_non_pypi_source_not_updated() {
        let content = r#"
[tool.poetry]
dependencies = { private-pkg = { version = "^1.0", source = "internal" } }
"#;

        // parse も update も対象外 (report/apply の整合)
        assert!(parse(content).unwrap().is_empty());
        assert!(
            PyprojectTomlParser
                .update_version(content, "private-pkg", "1.2.0")
                .is_err()
        );
    }

    /// inline table の別フィールドに `version = "..."` を含む文字列があっても、
    /// そこを書き換えないこと (文字列リテラルの内側を読み飛ばす)。
    #[test]
    fn test_poetry_inline_table_ignores_version_inside_string_literal() {
        let content = r#"
[tool.poetry.dependencies]
mypkg = { markers = 'version = "9.9.9"', version = "^1.0.0" }
"#;

        let result = PyprojectTomlParser
            .update_version(content, "mypkg", "2.0.0")
            .unwrap();
        assert!(
            result.contains(r#"markers = 'version = "9.9.9"'"#),
            "別フィールドの文字列を壊してはいけない: {result}"
        );
        assert!(result.contains(r#"version = "^2.0.0""#), "{result}");
    }

    // --- 修正: コメント内・クォート内の `"""` / `'''` の誤検出 ---

    /// 回帰テスト: 行コメント内の `"""` をマルチライン文字列開始と誤認しないこと。
    /// 以前は誤認して以降の全行が素通しになり、依存を「更新あり」と報告した後に
    /// 書き込みが必ず失敗していた。
    #[test]
    fn test_update_version_ignores_multiline_delimiter_in_comment() {
        let content = r#"[project]
name = "x"
# NOTE: description には """ を使わないこと
dependencies = [
    "requests>=2.28.0",
]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains(r#""requests>=2.31.0""#), "{result}");
    }

    /// 回帰テスト: 基本文字列の内側の `'''`、リテラル文字列の内側の `"""` も
    /// マルチライン文字列開始と誤認しないこと。
    #[test]
    fn test_update_version_ignores_multiline_delimiter_inside_strings() {
        let content = r#"[project]
description = "Ain't got '''"
readme = 'see """ marker'
dependencies = [
    "requests>=2.28.0",
]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains(r#""requests>=2.31.0""#), "{result}");
        assert!(
            result.contains(r#"description = "Ain't got '''""#),
            "{result}"
        );
        assert!(result.contains(r#"readme = 'see """ marker'"#), "{result}");
    }

    /// 制御: 本物のマルチライン文字列 (`'''`) の内側は従来どおり素通しする。
    #[test]
    fn test_update_version_still_skips_real_literal_multiline_string() {
        let content = r#"[project]
description = '''
dependencies = [
  "requests>=2.0",
]
'''
dependencies = [
    "requests>=2.0",
]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert_eq!(
            result.matches("requests>=2.0").count(),
            1,
            "docstring 内の擬似依存が破壊された: {result}"
        );
        assert_eq!(
            result.matches("requests>=2.31.0").count(),
            1,
            "本物の依存が更新されていない: {result}"
        );
    }

    // --- 修正: プロジェクト全体の既定インデックスが PyPI 以外 ---

    /// 回帰テスト: Poetry の primary / default source が宣言されていると暗黙の PyPI が
    /// 無効化される。依存側には印が無いまま private index から解決されるため、
    /// PyPI 上の同名パッケージ (typosquat を含む) の版で書き換えてはいけない。
    #[test]
    fn test_poetry_primary_source_disables_updates() {
        for priority in [
            r#"priority = "primary""#,
            r#"priority = "default""#,
            "default = true",
            "", // priority 省略は primary 扱い
        ] {
            let content = format!(
                r#"
[[tool.poetry.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
{priority}

[tool.poetry.dependencies]
internal-auth = "^1.2.0"
"#
            );

            assert!(
                parse(&content).unwrap().is_empty(),
                "priority={priority:?} で依存が surface している"
            );
            assert!(
                PyprojectTomlParser
                    .update_version(&content, "internal-auth", "9.9.9")
                    .is_err(),
                "priority={priority:?} で書き換えられている"
            );
        }
    }

    /// PyPI が併記されていたり補助的な優先度なら、PyPI は生きているので従来どおり更新する。
    #[test]
    fn test_poetry_source_keeps_updates_when_pypi_is_alive() {
        let cases = [
            // PyPI を明示的に併記
            r#"
[[tool.poetry.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
priority = "primary"

[[tool.poetry.source]]
name = "pypi"
priority = "supplemental"
"#,
            // 補助的な優先度のみ
            r#"
[[tool.poetry.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
priority = "supplemental"
"#,
            r#"
[[tool.poetry.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
priority = "explicit"
"#,
            // legacy の secondary
            r#"
[[tool.poetry.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
secondary = true
"#,
            // source 設定なし
            "",
        ];

        for source in cases {
            let content = format!(
                r#"{source}
[tool.poetry.dependencies]
internal-auth = "^1.2.0"
"#
            );
            let deps = parse(&content).unwrap();
            assert_eq!(deps.len(), 1, "source={source}");
            assert!(
                PyprojectTomlParser
                    .update_version(&content, "internal-auth", "1.9.0")
                    .is_ok(),
                "source={source}"
            );
        }
    }

    /// 回帰テスト: uv の既定インデックス差し替え (`[[tool.uv.index]] default = true` /
    /// `[tool.uv] index-url`) も PyPI 以外なら更新対象から外す。
    #[test]
    fn test_uv_default_index_disables_updates() {
        let default_index = r#"
[project]
dependencies = ["internal-auth>=1.2.0"]

[[tool.uv.index]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
default = true
"#;
        assert!(parse(default_index).unwrap().is_empty());
        assert!(
            PyprojectTomlParser
                .update_version(default_index, "internal-auth", "9.9.9")
                .is_err()
        );

        let index_url = r#"
[project]
dependencies = ["internal-auth>=1.2.0"]

[tool.uv]
index-url = "https://pypi.internal.example.com/simple/"
"#;
        assert!(parse(index_url).unwrap().is_empty());

        // PyPI そのものを指す設定なら従来どおり更新する
        let pypi_index = r#"
[project]
dependencies = ["requests>=2.28.0"]

[[tool.uv.index]]
name = "pypi"
url = "https://pypi.org/simple"
default = true
"#;
        assert_eq!(parse(pypi_index).unwrap().len(), 1);

        // 既定でない追加インデックスは PyPI を無効化しない
        let extra_index = r#"
[project]
dependencies = ["requests>=2.28.0"]

[[tool.uv.index]]
name = "extra"
url = "https://pypi.internal.example.com/simple/"
"#;
        assert_eq!(parse(extra_index).unwrap().len(), 1);
    }

    /// 回帰テスト: PDM の `[[tool.pdm.source]]` による `pypi` 上書きも更新対象から外す。
    #[test]
    fn test_pdm_pypi_source_override_disables_updates() {
        let content = r#"
[project]
dependencies = ["internal-auth>=1.2.0"]

[[tool.pdm.source]]
name = "pypi"
url = "https://pypi.internal.example.com/simple/"
"#;
        assert!(parse(content).unwrap().is_empty());
        assert!(
            PyprojectTomlParser
                .update_version(content, "internal-auth", "9.9.9")
                .is_err()
        );

        // 別名の追加ソースは既定 PyPI を置き換えないので更新できる
        let extra = r#"
[project]
dependencies = ["requests>=2.28.0"]

[[tool.pdm.source]]
name = "internal"
url = "https://pypi.internal.example.com/simple/"
"#;
        assert_eq!(parse(extra).unwrap().len(), 1);
    }

    /// PyPI 判定はホスト部で行い、類似ホストを PyPI と誤認しない。
    #[test]
    fn test_index_url_is_pypi_host_matching() {
        assert!(index_url_is_pypi("https://pypi.org/simple"));
        assert!(index_url_is_pypi("https://PyPI.org/simple/"));
        assert!(index_url_is_pypi("https://test.pypi.org/simple"));
        assert!(index_url_is_pypi("https://user:pass@pypi.org:443/simple"));
        assert!(!index_url_is_pypi("https://pypi.org.evil.example/simple"));
        assert!(!index_url_is_pypi("https://mypypi.org/simple"));
        assert!(!index_url_is_pypi(
            "https://pypi.internal.example.com/simple/"
        ));
    }

    // --- 修正: PEP 440 local version 付き完全一致は書き換えない ---

    /// 回帰テスト: `torch==2.1.0+cu121` のような local version 付き完全一致は、
    /// ラベルを落とすと PyPI に存在しない別ビルドを指す制約になるため書き換えない。
    /// parse は依存として surface するので、judge が
    /// 「constraint cannot be updated safely」の skip として可視化できる。
    #[test]
    fn test_local_version_pin_is_reported_but_not_written() {
        let content = r#"
[project]
dependencies = ["torch==2.1.0+cu121"]
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "torch");
        assert_eq!(deps[0].version_spec.version, "2.1.0+cu121");
        assert!(
            deps[0].version_spec.try_format_updated("2.34.2").is_none(),
            "local label 付きの制約は書き換え不能であるべき"
        );

        // writer も書き換えない (report/apply の整合)
        assert!(
            PyprojectTomlParser
                .update_version(content, "torch", "2.34.2")
                .is_err()
        );

        // local label が無ければ従来どおり更新できる
        let plain = r#"
[project]
dependencies = ["torch==2.1.0"]
"#;
        let result = PyprojectTomlParser
            .update_version(plain, "torch", "2.34.2")
            .unwrap();
        assert!(result.contains(r#""torch==2.34.2""#), "{result}");
    }

    /// 論理パスによる走査でも CRLF の行末を保持すること。
    #[test]
    fn test_update_version_preserves_crlf() {
        let content = "[project]\r\ndependencies = [\r\n    \"requests>=2.28.0\",\r\n]\r\n\r\n[tool.poetry.dependencies]\r\nrequests = \"^2.28.0\"\r\n";
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(result.contains("\"requests>=2.31.0\",\r\n"), "{result:?}");
        assert!(result.contains("requests = \"^2.31.0\"\r\n"), "{result:?}");
        assert!(!result.contains("\n\n"), "LF へ潰れている: {result:?}");
    }

    /// PEP 735 の include-group (`{ include-group = "..." }`) が混ざった依存配列でも、
    /// 配列の内外を取り違えずに更新できること (inline table の括弧も深さで数える)。
    #[test]
    fn test_update_dependency_group_with_include_group() {
        let content = r#"
[dependency-groups]
test = ["pytest>=7.0.0"]
dev = [
    { include-group = "test" },
    "ruff>=0.5.0",
]

[project]
keywords = ["ruff>=9.9.9"]
"#;
        let result = PyprojectTomlParser
            .update_version(content, "ruff", "0.9.2")
            .unwrap();
        assert!(result.contains(r#""ruff>=0.9.2""#), "{result}");
        // 配列を抜けた後のメタデータ配列は書き換えない
        assert!(result.contains(r#"keywords = ["ruff>=9.9.9"]"#), "{result}");
    }

    /// Poetry 依存セクション内のコメント行・空行を挟んでも更新が続くこと。
    #[test]
    fn test_update_poetry_section_with_comment_lines() {
        let content = r#"
[tool.poetry.dependencies]
# HTTP クライアント
python = "^3.11"

requests = "^2.28.0"  # 本番で使用
"#;
        let result = PyprojectTomlParser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(
            result.contains(r#"requests = "^2.31.0"  # 本番で使用"#),
            "{result}"
        );
    }

    /// Poetry の bare 完全一致ピンでも local label は保持され、書き換えられない。
    #[test]
    fn test_poetry_local_version_pin_not_written() {
        let content = r#"
[tool.poetry.dependencies]
torch = "2.1.0+cu121"
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "2.1.0+cu121");
        assert!(
            PyprojectTomlParser
                .update_version(content, "torch", "2.34.2")
                .is_err()
        );
    }
}
