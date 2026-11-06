//! npm / pnpm / Yarn / Bun のレジストリの設定。
//!
//! マニフェストのディレクトリから、リポジトリのルート (`.git` のあるディレクトリ。なければ
//! 実行のルート) までの各ディレクトリにある `.npmrc`・`.yarnrc` (Yarn 1)・`.yarnrc.yml`
//! (Yarn 2 以降)・`bunfig.toml`・`pnpm-workspace.yaml` を読む。workspace のルートの設定が
//! メンバーに効くため、マニフェストのディレクトリだけでは足りない。ホームディレクトリの設定
//! (ユーザーの設定) は読まない。PM ごとに効くファイルは違うが、どれかが公開レジストリ以外を
//! 指していれば外す (公開だと言い切れない依存を公開の版で更新しないため。PM の推定違いで
//! 漏らさない)。
use super::{Verdict, is_public_npm_registry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// レジストリの設定 1 件
#[derive(Debug, Clone)]
struct Setting {
    /// どのファイルのどの設定か (表示用。認証情報を含みうる URL は出さない)
    origin: String,
    /// 公開レジストリか。値を解釈できなければその理由
    public: Result<bool, String>,
}

/// Node のマニフェストに効くレジストリの設定
#[derive(Debug, Default)]
pub(crate) struct NodeSources {
    /// 既定のレジストリの設定
    default: Vec<Setting>,
    /// scope (`@example`) ごとの設定
    scopes: BTreeMap<String, Vec<Setting>>,
    /// 読めない・解釈できない設定ファイル。どの依存の取得元も確かめられない
    unreadable: Vec<String>,
}

impl NodeSources {
    /// `directory` (マニフェストのディレクトリ) からリポジトリのルートまでの各ディレクトリの
    /// 設定を読む。リポジトリの外なら実行のルート `root` まで、`root` の外なら `directory` だけ。
    pub(crate) fn load(directory: &Path, root: &Path) -> Self {
        let mut sources = Self::default();
        for directory in chain(directory, root) {
            sources.read_npmrc(&directory);
            sources.read_yarnrc(&directory);
            sources.read_yarnrc_yml(&directory);
            sources.read_bunfig(&directory);
            sources.read_pnpm_workspace(&directory);
        }
        sources
    }

    /// `package` (実際のパッケージ名。npm alias なら参照先) の取得元。scope の設定があれば
    /// それに従い (既定が私設でも、公開を明示した scope は残す)、なければ既定に従う。
    pub(crate) fn verdict(&self, package: &str) -> Verdict {
        if !self.unreadable.is_empty() {
            return Verdict::Unverified(self.unreadable.join("; "));
        }
        let scoped = package
            .strip_prefix('@')
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(scope, _)| self.scopes.get(&format!("@{scope}")));
        for setting in scoped.unwrap_or(&self.default) {
            match &setting.public {
                Ok(true) => {}
                Ok(false) => return Verdict::NonPublic(setting.origin.clone()),
                Err(reason) => {
                    return Verdict::Unverified(format!("{} {reason}", setting.origin));
                }
            }
        }
        Verdict::Public
    }

    /// 設定ファイルの中身。ないファイルは `None`、読めないファイルは記録して `None`。
    fn read(&mut self, directory: &Path, name: &str) -> Option<String> {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                self.unreadable
                    .push(format!("{name} could not be inspected: {error}"));
                return None;
            }
            Ok(_) => {}
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => Some(content),
            Err(error) => {
                self.unreadable
                    .push(format!("{name} could not be read: {error}"));
                None
            }
        }
    }

    fn push(&mut self, scope: Option<&str>, setting: Setting) {
        match scope {
            Some(scope) => {
                let scope = format!("@{}", scope.trim_start_matches('@'));
                self.scopes.entry(scope).or_default().push(setting);
            }
            None => self.default.push(setting),
        }
    }

    /// `.npmrc` の `registry=` と `@scope:registry=` (npm / pnpm / Yarn 1 / Bun が読む)
    fn read_npmrc(&mut self, directory: &Path) {
        let Some(content) = self.read(directory, ".npmrc") else {
            return;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with(['#', ';']) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if let Some(scope) = registry_key(key) {
                let setting = setting(".npmrc", key, unquote(value.trim()), "${");
                self.push(scope, setting);
            }
        }
    }

    /// Yarn 1 の `.yarnrc` (`registry "https://..."` / `"@scope:registry" "https://..."`)
    fn read_yarnrc(&mut self, directory: &Path) {
        let Some(content) = self.read(directory, ".yarnrc") else {
            return;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = split_quoted_key(line) else {
                continue;
            };
            if let Some(scope) = registry_key(key) {
                let setting = setting(".yarnrc", key, unquote(value.trim()), "${");
                self.push(scope, setting);
            }
        }
    }

    /// Yarn 2 以降の `.yarnrc.yml` の `npmRegistryServer` と `npmScopes.<scope>.npmRegistryServer`
    fn read_yarnrc_yml(&mut self, directory: &Path) {
        const FILE: &str = ".yarnrc.yml";
        let Some(content) = self.read(directory, FILE) else {
            return;
        };
        let mut in_scopes = false;
        let mut scope: Option<(usize, String)> = None;
        for (indent, key, value) in yaml_entries(&content) {
            if indent == 0 {
                in_scopes = false;
                scope = None;
                match key.as_str() {
                    "npmRegistryServer" => {
                        let setting = setting(FILE, "npmRegistryServer", &value, "${");
                        self.push(None, setting);
                    }
                    "npmScopes" if value.is_empty() => in_scopes = true,
                    "npmScopes" => {
                        self.unreadable
                            .push(format!("{FILE} npmScopes is not in block style"));
                    }
                    _ => {}
                }
                continue;
            }
            if !in_scopes {
                continue;
            }
            match &scope {
                Some((scope_indent, name)) if indent > *scope_indent => {
                    if key == "npmRegistryServer" {
                        let origin = format!("npmScopes.{name}.npmRegistryServer");
                        let setting = setting(FILE, &origin, &value, "${");
                        let name = name.clone();
                        self.push(Some(&name), setting);
                    }
                }
                _ if value.is_empty() => scope = Some((indent, key)),
                _ => {
                    self.unreadable
                        .push(format!("{FILE} npmScopes.{key} is not in block style"));
                }
            }
        }
    }

    /// Bun の `bunfig.toml` の `install.registry` と `install.scopes` (文字列か `url` を持つ表)
    fn read_bunfig(&mut self, directory: &Path) {
        const FILE: &str = "bunfig.toml";
        let Some(content) = self.read(directory, FILE) else {
            return;
        };
        let value: toml::Value = match toml::from_str(&content) {
            Ok(value) => value,
            Err(error) => {
                self.unreadable
                    .push(format!("{FILE} could not be parsed: {error}"));
                return;
            }
        };
        let Some(install) = value.get("install") else {
            return;
        };
        if let Some(registry) = install.get("registry") {
            let setting = bun_setting("install.registry", registry);
            self.push(None, setting);
        }
        match install.get("scopes") {
            None => {}
            Some(toml::Value::Table(scopes)) => {
                for (name, entry) in scopes {
                    let setting = bun_setting(&format!("install.scopes.{name}"), entry);
                    self.push(Some(name), setting);
                }
            }
            Some(_) => self
                .unreadable
                .push(format!("{FILE} install.scopes is not a table")),
        }
    }

    /// `pnpm-workspace.yaml` の `registry` と `registries` (`default` / `@scope`)
    fn read_pnpm_workspace(&mut self, directory: &Path) {
        const FILE: &str = "pnpm-workspace.yaml";
        let Some(content) = self.read(directory, FILE) else {
            return;
        };
        let mut in_registries = false;
        for (indent, key, value) in yaml_entries(&content) {
            if indent == 0 {
                in_registries = false;
                match key.as_str() {
                    "registry" => self.push(None, setting(FILE, "registry", &value, "${")),
                    "registries" if value.is_empty() => in_registries = true,
                    "registries" => {
                        self.unreadable
                            .push(format!("{FILE} registries is not in block style"));
                    }
                    _ => {}
                }
            } else if in_registries {
                let origin = format!("registries.{key}");
                let setting = setting(FILE, &origin, &value, "${");
                if key == "default" {
                    self.push(None, setting);
                } else if key.starts_with('@') {
                    self.push(Some(&key), setting);
                } else {
                    self.unreadable
                        .push(format!("{FILE} registries.{key} is not a scope"));
                }
            }
        }
    }
}

