//! Rust プロジェクト向けの `Cargo.toml` パーサ。
//!
//! 対応対象:
//! - `dependencies` セクション
//! - `dev-dependencies` セクション
//! - `build-dependencies` セクション
//! - `workspace.dependencies`（ワークスペースルート）
//! - inline table 形式: `{ version = "1.0" }`
//! - workspace 依存関係

use crate::domain::git_urls_match;
use crate::domain::{Dependency, GitReference, GitSource, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::{
    ManifestParser,
    line_utils::{
        assignment_value_offset, opens_unclosed_multiline_string, parse_assignment_key_path,
        parse_toml_section_header, quoted_value_span, split_line_ending, split_toml_key_path,
        toml_inline_table_end, toml_inline_table_string_value,
    },
};
use crate::parser::{VersionParser, get_parser};
use std::collections::HashSet;
use std::path::PathBuf;
use toml::Value;

/// `Cargo.toml` 用パーサ
pub struct CargoTomlParser;

/// 依存の種類のテーブル名 (target 固有のテーブルの末尾にも使う)
fn is_dependency_kind(key: &str) -> bool {
    matches!(
        key,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    )
}

/// 依存の宣言を持つテーブルの論理パスか。parse が読むテーブル (`dependencies`・
/// `dev-dependencies`・`build-dependencies`・`workspace.dependencies`・
/// `target.<cfg>.<種類>`) と同じ
fn is_dependency_table(table: &[String]) -> bool {
    match table {
        [kind] => is_dependency_kind(kind),
        [workspace, kind] => workspace == "workspace" && kind == "dependencies",
        [target, _, kind] => target == "target" && is_dependency_kind(kind),
        _ => false,
    }
}

/// `[patch.<registry>]` のテーブルなら registry を返す
fn patch_table_registry(table: &[String]) -> Option<&str> {
    match table {
        [patch, registry] if patch == "patch" && !registry.is_empty() => Some(registry),
        _ => None,
    }
}

/// `[patch.<registry>]` で `package` を、依存セクションに書いた `package` の `git` のどれとも違う
/// リポジトリ (fork など) へ差し替えている registry の一覧。
///
/// 新しい tag は依存の URL (upstream) の `git ls-remote` で決めたものなので、別のリポジトリを指す
/// patch の tag は書き換えない。以前は URL を照合せずに書き換え、fork に無い tag を書いて patch を
/// 壊していた (#160)。TOML として読めなければ空 (どの patch も別のリポジトリとはみなさない)。
fn foreign_git_patch_registries(content: &str, package: &str) -> HashSet<String> {
    // `patch` の無いファイルに patch の宣言は無い。tag の更新のたびに全体を TOML として読まない
    if !content.contains("patch") {
        return HashSet::new();
    }
    let Ok(toml) = toml::from_str::<Value>(content) else {
        return HashSet::new();
    };
    fn git_of<'a>(dependencies: Option<&'a Value>, package: &str) -> Option<&'a str> {
        dependencies?.get(package)?.get("git")?.as_str()
    }
    const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut upstream: Vec<&str> = SECTIONS
        .iter()
        .filter_map(|section| git_of(toml.get(*section), package))
        .collect();
    if let Some(targets) = toml.get("target").and_then(Value::as_table) {
        for config in targets.values() {
            upstream.extend(
                SECTIONS
                    .iter()
                    .filter_map(|section| git_of(config.get(*section), package)),
            );
        }
    }
    upstream.extend(git_of(
        toml.get("workspace")
            .and_then(|workspace| workspace.get("dependencies")),
        package,
    ));
    // 同じファイルに依存の宣言が無ければ取得元を照合できないので、従来どおり書き換える
    if upstream.is_empty() {
        return HashSet::new();
    }
    toml.get("patch")
        .and_then(Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(registry, patches)| {
            let url = patches.get(package)?.get("git")?.as_str()?;
            (!upstream
                .iter()
                .any(|upstream| git_urls_match(url, upstream)))
            .then(|| registry.clone())
        })
        .collect()
}

/// inline table が crates.io 以外 (path / 別レジストリ) を指すか。parse と同じく
/// inline table の直下のキーだけを見る。
fn inline_table_declares_non_crates_io_source(content: &str, open: usize, end: usize) -> bool {
    let string = |key: &str| {
        toml_inline_table_string_value(content, open, end, key)
            .map(|(start, end)| &content[start + 1..end - 1])
    };
    string("path").is_some()
        || string("registry-index").is_some()
        || string("registry").is_some_and(|registry| registry != "crates-io")
}

/// `end` を含む行の終わり (改行の次) の位置
fn line_end_after(content: &str, end: usize) -> usize {
    content[end..]
        .find('\n')
        .map_or(content.len(), |offset| end + offset + 1)
}

/// Cargo.toml の中の、対象パッケージの宣言の 1 か所 (#168)。
///
/// セクションの見出しと行頭のキーを TOML のキーとして読み、その論理パスで照合する
/// (`[dependencies."ryu"]` の `version = ...` → `dependencies.ryu.version`、
/// `[target.'cfg(windows)']` の `dependencies.cfg-if = ...` →
/// `target.cfg(windows).dependencies.cfg-if`)。parse (toml クレート) が読む宣言と同じものを
/// 書き換えるため。以前は名前とセクション名を文字どおりに照合していたので、引用符付きのキー・
/// 親のテーブルの dotted key・点の前後の空白で書いた依存を、報告した後に書き換えられなかった。
struct CargoSite {
    /// 宣言を持つテーブル (`dependencies`・`target.<cfg>.dependencies`・`patch.<registry>` など)
    table: Vec<String>,
    /// 宣言の中のキー。`None` は `package = <値>` (文字列か inline table) そのもの
    field: Option<String>,
    /// 宣言の行の開始位置 (content の中のバイト位置)
    line_start: usize,
    /// 値の開始位置 (content の中のバイト位置)
    value_start: usize,
}

impl CargoSite {
    /// 論理パスが `package` の宣言 (`<テーブル>.package` か `<テーブル>.package.<キー>`) なら作る
    fn at(
        mut path: Vec<String>,
        package: &str,
        line_start: usize,
        value_start: usize,
    ) -> Option<Self> {
        let declares =
            |table: &[String]| is_dependency_table(table) || patch_table_registry(table).is_some();
        let len = path.len();
        if len >= 2 && path[len - 1] == package && declares(&path[..len - 1]) {
            path.pop();
            return Some(Self {
                table: path,
                field: None,
                line_start,
                value_start,
            });
        }
        if len >= 3 && path[len - 2] == package && declares(&path[..len - 2]) {
            let field = path.pop();
            path.pop();
            return Some(Self {
                table: path,
                field,
                line_start,
                value_start,
            });
        }
        None
    }

    /// 値が inline table なら、その `{` の位置と閉じる `}` の次の位置
    fn inline_table(&self, content: &str) -> Option<(usize, usize)> {
        if self.field.is_some() || !content[self.value_start..].starts_with('{') {
            return None;
        }
        Some((
            self.value_start,
            toml_inline_table_end(content, self.value_start)?,
        ))
    }

    /// 値の行にある文字列の値の範囲 (引用符の内側)
    fn own_string_span(&self, content: &str) -> Option<(usize, usize)> {
        let line_end = content[self.line_start..]
            .find('\n')
            .map_or(content.len(), |offset| self.line_start + offset);
        let (line, _) = split_line_ending(&content[self.line_start..line_end]);
        quoted_value_span(line, self.value_start - self.line_start)
            .map(|(start, end)| (self.line_start + start, self.line_start + end))
    }

    /// この宣言の `key` (`version` / `tag` など) の文字列の範囲 (引用符の内側)。
    /// `package = "1.0"` の文字列は `version` の値として扱う
    fn string_span(&self, content: &str, key: &str) -> Option<(usize, usize)> {
        match &self.field {
            Some(field) if field == key => self.own_string_span(content),
            Some(_) => None,
            None => match self.inline_table(content) {
                Some((open, end)) => toml_inline_table_string_value(content, open, end, key)
                    .map(|(start, end)| (start + 1, end - 1)),
                None if key == "version" => self.own_string_span(content),
                None => None,
            },
        }
    }

    /// 宣言の直下のキーが crates.io 以外 (path / registry-index / crates-io 以外の registry) を
    /// 指すか。parse と同じく宣言の直下のキーだけを見る
    fn declares_non_crates_io_source(&self, content: &str) -> bool {
        match self.field.as_deref() {
            Some("path" | "registry-index") => true,
            Some("registry") => self
                .own_string_span(content)
                .is_none_or(|(start, end)| &content[start..end] != "crates-io"),
            Some(_) => false,
            None => self.inline_table(content).is_some_and(|(open, end)| {
                inline_table_declares_non_crates_io_source(content, open, end)
            }),
        }
    }
}

