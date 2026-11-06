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

/// 単一行の文字列 (`text[start]` が引用符) の終わり (閉じ引用符の次) の位置。
/// 改行に達する・閉じていないときは `None`。
fn single_line_string_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quote == b'"' => index += 2,
            b'\n' | b'\r' => return None,
            byte if byte == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// 複数行の文字列が始まるか (基本文字列と literal 文字列の両方)
fn starts_multiline_string(text: &str, index: usize) -> bool {
    text[index..].starts_with("\"\"\"") || text[index..].starts_with("'''")
}

/// 複数行の文字列 (`text[start..]` が `"""` か `'''`) の終わり (閉じる区切りの次) の位置。
/// 閉じていないときは `None`。TOML では閉じる区切りの直前に引用符を 2 つまで書けるので
/// (`""""` は内容の `"` と区切り)、続く引用符は 5 つまでを文字列に含める。
fn multiline_string_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let quote = bytes[start];
    let mut index = start + 3;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quote == b'"' => index += 2,
            byte if byte == quote => {
                let run = bytes[index..]
                    .iter()
                    .take(5)
                    .take_while(|&&byte| byte == quote)
                    .count();
                if run >= 3 {
                    return Some(index + run);
                }
                index += run;
            }
            _ => index += 1,
        }
    }
    None
}

/// 文字列 (`text[start]` が引用符) の終わり。複数行の文字列も読み飛ばす。
fn toml_string_end(text: &str, start: usize) -> Option<usize> {
    if starts_multiline_string(text, start) {
        multiline_string_end(text, start)
    } else {
        single_line_string_end(text, start)
    }
}

/// `text[open]` の `{` から始まる inline table の終わり (`}` の次の位置)。行をまたいでよい
/// (TOML 1.0 でも配列の中と複数行の文字列は改行でき、TOML 1.1 は inline table の中も
/// 改行できる)。入れ子の配列と表、文字列、コメントを考慮する。閉じていない・括弧が
/// 食い違うときは `None`。
pub(crate) fn toml_inline_table_end(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut stack: Vec<u8> = Vec::new();
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'"' | b'\'' => {
                index = toml_string_end(text, index)?;
                continue;
            }
            b'#' => {
                index = text[index..]
                    .find('\n')
                    .map_or(bytes.len(), |offset| index + offset);
                continue;
            }
            bracket @ (b'{' | b'[') => stack.push(bracket),
            b'}' => {
                if stack.pop()? != b'{' {
                    return None;
                }
                if stack.is_empty() {
                    return Some(index + 1);
                }
            }
            b']' if stack.pop()? != b'[' => return None,
            _ => {}
        }
        index += 1;
    }
    None
}

