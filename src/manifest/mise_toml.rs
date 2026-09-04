//! mise の設定ファイル (`mise.toml` / `.mise.toml` / `.config/mise/config.toml`) パーサ
//!
//! `[tools]` セクションだけを解析・更新する。`[settings]` / `[env]` / `[tasks]` /
//! `[alias]` に同名のキーがあっても書き換えない (parse が読む範囲と update が
//! 書き換える範囲を一致させ、誤書き換えを防ぐ)。
//!
//! 対応する記法:
//!
//! ```toml
//! [tools]
//! node = "26.7.0"                                   # 文字列
//! java = { version = "temurin-21", postinstall = "" } # inline table
//! "npm:prettier" = "3.9.6"                          # クォート付きキー
//! python = ["3.12", "3.13"]                         # 配列 (更新対象外)
//!
//! [tools.terraform]                                 # テーブル形式
//! version = "1.15.0"
//! ```
//!
//! 配列指定は「1 依存 = 1 バージョン = 1 書き換え」モデルに乗らず、どの要素を
//! 更新すべきか決められないため意図的に更新対象から外す (Poetry のマルチ制約
//! 配列と同じ安全側のスキップ)。

use super::ManifestParser;
use super::line_utils::{
    HashCommentMode, parse_toml_section_header, split_line_ending, strip_hash_line_comment,
};
use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::parser::{MiseVersionParser, VersionParser, format_mise_version};
use regex::Regex;
use std::sync::LazyLock;

/// ツール定義を置く TOML セクション名
const TOOLS_SECTION: &str = "tools";

/// inline table / テーブル形式で使うバージョンキー
const VERSION_KEY: &str = "version";

/// inline table / テーブル形式の前方一致キー。
///
/// mise 公式スキーマの `tool_options` は `version` / `path` / `prefix` / `ref` の
/// oneOf。`prefix` は文字列形式の `prefix:26` と同義なので同じ Prefix 指定として
/// 扱い、`path` / `ref` は文字列形式の `path:` / `ref:` と同じく更新対象外にする
/// (同義の 2 記法で挙動が割れないようにする)。
const PREFIX_KEY: &str = "prefix";

/// inline table 内の `version = "..."` を捉える (行内の最初の 1 件だけ置換する)
static INLINE_VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\bversion\s*=\s*)(["'])([^"']*)(["'])"#).expect("valid inline version regex")
});

/// inline table 内の `prefix = "..."` を捉える (`version` が無いときの代替)
static INLINE_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\bprefix\s*=\s*)(["'])([^"']*)(["'])"#).expect("valid inline prefix regex")
});

/// mise 設定ファイルのパーサ
pub struct MiseTomlParser;

/// TOML の値からバージョン文字列を取り出す。
///
/// - 文字列 → そのまま
/// - inline table / テーブル → `version` キー、無ければ `prefix` キー
/// - 配列やその他 → `None` (更新対象外)
///
/// `prefix` は文字列形式の `prefix:` セレクタへ寄せて返す。こうしておくと
/// `{ prefix = "26" }` と `"prefix:26"` が同じ `VersionSpecKind::Prefix` になり、
/// セグメント数の保持 (`26` → `27`) も 1 箇所の実装で済む。
/// `path` / `ref` しか持たないテーブルは `None` (文字列形式と同じくスキップ)。
fn extract_version_value(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(table) => {
            if let Some(version) = table.get(VERSION_KEY).and_then(|v| v.as_str()) {
                return Some(version.to_string());
            }
            let prefix = table.get(PREFIX_KEY).and_then(|v| v.as_str())?;
            Some(format!("prefix:{prefix}"))
        }
        _ => None,
    }
}

/// 行頭が `key` / `"key"` / `'key'` であればその長さを返す。
///
/// 直後は空白 / `=` / `.` のいずれかでなければならない
/// (`node` が `node_extra` のような別キーへ前方一致しないようにする。
/// `.` を許すのは dotted key (`node.version = "..."`) を続けて読むため)。
fn match_key_len(text: &str, key: &str) -> Option<usize> {
    let candidates = [key.to_string(), format!("\"{key}\""), format!("'{key}'")];
    candidates.iter().find_map(|candidate| {
        let after = text.strip_prefix(candidate.as_str())?;
        after
            .starts_with(|c: char| c.is_whitespace() || c == '=' || c == '.')
            .then_some(candidate.len())
    })
}

