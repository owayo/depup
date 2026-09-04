//! Ruby プロジェクト向けの `Gemfile` パーサ。
//!
//! 対応対象:
//! - バージョン制約付き `gem` 宣言
//! - 開発グループ依存関係
//! - ペシミスティック制約 (`~>`)
//! - 複数バージョン制約の解析
//! - シングルクォートとダブルクォート

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::{
    ManifestParser,
    line_utils::{HashCommentMode, split_line_ending, strip_hash_line_comment},
};
use crate::parser::get_parser;
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// `Gemfile` 用パーサ
pub struct GemfileParser;

enum GemfileBlock {
    Group(bool),
    /// `git ... do` / `github ... do` / `path ... do` / `source ... do` ブロック。
    /// 内側の gem は RubyGems のレジストリ依存ではないため更新対象から除外する
    /// (行オプション形式 `gem 'x', git: '...'` と同じ方針)
    NonRegistry,
    Other,
}

// `gem 'name'` / `gem "name"` / `gem('name')` を解釈する正規表現
static GEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 例:
    // 解析対象の例: gem 'rails', '~> 7.0'
    // 複合制約の例: gem "pg", ">= 0.18", "< 2.0"
    // 括弧付き呼び出しの例: gem("rack", "~> 3.0")
    // バージョン指定なしの例: gem 'bcrypt'
    // 末尾コンテキストは `,` / 行末 / `)` / コメント `#` に加え、行末条件修飾子
    // (`gem 'wdm', '>= 0.1.0' if Gem.win_platform?`) も許容する。これがないと
    // ` if ...` でバックトラックして version 引数を取りこぼし Any と誤分類する。
    Regex::new(
        r#"^\s*gem(?:\s+|\s*\(\s*)['"]([^'"]+)['"](?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,\s*['"]([^'"]+)['"])?(?:\s*,|\s*\)?\s*$|\s*\)?\s*#|\s*\)?\s+(?:if|unless)\b)"#,
    )
    .unwrap()
});

// `group ... do` 開始行
static GROUP_START_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 例:
    // 単一グループの例: group :development do
    // 複数グループの例: group :development, :test do
    // group :development do # security gems  <- 行末コメントも許容する
    Regex::new(r"^\s*group\s+(.+?)\s+do\s*(?:#.*)?$").unwrap()
});

// `group` ブロック終端
static GROUP_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*end\s*(?:#.*)?$").unwrap());

// `do` で終わるブロック開始行（group 以外）。
// `%w[...].each do |name|` / `git_source(:github) do |repo|` のようなブロック引数付きも
// ブロック開始として扱う。これを取りこぼすと、対応する `end` が別のブロックを pop して
// `source ... do` などの非レジストリブロックが早期に閉じ、内側の gem が
// rubygems.org の版で書き換えられてしまう。
static DO_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bdo(?:\s*\|[^|]*\|)?\s*(?:#.*)?$").unwrap());

// `end` を必要とするブロック開始キーワード (行頭のもののみ)。
// これらを push しないと、内側の `if ... end` の `end` が
// `group` / `source` ブロックを誤って pop してしまう。
// `else` / `elsif` / `rescue` / `ensure` / `when` / `in` は push も pop もしない。
static BLOCK_KEYWORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:if|unless|while|until|for|case|begin|def|class|module)\b").unwrap()
});

// 非レジストリソースのブロック開始行:
//   git "https://github.com/rails/rails.git", branch: "main" do
//   path "components" do
//   source "https://gems.example.com" do
// これらのブロック内の gem は rubygems.org の版で書き換えてはいけない
// (git/path なら `bundle install` が壊れ、private source なら同名の公開 gem の版が入る)
static NON_REGISTRY_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:git|github|gitlab|bitbucket|gist|path|source)\b").unwrap()
});

// `git_source(:stash) { |repo| "https://stash.example.com/#{repo}.git" }` の宣言。
// Bundler はここで登録した名前を git ソースのショートハンドオプションとして受け付ける
// (`gem 'rails', stash: 'forks/rails'`)。ブロックが `{ ... }` でも `do ... end` でも
// 行頭の形は同じなので `git_source(:NAME)` までを見れば足りる。
static GIT_SOURCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*git_source\(\s*:(\w+)\s*\)").unwrap());

// 開発用グループかどうかを判定する
fn is_dev_group(group_line: &str) -> bool {
    let lowered = group_line.to_lowercase();
    // `:development` と `:test` を開発系として扱う
    lowered.contains(":development") || lowered.contains(":test")
}

fn has_dev_group_option(line: &str) -> bool {
    let lowered = line.to_lowercase();
    let has_group_option = lowered.contains("group:")
        || lowered.contains("groups:")
        || lowered.contains(":group =>")
        || lowered.contains(":groups =>");
    has_group_option
        && (lowered.contains(":development")
            || lowered.contains(":test")
            || lowered.contains("\"development\"")
            || lowered.contains("\"test\"")
            || lowered.contains("'development'")
            || lowered.contains("'test'"))
}

/// Ruby のオプションキーは `key: value` と `:key => value` の 2 通りで書ける。
/// 両綴りを検出する (`has_dev_group_option` が `:group =>` を見ているのと揃える)。
fn has_option_key(lowered: &str, key: &str) -> bool {
    lowered.contains(&format!("{key}:"))
        || lowered.contains(&format!(":{key} =>"))
        || lowered.contains(&format!(":{key}=>"))
}

/// Bundler が組み込みで登録する git source ショートハンド。
/// `dsl.rb` の `add_git_sources` が `github` / `gist` / `bitbucket` / `gitlab` を登録する
/// (`gitlab` は Bundler 2.5.7 で追加)。`git` / `path` / `source` は通常のオプションキー
const NON_REGISTRY_OPTION_KEYS: [&str; 7] = [
    "git",
    "github",
    "gitlab",
    "bitbucket",
    "gist",
    "path",
    "source",
];

/// `git_source(:NAME)` で登録されたユーザー定義の git ソースショートハンドを集める。
///
/// Bundler は組み込みの `github` / `gitlab` などと同じ扱いでこの名前をオプションキーとして
/// 受け付けるため、集めないと `gem 'rails', stash: 'forks/rails'` が
/// 「バージョンなしの rubygems 依存」に見え、rubygems.org の同名 gem (typosquat を含む) の
/// 版を注入してしまう。parse と update_version の両方で同じ集合を使い、
/// 「parse は除外したのに writer が書き換える」非対称を作らない。
fn collect_git_source_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // コメントアウトされた宣言を拾わないよう、行コメント除去後に判定する
        if let Some(caps) = GIT_SOURCE_RE.captures(strip_line_comment(trimmed)) {
            keys.push(caps[1].to_lowercase());
        }
    }
    keys
}

fn has_non_registry_source(line: &str, custom_git_sources: &[String]) -> bool {
    let lowered = line.to_lowercase();
    NON_REGISTRY_OPTION_KEYS
        .iter()
        .copied()
        .any(|key| has_option_key(&lowered, key))
        || custom_git_sources
            .iter()
            .any(|key| has_option_key(&lowered, key))
}

/// クォート外の `#` 以降 (行コメント) を取り除いた部分文字列を返す。
/// 文字列リテラル内の `#` (例: `source 'http://x#y'`) はコメント扱いしない
fn strip_line_comment(line: &str) -> &str {
    // Ruby はバックスラッシュエスケープを解釈する (`"a\"#b"` の `#` は文字列内)
    strip_hash_line_comment(line, HashCommentMode::BackslashEscapes)
}

