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

/// 行がツールキーの定義行かどうかを判定し、値部分の範囲を返す。
///
/// 生キー (`node = ...`) と TOML のクォート付きキー (`"npm:prettier" = ...`、
/// `'npm:prettier' = ...`) の両方を受け付ける。ドットを含む名前や `:` を含む
/// バックエンド指定はクォートが必須なので、両方を見ないと取りこぼす。
fn tool_key_value_start(line: &str, tool: &str) -> Option<usize> {
    let trimmed_start = line.len() - line.trim_start().len();
    let rest = &line[trimmed_start..];

    let key_len = if let Some(after) = rest.strip_prefix(tool) {
        // 生キー: 直後は空白か `=` のみ (`node_extra` のような別キーに前方一致しない)
        if after.starts_with(|c: char| c.is_whitespace() || c == '=') {
            tool.len()
        } else {
            return None;
        }
    } else {
        let quoted_double = format!("\"{tool}\"");
        let quoted_single = format!("'{tool}'");
        if rest.starts_with(&quoted_double) {
            quoted_double.len()
        } else if rest.starts_with(&quoted_single) {
            quoted_single.len()
        } else {
            return None;
        }
    };

    let after_key = &rest[key_len..];
    let eq_offset = after_key.find('=')?;
    // キーと `=` の間に空白以外があれば別の構文
    if !after_key[..eq_offset].chars().all(char::is_whitespace) {
        return None;
    }
    Some(trimmed_start + key_len + eq_offset + 1)
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
        let table_section = format!("{TOOLS_SECTION}.{package}");
        let mut result = String::with_capacity(content.len());
        let mut current_section: Option<String> = None;
        let mut updated = false;

        for raw_line in content.split_inclusive('\n') {
            let (line, line_ending) = split_line_ending(raw_line);

            if let Some(section) = parse_toml_section_header(line) {
                current_section = Some(section.to_string());
                result.push_str(raw_line);
                continue;
            }

            let in_tools_section = current_section.as_deref() == Some(TOOLS_SECTION);
            let in_tool_table = current_section.as_deref() == Some(table_section.as_str());

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

/// inline table の `version = "..."` だけを置換する (他のオプションは保持)
fn replace_inline_table_version(value_part: &str, new_version: &str) -> Option<String> {
    if !INLINE_VERSION_RE.is_match(value_part) {
        return None;
    }
    Some(
        INLINE_VERSION_RE
            .replace(value_part, |caps: &regex::Captures| {
                let formatted = format_mise_version(&caps[3], new_version);
                format!("{}{}{}{}", &caps[1], &caps[2], formatted, &caps[4])
            })
            .into_owned(),
    )
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