/// 行がツールキーの定義行かどうかを判定し、値部分の開始位置を返す。
///
/// 受け付ける形:
/// - `node = "26.7.0"` (生キー)
/// - `"npm:prettier" = "3.9.6"` (クォート付きキー。`:` や `.` を含む名前は必須)
/// - `node.version = "26.7.0"` (dotted key)
///
/// dotted key は toml クレートが inline table と同じ構造へ畳むため、parse 側は
/// 依存として surface する。書き換え側が対応していないと「更新あり」と報告した
/// 後に書き込みが失敗して report/apply が矛盾する (Cargo.toml と同じ経路)。
fn tool_key_value_start(line: &str, tool: &str) -> Option<usize> {
    let mut pos = line.len() - line.trim_start().len();
    pos += match_key_len(&line[pos..], tool)?;

    // dotted key (`node.version = "..."`) なら `.version` まで読み進める
    let after_key = &line[pos..];
    let ws = after_key.len() - after_key.trim_start().len();
    if line[pos + ws..].starts_with('.') {
        pos += ws + 1;
        let after_dot = &line[pos..];
        pos += after_dot.len() - after_dot.trim_start().len();
        pos += match_key_len(&line[pos..], VERSION_KEY)?;
    }

    let rest = &line[pos..];
    let eq_offset = rest.find('=')?;
    // キーと `=` の間に空白以外があれば別の構文
    if !rest[..eq_offset].chars().all(char::is_whitespace) {
        return None;
    }
    Some(pos + eq_offset + 1)
}

/// トップレベルの dotted key (`tools.node = "26.7.0"`) を照合し、値部分の開始位置を返す。
///
/// `tools.node = "..."` は `[tools]` + `node = "..."` と等価な有効 TOML で、parse
/// (toml クレート) は依存として surface する。セクションヘッダ行しか見ていないと
/// 「更新あり」と報告した後に必ず書き込みが失敗する (Cargo.toml で既に塞いだのと
/// 同じ経路)。`tools."npm:prettier" = "..."` / `tools.node.version = "..."` も対象。
fn tools_dotted_key_value_start(line: &str, tool: &str) -> Option<usize> {
    let mut pos = line.len() - line.trim_start().len();
    pos += match_key_len(&line[pos..], TOOLS_SECTION)?;

    // `tools` と `.` の間の空白を読み飛ばす (`tools . node` も有効な TOML)
    let after_key = &line[pos..];
    pos += after_key.len() - after_key.trim_start().len();
    if !line[pos..].starts_with('.') {
        return None;
    }
    pos += 1;
    let after_dot = &line[pos..];
    pos += after_dot.len() - after_dot.trim_start().len();

    Some(pos + tool_key_value_start(&line[pos..], tool)?)
}

/// 行がマルチライン文字列 (`"""` / `'''`) を開いて同一行で閉じない場合、その区切りを返す。
///
/// mise のトップレベルには `tasks` / `task_templates` / `hooks` / `bootstrap` /
/// `env` があり、いずれもシェルスクリプトをマルチライン文字列で書くのが普通。
/// その中に `[tools]` や `node = "0.0.1"` がそのまま現れても、TOML 仕様上は
/// 文字列の中身なので構文として解釈してはいけない。行走査でこれを追跡しないと
/// タスクスクリプト側を書き換えてしまい、本物の宣言は `updated` ガードで
/// スキップされる (parse 側の toml クレートは当然無視するので report/apply が
/// 食い違う)。
///
/// pyproject_toml.rs が同じ問題を独自に塞いでいる。将来 line_utils へ共通化する
/// 余地があるが、今は各パーサでローカルに閉じている。
fn opens_unclosed_multiline_string(line: &str) -> Option<&'static str> {
    // 行コメント (`# ... """ ...`) 内の区切りを開始と誤検出しない。
    // TOML なのでバックスラッシュはリテラル扱い (Plain)。
    let body = strip_hash_line_comment(line, HashCommentMode::Plain);

    // 行内で最初に現れる区切り (""" / ''') を選ぶ
    let mut earliest: Option<(usize, &'static str)> = None;
    for delim in ["\"\"\"", "'''"] {
        if let Some(pos) = body.find(delim)
            && earliest.map(|(p, _)| pos < p).unwrap_or(true)
        {
            earliest = Some((pos, delim));
        }
    }
    let (pos, delim) = earliest?;
    // 開始区切りの直後に同じ区切りが再度現れれば、同一行で閉じている
    if body[pos + delim.len()..].contains(delim) {
        None
    } else {
        Some(delim)
    }
}

/// 値部分がマルチライン文字列リテラル (`"""..."""` / `'''...'''`) かどうか。
///
/// `node = """26.7.0"""` は TOML として valid だが、区切りが 3 文字あるため
/// 単一行文字列と同じ置換ロジックでは終端を誤認して壊れた TOML を書き戻す。
/// 更新対象から外し (parse 側でも同じ判定で除外する)、report/apply を一致させる。
fn is_multiline_string_value(value_part: &str) -> bool {
    let body = value_part.trim_start();
    body.starts_with("\"\"\"") || body.starts_with("'''")
}

