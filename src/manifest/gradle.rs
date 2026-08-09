//! Java プロジェクト向け Gradle マニフェストパーサ
//!
//! 対応内容:
//! - build.gradle（Groovy DSL）対応
//! - build.gradle.kts（Kotlin DSL）対応
//! - 変数定義 (def, val, ext block)
//! - map 記法依存: group: 'x', name: 'y', version: 'z'
//! - 文字列記法依存: 'group:name:version'
//! - バージョン内の変数参照

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::{ManifestParser, gradle_version_catalog, line_utils::split_line_ending};
use crate::parser::get_parser;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

/// build.gradle / build.gradle.kts 用パーサ
pub struct GradleParser;

/// 変数定義情報
#[derive(Debug, Clone)]
struct VariableDefinition {
    /// 変数の値
    value: String,
    /// 行番号 (1-based)
    line_number: usize,
    /// 同名の定義が異なる値で複数存在するか。
    /// `object Versions { ... }` と `object Deps { ... }` が同じ短名を持つ場合、
    /// 修飾付き参照 (`${Versions.x}`) を最終セグメントで引き当てると別オブジェクトの
    /// 値を拾いうるため、曖昧な短名では解決しない (誤更新を防ぐ安全側のスキップ)。
    ambiguous: bool,
}

/// Gradle rich version の宣言種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RichVersionMethod {
    Strictly,
    Require,
    Prefer,
    Reject,
}

/// rich version ブロック内で見つけた 1 つの宣言
#[derive(Debug, Clone)]
struct RichVersionMatch {
    spec: Option<VersionSpec>,
    line_offset: usize,
    value_start: usize,
    value_end: usize,
}

/// rich version として採用する宣言と、更新すべき位置
#[derive(Debug, Clone)]
struct RichVersionSelection {
    spec: VersionSpec,
    update_line_offset: usize,
    value_start: usize,
    value_end: usize,
}

// Gradle DSL 用の正規表現

// 変数定義 (Groovy): def wicketVersion = '1.2.3' / "1.2.3"。
// 型宣言 String wicketVersion = '...' (def を伴わない明示型) も許容する。
static VAR_DEF_GROOVY_SINGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:def|String)\s+(\w+)\s*=\s*'([^']+)'"#).unwrap());
static VAR_DEF_GROOVY_DOUBLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:def|String)\s+(\w+)\s*=\s*"([^"]+)""#).unwrap());

// 変数定義 (Kotlin): val wicketVersion = "1.2.3"。
// 型注釈付き val wicketVersion: String = "..." (Kotlin DSL の慣用形) も許容する。
// `const val WICKET = "1.2.3"` (Kotlin DSL で最も一般的なバージョン定義形式) も対象にする。
// 先頭の `const ` は任意とし、キャプチャ位置 (グループ1=名前, グループ2=値) は変えない。
static VAR_DEF_KOTLIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:const\s+)?val\s+(\w+)\s*(?::\s*\w+)?\s*=\s*"([^"]+)""#).unwrap()
});

// ext のドット代入: ext.wicketVersion = '1.2.3' / project.ext.wicketVersion = "1.2.3"。
// ext ブロックの外に書かれるため EXT_VAR_* (ブロック内限定) では拾えないが、
// 同一ファイル内に静的に書かれた解決可能な定義なので変数表へ載せる。
static EXT_DOT_VAR_SINGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:project\.)?ext\.(\w+)\s*=\s*'([^']+)'"#).unwrap());
static EXT_DOT_VAR_DOUBLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:project\.)?ext\.(\w+)\s*=\s*"([^"]+)""#).unwrap());

// ext ブロック内変数: wicketVersion = '1.2.3' または "1.2.3"
static EXT_VAR_SINGLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(\w+)\s*=\s*'([^']+)'"#).unwrap());
static EXT_VAR_DOUBLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(\w+)\s*=\s*"([^"]+)""#).unwrap());

// ext ブロック開始
static EXT_BLOCK_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*ext\s*\{").unwrap());

// configuration 名と依存座標の間に挟まる宣言ラッパ。
// BOM は `implementation platform('g:a:1.0')` / `implementation(platform("g:a:1.0"))` と
// 書くのが Gradle の標準で、これを取りこぼすと推移依存のバージョンを一括決定する
// 宣言だけが無言で更新対象外になる (書き戻し側は行頭アンカーが無いため既に対応済み)。
const DEP_WRAPPER: &str = r"(?:(?:enforcedPlatform|platform|testFixtures)\s*\(\s*)?";

// map 記法依存: implementation group: 'x', name: 'y', version: 'z'
// implementation(group: 'x', name: 'y', version: 'z') も処理
// 非後方参照パターンを使用 (シングル/ダブルクォート両対応)
static DEP_MAP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^\s*(\w+)\s*[\(\s]+{DEP_WRAPPER}group\s*[:=]\s*['"]([^'"]+)['"]\s*,\s*name\s*[:=]\s*['"]([^'"]+)['"]\s*,\s*version\s*[:=]\s*['"]?([^'",\)\s]+)['"]?"#,
    ))
    .unwrap()
});

// 文字列記法依存: implementation 'group:name:version'
// 非後方参照パターンを使用 (シングル/ダブルクォート両対応)
static DEP_STRING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^\s*(\w+)\s*[\(\s]*{DEP_WRAPPER}['"]([^:'"]+):([^:'"]+):([^:'"@]+)(?::[^'"]+)?(?:@[^'"]+)?['"]"#,
    ))
    .unwrap()
});

// 変数展開あり文字列記法: implementation "group:name:$version"
// 参照名は `${Versions.retrofit}` / `${rootProject.ext.springVersion}` のように
// 修飾付きでも書けるため `.` を許容し、解決時に最終セグメントで引き当てる。
static DEP_STRING_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^\s*(\w+)\s*[\(\s]*{DEP_WRAPPER}"([^:"]+):([^:"]+):\$\{{?([\w.]+)\}}?""#
    ))
    .unwrap()
});

// rich version ブロックを持つ文字列記法依存: implementation("group:name") { ... }
// `implementation(platform("g:a")) { ... }` のようにラッパで括弧が増える形にも対応する
static DEP_STRING_NO_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"^\s*(\w+)\s*[\(\s]*{DEP_WRAPPER}['"]([^:'"]+):([^:'"]+)['"]\s*\)*\s*(?:\{{|$)"#
    ))
    .unwrap()
});

// rich version 宣言: strictly("1.2.3") / require '1.2.3' / prefer "1.2.3" / reject("1.2.3")
//
// メソッド名の後ろにも語境界 (`\b`) を要求する。これがないと
// `capabilities { requireCapability("com.example:lib-feature") }` の
// `requireCapability` が `require` 宣言として誤マッチし、パースできない引数で
// `strong` が上書きされて依存ごと消える (警告なしの取りこぼし)。
static RICH_VERSION_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\b(strictly|require|prefer|reject)\b\s*(?:\(\s*)?((?:"[^"]+"|'[^']+')(?:\s*,\s*(?:"[^"]+"|'[^']+'))*)\s*\)?"#,
    )
    .unwrap()
});
static RICH_VERSION_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)"|'([^']+)'"#).unwrap());
// `rejectAll()` (引数なし) を検出する。`reject(...)` とは別に「全バージョン拒否」を意味し、
// RICH_VERSION_DECL は引数必須のためこれを拾えない。version catalog の `rejectAll = true` と対になる。
static REJECT_ALL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\brejectAll\s*\(\s*\)").unwrap());

// 開発用 configuration
const DEV_CONFIGURATIONS: [&str; 6] = [
    "testImplementation",
    "testCompileOnly",
    "testRuntimeOnly",
    "testApi",
    "androidTestImplementation",
    "debugImplementation",
];

fn rich_version_method(name: &str) -> Option<RichVersionMethod> {
    match name {
        "strictly" => Some(RichVersionMethod::Strictly),
        "require" => Some(RichVersionMethod::Require),
        "prefer" => Some(RichVersionMethod::Prefer),
        "reject" => Some(RichVersionMethod::Reject),
        _ => None,
    }
}

fn strip_gradle_comments_from_line(line: &str, in_block_comment: &mut bool) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string: Option<char> = None;

    while let Some(ch) = chars.next() {
        if *in_block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                output.push_str("  ");
                *in_block_comment = false;
            } else {
                // 後段がバイト位置で元の行を参照できるよう、文字のバイト長ぶん空白に置換する
                for _ in 0..ch.len_utf8() {
                    output.push(' ');
                }
            }
            continue;
        }

        if let Some(quote) = in_string {
            // 文字列リテラル内の // や /* ('http://...' や 'META-INF/*' など) はコードとして残す
            output.push(ch);
            if ch == '\\' {
                if let Some(escaped) = chars.next() {
                    output.push(escaped);
                }
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                in_string = Some(ch);
                output.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                // 文字列外の行コメント: 行末まで切り捨てる
                return output;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                output.push_str("  ");
                *in_block_comment = true;
            }
            _ => output.push(ch),
        }
    }

    output
}

