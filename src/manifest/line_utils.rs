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

#[cfg(test)]
mod tests {
    use super::*;

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
}