/// `[tools]` に属するバージョン値の位置
struct ToolValue<'a> {
    /// `content.split_inclusive('\n')` での行番号
    line_index: usize,
    /// 行内での値部分 (`=` の右側) の開始バイト位置
    value_start: usize,
    /// 値部分 (`=` の右側。行末コメント込み)
    value_part: &'a str,
    /// inline table (`{ version = "..." }`) を許す位置かどうか
    allow_inline_table: bool,
}

/// ツールのバージョン値が書かれている位置を探す。
///
/// parse (報告) と update (書き込み) で同じ locator を共有することで、
/// 「報告したのに書き込めない」形が原理的に生まれないようにする。
/// 見つからない書き方 (トップレベルの inline table `tools = { node = "20" }` や
/// エスケープ付きキー) は parse 側でも依存として surface しない。
fn locate_tool_value<'a>(content: &'a str, tool: &str) -> Option<ToolValue<'a>> {
    let mut current_section: Option<Vec<String>> = None;
    let mut multiline: Option<&'static str> = None;
    // `[tools.<tool>]` 配下に `version` が無い場合の代替 (`prefix = "26"`)
    let mut prefix_fallback: Option<ToolValue<'a>> = None;

    for (line_index, raw_line) in content.split_inclusive('\n').enumerate() {
        let (line, _) = split_line_ending(raw_line);

        // マルチライン文字列の内側は TOML 構文として解釈しない
        if let Some(delim) = multiline {
            if line.contains(delim) {
                multiline = None;
            }
            continue;
        }
        if let Some(delim) = opens_unclosed_multiline_string(line) {
            multiline = Some(delim);
            continue;
        }

        if let Some(section) = parse_toml_section_header(line) {
            current_section = Some(split_section_key(section));
            continue;
        }

        let locate = |value_start: usize, allow_inline_table: bool| ToolValue {
            line_index,
            value_start,
            value_part: &line[value_start..],
            allow_inline_table,
        };

        match current_section.as_deref() {
            // `[tools]` 直下: `<tool> = <value>` / `<tool>.version = <value>`
            Some([section]) if section == TOOLS_SECTION => {
                if let Some(value_start) = tool_key_value_start(line, tool) {
                    return Some(locate(value_start, true));
                }
            }
            // `[tools.<tool>]` 配下: `version = <value>` (無ければ `prefix = <value>`)
            Some([section, name]) if section == TOOLS_SECTION && name == tool => {
                if let Some(value_start) = tool_key_value_start(line, VERSION_KEY) {
                    return Some(locate(value_start, false));
                }
                if prefix_fallback.is_none()
                    && let Some(value_start) = tool_key_value_start(line, PREFIX_KEY)
                {
                    prefix_fallback = Some(locate(value_start, false));
                }
            }
            // セクションヘッダより前のトップレベル dotted key: `tools.<tool> = <value>`
            None => {
                if let Some(value_start) = tools_dotted_key_value_start(line, tool) {
                    return Some(locate(value_start, true));
                }
            }
            _ => {}
        }
    }

    prefix_fallback
}

/// TOML のセクションキー (`tools."npm:prettier"`) をセグメントへ分解し、
/// クォートを剥がす。
///
/// `:` や `.` を含むツール名はテーブルヘッダでクォートが必須
/// (`[tools."npm:prettier"]`) なので、素朴な文字列比較では一致しない。
fn split_section_key(key: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in key.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '.' => {
                    segments.push(std::mem::take(&mut current).trim().to_string());
                }
                _ => current.push(ch),
            },
        }
    }
    segments.push(current.trim().to_string());
    segments
}

/// 値部分の文字列リテラルを新しいバージョンへ置き換える。
///
/// 引用符の種別 (`"` / `'`) と行末コメントを保持する。
/// 値が単一行の文字列リテラルでない (配列・inline table・数値・マルチライン
/// 文字列) 場合は `None`。
fn replace_string_value(value_part: &str, new_version: &str) -> Option<String> {
    // `node = """26.7.0"""` は 3 文字区切りなので、終端を 1 文字目のクォートで
    // 探すと `node = "26.8.1""26.7.0"""` のような壊れた TOML になる。触らない。
    if is_multiline_string_value(value_part) {
        return None;
    }

    let leading_len = value_part.len() - value_part.trim_start().len();
    let (leading, body) = value_part.split_at(leading_len);
    let quote = body.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let closing = body[1..].find(quote)? + 1;
    // 元の表記からベンダー接頭辞・`prefix:` セレクタ・セグメント数を復元する
    let formatted = format_mise_version(&body[1..closing], new_version);
    Some(format!(
        "{}{}{}{}{}",
        leading,
        quote,
        formatted,
        quote,
        &body[closing + 1..]
    ))
}

