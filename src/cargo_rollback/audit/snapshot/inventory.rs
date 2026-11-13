//! workspace のメンバーの集合を、Cargo を実行せずに求める (緊急の復元の前提の確認に使う)。
//!
//! Cargo は `[workspace] members` の各パターンを `glob` crate で展開する。同じく、パスの要素
//! ごとに照合し、glob を含む要素のときだけそのディレクトリの一覧を読む。そのため `target/` や
//! `node_modules/` のように、パターンに一致しえないディレクトリは辿らない (#90)。
use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// 読むディレクトリの項目数の上限。`**` で巨大な木を辿ったときなどに打ち切る。
const MAX_ENTRIES: usize = 100_000;

/// メンバーの Cargo.toml。展開したパス (別名を含む) と、その実体の canonical なパスの組。
pub(super) type Inventory = Vec<(PathBuf, PathBuf)>;

/// `[workspace] members` のパターンに一致するディレクトリの Cargo.toml を列挙する。
/// 照合に要るディレクトリを読めない・上限を超えたときは、欠けた集合で成功させない。
pub(super) fn manifest_inventory(root: &Path) -> Result<Inventory, String> {
    inventory_within(root, MAX_ENTRIES)
}

fn inventory_within(root: &Path, limit: usize) -> Result<Inventory, String> {
    let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let content = match fs::read_to_string(root.join("Cargo.toml")) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let manifest: toml::Value = toml::from_str(&content).map_err(|error| error.to_string())?;
    let mut walk = Walk {
        limit,
        examined: 0,
        visited: HashSet::new(),
        manifests: BTreeSet::new(),
    };
    for pattern in manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let pattern = pattern.as_str().ok_or("invalid workspace member pattern")?;
        let parts = parse(pattern)?;
        // `**` の訪問済みはパターンの中の位置で区別するので、パターンごとに数え直す。
        walk.visited.clear();
        if !walk.expand(root.clone(), &parts)? {
            // Cargo と同じく、何にも一致しないパターンは書かれたパスのまま使う。
            walk.record(&root.join(pattern))?;
        }
    }
    Ok(walk.manifests.into_iter().collect())
}

/// パターンの 1 要素
enum Part {
    /// glob を含まない名前。ディレクトリの一覧を読まずにつなぐ
    Literal(OsString),
    /// `*` `?` `[...]` を含む名前
    Pattern(Vec<char>),
    /// 要素全体の `**` (0 個以上のディレクトリ)
    Recursive,
}

fn parse(pattern: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    for component in Path::new(pattern).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => parts.push(classify(name.to_os_string())?),
            _ => return Err("workspace inventory would leave the project root".into()),
        }
    }
    Ok(parts)
}

fn classify(name: OsString) -> Result<Part, String> {
    let Some(text) = name.to_str() else {
        return Ok(Part::Literal(name));
    };
    if text == "**" {
        return Ok(Part::Recursive);
    }
    if text.contains("**") {
        return Err(format!(
            "invalid workspace member pattern `{text}`: `**` must be a whole path component"
        ));
    }
    if !text.contains(['*', '?', '[']) {
        return Ok(Part::Literal(name));
    }
    let pattern: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < pattern.len() {
        if pattern[index] == '[' {
            index = class_end(&pattern, index).ok_or_else(|| {
                format!("invalid workspace member pattern `{text}`: unclosed `[`")
            })?;
        }
        index += 1;
    }
    Ok(Part::Pattern(pattern))
}

struct Walk {
    limit: usize,
    examined: usize,
    /// `**` で辿ったディレクトリ (canonical) と、そのときの残りの要素数
    visited: HashSet<(PathBuf, usize)>,
    manifests: BTreeSet<(PathBuf, PathBuf)>,
}

