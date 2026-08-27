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
use super::line_utils::{parse_toml_section_header, split_line_ending};
use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::parser::{MiseVersionParser, VersionParser, format_mise_version};
use regex::Regex;
use std::sync::LazyLock;

/// ツール定義を置く TOML セクション名
const TOOLS_SECTION: &str = "tools";

/// inline table / テーブル形式で使うバージョンキー
const VERSION_KEY: &str = "version";

/// inline table 内の `version = "..."` を捉える (行内の最初の 1 件だけ置換する)
static INLINE_VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\bversion\s*=\s*)(["'])([^"']*)(["'])"#).expect("valid inline version regex")
});

/// mise 設定ファイルのパーサ
pub struct MiseTomlParser;

/// TOML の値からバージョン文字列を取り出す。
///
/// - 文字列 → そのまま
/// - inline table / テーブル → `version` キーの文字列
/// - 配列やその他 → `None` (更新対象外)
fn extract_version_value(value: &toml::Value) -> Option<&str> {
    match value {
        toml::Value::String(s) => Some(s.as_str()),
        toml::Value::Table(table) => table.get(VERSION_KEY).and_then(|v| v.as_str()),
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
/// 値が文字列リテラルでない (配列・inline table・数値) 場合は `None`。
fn replace_string_value(value_part: &str, new_version: &str) -> Option<String> {
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
                // 配列指定 (複数バージョン) やパス指定のテーブルは更新対象外
                continue;
            };
            let Some(spec) = parser.parse(version_str) else {
                // latest / lts / ref: / path: / sub-N: などバージョンでない指定
                continue;
            };
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
        let mut result = String::with_capacity(content.len());
        // セクションキーはクォートを剥がしたセグメント列で保持する
        // (`[tools."npm:prettier"]` を `["tools", "npm:prettier"]` として比較する)
        let mut current_section: Option<Vec<String>> = None;
        let mut updated = false;

        for raw_line in content.split_inclusive('\n') {
            let (line, line_ending) = split_line_ending(raw_line);

            if let Some(section) = parse_toml_section_header(line) {
                current_section = Some(split_section_key(section));
                result.push_str(raw_line);
                continue;
            }

            let segments = current_section.as_deref().unwrap_or(&[]);
            let in_tools_section = segments.len() == 1 && segments[0] == TOOLS_SECTION;
            let in_tool_table =
                segments.len() == 2 && segments[0] == TOOLS_SECTION && segments[1] == package;

            if !updated && (in_tools_section || in_tool_table) {
                // `[tools]` 直下は `<tool> = <value>`、`[tools.<tool>]` 配下は
                // `version = <value>` が書き換え対象
                let key = if in_tools_section {
                    package
                } else {
                    VERSION_KEY
                };
                if let Some(value_start) = tool_key_value_start(line, key) {
                    let (head, value_part) = line.split_at(value_start);
                    if let Some(replaced) = replace_string_value(value_part, new_version) {
                        result.push_str(head);
                        result.push_str(&replaced);
                        result.push_str(line_ending);
                        updated = true;
                        continue;
                    }
                    // inline table (`java = { version = "...", ... }`) は
                    // version フィールドだけを差し替えて他のオプションを保持する
                    if in_tools_section
                        && value_part.trim_start().starts_with('{')
                        && let Some(replaced) =
                            replace_inline_table_version(value_part, new_version)
                    {
                        result.push_str(head);
                        result.push_str(&replaced);
                        result.push_str(line_ending);
                        updated = true;
                        continue;
                    }
                }
            }

            result.push_str(raw_line);
        }

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: std::path::PathBuf::from(Language::Mise.manifest_filename()),
                spec: new_version.to_string(),
                message: format!("tool '{package}' not found in [tools] section"),
            });
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

/// inline table の `version = "..."` だけを置換する (他のオプションは保持)。
///
/// 正規表現の一致位置が別フィールドの文字列リテラルの内側にある場合は飛ばす。
/// 例えば `java = { postinstall = 'echo version = "x"', version = "temurin-21" }`
/// では `postinstall` の中身が先に一致するため、素朴に最初の一致を置換すると
/// ツールのバージョンではなくコマンド文字列を書き換えてしまう。
fn replace_inline_table_version(value_part: &str, new_version: &str) -> Option<String> {
    let literals = string_literal_ranges(value_part);
    let is_inside_literal = |pos: usize| literals.iter().any(|(s, e)| pos > *s && pos < *e);

    let caps = INLINE_VERSION_RE
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
            "[tools.node]\nversion = \"26.7.0\"\n",
            "[tools]\n\"npm:prettier\" = \"3.9.0\"\n",
            "[tools]\n'npm:prettier' = \"3.9.0\"\n",
            "[tools.\"npm:prettier\"]\nversion = \"3.9.0\"\n",
            "[tools]\njava = \"temurin-21.0.5\"\n",
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

    #[test]
    fn test_language() {
        assert_eq!(MiseTomlParser.language(), Language::Mise);
    }
}
