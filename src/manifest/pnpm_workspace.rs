//! pnpm-workspace.yaml の `packages` の読み取りと、そのパターンの照合 (#163)。
//!
//! pnpm (`@pnpm/fs.find-packages`) は、各パターンに `/package.json` を付けて glob で探し、
//! `!` で始まるパターンを除外として扱う。既定で `**/node_modules/**` と
//! `**/bower_components/**` も除く。含めるパターンの `*` と `**` は `.` で始まる名前に
//! 一致しない (fast-glob と tinyglobby の `dot: false`)。
//!
//! depup が照合できる構文は、相対の直接パス、要素の中の `*`、要素全体の `**` だけ。
//! `?`・文字クラス・brace・extglob・エスケープは解釈しない。除外のパターンにこれらがあると、
//! 除外の漏れで外したパッケージを書き換えないよう、呼び出し側はワークスペースのメンバーの
//! 展開を止める。

/// ワークスペースのパターン 1 つ (除外の `!` を外した形)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceGlob {
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// `**`: 0 個以上の要素
    Globstar,
    /// `*` を含みうる要素 1 つ
    Name(String),
}

/// 照合できない構文と、その理由 (警告に出す)。
const UNSUPPORTED_SYNTAX: &[(char, &str)] = &[
    ('?', "`?` is not supported"),
    ('[', "character classes are not supported"),
    (']', "character classes are not supported"),
    ('{', "brace expansion is not supported"),
    ('}', "brace expansion is not supported"),
    ('(', "extglobs and groups are not supported"),
    (')', "extglobs and groups are not supported"),
    ('|', "extglobs and groups are not supported"),
    ('!', "negation inside a pattern is not supported"),
    ('\\', "escapes are not supported"),
];

impl WorkspaceGlob {
    /// パターンを解釈する。照合できない構文なら、その理由を返す。
    ///
    /// 先頭の `./`、途中の `.`、重複した `/`、末尾の `/` は畳む (pnpm も末尾の `/` を外して
    /// `/package.json` を付ける)。プロジェクトの外を指す絶対パスと `..` は扱わない。
    pub(super) fn parse(pattern: &str) -> Result<Self, &'static str> {
        if let Some((_, reason)) = UNSUPPORTED_SYNTAX
            .iter()
            .find(|(symbol, _)| pattern.contains(*symbol))
        {
            return Err(reason);
        }
        if pattern.starts_with('/') || has_drive_prefix(pattern) {
            return Err("absolute paths are not supported");
        }

        let mut segments = Vec::new();
        for part in pattern.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err("parent directories are not supported"),
                "**" => {
                    if segments.last() != Some(&Segment::Globstar) {
                        segments.push(Segment::Globstar);
                    }
                }
                part if part.contains("**") => return Err("`**` must be a whole path segment"),
                part => segments.push(Segment::Name(part.to_string())),
            }
        }
        Ok(Self { segments })
    }

    /// ルートからの相対パス (要素の並び) のディレクトリが、このパターンに一致するか。
    ///
    /// pnpm はパターンに `/package.json` を付けて照合するので、`packages/test/**` は
    /// `packages/test` 自身にも一致する (`**` は 0 個の要素にも一致する)。`dot` が false なら、
    /// `*` と `**` は `.` で始まる名前に一致しない (明示した `.cache` は一致する)。
    pub(super) fn matches(&self, path: &[&str], dot: bool) -> bool {
        matches_from(&self.segments, path, dot)
    }
}

fn has_drive_prefix(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn matches_from(segments: &[Segment], path: &[&str], dot: bool) -> bool {
    match segments.split_first() {
        None => path.is_empty(),
        Some((Segment::Globstar, rest)) => {
            // 0 個の要素に一致させるか、先頭の要素を 1 つ飲み込んで続ける
            matches_from(rest, path, dot)
                || path.split_first().is_some_and(|(name, tail)| {
                    (dot || !name.starts_with('.')) && matches_from(segments, tail, dot)
                })
        }
        Some((Segment::Name(pattern), rest)) => path.split_first().is_some_and(|(name, tail)| {
            name_matches(pattern, name, dot) && matches_from(rest, tail, dot)
        }),
    }
}

/// 要素 1 つの照合。`*` は `/` を含まない任意の文字列に一致する。
fn name_matches(pattern: &str, name: &str, dot: bool) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    if !dot && name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split は 1 つ以上の要素を返す");
    let (last, middle) = rest.split_last().expect("`*` を含むので 2 つ以上ある");
    let Some(mut remaining) = name.strip_prefix(first) else {
        return false;
    };
    for part in middle {
        let Some(index) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[index + part.len()..];
    }
    remaining.len() >= last.len() && remaining.ends_with(last)
}