impl Walk {
    /// `directory` から `parts` を照合し、メンバーの Cargo.toml を記録する。パターン全体に
    /// 一致するパスが (ディレクトリでなくても) 1 つでもあったかを返す。
    fn expand(&mut self, directory: PathBuf, parts: &[Part]) -> Result<bool, String> {
        let Some((part, rest)) = parts.split_first() else {
            let found = directory.exists();
            self.record(&directory)?;
            return Ok(found);
        };
        match part {
            Part::Literal(name) => self.expand(directory.join(name), rest),
            Part::Pattern(pattern) => {
                let mut found = false;
                for (name, path, is_dir) in self.entries(&directory)? {
                    if !name.to_str().is_some_and(|name| matches(pattern, name)) {
                        continue;
                    }
                    if rest.is_empty() {
                        // Cargo は一致したもののうちディレクトリだけをメンバーにする。
                        found = true;
                        if is_dir {
                            self.record(&path)?;
                        }
                    } else if is_dir {
                        found |= self.expand(path, rest)?;
                    }
                }
                Ok(found)
            }
            Part::Recursive => {
                let key = match fs::canonicalize(&directory) {
                    Ok(key) => key,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(false);
                    }
                    Err(error) => {
                        return Err(format!(
                            "{}: {error}",
                            crate::user_path::display(&directory)
                        ));
                    }
                };
                // symlink の輪や、別の経路で同じディレクトリに同じ位置で戻ったら辿らない。
                if !key.is_dir() || !self.visited.insert((key, rest.len())) {
                    return Ok(false);
                }
                let mut found = self.expand(directory.clone(), rest)?;
                for (_, path, is_dir) in self.entries(&directory)? {
                    if is_dir {
                        found |= self.expand(path, parts)?;
                    }
                }
                Ok(found)
            }
        }
    }

    /// メンバーのディレクトリの Cargo.toml を、展開したパスと実体の組で記録する。
    fn record(&mut self, directory: &Path) -> Result<(), String> {
        let manifest = directory.join("Cargo.toml");
        if manifest.is_file() {
            let canonical = fs::canonicalize(&manifest)
                .map_err(|error| format!("{}: {error}", crate::user_path::display(&manifest)))?;
            self.manifests.insert((manifest, canonical));
        }
        Ok(())
    }

    /// `directory` の直下の項目と、それがディレクトリ (を指す symlink) か。
    /// `directory` がディレクトリでなければ空。
    fn entries(&mut self, directory: &Path) -> Result<Vec<(OsString, PathBuf, bool)>, String> {
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let unreadable = |error: std::io::Error| {
            format!(
                "workspace inventory could not read {}: {error}",
                crate::user_path::display(directory)
            )
        };
        let mut entries = Vec::new();
        for entry in fs::read_dir(directory).map_err(unreadable)? {
            self.examined += 1;
            if self.examined > self.limit {
                return Err(format!(
                    "workspace inventory exceeded {} entries",
                    self.limit
                ));
            }
            let entry = entry.map_err(unreadable)?;
            let path = entry.path();
            let kind = entry.file_type().map_err(unreadable)?;
            let is_dir = kind.is_dir() || (kind.is_symlink() && path.is_dir());
            entries.push((entry.file_name(), path, is_dir));
        }
        Ok(entries)
    }
}

/// `glob` crate の既定と同じ規則で、パスの 1 要素を照合する。`*` は任意の文字列、`?` は
/// 任意の 1 文字、`[...]` は集合 (`[!...]` は否定、`a-z` は範囲)。大文字と小文字を区別し、
/// 先頭のドットにも `*` が一致する。
fn matches(pattern: &[char], name: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    // 直前の `*` の直後の位置と、その `*` が飲み込み始めた名前の位置
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        let next = match pattern.get(p) {
            Some('*') => {
                star = Some((p + 1, n));
                p += 1;
                continue;
            }
            Some('?') => Some(p + 1),
            Some('[') => {
                let end = class_end(pattern, p).expect("validated by classify");
                let negated = pattern[p + 1] == '!';
                let set = &pattern[p + 1 + usize::from(negated)..end];
                (class_contains(set, name[n]) != negated).then_some(end + 1)
            }
            Some(&c) if c == name[n] => Some(p + 1),
            _ => None,
        };
        match (next, star) {
            (Some(next), _) => {
                p = next;
                n += 1;
            }
            // 直前の `*` に 1 文字多く飲み込ませてやり直す。
            (None, Some((after, start))) => {
                p = after;
                n = start + 1;
                star = Some((after, start + 1));
            }
            (None, None) => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// `pattern[start]` の `[` から始まる集合を閉じる `]` の位置。`[` や `[!` の直後の `]` は
/// 集合の文字として扱う (`glob` crate と同じ)。
fn class_end(pattern: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if pattern.get(index) == Some(&'!') {
        index += 1;
    }
    if pattern.get(index) == Some(&']') {
        index += 1;
    }
    (index..pattern.len()).find(|&index| pattern[index] == ']')
}

/// 集合の中身 (`[` と `!` の後から `]` の前まで) が `c` を含むか。先頭か末尾の `-` は文字。
fn class_contains(set: &[char], c: char) -> bool {
    let mut index = 0;
    while index < set.len() {
        if index + 2 < set.len() && set[index + 1] == '-' {
            if (set[index]..=set[index + 2]).contains(&c) {
                return true;
            }
            index += 3;
        } else {
            if set[index] == c {
                return true;
            }
            index += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests;