/// `content` の中の、`package` の宣言の場所を、書かれた順に返す。
///
/// 複数行の文字列と、複数行の inline table の内側の行は、宣言の行として読まない。
fn cargo_sites(content: &str, package: &str) -> Vec<CargoSite> {
    let mut sites = Vec::new();
    // 今の見出しのキーと、その論理パス (対象の行があったときだけ読む)
    let mut header = "";
    let mut header_names_package = false;
    let mut section: Option<Vec<String>> = None;
    // 複数行の文字列は、ファイルに区切りが無ければ行ごとに探さない
    let may_have_multiline_strings = content.contains("\"\"\"") || content.contains("'''");
    let mut multiline_delim: Option<&str> = None;
    let mut skip_until = 0usize;
    let mut line_start = 0usize;
    for segment in content.split_inclusive('\n') {
        let start = line_start;
        line_start += segment.len();
        let (line, _) = split_line_ending(segment);
        if let Some(delim) = multiline_delim {
            if line.contains(delim) {
                multiline_delim = None;
            }
            continue;
        }
        if start < skip_until {
            continue;
        }
        if let Some(key) = parse_toml_section_header(line) {
            header = key;
            header_names_package = key.contains(package);
            section = None;
            continue;
        }
        if may_have_multiline_strings && let Some(delim) = opens_unclosed_multiline_string(line) {
            multiline_delim = Some(delim);
            continue;
        }
        // 複数行の inline table の内側の行を読み飛ばすため、`{` が `}` より多い行 (行の中で
        // 閉じない inline table を開いた行) は値の形を見る。文字列の中の `}` で数が釣り合う形は
        // 1 行で閉じるものとみなす (正確に調べると、inline table の多いファイルで書き換えが
        // 2 倍以上遅くなる)
        if count_byte(line, b'{') > count_byte(line, b'}')
            && let Some(value_offset) = assignment_value_offset(line)
            && content[start + value_offset..].starts_with('{')
            && let Some(end) = toml_inline_table_end(content, start + value_offset)
        {
            skip_until = line_end_after(content, end);
        }
        // キーを読むのは、パッケージ名を書いた行と、パッケージ名を書いた見出しの本文の行だけ
        // (`[dependencies.<pkg>]` の本文は名前を書かない)。全部の行のキーを文字列にすると、
        // 依存の数に比例して確保が増える
        if !(header_names_package || line.contains(package)) {
            continue;
        }
        let Some((keys, value_offset)) = parse_assignment_key_path(line) else {
            continue;
        };
        // 読めない見出しは、どの依存のテーブルにも当たらない空のパスにする
        let mut path = section
            .get_or_insert_with(|| split_toml_key_path(header).unwrap_or_default())
            .clone();
        path.extend(keys);
        if let Some(site) = CargoSite::at(path, package, start, start + value_offset) {
            sites.push(site);
        }
    }
    sites
}

fn count_byte(line: &str, byte: u8) -> usize {
    line.bytes().filter(|&candidate| candidate == byte).count()
}

/// 範囲 (start, end) を新しい文字列へ置き換えた全体を返す。置き換えが無ければ `None`
fn apply_replacements(
    content: &str,
    mut replacements: Vec<(usize, usize, String)>,
) -> Option<String> {
    if replacements.is_empty() {
        return None;
    }
    replacements.sort_by_key(|(start, _, _)| *start);
    let mut result = String::with_capacity(content.len());
    let mut copied = 0;
    for (start, end, value) in replacements {
        result.push_str(&content[copied..start]);
        result.push_str(&value);
        copied = end;
    }
    result.push_str(&content[copied..]);
    Some(result)
}

impl ManifestParser for CargoTomlParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        let toml: Value = toml::from_str(content).map_err(|e: toml::de::Error| {
            ManifestError::TomlParseError {
                path: PathBuf::from("Cargo.toml"),
                message: e.to_string(),
            }
        })?;

        let mut dependencies = CargoDeclarations::default();
        let parser = get_parser(Language::Rust);

        // 通常の依存関係を読む
        if let Some(deps) = toml.get("dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
        }

        // 開発依存を読む
        if let Some(deps) = toml.get("dev-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
        }

        // build-dependencies は開発依存として扱う
        if let Some(deps) = toml.get("build-dependencies").and_then(|d| d.as_table()) {
            parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
        }

        // target 固有依存を読む
        if let Some(target) = toml.get("target").and_then(|t| t.as_table()) {
            for (_target_name, target_config) in target {
                if let Some(deps) = target_config.get("dependencies").and_then(|d| d.as_table()) {
                    parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
                }
                if let Some(deps) = target_config
                    .get("dev-dependencies")
                    .and_then(|d| d.as_table())
                {
                    parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
                }
                if let Some(deps) = target_config
                    .get("build-dependencies")
                    .and_then(|d| d.as_table())
                {
                    parse_cargo_dependencies(deps, parser.as_ref(), true, &mut dependencies);
                }
            }
        }

        // ワークスペースルートの `workspace.dependencies` を読む
        if let Some(workspace) = toml.get("workspace").and_then(|w| w.as_table())
            && let Some(deps) = workspace.get("dependencies").and_then(|d| d.as_table())
        {
            parse_cargo_dependencies(deps, parser.as_ref(), false, &mut dependencies);
        }

        Ok(dependencies.dependencies)
    }

    fn language(&self) -> Language {
        Language::Rust
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        // Cargo 警告を避けるため semver の build metadata (`+...`) は落とす。
        // 判定と `--diff` も同じ `Language::manifest_version` を使う (#157)
        let new_version = Language::Rust.manifest_version(new_version);
        let parser = get_parser(Language::Rust);

        // 依存の宣言を、単純な指定・inline table (複数行も)・dotted key・`[dependencies.<pkg>]`
        // テーブルのどれで書いても、parse と同じ論理パスで探す (#168)。同名のクレートが複数の
        // 依存のテーブルにある場合に備えて、全部の宣言を書き換える (各宣言は自身の旧値から整形する)。
        let sites = cargo_sites(content, package);
        // path / 別レジストリを指す宣言は parse も依存として拾わないので、そのテーブルの宣言は
        // 書き換えない (`version` より後ろに `path` が書かれることもあるので先に集める)
        let excluded: Vec<&[String]> = sites
            .iter()
            .filter(|site| site.declares_non_crates_io_source(content))
            .map(|site| site.table.as_slice())
            .collect();
        let replacements = sites
            .iter()
            .filter(|site| {
                is_dependency_table(&site.table) && !excluded.contains(&site.table.as_slice())
            })
            .filter_map(|site| {
                let (start, end) = site.string_span(content, "version")?;
                let new_value = parser
                    .parse(&content[start..end])?
                    .try_format_updated(new_version)?;
                Some((start, end, new_value))
            })
            .collect();

        apply_replacements(content, replacements).ok_or_else(|| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Cargo.toml"),
            spec: package.to_string(),
            message: "package not found or version could not be updated".to_string(),
        })
    }

    fn update_git_tag(
        &self,
        content: &str,
        package: &str,
        new_tag: &str,
    ) -> Result<String, ManifestError> {
        // inline table (複数行も)・dotted key・`[dependencies.<pkg>]` / `[patch.<registry>.<pkg>]`
        // テーブルの `tag` を、依存のテーブルと `[patch.<registry>]` で書き換える。TOML の単一引用符も
        // 保つ。別のリポジトリ (fork など) を指す `[patch]` の宣言は書き換えない (#160)。
        let foreign = foreign_git_patch_registries(content, package);
        let replacements = cargo_sites(content, package)
            .iter()
            .filter(|site| {
                !patch_table_registry(&site.table)
                    .is_some_and(|registry| foreign.contains(registry))
            })
            .filter_map(|site| {
                let (start, end) = site.string_span(content, "tag")?;
                Some((start, end, new_tag.to_string()))
            })
            .collect();

        apply_replacements(content, replacements).ok_or_else(|| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Cargo.toml"),
            spec: package.to_string(),
            message: "git tag not found or could not be updated".to_string(),
        })
    }
}

fn parse_cargo_dependencies(
    deps: &toml::map::Map<String, Value>,
    parser: &dyn VersionParser,
    is_dev: bool,
    output: &mut CargoDeclarations,
) {
    for (name, value) in deps {
        match value {
            // 単純な文字列: `package = "1.0.0"`
            Value::String(s) => {
                if let Some(spec) = parser.parse(s) {
                    output.push(new_dependency(name, spec, is_dev, None, None), None);
                }
            }
            // インラインテーブル 形式: `package = { version = "1.0.0", features = [...] }`
            Value::Table(t) => {
                let package_name = t.get("package").and_then(|v| v.as_str()).unwrap_or(name);
                let manifest_name = (package_name != name).then_some(name.as_str());

                // git 依存の検出を先に試みる
                if let Some(git_source) = try_parse_git_source(t) {
                    let spec = git_reference_spec(&git_source.reference);
                    // 公開用に併記した `version` は git 依存の Dependency には残らないが、
                    // 宣言をまとめてよいかの判定には要る
                    let published_version = t.get("version").and_then(|v| v.as_str());
                    output.push(
                        new_dependency(package_name, spec, is_dev, manifest_name, Some(git_source)),
                        published_version,
                    );
                    continue;
                }
                if t.contains_key("path") {
                    // path 依存はローカルのクレートで解決されるため crates.io の候補で
                    // 書き換えてはいけない。publish 用に `version` を併記していても、
                    // 公開済み最新版へ引き上げるとローカルクレートの実バージョンが
                    // 要求を満たさなくなり `cargo build` が壊れる
                    // (Cargo.lock 側が source 無しエントリを path 依存として除外するのと同じ方針)。
                    continue;
                }
                if t.get("registry")
                    .and_then(|v| v.as_str())
                    .is_some_and(|registry| registry != "crates-io")
                    || t.contains_key("registry-index")
                {
                    // crates.io API の候補で別レジストリの依存を書き換えると誤更新になる。
                    // `registry-index` はインデックス URL の直接指定なので、値に関わらず
                    // crates.io 以外を指す (社内 private registry で使われる)
                    continue;
                }
                if let Some(version_str) = t.get("version").and_then(|v| v.as_str())
                    && let Some(spec) = parser.parse(version_str)
                {
                    output.push(
                        new_dependency(package_name, spec, is_dev, manifest_name, None),
                        None,
                    );
                }
            }
            _ => {}
        }
    }
}

