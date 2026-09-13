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
/// TOML のセクションヘッダ (`[tools]`) や `キー = 値` の行が 1 行でもあれば、
/// 壊れた TOML であっても `.tool-versions` とは扱わない (TOML パーサ側で
/// エラーにした方が安全)。
///
/// ただし `=` / `[` の判定は **キーとしての `=`** / **行頭の `[`** に限定する。
/// mise のバックエンドオプションはツール名の末尾にブラケット記法で付き
/// (`ubi:BurntSushi/ripgrep[exe=rg] 14.1.1`)、`=` と `[` を含むが TOML ではない。
/// 素朴に「`=` を含めば TOML」と判定すると、この 1 行のせいでファイル全体が
/// TOML パーサへ渡って `TomlParseError` になり、同じファイルにある他の全ツールまで
/// 失われる。TOML の `キー = 値` では `=` が先頭トークンの外側に現れるため、
/// 先頭トークン (= ツール名) の内側の `=` は `.tool-versions` として受け入れる。
pub(crate) fn looks_like_tool_versions(content: &str) -> bool {
    let mut has_entry = false;
    for line in content.lines() {
        let body = strip_comment(line).trim();
        if body.is_empty() {
            continue;
        }
        // TOML のセクションヘッダ (`[tools]` / `[tools."npm:prettier"]`) は行頭が `[`。
        // ツール名末尾のブラケットオプションは行頭には来ない
        if body.starts_with('[') {
            return false;
        }
        let mut tokens = body.split_whitespace();
        let Some(tool) = tokens.next() else {
            continue;
        };
        let rest: Vec<&str> = tokens.collect();
        if rest.is_empty() {
            // バージョンのない行 (`node` だけ / `node="26.7.0"` のような
            // 空白なしの TOML キー行) は .tool-versions として不正
            return false;
        }
        // 先頭トークンより後ろに `=` があれば TOML の `キー = 値`。
        // `ubi:x/y[exe=rg] 1.2.3` は先頭トークン内なので .tool-versions と認める
        if rest.iter().any(|token| token.contains('=')) {
            return false;
        }
        // 先頭トークン内の `=` はブラケットオプションの中だけ許す
        // (`node= 26.7.0` のような書き損じた TOML を取り込まない)
        let tool_base = tool.split_once('[').map_or(tool, |(base, _)| base);
        if tool_base.contains('=') {
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
                // parse が依存として採用した行だけを書き換える。
                // 浮動指定 (`python latest`) や VCS ref (`erlang ref:master`) の行は
                // parse が捨てているため、ここで弾かないと
                // ```
                // python latest
                // python 3.13.1
                // ```
                // で parse が 2 行目を読み writer が 1 行目を潰す。
                // asdf/mise は先頭行を採用するので、意図した浮動指定が無言でピンに変わる
                // (`format_mise_version` は parse 失敗時に new_version をそのまま返す)
                && MiseVersionParser.parse(versions[0]).is_some()
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

    /// タブ区切りやマルチバイトのコメントでもバイト範囲計算がずれない
    #[test]
    fn test_update_handles_tabs_and_multibyte_comments() {
        let updated = ToolVersionsParser
            .update_version("node\t26.7.0\t# 本番と揃える\n", "node", "26.8.1")
            .unwrap();
        assert_eq!(updated, "node\t26.8.1\t# 本番と揃える\n");
    }

    /// parse が依存として surface した行は、必ず update でも書き換えられること
    #[test]
    fn test_every_parsed_form_is_updatable() {
        let manifests = [
            "node 26.7.0\n",
            "node    26.7.0   # comment\n",
            "node\t26.7.0\n",
            "ruby 3\n",
            "go prefix:1.19\n",
            "java temurin-21.0.5\n",
            "# 先頭コメント\n\nnode 26.7.0\n",
            // 最終行に改行がない
            "node 26.7.0",
        ];

        for content in manifests {
            let deps = ToolVersionsParser.parse(content).unwrap();
            assert!(!deps.is_empty(), "no dependency parsed from: {content:?}");
            for dep in deps {
                let result = ToolVersionsParser.update_version(content, &dep.name, "99.0.0");
                assert!(
                    result.is_ok(),
                    "parsed {} but could not update it in: {content:?}",
                    dep.name
                );
                assert_ne!(
                    result.unwrap(),
                    content,
                    "update was a no-op for: {content:?}"
                );
            }
        }
    }

    /// バグ回帰テスト: ブラケットオプション付きツール名を含む `.tool-versions` を
    /// TOML と誤判定しない。
    ///
    /// mise の ubi バックエンドは `ubi:BurntSushi/ripgrep[exe=rg]` のように `=` を含む
    /// ツール名を書ける。以前は「意味のある行が 1 つでも `=` を含めば TOML」と判定して
    /// いたため、この 1 行のせいでファイル全体が TOML パーサへ渡って `TomlParseError` に
    /// なり、同じファイルの `node` を含む全ツールが失われていた。
    #[test]
    fn test_looks_like_tool_versions_with_backend_options() {
        let content = "ubi:BurntSushi/ripgrep[exe=rg] 14.1.1\nnode 26.7.0\n";
        assert!(looks_like_tool_versions(content));

        let deps = parse(content);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "ubi:BurntSushi/ripgrep[exe=rg]");
        assert_eq!(deps[0].version(), "14.1.1");
        assert_eq!(deps[1].name, "node");

        // 複数オプション / 正規表現オプションも同様
        assert!(looks_like_tool_versions(
            "ubi:cli/cli[exe=gh][provider=github] 2.60.1\n"
        ));
        assert!(looks_like_tool_versions(
            r"ubi:cargo-bins/cargo-binstall[tag_regex=^\d+\.] 1.10.0"
        ));
    }

    /// ブラケットオプション付きツール名も書き換えられること (parse/update の整合)
    #[test]
    fn test_update_tool_with_backend_options() {
        let updated = ToolVersionsParser
            .update_version(
                "ubi:BurntSushi/ripgrep[exe=rg] 14.1.1\nnode 26.7.0\n",
                "ubi:BurntSushi/ripgrep[exe=rg]",
                "14.2.0",
            )
            .unwrap();
        assert_eq!(
            updated,
            "ubi:BurntSushi/ripgrep[exe=rg] 14.2.0\nnode 26.7.0\n"
        );
    }

    /// 「壊れた TOML を `.tool-versions` と誤認して黙って読み飛ばさない」既存の意図は維持する
    #[test]
    fn test_looks_like_tool_versions_rejects_toml_forms() {
        // セクションヘッダ
        assert!(!looks_like_tool_versions("[tools]\nnode = \"26.7.0\"\n"));
        assert!(!looks_like_tool_versions(
            "[tools.\"npm:prettier\"]\nversion = \"3.9.6\"\n"
        ));
        // キー = 値 (空白の有無を問わない)
        assert!(!looks_like_tool_versions("node = \"26.7.0\"\n"));
        assert!(!looks_like_tool_versions("node=\"26.7.0\"\n"));
        assert!(!looks_like_tool_versions("node =\"26.7.0\"\n"));
        assert!(!looks_like_tool_versions("node= 26.7.0\n"));
        // dotted key / inline table
        assert!(!looks_like_tool_versions("tools.node = \"26.7.0\"\n"));
        assert!(!looks_like_tool_versions(
            "[tools]\njava = { version = \"temurin-21\" }\n"
        ));
        // 壊れた TOML (閉じていない文字列) も .tool-versions とは認めない
        assert!(!looks_like_tool_versions("[tools]\nnode = \"26.7.0\n"));
        // バージョンのない行
        assert!(!looks_like_tool_versions("node\n"));
        // 空・コメントだけのファイルはエントリなし
        assert!(!looks_like_tool_versions(""));
        assert!(!looks_like_tool_versions("# comment only\n"));
    }

    /// バグ回帰テスト: parse が採用しなかった行を writer が書き換えない。
    ///
    /// `python latest` は parse が捨てる (浮動指定) ので依存の版は 2 行目由来。
    /// 以前は writer が「ツール名が一致する最初の行」を無条件に潰しており、
    /// asdf/mise が実際に採用する先頭行の `latest` が無言でピンに変わっていた。
    #[test]
    fn test_update_skips_unparsable_version_line() {
        let content = "python latest\npython 3.13.1\n";
        let updated = ToolVersionsParser
            .update_version(content, "python", "3.14.0")
            .unwrap();
        assert_eq!(updated, "python latest\npython 3.14.0\n");
    }

    /// 浮動指定・VCS ref・path 指定の行だけなら書き換え対象が無くエラーになる
    #[test]
    fn test_update_only_unparsable_lines_is_error() {
        for content in [
            "python latest\n",
            "erlang ref:master\n",
            "shfmt path:./shfmt\n",
            "node lts\n",
            "python sub-0.1:latest\n",
        ] {
            let tool = content.split_whitespace().next().unwrap();
            assert!(
                ToolVersionsParser
                    .update_version(content, tool, "9.9.9")
                    .is_err(),
                "unexpectedly updated an unparsable line: {content:?}"
            );
        }
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
