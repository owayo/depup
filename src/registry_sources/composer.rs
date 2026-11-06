//! composer.json の `repositories`。
//!
//! Composer は上に書いたリポジトリから順にパッケージを探し、見つかったリポジトリの版を使う
//! (`canonical: false` なら下のリポジトリの版も候補にする)。Packagist 以外のリポジトリが
//! 供給しうる依存は、Packagist の版では判定できないので外す。
use super::{Verdict, is_packagist};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// Packagist 以外のリポジトリの規則
#[derive(Debug)]
enum Rule {
    /// Packagist を使わない
    PackagistDisabled,
    /// どのパッケージを持つか分からないリポジトリ。`only` / `exclude` があれば従う
    Repository {
        origin: String,
        only: Vec<String>,
        exclude: Vec<String>,
    },
    /// 名前が分かるパッケージだけを供給するリポジトリ (`package` / `path`)
    Packages {
        origin: String,
        names: BTreeSet<String>,
    },
}

/// composer.json の依存に効くリポジトリの設定
#[derive(Debug, Default)]
pub(crate) struct ComposerSources {
    rules: Vec<Rule>,
    /// 解釈できない設定。どの依存の取得元も確かめられない
    problems: Vec<String>,
}

impl ComposerSources {
    /// composer.json を読んで `repositories` を解釈する。読めなければどの依存も確かめられない。
    pub(crate) fn read(manifest: &Path) -> Self {
        match std::fs::read_to_string(manifest) {
            Ok(content) => Self::from_manifest(manifest, &content),
            Err(error) => Self {
                rules: Vec::new(),
                problems: vec![format!("composer.json could not be read: {error}")],
            },
        }
    }

    /// `manifest` (composer.json) の `repositories` を読む。
    pub(crate) fn from_manifest(manifest: &Path, content: &str) -> Self {
        let mut sources = Self::default();
        let json: Value = match serde_json::from_str(content) {
            Ok(json) => json,
            Err(error) => {
                sources
                    .problems
                    .push(format!("composer.json could not be parsed: {error}"));
                return sources;
            }
        };
        let base = manifest.parent().unwrap_or(Path::new("."));
        match json.get("repositories") {
            None | Some(Value::Null) => {}
            Some(Value::Array(repositories)) => {
                for (index, repository) in repositories.iter().enumerate() {
                    sources.add(base, &format!("repositories[{index}]"), None, repository);
                }
            }
            Some(Value::Object(repositories)) => {
                for (name, repository) in repositories {
                    sources.add(
                        base,
                        &format!("repositories.{name}"),
                        Some(name),
                        repository,
                    );
                }
            }
            Some(_) => sources
                .problems
                .push("composer.json repositories is not an array or object".into()),
        }
        sources
    }

    /// `package` の取得元
    pub(crate) fn verdict(&self, package: &str) -> Verdict {
        if !self.problems.is_empty() {
            return Verdict::Unverified(self.problems.join("; "));
        }
        let package = package.to_ascii_lowercase();
        for rule in &self.rules {
            let origin = match rule {
                Rule::PackagistDisabled => {
                    return Verdict::NonPublic("composer.json: Packagist is disabled".into());
                }
                Rule::Repository {
                    origin,
                    only,
                    exclude,
                } => {
                    let supplies = if !only.is_empty() {
                        only.iter().any(|pattern| matches(pattern, &package))
                    } else {
                        !exclude.iter().any(|pattern| matches(pattern, &package))
                    };
                    supplies.then_some(origin)
                }
                Rule::Packages { origin, names } => names.contains(&package).then_some(origin),
            };
            if let Some(origin) = origin {
                return Verdict::NonPublic(format!("composer.json: {origin}"));
            }
        }
        Verdict::Public
    }

