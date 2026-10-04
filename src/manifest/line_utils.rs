//! マニフェストパーサ間で共有する行・キャプチャ処理ヘルパ。

/// 行から改行コード (`\r\n` / `\n` / なし) を分離して (本文, 改行) を返す。
/// CRLF ファイルの更新で行末を保持するために使う (content.lines()+join は CRLF を潰す)。
pub(crate) fn split_line_ending(raw_line: &str) -> (&str, &str) {
    if let Some(body) = raw_line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = raw_line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (raw_line, "")
    }
}

/// 正規表現キャプチャから引用符種別と旧バージョン文字列 (グループ 2/3) を取り出す
pub(crate) fn captured_quote_and_version<'a>(
    caps: &regex::Captures<'a>,
) -> (&'static str, &'a str) {
    if let Some(m) = caps.get(2) {
        ("\"", m.as_str())
    } else if let Some(m) = caps.get(3) {
        ("'", m.as_str())
    } else {
        ("\"", "")
    }
}

/// クォート外の `#` 以降を落とす際のエスケープ規則
pub(crate) enum HashCommentMode {
    /// バックスラッシュエスケープを解釈する (Ruby / Gemfile)
    BackslashEscapes,
    /// バックスラッシュをリテラル扱いする (TOML)
    Plain,
}