/// 設定を読むディレクトリ (近い順)。リポジトリのルート、なければ実行のルートで止め、
/// ホームディレクトリには入らない。
fn chain(directory: &Path, root: &Path) -> Vec<PathBuf> {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
    let directory = canonical(directory);
    let root = canonical(root);
    let home = dirs::home_dir().map(|home| canonical(&home));
    let boundary = directory
        .ancestors()
        .take_while(|ancestor| Some(*ancestor) != home.as_deref())
        .find(|ancestor| ancestor.join(".git").exists())
        .or_else(|| directory.starts_with(&root).then_some(root.as_path()))
        .unwrap_or(&directory)
        .to_path_buf();
    let mut chain = Vec::new();
    for ancestor in directory.ancestors() {
        if Some(ancestor) == home.as_deref() {
            break;
        }
        chain.push(ancestor.to_path_buf());
        if ancestor == boundary {
            break;
        }
    }
    chain
}

/// npm の設定のキーがレジストリを指すなら、その scope (既定なら `None`)。
fn registry_key(key: &str) -> Option<Option<&str>> {
    if key.eq_ignore_ascii_case("registry") {
        return Some(None);
    }
    let scope = key.strip_suffix(":registry")?;
    scope.starts_with('@').then_some(Some(scope))
}

fn setting(file: &str, key: &str, value: &str, variable: &str) -> Setting {
    let origin = format!("{file}: {key}");
    let public = if value.contains(variable) {
        Err("uses an environment variable".into())
    } else if value.is_empty() {
        Err("is empty".into())
    } else {
        Ok(is_public_npm_registry(value))
    };
    Setting { origin, public }
}