/// `packages` の値 (文字列の配列) を、書かれた順に返す。`packages` が無ければ空。
///
/// block 形式 (`- 'packages/*'`) と flow 形式 (`['a/*', 'b/*']`。複数行も可) を読む。
/// 引用符の中の `,`・`]`・`#` は値の一部として扱う。閉じていない引用符や `]` は、
/// パターンを黙って落とさないようエラーにする。
pub(super) fn package_patterns(content: &str) -> Result<Vec<String>, String> {
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim().strip_prefix("packages:") else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.starts_with('[') {
            let mut text = rest.to_string();
            for line in lines {
                text.push('\n');
                text.push_str(line);
            }
            return flow_sequence(&text);
        }
        if !rest.is_empty() && !rest.starts_with('#') {
            return Err("`packages` must be a list".to_string());
        }
        return block_sequence(lines);
    }
    Ok(Vec::new())
}

/// `packages:` の次の行からの block 形式の配列を読む。`-` で始まらない行で終わる。
fn block_sequence<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Vec<String>, String> {
    let mut patterns = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(item) = trimmed.strip_prefix('-') else {
            break;
        };
        if let Some(value) = block_scalar(item)? {
            patterns.push(value);
        }
    }
    Ok(patterns)
}

/// block 形式の要素 1 つの値。値の無い要素 (`-` だけ) は `None`。
fn block_scalar(item: &str) -> Result<Option<String>, String> {
    let item = item.trim();
    let mut chars = item.chars();
    match chars.next() {
        None | Some('#') => Ok(None),
        Some(quote @ ('\'' | '"')) => {
            let (value, rest) = quoted_scalar(chars.as_str(), quote)?;
            let rest = rest.trim_start();
            if rest.is_empty() || rest.starts_with('#') {
                Ok(Some(value))
            } else {
                Err(format!("unexpected text after a quoted value: {item}"))
            }
        }
        Some(_) => {
            // 引用符の無い値は、空白と `#` から後ろがコメント
            let value = item.find(" #").map_or(item, |index| &item[..index]).trim();
            Ok(Some(value.to_string()))
        }
    }
}

/// 開き引用符の後ろから、閉じ引用符までの値と、その後ろの残りを返す。
///
/// 単一引用符は `''` で `'` を表す。二重引用符は `\"`・`\\`・`\/`・`\t`・`\n` だけを読む。
fn quoted_scalar(text: &str, quote: char) -> Result<(String, &str), String> {
    let mut value = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\'' if quote == '\'' => {
                if chars.peek().is_some_and(|(_, next)| *next == '\'') {
                    chars.next();
                    value.push('\'');
                } else {
                    return Ok((value, &text[index + 1..]));
                }
            }
            '"' if quote == '"' => return Ok((value, &text[index + 1..])),
            '\\' if quote == '"' => match chars.next().map(|(_, escaped)| escaped) {
                Some(escaped @ ('"' | '\\' | '/')) => value.push(escaped),
                Some('t') => value.push('\t'),
                Some('n') => value.push('\n'),
                _ => return Err(format!("unsupported escape in {quote}{text}")),
            },
            // 引用符の中の改行は、YAML と同じく空白に畳む
            '\n' => value.push(' '),
            _ => value.push(ch),
        }
    }
    Err(format!("unterminated quoted value: {quote}{text}"))
}

/// `[` から始まる flow 形式の配列を、対応する `]` まで読む。
fn flow_sequence(text: &str) -> Result<Vec<String>, String> {
    let mut patterns = Vec::new();
    let mut rest = &text[1..];
    loop {
        rest = skip_flow_space(rest);
        let Some(ch) = rest.chars().next() else {
            return Err("unclosed `[` in `packages`".to_string());
        };
        match ch {
            ']' => return Ok(patterns),
            ',' => {
                rest = &rest[1..];
                continue;
            }
            '\'' | '"' => {
                let (value, after) = quoted_scalar(&rest[1..], ch)?;
                patterns.push(value);
                rest = skip_flow_space(after);
            }
            _ => {
                let end = rest.find([',', ']', '\n']).unwrap_or(rest.len());
                let value = rest[..end].trim();
                if value.contains(['[', '{', '}', '\'', '"', '#']) {
                    return Err(format!(
                        "cannot read the unquoted value `{value}` in `packages`"
                    ));
                }
                patterns.push(value.to_string());
                rest = &rest[end..];
            }
        }
        match rest.chars().next() {
            Some(',') => rest = &rest[1..],
            Some(']') => return Ok(patterns),
            Some('\n') => {}
            Some(other) => {
                return Err(format!("unexpected `{other}` in `packages`"));
            }
            None => return Err("unclosed `[` in `packages`".to_string()),
        }
    }
}

