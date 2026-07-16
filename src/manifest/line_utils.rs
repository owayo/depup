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
