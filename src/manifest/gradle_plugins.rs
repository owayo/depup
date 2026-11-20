//! ビルドスクリプトのトップレベルの plugins ブロックにあるリテラル版を扱う。

use crate::domain::{Dependency, Language, VersionSpec};
use crate::error::ManifestError;
use crate::parser::VersionParser;
use crate::registry::is_valid_registry_id_segment;
use regex::{Match, Regex};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::LazyLock;

/// 文字列やコメント内のトークンを別の宣言として拾わない。トークンの位置は
/// 元のソース上のバイト位置なので、そのまま書き換えに使える。
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r###"(?xs)
          /\*
        | // [^\r\n]*
        | """ .*? (?: """ | \z )
        | ''' .*? (?: ''' | \z )
        | " (?: \\. | [^"\\] )* (?: " | \z )
        | ' (?: \\. | [^'\\] )* (?: ' | \z )
        | \$/ .*? (?: /\$ | \z )
        | \x60 [^\x60\r\n]* \x60
        | [\w$]+
        | \S
        "###,
    )
    .unwrap()
});

static BLOCK_COMMENT_DELIMITER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/\*|\*/").unwrap());

fn block_comment_end(content: &str, start: usize) -> usize {
    let mut depth = 1;
    let mut first_close = None;
    for delimiter in BLOCK_COMMENT_DELIMITER.find_iter(&content[start..]) {
        if delimiter.as_str() == "/*" {
            depth += 1;
        } else {
            let end = start + delimiter.end();
            first_close.get_or_insert(end);
            depth -= 1;
            if depth == 0 {
                return end;
            }
        }
    }
    // Kotlin はコメントをネストできる。閉じが最初の一つだけなら Groovy と同様にそこで抜ける。
    first_close.unwrap_or(content.len())
}

fn tokens(content: &str) -> Vec<Match<'_>> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(token) = TOKEN.find_at(content, cursor) {
        cursor = token.end();
        if token.as_str() == "/*" {
            cursor = block_comment_end(content, cursor);
        } else if !token.as_str().starts_with("//") {
            result.push(token);
        }
    }
    result
}

pub(super) fn plugin_marker(id: &str) -> String {
    format!("{id}:{id}.gradle.plugin")
}

struct PluginVersion<'a> {
    id: &'a str,
    spec: VersionSpec,
    span: Range<usize>,
}

fn token_is(tokens: &[Match<'_>], index: usize, expected: &str) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| token.as_str() == expected)
}

fn starts_statement(content: &str, tokens: &[Match<'_>], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let previous = tokens[index - 1];
    matches!(previous.as_str(), "{" | "}" | ";")
        || (!matches!(previous.as_str(), "." | "=" | "+")
            && content[previous.end()..tokens[index].start()].contains('\n'))
}

fn argument<'a>(tokens: &[Match<'a>], index: usize) -> Option<(Match<'a>, usize)> {
    let first = *tokens.get(index)?;
    if first.as_str() == "(" {
        let value = *tokens.get(index + 1)?;
        return token_is(tokens, index + 2, ")").then_some((value, index + 3));
    }
    Some((first, index + 1))
}

fn literal(token: Match<'_>) -> Option<&str> {
    let text = token.as_str();
    let quote = text.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    text.strip_prefix(quote)?.strip_suffix(quote)
}

fn plugin_version<'a>(
    content: &str,
    tokens: &[Match<'a>],
    index: usize,
    parser: &dyn VersionParser,
) -> Option<PluginVersion<'a>> {
    let (id, next) = argument(tokens, index + 1)?;
    let id = literal(id).filter(|id| is_valid_registry_id_segment(id))?;
    let method = next + usize::from(token_is(tokens, next, "."));
    if !token_is(tokens, method, "version") {
        return None;
    }
    let (version, next) = argument(tokens, method + 1)?;
    let raw = literal(version)?;
    let spec = parser.parse(raw).filter(|spec| spec.raw == raw)?;
    let method = next + usize::from(token_is(tokens, next, "."));
    let end = if token_is(tokens, method, "apply") {
        let (apply, end) = argument(tokens, method + 1)?;
        if !matches!(apply.as_str(), "true" | "false") {
            return None;
        }
        end
    } else {
        next
    };
    if let Some(token) = tokens.get(end)
        && !matches!(token.as_str(), "}" | ";")
        && (matches!(token.as_str(), "." | "+") || !starts_statement(content, tokens, end))
    {
        return None;
    }
    Some(PluginVersion {
        id,
        spec,
        span: version.start() + 1..version.end() - 1,
    })
}

fn plugin_versions<'a>(content: &'a str, parser: &dyn VersionParser) -> Vec<PluginVersion<'a>> {
    if !content.contains("plugins") {
        return Vec::new();
    }
    let tokens = tokens(content);
    let mut scopes = Vec::new();
    let mut versions = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "{" | "(" | "[" => {
                let opens_plugins = token.as_str() == "{"
                    && scopes.is_empty()
                    && index > 0
                    && token_is(&tokens, index - 1, "plugins")
                    && starts_statement(content, &tokens, index - 1);
                scopes.push(opens_plugins);
            }
            "}" | ")" | "]" => {
                scopes.pop();
            }
            "id" if scopes.as_slice() == [true] && starts_statement(content, &tokens, index) => {
                versions.extend(plugin_version(content, &tokens, index, parser));
            }
            _ => {}
        }
    }
    versions
}

pub(super) fn parse(content: &str, parser: &dyn VersionParser) -> impl Iterator<Item = Dependency> {
    plugin_versions(content, parser)
        .into_iter()
        .map(|plugin| Dependency::production(plugin_marker(plugin.id), plugin.spec, Language::Java))
}

pub(super) fn update_version(
    content: &str,
    package: &str,
    new_version: &str,
    parser: &dyn VersionParser,
) -> Result<Option<String>, ManifestError> {
    let Some((id, artifact)) = package.split_once(':') else {
        return Ok(None);
    };
    if artifact.strip_suffix(".gradle.plugin") != Some(id) {
        return Ok(None);
    }
    let mut matches = plugin_versions(content, parser)
        .into_iter()
        .filter(|plugin| plugin.id == id);
    let Some(plugin) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: "multiple Gradle plugin declarations".to_string(),
        });
    }
    if parser
        .parse(new_version)
        .is_none_or(|spec| spec.raw != new_version)
    {
        return Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: new_version.to_string(),
            message: "invalid Gradle plugin version".to_string(),
        });
    }
    let mut updated = content.to_string();
    updated.replace_range(plugin.span, new_version);
    Ok(Some(updated))
}