fn bun_setting(key: &str, value: &toml::Value) -> Setting {
    const FILE: &str = "bunfig.toml";
    match value {
        // Bun は `$NAME` を環境変数として展開する。
        toml::Value::String(url) => setting(FILE, key, url, "$"),
        toml::Value::Table(table) => match table.get("url").and_then(toml::Value::as_str) {
            Some(url) => setting(FILE, key, url, "$"),
            None => Setting {
                origin: format!("{FILE}: {key}"),
                public: Err("has no url".into()),
            },
        },
        _ => Setting {
            origin: format!("{FILE}: {key}"),
            public: Err("has an unsupported value".into()),
        },
    }
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// `.yarnrc` の 1 行を、引用符を考慮してキーと値に分ける。
fn split_quoted_key(line: &str) -> Option<(&str, &str)> {
    if let Some(rest) = line.strip_prefix('"') {
        let (key, value) = rest.split_once('"')?;
        return Some((key, value));
    }
    line.split_once(char::is_whitespace)
}

/// YAML の `key: value` の行を、字下げ・キー・値 (引用符と行末のコメントを除く) で並べる。
/// 依存の取得元に関わるキーだけを読む簡易な読み取りで、行が続く値やフロー形式は扱わない
/// (呼び出し側が値の形を確かめ、想定外なら取得元を確かめられないとする)。
fn yaml_entries(content: &str) -> impl Iterator<Item = (usize, String, String)> + '_ {
    content.lines().filter_map(|line| {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            return None;
        }
        let indent = line.len() - trimmed.len();
        let (key, value) = split_yaml_key(trimmed)?;
        let value = value
            .split_once(" #")
            .map_or(value, |(value, _)| value)
            .trim();
        Some((indent, unquote(key).to_string(), unquote(value).to_string()))
    })
}

/// `key: value` を分ける。キーは引用符で囲まれていてもよい。
fn split_yaml_key(line: &str) -> Option<(&str, &str)> {
    for quote in ['"', '\''] {
        if let Some(rest) = line.strip_prefix(quote) {
            let (key, rest) = rest.split_once(quote)?;
            return Some((key, rest.trim_start().strip_prefix(':')?));
        }
    }
    let (key, value) = line.split_once(':')?;
    // URL の `https:` のように、キーの後に空白か行末が続かないものは値の一部
    (value.is_empty() || value.starts_with(char::is_whitespace)).then_some((key.trim(), value))
}

#[cfg(test)]
mod tests;