/// ブロックコメント状態を行間で引き継ぎながら全行のコメントを除去する
fn strip_gradle_comment_lines(content: &str) -> Vec<String> {
    let mut in_block_comment = false;
    content
        .lines()
        .map(|line| strip_gradle_comments_from_line(line, &mut in_block_comment))
        .collect()
}

fn replace_all_active_gradle_matches<F>(
    content: &str,
    re: &Regex,
    mut replacement: F,
) -> (String, bool)
where
    F: FnMut(&regex::Captures) -> Option<String>,
{
    let mut output = String::new();
    let mut updated = false;
    let mut in_block_comment = false;

    for segment in content.split_inclusive('\n') {
        let (line, newline) = split_line_ending(segment);

        let active_line = strip_gradle_comments_from_line(line, &mut in_block_comment);
        let mut rebuilt = String::new();
        let mut last_end = 0usize;

        // 同一座標の複数宣言 (compileOnly + annotationProcessor 等) を全て更新するため、
        // アクティブ行の全マッチを出現ごとに自身の旧値から整形して置換する。
        // コメント除去はバイト位置を保つため、マッチ位置を元の行へそのまま適用できる。
        for caps in re.captures_iter(&active_line) {
            let Some(whole) = caps.get(0) else {
                continue;
            };
            let Some(new_match) = replacement(&caps) else {
                continue;
            };
            rebuilt.push_str(&line[last_end..whole.start()]);
            rebuilt.push_str(&new_match);
            last_end = whole.end();
            updated = true;
        }

        if last_end > 0 {
            rebuilt.push_str(&line[last_end..]);
            rebuilt.push_str(newline);
            output.push_str(&rebuilt);
        } else {
            output.push_str(segment);
        }
    }

    (output, updated)
}

/// 変数定義キャプチャ (グループ 1=名前, グループ 2=値) を検証して変数表へ登録する。
/// `exclude_non_version_names` が真のとき、ext ブロックで一般的な非バージョン変数
/// (`source*` / `target*` / `encoding`) は除外する。
fn insert_captured_variable(
    variables: &mut HashMap<String, VariableDefinition>,
    caps: &regex::Captures,
    line_number: usize,
    exclude_non_version_names: bool,
) {
    let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    let value = caps.get(2).map(|m| m.as_str()).unwrap_or("");

    if name.is_empty() || value.is_empty() {
        return;
    }

    // 一般的な非バージョン変数は除外
    if exclude_non_version_names
        && (name.starts_with("source") || name.starts_with("target") || name == "encoding")
    {
        return;
    }

    let ambiguous = variables
        .get(name)
        .is_some_and(|existing| existing.value != value);

    variables.insert(
        name.to_string(),
        VariableDefinition {
            value: value.to_string(),
            line_number,
            ambiguous,
        },
    );
}

/// `$var` / `${var}` の参照名から変数定義を引き当て、実キーと定義を返す。
///
/// 完全一致を優先し、見つからなければ `Versions.retrofit` / `rootProject.ext.springVersion`
/// のような修飾付き参照とみなして最終セグメントで引く。書き戻し側は返された実キーで
/// 変数定義行を探すため、参照表記ではなくキー名を返す。
fn lookup_variable<'a>(
    variables: &'a HashMap<String, VariableDefinition>,
    reference: &str,
) -> Option<(&'a str, &'a VariableDefinition)> {
    if let Some((key, def)) = variables.get_key_value(reference) {
        return Some((key.as_str(), def));
    }
    let last = reference.rsplit('.').next()?;
    if last == reference {
        return None;
    }
    variables
        .get_key_value(last)
        .filter(|(_, def)| !def.ambiguous)
        .map(|(key, def)| (key.as_str(), def))
}

impl GradleParser {
    /// content から変数定義を抽出
    fn extract_variables(&self, content: &str) -> HashMap<String, VariableDefinition> {
        let mut variables = HashMap::new();
        let mut in_ext_block = false;
        let mut brace_depth = 0;
        let mut in_block_comment = false;

        for (line_idx, raw_line) in content.lines().enumerate() {
            let line_number = line_idx + 1;
            // 行コメント・ブロックコメント内の変数定義を拾わないよう、除去済みの行で判定する
            let line = strip_gradle_comments_from_line(raw_line, &mut in_block_comment);
            let line = line.as_str();
            let trimmed = line.trim();

            // 空行とコメント行をスキップ
            if trimmed.is_empty() {
                continue;
            }

            // ext ブロックの開始/終了を追跡
            if EXT_BLOCK_START.is_match(trimmed) {
                in_ext_block = true;
                brace_depth = 1;
                // 1 行完結の ext ブロックを判定
                if trimmed.contains('}') {
                    brace_depth = 0;
                    in_ext_block = false;
                }
                continue;
            }

            // ext ブロック内のネスト深さを追跡
            if in_ext_block {
                brace_depth += trimmed.matches('{').count();
                brace_depth = brace_depth.saturating_sub(trimmed.matches('}').count());
                if brace_depth == 0 {
                    in_ext_block = false;
                }
            }

            // `ext.<name> = '...'` / `project.ext.<name> = "..."` のドット代入を先に判定する
            // (ext ブロック外に書けるため EXT_VAR_* では拾えない)
            let ext_dot_regexes: [&Regex; 2] = [&*EXT_DOT_VAR_SINGLE, &*EXT_DOT_VAR_DOUBLE];
            if let Some(caps) = ext_dot_regexes.into_iter().find_map(|re| re.captures(line)) {
                insert_captured_variable(&mut variables, &caps, line_number, true);
                continue;
            }

            // 変数定義 (Groovy def シングル/ダブルクォート, Kotlin val) を従来の判定順で試し、
            // 最初にマッチした正規表現を採用する
            let var_def_regexes: [&Regex; 3] = [
                &*VAR_DEF_GROOVY_SINGLE,
                &*VAR_DEF_GROOVY_DOUBLE,
                &*VAR_DEF_KOTLIN,
            ];
            if let Some(caps) = var_def_regexes.into_iter().find_map(|re| re.captures(line)) {
                insert_captured_variable(&mut variables, &caps, line_number, false);
                continue;
            }

            // ext ブロック変数 (シングル/ダブルクォート) を判定
            if in_ext_block {
                let ext_var_regexes: [&Regex; 2] = [&*EXT_VAR_SINGLE, &*EXT_VAR_DOUBLE];
                if let Some(caps) = ext_var_regexes.into_iter().find_map(|re| re.captures(line)) {
                    insert_captured_variable(&mut variables, &caps, line_number, true);
                }
            }
        }

        variables
    }