impl ManifestParser for MiseTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let parsed: toml::Value =
            toml::from_str(content).map_err(|e| ManifestError::TomlParseError {
                path: std::path::PathBuf::from(Language::Mise.manifest_filename()),
                message: e.to_string(),
            })?;

        let Some(tools) = parsed.get(TOOLS_SECTION).and_then(|v| v.as_table()) else {
            return Ok(Vec::new());
        };

        let parser = MiseVersionParser;
        let mut dependencies = Vec::new();
        for (name, value) in tools {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let Some(version_str) = extract_version_value(value) else {
                // 配列指定 (複数バージョン) や path / ref 指定のテーブルは更新対象外
                continue;
            };
            let Some(spec) = parser.parse(&version_str) else {
                // latest / lts / ref: / path: / sub-N: などバージョンでない指定
                continue;
            };
            // 書き換え位置を writer と同じ locator で確認してから報告する。
            // トップレベルの inline table (`tools = { node = "20" }`) のように
            // 値の位置を特定できない書き方は、報告しても書き込みが必ず失敗する
            // ため依存として surface しない (report/apply の整合)。
            let Some(location) = locate_tool_value(content, name) else {
                continue;
            };
            // マルチライン文字列で書かれた値は安全に書き戻せないので同じく除外する
            if is_multiline_string_value(location.value_part) {
                continue;
            }
            dependencies.push(Dependency::production(name, spec, Language::Mise));
        }

        // TOML のテーブルは順序を保持しないため、表示順を安定させる
        dependencies.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Mise
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let error = |message: String| ManifestError::InvalidVersionSpec {
            path: std::path::PathBuf::from(Language::Mise.manifest_filename()),
            spec: new_version.to_string(),
            message,
        };

        // 書き換え位置は parse と共有する locator で決める。`[tasks]` 等の
        // マルチライン文字列に紛れた `[tools]` / `node = "..."` を掴まない。
        let location = locate_tool_value(content, package)
            .ok_or_else(|| error(format!("tool '{package}' not found in [tools] section")))?;

        let mut result = String::with_capacity(content.len());
        let mut updated = false;

        for (line_index, raw_line) in content.split_inclusive('\n').enumerate() {
            if line_index == location.line_index {
                let (line, line_ending) = split_line_ending(raw_line);
                let (head, value_part) = line.split_at(location.value_start);
                // inline table (`java = { version = "...", ... }`) は version /
                // prefix フィールドだけを差し替えて他のオプションを保持する
                let replaced = replace_string_value(value_part, new_version).or_else(|| {
                    (location.allow_inline_table && value_part.trim_start().starts_with('{'))
                        .then(|| replace_inline_table_version(value_part, new_version))
                        .flatten()
                });
                if let Some(replaced) = replaced {
                    result.push_str(head);
                    result.push_str(&replaced);
                    result.push_str(line_ending);
                    updated = true;
                    continue;
                }
            }

            result.push_str(raw_line);
        }

        if !updated {
            // 配列やマルチライン文字列など、安全に書き換えられない値の形
            return Err(error(format!(
                "tool '{package}' version is not a single-line string in [tools] section"
            )));
        }

        Ok(result)
    }
}

/// 文字列リテラル (`"..."` / `'...'`) の範囲を列挙する。
///
/// 返す範囲は `(開始クォート位置, 終了クォート位置)`。閉じられていない場合は
/// 行末までを範囲とする。basic string (`"`) はバックスラッシュエスケープを
/// 解釈し、literal string (`'`) は解釈しない (TOML 仕様どおり)。
fn string_literal_ranges(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() {
            if quote == b'"' && bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                break;
            }
            i += 1;
        }
        if i < bytes.len() {
            ranges.push((start, i));
            i += 1;
        } else {
            ranges.push((start, text.len()));
            break;
        }
    }
    ranges
}

/// inline table のバージョンフィールドだけを置換する (他のオプションは保持)。
///
/// `version` を優先し、無ければ `prefix` を対象にする (mise の `tool_options` は
/// oneOf なので両方が同時に書かれることはない)。
fn replace_inline_table_version(value_part: &str, new_version: &str) -> Option<String> {
    [&*INLINE_VERSION_RE, &*INLINE_PREFIX_RE]
        .into_iter()
        .find_map(|re| replace_inline_table_field(value_part, re, new_version))
}