/// クォート外に Ruby の `end` キーワードがあるかを判定する。
///
/// `if condition then ... end` のように同一行で完結するブロックをスタックへ
/// 積まないために使う。文字列中の `"end"` は終端として扱わない。
fn has_unquoted_end_keyword(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut quote = None;
    let mut escaped = false;

    for index in 0..bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            continue;
        }
        if quote.is_some() && byte == b'\\' {
            escaped = true;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
            continue;
        }
        if quote.is_some() || bytes.get(index..index + 3) != Some(b"end") {
            continue;
        }

        // Ruby の識別子は Unicode も許容するため、非 ASCII バイトも境界にはしない。
        let is_identifier_byte =
            |value: u8| value.is_ascii_alphanumeric() || value == b'_' || !value.is_ascii();
        let has_left_boundary = index == 0 || !is_identifier_byte(bytes[index - 1]);
        let has_right_boundary = bytes
            .get(index + 3)
            .is_none_or(|value| !is_identifier_byte(*value));
        if has_left_boundary && has_right_boundary {
            return true;
        }
    }

    false
}

/// ブロックの開始・終了行ならスタックを更新して `true` を返す。
///
/// parse と `update_version` が同じ規則を共有するための唯一の情報源。片方にしか
/// 追跡がないと「parse は `path ... do` ブロック内の gem を除外したのに、writer は
/// 同名の行を全部書き換えてローカル依存へ rubygems の版を注入する」非対称が起きる。
fn track_block_structure(code: &str, block_stack: &mut Vec<GemfileBlock>) -> bool {
    // `group ... do` を積む
    if let Some(caps) = GROUP_START_RE.captures(code) {
        let group_spec = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        block_stack.push(GemfileBlock::Group(is_dev_group(group_spec)));
        return true;
    }

    // group 以外の `do` ブロック（platforms, git, path, source 等）を追跡する
    if DO_BLOCK_RE.is_match(code) {
        block_stack.push(if NON_REGISTRY_BLOCK_RE.is_match(code) {
            GemfileBlock::NonRegistry
        } else {
            GemfileBlock::Other
        });
        return true;
    }

    // `if` / `case` / `begin` 等も `end` を消費するため、スタックの深さを合わせる。
    // (行末修飾子の `... if cond` は行頭が `if` ではないので該当しない)
    if BLOCK_KEYWORD_RE.is_match(code) {
        if !has_unquoted_end_keyword(code) {
            block_stack.push(GemfileBlock::Other);
        }
        return true;
    }

    // 対応する `end` で適切なスタック/カウンタを戻す
    if GROUP_END_RE.is_match(code) {
        block_stack.pop();
        return true;
    }

    false
}

/// レジストリ外ソース (`git` / `path` / `source` 等) のブロック内かどうか
fn in_non_registry_block(block_stack: &[GemfileBlock]) -> bool {
    block_stack
        .iter()
        .any(|block| matches!(block, GemfileBlock::NonRegistry))
}

/// 更新後の制約文字列を、元の `gem` 引数の個数へ配り直す。
///
/// parse は `gem "pg", ">= 0.18", "< 2.0"` の複数引数を `", "` で 1 本に繋いでから
/// 解釈するため、`try_format_updated` の結果も 1 本の文字列 (`">= 1.5.0, < 2.0"`)
/// になる。書き戻しでは元の引数へ分け直す必要がある。要素数が一致しない場合は
/// どの引数へ何を書くか決められないため `None` を返し、呼び出し側でエラーにする。
fn split_updated_constraint(formatted: &str, original_parts: &[&str]) -> Option<Vec<String>> {
    if original_parts.len() <= 1 {
        // 単一引数はカンマを含んでいてもそのまま 1 つの文字列として書き戻す
        // (`gem "pg", ">= 0.18, < 2.0"` の形)
        return Some(vec![formatted.to_string()]);
    }

    // 元の引数自身がカンマを含むことがある (`gem 'pg', '>= 0.18, < 1.0', '<= 2.0'`)。
    // 単純にカンマ数で割ると要素数が合わずエラーになるので、引数ごとの
    // カンマ数だけトークンを取って配り直す。
    let tokens: Vec<&str> = formatted.split(',').map(str::trim).collect();
    let expected_tokens: usize = original_parts
        .iter()
        .map(|part| part.matches(',').count() + 1)
        .sum();
    if tokens.len() != expected_tokens {
        return None;
    }

    let mut rebuilt = Vec::with_capacity(original_parts.len());
    let mut cursor = 0usize;
    for part in original_parts {
        let take = part.matches(',').count() + 1;
        rebuilt.push(tokens[cursor..cursor + take].join(", "));
        cursor += take;
    }
    Some(rebuilt)
}