/// flow 形式の空白・改行・コメント (`#` から行末まで) を読み飛ばす。
fn skip_flow_space(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start();
        if let Some(comment) = trimmed.strip_prefix('#') {
            text = comment.find('\n').map_or("", |index| &comment[index..]);
        } else {
            return trimmed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(pattern: &str) -> WorkspaceGlob {
        WorkspaceGlob::parse(pattern).unwrap()
    }

    fn path(text: &str) -> Vec<&str> {
        text.split('/').filter(|part| !part.is_empty()).collect()
    }

    #[test]
    fn globstar_matches_zero_or_more_segments_on_both_sides() {
        let test = glob("**/test/**");
        for matched in ["test", "packages/test", "packages/a/test/b", "test/nested"] {
            assert!(test.matches(&path(matched), true), "{matched}");
        }
        for unmatched in ["packages/test-app", "packages/mytest", "packages/app"] {
            assert!(!test.matches(&path(unmatched), true), "{unmatched}");
        }
    }

    #[test]
    fn direct_and_recursive_exclusions_differ() {
        let direct = glob("packages/test");
        assert!(direct.matches(&path("packages/test"), true));
        assert!(!direct.matches(&path("packages/test/nested"), true));

        let recursive = glob("packages/test/**");
        assert!(recursive.matches(&path("packages/test"), true));
        assert!(recursive.matches(&path("packages/test/nested"), true));
    }

    #[test]
    fn star_stays_within_one_segment() {
        let star = glob("packages/*");
        assert!(star.matches(&path("packages/app"), false));
        assert!(!star.matches(&path("packages"), false));
        assert!(!star.matches(&path("packages/a/b"), false));

        let affixed = glob("crates/util-*-rs");
        assert!(affixed.matches(&path("crates/util-core-rs"), false));
        assert!(affixed.matches(&path("crates/util--rs"), false));
        assert!(!affixed.matches(&path("crates/util-rs"), false));
        assert!(!affixed.matches(&path("crates/core"), false));
    }

    #[test]
    fn wildcards_skip_dot_names_unless_dot_is_set_or_the_name_is_explicit() {
        let star = glob("packages/*");
        assert!(!star.matches(&path("packages/.cache"), false));
        assert!(star.matches(&path("packages/.cache"), true));
        assert!(glob("packages/.cache").matches(&path("packages/.cache"), false));
        assert!(glob("packages/.*").matches(&path("packages/.cache"), false));

        let globstar = glob("**/test");
        assert!(!globstar.matches(&path(".hidden/test"), false));
        assert!(globstar.matches(&path(".hidden/test"), true));
    }

    #[test]
    fn patterns_are_normalized() {
        assert_eq!(glob("./packages//test/"), glob("packages/test"));
        assert_eq!(glob("packages/./test"), glob("packages/test"));
        assert_eq!(glob("**/**/test"), glob("**/test"));
        assert!(glob(".").matches(&[], false));
    }

    #[test]
    fn unsupported_syntax_is_reported() {
        for pattern in [
            "packages/{a,b}",
            "packages/[ab]",
            "packages/?",
            "packages/@(a|b)",
            "packages/+(a)",
            "!packages/a",
            "packages\\a",
            "../shared/*",
            "/abs/*",
            "C:/work/*",
            "packages/a**b",
            "**test",
        ] {
            assert!(WorkspaceGlob::parse(pattern).is_err(), "{pattern}");
        }
    }

    #[test]
    fn block_sequences_keep_quoted_text_and_drop_comments() {
        let content = "packages:\n  # members\n  - 'packages/*' # apps\n  - \"!**/test/**\"\n  - apps/web # web\n  - 'it''s'\n  -\nonlyBuiltDependencies:\n  - esbuild\n";
        assert_eq!(
            package_patterns(content).unwrap(),
            vec!["packages/*", "!**/test/**", "apps/web", "it's"]
        );
    }

    #[test]
    fn flow_sequences_keep_commas_and_brackets_inside_quotes() {
        assert_eq!(
            package_patterns("packages: ['packages/*', '!packages/{test,tests}/**']\n").unwrap(),
            vec!["packages/*", "!packages/{test,tests}/**"]
        );
        assert_eq!(
            package_patterns("packages: [\"packages/[ab]\", apps/*]\n").unwrap(),
            vec!["packages/[ab]", "apps/*"]
        );
        assert_eq!(
            package_patterns(
                "packages: [\n  'packages/*', # members\n  '!packages/legacy',\n]\ncatalog:\n  react: ^18\n"
            )
            .unwrap(),
            vec!["packages/*", "!packages/legacy"]
        );
    }

    #[test]
    fn malformed_lists_are_errors_rather_than_dropped_patterns() {
        for content in [
            "packages:\n  - '!packages/test\n",
            "packages: ['packages/*', '!packages/test\n",
            "packages: ['packages/*'\n",
            "packages: [packages/{a,b}]\n",
            "packages: 'packages/*'\n",
        ] {
            assert!(package_patterns(content).is_err(), "{content:?}");
        }
    }

    #[test]
    fn a_missing_packages_key_lists_nothing() {
        assert!(
            package_patterns("catalog:\n  react: ^18\n")
                .unwrap()
                .is_empty()
        );
    }
}