fn new_dependency(
    name: &str,
    spec: VersionSpec,
    is_dev: bool,
    manifest_name: Option<&str>,
    git_source: Option<GitSource>,
) -> Dependency {
    let mut dep = if is_dev {
        Dependency::development(name.to_string(), spec, Language::Rust)
    } else {
        Dependency::production(name.to_string(), spec, Language::Rust)
    };
    if let Some(manifest_name) = manifest_name {
        dep = dep.with_manifest_name(manifest_name);
    }
    if let Some(gs) = git_source {
        dep = dep.with_git_source(gs);
    }
    dep
}

/// 宣言をまとめるときのキー: (実パッケージ名, マニフェスト上の依存キー, 版の指定の生表記,
/// git の取得元 (URL, 参照, 公開用に併記した `version` の生表記))
type DeclarationKey = (
    String,
    String,
    String,
    Option<(String, GitReference, Option<String>)>,
);

/// parse した依存の一覧。同じ取得元・同じ版を指す宣言は 1 依存にまとめる。
///
/// `[dependencies]` と `[dev-dependencies]` に同じ依存を同じ版で書き、dev 側にだけ
/// `features` を足すのは Rust でよくある書き方。宣言ごとに 1 依存を返すと writer の
/// 曖昧性ガード (同じ依存キーの宣言が 2 件以上あれば更新を拒否) に必ず掛かり、
/// 「2 updates」と表示しながら 1 バイトも書けずに exit code 2 になっていた。
/// `update_version` / `update_git_tag` は全依存セクションの同名の宣言をすべて書き換えるので、
/// 取得元と版の指定が一致する宣言は「1 依存 = 1 書き換え」として扱ってよい。
/// まとめればレジストリや `git ls-remote` への問い合わせも 1 回で済む。
///
/// キー ([`DeclarationKey`]) が 1 つでも食い違う宣言はまとめない。マニフェストを書き換える
/// 更新は、従来どおり writer が曖昧として拒否する (git の branch / rev / 既定ブランチの更新は
/// マニフェストを書き換えないので、曖昧判定の対象外)。版の指定は生表記で比べるので、
/// Cargo では同じ意味の `"1.0"` と `"^1.0"` もまとめない。`features` などの付帯指定と
/// `is_dev` はキーに含めない。
/// 残すのは最初の宣言だが、`is_dev` はどれか 1 つでも本番の宣言があれば本番にする
/// (`[dev-dependencies]` とターゲット固有の `[dependencies]` の組では dev 側が先に読まれる)。
#[derive(Default)]
struct CargoDeclarations {
    dependencies: Vec<Dependency>,
    positions: std::collections::HashMap<DeclarationKey, usize>,
}

impl CargoDeclarations {
    /// 依存を足す。git 依存では、公開用に併記した `version` も `published_version` で渡す
    fn push(&mut self, dependency: Dependency, published_version: Option<&str>) {
        let key = (
            dependency.name.clone(),
            dependency.manifest_name().to_string(),
            dependency.version_spec.raw.clone(),
            dependency.git_source.as_ref().map(|git| {
                (
                    git.url.clone(),
                    git.reference.clone(),
                    published_version.map(str::to_string),
                )
            }),
        );
        if let Some(&position) = self.positions.get(&key) {
            self.dependencies[position].is_dev &= dependency.is_dev;
        } else {
            self.positions.insert(key, self.dependencies.len());
            self.dependencies.push(dependency);
        }
    }
}

/// インラインテーブル から git 依存を検出して `GitSource` を組み立てる
fn try_parse_git_source(table: &toml::map::Map<String, Value>) -> Option<GitSource> {
    let url = table.get("git").and_then(|v| v.as_str())?;
    let reference = if let Some(branch) = table.get("branch").and_then(|v| v.as_str()) {
        GitReference::Branch(branch.to_string())
    } else if let Some(tag) = table.get("tag").and_then(|v| v.as_str()) {
        GitReference::Tag(tag.to_string())
    } else if let Some(rev) = table.get("rev").and_then(|v| v.as_str()) {
        GitReference::Rev(rev.to_string())
    } else {
        GitReference::DefaultBranch
    };
    Some(GitSource::new(url, reference))
}

