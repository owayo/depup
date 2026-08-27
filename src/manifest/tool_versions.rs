//! asdf 互換の `.tool-versions` パーサ (mise が読む旧形式)
//!
//! ```text
//! node 26.7.0        # 行末コメントを書ける
//! ruby 3             # 部分指定 (前方一致)
//! shellcheck latest  # 浮動指定は更新対象外
//! erlang ref:master  # VCS ref も更新対象外
//! nodejs 20.11.1 22.0.0  # 複数バージョンは更新対象外
//! ```
//!
//! 複数バージョン指定は「1 依存 = 1 バージョン = 1 書き換え」モデルに乗らず、
//! どれを更新すべきか決められないため意図的に更新対象から外す。

use super::ManifestParser;
use super::line_utils::split_line_ending;
use crate::domain::{Dependency, Language};
use crate::error::ManifestError;
use crate::parser::{MiseVersionParser, VersionParser, format_mise_version};

/// `.tool-versions` のファイル名
pub const TOOL_VERSIONS_FILENAME: &str = ".tool-versions";

/// `.tool-versions` のパーサ
pub struct ToolVersionsParser;

/// 行からコメントを除いた本体を返す。
///
/// `.tool-versions` は文字列リテラルを持たない単純な空白区切り形式なので、
/// 最初の `#` 以降を無条件にコメントとして落とす。
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// 内容が `.tool-versions` 形式かどうかを判定する。
///
/// mise は TOML 形式 (`mise.toml`) と asdf 互換の空白区切り形式
/// (`.tool-versions`) の両方を読む。`ManifestParser` には内容しか渡らないため、
/// Gradle が build.gradle と version catalog を内容で見分けているのと同じ方式で
/// 形式を判別する。
///
/// 誤判定でファイルを壊さないよう、`.tool-versions` と認めるのは
/// **全ての実行に意味がある行**が `<tool> <version>...` 形式のときだけにする。
/// TOML のセクションヘッダ (`[tools]`) や `=` を含む行が 1 行でもあれば、
/// 壊れた TOML であっても `.tool-versions` とは扱わない (TOML パーサ側で
/// エラーにした方が安全)。
pub(crate) fn looks_like_tool_versions(content: &str) -> bool {
    let mut has_entry = false;
    for line in content.lines() {
        let body = strip_comment(line).trim();
        if body.is_empty() {
            continue;
        }
        // TOML のセクションヘッダ / キー = 値 は .tool-versions には現れない
        if body.starts_with('[') || body.contains('=') {
            return false;
        }
        let mut tokens = body.split_whitespace();
        if tokens.next().is_none() {
            continue;
        }
        if tokens.next().is_none() {
            // バージョンのない行 (`node` だけ) は .tool-versions として不正
            return false;
        }
        has_entry = true;
    }
    has_entry
}

/// 行を `(ツール名, バージョン列)` に分解する。空行・コメント行は `None`。
fn split_tool_line(line: &str) -> Option<(&str, Vec<&str>)> {
    let body = strip_comment(line);
    let mut tokens = body.split_whitespace();
    let tool = tokens.next()?;
    let versions: Vec<&str> = tokens.collect();
    if versions.is_empty() {
        return None;
    }
    Some((tool, versions))
}