    /// map 記法依存をパース
    fn parse_map_notation(
        &self,
        line: &str,
        variables: &HashMap<String, VariableDefinition>,
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<(Dependency, Option<String>)> {
        let caps = DEP_MAP.captures(line)?;

        let config = caps.get(1).map(|m| m.as_str())?;
        let group = caps.get(2).map(|m| m.as_str())?;
        let artifact = caps.get(3).map(|m| m.as_str())?;
        let version_raw = caps.get(4).map(|m| m.as_str())?;

        // version が変数参照か判定
        let (version, variable_name) = self.resolve_version(version_raw, variables);

        // バージョン文字列をパース
        let spec = if version.is_empty() {
            VersionSpec::new(VersionSpecKind::Any, "", "")
        } else {
            parser.parse(&version)?
        };

        let is_dev = DEV_CONFIGURATIONS.contains(&config);
        let name = format!("{}:{}", group, artifact);

        let dep = if is_dev {
            Dependency::development(name, spec, Language::Java)
        } else {
            Dependency::production(name, spec, Language::Java)
        };

        Some((dep, variable_name))
    }

    /// 文字列記法依存をパース
    fn parse_string_notation(
        &self,
        line: &str,
        variables: &HashMap<String, VariableDefinition>,
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<(Dependency, Option<String>)> {
        // まず変数展開あり文字列記法を試す
        if let Some(caps) = DEP_STRING_VAR.captures(line) {
            let config = caps.get(1).map(|m| m.as_str())?;
            let group = caps.get(2).map(|m| m.as_str())?;
            let artifact = caps.get(3).map(|m| m.as_str())?;
            let var_ref = caps.get(4).map(|m| m.as_str())?;

            // 変数参照を解決する。depup が追跡しない場所 (gradle.properties / `by extra(...)` /
            // 計算値など) で定義された変数は解決できない。その場合に空の `Any` 依存を作ると、
            // judge は「更新あり」と報告する一方で writer は書き換え先 (リテラル version も
            // 変数定義も) を見つけられず失敗し、「更新を報告したのに適用エラー」という
            // 矛盾した結果になる。更新できないことが確定しているため依存ごとスキップする。
            let (var_name, definition) = lookup_variable(variables, var_ref)?;
            let version = definition.value.clone();

            let spec = parser.parse(&version)?;

            let is_dev = DEV_CONFIGURATIONS.contains(&config);
            let name = format!("{}:{}", group, artifact);

            let dep = if is_dev {
                Dependency::development(name, spec, Language::Java)
            } else {
                Dependency::production(name, spec, Language::Java)
            };

            return Some((dep, Some(var_name.to_string())));
        }

        // 通常の文字列記法を試す
        let caps = DEP_STRING.captures(line)?;

        let config = caps.get(1).map(|m| m.as_str())?;
        let group = caps.get(2).map(|m| m.as_str())?;
        let artifact = caps.get(3).map(|m| m.as_str())?;
        let version = caps.get(4).map(|m| m.as_str())?;

        // maven { url 'http://nexus:8081' } のような URL (group が http/https 等の
        // スキームで artifact が // 始まり) は依存座標ではないため除外する
        if artifact.starts_with("//") {
            return None;
        }

        let spec = parser.parse(version)?;
        let is_dev = DEV_CONFIGURATIONS.contains(&config);
        let name = format!("{}:{}", group, artifact);

        let dep = if is_dev {
            Dependency::development(name, spec, Language::Java)
        } else {
            Dependency::production(name, spec, Language::Java)
        };

        Some((dep, None))
    }

    /// 依存宣言のクロージャ範囲を行番号で取得
    fn dependency_block_end(&self, lines: &[&str], start_index: usize) -> Option<usize> {
        let first_line = lines.get(start_index)?;
        if !first_line.contains('{') {
            return None;
        }

        let mut depth = 0usize;
        let mut opened = false;

        for (line_index, line) in lines.iter().enumerate().skip(start_index) {
            let open_count = line.matches('{').count();
            let close_count = line.matches('}').count();

            if open_count > 0 {
                opened = true;
            }

            if opened {
                depth = depth.saturating_add(open_count);
                depth = depth.saturating_sub(close_count);

                if depth == 0 {
                    return Some(line_index);
                }
            }
        }

        if opened { Some(lines.len() - 1) } else { None }
    }

    /// rich version ブロックの代表バージョンを選ぶ
    fn find_rich_version_selection(
        &self,
        block_lines: &[&str],
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<RichVersionSelection> {
        let mut strong: Option<RichVersionMatch> = None;
        let mut prefer: Option<RichVersionMatch> = None;
        let mut rejected_versions = Vec::new();
        let mut in_block_comment = false;

        for (line_offset, line) in block_lines.iter().enumerate() {
            let code = strip_gradle_comments_from_line(line, &mut in_block_comment);
            let trimmed = code.trim();
            if trimmed.is_empty() {
                continue;
            }

            // `rejectAll()` は全バージョンを拒否するため、宣言があれば更新対象から外す。
            // version catalog 側 (`rejectAll = true`) と挙動を揃える。「拒否制約を無視して更新」を
            // 防ぐため、ブロック内に rejectAll() が現れたら他の宣言の有無に関わらずスキップする
            // (安全側)。
            if REJECT_ALL_RE.is_match(&code) {
                return None;
            }

            for caps in RICH_VERSION_DECL.captures_iter(&code) {
                let method = rich_version_method(caps.get(1)?.as_str())?;
                let args_match = caps.get(2)?;
                let args = args_match.as_str();

                if method == RichVersionMethod::Reject {
                    for value_caps in RICH_VERSION_VALUE.captures_iter(args) {
                        let Some(value_match) = value_caps.get(1).or_else(|| value_caps.get(2))
                        else {
                            continue;
                        };
                        rejected_versions.push(value_match.as_str().to_string());
                    }
                    continue;
                }

                let Some(value_caps) = RICH_VERSION_VALUE.captures(args) else {
                    continue;
                };
                let Some(version_match) = value_caps.get(1).or_else(|| value_caps.get(2)) else {
                    continue;
                };
                let version = version_match.as_str();
                let found = RichVersionMatch {
                    spec: parser.parse(version),
                    line_offset,
                    value_start: args_match.start() + version_match.start(),
                    value_end: args_match.start() + version_match.end(),
                };

                // Gradle の MutableVersionConstraint は後続の version 宣言で既存の reject を消す。
                rejected_versions.clear();

                match method {
                    RichVersionMethod::Strictly | RichVersionMethod::Require => {
                        // MutableVersionConstraint は後勝ちなので、最後の強い宣言を採用する。
                        strong = Some(found);
                    }
                    RichVersionMethod::Prefer => {
                        prefer = Some(found);
                    }
                    RichVersionMethod::Reject => {}
                }
            }
        }

        if let Some(strong_match) = strong {
            let strong_spec = strong_match.spec.clone()?;

            if strong_spec.kind == VersionSpecKind::Range
                && let Some(prefer_match) = prefer
                && let Some(prefer_spec) = prefer_match.spec.clone()
            {
                return Some(RichVersionSelection {
                    // strictly/require の範囲は上限判定に使い、現在値と更新対象は prefer とする。
                    spec: VersionSpec::new(
                        VersionSpecKind::Range,
                        strong_spec.raw,
                        prefer_spec.version,
                    )
                    .with_rejected_versions(rejected_versions.clone()),
                    update_line_offset: prefer_match.line_offset,
                    value_start: prefer_match.value_start,
                    value_end: prefer_match.value_end,
                });
            }

            return Some(RichVersionSelection {
                spec: strong_spec.with_rejected_versions(rejected_versions),
                update_line_offset: strong_match.line_offset,
                value_start: strong_match.value_start,
                value_end: strong_match.value_end,
            });
        }

        let prefer_match = prefer?;
        Some(RichVersionSelection {
            spec: prefer_match.spec?.with_rejected_versions(rejected_versions),
            update_line_offset: prefer_match.line_offset,
            value_start: prefer_match.value_start,
            value_end: prefer_match.value_end,
        })
    }

    /// rich version ブロック付き依存をパース
    fn parse_rich_version_notation(
        &self,
        lines: &[&str],
        start_index: usize,
        parser: &dyn crate::parser::VersionParser,
    ) -> Option<Dependency> {
        let line = lines.get(start_index)?;
        let caps = DEP_STRING_NO_VERSION.captures(line)?;

        let config = caps.get(1).map(|m| m.as_str())?;
        let group = caps.get(2).map(|m| m.as_str())?;
        let artifact = caps.get(3).map(|m| m.as_str())?;
        let end_index = self.dependency_block_end(lines, start_index)?;
        let selection =
            self.find_rich_version_selection(&lines[start_index..=end_index], parser)?;
        let is_dev = DEV_CONFIGURATIONS.contains(&config);
        let name = format!("{}:{}", group, artifact);

        let dep = if is_dev {
            Dependency::development(name, selection.spec, Language::Java)
        } else {
            Dependency::production(name, selection.spec, Language::Java)
        };

        Some(dep)
    }

    /// 変数参照を考慮してバージョン値を解決
    fn resolve_version(
        &self,
        version_raw: &str,
        variables: &HashMap<String, VariableDefinition>,
    ) -> (String, Option<String>) {
        let trimmed = version_raw.trim();

        // 変数参照パターンを判定
        // パターン1: $variableName
        // パターン2: ${variableName}
        // パターン3: variableName (非クォート)

        let var_name =
            if let Some(inner) = trimmed.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
                Some(inner)
            } else if let Some(stripped) = trimmed.strip_prefix('$') {
                Some(stripped)
            } else if !trimmed.starts_with('\'')
                && !trimmed.starts_with('"')
                && !trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
            {
                // 非クォートかつ非数値始まりは変数の可能性がある
                Some(trimmed)
            } else {
                None
            };

        if let Some(var_name) = var_name
            && let Some(var_def) = variables.get(var_name)
        {
            return (var_def.value.clone(), Some(var_name.to_string()));
        }

        // 変数参照でなければそのまま返す (前後のクォートのみ除去)
        let version = trimmed
            .trim_start_matches(['\'', '"'])
            .trim_end_matches(['\'', '"']);
        (version.to_string(), None)
    }
}

impl ManifestParser for GradleParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        if let Some(dependencies) = gradle_version_catalog::parse(content)? {
            return Ok(dependencies);
        }

        let mut dependencies = Vec::new();
        let parser = get_parser(Language::Java);
        let variables = self.extract_variables(content);
        // ブロックコメント内の宣言を生きた依存として拾わないよう、除去済みの行で判定する
        let stripped_lines = strip_gradle_comment_lines(content);
        let lines: Vec<&str> = stripped_lines.iter().map(|line| line.as_str()).collect();

        for (line_index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // 空行とコメント行をスキップ
            if trimmed.is_empty() {
                continue;
            }

            // map 記法を先に試す
            if let Some((dep, var_name)) =
                self.parse_map_notation(line, &variables, parser.as_ref())
            {
                let dep = if let Some(ref name) = var_name {
                    dep.with_variable(name)
                } else {
                    dep
                };
                dependencies.push(dep);
                continue;
            }

            // 文字列記法を試す
            if let Some((dep, var_name)) =
                self.parse_string_notation(line, &variables, parser.as_ref())
            {
                let dep = if let Some(ref name) = var_name {
                    dep.with_variable(name)
                } else {
                    dep
                };
                dependencies.push(dep);
                continue;
            }

            // rich version ブロック付き文字列記法を試す
            if let Some(dep) = self.parse_rich_version_notation(&lines, line_index, parser.as_ref())
            {
                dependencies.push(dep);
            }
        }

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Java
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        if let Some(result) = gradle_version_catalog::update_version(content, package, new_version)?
        {
            return Ok(result);
        }

        let parser = get_parser(Language::Java);
        let variables = self.extract_variables(content);
        // コメントアウトされた宣言に変数バインドを誘導されないよう、除去済みの行で判定する
        let stripped_lines = strip_gradle_comment_lines(content);

        // このパッケージに使われている変数名を特定
        let mut variable_for_package: Option<String> = None;

        for line in &stripped_lines {
            // map 記法を確認
            if let Some((_dep, var_name)) =
                self.parse_map_notation(line, &variables, parser.as_ref())
                && _dep.name == package
            {
                variable_for_package = var_name;
                break;
            }

            // 文字列記法を確認
            if let Some((_dep, var_name)) =
                self.parse_string_notation(line, &variables, parser.as_ref())
                && _dep.name == package
            {
                variable_for_package = var_name;
                break;
            }
        }

        // 変数経由の場合は変数定義を更新
        if let Some(var_name) = variable_for_package
            && let Some(var_def) = variables.get(&var_name)
        {
            return self.update_variable_definition(content, &var_name, var_def, new_version);
        }

        // それ以外は依存行の直接バージョンを更新
        self.update_direct_version(content, package, new_version)
    }
}