/// git 依存に対応する VersionSpec を組み立てる。
/// raw/version は branch/tag/rev の表示名を保持する (更新判定・出力表示で利用)。
fn git_reference_spec(reference: &GitReference) -> VersionSpec {
    let display = match reference {
        GitReference::Branch(_) | GitReference::Tag(_) | GitReference::Rev(_) => {
            reference.raw_value().unwrap_or("").to_string()
        }
        GitReference::DefaultBranch => "HEAD".to_string(),
    };
    VersionSpec::new(VersionSpecKind::Exact, display.clone(), display)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        CargoTomlParser.parse(content)
    }

    /// Issue #98: 複数行に分けた inline table の依存も書き換える。parse (toml クレート) は
    /// 依存として報告するのに、以前は宣言の 1 行目しか見ず、後の行の `version` / `tag` に
    /// 届かずに書き込みが失敗していた。
    #[test]
    fn multiline_inline_tables_are_updated() {
        let content = "[dependencies]\ntokio = { features = [\n  \"rt\",\n], version = \"1.40.0\" }\nserde = \"1.0\"\n";
        let deps = parse(content).unwrap();
        assert!(deps.iter().any(|dep| dep.name == "tokio"));
        let updated = CargoTomlParser
            .update_version(content, "tokio", "1.48.0")
            .unwrap();
        assert_eq!(updated, content.replace("\"1.40.0\"", "\"1.48.0\""));
        // TOML 1.1 の、改行を含む inline table
        let content =
            "[dependencies]\nreqwest = {\n  default-features = false,\n  version = '0.12.0',\n}\n";
        let updated = CargoTomlParser
            .update_version(content, "reqwest", "0.12.9")
            .unwrap();
        assert_eq!(updated, content.replace("'0.12.0'", "'0.12.9'"));
        // git の tag も同じ
        let content = "[dependencies]\nmylib = { git = \"https://example.com/mylib.git\",\n  tag = \"v1.0.0\" }\n";
        let updated = CargoTomlParser
            .update_git_tag(content, "mylib", "v1.1.0")
            .unwrap();
        assert_eq!(updated, content.replace("v1.0.0", "v1.1.0"));
    }

    /// 複数行の inline table でも、path や別レジストリを指す宣言は書き換えない (parse と同じ)。
    #[test]
    fn multiline_inline_tables_with_other_sources_are_not_updated() {
        for source in [
            "path = \"../local\"",
            "registry = \"internal\"",
            "registry-index = \"https://example.com/index\"",
        ] {
            let content = format!(
                "[dependencies]\nmylib = {{ features = [\n  \"a\",\n], version = \"1.0.0\", {source} }}\n"
            );
            assert!(
                CargoTomlParser
                    .update_version(&content, "mylib", "1.1.0")
                    .is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn test_update_version_key_boundaries_and_spacing() {
        // 接頭辞を共有する別依存の取得元で、対象のセクションを除外しない。
        let content = "[dependencies]\r\n\tserde\t=  '1.0' # 保持\r\nserde_json.path = '../serde_json'\r\nserde_json.version = '1.0'\r\n[dev-dependencies]\r\n  serde . version\t= '1.0' # 保持\r\n[build-dependencies]\r\nserde={ features = ['std'], version\t= '1.0' } # 保持\r\n[package.metadata]\r\nserde = '1.0'";
        // 等号の前後の空白は、単純な指定でも書かれたとおりに保つ (#168 で値の範囲だけを置き換える
        // 形にした。以前は単純な指定だけ空白 1 つに詰めていた)
        let expected = content
            .replace("\tserde\t=  '1.0'", "\tserde\t=  '1.2.3'")
            .replace("serde . version\t= '1.0'", "serde . version\t= '1.2.3'")
            .replace("version\t= '1.0' }", "version\t= '1.2.3' }");
        assert_eq!(
            CargoTomlParser
                .update_version(content, "serde", "1.2.3")
                .unwrap(),
            expected
        );
        toml::from_str::<Value>(&expected).unwrap();
    }

    /// 回帰テスト (#168): 引用符付きのキー・親のテーブルの dotted key・点の前後の空白で書いた
    /// 依存も、parse と同じ宣言を書き換える
    #[test]
    fn quoted_and_dotted_keys_are_updated_like_parse_reads_them() {
        let content = "[dependencies]
\"itoa\" = \"1.0.0\"

[dependencies.\"ryu\"]
version = \"1.0.0\"

[dependencies . semver]
version = \"1.0.0\"

[target.'cfg(windows)']
dependencies.cfg-if = \"0.1.0\"
dev-dependencies.\"log\".version = \"0.4.0\"
";
        let deps = parse(content).unwrap();
        for (name, old) in [
            ("itoa", "\"itoa\" = \"1.0.0\""),
            (
                "ryu",
                "[dependencies.\"ryu\"]
version = \"1.0.0\"",
            ),
            (
                "semver",
                "[dependencies . semver]
version = \"1.0.0\"",
            ),
            ("cfg-if", "dependencies.cfg-if = \"0.1.0\""),
            ("log", "dev-dependencies.\"log\".version = \"0.4.0\""),
        ] {
            assert!(deps.iter().any(|dep| dep.name == name), "{name}: {deps:?}");
            let updated = CargoTomlParser
                .update_version(content, name, "9.9.9")
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            let new = old
                .replace("1.0.0", "9.9.9")
                .replace("0.1.0", "9.9.9")
                .replace("0.4.0", "9.9.9");
            assert_eq!(updated, content.replace(old, &new), "{name}");
            toml::from_str::<Value>(&updated).unwrap();
        }
    }

    /// 回帰テスト (#168): git の tag も、引用符付きのキーと dotted key の宣言を書き換える
    #[test]
    fn quoted_and_dotted_git_tags_are_updated() {
        let content = "[dependencies]
\"mylib\" = { git = \"https://example.com/mylib\", tag = \"v1.0.0\" }

[target.'cfg(unix)'.dependencies.\"other\"]
git = \"https://example.com/other\"
tag = \"v1.0.0\"
";
        for name in ["mylib", "other"] {
            let updated = CargoTomlParser
                .update_git_tag(content, name, "v2.0.0")
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(updated.matches("v2.0.0").count(), 1, "{name}: {updated}");
            toml::from_str::<Value>(&updated).unwrap();
        }
    }

    /// 引用符付きのキーで書いた path 依存は、parse と同じく書き換えない
    #[test]
    fn quoted_keys_pointing_at_paths_are_not_updated() {
        for content in [
            "[dependencies]
\"itoa\" = { path = \"../itoa\", version = \"1.0.0\" }
",
            "[dependencies.\"itoa\"]
version = \"1.0.0\"
path = \"../itoa\"
",
            "[dependencies]
\"itoa\".version = \"1.0.0\"
\"itoa\" . path = \"../itoa\"
",
        ] {
            assert!(parse(content).unwrap().is_empty(), "{content}");
            assert!(
                CargoTomlParser
                    .update_version(content, "itoa", "9.9.9")
                    .is_err(),
                "{content}"
            );
        }
    }

    #[test]
    fn test_update_git_tag_key_boundaries_and_spacing() {
        let content = "[dependencies]\r\n  my-lib = { git = 'https://example.com/repo', tag\t= 'v1.0.0' } # 保持\r\nmy-lib-extra = { git = 'https://example.com/repo', tag = 'v1.0.0' }\r\n[dev-dependencies]\r\n  my-lib . tag\t= 'v1.0.0' # 保持\r\nmy-lib.git = 'https://example.com/repo'\r\n[package.metadata]\r\nmy-lib.tag = 'v1.0.0'";
        let expected = content.replace("tag\t= 'v1.0.0'", "tag\t= 'v2.0.0'");
        assert_eq!(
            CargoTomlParser
                .update_git_tag(content, "my-lib", "v2.0.0")
                .unwrap(),
            expected
        );
        toml::from_str::<Value>(&expected).unwrap();
    }

    #[test]
    fn test_parse_simple_dependencies() {
        let content = r#"
[dependencies]
serde = "1.0"
tokio = "^1.28.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);
        assert!(!serde.is_dev);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_pinned_version() {
        let content = r#"
[dependencies]
exact = "=1.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_inline_table() {
        let content = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "^1.28.0", features = ["full"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_dev_dependencies() {
        let content = r#"
[dev-dependencies]
criterion = "0.5"
tempfile = "3.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().all(|d| d.is_dev));
    }

    #[test]
    fn test_parse_build_dependencies() {
        let content = r#"
[build-dependencies]
cc = "1.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_mixed_dependencies() {
        let content = r#"
[dependencies]
serde = "1.0"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde.is_dev);

        let criterion = deps.iter().find(|d| d.name == "criterion").unwrap();
        assert!(criterion.is_dev);
    }

    #[test]
    fn test_parse_tilde_version() {
        let content = r#"
[dependencies]
regex = "~1.9"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"
[package]
name = "test"
version = "0.1.0"
"#;

        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_invalid_toml() {
        let content = "not valid toml";
        let result = parse(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_git_dependency_default_branch() {
        let content = r#"
[dependencies]
my-crate = { git = "https://github.com/example/my-crate" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let dep = &deps[0];
        assert_eq!(dep.name, "my-crate");
        let git = dep.git_source.as_ref().unwrap();
        assert_eq!(git.url, "https://github.com/example/my-crate");
        assert_eq!(git.reference, GitReference::DefaultBranch);
    }

    #[test]
    fn test_parse_git_dependency_branch() {
        let content = r#"
[dependencies]
foo = { git = "https://github.com/owner/foo.git", branch = "main" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.url, "https://github.com/owner/foo.git");
        assert_eq!(git.reference, GitReference::Branch("main".to_string()));
    }

    #[test]
    fn test_parse_git_dependency_tag() {
        let content = r#"
[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.reference, GitReference::Tag("v1.2.3".to_string()));
        // tag 指定は is_pinned() = false (更新対象)
        assert!(!deps[0].is_pinned());
    }

    #[test]
    fn test_parse_git_dependency_rev_is_pinned() {
        let content = r#"
[dependencies]
baz = { git = "https://github.com/owner/baz.git", rev = "abc1234" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.reference, GitReference::Rev("abc1234".to_string()));
        // rev 指定は is_pinned() = true
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_update_git_tag_inline_table() {
        let content = r#"[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3" }
serde = "1.0.0"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.3.0""#));
        assert!(!result.contains(r#"tag = "v1.2.3""#));
        // 他の行が保持される
        assert!(result.contains(r#"serde = "1.0.0""#));
    }

    #[test]
    fn test_update_git_tag_inline_table_single_quoted_tag() {
        let content = r#"[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = 'v1.2.3' }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains("tag = 'v1.3.0'"));
        assert!(!result.contains("tag = 'v1.2.3'"));
    }

    #[test]
    fn test_update_git_tag_inline_table_scoped_to_dependency_sections() {
        let content = r#"[package.metadata]
bar = { tag = "keep" }

[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3" }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();

        assert!(result.contains(
            r#"[package.metadata]
bar = { tag = "keep" }"#
        ));
        assert!(
            result
                .contains(r#"bar = { git = "https://github.com/owner/bar.git", tag = "v1.3.0" }"#)
        );
    }

    #[test]
    fn test_update_git_tag_patch_inline_table() {
        let content = r#"[patch.crates-io]
foo = { git = "https://example.com/foo", tag = "v1.0.0" }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v1.1.0")
            .unwrap();
        assert!(result.contains(r#"foo = { git = "https://example.com/foo", tag = "v1.1.0" }"#));
    }

    #[test]
    fn test_update_git_tag_multiline_table() {
        let content = r#"[dependencies.bar]
git = "https://github.com/owner/bar.git"
tag = "v1.2.3"
features = ["async"]
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.3.0""#));
        assert!(result.contains(r#"features = ["async"]"#));
    }

    #[test]
    fn test_update_git_tag_not_found() {
        let content = r#"[dependencies]
serde = "1.0.0"
"#;
        let result = CargoTomlParser.update_git_tag(content, "serde", "v1.3.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_version_ignores_git_dependency() {
        // update_version は git 依存 (version 無し) を見つけられずエラーを返す
        let content = r#"[dependencies]
my-crate = { git = "https://github.com/example/my-crate", branch = "main" }
"#;
        let result = CargoTomlParser.update_version(content, "my-crate", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_path_dependency_skipped() {
        let content = r#"
[dependencies]
local-crate = { path = "../local-crate" }
"#;

        let deps = parse(content).unwrap();
        // バージョンを持たない path 依存はスキップする
        assert!(deps.is_empty());
    }

    /// 回帰テスト: publish 用に `version` を併記した path 依存も crates.io の候補で
    /// 書き換えてはいけない。ローカルクレートの実バージョンが要求を満たさなくなり
    /// `cargo build` が壊れるため (crates.io に同名クレートが実在する場合は
    /// まったく無関係なクレートの版で上書きされる)。
    #[test]
    fn test_parse_path_dependency_with_version_skipped() {
        let content = r#"
[dependencies]
common = { path = "../common", version = "0.1.0" }
tokio = { version = "1.0" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "tokio");
    }

    /// 回帰テスト: TOML の dotted key (`tokio.version = "1.38"`) は parse が依存として
    /// 読むため、update も同じ範囲を書き換えられること (report/apply の整合)。
    #[test]
    fn test_dotted_key_dependency_parse_and_update() {
        let content = r#"
[dependencies]
tokio.version = "1.38"
tokio.features = ["full"]
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "tokio");
        assert_eq!(deps[0].version_spec.version, "1.38");

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.52.4")
            .unwrap();
        assert!(result.contains(r#"tokio.version = "1.52.4""#), "{}", result);
        // 他の dotted key は触らない
        assert!(result.contains(r#"tokio.features = ["full"]"#));
    }

    /// 依存セクション外の dotted key は書き換えない。
    #[test]
    fn test_dotted_key_path_dependency_is_not_updated() {
        let content = r#"
[dependencies]
tokio.version = "1.38"

[dev-dependencies]
tokio.path = "../tokio"
tokio.version = "1.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.52.4")
            .unwrap();

        assert!(result.contains(r#"tokio.version = "1.52.4""#));
        assert!(result.contains(r#"tokio.path = "../tokio""#));
        assert!(result.contains(r#"tokio.version = "1.0""#));
    }

    #[test]
    fn test_dotted_key_outside_dependency_section_not_updated() {
        let content = r#"
[package.metadata.custom]
tokio.version = "1.38"
"#;

        let result = CargoTomlParser.update_version(content, "tokio", "1.52.4");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_workspace_dependency() {
        let content = r#"
[dependencies]
serde = { workspace = true }
"#;

        let deps = parse(content).unwrap();
        // 明示バージョンのない workspace 依存はスキップする
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_custom_registry_dependency_skipped() {
        let content = r#"
[dependencies]
private-crate = { version = "1.0", registry = "internal" }
public-crate = { version = "2.0", registry = "crates-io" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "public-crate");
        assert_eq!(deps[0].version_spec.version, "2.0");
    }

    /// `registry-index` はインデックス URL の直接指定で、`registry` と同じく
    /// crates.io 以外を指す。crates.io の候補で書き換えると、社内 private registry の
    /// 依存が同名の公開クレート (typosquat を含む) の版で上書きされる
    #[test]
    fn test_parse_registry_index_dependency_skipped() {
        let content = r#"
[dependencies]
internal-crate = { version = "1.0", registry-index = "https://intranet.example/index" }
public-crate = { version = "2.0" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(
            deps.len(),
            1,
            "registry-index 依存は surface しない: {deps:?}"
        );
        assert_eq!(deps[0].name, "public-crate");
    }

    /// 複数行テーブル / dotted key 形式の `registry-index` も writer が書き換えない
    #[test]
    fn test_update_does_not_touch_registry_index_dependency() {
        for content in [
            // インラインテーブル
            "[dependencies]\ninternal-crate = { version = \"1.0\", registry-index = \"https://intranet.example/index\" }\n",
            // 複数行テーブル (version より後ろに registry-index)
            "[dependencies.internal-crate]\nversion = \"1.0\"\nregistry-index = \"https://intranet.example/index\"\n",
            // ドット付きキー
            "[dependencies]\ninternal-crate.version = \"1.0\"\ninternal-crate.registry-index = \"https://intranet.example/index\"\n",
        ] {
            let result = CargoTomlParser.update_version(content, "internal-crate", "9.9.9");
            match result {
                // 書き換え対象が見つからずエラーになるのが期待動作 (誤更新しない)
                Err(_) => {}
                Ok(updated) => assert_eq!(
                    updated, content,
                    "registry-index 依存を書き換えてはいけない: {content}"
                ),
            }
        }
    }

    /// 回帰: `path` / `registry` は crates.io に実在するクレート名でもある。
    /// 依存セクションの行頭キーはパッケージ名なので、そこに現れた `path = "0.1.1"` を
    /// 「path 依存の宣言」と誤認してはいけない。以前は writer が行ごとスキップし、
    /// judge が「更新あり」と報告した後に
    /// 「package not found or version could not be updated」で失敗していた。
    #[test]
    fn test_update_crate_named_like_source_keys() {
        for (package, content, expected) in [
            (
                "path",
                "[dependencies]\npath = \"0.1.1\"\n",
                "[dependencies]\npath = \"9.9.9\"\n",
            ),
            (
                "registry",
                "[dependencies]\nregistry = \"0.2\"\n",
                "[dependencies]\nregistry = \"9.9.9\"\n",
            ),
            (
                "registry-index",
                "[dependencies]\nregistry-index = \"0.2\"\n",
                "[dependencies]\nregistry-index = \"9.9.9\"\n",
            ),
            // 名前が前方一致するだけのクレートは従来どおり更新できる
            (
                "pathdiff",
                "[dependencies]\npathdiff = \"0.2\"\n",
                "[dependencies]\npathdiff = \"9.9.9\"\n",
            ),
        ] {
            let deps = parse(content).unwrap();
            assert_eq!(deps.len(), 1, "{package} は依存として surface する");
            assert_eq!(deps[0].name, package);

            let result = CargoTomlParser
                .update_version(content, package, "9.9.9")
                .unwrap_or_else(|e| panic!("{package} を更新できるべき: {e}"));
            assert_eq!(result, expected);
        }
    }

    /// 行頭キーの誤認を直しても、inline table 内のソース指定による除外は維持する
    #[test]
    fn test_update_still_excludes_inline_source_keys() {
        for content in [
            "[dependencies]\nfoo = { path = \"../x\", version = \"1.0\" }\n",
            "[dependencies]\nfoo = { registry = \"internal\", version = \"1.0\" }\n",
            "[dependencies]\nfoo = { registry-index = \"https://intranet.example/index\", version = \"1.0\" }\n",
            // クレート名が `path` でも、実体が path 依存なら書き換えない
            "[dependencies]\npath = { path = \"../path\", version = \"0.1.1\" }\n",
        ] {
            let package = if content.contains("\npath = {") {
                "path"
            } else {
                "foo"
            };
            match CargoTomlParser.update_version(content, package, "9.9.9") {
                // 書き換え対象が見つからずエラーになるのが期待動作 (誤更新しない)
                Err(_) => {}
                Ok(updated) => assert_eq!(
                    updated, content,
                    "crates.io 以外を指す依存を書き換えてはいけない: {content}"
                ),
            }
        }
    }

    /// 回帰: dotted key の git tag 依存。toml クレートは dotted key を inline table と
    /// 同じ構造へ畳むため parse は git tag 依存として surface する。writer に dotted key の
    /// パターンが無いと「更新あり」と報告した後に
    /// 「git tag not found or could not be updated」で必ず失敗していた。
    #[test]
    fn test_update_git_tag_dotted_key() {
        let content = r#"[dependencies]
mylib.git = "https://github.com/org/mylib"
mylib.tag = "v1.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "mylib");
        assert_eq!(
            deps[0].git_source.as_ref().map(|git| &git.reference),
            Some(&GitReference::Tag("v1.0.0".to_string()))
        );

        let result = CargoTomlParser
            .update_git_tag(content, "mylib", "v1.1.0")
            .unwrap();
        assert!(result.contains(r#"mylib.tag = "v1.1.0""#), "{}", result);
        assert!(result.contains(r#"mylib.git = "https://github.com/org/mylib""#));
    }

    #[test]
    fn test_update_git_tag_dotted_key_preserves_single_quotes() {
        let content =
            "[dependencies]\nmylib.git = 'https://github.com/org/mylib'\nmylib.tag = 'v1.0.0'\n";

        let result = CargoTomlParser
            .update_git_tag(content, "mylib", "v1.1.0")
            .unwrap();
        assert!(result.contains("mylib.tag = 'v1.1.0'"), "{}", result);
    }

    #[test]
    fn test_update_git_tag_dotted_key_scoped_to_dependency_sections() {
        let content = r#"[package.metadata.custom]
mylib.tag = "keep"

[dependencies]
mylib.git = "https://github.com/org/mylib"
mylib.tag = "v1.0.0"
"#;

        let result = CargoTomlParser
            .update_git_tag(content, "mylib", "v1.1.0")
            .unwrap();
        assert!(result.contains(r#"mylib.tag = "keep""#), "{}", result);
        assert!(result.contains(r#"mylib.tag = "v1.1.0""#), "{}", result);
    }

    #[test]
    fn test_parse_target_specific() {
        let content = r#"
[target.'cfg(windows)'.dependencies]
winapi = "0.3"

[target.'cfg(unix)'.dependencies]
libc = "0.2"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2);

        let winapi = deps.iter().find(|d| d.name == "winapi").unwrap();
        assert!(!winapi.is_dev);

        let libc = deps.iter().find(|d| d.name == "libc").unwrap();
        assert!(!libc.is_dev);
    }

    #[test]
    fn test_parse_target_specific_build_dependencies() {
        let content = r#"
[target.'cfg(unix)'.build-dependencies]
cc = "1.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "cc");
        assert!(deps[0].is_dev);
    }

    #[test]
    fn test_parse_renamed_package_dependency() {
        let content = r#"
[dependencies]
tokio_v1 = { package = "tokio", version = "1.0", features = ["rt"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "tokio");
        assert_eq!(deps[0].manifest_name(), "tokio_v1");
        assert_eq!(deps[0].version_spec.version, "1.0");
    }

    #[test]
    fn test_parse_renamed_git_dependency() {
        let content = r#"
[dependencies]
regex_alias = { package = "regex", git = "https://github.com/rust-lang/regex.git", tag = "1.10.3" }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "regex");
        assert_eq!(deps[0].manifest_name(), "regex_alias");
        assert!(deps[0].is_git());
    }

    #[test]
    fn test_update_simple_version() {
        let content = r#"
[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(result.contains("\"1.1.0\""));
    }

    #[test]
    fn test_update_simple_version_single_quotes() {
        // TOML のリテラル文字列でも、引用符の種類を維持して更新する
        let content = r#"
[dependencies]
serde = '1.0.0'
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(
            result.contains("serde = '1.1.0'"),
            "単一引用符の依存を更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_simple_version_ignores_non_dependency_section() {
        // 同名キーが依存セクション外にあっても、実際の依存だけを更新する
        let content = r#"
[package.metadata]
serde = "metadata-value"

[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(r#"serde = "metadata-value""#));
        assert!(result.contains(r#"serde = "1.1.0""#));
    }

    #[test]
    fn test_update_simple_version_under_commented_section_header() {
        let content = r#"
[dependencies] # direct deps
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(r#"serde = "1.1.0""#));
    }

    /// 回帰テスト: TOML 仕様が許容するヘッダ内空白 (`[ dependencies ]`) でも
    /// 依存セクションとして認識して更新できる。以前はセクション名に空白が
    /// 残って照合に失敗し、parse (toml クレート) は依存として読むのに
    /// writer は書き換えないという parse/write 不整合があった。
    #[test]
    fn test_update_simple_version_under_whitespace_padded_section_header() {
        let content = r#"
[ dependencies ]
serde = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(r#"serde = "1.1.0""#));
    }

    #[test]
    fn test_update_renamed_package_dependency_uses_manifest_key() {
        let content = r#"
[dependencies]
tokio_v1 = { package = "tokio", version = "1.0", features = ["rt"] }
"#;

        let deps = parse(content).unwrap();
        let result = CargoTomlParser
            .update_version(content, deps[0].manifest_name(), "1.45.0")
            .unwrap();
        assert!(result.contains(r#"tokio_v1 = { package = "tokio", version = "1.45.0""#));
    }

    #[test]
    fn test_update_target_specific_multiline_table() {
        let content = r#"
[target.'cfg(unix)'.dependencies.openssl]
version = "0.10"
features = ["vendored"]
"#;

        let result = CargoTomlParser
            .update_version(content, "openssl", "0.10.72")
            .unwrap();
        assert!(result.contains(r#"version = "0.10.72""#));
        assert!(result.contains(r#"features = ["vendored"]"#));
    }

    #[test]
    fn test_update_target_specific_git_tag_multiline_table() {
        let content = r#"
[target.'cfg(unix)'.dependencies.regex]
git = "https://github.com/rust-lang/regex.git"
tag = "1.10.3"
"#;

        let result = CargoTomlParser
            .update_git_tag(content, "regex", "1.11.0")
            .unwrap();
        assert!(result.contains(r#"tag = "1.11.0""#));
    }

    #[test]
    fn test_update_wildcard_version_preserves_shape() {
        let content = r#"
[dependencies]
serde = "1.*"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "2.3.4")
            .unwrap();
        assert!(result.contains("\"2.*\""));
    }

    #[test]
    fn test_update_wildcard_x_version_preserves_shape() {
        // 回帰テスト: cargo (semver crate) が受理する `1.x` 形式も形を保って更新する
        let content = r#"
[dependencies]
serde = "1.x"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Wildcard);

        let result = CargoTomlParser
            .update_version(content, "serde", "2.3.4")
            .unwrap();
        assert!(result.contains(r#"serde = "2.x""#));
    }

    #[test]
    fn test_update_caret_version() {
        let content = r#"
[dependencies]
tokio = "^1.28.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.35.0")
            .unwrap();
        assert!(result.contains("\"^1.35.0\""));
    }

    #[test]
    fn test_update_range_version_keeps_upper_bound() {
        let content = r#"
[dependencies]
serde = ">=1.0, <2.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.9.3")
            .unwrap();
        assert!(result.contains("\">=1.9.3, <2.0\""));
    }

    #[test]
    fn test_update_inline_table() {
        let content = r#"
[dependencies]
serde = { version = "1.0.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(result.contains("\"1.1.0\""));
        assert!(result.contains("features"));
    }

    #[test]
    fn test_update_inline_table_single_quotes() {
        // インラインテーブル の version 値でも単一引用符を維持する
        let content = r#"
[dependencies]
serde = { version = '1.0.0', features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(
            result.contains("version = '1.1.0'"),
            "inline table の単一引用符バージョンを更新できていません: {}",
            result
        );
    }

    #[test]
    fn test_update_inline_table_ignores_non_dependency_section() {
        // metadata の inline table ではなく、依存セクションの inline table だけを更新する
        let content = r#"
[package.metadata]
serde = { version = "metadata-value" }

[dependencies]
serde = { version = "1.0.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains(
            r#"[package.metadata]
serde = { version = "metadata-value" }"#
        ));
        assert!(result.contains(r#"serde = { version = "1.1.0", features = ["derive"] }"#));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#"
[dependencies]
serde = "1.0.0"
"#;

        let result = CargoTomlParser.update_version(content, "nonexistent", "1.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_language() {
        assert_eq!(CargoTomlParser.language(), Language::Rust);
    }

    #[test]
    fn test_parse_comparison_operators() {
        let content = r#"
[dependencies]
pkg1 = ">=1.0.0"
pkg2 = ">1.0.0"
pkg3 = "<=2.0.0"
pkg4 = "<2.0.0"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        let pkg1 = deps.iter().find(|d| d.name == "pkg1").unwrap();
        assert_eq!(pkg1.version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let pkg2 = deps.iter().find(|d| d.name == "pkg2").unwrap();
        assert_eq!(pkg2.version_spec.kind, VersionSpecKind::Greater);

        let pkg3 = deps.iter().find(|d| d.name == "pkg3").unwrap();
        assert_eq!(pkg3.version_spec.kind, VersionSpecKind::LessOrEqual);

        let pkg4 = deps.iter().find(|d| d.name == "pkg4").unwrap();
        assert_eq!(pkg4.version_spec.kind, VersionSpecKind::Less);
    }

    #[test]
    fn test_update_multiline_table() {
        let content = r#"[dependencies.tree-sitter]
version = "0.22"

[dependencies.tree-sitter-bash]
version = "0.21"
"#;

        let result = CargoTomlParser
            .update_version(content, "tree-sitter", "0.26.3")
            .unwrap();

        // version が正しく引用符で囲まれていることを確認する
        assert!(result.contains("version = \"0.26.3\""));
        // 閉じ引用符が欠けていないことを確認する
        assert!(!result.contains("\"0.26.3\n"));

        // 2つ目のパッケージも更新する
        let result2 = CargoTomlParser
            .update_version(&result, "tree-sitter-bash", "0.25.1")
            .unwrap();

        assert!(result2.contains("version = \"0.25.1\""));
        assert!(result2.contains("version = \"0.26.3\""));
    }

    #[test]
    fn test_update_multiline_table_single_quotes() {
        // 複数行テーブルの version 値でも単一引用符を維持する
        let content = r#"[dependencies.tree-sitter]
version = '0.22'
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "tree-sitter", "0.26.3")
            .unwrap();

        assert!(
            result.contains("version = '0.26.3'"),
            "複数行テーブルの単一引用符バージョンを更新できていません: {}",
            result
        );
        assert!(result.contains(r#"features = ["derive"]"#));
    }

    #[test]
    fn test_update_multiline_table_prefix_collision() {
        // 回帰テスト: パッケージ名が他のパッケージのプレフィックスと一致する場合に
        // 誤って prefix-only のパッケージ更新で suffix が長いパッケージが書き換わらない
        // 例: `[dependencies.serde_json]` が先に出現する状況で `serde` を更新する
        let content = r#"[dependencies.serde_json]
version = "1.0.0"

[dependencies.serde]
version = "1.0.2"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        // serde_json は変更されていないこと
        assert!(
            result.contains("[dependencies.serde_json]\nversion = \"1.0.0\""),
            "serde_json should not be modified, but got:\n{}",
            result
        );
        // serde が更新されていること
        assert!(
            result.contains("[dependencies.serde]\nversion = \"1.1.0\""),
            "serde should be updated, but got:\n{}",
            result
        );
    }

    #[test]
    fn test_update_git_tag_prefix_collision() {
        // 回帰テスト: プレフィックスが一致する別パッケージの git tag が誤って書き換わらない
        let content = r#"[dependencies.foo-extra]
git = "https://example.com/foo-extra"
tag = "v1.0.0"

[dependencies.foo]
git = "https://example.com/foo"
tag = "v0.5.0"
"#;

        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v0.6.0")
            .unwrap();

        // foo-extra の tag は変わらない
        assert!(
            result.contains("[dependencies.foo-extra]\ngit = \"https://example.com/foo-extra\"\ntag = \"v1.0.0\""),
            "foo-extra tag should not be modified, but got:\n{}",
            result
        );
        // foo の tag は更新される
        assert!(
            result.contains(
                "[dependencies.foo]\ngit = \"https://example.com/foo\"\ntag = \"v0.6.0\""
            ),
            "foo tag should be updated, but got:\n{}",
            result
        );
    }

    #[test]
    fn test_update_git_tag_multiline_table_tag_after_features() {
        // 回帰テスト: `features = [...]` が `tag` より前にあるテーブルでも更新できる
        let content = r#"[dependencies.bar]
git = "https://github.com/owner/bar.git"
features = ["async"]
tag = "v1.2.3"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(
            result.contains(r#"tag = "v1.3.0""#),
            "features が先行するテーブルの tag を更新できていません: {}",
            result
        );
        assert!(result.contains(r#"features = ["async"]"#));
    }

    #[test]
    fn test_update_git_tag_multiline_table_ignores_commented_tag() {
        // 回帰テスト: テーブル内コメントの `tag = "..."` は書き換えない
        let content = r#"[dependencies.bar]
git = "https://github.com/owner/bar.git"
# tag = "v0.9.0" 以前のタグ
tag = "v1.2.3"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "bar", "v1.3.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.3.0""#));
        assert!(result.contains(r#"# tag = "v0.9.0" 以前のタグ"#));
        assert!(!result.contains(r#"tag = "v1.2.3""#));
    }

    /// Issue #160: 別のリポジトリ (fork) を指す `[patch]` の tag は、upstream の新しい tag で
    /// 書き換えない。同じリポジトリを指す patch は従来どおり揃える。
    #[test]
    fn test_update_git_tag_keeps_patches_that_point_to_another_repository() {
        let content = r#"[dependencies]
foo = { git = "https://example.com/upstream.git", tag = "v1.0.0" }

[patch."https://example.com/upstream.git"]
foo = { git = "https://example.com/fork.git", tag = "v1.0.0-fix" }

[patch.crates-io.foo]
git = "https://example.com/fork"
tag = "v1.0.0-fix"

[patch.'https://example.com/other.git']
foo = { git = "https://example.com/upstream", tag = "v1.0.0" }
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v1.1.0")
            .unwrap();
        assert_eq!(
            result,
            content
                .replacen(r#"tag = "v1.0.0" }"#, r#"tag = "v1.1.0" }"#, 1)
                .replace(
                    r#"foo = { git = "https://example.com/upstream", tag = "v1.0.0" }"#,
                    r#"foo = { git = "https://example.com/upstream", tag = "v1.1.0" }"#
                )
        );
        assert_eq!(result.matches("v1.0.0-fix").count(), 2, "{result}");
    }

    #[test]
    fn test_update_git_tag_patch_section_multiline_table() {
        // `[patch.<registry>.<package>]` 形式の複数行テーブルも更新対象
        let content = r#"[patch.crates-io.foo]
git = "https://example.com/foo"
tag = "v1.0.0"
"#;
        let result = CargoTomlParser
            .update_git_tag(content, "foo", "v1.1.0")
            .unwrap();
        assert!(result.contains(r#"tag = "v1.1.0""#));
        assert!(result.contains(r#"git = "https://example.com/foo""#));
    }

    #[test]
    fn test_update_multiline_table_with_features() {
        let content = r#"[dependencies.serde]
version = "1.0.0"
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("version = \"1.1.0\""));
        assert!(result.contains("features = [\"derive\"]"));
    }

    #[test]
    fn test_update_multiline_table_version_after_features() {
        // 回帰テスト: `features = [...]` が `version` より前にあるテーブルでも更新できる
        // (旧実装は `[^\[]*` が features の `[` で止まり、常に更新失敗していた)
        let content = r#"[dependencies.serde]
features = ["derive"]
version = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(
            result.contains("version = \"1.1.0\""),
            "features が先行するテーブルの version を更新できていません: {}",
            result
        );
        assert!(result.contains("features = [\"derive\"]"));
    }

    #[test]
    fn test_update_multiline_table_ignores_commented_version() {
        // 回帰テスト: テーブル内コメントの `version = "..."` は書き換えず、
        // 実際の version キー行だけを更新する (旧実装は greedy マッチで
        // テーブル末尾コメント内の version を誤置換していた)
        let content = r#"[dependencies.serde]
# version = "0.9" 以前の固定値
version = "1.0.0"
# version = "0.8" まで利用していた

[features]
default = []
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("version = \"1.1.0\""));
        assert!(result.contains("# version = \"0.9\" 以前の固定値"));
        assert!(result.contains("# version = \"0.8\" まで利用していた"));
        assert!(!result.contains("version = \"1.0.0\""));
    }

    #[test]
    fn test_update_version_updates_all_dependency_sections() {
        // 回帰テスト: 同名クレートが複数の依存セクションにある場合、
        // 1回の update_version で全出現が更新される (旧実装は最初の1箇所のみ)
        let content = r#"
[dependencies]
tokio = { version = "1.28.0", features = ["full"] }

[dev-dependencies]
tokio = "~1.28.0"

[package.metadata]
tokio = "1.0.0"
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();

        // dependencies 側 (inline table) は自身の制約形式で更新される
        assert!(result.contains(r#"tokio = { version = "1.45.0", features = ["full"] }"#));
        // dev-dependencies 側 (tilde) も同じ呼び出しで更新される
        assert!(
            result.contains(r#"tokio = "~1.45.0""#),
            "dev-dependencies 側の同名依存が更新されていません: {}",
            result
        );
        // 依存セクション外の同名キーは書き換えない
        assert!(result.contains(r#"tokio = "1.0.0""#));
    }

    #[test]
    fn test_update_version_updates_all_multiline_tables() {
        // 同名クレートの複数行テーブルが複数の依存セクションにある場合も全て更新する
        let content = r#"[dependencies.serde]
version = "1.0.0"
features = ["derive"]

[dev-dependencies.serde]
version = "1.0.1"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();

        assert!(result.contains("[dependencies.serde]\nversion = \"1.1.0\""));
        assert!(
            result.contains("[dev-dependencies.serde]\nversion = \"1.1.0\""),
            "dev-dependencies 側の複数行テーブルが更新されていません: {}",
            result
        );
    }

    #[test]
    fn test_update_mixed_dependency_formats() {
        // 実運用に近い混在形式の Cargo.toml:
        // - 単純形式: `pkg = "version"`
        // - inline table 形式: `pkg = { version = "...", features = [...] }`
        // - 複数行テーブル: `[dependencies.pkg]`
        let content = r#"[package]
name = "example-hooks"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
dirs = "5"
regex = "1"
thiserror = "1"
anyhow = "1"

[dependencies.ts-parser]
version = "0.22"
optional = true

[dependencies.ts-bash]
version = "0.21"
optional = true

[features]
default = ["ast-parser"]
ast-parser = ["ts-parser", "ts-bash"]
"#;

        // 単純形式の更新
        let result = CargoTomlParser
            .update_version(content, "serde_json", "1.0.140")
            .unwrap();
        assert!(result.contains("serde_json = \"1.0.140\""));

        // インラインテーブル 形式の更新
        let result = CargoTomlParser
            .update_version(&result, "clap", "4.5.0")
            .unwrap();
        assert!(result.contains("version = \"4.5.0\""));
        assert!(result.contains("features = [\"derive\"]"));

        // 別の inline table も更新する
        let result = CargoTomlParser
            .update_version(&result, "tracing-subscriber", "0.3.20")
            .unwrap();
        assert!(result.contains("version = \"0.3.20\""));
        assert!(result.contains("features = [\"env-filter\"]"));

        // 複数行テーブル形式の更新でも引用符が壊れないことを確認する
        let result = CargoTomlParser
            .update_version(&result, "ts-parser", "0.26.3")
            .unwrap();
        assert!(result.contains("version = \"0.26.3\""));
        // 閉じ引用符が壊れていないことを確認する
        assert!(!result.contains("\"0.26.3\n["));

        let result = CargoTomlParser
            .update_version(&result, "ts-bash", "0.25.1")
            .unwrap();
        assert!(result.contains("version = \"0.25.1\""));
        assert!(!result.contains("\"0.25.1\n["));

        // すべての更新結果が保持されていることを確認する
        assert!(result.contains("serde_json = \"1.0.140\""));
        assert!(result.contains("clap = { version = \"4.5.0\""));
        assert!(result.contains("version = \"0.26.3\""));
        assert!(result.contains("version = \"0.25.1\""));

        // 関係ない内容が保持されていることを確認する
        assert!(result.contains("[features]"));
        assert!(result.contains("ast-parser = [\"ts-parser\", \"ts-bash\"]"));
    }

    #[test]
    fn test_parse_workspace_dependencies() {
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/core", "crates/cli"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.kind, VersionSpecKind::Caret);
        assert!(!tokio.is_dev);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.version_spec.kind, VersionSpecKind::Caret);

        let serde_json = deps.iter().find(|d| d.name == "serde_json").unwrap();
        assert_eq!(serde_json.version_spec.kind, VersionSpecKind::Caret);

        let thiserror = deps.iter().find(|d| d.name == "thiserror").unwrap();
        assert_eq!(thiserror.version_spec.kind, VersionSpecKind::Caret);

        let clap = deps.iter().find(|d| d.name == "clap").unwrap();
        assert_eq!(clap.version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_update_workspace_dependencies_simple() {
        let content = r#"
[workspace.dependencies]
serde_json = "1"
thiserror = "2"
"#;

        let result = CargoTomlParser
            .update_version(content, "serde_json", "1.0.140")
            .unwrap();
        assert!(result.contains("serde_json = \"1.0.140\""));
        // 他の依存関係が保持されていることを確認する
        assert!(result.contains("thiserror = \"2\""));
    }

    #[test]
    fn test_update_workspace_dependencies_inline_table() {
        let content = r#"
[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        let result = CargoTomlParser
            .update_version(&result, "serde", "1.0.220")
            .unwrap();
        assert!(result.contains("serde = { version = \"1.0.220\""));
    }

    #[test]
    fn test_update_workspace_dependencies_multiline_table() {
        let content = r#"[workspace.dependencies.tokio]
version = "1"
features = ["full"]

[workspace.dependencies.serde]
version = "1"
features = ["derive"]
"#;

        let result = CargoTomlParser
            .update_version(content, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        let result = CargoTomlParser
            .update_version(&result, "serde", "1.0.220")
            .unwrap();
        assert!(result.contains("version = \"1.0.220\""));
    }

    #[test]
    fn test_parse_full_workspace_cargo_toml() {
        // ユーザーの実例に近いワークスペース構成
        let content = r#"
[workspace]
resolver = "2"
members = [
    "crates/omni-pty-core",
    "crates/term-ipc",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
authors = ["omni-pty contributors"]
repository = "https://github.com/omni-pty/omni-pty"

[workspace.dependencies]
# Core dependencies
portable-pty = "0.9"
vte = "0.15"
tokio = { version = "1", features = ["full"] }
kdl = "4"

# Utilities
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
parking_lot = "0.12"
log = "0.4"
libc = "0.2"

# CLI
clap = { version = "4", features = ["derive"] }

# Testing
tokio-test = "0.4"

# Swift FFI
swift-bridge = "0.1"
swift-bridge-build = "0.1"
"#;

        let deps = parse(content).unwrap();
        // `workspace.dependencies` をすべて解釈できることを確認する（合計16件）
        assert_eq!(deps.len(), 16);

        // 代表的な依存関係の内容を確認する
        let portable_pty = deps.iter().find(|d| d.name == "portable-pty").unwrap();
        assert_eq!(portable_pty.version_spec.version, "0.9");

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.version_spec.version, "1");

        let uuid = deps.iter().find(|d| d.name == "uuid").unwrap();
        assert_eq!(uuid.version_spec.version, "1");

        let swift_bridge = deps.iter().find(|d| d.name == "swift-bridge").unwrap();
        assert_eq!(swift_bridge.version_spec.version, "0.1");
    }

    #[test]
    fn test_update_full_workspace_cargo_toml() {
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/core"]

[workspace.dependencies]
portable-pty = "0.9"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
"#;

        // 単純形式を更新する
        let result = CargoTomlParser
            .update_version(content, "portable-pty", "0.10.0")
            .unwrap();
        assert!(result.contains("portable-pty = \"0.10.0\""));

        // インラインテーブル 形式を更新する
        let result = CargoTomlParser
            .update_version(&result, "tokio", "1.45.0")
            .unwrap();
        assert!(result.contains("version = \"1.45.0\""));
        assert!(result.contains("features = [\"full\"]"));

        // workspace メタデータが保持されていることを確認する
        assert!(result.contains("resolver = \"2\""));
        assert!(result.contains("members = [\"crates/core\"]"));
    }

    #[test]
    fn test_parse_workspace_with_regular_dependencies() {
        // `workspace.dependencies` と通常依存が共存する workspace ルート
        let content = r#"
[workspace]
resolver = "2"
members = ["crates/cli"]

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = "1"

[dependencies]
clap = "4"

[dev-dependencies]
criterion = "0.5"
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        // workspace 依存
        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert!(!tokio.is_dev);

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert!(!serde.is_dev);

        // 通常依存
        let clap = deps.iter().find(|d| d.name == "clap").unwrap();
        assert!(!clap.is_dev);

        // 開発依存
        let criterion = deps.iter().find(|d| d.name == "criterion").unwrap();
        assert!(criterion.is_dev);
    }

    #[test]
    fn test_update_version_strips_build_metadata() {
        let content = r#"
[dependencies]
toml = "0.8.0"
"#;

        // crates.io は `"1.0.0+spec-1.1.0"` のような build metadata 付きで返す場合がある
        let result = CargoTomlParser
            .update_version(content, "toml", "1.0.0+spec-1.1.0")
            .unwrap();
        // build metadata は書き込み前に除去する
        assert!(result.contains("\"1.0.0\""));
        assert!(!result.contains("+spec-1.1.0"));
    }

    #[test]
    fn test_update_version_strips_build_metadata_inline_table() {
        let content = r#"
[dependencies]
toml = { version = "0.8.0", features = ["derive"] }
"#;

        let result = CargoTomlParser
            .update_version(content, "toml", "1.0.0+spec-1.1.0")
            .unwrap();
        assert!(result.contains("version = \"1.0.0\""));
        assert!(!result.contains("+spec-1.1.0"));
        assert!(result.contains("features"));
    }

    #[test]
    fn test_update_version_preserves_crlf() {
        // 回帰: CRLF の Cargo.toml を update_version しても改行コードを保持し、
        // 通常依存・inline table・複数行テーブルのいずれの経路でも正しく更新する。
        let content = "[dependencies]\r\nserde = \"1.0.0\"\r\ntokio = { version = \"1.0.0\", features = [\"full\"] }\r\n\r\n[dependencies.reqwest]\r\nversion = \"0.11.0\"\r\n";

        let simple = CargoTomlParser
            .update_version(content, "serde", "1.1.0")
            .unwrap();
        assert!(simple.contains("serde = \"1.1.0\""));
        assert_eq!(
            simple.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!simple.replace("\r\n", "").contains('\n'));

        let inline = CargoTomlParser
            .update_version(content, "tokio", "1.1.0")
            .unwrap();
        assert!(inline.contains("version = \"1.1.0\""));
        assert!(inline.contains("features = [\"full\"]"));
        assert_eq!(
            inline.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!inline.replace("\r\n", "").contains('\n'));

        let multiline = CargoTomlParser
            .update_version(content, "reqwest", "0.11.20")
            .unwrap();
        assert!(multiline.contains("version = \"0.11.20\""));
        assert_eq!(
            multiline.matches("\r\n").count(),
            content.matches("\r\n").count()
        );
        assert!(!multiline.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_update_git_tag_preserves_crlf() {
        // 回帰: CRLF の Cargo.toml を update_git_tag しても改行コードを保持し、
        // インラインテーブル・複数行テーブルの両タグ書き換え経路でも正しく更新する。
        let inline_content = "[dependencies]\r\nbar = { git = \"https://github.com/owner/bar.git\", tag = \"v1.2.3\" }\r\n";
        let inline = CargoTomlParser
            .update_git_tag(inline_content, "bar", "v1.3.0")
            .unwrap();
        assert!(inline.contains("tag = \"v1.3.0\""));
        assert_eq!(
            inline.matches("\r\n").count(),
            inline_content.matches("\r\n").count()
        );
        assert!(!inline.replace("\r\n", "").contains('\n'));

        let table_content = "[dependencies.bar]\r\ngit = \"https://github.com/owner/bar.git\"\r\ntag = \"v1.2.3\"\r\n";
        let table = CargoTomlParser
            .update_git_tag(table_content, "bar", "v1.3.0")
            .unwrap();
        assert!(table.contains("tag = \"v1.3.0\""));
        assert_eq!(
            table.matches("\r\n").count(),
            table_content.matches("\r\n").count()
        );
        assert!(!table.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_parse_merges_declarations_with_same_source_and_version() {
        // 回帰 (#20): 同じ依存を [dependencies] と [dev-dependencies] に同じ取得元・
        // 同じ版で書き、dev 側にだけ features を足す構成は 1 依存にまとめる。
        // 宣言ごとに返すと writer の曖昧性ガードで更新を拒否していた
        let content = r#"
[dependencies]
tagged = { git = "https://github.com/example/tagged", tag = "v1.0.100" }
anyhow = "1.0.0"
tracked = { git = "https://github.com/example/tracked", branch = "master" }
quoted = "2.0.0"

[dev-dependencies]
tagged = { git = "https://github.com/example/tagged", tag = "v1.0.100", features = ["preserve_order"] }
anyhow = { version = "1.0.0", features = ["backtrace"] }
tracked = { git = "https://github.com/example/tracked", branch = "master", features = ["preserve_order"] }
quoted = '2.0.0'
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4, "{deps:?}");

        let tagged = deps.iter().find(|d| d.name == "tagged").unwrap();
        assert!(!tagged.is_dev);
        assert_eq!(
            tagged.git_source.as_ref().unwrap().reference,
            GitReference::Tag("v1.0.100".to_string())
        );

        let anyhow = deps.iter().find(|d| d.name == "anyhow").unwrap();
        assert!(!anyhow.is_dev);
        assert_eq!(anyhow.version_spec.raw, "1.0.0");

        let tracked = deps.iter().find(|d| d.name == "tracked").unwrap();
        assert!(!tracked.is_dev);
        assert_eq!(
            tracked.git_source.as_ref().unwrap().reference,
            GitReference::Branch("master".to_string())
        );
    }

    #[test]
    fn test_parse_keeps_declarations_with_different_source_or_version() {
        // 版の指定・タグ・URL・取得元の種類・git に併記した version のどれかが食い違う宣言は
        // まとめない (writer が従来どおり曖昧として拒否する)。版の指定は生表記で比べるので、
        // Cargo では同じ意味の "1.0" と "^1.0" もまとめない
        let content = r#"
[dependencies]
ranged = "^1.0.0"
caret = "1.0"
tagged = { git = "https://github.com/example/tagged", tag = "v1.0.0" }
forked = { git = "https://github.com/example/forked", tag = "v1.0.0" }
mixed = { git = "https://github.com/example/mixed", branch = "main" }
published = { git = "https://github.com/example/published", tag = "v1.0.0", version = "1.0" }

[dev-dependencies]
ranged = "=2.0.0"
caret = "^1.0"
tagged = { git = "https://github.com/example/tagged", tag = "v2.0.0" }
forked = { git = "https://github.com/someone/forked", tag = "v1.0.0" }
mixed = "1.0.0"
published = { git = "https://github.com/example/published", tag = "v1.0.0", version = "2.0" }
"#;
        let deps = parse(content).unwrap();
        for name in ["ranged", "caret", "tagged", "forked", "mixed", "published"] {
            assert_eq!(
                deps.iter().filter(|d| d.name == name).count(),
                2,
                "{name} の宣言はまとめない: {deps:?}"
            );
        }
    }

    #[test]
    fn test_parse_keeps_same_key_renamed_to_different_packages() {
        // 依存キーが同じでも `package` で指す実パッケージが違う宣言はまとめない
        let content = r#"
[dependencies]
http = { package = "http", version = "1.0.0" }

[dev-dependencies]
http = { package = "http-legacy", version = "1.0.0" }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2, "{deps:?}");
        assert!(deps.iter().all(|d| d.manifest_name() == "http"));
    }

    #[test]
    fn test_parse_merged_declaration_is_production_when_any_declaration_is() {
        // [dev-dependencies] が target 固有の [dependencies] より先に読まれても、
        // 本番の宣言が 1 つでもあれば本番の依存として扱う
        let content = r#"
[dev-dependencies]
libc = "0.2.150"
cc = "1.0.83"

[build-dependencies]
cc = "1.0.83"

[target.'cfg(unix)'.dependencies]
libc = { version = "0.2.150", features = ["extra_traits"] }
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 2, "{deps:?}");
        assert!(!deps.iter().find(|d| d.name == "libc").unwrap().is_dev);
        assert!(deps.iter().find(|d| d.name == "cc").unwrap().is_dev);
    }
}