impl ManifestParser for ToolVersionsParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let parser = MiseVersionParser;
        let mut dependencies = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for line in content.lines() {
            let Some((tool, versions)) = split_tool_line(line) else {
                continue;
            };
            // 複数バージョン指定はどれを更新すべきか決められないので対象外
            if versions.len() != 1 {
                continue;
            }
            let Some(spec) = parser.parse(versions[0]) else {
                continue;
            };
            // 同じツールが複数行で宣言されている場合は書き換え位置を一意に
            // 決められないため、両方とも更新対象から外す
            if seen.iter().any(|name| name == tool) {
                dependencies.retain(|dep: &Dependency| dep.name != tool);
                continue;
            }
            seen.push(tool.to_string());
            dependencies.push(Dependency::production(tool, spec, Language::Mise));
        }

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
        let mut updated = false;

        for raw_line in content.split_inclusive('\n') {
            let (line, line_ending) = split_line_ending(raw_line);

            if !updated
                && let Some((tool, versions)) = split_tool_line(line)
                && tool == package
                && versions.len() == 1
            {
                // 空白の並びとコメントを保つため、バージョントークンの
                // バイト範囲だけを差し替える
                let version_token = versions[0];
                let body = strip_comment(line);
                let tool_end = body.find(tool).map(|i| i + tool.len()).unwrap_or(0);
                if let Some(relative) = body[tool_end..].find(version_token) {
                    let start = tool_end + relative;
                    let end = start + version_token.len();
                    // 元の表記からベンダー接頭辞・`prefix:` セレクタ・
                    // セグメント数を復元する
                    let formatted = format_mise_version(version_token, new_version);
                    result.push_str(&line[..start]);
                    result.push_str(&formatted);
                    result.push_str(&line[end..]);
                    result.push_str(line_ending);
                    updated = true;
                    continue;
                }
            }

            result.push_str(raw_line);
        }

        if !updated {
            return Err(ManifestError::InvalidVersionSpec {
                path: std::path::PathBuf::from(TOOL_VERSIONS_FILENAME),
                spec: new_version.to_string(),
                message: format!("tool '{package}' not found in .tool-versions"),
            });
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Vec<Dependency> {
        ToolVersionsParser.parse(content).unwrap()
    }

    #[test]
    fn test_parse_basic() {
        let content = "node 26.7.0\nruby 3.4.2\n";
        let deps = parse(content);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "node");
        assert_eq!(deps[0].version(), "26.7.0");
        assert_eq!(deps[1].name, "ruby");
    }

    #[test]
    fn test_parse_with_comments_and_blank_lines() {
        let content = "# comment line\n\nnode 26.7.0 # trailing comment\n";
        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version(), "26.7.0");
    }

    #[test]
    fn test_parse_partial_version() {
        let deps = parse("ruby 3\n");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Prefix);
    }

    #[test]
    fn test_parse_skips_floating_and_multi_version() {
        let content = concat!(
            "shellcheck latest\n",
            "erlang ref:master\n",
            "shfmt path:./shfmt\n",
            "node lts\n",
            "python sub-0.1:latest\n",
            "nodejs 20.11.1 22.0.0\n",
            "go 1.24.3\n",
        );
        let deps = parse(content);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "go");
    }

    /// 同じツールが複数行にあると書き換え位置が一意に決まらないので両方外す
    #[test]
    fn test_parse_duplicate_tool_is_dropped() {
        let deps = parse("node 26.7.0\nnode 24.1.0\n");
        assert!(deps.is_empty());
    }

    #[test]
    fn test_update_basic() {
        let updated = ToolVersionsParser
            .update_version("node 26.7.0\nruby 3.4.2\n", "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "node 26.8.1\nruby 3.4.2\n");
    }

    #[test]
    fn test_update_preserves_spacing_and_comment() {
        let updated = ToolVersionsParser
            .update_version("node    26.7.0   # pinned for CI\n", "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "node    26.8.1   # pinned for CI\n");
    }

    #[test]
    fn test_update_preserves_crlf() {
        let updated = ToolVersionsParser
            .update_version("node 26.7.0\r\nruby 3.4.2\r\n", "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "node 26.8.1\r\nruby 3.4.2\r\n");
    }

    /// バージョン文字列がツール名と同じ文字列を含んでも壊れない
    #[test]
    fn test_update_does_not_match_inside_tool_name() {
        let updated = ToolVersionsParser
            .update_version("java temurin-21.0.5\n", "java", "temurin-21.0.9")
            .unwrap();
        assert_eq!(updated, "java temurin-21.0.9\n");
    }

    /// judge が渡すのは数値部だけなので、書き戻しで接頭辞を復元する
    #[test]
    fn test_update_restores_vendor_prefix_from_manifest() {
        let updated = ToolVersionsParser
            .update_version("java temurin-21.0.5\n", "java", "21.0.9")
            .unwrap();
        assert_eq!(updated, "java temurin-21.0.9\n");
    }

    /// 前方一致指定はセグメント数を保つ
    #[test]
    fn test_update_keeps_prefix_segment_count() {
        let updated = ToolVersionsParser
            .update_version("ruby 3\n", "ruby", "3.4.2")
            .unwrap();
        assert_eq!(updated, "ruby 3\n");

        let updated = ToolVersionsParser
            .update_version("ruby 3.3\n", "ruby", "3.4.2")
            .unwrap();
        assert_eq!(updated, "ruby 3.4\n");
    }

    #[test]
    fn test_update_missing_tool_is_error() {
        assert!(
            ToolVersionsParser
                .update_version("node 26.7.0\n", "python", "3.13.0")
                .is_err()
        );
    }

    #[test]
    fn test_update_multi_version_is_error() {
        assert!(
            ToolVersionsParser
                .update_version("nodejs 20.11.1 22.0.0\n", "nodejs", "22.1.0")
                .is_err()
        );
    }

    #[test]
    fn test_language() {
        assert_eq!(ToolVersionsParser.language(), Language::Mise);
    }
}