    fn add(&mut self, base: &Path, origin: &str, name: Option<&str>, repository: &Value) {
        let disables_packagist = |name: Option<&str>| {
            name.is_some_and(|name| matches!(name, "packagist" | "packagist.org"))
        };
        let object = match repository {
            // `"packagist.org": false` (object の形)
            Value::Bool(false) if disables_packagist(name) => {
                self.rules.push(Rule::PackagistDisabled);
                return;
            }
            Value::Object(object) => object,
            _ => {
                self.problems
                    .push(format!("composer.json {origin} is not a repository"));
                return;
            }
        };
        // `{"packagist.org": false}` (配列の要素の形)
        if object.len() == 1
            && let Some((key, Value::Bool(false))) = object.iter().next()
            && disables_packagist(Some(key))
        {
            self.rules.push(Rule::PackagistDisabled);
            return;
        }
        let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
        let origin = format!(
            "{origin} ({})",
            if kind.is_empty() { "no type" } else { kind }
        );
        match kind {
            "composer"
                if object
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(is_packagist) => {}
            "package" => match package_names(object.get("package")) {
                Some(names) => self.rules.push(Rule::Packages { origin, names }),
                None => self.problems.push(format!(
                    "composer.json {origin} has a package without a name"
                )),
            },
            "path" => match path_names(base, object.get("url").and_then(Value::as_str)) {
                Ok(names) => self.rules.push(Rule::Packages { origin, names }),
                Err(reason) => self
                    .problems
                    .push(format!("composer.json {origin}: {reason}")),
            },
            // composer (私設) / vcs / git / github / artifact など、持つパッケージが分からない型
            _ => {
                let patterns = |key: &str| -> Vec<String> {
                    object
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_ascii_lowercase)
                        .collect()
                };
                self.rules.push(Rule::Repository {
                    origin,
                    only: patterns("only"),
                    exclude: patterns("exclude"),
                });
            }
        }
    }
}

/// `package` 型の定義 (1 つか配列) が供給するパッケージの名前 (`provide` / `replace` を含む)
fn package_names(definitions: Option<&Value>) -> Option<BTreeSet<String>> {
    let definitions: Vec<&Value> = match definitions? {
        Value::Array(definitions) => definitions.iter().collect(),
        definition => vec![definition],
    };
    let mut names = BTreeSet::new();
    for definition in definitions {
        names.extend(supplied_names(definition)?);
    }
    Some(names)
}

/// パッケージの定義 (composer.json) の名前と、`provide` / `replace` で供給する名前
fn supplied_names(definition: &Value) -> Option<BTreeSet<String>> {
    let mut names = BTreeSet::from([definition.get("name")?.as_str()?.to_ascii_lowercase()]);
    for key in ["provide", "replace"] {
        if let Some(Value::Object(entries)) = definition.get(key) {
            names.extend(entries.keys().map(|name| name.to_ascii_lowercase()));
        }
    }
    Some(names)
}

/// `path` 型のリポジトリ (url は composer.json からの相対パス。最後の要素の `*` / `?` だけを
/// glob として扱う) のパッケージの名前
fn path_names(base: &Path, url: Option<&str>) -> Result<BTreeSet<String>, String> {
    let url = url.ok_or("has no url")?;
    let path = base.join(url);
    let directories = match path.file_name().and_then(|name| name.to_str()) {
        Some(pattern) if pattern.contains(['*', '?']) => {
            let parent = path.parent().ok_or("has no parent directory")?;
            if parent.to_string_lossy().contains(['*', '?']) {
                return Err("uses a glob outside the last path component".into());
            }
            let entries = std::fs::read_dir(parent)
                .map_err(|error| format!("could not list {}: {error}", parent.display()))?;
            let mut directories = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = entry.file_name();
                if entry.path().is_dir() && name.to_str().is_some_and(|name| glob(pattern, name)) {
                    directories.push(entry.path());
                }
            }
            directories
        }
        _ => vec![path],
    };
    let mut names = BTreeSet::new();
    for directory in directories {
        let manifest = directory.join("composer.json");
        let content = std::fs::read_to_string(&manifest)
            .map_err(|error| format!("could not read {}: {error}", manifest.display()))?;
        let definition: Value = serde_json::from_str(&content)
            .map_err(|error| format!("could not parse {}: {error}", manifest.display()))?;
        names.extend(
            supplied_names(&definition)
                .ok_or_else(|| format!("{} has no name", manifest.display()))?,
        );
    }
    Ok(names)
}

/// Composer の `only` / `exclude` のパターン (`*` は任意の文字列)
fn matches(pattern: &str, package: &str) -> bool {
    glob(pattern, package)
}

/// `*` (任意の文字列) と `?` (任意の 1 文字) だけの照合。`[...]` は文字どおりに扱う。
fn glob(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p + 1, n));
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some(&c) if c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((after, start)) => {
                    p = after;
                    n = start + 1;
                    star = Some((after, start + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests;