impl GradleParser {
    /// 新しいバージョンで変数定義を更新
    fn update_variable_definition(
        &self,
        content: &str,
        var_name: &str,
        var_def: &VariableDefinition,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let version_parser = get_parser(Language::Java);
        let Some(formatted_version) = version_parser
            .parse(&var_def.value)
            .and_then(|spec| spec.try_format_updated(new_version))
        else {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: var_name.to_string(),
                message: "version could not be updated safely".to_string(),
            });
        };

        let mut result = String::new();
        let mut in_block_comment = false;

        // CRLF/LF を保持するため split_inclusive で改行込みに走査し、本文だけを
        // 処理してから元の改行を再付与する (content.lines()+join は CRLF を潰す)。
        for (idx, raw_line) in content.split_inclusive('\n').enumerate() {
            let (line, line_ending) = split_line_ending(raw_line);
            let active_line = strip_gradle_comments_from_line(line, &mut in_block_comment);

            if idx + 1 == var_def.line_number
                && let Some(replaced) =
                    replace_variable_version_value(line, &active_line, &formatted_version)
            {
                result.push_str(&replaced);
                result.push_str(line_ending);
                continue;
            }

            result.push_str(line);
            result.push_str(line_ending);
        }

        Ok(result)
    }

    /// 依存行の直接バージョンを更新
    fn update_direct_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        let parts: Vec<&str> = package.split(':').collect();
        if parts.len() != 2 {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: package.to_string(),
                message: "invalid package format, expected 'group:artifact'".to_string(),
            });
        }
        let (group, artifact) = (parts[0], parts[1]);
        let escaped_group = regex::escape(group);
        let escaped_artifact = regex::escape(artifact);
        let version_parser = get_parser(Language::Java);
        let format_updated = |current_version: &str| -> Option<String> {
            version_parser
                .parse(current_version)
                .and_then(|spec| spec.try_format_updated(new_version))
        };

        // map 記法を更新: group: 'x', name: 'y', version: 'z'
        // 非後方参照パターンでシングル/ダブルクォート両対応
        let map_pattern = format!(
            r#"(group\s*[:=]\s*['"]{}['"]\s*,\s*name\s*[:=]\s*['"]{}['"]\s*,\s*version\s*[:=]\s*)(['"])([^'"]+)['"]"#,
            escaped_group, escaped_artifact
        );
        let map_re = Regex::new(&map_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        let (result, mut updated) =
            replace_all_active_gradle_matches(content, &map_re, |caps: &regex::Captures| {
                let prefix = &caps[1];
                let quote = &caps[2];
                let current_version = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                format_updated(current_version).map(|updated_version| {
                    format!("{}{}{}{}", prefix, quote, updated_version, quote)
                })
            });

        if updated {
            return Ok(result);
        }

        // 文字列記法を更新: 'group:artifact:version'
        // 非後方参照パターンでシングル/ダブルクォート両対応
        let string_pattern = format!(
            r#"(['"]){}:{}:([^:'"@]+)((?::[^'"]+)?(?:@[^'"]+)?)['"]"#,
            escaped_group, escaped_artifact
        );
        let string_re =
            Regex::new(&string_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("build.gradle"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        let (result, string_updated) =
            replace_all_active_gradle_matches(content, &string_re, |caps: &regex::Captures| {
                let quote = &caps[1];
                let current_version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let suffix = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                format_updated(current_version).map(|updated_version| {
                    format!(
                        "{}{}:{}:{}{}{}",
                        quote, group, artifact, updated_version, suffix, quote
                    )
                })
            });
        updated = string_updated;

        if updated {
            return Ok(result);
        }

        if let Some(result) =
            self.update_rich_version_block(content, group, artifact, new_version)?
        {
            return Ok(result);
        }

        Err(ManifestError::InvalidVersionSpec {
            path: PathBuf::from("build.gradle"),
            spec: package.to_string(),
            message: "dependency not found or version could not be updated".to_string(),
        })
    }

    /// rich version ブロック内の更新対象宣言を書き換える
    fn update_rich_version_block(
        &self,
        content: &str,
        group: &str,
        artifact: &str,
        new_version: &str,
    ) -> Result<Option<String>, ManifestError> {
        let lines: Vec<&str> = content.lines().collect();
        // ブロックコメント内のオープナーや宣言を書き換えないよう、除去済みの行で検出する
        let stripped_lines = strip_gradle_comment_lines(content);
        let active_lines: Vec<&str> = stripped_lines.iter().map(|line| line.as_str()).collect();
        let parser = get_parser(Language::Java);

        for (line_index, active_line) in active_lines.iter().enumerate() {
            let Some(caps) = DEP_STRING_NO_VERSION.captures(active_line) else {
                continue;
            };

            if caps.get(2).map(|m| m.as_str()) != Some(group)
                || caps.get(3).map(|m| m.as_str()) != Some(artifact)
            {
                continue;
            }

            let Some(end_index) = self.dependency_block_end(&active_lines, line_index) else {
                continue;
            };
            let Some(selection) = self.find_rich_version_selection(
                &active_lines[line_index..=end_index],
                parser.as_ref(),
            ) else {
                continue;
            };

            let update_line_index = line_index + selection.update_line_offset;
            let current_line = lines[update_line_index];
            let current_version = &current_line[selection.value_start..selection.value_end];
            let Some(formatted_version) = parser
                .parse(current_version)
                .and_then(|spec| spec.try_format_updated(new_version))
            else {
                return Err(ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("build.gradle"),
                    spec: package_name(group, artifact),
                    message: "version could not be updated safely".to_string(),
                });
            };

            // CRLF/LF を保持するため、更新行以外は元の生セグメント (改行込み) をそのまま使い、
            // 更新行だけ本文を差し替えて元の改行を再付与する (join("\n") は CRLF を潰す)。
            let mut result = String::new();
            for (i, raw_line) in content.split_inclusive('\n').enumerate() {
                if i == update_line_index {
                    let (_, line_ending) = split_line_ending(raw_line);
                    result.push_str(&current_line[..selection.value_start]);
                    result.push_str(&formatted_version);
                    result.push_str(&current_line[selection.value_end..]);
                    result.push_str(line_ending);
                } else {
                    result.push_str(raw_line);
                }
            }
            return Ok(Some(result));
        }

        Ok(None)
    }
}

fn package_name(group: &str, artifact: &str) -> String {
    format!("{}:{}", group, artifact)
}