/// クォート外の `#` 以降 (行コメント) を取り除いた部分文字列を返す。
/// 文字列リテラル内 (`"..."` / `'...'`) の `#` はコメント扱いせず保持する。
/// コメントがなければ行全体 (改行コードや末尾の空白込み) をそのまま返す。
pub(crate) fn strip_hash_line_comment(line: &str, mode: HashCommentMode) -> &str {
    let interpret_backslash = matches!(mode, HashCommentMode::BackslashEscapes);
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if interpret_backslash && (in_single || in_double) => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// 行が TOML のセクションヘッダ (`[key]` / `[[key]]`、行末 `#` コメント許容) なら
/// ドット区切りのセクションキーを取り出す。
///
/// cargo_toml / gradle_version_catalog / pyproject_toml のセクション追跡が共有する
/// 字句解析の単一情報源。`[[key]]` (array of tables) も通常セクションと同じく
/// キーを返す (3 呼び手とも依存セクション名の照合にのみ使うため区別不要。
/// 区別が必要になったらフラグ付きの戻り値へ拡張する)。
/// キーの前後空白は除去する (`[ deps ]` → `deps`。TOML 仕様はヘッダ内の空白を
/// 許容するため、toml クレートによる parse 側の解釈と一致させる)。
/// 空キー (`[]`) や `]` の後にコメント以外が続く行はヘッダとして扱わない。
pub(crate) fn parse_toml_section_header(line: &str) -> Option<&str> {
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

/// TOML の 1 行を走査し、文字列リテラルの外側にある文字だけを `visit` へ渡す。
///
/// - 単一行の基本文字列 (`"..."`、バックスラッシュエスケープを解釈) とリテラル文字列
///   (`'...'`) の内側は訪問しない。
/// - 文字列外の `#` に達した時点で走査を打ち切る (行コメント)。
/// - 行内で閉じないマルチライン文字列 (`"""` / `'''`) を開いた場合はその区切りを返す。
///
/// マルチライン区切りを単一行クォートより**先に**判定するのが要点。逆にすると
/// `"""` の 1 文字目で単一行の基本文字列に入ったと誤認する。
pub(crate) fn scan_toml_line_outside_strings(
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
pub(crate) fn skip_single_line_string(rest: &str, basic: bool) -> usize {
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
pub(crate) fn opens_unclosed_multiline_string(line: &str) -> Option<&'static str> {
    scan_toml_line_outside_strings(line, |_, _| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_delimiters_inside_strings_and_comments_do_not_open_a_block() {
        for line in [
            r#"description = "Ain't got '''""#,
            r#"description = 'see """ marker'"#,
            r##"description = "escaped \"#'''" # """"##,
            r#"description = "日本語 '''" # """"#,
            r#"# description = """"#,
            r#"description = """closed""" # '''"#,
        ] {
            assert_eq!(opens_unclosed_multiline_string(line), None, "{line}");
        }
        assert_eq!(
            opens_unclosed_multiline_string(r#"run = """echo"#),
            Some("\"\"\"")
        );
        assert_eq!(
            opens_unclosed_multiline_string("run = '''echo"),
            Some("'''")
        );
    }

    #[test]
    fn toml_scanner_visits_only_syntax_outside_strings() {
        let line = r##"deps = ["pkg[extra]", 'a{b}', "escaped \"#]"] # }"##;
        let mut brackets = String::new();
        assert_eq!(
            scan_toml_line_outside_strings(line, |_, ch| {
                if "[]{}".contains(ch) {
                    brackets.push(ch);
                }
            }),
            None
        );
        assert_eq!(brackets, "[]");
    }

    #[test]
    fn test_strip_hash_line_comment_keeps_quoted_hash() {
        // クォート内の `#` はどちらのモードでもコメント扱いしない
        assert_eq!(
            strip_hash_line_comment("gem 'x#y' # comment", HashCommentMode::BackslashEscapes),
            "gem 'x#y' "
        );
        assert_eq!(
            strip_hash_line_comment("a = \"x#y\"  # comment", HashCommentMode::Plain),
            "a = \"x#y\"  "
        );
    }

    #[test]
    fn test_strip_hash_line_comment_mode_difference_on_escaped_quote() {
        // バックスラッシュでエスケープされたクォートを含む同一入力での挙動差:
        // BackslashEscapes は `\"` を文字列内のクォートとして解釈するため `#` を保持し、
        // Plain は `\` をリテラル扱いするため `"` で文字列が閉じて `#` がコメントになる
        let line = r##"key = "a\"#b" # comment"##;
        assert_eq!(
            strip_hash_line_comment(line, HashCommentMode::BackslashEscapes),
            r##"key = "a\"#b" "##
        );
        assert_eq!(
            strip_hash_line_comment(line, HashCommentMode::Plain),
            r#"key = "a\""#
        );
    }

    #[test]
    fn test_strip_hash_line_comment_without_comment_returns_line() {
        // コメントがなければ改行コード込みでそのまま返す
        assert_eq!(
            strip_hash_line_comment("a = 1\r\n", HashCommentMode::Plain),
            "a = 1\r\n"
        );
        assert_eq!(
            strip_hash_line_comment("gem 'rails'", HashCommentMode::BackslashEscapes),
            "gem 'rails'"
        );
    }

    #[test]
    fn test_parse_toml_section_header_basic_forms() {
        // 通常セクション / ドット区切り / array of tables
        assert_eq!(
            parse_toml_section_header("[dependencies]"),
            Some("dependencies")
        );
        assert_eq!(
            parse_toml_section_header("[tool.poetry.dependencies]"),
            Some("tool.poetry.dependencies")
        );
        assert_eq!(parse_toml_section_header("[[bin]]"), Some("bin"));
        // 行頭インデントと行末コメントを許容
        assert_eq!(
            parse_toml_section_header("  [versions]  # libs"),
            Some("versions")
        );
        assert_eq!(
            parse_toml_section_header("[libraries]#c"),
            Some("libraries")
        );
    }

    #[test]
    fn test_parse_toml_section_header_trims_inner_whitespace() {
        // TOML 仕様はヘッダ内の空白を許容する。toml クレートの parse 側と
        // 解釈を一致させるためキーの前後空白は除去する
        assert_eq!(
            parse_toml_section_header("[ dependencies ]"),
            Some("dependencies")
        );
        assert_eq!(parse_toml_section_header("[[ bin ]]"), Some("bin"));
    }

    #[test]
    fn test_parse_toml_section_header_rejects_non_headers() {
        // ヘッダ以外の行
        assert_eq!(parse_toml_section_header("version = \"1.0\""), None);
        // コメントアウトされたヘッダ
        assert_eq!(parse_toml_section_header("# [dependencies]"), None);
        // 空キー
        assert_eq!(parse_toml_section_header("[]"), None);
        assert_eq!(parse_toml_section_header("[  ]"), None);
        // `]` の後にコメント以外が続く行
        assert_eq!(parse_toml_section_header("[deps] junk"), None);
        assert_eq!(parse_toml_section_header("[a]b]"), None);
        // 閉じ括弧なし
        assert_eq!(parse_toml_section_header("[deps"), None);
    }
}