/// inline table (`text[open..end]`) の直下のキー `key` の、単一行の文字列の値の範囲
/// (引用符を含む)。入れ子の値の中や文字列の中の同じ名前は見ない。キーの値が単一行の
/// 文字列でない (複数行の文字列や dotted key の一部を含む) ときと、キーがないときは `None`。
pub(crate) fn toml_inline_table_string_value(
    text: &str,
    open: usize,
    end: usize,
    key: &str,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut expecting_key = false;
    let mut index = open;
    while index < end {
        match bytes[index] {
            b'{' | b'[' => {
                depth += 1;
                expecting_key = depth == 1;
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b',' => {
                expecting_key = depth == 1;
                index += 1;
            }
            b'#' => {
                index = text[index..end]
                    .find('\n')
                    .map_or(end, |offset| index + offset);
            }
            b'"' | b'\'' => {
                // 複数行の文字列はキーにならないので、値として読み飛ばす
                let multiline = starts_multiline_string(text, index);
                let string_end = toml_string_end(text, index)?;
                if depth == 1 && expecting_key {
                    if !multiline && &text[index + 1..string_end - 1] == key {
                        return string_value_after_key(text, string_end, end);
                    }
                    expecting_key = false;
                }
                index = string_end;
            }
            byte if byte.is_ascii_whitespace() => index += 1,
            byte if depth == 1 && expecting_key && is_bare_key_byte(byte) => {
                let start = index;
                while index < end && is_bare_key_byte(bytes[index]) {
                    index += 1;
                }
                if &text[start..index] == key {
                    return string_value_after_key(text, index, end);
                }
                expecting_key = false;
            }
            _ => index += 1,
        }
    }
    None
}

fn is_bare_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

/// キーの直後 (`after`) の `=` に続く単一行の文字列の値の範囲 (引用符を含む)。
fn string_value_after_key(text: &str, after: usize, end: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let skip_blank = |mut index: usize| {
        while index < end && matches!(bytes[index], b' ' | b'\t') {
            index += 1;
        }
        index
    };
    let equals = skip_blank(after);
    // `version.workspace = true` のような dotted key は文字列の値ではない
    if bytes.get(equals) != Some(&b'=') {
        return None;
    }
    let value = skip_blank(equals + 1);
    if !matches!(bytes.get(value), Some(b'"' | b'\'')) || starts_multiline_string(text, value) {
        return None;
    }
    let value_end = single_line_string_end(text, value)?;
    (value_end <= end).then_some((value, value_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_tables_may_span_lines_with_nested_values_strings_and_comments() {
        let text = "x = { features = [\n  \"rt\", # comment }\n  \"a}b\",\n], nested = { k = '}' }, version = \"1.0\" } # tail\n";
        let open = text.find('{').unwrap();
        let end = toml_inline_table_end(text, open).unwrap();
        assert_eq!(&text[end - 1..end], "}");
        assert!(text[end..].starts_with(" # tail"));
        let (start, value_end) =
            toml_inline_table_string_value(text, open, end, "version").unwrap();
        assert_eq!(&text[start..value_end], "\"1.0\"");
        // 入れ子の値や文字列の中の同じ名前は直下のキーではない
        assert_eq!(toml_inline_table_string_value(text, open, end, "k"), None);
        assert_eq!(toml_inline_table_string_value(text, open, end, "rt"), None);
        // 閉じていない・括弧が食い違う inline table は扱わない
        for broken in [
            "x = { version = \"1\"\n",
            "x = { a = [ } ]\n",
            "x = { a = \"\"\"\nb }\n",
        ] {
            let open = broken.find('{').unwrap();
            assert_eq!(toml_inline_table_end(broken, open), None, "{broken}");
        }
    }

    #[test]
    fn multiline_strings_inside_inline_tables_are_skipped_as_values() {
        // 複数行の文字列の中の括弧・引用符・コメントの記号は数えない。閉じる区切りの直前の
        // 引用符 (`""""` / `'''''`) は内容で、エスケープした引用符は区切りにならない。
        let text = "x = { run = \"\"\"\necho \\\"\"\" } # ]\"\"\"\", other = '''\n}''''', version = \"2\" }\n";
        assert!(toml::from_str::<toml::Value>(text).is_ok(), "{text}");
        let open = text.find('{').unwrap();
        let end = toml_inline_table_end(text, open).unwrap();
        assert_eq!(&text[end..], "\n");
        let (start, value_end) =
            toml_inline_table_string_value(text, open, end, "version").unwrap();
        assert_eq!(&text[start..value_end], "\"2\"");
        // 複数行の文字列の値は、単一行の文字列として書き換える先にならない
        assert_eq!(toml_inline_table_string_value(text, open, end, "run"), None);
        assert_eq!(
            toml_inline_table_string_value(text, open, end, "other"),
            None
        );
    }

    #[test]
    fn only_string_values_of_top_level_keys_are_found() {
        for (text, expected) in [
            ("{ 'version' = '2.0' }", Some("'2.0'")),
            ("{\n  version = \"1.0\",\n}", Some("\"1.0\"")),
            // dotted key と文字列以外の値は書き換え先にならない
            ("{ version.workspace = true }", None),
            ("{ version = 1 }", None),
            ("{ other = \"version = 'x'\" }", None),
        ] {
            let end = toml_inline_table_end(text, 0).unwrap();
            let found = toml_inline_table_string_value(text, 0, end, "version")
                .map(|(start, end)| &text[start..end]);
            assert_eq!(found, expected, "{text}");
        }
    }

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
