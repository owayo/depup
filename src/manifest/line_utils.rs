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