impl ManifestParser for GemfileParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Ruby);
        let mut block_stack = Vec::new();
        // `git_source(:NAME)` は宣言より後ろの gem でも先でも効くため、走査前に集める
        let custom_git_sources = collect_git_source_keys(content);

        for line in content.lines() {
            let trimmed = line.trim();

            // 空行とコメントは無視する
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // ブロック判定とオプション判定はコメント除去後の文字列で行う
            // (行末コメントの `do` や `git:` をブロック開始・git 依存と誤認しないため)
            let code = strip_line_comment(trimmed);

            // ブロックの開始・終了は update_version と同じ走査規則で追跡する
            if track_block_structure(code, &mut block_stack) {
                continue;
            }

            // `gem` 宣言を解釈する
            if let Some(caps) = GEM_RE.captures(line) {
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();

                if name.is_empty() {
                    continue;
                }

                // 最大 3 個までのバージョン制約を回収する
                let mut version_parts = Vec::new();
                for i in 2..=4 {
                    if let Some(v) = caps.get(i) {
                        version_parts.push(v.as_str().to_string());
                    }
                }

                // git / path / source ブロックの内側は RubyGems のレジストリ依存ではない
                if in_non_registry_block(&block_stack) {
                    continue;
                }

                // バージョン指定から `VersionSpec` を組み立てる
                let spec = if version_parts.is_empty() {
                    // バージョン指定のない git / path / private source は
                    // RubyGems のレジストリ依存ではないため除外する。
                    // version が明示されている場合は Bundler の gemspec 検証用制約として扱える。
                    if has_non_registry_source(code, &custom_git_sources) {
                        continue;
                    }
                    // 引数が次行へ続く宣言 (`gem "devise",` で行が終わる形) は、
                    // 実際には次行に version や `git:` がある。この行だけを見て
                    // 「バージョンなしのレジストリ依存」と報告すると、書き込み側は
                    // 挿入位置を見つけられず必ず失敗する。安全側で取りこぼす。
                    if code.trim_end().ends_with(',') {
                        continue;
                    }
                    // バージョン指定がなければ `Any`
                    VersionSpec::new(VersionSpecKind::Any, "", "")
                } else {
                    let version_str = version_parts.join(", ");
                    match parser.parse(&version_str) {
                        Some(s) => s,
                        None => continue,
                    }
                };

                let dep = if block_stack
                    .iter()
                    .any(|block| matches!(block, GemfileBlock::Group(true)))
                    || has_dev_group_option(code)
                {
                    Dependency::development(name, spec, Language::Ruby)
                } else {
                    Dependency::production(name, spec, Language::Ruby)
                };
                dependencies.push(dep);
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Ruby
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parser = get_parser(Language::Ruby);
        let escaped_name = regex::escape(package);
        let mut updated = false;
        // 末尾コンテキストは 行末/コメント/`)`、オプション (`, require:` 等) に加え、
        // 行末条件修飾子 (`gem 'wdm' if Gem.win_platform?`) も許容する。これがないと
        // parse は versionless (Any=更新可能) として拾うのに update で挿入先が見つからず
        // report/apply が矛盾する (GEM_RE 側は既に if/unless を許容済み)。
        // if/unless の直前までを一致させ、修飾子本体は line[matched_range.end..] で保持する。
        let no_version_pattern = format!(
            r#"(gem(?:\s+|\s*\(\s*))(['"])({escaped_name})(['"])(\s*(?:(?:\)\s*)?(?:#|$)|,\s*(?::\w+\s*=>|\w+\s*:)|(?:\)\s*)?(?:if|unless)\b))"#
        );

        let no_version_re =
            Regex::new(&no_version_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Gemfile"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let simple_pattern =
            format!(r#"(gem(?:\s+|\s*\(\s*))(['"])({escaped_name})(['"])(\s*\)?\s*)$"#);

        let simple_re =
            Regex::new(&simple_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Gemfile"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let mut lines = Vec::new();
        let mut block_stack = Vec::new();
        // parse と同じ集合を使い、独自 git ソース (`git_source(:stash)`) を持つ gem を
        // レジストリ依存として書き換えないようにする
        let custom_git_sources = collect_git_source_keys(content);

        for raw_line in content.split_inclusive('\n') {
            // 行末の改行コード (`\n` / `\r\n`) を退避し、本文のみを処理対象にする
            let (line, line_ending) = split_line_ending(raw_line);

            let trimmed = line.trim();
            let is_blank_or_comment = trimmed.is_empty() || trimmed.starts_with('#');
            // ブロック判定とオプション判定はコメント除去後の文字列で行う
            // (行末コメントの `do` や `git:` をブロック開始・git 依存と誤認しないため)
            let code = if is_blank_or_comment {
                ""
            } else {
                strip_line_comment(trimmed)
            };

            // parse と同じ規則でブロックを追跡する。追跡がないと、parse が除外した
            // `path "../mygem" do` ブロック内の gem まで名前一致だけで書き換えてしまい、
            // ローカル依存へ rubygems.org の版を注入する。
            if !is_blank_or_comment && track_block_structure(code, &mut block_stack) {
                lines.push(raw_line.to_string());
                continue;
            }

            // 同名 gem が複数箇所 (group 内外など) に宣言されている場合は全出現を更新する。
            // Cargo / Gradle / pyproject と同じく「1 依存 = 全出現を書き換え」の不変条件を守る。
            if let Some(caps) = GEM_RE.captures(line)
                && caps.get(1).map(|m| m.as_str()) == Some(package)
            {
                // レジストリ外ブロックの内側は parse が依存として surface しないので、
                // writer 側も触らない (触ると git / path 依存へ rubygems の版が入る)
                if in_non_registry_block(&block_stack) {
                    lines.push(raw_line.to_string());
                    continue;
                }

                let mut version_parts = Vec::new();
                for i in 2..=4 {
                    if let Some(m) = caps.get(i) {
                        version_parts.push(m);
                    }
                }

                match version_parts.len() {
                    0 => {
                        // parse と同じく、バージョンなしの非レジストリ依存だけを除外する。
                        if has_non_registry_source(code, &custom_git_sources) {
                            lines.push(raw_line.to_string());
                            continue;
                        }

                        if let Some(caps) = no_version_re.captures(line) {
                            let gem_keyword = &caps[1];
                            let quote_start = &caps[2];
                            let name = &caps[3];
                            let quote_end = &caps[4];
                            let suffix = &caps[5];
                            let matched_range = caps.get(0).unwrap().range();
                            updated = true;
                            let inserted = format!(
                                "{}{}{}{}, {}{}{}{}",
                                gem_keyword,
                                quote_start,
                                name,
                                quote_end,
                                quote_start,
                                new_version,
                                quote_end,
                                suffix
                            );
                            let mut updated_line =
                                String::with_capacity(raw_line.len() + inserted.len() + 8);
                            updated_line.push_str(&line[..matched_range.start]);
                            updated_line.push_str(&inserted);
                            updated_line.push_str(&line[matched_range.end..]);
                            updated_line.push_str(line_ending);
                            lines.push(updated_line);
                            continue;
                        }

                        if let Some(caps) = simple_re.captures(line) {
                            let gem_keyword = &caps[1];
                            let quote_start = &caps[2];
                            let name = &caps[3];
                            let quote_end = &caps[4];
                            let trailing = &caps[5];
                            updated = true;
                            lines.push(format!(
                                "{}{}{}{}, {}{}{}{}{}",
                                gem_keyword,
                                quote_start,
                                name,
                                quote_end,
                                quote_start,
                                new_version,
                                quote_end,
                                trailing,
                                line_ending
                            ));
                            continue;
                        }
                    }
                    // 単一制約も複合制約 (`gem "pg", ">= 0.18", "< 2.0"`) も同じ経路で扱う。
                    // parse は複数引数を `", "` で 1 本に繋いで judge へ渡すので、writer も
                    // 同じ文字列を組み立てて更新し、結果を元の引数へ配り直す。片方だけ
                    // 「複合は書き換え不可」にすると judge が Update を報告した更新を
                    // writer が必ず失敗させる (report/apply の矛盾) 。
                    _ => {
                        let original_parts: Vec<&str> =
                            version_parts.iter().map(|part| part.as_str()).collect();
                        let joined = original_parts.join(", ");

                        if let Some(spec) = parser.parse(&joined) {
                            // judge も同じ `try_format_updated` で更新可否を決めているため、
                            // ここが None になるのは judge が Skip した制約だけ
                            // (`!= 2.2.4` などの除外制約)
                            let Some(formatted) = spec.try_format_updated(new_version) else {
                                return Err(ManifestError::InvalidVersionSpec {
                                    path: PathBuf::from("Gemfile"),
                                    spec: package.to_string(),
                                    message: "この制約は安全に書き換えられません".to_string(),
                                });
                            };

                            let Some(new_parts) =
                                split_updated_constraint(&formatted, &original_parts)
                            else {
                                return Err(ManifestError::InvalidVersionSpec {
                                    path: PathBuf::from("Gemfile"),
                                    spec: package.to_string(),
                                    message: "複合バージョン制約は安全に書き換えられません"
                                        .to_string(),
                                });
                            };

                            // クォート種別・引数間の空白・括弧・行末修飾子・コメントを保つため、
                            // 各引数のクォート内側だけを差し替える
                            let mut updated_line =
                                String::with_capacity(raw_line.len() + formatted.len());
                            let mut cursor = 0;
                            for (part, new_part) in version_parts.iter().zip(new_parts.iter()) {
                                updated_line.push_str(&line[cursor..part.start()]);
                                updated_line.push_str(new_part);
                                cursor = part.end();
                            }
                            updated_line.push_str(&line[cursor..]);
                            updated_line.push_str(line_ending);
                            updated = true;
                            lines.push(updated_line);
                            continue;
                        }
                    }
                }
            }

            lines.push(raw_line.to_string());
        }

        if updated {
            // 各行が元の改行コード (`\n` / `\r\n`) を保持したまま連結する
            return Ok(lines.concat());
        }

        Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Gemfile"),
            spec: package.to_string(),
            message: "gem not found or version could not be updated".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        GemfileParser.parse(content)
    }

    #[test]
    fn test_parse_simple_gem() {
        let content = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "7.0");
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_parenthesized_gem() {
        let content = r#"gem("rack", "~> 3.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rack");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "3.0");
    }

    #[test]
    fn test_parse_exact_version() {
        let content = r#"gem 'bcrypt', '3.1.7'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "bcrypt");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_pessimistic_constraint() {
        let content = r#"gem 'puma', '~> 5.0'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "5.0");
    }

    #[test]
    fn test_parse_compound_constraints() {
        let content = r#"gem 'pg', '>= 0.18', '< 2.0'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pg");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_parse_gem_with_trailing_conditional_modifier() {
        // 行末条件修飾子 (`if`/`unless`) が付いていても version 制約を取りこぼさない。
        // Rails Gemfile で頻出する `gem 'wdm', '>= 0.1.0' if Gem.win_platform?` パターン。
        let content = r#"
gem 'wdm', '>= 0.1.0' if Gem.win_platform?
gem 'tzinfo-data', '~> 1.2' unless Gem.win_platform?
gem("sqlite3", "~> 1.4") if Gem.win_platform?
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        assert_eq!(deps[0].name, "wdm");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GreaterOrEqual);
        assert_eq!(deps[0].version_spec.version, "0.1.0");

        assert_eq!(deps[1].name, "tzinfo-data");
        assert_eq!(deps[1].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[1].version_spec.version, "1.2");

        assert_eq!(deps[2].name, "sqlite3");
        assert_eq!(deps[2].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[2].version_spec.version, "1.4");
    }

    #[test]
    fn test_parse_no_version() {
        let content = r#"gem 'some_gem'"#;
        let deps = parse(content).unwrap();
        // バージョンなしの gem は `Any` として扱う
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "some_gem");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_multiple_no_version() {
        let content = r#"
gem 'rmagick'
gem 'nokogiri'
gem 'playwright-ruby-client', '1.57.1'
gem 'websocket-driver'
gem 'rtesseract'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        // バージョンなしの gem を確認する
        assert_eq!(deps[0].name, "rmagick");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);

        assert_eq!(deps[1].name, "nokogiri");
        assert_eq!(deps[1].version_spec.kind, VersionSpecKind::Any);

        // バージョン付きの gem を確認する
        assert_eq!(deps[2].name, "playwright-ruby-client");
        assert_eq!(deps[2].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[2].version_spec.version, "1.57.1");

        assert_eq!(deps[3].name, "websocket-driver");
        assert_eq!(deps[3].version_spec.kind, VersionSpecKind::Any);

        assert_eq!(deps[4].name, "rtesseract");
        assert_eq!(deps[4].version_spec.kind, VersionSpecKind::Any);
    }

    #[test]
    fn test_parse_double_quotes() {
        let content = r#"gem "rails", "~> 7.0""#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
    }

    #[test]
    fn test_parse_development_group() {
        let content = r#"
group :development do
  gem 'web-console', '>= 4.1.0'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "web-console");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_development_test_group() {
        let content = r#"
group :development, :test do
  gem 'rspec-rails', '~> 5.0'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_nested_dev_group_does_not_leak() {
        let content = r#"
group :production do
  group :development do
    gem 'rubocop', '~> 1.0'
  end
  gem 'pg', '~> 1.1'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let rubocop = deps.iter().find(|dep| dep.name == "rubocop").unwrap();
        let pg = deps.iter().find(|dep| dep.name == "pg").unwrap();

        assert!(rubocop.is_dev);
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_mixed_groups() {
        let content = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem 'pg', '~> 1.1'

group :development, :test do
  gem 'rspec-rails', '~> 5.0'
  gem 'factory_bot_rails', '~> 6.0'
end

group :development do
  gem 'web-console', '>= 4.1.0'
end

gem 'bcrypt', '~> 3.1.7'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 6);

        let prod_deps: Vec<_> = deps.iter().filter(|d| !d.is_dev).collect();
        let dev_deps: Vec<_> = deps.iter().filter(|d| d.is_dev).collect();

        assert_eq!(prod_deps.len(), 3); // rails, pg, bcrypt の 3 件
        assert_eq!(dev_deps.len(), 3); // 開発系 3 件
    }

    #[test]
    fn test_parse_with_comments() {
        let content = r#"
# コメント
gem 'rails', '~> 7.0' # inline comment
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_parse_empty() {
        let deps = parse("").unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_source_only() {
        let content = r#"source 'https://rubygems.org'"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_update_version() {
        let content = r#"
source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem 'pg', '~> 1.1'
"#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        // `~> 7.0` は `>= 7.0, < 8.0`。セグメント数を保って `~> 7.1` にする
        // (`~> 7.1.0` にすると上限が `< 7.2` へ縮まってしまう)
        assert!(result.contains("'~> 7.1'"));
        assert!(result.contains("gem 'pg'")); // 他の gem は変更しない
    }

    #[test]
    fn test_update_version_exact() {
        let content = r#"gem 'bcrypt', '3.1.7'"#;
        let result = GemfileParser
            .update_version(content, "bcrypt", "3.1.18")
            .unwrap();
        assert!(result.contains("'3.1.18'"));
    }

    #[test]
    fn test_update_version_maintains_format() {
        let content = r#"gem 'puma', '>= 5.0'"#;
        let result = GemfileParser
            .update_version(content, "puma", "6.0")
            .unwrap();
        assert!(result.contains("'>= 6.0'"));
    }

    #[test]
    fn test_update_version_gem_with_conditional_modifier() {
        // 行末条件修飾子付きの gem も version だけを更新し、修飾子を保持する
        let content = r#"gem 'wdm', '>= 0.1.0' if Gem.win_platform?"#;
        let result = GemfileParser
            .update_version(content, "wdm", "0.2.0")
            .unwrap();
        assert!(result.contains("'>= 0.2.0'"));
        // 修飾子は保持される
        assert!(result.contains("if Gem.win_platform?"));
    }

    #[test]
    fn test_update_version_versionless_gem_with_conditional_modifier() {
        // 回帰: バージョンなし gem + 行末条件修飾子 (`if`/`unless`)。
        // parse は versionless (Any=更新可能) として拾うため judge が更新候補にするが、
        // 以前は update_version の挿入パターンが ` if`/` unless` を許容せず書き込みに失敗し、
        // 「更新あり」と報告した後に適用できず矛盾していた。
        let content = "gem 'wdm' if Gem.win_platform?\n";
        // parse は更新可能な versionless dep として拾う (report/apply 整合の前提)
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "wdm");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);
        // update も成功し、version 挿入と修飾子保持が両立する
        let result = GemfileParser
            .update_version(content, "wdm", "0.2.0")
            .unwrap();
        assert!(result.contains("'wdm'"));
        assert!(result.contains("'0.2.0'"));
        assert!(result.contains("if Gem.win_platform?"));
    }

    #[test]
    fn test_update_version_versionless_gem_with_unless_modifier() {
        // `unless` 修飾子・ダブルクォート・括弧付き呼び出しでも同様に成立すること
        let content = "gem \"wdm\" unless RUBY_PLATFORM =~ /mingw/\n";
        let result = GemfileParser
            .update_version(content, "wdm", "0.2.0")
            .unwrap();
        assert!(result.contains("\"wdm\""));
        assert!(result.contains("\"0.2.0\""));
        assert!(result.contains("unless RUBY_PLATFORM"));
    }

    #[test]
    fn test_update_version_double_quotes() {
        let content = r#"gem "rails", "~> 7.0""#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert!(result.contains("\"~> 7.1\""));
    }

    #[test]
    fn test_update_version_parenthesized_gem() {
        let content = r#"gem("rack", "~> 3.0")"#;
        let result = GemfileParser
            .update_version(content, "rack", "3.1.0")
            .unwrap();
        assert_eq!(result, r#"gem("rack", "~> 3.1")"#);
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"gem 'rails', '~> 7.0'"#;
        let result = GemfileParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_gemfile_parser_language() {
        let parser = GemfileParser;
        assert_eq!(parser.language(), Language::Ruby);
    }

    #[test]
    fn test_parse_gem_with_options() {
        let content = r#"gem 'rails', '~> 7.0', require: false"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.version, "7.0");
    }

    #[test]
    fn test_parse_gem_with_git_source() {
        // git ソースでもバージョンがあれば解釈する
        let content = r#"gem 'rails', '~> 7.0', git: 'https://github.com/rails/rails'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[test]
    fn test_update_versioned_gem_with_git_source() {
        let content = r#"gem 'rails', '~> 7.0', git: 'https://github.com/rails/rails'"#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();

        assert_eq!(
            result,
            r#"gem 'rails', '~> 7.1', git: 'https://github.com/rails/rails'"#
        );
    }

    #[test]
    fn test_parse_unversioned_gem_with_git_source_skipped() {
        let content = r#"
gem 'rails', git: 'https://github.com/rails/rails'
gem 'pg', '~> 1.5'
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pg");
    }

    /// Bundler が組み込みで登録する git source ショートハンド 4 種はすべて
    /// レジストリ外依存として扱う。`gitlab:` を取りこぼすと GitLab の git 依存へ
    /// rubygems.org 側の同名 gem の版が書き込まれ `bundle install` が壊れる
    #[test]
    fn test_parse_skips_all_builtin_git_source_shorthands() {
        for shorthand in ["github", "gitlab", "bitbucket", "gist"] {
            let content = format!("gem 'foo', {shorthand}: 'group/foo'\ngem 'pg', '~> 1.5'\n");
            let deps = parse(&content).unwrap();
            assert_eq!(
                deps.len(),
                1,
                "{shorthand}: レジストリ外依存を surface してはいけない ({deps:?})"
            );
            assert_eq!(deps[0].name, "pg", "{shorthand}");
        }
    }

    /// ブロック形式の `gitlab ... do` 内の gem も更新対象にしない
    #[test]
    fn test_parse_skips_gems_inside_gitlab_block() {
        let content = r#"
gitlab 'group/monorepo' do
  gem 'inner-gem'
end
gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1, "{deps:?}");
        assert_eq!(deps[0].name, "pg");
    }

    #[test]
    fn test_parse_gte_constraint() {
        let content = r#"gem 'web-console', '>= 4.1.0'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GreaterOrEqual);
    }

    #[test]
    fn test_format_updated_maintains_prefix() {
        let content = r#"gem 'rails', '~> 7.0'"#;
        let result = GemfileParser
            .update_version(content, "rails", "7.1")
            .unwrap();
        assert!(result.contains("'~> 7.1'"));
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem() {
        let content = r#"gem 'rmagick'"#;
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_double_quotes() {
        let content = r#"gem "nokogiri""#;
        let result = GemfileParser
            .update_version(content, "nokogiri", "1.16.0")
            .unwrap();
        assert!(result.contains("gem \"nokogiri\", \"1.16.0\""));
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_with_options() {
        let content = r#"gem 'my_gem', require: false"#;
        let result = GemfileParser
            .update_version(content, "my_gem", "1.0.0")
            .unwrap();
        assert!(result.contains("gem 'my_gem', '1.0.0', require: false"));
    }

    #[test]
    fn test_update_version_add_to_parenthesized_unversioned_gem() {
        let content = r#"gem("my_gem", require: false)"#;
        let result = GemfileParser
            .update_version(content, "my_gem", "1.0.0")
            .unwrap();
        assert_eq!(result, r#"gem("my_gem", "1.0.0", require: false)"#);
    }

    #[test]
    fn test_update_version_add_to_parenthesized_unversioned_gem_with_comment() {
        let content = r#"gem("my_gem") # comment"#;
        let result = GemfileParser
            .update_version(content, "my_gem", "1.0.0")
            .unwrap();
        assert_eq!(result, r#"gem("my_gem", "1.0.0") # comment"#);
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_in_multiline() {
        let content = r#"
gem 'rmagick'
gem 'nokogiri'
gem 'playwright-ruby-client', '1.57.1'
"#;
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
        // 他の gem は変更しない
        assert!(result.contains("gem 'nokogiri'"));
        assert!(result.contains("gem 'playwright-ruby-client', '1.57.1'"));
    }

    #[test]
    fn test_update_versionless_gem_preserves_trailing_newline() {
        // バージョンなし gem の更新で末尾改行を保持する
        let content = "gem 'rmagick'\ngem 'nokogiri'\n";
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_update_versionless_gem_no_trailing_newline() {
        // 末尾改行がないファイルは付けない
        let content = "gem 'rmagick'";
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result.contains("gem 'rmagick', '5.3.0'"));
        assert!(!result.ends_with('\n'));
    }

    #[test]
    fn test_update_version_mixed_versioned_and_unversioned() {
        let content = r#"
gem 'rmagick'
gem 'rails', '~> 7.0'
gem 'nokogiri'
"#;
        // バージョン付き gem の更新
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert!(result.contains("'~> 7.1'"));

        // バージョンなし gem の更新
        let result2 = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert!(result2.contains("gem 'rmagick', '5.3.0'"));
    }

    #[test]
    fn test_update_version_not_equal_constraint_returns_err() {
        let content = r#"gem 'rack', '!= 2.2.4'"#;
        let result = GemfileParser.update_version(content, "rack", "3.0.0");
        assert!(result.is_err());
    }

    /// 回帰テスト: 複数引数の複合制約 (`gem "rails", ">= 6.0", "< 8.0"`) を
    /// judge が Update と判定したら writer も書き込めること。
    ///
    /// 以前は writer が「引数が 2 個以上なら無条件でエラー」としていたため、
    /// Rails 標準の Gemfile で「1 updated」と表示しながら exit code 2 で
    /// 1 バイトも書き換えられない report/apply 矛盾が起きていた。
    #[test]
    fn test_update_version_compound_constraint_round_trip() {
        let content = "gem \"rails\", \">= 6.0\", \"< 8.0\"\n";

        // parse は複数引数を 1 本の Range 制約として解釈する
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, ">= 6.0, < 8.0");

        // judge が更新可否に使う書式化も成功する (= Update が報告される)
        assert_eq!(
            deps[0]
                .version_spec
                .try_format_updated("7.2.3.2")
                .as_deref(),
            Some(">= 7.2.3.2, < 8.0")
        );

        // writer は元の引数の個数・順序・クォート種別を保って書き戻す
        let result = GemfileParser
            .update_version(content, "rails", "7.2.3.2")
            .unwrap();
        assert_eq!(result, "gem \"rails\", \">= 7.2.3.2\", \"< 8.0\"\n");
    }

    /// シングルクォート・3 引数・上限が先に書かれた並びでも同様に書き戻せること
    #[test]
    fn test_update_version_compound_constraint_variants() {
        for (content, expected) in [
            // シングルクォート 2 引数
            (
                "gem 'pg', '>= 0.18', '< 2.0'\n",
                "gem 'pg', '>= 1.5.0', '< 2.0'\n",
            ),
            // 3 引数 (書き換えるのは包含下限だけ)
            (
                "gem 'pg', '>= 0.18', '<= 1.9', '< 2.0'\n",
                "gem 'pg', '>= 1.5.0', '<= 1.9', '< 2.0'\n",
            ),
            // 上限が先に書かれていても下限側だけを進める
            (
                "gem 'pg', '< 2.0', '>= 0.18'\n",
                "gem 'pg', '< 2.0', '>= 1.5.0'\n",
            ),
            // 括弧付き呼び出し
            (
                "gem(\"pg\", \">= 0.18\", \"< 2.0\")\n",
                "gem(\"pg\", \">= 1.5.0\", \"< 2.0\")\n",
            ),
            // 行末条件修飾子
            (
                "gem 'pg', '>= 0.18', '< 2.0' if ENV['DB']\n",
                "gem 'pg', '>= 1.5.0', '< 2.0' if ENV['DB']\n",
            ),
            // 行末コメント
            (
                "gem 'pg', '>= 0.18', '< 2.0' # database\n",
                "gem 'pg', '>= 1.5.0', '< 2.0' # database\n",
            ),
            // CRLF は保持する
            (
                "gem 'pg', '>= 0.18', '< 2.0'\r\n",
                "gem 'pg', '>= 1.5.0', '< 2.0'\r\n",
            ),
            // 引数間の空白 (詰めた書き方) も保持する
            (
                "gem 'pg','>= 0.18','< 2.0'\n",
                "gem 'pg','>= 1.5.0','< 2.0'\n",
            ),
        ] {
            let result = GemfileParser
                .update_version(content, "pg", "1.5.0")
                .unwrap_or_else(|e| panic!("{content:?}: {e}"));
            assert_eq!(result, expected, "input={content:?}");
        }
    }

    /// 回帰テスト: 元の引数自身がカンマを含んでいても、引数ごとのカンマ数で
    /// トークンを配り直して書き戻せる。
    ///
    /// カンマ数だけで機械的に割っていたときは要素数が合わずエラーになり、
    /// judge が Update を報告した更新を writer が必ず失敗させていた。
    #[test]
    fn test_update_version_compound_constraint_with_comma_inside_argument() {
        let content = "gem 'pg', '>= 0.18, < 3.0', '<= 2.0'\n";
        let result = GemfileParser
            .update_version(content, "pg", "1.5.0")
            .unwrap();
        assert_eq!(result, "gem 'pg', '>= 1.5.0, < 3.0', '<= 2.0'\n");
    }

    /// カンマ分割の要素数が元の引数から期待される数と一致しない場合は、どの引数へ
    /// 何を書くか決められないため安全側で `None` を返す (誤書き込みを防ぐ)。
    #[test]
    fn test_split_updated_constraint_token_count_mismatch() {
        // 元は 3 トークン (2 + 1) を期待するのに 2 トークンしか無い
        assert_eq!(
            split_updated_constraint(">= 1.5.0, < 3.0", &[">= 0.18, < 3.0", "<= 2.0"]),
            None
        );
        // 期待どおりなら引数ごとに配り直す
        assert_eq!(
            split_updated_constraint(">= 1.5.0, < 3.0, <= 2.0", &[">= 0.18, < 3.0", "<= 2.0"]),
            Some(vec![">= 1.5.0, < 3.0".to_string(), "<= 2.0".to_string()])
        );
        // 単一引数はカンマを含んでもそのまま 1 本で返す
        assert_eq!(
            split_updated_constraint(">= 1.5.0, < 2.0", &[">= 0.18, < 2.0"]),
            Some(vec![">= 1.5.0, < 2.0".to_string()])
        );
    }

    /// 除外制約 (`!=`) を含む複合制約は judge が Skip するため writer には来ないが、
    /// 来た場合も従来どおりエラーにする。
    #[test]
    fn test_update_version_compound_constraint_with_not_equal_returns_err() {
        let content = "gem 'pg', '>= 0.18', '!= 1.2.0', '< 2.0'\n";
        let result = GemfileParser.update_version(content, "pg", "1.5.0");
        assert!(result.is_err(), "{result:?}");
    }

    /// 単一引数に書かれた複合制約 (`'>= 0.18, < 2.0'`) も同じ経路で書き戻せること。
    /// judge は複数引数と同じ Range として扱うため、writer だけ拒否すると矛盾する。
    #[test]
    fn test_update_version_single_argument_compound_constraint() {
        let content = "gem 'pg', '>= 0.18, < 2.0'\n";
        let result = GemfileParser
            .update_version(content, "pg", "1.5.0")
            .unwrap();
        assert_eq!(result, "gem 'pg', '>= 1.5.0, < 2.0'\n");
    }

    // --- 追加エッジケーステスト ---

    #[test]
    fn test_parse_four_segment_version() {
        // 4セグメントバージョン（例: loofah のパッチリリース）
        let content = r#"gem 'loofah', '2.22.0.1'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "loofah");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "2.22.0.1");
    }

    #[test]
    fn test_parse_prerelease_gem() {
        // プレリリースバージョンの gem をパースできること
        let content = r#"gem 'rails', '7.0.0.alpha'"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.version, "7.0.0.alpha");
    }

    #[test]
    fn test_parse_double_quoted_pessimistic() {
        // ダブルクォート + ペシミスティック制約の組み合わせ
        let content = r#"gem "nokogiri", "~> 1.15""#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "nokogiri");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "1.15");
    }

    #[test]
    fn test_parse_gem_with_require_false_option() {
        // require: false オプション付きの gem がバージョン制約だけ解釈されること
        let content = r#"gem 'webpacker', '~> 5.0', require: false"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "webpacker");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "5.0");
    }

    #[test]
    fn test_parse_multiple_groups_on_one_line() {
        // 1行に複数グループを指定した group ブロック
        let content = r#"
group :development, :test do
  gem 'pry', '~> 0.14'
  gem 'faker', '~> 3.0'
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let pry = deps.iter().find(|d| d.name == "pry").unwrap();
        let faker = deps.iter().find(|d| d.name == "faker").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        // development, test グループ内の gem は開発依存
        assert!(pry.is_dev);
        assert!(faker.is_dev);
        // グループ外の gem は本番依存
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_nested_do_end_in_group() {
        // group 内の platforms do...end がグループスタックを壊さないこと
        let content = r#"
group :development do
  platforms :jruby do
    gem 'activerecord-jdbc-adapter', '~> 1.3'
  end
  gem 'web-console', '>= 4.1.0'
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let jdbc = deps
            .iter()
            .find(|d| d.name == "activerecord-jdbc-adapter")
            .unwrap();
        let console = deps.iter().find(|d| d.name == "web-console").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        // platforms ブロック内の gem も group :development 内なので開発依存
        assert!(jdbc.is_dev);
        // platforms の end でグループが外れてはいけない
        assert!(console.is_dev);
        // グループ外は本番依存
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_source_do_end_in_group() {
        // group 内の source do...end がグループスタックを壊さないこと。
        // source ブロック内の gem は rubygems.org の依存ではないため除外し、
        // ブロックを抜けた後の gem は従来どおり開発依存として拾う。
        let content = r#"
group :development do
  source 'https://gems.example.com' do
    gem 'private-gem', '~> 1.0'
  end
  gem 'debug', '~> 1.0'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);

        let debug = deps.iter().find(|d| d.name == "debug").unwrap();
        assert!(debug.is_dev);
    }

    /// 回帰テスト: git / path / source ブロック内の gem に rubygems.org の版を
    /// 書き込むと `bundle install` が壊れる (private source なら同名の公開 gem の
    /// 版が入る)。行オプション形式と同じく除外する。
    #[test]
    fn test_parse_non_registry_blocks_excluded() {
        let content = r#"
git "https://github.com/rails/rails.git", branch: "main" do
  gem "activesupport"
  gem "actionpack"
end

path "components" do
  gem "admin_ui"
end

source "https://gems.example.com" do
  gem "private-gem"
end

gem "rails", "~> 7.0"
"#;
        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["rails"]);
    }

    /// `platforms` / `install_if` のような通常のブロックは従来どおり対象にする。
    #[test]
    fn test_parse_platforms_block_still_included() {
        let content = r#"
platforms :ruby do
  gem 'pg', '~> 1.5'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pg");
    }

    /// 回帰テスト: hash-rocket 記法 (`:git => '...'`) も非レジストリ依存として除外する。
    /// 以前は `git:` 綴りしか見ておらず、git 依存を更新候補として誤って報告した上で
    /// 書き込み時に必ずエラーになっていた。
    #[test]
    fn test_parse_hash_rocket_git_option_excluded() {
        let content = r#"
gem 'rails', :git => 'https://github.com/rails/rails.git'
gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["pg"]);
    }

    /// hash-rocket の無害なオプション付きバージョンなし gem には version を挿入できる。
    #[test]
    fn test_update_version_add_to_unversioned_gem_with_hash_rocket_option() {
        let content = "gem 'sidekiq', :require => false\n";
        let result = GemfileParser
            .update_version(content, "sidekiq", "7.3.0")
            .unwrap();
        assert_eq!(result, "gem 'sidekiq', '7.3.0', :require => false\n");
    }

    /// `git_source` で定義した独自ショートハンド付きの gem にも挿入できる
    /// (キーワード列挙ではなく `key:` の一般形で判定するため)。
    #[test]
    fn test_update_version_add_to_unversioned_gem_with_custom_shorthand() {
        let content = "gem 'my-gem', codeberg: 'user/my-gem'\n";
        let result = GemfileParser
            .update_version(content, "my-gem", "2.0.0")
            .unwrap();
        assert_eq!(result, "gem 'my-gem', '2.0.0', codeberg: 'user/my-gem'\n");
    }

    /// 回帰テスト: 引数が次行へ続く宣言はこの行だけでは版を決められないため、
    /// 「バージョンなしのレジストリ依存」と誤報告せず取りこぼす
    /// (以前は更新を報告した上で書き込みが必ず失敗していた)。
    #[test]
    fn test_parse_multiline_gem_declaration_skipped() {
        let content = r#"
gem "sidekiq",
    "~> 7.2"

gem "devise",
    git: "https://github.com/heartcombo/devise.git",
    branch: "main"

gem "pg", "~> 1.5"
"#;
        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["pg"]);
    }

    /// 同一行に version がある場合は、末尾に `,` が続いても従来どおり更新対象。
    #[test]
    fn test_parse_versioned_gem_with_trailing_comma_still_parsed() {
        let content = "gem \"sidekiq\", \"~> 7.2\",\n    require: false\n";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "sidekiq");
        assert_eq!(deps[0].version_spec.version, "7.2");
    }

    #[test]
    fn test_parse_group_inside_block_does_not_leak() {
        // ブロックの内側で閉じた group が、後続の gem に漏れないこと
        let content = r#"
platforms :ruby do
  group :development do
    gem 'debug', '~> 1.0'
  end
  gem 'pg', '~> 1.5'
end
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let debug = deps.iter().find(|d| d.name == "debug").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        assert!(debug.is_dev);
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_single_line_keyword_block_does_not_shift_group_end() {
        let content = r#"
group :development do
  if RUBY_VERSION.start_with?('3') then puts 'supported' end
end
gem 'rails', '~> 7.0'
"#;
        let deps = parse(content).unwrap();

        let rails = deps
            .iter()
            .find(|dependency| dependency.name == "rails")
            .unwrap();
        assert!(!rails.is_dev);
    }

    #[test]
    fn test_parse_keyword_block_ignores_end_inside_string() {
        let content = r#"
group :development do
  if ENV['MODE'] == 'frontend'
    puts 'development'
  end
end
gem 'rails', '~> 7.0'
"#;
        let deps = parse(content).unwrap();

        let rails = deps
            .iter()
            .find(|dependency| dependency.name == "rails")
            .unwrap();
        assert!(!rails.is_dev);
    }

    #[test]
    fn test_parse_keyword_block_ignores_end_inside_unicode_identifier() {
        let content = r#"
group :development do
  if éend
    puts 'development'
  end
end
gem 'rails', '~> 7.0'
"#;
        let deps = parse(content).unwrap();

        let rails = deps
            .iter()
            .find(|dependency| dependency.name == "rails")
            .unwrap();
        assert!(!rails.is_dev);
    }

    #[test]
    fn test_parse_group_with_inline_comment() {
        // 回帰テスト: `group :development do # comment` のように
        // インラインコメントが付いていても開発グループとして認識されること
        let content = r#"
group :development do # security gems
  gem 'brakeman', '~> 6.0'
end

group :test, :development do  # bundler comment
  gem 'rspec', '~> 3.13'
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let brakeman = deps.iter().find(|d| d.name == "brakeman").unwrap();
        let rspec = deps.iter().find(|d| d.name == "rspec").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        // インラインコメント付きの group も開発依存と認識されること
        assert!(
            brakeman.is_dev,
            "brakeman should be dev (group has inline comment)"
        );
        assert!(
            rspec.is_dev,
            "rspec should be dev (group has trailing space + comment)"
        );
        // グループ外の gem は本番依存
        assert!(!pg.is_dev);
    }

    #[test]
    fn test_parse_gem_group_option_as_dev() {
        let content = r#"
gem 'rspec', '~> 3.0', group: :test
gem 'rubocop', '~> 1.0', groups: [:development, :test]
gem 'rails', '~> 7.0'
"#;

        let deps = parse(content).unwrap();
        let rspec = deps.iter().find(|d| d.name == "rspec").unwrap();
        let rubocop = deps.iter().find(|d| d.name == "rubocop").unwrap();
        let rails = deps.iter().find(|d| d.name == "rails").unwrap();
        assert!(rspec.is_dev);
        assert!(rubocop.is_dev);
        assert!(!rails.is_dev);
    }

    #[test]
    fn test_update_unversioned_git_source_is_skipped() {
        let content = r#"gem 'rails', git: 'https://github.com/rails/rails'"#;
        let result = GemfileParser.update_version(content, "rails", "7.1.0");
        assert!(result.is_err());
    }

    // --- 行末コメントの誤認に対する回帰テスト ---

    #[test]
    fn test_parse_gem_with_comment_ending_in_do() {
        // 回帰テスト: 行末コメントが "do" で終わってもブロック開始と誤認せず、
        // 通常の gem として解析されること
        let content = r#"
gem 'debug' # things to do
gem 'rails', '~> 7.0' # more things to do
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "debug");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);
        assert_eq!(deps[1].name, "rails");
        assert_eq!(deps[1].version_spec.version, "7.0");
    }

    #[test]
    fn test_update_version_gem_with_comment_ending_in_do() {
        // 回帰テスト: "do" で終わるコメント付きの gem 行も更新できること
        let content = "gem 'rails', '~> 7.0' # more things to do\n";
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert_eq!(result, "gem 'rails', '~> 7.1' # more things to do\n");
    }

    #[test]
    fn test_parse_comment_ending_in_do_keeps_block_stack() {
        // 回帰テスト: コメントの "do" でブロックスタックがずれて、
        // group 終端後の gem が開発依存と誤判定されないこと
        let content = r#"
group :development do
  gem 'debug' # things to do
end

gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let debug = deps.iter().find(|d| d.name == "debug").unwrap();
        let pg = deps.iter().find(|d| d.name == "pg").unwrap();

        assert!(debug.is_dev);
        assert!(!pg.is_dev, "pg should not leak into the development group");
    }

    #[test]
    fn test_parse_gem_with_git_mention_in_comment() {
        // 回帰テスト: コメント中の `git:` 文言で非レジストリ依存と誤認しないこと
        let content = "gem 'foo' # migrate to git: later\n";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "foo");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);
    }

    #[test]
    fn test_update_version_gem_with_git_mention_in_comment() {
        // 回帰テスト: コメント中の `git:` 文言があってもバージョンを挿入できること
        let content = "gem 'foo' # migrate to git: later\n";
        let result = GemfileParser
            .update_version(content, "foo", "1.2.0")
            .unwrap();
        assert_eq!(result, "gem 'foo', '1.2.0' # migrate to git: later\n");
    }

    #[test]
    fn test_parse_quoted_hash_is_not_comment() {
        // 文字列リテラル内の `#` はコメント扱いされないこと。
        // `#fragment` がコメント扱いされると行末の `do` が消えてブロックが積まれず、
        // 続く `end` が別のブロックを pop してしまう。source ブロック内の gem が
        // 除外され、`end` の後の gem が拾えることで正しい追跡を確認する。
        let content = "source 'https://example.com#fragment' do\n  gem 'private-gem', '~> 1.0'\nend\ngem 'pg', '~> 1.5'\n";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "pg");
    }

    // --- バージョンなし gem のオプションキーワードに対する回帰テスト ---

    #[test]
    fn test_update_version_add_to_unversioned_gem_with_groups_option() {
        // 回帰テスト: `groups:` オプション付きのバージョンなし gem に挿入できること
        let content = "gem 'rubocop', groups: [:development, :test]\n";
        let result = GemfileParser
            .update_version(content, "rubocop", "1.60.0")
            .unwrap();
        assert_eq!(
            result,
            "gem 'rubocop', '1.60.0', groups: [:development, :test]\n"
        );
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_with_install_if_option() {
        // 回帰テスト: `install_if:` オプション付きのバージョンなし gem に挿入できること
        let content = "gem 'sidekiq', install_if: -> { ENV['WORKER'] }\n";
        let result = GemfileParser
            .update_version(content, "sidekiq", "7.2.0")
            .unwrap();
        assert_eq!(
            result,
            "gem 'sidekiq', '7.2.0', install_if: -> { ENV['WORKER'] }\n"
        );
    }

    #[test]
    fn test_update_version_add_to_unversioned_gem_with_force_ruby_platform_option() {
        // 回帰テスト: `force_ruby_platform:` オプション付きのバージョンなし gem に挿入できること
        let content = "gem 'grpc', force_ruby_platform: true\n";
        let result = GemfileParser
            .update_version(content, "grpc", "1.62.0")
            .unwrap();
        assert_eq!(result, "gem 'grpc', '1.62.0', force_ruby_platform: true\n");
    }

    #[test]
    fn test_update_version_duplicate_gem_updates_all_occurrences() {
        // 同名 gem が複数箇所 (group 内外) にある場合は全出現が更新される。
        // Cargo / Gradle / pyproject と同じ「1 依存 = 全出現を書き換え」の不変条件。
        let content = r#"
gem 'rails', '~> 7.0'

group :test do
  gem 'rails', '~> 7.0'
end
"#;
        // parse は同名 gem を 2 件返す
        let deps = parse(content).unwrap();
        assert_eq!(deps.iter().filter(|d| d.name == "rails").count(), 2);
        // update_version は両方の出現を更新する
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        // `~> 7.0` はセグメント数を保って `~> 7.1` になる
        assert_eq!(
            result.matches("'~> 7.1'").count(),
            2,
            "同名 gem の全出現が更新されるべき:\n{}",
            result
        );
    }

    // --- CRLF 改行コード保持の回帰テスト ---

    #[test]
    fn test_update_version_preserves_crlf_line_endings() {
        // 回帰テスト: CRLF の Gemfile を 1 依存更新しても全行が LF 化されないこと
        let content =
            "source 'https://rubygems.org'\r\n\r\ngem 'rails', '~> 7.0'\r\ngem 'pg', '~> 1.1'\r\n";
        let result = GemfileParser
            .update_version(content, "rails", "7.1.0")
            .unwrap();
        assert_eq!(
            result,
            "source 'https://rubygems.org'\r\n\r\ngem 'rails', '~> 7.1'\r\ngem 'pg', '~> 1.1'\r\n"
        );
    }

    #[test]
    fn test_update_versionless_gem_preserves_crlf_line_endings() {
        // 回帰テスト: CRLF ファイルへのバージョン挿入でも改行コードを保持すること
        let content = "gem 'rmagick'\r\ngem 'nokogiri'\r\n";
        let result = GemfileParser
            .update_version(content, "rmagick", "5.3.0")
            .unwrap();
        assert_eq!(result, "gem 'rmagick', '5.3.0'\r\ngem 'nokogiri'\r\n");
    }

    // --- `git_source` で登録した独自ショートハンドの回帰テスト ---

    /// 回帰テスト: `git_source(:stash)` で登録したキーを持つ gem は git 依存なので
    /// 更新対象にしない。以前は組み込み 7 キーしか見ておらず、
    /// `gem 'rails', stash: 'forks/rails'` を「バージョンなしの rubygems 依存」と
    /// 判定して rubygems.org の版を注入し、`bundle install` を壊していた
    /// (同名の公開 gem = typosquat の版を入れる供給網リスクでもある)。
    #[test]
    fn test_parse_skips_gem_with_custom_git_source_shorthand() {
        let content = r#"
git_source(:stash) { |repo_name| "https://stash.example.com/#{repo_name}.git" }

gem 'rails', stash: 'forks/rails'
gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["pg"], "{deps:?}");
    }

    /// `do |repo| ... end` 形式の `git_source` 宣言でも同じく認識すること
    #[test]
    fn test_parse_skips_gem_with_custom_git_source_do_block_form() {
        let content = r#"
git_source(:stash) do |repo_name|
  "https://stash.example.com/#{repo_name}.git"
end

gem 'rails', :stash => 'forks/rails'
gem 'pg', '~> 1.5'
"#;
        let deps = parse(content).unwrap();
        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["pg"], "{deps:?}");
    }

    /// parse と writer で同じ集合を使うこと。writer 側だけ知らないと
    /// 「parse は除外したのに writer が書き換える」非対称に戻る。
    #[test]
    fn test_update_version_skips_gem_with_custom_git_source_shorthand() {
        let content = r#"git_source(:stash) { |repo_name| "https://stash.example.com/#{repo_name}.git" }
gem 'rails', stash: 'forks/rails'
"#;
        let result = GemfileParser.update_version(content, "rails", "8.0.2");
        assert!(
            result.is_err(),
            "git 依存へ rubygems の版を注入してはいけない: {result:?}"
        );
    }

    /// `git_source` 宣言がなければ未知のキーは通常のオプションとして扱う
    /// (全キーをレジストリ外扱いして更新を取りこぼさないこと)。
    #[test]
    fn test_parse_unknown_option_key_without_git_source_is_registry_dep() {
        let content = "gem 'rails', stash: 'forks/rails'\n";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1, "{deps:?}");
        assert_eq!(deps[0].name, "rails");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Any);

        // writer 側も従来どおりバージョンを挿入できる
        let result = GemfileParser
            .update_version(content, "rails", "8.0.2")
            .unwrap();
        assert_eq!(result, "gem 'rails', '8.0.2', stash: 'forks/rails'\n");
    }

    /// コメントアウトされた `git_source` 宣言は登録扱いしない
    #[test]
    fn test_parse_commented_out_git_source_is_ignored() {
        let content = "# git_source(:stash) { |repo| repo }\ngem 'rails', stash: 'forks/rails'\n";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1, "{deps:?}");
        assert_eq!(deps[0].name, "rails");
    }

    // --- update_version のブロック追跡に対する回帰テスト ---

    /// 回帰テスト: parse は `path ... do` ブロック内の gem を除外するのに、
    /// update_version にはブロック追跡がなく名前一致した全行を書き換えていた。
    /// 結果としてローカル path 依存へ rubygems の版が注入されていた。
    #[test]
    fn test_update_version_skips_gems_inside_non_registry_block() {
        let content = r#"if ENV["LOCAL_MYGEM"]
  path "../mygem" do
    gem "mygem"
  end
else
  gem "mygem", "~> 2.0"
end
"#;
        // parse はブロック内の宣言を surface しない
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1, "{deps:?}");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);

        let result = GemfileParser
            .update_version(content, "mygem", "2.5.0")
            .unwrap();
        assert_eq!(
            result,
            r#"if ENV["LOCAL_MYGEM"]
  path "../mygem" do
    gem "mygem"
  end
else
  gem "mygem", "~> 2.5"
end
"#
        );
    }

    /// バージョンなし gem の挿入経路 (`version_parts` が空の分岐) でも
    /// 非レジストリブロック内は書き換えないこと
    #[test]
    fn test_update_version_skips_versionless_gem_inside_git_block() {
        let content = r#"git "https://github.com/rails/rails.git", branch: "main" do
  gem "activesupport"
end

gem "activesupport", "~> 7.0"
"#;
        let result = GemfileParser
            .update_version(content, "activesupport", "7.1.0")
            .unwrap();
        assert_eq!(
            result,
            r#"git "https://github.com/rails/rails.git", branch: "main" do
  gem "activesupport"
end

gem "activesupport", "~> 7.1"
"#
        );
    }

    /// `platforms` / `install_if` のような通常ブロック内の gem は従来どおり更新する
    #[test]
    fn test_update_version_updates_gems_inside_regular_block() {
        let content = "platforms :ruby do\n  gem 'pg', '~> 1.5'\nend\n";
        let result = GemfileParser
            .update_version(content, "pg", "1.6.0")
            .unwrap();
        assert_eq!(result, "platforms :ruby do\n  gem 'pg', '~> 1.6'\nend\n");
    }

    /// 非レジストリブロックを抜けた後の gem は更新対象に戻ること
    /// (ブロック追跡が `end` で正しく pop されること)
    #[test]
    fn test_update_version_after_non_registry_block_is_updated() {
        let content =
            "source 'https://gems.example.com' do\n  gem 'pg', '~> 1.0'\nend\ngem 'pg', '~> 1.5'\n";
        let result = GemfileParser
            .update_version(content, "pg", "1.6.0")
            .unwrap();
        assert_eq!(
            result,
            "source 'https://gems.example.com' do\n  gem 'pg', '~> 1.0'\nend\ngem 'pg', '~> 1.6'\n"
        );
    }

    #[test]
    fn test_update_version_preserves_mixed_line_endings() {
        // 回帰テスト: LF と CRLF が混在するファイルでも各行の改行コードを保持すること
        let content = "gem 'rails', '~> 7.0'\r\ngem 'pg', '~> 1.1'\n";
        let result = GemfileParser
            .update_version(content, "pg", "1.5.0")
            .unwrap();
        assert_eq!(result, "gem 'rails', '~> 7.0'\r\ngem 'pg', '~> 1.5'\n");
    }
}