/// inline table の指定フィールド (`version` / `prefix`) を置換する。
///
/// 正規表現の一致位置が別フィールドの文字列リテラルの内側にある場合は飛ばす。
/// 例えば `java = { postinstall = 'echo version = "x"', version = "temurin-21" }`
/// では `postinstall` の中身が先に一致するため、素朴に最初の一致を置換すると
/// ツールのバージョンではなくコマンド文字列を書き換えてしまう。
fn replace_inline_table_field(
    value_part: &str,
    field_re: &Regex,
    new_version: &str,
) -> Option<String> {
    let literals = string_literal_ranges(value_part);
    let is_inside_literal = |pos: usize| literals.iter().any(|(s, e)| pos > *s && pos < *e);

    let caps = field_re
        .captures_iter(value_part)
        .find(|caps| !is_inside_literal(caps.get(0).expect("group 0 always exists").start()))?;

    let whole = caps.get(0).expect("group 0 always exists");
    let formatted = format_mise_version(&caps[3], new_version);
    Some(format!(
        "{}{}{}{}{}{}",
        &value_part[..whole.start()],
        &caps[1],
        &caps[2],
        formatted,
        &caps[4],
        &value_part[whole.end()..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Vec<Dependency> {
        MiseTomlParser.parse(content).unwrap()
    }

    #[test]
    fn test_parse_simple_tools() {
        let content = r#"
[tools]
node = "26.7.0"
pnpm = "11.23.0"
"#;
        let deps = parse(content);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "node");
        assert_eq!(deps[0].version(), "26.7.0");
        assert_eq!(deps[0].language, Language::Mise);
        assert_eq!(deps[1].name, "pnpm");
    }

    #[test]
    fn test_parse_inline_table() {
        let content = r#"
[tools]
java = { version = "temurin-21.0.5", postinstall = "echo hi" }
"#;
        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version(), "21.0.5");
        assert_eq!(deps[0].version_spec.prefix.as_deref(), Some("temurin-"));
    }

    #[test]
    fn test_parse_table_form() {
        let content = r#"
[tools.terraform]
version = "1.15.0"
"#;
        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "terraform");
        assert_eq!(deps[0].version(), "1.15.0");
    }

    #[test]
    fn test_parse_quoted_backend_key() {
        let content = r#"
[tools]
"npm:prettier" = "3.9.6"
"cargo:ripgrep" = "14.1.1"
"#;
        let deps = parse(content);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "cargo:ripgrep");
        assert_eq!(deps[1].name, "npm:prettier");
    }

    #[test]
    fn test_parse_skips_arrays_and_floating() {
        let content = r#"
[tools]
python = ["3.12", "3.13"]
gh = "latest"
erlang = "ref:master"
shfmt = "path:./shfmt"
node = "sub-2:lts"
terraform = "1.15.0"
"#;
        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "terraform");
    }

    #[test]
    fn test_parse_partial_version_is_prefix() {
        let content = r#"
[tools]
node = "26"
python = "3.13"
"#;
        let deps = parse(content);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Prefix);
        assert_eq!(deps[1].version_spec.kind, VersionSpecKind::Prefix);
    }

    #[test]
    fn test_parse_without_tools_section() {
        let content = r#"
[settings]
minimum_release_age = "7d"
"#;
        assert!(parse(content).is_empty());
    }

    #[test]
    fn test_update_simple() {
        let content = "[tools]\nnode = \"26.7.0\"\npnpm = \"11.23.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "[tools]\nnode = \"26.8.1\"\npnpm = \"11.23.0\"\n");
    }

    /// 回帰: dotted key (`node.version = "..."`) は toml クレートが inline table と
    /// 同じ構造へ畳むため parse が依存として surface する。書き換え側が対応して
    /// いないと「更新あり」と報告した後に書き込みが失敗する。
    #[test]
    fn test_update_dotted_key() {
        let content = "[tools]\nnode.version = \"26.7.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "[tools]\nnode.version = \"26.8.1\"\n");
    }

    #[test]
    fn test_update_quoted_dotted_key() {
        let content = "[tools]\n\"npm:prettier\".version = \"3.9.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "npm:prettier", "3.9.6")
            .unwrap();
        assert_eq!(updated, "[tools]\n\"npm:prettier\".version = \"3.9.6\"\n");
    }

    /// 回帰: `:` を含むツール名はテーブルヘッダでクォートが必須なので、
    /// 素朴な文字列比較 (`tools.npm:prettier`) ではセクションが一致しない
    #[test]
    fn test_update_quoted_table_section() {
        let content = "[tools.\"npm:prettier\"]\nversion = \"3.9.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "npm:prettier", "3.9.6")
            .unwrap();
        assert_eq!(updated, "[tools.\"npm:prettier\"]\nversion = \"3.9.6\"\n");
    }

    /// 回帰: inline table の別フィールドに `version = "..."` を含む文字列が
    /// あっても、そちらを書き換えない (正規表現の最初の一致を素朴に使うと
    /// コマンド文字列の中身を破壊していた)
    #[test]
    fn test_update_inline_table_ignores_version_inside_other_string() {
        let content = "[tools]\njava = { postinstall = 'echo version = \"do-not-touch\"', version = \"temurin-21.0.5\" }\n";
        let updated = MiseTomlParser
            .update_version(content, "java", "21.0.9")
            .unwrap();
        assert_eq!(
            updated,
            "[tools]\njava = { postinstall = 'echo version = \"do-not-touch\"', version = \"temurin-21.0.9\" }\n"
        );
    }

    /// parse が依存として surface した記法は、必ず update でも書き換えられること
    /// (report と apply が食い違わないことの網羅チェック)
    #[test]
    fn test_every_parsed_form_is_updatable() {
        let manifests = [
            "[tools]\nnode = \"26.7.0\"\n",
            "[tools]\nnode = '26.7.0'\n",
            "[tools]\nnode = \"26\"\n",
            "[tools]\nnode = \"prefix:26\"\n",
            "[tools]\nnode.version = \"26.7.0\"\n",
            "[tools]\nnode = { version = \"26.7.0\", postinstall = \"echo hi\" }\n",
            "[tools]\nnode = { prefix = \"26\" }\n",
            "[tools.node]\nversion = \"26.7.0\"\n",
            "[tools.node]\nprefix = \"26\"\n",
            "[tools]\n\"npm:prettier\" = \"3.9.0\"\n",
            "[tools]\n'npm:prettier' = \"3.9.0\"\n",
            "[tools.\"npm:prettier\"]\nversion = \"3.9.0\"\n",
            "[tools]\njava = \"temurin-21.0.5\"\n",
            // トップレベルの dotted key (`[tools]` ヘッダを書かない等価な TOML)
            "tools.node = \"26.7.0\"\n",
            "tools.\"npm:prettier\" = \"3.9.0\"\n",
            "tools.node.version = \"26.7.0\"\n",
            // タスクスクリプト内の擬似 `[tools]` に釣られず本物を書き換えられること
            "[tasks.a]\nrun = \"\"\"\n[tools]\nnode = \"0.0.1\"\n\"\"\"\n\n[tools]\nnode = \"26.7.0\"\n",
        ];

        for content in manifests {
            let deps = MiseTomlParser.parse(content).unwrap();
            assert!(!deps.is_empty(), "no dependency parsed from: {content}");
            for dep in deps {
                let result = MiseTomlParser.update_version(content, &dep.name, "99.0.0");
                assert!(
                    result.is_ok(),
                    "parsed {} but could not update it in: {content}",
                    dep.name
                );
                let updated = result.unwrap();
                assert_ne!(updated, content, "update was a no-op for: {content}");
            }
        }
    }

    #[test]
    fn test_update_preserves_single_quotes_and_comment() {
        let content = "[tools]\nnode = '26.7.0'  # LTS\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "[tools]\nnode = '26.8.1'  # LTS\n");
    }

    #[test]
    fn test_update_preserves_crlf() {
        let content = "[tools]\r\nnode = \"26.7.0\"\r\npnpm = \"11.23.0\"\r\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(
            updated,
            "[tools]\r\nnode = \"26.8.1\"\r\npnpm = \"11.23.0\"\r\n"
        );
    }

    /// judge が渡すのはベンダー接頭辞を落とした数値部 (`21.0.9`)。
    /// 書き戻し側でマニフェスト上の接頭辞を復元する必要がある。
    #[test]
    fn test_update_restores_vendor_prefix_from_manifest() {
        let content = "[tools]\njava = \"temurin-21.0.5\"\n";
        let updated = MiseTomlParser
            .update_version(content, "java", "21.0.9")
            .unwrap();
        assert_eq!(updated, "[tools]\njava = \"temurin-21.0.9\"\n");
    }

    /// 前方一致指定は書き戻しでもセグメント数を保つ (`26` を `27.1.0` で潰さない)
    #[test]
    fn test_update_keeps_prefix_segment_count() {
        let content = "[tools]\nnode = \"26\"\npython = \"3.13\"\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "27.1.0")
            .unwrap();
        assert_eq!(updated, "[tools]\nnode = \"27\"\npython = \"3.13\"\n");

        let updated = MiseTomlParser
            .update_version(&updated, "python", "3.14.7")
            .unwrap();
        assert_eq!(updated, "[tools]\nnode = \"27\"\npython = \"3.14\"\n");
    }

    /// `prefix:` セレクタも書き戻しで保持する
    #[test]
    fn test_update_keeps_prefix_selector() {
        let content = "[tools]\ngo = \"prefix:1.19\"\n";
        let updated = MiseTomlParser
            .update_version(content, "go", "1.24.3")
            .unwrap();
        assert_eq!(updated, "[tools]\ngo = \"prefix:1.24\"\n");
    }

    #[test]
    fn test_update_inline_table_keeps_other_options() {
        let content =
            "[tools]\njava = { version = \"temurin-21.0.5\", postinstall = \"echo hi\" }\n";
        let updated = MiseTomlParser
            .update_version(content, "java", "temurin-21.0.9")
            .unwrap();
        assert_eq!(
            updated,
            "[tools]\njava = { version = \"temurin-21.0.9\", postinstall = \"echo hi\" }\n"
        );
    }

    #[test]
    fn test_update_table_form() {
        let content = "[tools.terraform]\nversion = \"1.15.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "terraform", "1.16.0")
            .unwrap();
        assert_eq!(updated, "[tools.terraform]\nversion = \"1.16.0\"\n");
    }

    #[test]
    fn test_update_quoted_key() {
        let content = "[tools]\n\"npm:prettier\" = \"3.9.6\"\n";
        let updated = MiseTomlParser
            .update_version(content, "npm:prettier", "3.9.7")
            .unwrap();
        assert_eq!(updated, "[tools]\n\"npm:prettier\" = \"3.9.7\"\n");
    }

    /// `[settings]` や `[env]` に同名キーがあっても書き換えない
    #[test]
    fn test_update_does_not_touch_other_sections() {
        let content = "[env]\nnode = \"do-not-touch\"\n\n[tools]\nnode = \"26.7.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(
            updated,
            "[env]\nnode = \"do-not-touch\"\n\n[tools]\nnode = \"26.8.1\"\n"
        );
    }

    /// キーの前方一致で別ツールを壊さない
    #[test]
    fn test_update_does_not_prefix_match_other_tools() {
        let content = "[tools]\nnode-build = \"5.0.0\"\nnode = \"26.7.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(
            updated,
            "[tools]\nnode-build = \"5.0.0\"\nnode = \"26.8.1\"\n"
        );
    }

    #[test]
    fn test_update_missing_tool_is_error() {
        let content = "[tools]\nnode = \"26.7.0\"\n";
        assert!(
            MiseTomlParser
                .update_version(content, "python", "3.13.0")
                .is_err()
        );
    }

    /// 配列指定は parse でスキップされるが、writer から呼ばれてもファイルを壊さない
    #[test]
    fn test_update_array_value_is_error() {
        let content = "[tools]\npython = [\"3.12\", \"3.13\"]\n";
        assert!(
            MiseTomlParser
                .update_version(content, "python", "3.14.0")
                .is_err()
        );
    }

    /// 回帰: `[tasks]` のマルチライン文字列に書かれたスクリプト内の `[tools]` /
    /// `node = "..."` を掴んでしまい、タスクを壊した上で本物の宣言 (更新対象) を
    /// `updated` ガードで取りこぼしていた
    #[test]
    fn test_update_ignores_tools_inside_task_multiline_string() {
        let content = concat!(
            "[tasks.bootstrap]\n",
            "run = \"\"\"\n",
            "cat > .mise.toml <<EOF\n",
            "[tools]\n",
            "node = \"0.0.1\"\n",
            "EOF\n",
            "\"\"\"\n",
            "\n",
            "[tools]\n",
            "node = \"26.7.0\"\n",
        );

        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version(), "26.7.0");

        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        // タスクスクリプトは無傷で、本物の宣言だけが更新される
        assert_eq!(
            updated,
            content.replace("node = \"26.7.0\"", "node = \"26.8.1\"")
        );
        assert!(updated.contains("node = \"0.0.1\""));
    }

    /// リテラル文字列 (`'''`) のマルチライン形式も同じく素通しする
    #[test]
    fn test_update_ignores_tools_inside_literal_multiline_string() {
        let content = concat!(
            "[tasks.bootstrap]\n",
            "run = '''\n",
            "[tools]\n",
            "node = \"0.0.1\"\n",
            "'''\n",
            "\n",
            "[tools]\n",
            "node = \"26.7.0\"\n",
        );

        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(
            updated,
            content.replace("node = \"26.7.0\"", "node = \"26.8.1\"")
        );
        assert!(updated.contains("node = \"0.0.1\""));
    }

    /// コメント内の `"""` はマルチライン文字列の開始ではない
    /// (誤検出すると以降の `[tools]` 全体を素通ししてしまう)
    #[test]
    fn test_update_ignores_multiline_delimiter_inside_comment() {
        let content = "# 例: run = \"\"\"\n[tools]\nnode = \"26.7.0\"\n";

        let deps = parse(content);
        assert_eq!(deps.len(), 1);

        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "# 例: run = \"\"\"\n[tools]\nnode = \"26.8.1\"\n");
    }

    /// 回帰: `tools.node = "..."` は `[tools]` + `node = "..."` と等価な有効 TOML。
    /// parse が surface する以上、書き換えられなければ report/apply が矛盾する
    #[test]
    fn test_update_top_level_dotted_key() {
        let content = "tools.node = \"20.0.0\"\n";

        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "node");

        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "tools.node = \"26.8.1\"\n");
    }

    #[test]
    fn test_update_top_level_dotted_quoted_key() {
        let content = "tools.\"npm:prettier\" = \"3.9.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "npm:prettier", "3.9.6")
            .unwrap();
        assert_eq!(updated, "tools.\"npm:prettier\" = \"3.9.6\"\n");
    }

    #[test]
    fn test_update_top_level_dotted_version_key() {
        let content = "tools.node.version = \"20.0.0\"\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "tools.node.version = \"26.8.1\"\n");
    }

    /// セクション配下の `tools.<name>` は別物 (`[settings] tools.node`) なので触らない
    #[test]
    fn test_update_dotted_key_outside_top_level_is_not_matched() {
        let content = "[settings]\ntools.node = \"do-not-touch\"\n";
        assert!(
            MiseTomlParser
                .update_version(content, "node", "26.8.1")
                .is_err()
        );
    }

    /// トップレベルの inline table (`tools = { node = "20" }`) は値の位置を
    /// 特定できないので依存として報告しない (報告すると書き込みが必ず失敗する)
    #[test]
    fn test_parse_skips_top_level_inline_tools_table() {
        let content = "tools = { node = \"20.0.0\" }\n";
        assert!(parse(content).is_empty());
        assert!(
            MiseTomlParser
                .update_version(content, "node", "26.8.1")
                .is_err()
        );
    }

    /// 回帰: `node = """26.7.0"""` は TOML として valid だが、単一行文字列と同じ
    /// 置換をすると `node = "26.8.1""26.7.0"""` になり TOML が壊れる。
    /// 報告も書き込みもしない (安全側のスキップ)
    #[test]
    fn test_multiline_string_value_is_skipped() {
        let content = "[tools]\nnode = \"\"\"26.7.0\"\"\"\n";
        assert!(parse(content).is_empty());
        assert!(
            MiseTomlParser
                .update_version(content, "node", "26.8.1")
                .is_err()
        );

        // 複数行にまたがる形も同様
        let content = "[tools]\nnode = \"\"\"\n26.7.0\n\"\"\"\n";
        assert!(parse(content).is_empty());
        assert!(
            MiseTomlParser
                .update_version(content, "node", "26.8.1")
                .is_err()
        );
    }

    /// mise 公式スキーマの `tool_options` は version / path / prefix / ref の oneOf。
    /// `{ prefix = "26" }` は文字列形式の `"prefix:26"` と同義なので Prefix として
    /// 更新し、セグメント数を保つ
    #[test]
    fn test_parse_and_update_inline_table_prefix_option() {
        let content = "[tools]\nnode = { prefix = \"26\" }\n";

        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Prefix);
        assert_eq!(deps[0].version(), "26");

        let updated = MiseTomlParser
            .update_version(content, "node", "27.1.0")
            .unwrap();
        assert_eq!(updated, "[tools]\nnode = { prefix = \"27\" }\n");
    }

    /// テーブル形式の `prefix` オプションも同じ扱い
    #[test]
    fn test_parse_and_update_table_form_prefix_option() {
        let content = "[tools.node]\nprefix = \"26\"\n";

        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Prefix);

        let updated = MiseTomlParser
            .update_version(content, "node", "27.1.0")
            .unwrap();
        assert_eq!(updated, "[tools.node]\nprefix = \"27\"\n");
    }

    /// `path` / `ref` は文字列形式の `path:` / `ref:` と同じく更新対象外
    #[test]
    fn test_parse_skips_table_option_path_and_ref() {
        let content = concat!(
            "[tools]\n",
            "go = { ref = \"master\" }\n",
            "java = { path = \"/opt/jdk\" }\n",
            "node = \"26.7.0\"\n",
        );
        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "node");
    }

    /// `version` と `prefix` が同居する (スキーマ違反の) テーブルでは、
    /// parse と同じく `version` を優先して書き換える
    #[test]
    fn test_update_inline_table_prefers_version_over_prefix() {
        let content = "[tools]\nnode = { prefix = \"26\", version = \"26.7.0\" }\n";
        let updated = MiseTomlParser
            .update_version(content, "node", "26.8.1")
            .unwrap();
        assert_eq!(
            updated,
            "[tools]\nnode = { prefix = \"26\", version = \"26.8.1\" }\n"
        );
    }

    #[test]
    fn test_language() {
        assert_eq!(MiseTomlParser.language(), Language::Mise);
    }
}