/// 変数定義行のバージョン値 (キャプチャ範囲) だけを差し替える。
/// 行を再構築しないため、インデント・行末コメント・クォート文字は元のまま保持される。
fn replace_variable_version_value(
    line: &str,
    active_line: &str,
    formatted_version: &str,
) -> Option<String> {
    let regexes: [&Regex; 7] = [
        &*VAR_DEF_GROOVY_SINGLE,
        &*VAR_DEF_GROOVY_DOUBLE,
        &*VAR_DEF_KOTLIN,
        // parse 側が拾う `ext.<name> = '...'` も書き戻せるようにする
        // (ここを欠くと report/apply が矛盾する)
        &*EXT_DOT_VAR_SINGLE,
        &*EXT_DOT_VAR_DOUBLE,
        &*EXT_VAR_SINGLE,
        &*EXT_VAR_DOUBLE,
    ];

    for re in regexes {
        let Some(value) = re.captures(active_line).and_then(|caps| caps.get(2)) else {
            continue;
        };
        // コメント除去はバイト位置を保つため、値の範囲を元の行へそのまま適用できる
        let mut replaced = String::with_capacity(line.len() + formatted_version.len());
        replaced.push_str(&line[..value.start()]);
        replaced.push_str(formatted_version);
        replaced.push_str(&line[value.end()..]);
        return Some(replaced);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{UpdateResult, VersionSpecKind};
    use crate::update::{UpdateFilter, UpdateJudge, VersionInfo};
    use chrono::{TimeZone, Utc};

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        GradleParser.parse(content)
    }

    // 基本的な依存関係パースのテスト

    #[test]
    fn test_parse_string_notation() {
        let content = r#"
dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
        assert_eq!(deps[0].version_spec.version, "9.12.0");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_string_notation_with_classifier_and_extension() {
        // Gradle の group:name:version:classifier@extension 形式も version 部だけを解析する
        let content = r#"
dependencies {
    runtimeOnly("net.sf.docbook:docbook-xsl:1.75.2:resources@zip")
    implementation("com.google.android.material:material:1.11.0@aar")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let docbook = deps
            .iter()
            .find(|d| d.name == "net.sf.docbook:docbook-xsl")
            .unwrap();
        assert_eq!(docbook.version_spec.version, "1.75.2");

        let material = deps
            .iter()
            .find(|d| d.name == "com.google.android.material:material")
            .unwrap();
        assert_eq!(material.version_spec.version, "1.11.0");
    }

    #[test]
    fn test_parse_string_notation_double_quotes() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:5.3.23"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.springframework:spring-core");
        assert_eq!(deps[0].version_spec.version, "5.3.23");
    }

    #[test]
    fn test_parse_string_notation_maven_alt_range() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:]5.2.0,5.3.8["
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.springframework:spring-core");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.version, "5.2.0");
    }

    #[test]
    fn test_parse_string_notation_maven_range_with_multi_part_qualifier() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:[5.2.0,5.3.8-beta1-SNAPSHOT]"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.springframework:spring-core");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.version, "5.2.0");
    }

    #[test]
    fn test_parse_map_notation() {
        let content = r#"
dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: '9.12.0'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    #[test]
    fn test_parse_map_notation_with_parens() {
        let content = r#"
dependencies {
    implementation(group: 'org.apache.wicket', name: 'wicket-core', version: '9.12.0')
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
    }

    #[test]
    fn test_parse_kotlin_named_argument_map_notation() {
        let content = r#"
dependencies {
    implementation(group = "com.google.guava", name = "guava", version = "32.1.2-jre")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
    }

    #[test]
    fn test_parse_test_implementation() {
        let content = r#"
dependencies {
    testImplementation 'junit:junit:4.13.2'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_rich_version_strictly_groovy() {
        let content = r#"
dependencies {
    implementation('org.slf4j:slf4j-api') {
        version {
            strictly '1.7.36'
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.slf4j:slf4j-api");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "1.7.36");
    }

    #[test]
    fn test_parse_rich_version_range_with_prefer_kotlin() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.slf4j:slf4j-api");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, "[1.7, 1.8[");
        // 更新判定に使う現在値は Gradle が選好する prefer 側を採用する。
        assert_eq!(deps[0].version_spec.version, "1.7.25");
    }

    #[test]
    fn test_parse_string_notation_strict_range_with_prefer() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.slf4j:slf4j-api");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, "[1.7, 1.8[!!1.7.25");
        assert_eq!(deps[0].version_spec.version, "1.7.25");
    }

    #[test]
    fn test_parse_rich_version_ignores_commented_declaration() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            // 優先バージョン指定の例: prefer("1.7.25")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_rich_version_ignores_block_commented_declaration() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            /*
             * prefer("1.7.25")
             */
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_judge_rich_version_prefer_respects_strict_range() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
        }
    }
}
"#;
        let dep = parse(content).unwrap().remove(0);
        let released_at = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let versions = vec![
            VersionInfo::new("1.7.36", released_at),
            VersionInfo::new("1.8.0", released_at),
        ];

        let result = UpdateJudge::new(UpdateFilter::new()).judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.7.36");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_judge_string_notation_strict_range_with_prefer_respects_upper_bound() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25")
}
"#;
        let dep = parse(content).unwrap().remove(0);
        let released_at = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let versions = vec![
            VersionInfo::new("1.7.36", released_at),
            VersionInfo::new("1.8.0", released_at),
        ];

        let result = UpdateJudge::new(UpdateFilter::new()).judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.7.36");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_parse_rich_version_rejects() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
            reject("1.7.36", "1.7.37")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(
            deps[0].version_spec.rejected_versions,
            vec!["1.7.36", "1.7.37"]
        );
    }

    #[test]
    fn test_parse_rich_version_reject_all_skips_dependency() {
        // rejectAll() は全バージョンを拒否するため、該当依存は更新対象から外れる。
        // version catalog の `rejectAll = true` と同じ扱いにし、拒否制約を無視した
        // 誤更新を防ぐ。
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.0, 2.0[")
            rejectAll()
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_rich_version_reject_all_only_skips_dependency() {
        // rejectAll() 単独 (strictly/prefer なし) でもスキップする
        let content = r#"
dependencies {
    implementation("org.example:lib") {
        version {
            rejectAll()
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_judge_rich_version_rejects_candidate() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
            reject("1.7.36")
        }
    }
}
"#;
        let dep = parse(content).unwrap().remove(0);
        let released_at = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let versions = vec![
            VersionInfo::new("1.7.35", released_at),
            VersionInfo::new("1.7.36", released_at),
        ];

        let result = UpdateJudge::new(UpdateFilter::new()).judge(&dep, &versions);
        match result {
            UpdateResult::Update { new_version, .. } => {
                assert_eq!(new_version, "1.7.35");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn test_parse_rich_version_declaration_clears_previous_rejects() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            reject("1.7.36")
            prefer("1.7.25")
        }
    }
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].version_spec.rejected_versions.is_empty());
    }

    #[test]
    fn test_parse_multiple_dependencies() {
        let content = r#"
dependencies {
    implementation 'org.springframework:spring-core:5.3.23'
    implementation 'org.springframework:spring-web:5.3.23'
    testImplementation 'junit:junit:4.13.2'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);

        let prod_deps: Vec<_> = deps.iter().filter(|d| !d.is_dev).collect();
        let dev_deps: Vec<_> = deps.iter().filter(|d| d.is_dev).collect();

        assert_eq!(prod_deps.len(), 2);
        assert_eq!(dev_deps.len(), 1);
    }

    // 変数定義のテスト

    #[test]
    fn test_parse_groovy_variable() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.wicket:wicket-core");
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    #[test]
    fn test_update_variable_definition_preserves_crlf() {
        // 回帰: CRLF の build.gradle を変数定義経由で更新しても改行コードを保持する。
        // 以前は content.lines() + join("\n") が全行の \r を落として LF 化していた
        // (文字列記法経路は split_inclusive で CRLF を保持しており挙動が食い違っていた)。
        let content = "def wicketVersion = '1.2.3'\r\ndependencies {\r\n    implementation \"org.apache.wicket:wicket-core:$wicketVersion\"\r\n}\r\n";
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "9.12.0")
            .unwrap();
        assert!(result.contains("wicketVersion = '9.12.0'"));
        // すべての行末が CRLF のまま保持される (LF 化しない)
        assert_eq!(
            result.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!result.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_update_rich_version_block_preserves_crlf() {
        // 回帰: rich version ブロック経由の更新でも CRLF を保持する
        // (update_rich_version_block も以前は content.lines()+join で LF 化していた)。
        let content = "dependencies {\r\n    implementation('org.slf4j:slf4j-api') {\r\n        version {\r\n            strictly '1.7.36'\r\n        }\r\n    }\r\n}\r\n";
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "2.0.9")
            .unwrap();
        assert!(result.contains("strictly '2.0.9'"));
        assert_eq!(
            result.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!result.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_update_version_catalog_preserves_crlf() {
        // 回帰: version catalog (.versions.toml) の version.ref 経由更新でも CRLF を保持する。
        let content = "[versions]\r\njunit = \"4.13.2\"\r\n\r\n[libraries]\r\njunit-core = { module = \"junit:junit\", version.ref = \"junit\" }\r\n";
        let result = GradleParser
            .update_version(content, "junit:junit", "4.13.3")
            .unwrap();
        assert!(result.contains("junit = \"4.13.3\""));
        assert_eq!(
            result.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!result.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_update_string_notation_preserves_crlf() {
        // 回帰: 文字列記法依存 (リテラルバージョン) を CRLF の build.gradle で更新しても
        // 改行コードを保持する (replace_all_active_gradle_matches 経路)。
        let content = "dependencies {\r\n    implementation 'org.apache.commons:commons-lang3:3.12.0'\r\n}\r\n";
        let result = GradleParser
            .update_version(content, "org.apache.commons:commons-lang3", "3.14.0")
            .unwrap();
        assert!(result.contains("commons-lang3:3.14.0"));
        assert_eq!(
            result.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!result.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_parse_kotlin_variable() {
        let content = r#"
val wicketVersion = "9.12.0"

dependencies {
    implementation(group = "org.apache.wicket", name = "wicket-core", version = wicketVersion)
}
"#;
        // Kotlin DSL は構文が異なるが、変数抽出は機能する必要がある
        let parser = GradleParser;
        let vars = parser.extract_variables(content);
        assert_eq!(
            vars.get("wicketVersion").map(|v| v.value.as_str()),
            Some("9.12.0")
        );
    }

    #[test]
    fn test_parse_ext_block_variable() {
        let content = r#"
ext {
    springVersion = '5.3.23'
}

dependencies {
    implementation group: 'org.springframework', name: 'spring-core', version: springVersion
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "5.3.23");
    }

    #[test]
    fn test_parse_string_interpolation_variable() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation "org.apache.wicket:wicket-core:$wicketVersion"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    #[test]
    fn test_parse_string_interpolation_braces() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation "org.apache.wicket:wicket-core:${wicketVersion}"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "9.12.0");
    }

    // バージョン更新のテスト

    #[test]
    fn test_update_version_string_notation() {
        let content = r#"
dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("'org.apache.wicket:wicket-core:10.0.0'"));
    }

    #[test]
    fn test_update_string_notation_preserves_classifier_and_extension() {
        // classifier / extension は依存座標の一部なので、version だけを差し替えて維持する
        let content = r#"
dependencies {
    runtimeOnly("net.sf.docbook:docbook-xsl:1.75.2:resources@zip")
}
"#;
        let result = GradleParser
            .update_version(content, "net.sf.docbook:docbook-xsl", "1.76.0")
            .unwrap();
        assert!(result.contains(r#""net.sf.docbook:docbook-xsl:1.76.0:resources@zip""#));
    }

    #[test]
    fn test_update_string_notation_ignores_line_comment() {
        let content = r#"
dependencies {
    // implementation 'org.apache.wicket:wicket-core:9.12.0'
    implementation 'org.apache.wicket:wicket-core:9.13.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("// implementation 'org.apache.wicket:wicket-core:9.12.0'"));
        assert!(result.contains("implementation 'org.apache.wicket:wicket-core:10.0.0'"));
    }

    #[test]
    fn test_update_version_map_notation() {
        let content = r#"
dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: '9.12.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("version: '10.0.0'"));
    }

    #[test]
    fn test_update_kotlin_named_argument_map_notation() {
        let content = r#"
dependencies {
    implementation(group = "com.google.guava", name = "guava", version = "32.1.2-jre")
}
"#;
        let result = GradleParser
            .update_version(content, "com.google.guava:guava", "33.4.0-jre")
            .unwrap();
        assert!(result.contains(r#"version = "33.4.0-jre""#));
    }

    #[test]
    fn test_update_version_variable() {
        let content = r#"
def wicketVersion = '9.12.0'

dependencies {
    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0'"));
        // 元の変数参照は保持される
        assert!(result.contains("version: wicketVersion"));
    }

    #[test]
    fn test_update_version_ext_variable() {
        let content = r#"
ext {
    springVersion = '5.3.23'
}

dependencies {
    implementation group: 'org.springframework', name: 'spring-core', version: springVersion
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("springVersion = '6.0.0'"));
    }

    #[test]
    fn test_update_version_preserves_quote_style() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:5.3.23"
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("\"org.springframework:spring-core:6.0.0\""));
    }

    #[test]
    fn test_update_version_preserves_strict_notation() {
        let content = r#"
dependencies {
    implementation "org.springframework:spring-core:5.3.23!!"
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("\"org.springframework:spring-core:6.0.0!!\""));
    }

    #[test]
    fn test_parse_and_update_strict_prefix_version() {
        let content = r#"dependencies {
    implementation "org.springframework:spring-core:5.3.+!!"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(deps[0].version_spec.version, "5.3");

        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.1.2")
            .unwrap();
        assert!(result.contains("\"org.springframework:spring-core:6.1.+!!\""));
    }

    #[test]
    fn test_parse_and_update_strict_range_without_prefer() {
        let content = r#"dependencies {
    implementation "org.slf4j:slf4j-api:[1.7, 1.8[!!"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.version, "1.7");

        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains("\"org.slf4j:slf4j-api:[1.7.36, 1.8[!!\""));
    }

    #[test]
    fn test_update_string_notation_strict_range_with_prefer() {
        let content = r#"
dependencies {
    implementation "org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.25"
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains(r#""org.slf4j:slf4j-api:[1.7, 1.8[!!1.7.36""#));
    }

    #[test]
    fn test_update_rich_version_strictly() {
        let content = r#"
dependencies {
    implementation('org.slf4j:slf4j-api') {
        version {
            strictly '1.7.36'
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "2.0.17")
            .unwrap();
        assert!(result.contains("strictly '2.0.17'"));
    }

    #[test]
    fn test_update_rich_version_prefer_with_strict_range() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            strictly("[1.7, 1.8[")
            prefer("1.7.25")
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains(r#"strictly("[1.7, 1.8[")"#));
        assert!(result.contains(r#"prefer("1.7.36")"#));
    }

    #[test]
    fn test_update_rich_version_after_inline_block_comment() {
        let content = r#"
dependencies {
    implementation("org.slf4j:slf4j-api") {
        version {
            /* 現在の推奨版 */ prefer("1.7.25")
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains(r#"/* 現在の推奨版 */ prefer("1.7.36")"#));
    }

    #[test]
    fn test_parse_version_catalog_string_library() {
        let content = r#"
[libraries]
junit = "junit:junit:4.13.2"
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "junit:junit");
        assert_eq!(deps[0].version_spec.version, "4.13.2");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_version_catalog_table_and_version_ref() {
        let content = r#"
[versions]
groovy = "3.0.5"

[libraries]
groovy-core = { module = "org.codehaus.groovy:groovy", version.ref = "groovy" }
commons-lang3 = { group = "org.apache.commons", name = "commons-lang3", version = "3.12.0" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let groovy = deps
            .iter()
            .find(|dep| dep.name == "org.codehaus.groovy:groovy")
            .unwrap();
        assert_eq!(groovy.version_spec.version, "3.0.5");
        assert_eq!(groovy.variable_name.as_deref(), Some("groovy"));

        let commons = deps
            .iter()
            .find(|dep| dep.name == "org.apache.commons:commons-lang3")
            .unwrap();
        assert_eq!(commons.version_spec.version, "3.12.0");
        assert_eq!(commons.variable_name, None);
    }

    #[test]
    fn test_parse_version_catalog_rich_version() {
        let content = r#"
[libraries]
commons-lang3 = { group = "org.apache.commons", name = "commons-lang3", version = { strictly = "[3.8, 4.0[", prefer = "3.9", reject = ["3.10"] } }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.apache.commons:commons-lang3");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.raw, "[3.8, 4.0[");
        assert_eq!(deps[0].version_spec.version, "3.9");
        assert_eq!(deps[0].version_spec.rejected_versions, vec!["3.10"]);
    }

    #[test]
    fn test_update_version_catalog_strict_dynamic_version() {
        let content = r#"
[versions]
spring = { strictly = "5.3.+" }

[libraries]
spring-core = { module = "org.springframework:spring-core", version.ref = "spring" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Wildcard);
        assert_eq!(deps[0].version_spec.version, "5.3");

        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.1.2")
            .unwrap();
        assert!(result.contains(r#"strictly = "6.1.+""#));
    }

    #[test]
    fn test_parse_version_catalog_skips_plugins() {
        let content = r#"
[plugins]
versions = { id = "com.github.ben-manes.versions", version = "0.45.0" }
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_version_catalog_invalid_toml_returns_error() {
        let content = r#"
[libraries]
junit = { module = "junit:junit", version = "4.13.2"
"#;
        let err = parse(content).unwrap_err();
        assert!(matches!(err, ManifestError::TomlParseError { .. }));
    }

    #[test]
    fn test_update_version_catalog_string_library() {
        let content = r#"
[libraries]
junit = "junit:junit:4.13.2"
"#;
        let result = GradleParser
            .update_version(content, "junit:junit", "4.13.3")
            .unwrap();
        assert!(result.contains(r#"junit = "junit:junit:4.13.3""#));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_update_version_catalog_direct_table_version() {
        let content = r#"
[libraries]
commons-lang3 = { group = "org.apache.commons", name = "commons-lang3", version = "3.12.0" }
"#;
        let result = GradleParser
            .update_version(content, "org.apache.commons:commons-lang3", "3.14.0")
            .unwrap();
        assert!(result.contains(r#"version = "3.14.0""#));
    }

    #[test]
    fn test_update_version_catalog_version_ref() {
        let content = r#"
[versions]
groovy = "3.0.5"

[libraries]
groovy-core = { module = "org.codehaus.groovy:groovy", version.ref = "groovy" }
"#;
        let result = GradleParser
            .update_version(content, "org.codehaus.groovy:groovy", "3.0.6")
            .unwrap();
        assert!(result.contains(r#"groovy = "3.0.6""#));
        assert!(result.contains(r#"version.ref = "groovy""#));
    }

    #[test]
    fn test_update_version_catalog_rich_version_ref_updates_prefer() {
        let content = r#"
[versions]
commons = { strictly = "[3.8, 4.0[", prefer = "3.9" }

[libraries]
commons-lang3 = { group = "org.apache.commons", name = "commons-lang3", version.ref = "commons" }
"#;
        let result = GradleParser
            .update_version(content, "org.apache.commons:commons-lang3", "3.13.0")
            .unwrap();
        assert!(result.contains(r#"strictly = "[3.8, 4.0[""#));
        assert!(result.contains(r#"prefer = "3.13.0""#));
    }

    #[test]
    fn test_update_version_catalog_multiline_library_version() {
        let content = r#"
[libraries]
commons-lang3 = {
    group = "org.apache.commons",
    name = "commons-lang3",
    version = "3.12.0"
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.commons:commons-lang3", "3.14.0")
            .unwrap();
        assert!(result.contains(r#"    version = "3.14.0""#));
    }

    #[test]
    fn test_update_version_catalog_library_table_section() {
        let content = r#"
[libraries.commons-lang3]
group = "org.apache.commons"
name = "commons-lang3"
version = "3.12.0"
"#;
        let result = GradleParser
            .update_version(content, "org.apache.commons:commons-lang3", "3.14.0")
            .unwrap();
        assert!(result.contains(r#"version = "3.14.0""#));
    }

    #[test]
    fn test_update_version_catalog_version_table_section() {
        let content = r#"
[versions.commons]
strictly = "[3.8, 4.0["
prefer = "3.9"

[libraries]
commons-lang3 = { group = "org.apache.commons", name = "commons-lang3", version.ref = "commons" }
"#;
        let result = GradleParser
            .update_version(content, "org.apache.commons:commons-lang3", "3.13.0")
            .unwrap();
        assert!(result.contains(r#"strictly = "[3.8, 4.0[""#));
        assert!(result.contains(r#"prefer = "3.13.0""#));
    }

    #[test]
    fn test_update_variable_preserves_strict_notation() {
        let content = r#"
def springVersion = '5.3.23!!'

dependencies {
    implementation "org.springframework:spring-core:$springVersion"
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("def springVersion = '6.0.0!!'"));
    }

    #[test]
    fn test_update_variable_preserves_trailing_newline() {
        // 変数定義の更新で末尾改行を保持する
        let content = "def wicketVersion = '9.12.0'\n\ndependencies {\n    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion\n}\n";
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0'"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_update_variable_no_trailing_newline() {
        // 末尾改行がないファイルは付けない
        let content = "def wicketVersion = '9.12.0'\n\ndependencies {\n    implementation group: 'org.apache.wicket', name: 'wicket-core', version: wicketVersion\n}";
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0'"));
        assert!(!result.ends_with('\n'));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"
dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let result = GradleParser.update_version(content, "nonexistent:package", "1.0.0");
        assert!(result.is_err());
    }

    // エッジケースのテスト

    #[test]
    fn test_parse_empty() {
        let deps = parse("").unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_comments_only() {
        let content = r#"
// これはコメントです
// implementation 'commented:out:1.0.0'
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_version_with_suffix() {
        let content = r#"
dependencies {
    implementation 'org.springframework:spring-core:5.3.23.RELEASE'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "5.3.23.RELEASE");
    }

    #[test]
    fn test_parse_snapshot_version() {
        let content = r#"
dependencies {
    implementation 'com.example:my-lib:1.0.0-SNAPSHOT'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.version, "1.0.0-SNAPSHOT");
    }

    #[test]
    fn test_gradle_parser_language() {
        let parser = GradleParser;
        assert_eq!(parser.language(), Language::Java);
    }

    // 実運用に近い例のテスト
    #[test]
    fn test_parse_realistic_build_gradle() {
        let content = r#"
plugins {
    id 'java'
    id 'org.springframework.boot' version '3.0.0'
}

def lombokVersion = '1.18.24'
def junitVersion = '5.9.0'

ext {
    springVersion = '6.0.0'
}

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter-web:3.0.0'
    implementation group: 'org.projectlombok', name: 'lombok', version: lombokVersion
    implementation "org.springframework:spring-core:$springVersion"

    testImplementation 'org.junit.jupiter:junit-jupiter-api:5.9.0'
    testImplementation "org.junit.jupiter:junit-jupiter-engine:${junitVersion}"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        // 主要な依存関係を確認
        let spring_boot = deps
            .iter()
            .find(|d| d.name.contains("spring-boot-starter-web"));
        assert!(spring_boot.is_some());
        assert_eq!(spring_boot.unwrap().version_spec.version, "3.0.0");

        let lombok = deps.iter().find(|d| d.name.contains("lombok"));
        assert!(lombok.is_some());
        assert_eq!(lombok.unwrap().version_spec.version, "1.18.24");

        let spring_core = deps.iter().find(|d| d.name.contains("spring-core"));
        assert!(spring_core.is_some());
        assert_eq!(spring_core.unwrap().version_spec.version, "6.0.0");

        let test_deps: Vec<_> = deps.iter().filter(|d| d.is_dev).collect();
        assert_eq!(test_deps.len(), 2);
    }

    // --- 追加エッジケーステスト ---

    #[test]
    fn test_parse_kotlin_dsl_parenthesized_string() {
        // Kotlin DSL: implementation("group:name:version") 形式
        let content = r#"
dependencies {
    implementation("com.google.guava:guava:32.1.2-jre")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
        assert!(!deps[0].is_dev);
    }

    #[test]
    fn test_parse_test_implementation_string_notation() {
        // testImplementation の文字列記法が開発依存として判定されること
        let content = r#"
dependencies {
    testImplementation 'org.mockito:mockito-core:5.5.0'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.mockito:mockito-core");
        assert_eq!(deps[0].version_spec.version, "5.5.0");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_variable_reference_in_string_interpolation() {
        // 変数参照を文字列展開で使用するケース
        let content = r#"
def guavaVersion = '32.1.2-jre'

dependencies {
    implementation "com.google.guava:guava:$guavaVersion"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        // 変数が解決されて実際のバージョンになること
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
    }

    #[test]
    fn test_parse_platform_dependency() {
        // 回帰テスト: BOM は `platform(...)` / `enforcedPlatform(...)` で宣言するのが
        // Gradle の標準。これを取りこぼすと推移依存のバージョンを一括決定する宣言だけが
        // 無言で更新対象外になっていた。configuration 名 (dev 判定) も保持する。
        let content = r#"
dependencies {
    implementation platform('com.google.cloud:libraries-bom:26.1.0')
    implementation(platform("org.springframework.boot:spring-boot-dependencies:3.2.0"))
    testImplementation(platform("org.junit:junit-bom:5.10.0"))
    implementation enforcedPlatform("io.netty:netty-bom:4.1.100.Final")
}
"#;
        let deps = parse(content).unwrap();

        let bom = deps
            .iter()
            .find(|d| d.name == "com.google.cloud:libraries-bom")
            .expect("platform 依存が検出されるべき");
        assert!(!bom.is_dev);
        assert_eq!(bom.version_spec.version, "26.1.0");

        let boot = deps
            .iter()
            .find(|d| d.name == "org.springframework.boot:spring-boot-dependencies")
            .expect("括弧付き platform 依存が検出されるべき");
        assert_eq!(boot.version_spec.version, "3.2.0");

        let junit = deps
            .iter()
            .find(|d| d.name == "org.junit:junit-bom")
            .expect("testImplementation の platform 依存が検出されるべき");
        assert!(junit.is_dev, "configuration 名が platform に潰されていない");

        let netty = deps
            .iter()
            .find(|d| d.name == "io.netty:netty-bom")
            .expect("enforcedPlatform 依存が検出されるべき");
        assert_eq!(netty.version_spec.version, "4.1.100.Final");
    }

    /// platform 依存も文字列記法と同じ経路で書き戻せること (report/apply の整合)。
    #[test]
    fn test_update_platform_dependency() {
        let content = r#"
dependencies {
    implementation platform('com.google.cloud:libraries-bom:26.1.0')
}
"#;
        let result = GradleParser
            .update_version(content, "com.google.cloud:libraries-bom", "26.30.0")
            .unwrap();
        assert!(
            result.contains("platform('com.google.cloud:libraries-bom:26.30.0')"),
            "{}",
            result
        );
    }

    /// 回帰テスト: `ext.<name> = '...'` のドット代入も変数として解決・書き戻しできること。
    /// `ext { ... }` ブロック形式と意味が同じなのに挙動が割れていた。
    #[test]
    fn test_ext_dot_variable_parse_and_update() {
        let content = r#"
ext.springVersion = '5.3.23'

dependencies {
    implementation "org.springframework:spring-core:$springVersion"
}
"#;
        let deps = parse(content).unwrap();
        let dep = deps
            .iter()
            .find(|d| d.name == "org.springframework:spring-core")
            .expect("ext.<name> 参照の依存が検出されるべき");
        assert_eq!(dep.version_spec.version, "5.3.23");

        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.1.0")
            .unwrap();
        assert!(result.contains("ext.springVersion = '6.1.0'"), "{}", result);
    }

    /// 回帰テスト: `${Versions.x}` のような修飾付き変数参照も最終セグメントで解決する。
    /// 解決に必要な定義が同一ファイル内にあるのに依存ごと落としていた。
    #[test]
    fn test_qualified_variable_reference_parse_and_update() {
        let content = r#"
object Versions {
    const val retrofit = "2.9.0"
}

dependencies {
    implementation("com.squareup.retrofit2:retrofit:${Versions.retrofit}")
}
"#;
        let deps = parse(content).unwrap();
        let dep = deps
            .iter()
            .find(|d| d.name == "com.squareup.retrofit2:retrofit")
            .expect("修飾付き変数参照の依存が検出されるべき");
        assert_eq!(dep.version_spec.version, "2.9.0");

        let result = GradleParser
            .update_version(content, "com.squareup.retrofit2:retrofit", "2.11.0")
            .unwrap();
        assert!(
            result.contains(r#"const val retrofit = "2.11.0""#),
            "{}",
            result
        );
    }

    /// 同じ短名が異なる値で複数定義されている場合、修飾付き参照は解決しない
    /// (別オブジェクトの値を拾う誤更新を防ぐ安全側のスキップ)。
    #[test]
    fn test_qualified_variable_reference_ambiguous_short_name_skipped() {
        let content = r#"
object Versions {
    const val retrofit = "2.9.0"
}
object Legacy {
    const val retrofit = "1.9.0"
}

dependencies {
    implementation("com.squareup.retrofit2:retrofit:${Versions.retrofit}")
}
"#;
        let deps = parse(content).unwrap();
        assert!(
            !deps
                .iter()
                .any(|d| d.name == "com.squareup.retrofit2:retrofit"),
            "曖昧な短名は解決せずスキップされるべき: {:?}",
            deps.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
    }

    // --- コメント除去の文字列リテラル対応 (回帰テスト) ---

    #[test]
    fn test_update_after_string_containing_block_comment_marker() {
        // 'META-INF/*.kotlin_module' の /* が閉じないブロックコメント開始と誤認されると、
        // 以降の全依存の更新が not found で失敗していた
        let content = r#"
android {
    packagingOptions {
        exclude 'META-INF/*.kotlin_module'
    }
}

dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("exclude 'META-INF/*.kotlin_module'"));
        assert!(result.contains("implementation 'org.apache.wicket:wicket-core:10.0.0'"));
    }

    #[test]
    fn test_update_preserves_url_string_with_double_slash() {
        // 文字列リテラル内の // を行コメントとして切り詰めない
        let content = r#"
repositories {
    maven { url 'https://repo.example.com/maven2' }
}

dependencies {
    implementation 'org.apache.wicket:wicket-core:9.12.0'
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("maven { url 'https://repo.example.com/maven2' }"));
        assert!(result.contains("implementation 'org.apache.wicket:wicket-core:10.0.0'"));
    }

    // --- 同一座標の複数宣言の一括更新 (回帰テスト) ---

    #[test]
    fn test_update_all_declarations_of_same_coordinate() {
        // compileOnly + annotationProcessor のような同一座標の複数宣言を全て更新する
        let content = r#"
dependencies {
    compileOnly 'org.projectlombok:lombok:1.18.30'
    annotationProcessor 'org.projectlombok:lombok:1.18.30'
}
"#;
        let result = GradleParser
            .update_version(content, "org.projectlombok:lombok", "1.18.36")
            .unwrap();
        assert!(result.contains("compileOnly 'org.projectlombok:lombok:1.18.36'"));
        assert!(result.contains("annotationProcessor 'org.projectlombok:lombok:1.18.36'"));
        assert!(!result.contains("1.18.30"));
    }

    #[test]
    fn test_update_all_declarations_formats_each_from_own_value() {
        // 出現ごとに旧バージョン・クォート・strict 表記が違っても、それぞれ自身の形式を保って更新する
        let content = r#"
dependencies {
    compileOnly 'org.projectlombok:lombok:1.18.28'
    annotationProcessor "org.projectlombok:lombok:1.18.30!!"
}
"#;
        let result = GradleParser
            .update_version(content, "org.projectlombok:lombok", "1.18.36")
            .unwrap();
        assert!(result.contains("compileOnly 'org.projectlombok:lombok:1.18.36'"));
        assert!(result.contains(r#"annotationProcessor "org.projectlombok:lombok:1.18.36!!""#));
    }

    // --- 変数定義更新のインデント・行末コメント保持 (回帰テスト) ---

    #[test]
    fn test_update_ext_variable_preserves_indent_and_trailing_comment() {
        let content = r#"
ext {
    springVersion = '5.3.23' // managed by depup
}

dependencies {
    implementation group: 'org.springframework', name: 'spring-core', version: springVersion
}
"#;
        let result = GradleParser
            .update_version(content, "org.springframework:spring-core", "6.0.0")
            .unwrap();
        assert!(result.contains("    springVersion = '6.0.0' // managed by depup"));
    }

    #[test]
    fn test_update_def_variable_preserves_trailing_comment() {
        let content = r#"
def wicketVersion = '9.12.0' // keep in sync with parent
dependencies {
    implementation "org.apache.wicket:wicket-core:$wicketVersion"
}
"#;
        let result = GradleParser
            .update_version(content, "org.apache.wicket:wicket-core", "10.0.0")
            .unwrap();
        assert!(result.contains("def wicketVersion = '10.0.0' // keep in sync with parent"));
    }

    // --- /* */ ブロックコメント内の宣言の無視 (回帰テスト) ---

    #[test]
    fn test_parse_skips_block_commented_dependency_declaration() {
        // 宣言全体がコメントアウトされた rich version ブロックは parse 対象にしない
        let content = r#"
dependencies {
    /*
    implementation("org.slf4j:slf4j-api") {
        version {
            prefer("1.7.25")
        }
    }
    */
    implementation 'junit:junit:4.13.2'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "junit:junit");
    }

    #[test]
    fn test_update_skips_block_commented_rich_version_block() {
        // コメントアウトされたブロック内の prefer は書き換えず、生きている宣言だけを更新する
        let content = r#"
dependencies {
    /*
    implementation("org.slf4j:slf4j-api") {
        version {
            prefer("1.7.20")
        }
    }
    */
    implementation("org.slf4j:slf4j-api") {
        version {
            prefer("1.7.25")
        }
    }
}
"#;
        let result = GradleParser
            .update_version(content, "org.slf4j:slf4j-api", "1.7.36")
            .unwrap();
        assert!(result.contains(r#"prefer("1.7.20")"#));
        assert!(result.contains(r#"prefer("1.7.36")"#));
        assert!(!result.contains(r#"prefer("1.7.25")"#));
    }

    #[test]
    fn test_extract_variables_ignores_block_commented_definition() {
        // ブロックコメント内の変数定義は抽出せず、生きている定義を上書きしない
        let content = r#"
def wicketVersion = '9.12.0'
/*
def wicketVersion = '1.0.0'
def junitVersion = '4.0.0'
*/
"#;
        let parser = GradleParser;
        let vars = parser.extract_variables(content);
        assert_eq!(
            vars.get("wicketVersion").map(|v| v.value.as_str()),
            Some("9.12.0")
        );
        assert!(!vars.contains_key("junitVersion"));
    }

    // --- repositories の URL の誤検出防止 (回帰テスト) ---

    #[test]
    fn test_parse_skips_repository_url() {
        // maven { url 'http://nexus:8081' } の url 行を依存として誤検出しない
        let content = r#"
repositories {
    maven {
        url 'http://nexus:8081'
    }
    maven {
        url "https://repo.example.com:8443/maven2"
    }
}

dependencies {
    implementation 'junit:junit:4.13.2'
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "junit:junit");
    }

    #[test]
    fn test_parse_kotlin_typed_variable() {
        // Kotlin DSL の型注釈付き変数 (val x: String = "...") も解決する
        let content = r#"
val guavaVersion: String = "32.1.2-jre"
dependencies {
    implementation("com.google.guava:guava:$guavaVersion")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
    }

    #[test]
    fn test_parse_groovy_typed_variable() {
        // Groovy の型宣言付き変数 (String x = '...') も解決する
        let content = r#"
String guavaVersion = '32.1.2-jre'
dependencies {
    implementation "com.google.guava:guava:${guavaVersion}"
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
    }

    #[test]
    fn test_parse_kotlin_const_val_variable() {
        // Kotlin DSL で最も一般的な `const val` 形式のバージョン定義も解決する
        let content = r#"
const val guavaVersion = "32.1.2-jre"
dependencies {
    implementation("com.google.guava:guava:$guavaVersion")
}
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "com.google.guava:guava");
        assert_eq!(deps[0].version_spec.version, "32.1.2-jre");
    }

    #[test]
    fn test_update_kotlin_const_val_variable() {
        // `const val` 定義の値が更新され、依存行の補間はそのまま保持される
        let content = r#"
const val guavaVersion = "32.1.2-jre"
dependencies {
    implementation("com.google.guava:guava:$guavaVersion")
}
"#;
        let updated = GradleParser
            .update_version(content, "com.google.guava:guava", "33.0.0-jre")
            .unwrap();
        assert!(updated.contains(r#"const val guavaVersion = "33.0.0-jre""#));
        // 依存宣言行は変数参照のまま維持される
        assert!(updated.contains(r#"implementation("com.google.guava:guava:$guavaVersion")"#));
    }

    #[test]
    fn test_parse_unresolved_variable_is_skipped() {
        // depup が追跡しない場所 (gradle.properties / by extra / 計算値) で定義された変数は
        // 解決できない。空の Any 依存を作って judge が更新を報告した後に writer が失敗する
        // 「報告したのに適用エラー」を避けるため、依存ごとスキップする。
        let content = r#"
dependencies {
    implementation("com.google.guava:guava:$undefinedVersion")
}
"#;
        let deps = parse(content).unwrap();
        assert!(
            deps.is_empty(),
            "未解決変数の依存は空の Any として残さずスキップされる"
        );
    }
}
