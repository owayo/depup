//! プロジェクト内の mise の lock と依存メタデータ。
use super::*;

pub(super) struct MiseScope {
    pub paths: Vec<PathBuf>,
    pub configs: BTreeSet<PathBuf>,
    pub locks: Vec<super::mise_lock::LockContext>,
}

/// プロジェクトのディレクトリの中で、mise の設定を置ける別の場所。祖先ごとに、この順に読む。
const ALTERNATE_CONFIGS: [&str; 4] = [
    "mise/config.toml",
    ".mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
];

/// 祖先の設定で `monorepo_root` が実効的に true だった、最寄りの monorepo の root。
struct MonorepoScope {
    root: PathBuf,
    /// `[monorepo] lockfile = true`。lock を root にまとめ、`config_roots` の設定も同じ lock を使う
    shared_lock: bool,
    config_roots: Vec<String>,
}

pub(super) fn managed_paths(
    directory: &Path,
    manifests: &[PathBuf],
    boundary: &Path,
) -> Result<MiseScope, String> {
    let boundary = identity(boundary);
    let directory = identity(directory);
    let environments =
        active_environments(&std::env::var("MISE_ENV").unwrap_or_default(), manifests);
    if ["MISE_CONFIG_FILE", "MISE_CONFIG_DIR"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return Err("Cannot safely prepare mise recovery with custom config paths".into());
    }
    // ホームは `HOME` だけでなく Windows の `USERPROFILE` からも求める (`native_age` と同じ)。
    // `HOME` がない Windows では、ホームのグローバルな設定を親のプロジェクトの設定と誤認していた (#97)。
    let home = dirs::home_dir().map(|home| identity(&home));
    let global = crate::manifest::native_age::global_config_path(Language::Mise);
    check_parent_scope(
        &directory,
        &boundary,
        &environments,
        home.as_deref(),
        global.as_deref(),
    )?;
    let mut configs = BTreeSet::new();
    let mut monorepo = None;
    for ancestor in directory
        .ancestors()
        .take_while(|path| path.starts_with(&boundary))
    {
        let layers = read_ancestor_configs(ancestor, &environments, &mut configs)?;
        update_monorepo_scope(ancestor, layers, &mut monorepo)?;
    }
    let shared = monorepo.filter(|monorepo| monorepo.shared_lock);
    if let Some(monorepo) = &shared {
        extend_shared_configs(monorepo, &mut configs)?;
    }
    configs.extend(
        manifests
            .iter()
            .filter(|manifest| !manifest.ends_with(".tool-versions"))
            .cloned(),
    );
    build_mise_scope(
        configs,
        shared.as_ref().map(|monorepo| monorepo.root.as_path()),
    )
}

/// 有効な mise の環境。`MISE_ENV` の値 (重複も書いたまま残す) に、明示的に選んだ環境別の設定
/// (`mise.<env>.toml`・`mise.<env>.local.toml`) の環境のうち、まだ無いものを足す。
fn active_environments(mise_env: &str, manifests: &[PathBuf]) -> Vec<String> {
    let mut environments: Vec<_> = mise_env
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    // 環境用の設定を明示的に選んだ場合、修復でその環境を有効にする前に、
    // ローカル設定と親の設定も同じ復元範囲へ含める。
    for manifest in manifests {
        let Some(environment) = manifest
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.trim_start_matches('.').strip_prefix("mise."))
            .and_then(|name| name.strip_suffix(".toml"))
            .map(|name| name.strip_suffix(".local").unwrap_or(name))
            .filter(|name| !name.is_empty() && *name != "local")
        else {
            continue;
        };
        if !environments.iter().any(|known| known == environment) {
            environments.push(environment.to_string());
        }
    }
    environments
}

/// mise の設定ファイル名の接尾辞を、優先度の低い順 (`""` → `.<env>` → `.local` →
/// `.<env>.local`) に返す。
fn config_suffixes(environments: &[String]) -> impl Iterator<Item = String> + '_ {
    std::iter::once(String::new())
        .chain(
            environments
                .iter()
                .map(|environment| format!(".{environment}")),
        )
        .chain(std::iter::once(".local".into()))
        .chain(
            environments
                .iter()
                .map(|environment| format!(".{environment}.local")),
        )
}

/// 祖先 `ancestor` の設定を読み、見つけたものを `configs` に足す。`mise[.<suffix>].toml` の層を
/// 優先度の低い順に返す。別の場所の設定 (`ALTERNATE_CONFIGS`) は層に入れず、monorepo の設定が
/// あれば止める。
fn read_ancestor_configs(
    ancestor: &Path,
    environments: &[String],
    configs: &mut BTreeSet<PathBuf>,
) -> Result<Vec<toml::Value>, String> {
    let mut layers = Vec::new();
    for suffix in config_suffixes(environments) {
        for prefix in ["mise", ".mise"] {
            let config = ancestor.join(format!("{prefix}{suffix}.toml"));
            match fs::read_to_string(&config) {
                Ok(content) => {
                    let value: toml::Value = toml::from_str(&content)
                        .map_err(|_| "Cannot parse active mise config for recovery")?;
                    layers.push(value);
                    configs.insert(config);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Cannot read active mise config: {error}")),
            }
        }
    }
    for name in ALTERNATE_CONFIGS {
        let config = ancestor.join(name);
        match fs::read_to_string(&config) {
            Ok(content) => {
                let value: toml::Value = toml::from_str(&content)
                    .map_err(|_| "Cannot parse mise config for recovery")?;
                if value.get("monorepo_root").is_some() || value.get("monorepo").is_some() {
                    return Err("Cannot safely prepare mise monorepo recovery from alternate config containers".into());
                }
                configs.insert(config);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot read mise config: {error}")),
        }
    }
    Ok(layers)
}

/// 祖先を近い順にたどりながら、その祖先の層 (`layers`) で monorepo の root を判定する。
/// root は最寄りの祖先のものを採る。それより上にも root があれば、入れ子として止める。
fn update_monorepo_scope(
    ancestor: &Path,
    layers: Vec<toml::Value>,
    monorepo: &mut Option<MonorepoScope>,
) -> Result<(), String> {
    if monorepo.is_some() {
        // 同じルートの設定は優先度順に重なる。root が実効的に true の場所が上にもあれば入れ子。
        if layers
            .iter()
            .filter_map(|layer| layer.get("monorepo_root"))
            .next_back()
            .and_then(toml::Value::as_bool)
            == Some(true)
        {
            return Err("Cannot safely prepare recovery across nested mise monorepo roots".into());
        }
        return Ok(());
    }
    let mut enabled = None;
    let mut shared = None;
    let mut config_roots = None;
    for layer in layers {
        if let Some(value) = layer.get("monorepo_root") {
            enabled = Some(value.as_bool().ok_or("Invalid mise monorepo_root")?);
        }
        if let Some(value) = layer
            .get("monorepo")
            .and_then(|table| table.get("config_roots"))
        {
            config_roots = Some(
                value
                    .as_array()
                    .ok_or("Invalid mise config_roots")?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .ok_or("Invalid mise config_roots entry")
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        if let Some(value) = layer
            .get("monorepo")
            .and_then(|table| table.get("lockfile"))
        {
            shared = Some(
                value
                    .as_bool()
                    .ok_or("Invalid mise monorepo lockfile setting")?,
            );
        }
    }
    if enabled == Some(true) {
        *monorepo = Some(MonorepoScope {
            root: ancestor.to_path_buf(),
            shared_lock: shared == Some(true),
            config_roots: config_roots.unwrap_or_default(),
        });
    }
    Ok(())
}

/// lock を root にまとめる monorepo では、`config_roots` のプロジェクトの設定も同じ lock を使うので
/// 復元の範囲に含める。そこに別の monorepo の root があれば止める。
fn extend_shared_configs(
    monorepo: &MonorepoScope,
    configs: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    for pattern in &monorepo.config_roots {
        for project in scope::expand(&monorepo.root, pattern)? {
            for name in crate::manifest::MISE_CONFIG_FILENAMES {
                let config = project.join(name);
                match fs::symlink_metadata(&config) {
                    Ok(_) => {
                        let content = fs::read_to_string(&config).map_err(|error| {
                            format!("Cannot read mise monorepo config: {error}")
                        })?;
                        let value: toml::Value = toml::from_str(&content)
                            .map_err(|_| "Cannot parse mise monorepo config")?;
                        if value.get("monorepo_root").and_then(toml::Value::as_bool) == Some(true) {
                            return Err("Cannot safely prepare recovery for a nested mise monorepo config root".into());
                        }
                        configs.insert(config);
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("Cannot inspect mise monorepo lock scope: {error}"));
                    }
                }
            }
        }
    }
    Ok(())
}

/// 設定ごとに、mise が書く lock の場所と、lock を作り直すときの文脈を決める。
/// `shared_root` は lock をまとめる monorepo の root。
fn build_mise_scope(
    configs: BTreeSet<PathBuf>,
    shared_root: Option<&Path>,
) -> Result<MiseScope, String> {
    let mut paths = BTreeSet::new();
    let mut locks = Vec::new();
    for config in &configs {
        let parent = config.parent().ok_or("Mise config has no directory")?;
        let filename = config
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Mise config filename is not UTF-8")?;
        let stem = filename
            .trim_start_matches('.')
            .strip_suffix(".toml")
            .ok_or("Unsupported mise config filename")?;
        let suffix = stem
            .strip_prefix("mise")
            .or_else(|| stem.strip_prefix("config"))
            .ok_or("Unsupported mise config filename")?;
        let name = format!("mise{suffix}.lock");
        // mise が移行する先の lock と、以前の同じ場所の lock を両方保存する。
        let colocated = parent.join(&name);
        let destination = shared_root.map_or_else(|| colocated.clone(), |root| root.join(&name));
        paths.insert(colocated.clone());
        paths.insert(destination.clone());
        locks.push(super::mise_lock::LockContext {
            source: identity(&colocated),
            destination: identity(&destination),
            directory: identity(shared_root.unwrap_or_else(|| project_directory(config, parent))),
            config: identity(config),
            local: suffix == ".local" || suffix.ends_with(".local"),
            environment: suffix
                .strip_prefix('.')
                .map(|suffix| suffix.strip_suffix(".local").unwrap_or(suffix))
                .filter(|environment| !environment.is_empty() && *environment != "local")
                .map(str::to_string),
        });
    }
    // 個々のスナップショットの失敗とは独立して、確定した論理パスを返す。
    // トランザクションは、それぞれの対象を復元の境界に照らして検証する。
    Ok(MiseScope {
        paths: paths.into_iter().collect(),
        configs: configs
            .into_iter()
            .map(|config| identity(&config))
            .collect(),
        locks,
    })
}

/// 別の場所の設定 (`.config/mise/config.toml` など) は、それを含むプロジェクトの設定として読む。
/// `Path::ends_with` は要素の末尾で照合するので、長い名前から試す
/// (`.config/mise/config.toml` は `mise/config.toml` にも一致する)。
fn project_directory<'a>(config: &'a Path, parent: &'a Path) -> &'a Path {
    [
        ".config/mise/config.toml",
        "mise/config.toml",
        ".mise/config.toml",
        ".config/mise.toml",
    ]
    .into_iter()
    .find_map(|suffix| {
        config.ends_with(suffix).then(|| {
            config
                .ancestors()
                .nth(Path::new(suffix).components().count())
                .unwrap()
        })
    })
    .unwrap_or(parent)
}

/// mise は、Cargo から得た許可境界より上の親プロジェクト設定を読むことがある。`home` の
/// `.config/mise` の設定と、mise のグローバルな設定 `global` は、親のプロジェクトとみなさない。
fn check_parent_scope(
    directory: &Path,
    boundary: &Path,
    environments: &[String],
    home: Option<&Path>,
    global: Option<&Path>,
) -> Result<(), String> {
    for ancestor in directory
        .ancestors()
        .filter(|ancestor| !ancestor.starts_with(boundary))
    {
        let mut names: Vec<_> = config_suffixes(environments)
            .flat_map(|suffix| [format!("mise{suffix}.toml"), format!(".mise{suffix}.toml")])
            .collect();
        names.extend(ALTERNATE_CONFIGS.into_iter().map(str::to_string));
        for name in names {
            let config = ancestor.join(&name);
            let content = match fs::read_to_string(&config) {
                Ok(content) => content,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("Cannot confirm parent mise scope: {error}")),
            };
            let value: toml::Value =
                toml::from_str(&content).map_err(|_| "Cannot parse parent mise scope")?;
            let parent_monorepo = value
                .get("monorepo_root")
                .map(|value| value.as_bool().ok_or("Invalid parent mise monorepo_root"))
                .transpose()?
                == Some(true)
                || value
                    .get("monorepo")
                    .and_then(|table| table.get("lockfile"))
                    .map(|value| value.as_bool().ok_or("Invalid parent mise lockfile scope"))
                    .transpose()?
                    == Some(true);
            // 標準のホーム設定はグローバルなので、その install 生成物は対象外にする。
            let is_global = (home == Some(ancestor) && name.starts_with(".config/mise"))
                || global.is_some_and(|global| {
                    crate::manifest::native_age::same_config_path(&config, global)
                });
            let stem = config
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("")
                .trim_start_matches('.');
            let suffix = stem
                .strip_prefix("mise")
                .or_else(|| stem.strip_prefix("config"))
                .ok_or("Unsupported parent mise config filename")?;
            let existing_lock =
                match fs::symlink_metadata(config.with_file_name(format!("mise{suffix}.lock"))) {
                    Ok(_) => true,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(format!("Cannot confirm parent mise lock scope: {error}"));
                    }
                };
            if parent_monorepo || (!is_global && (value.get("tools").is_some() || existing_lock)) {
                return Err("Parent mise project scope is outside the recovery boundary; run from the project root".into());
            }
        }
    }
    Ok(())
}

pub(super) fn sidecar(lock: &Path) -> PathBuf {
    let lock = identity(lock);
    let parent = lock.parent().unwrap();
    let stem = lock.file_stem().unwrap();
    let base = if parent
        .file_name()
        .is_some_and(|name| name == ".mise" || name == "mise")
    {
        parent.join("locks")
    } else if parent.file_name().is_some_and(|name| name == ".config") {
        parent.join("mise/locks")
    } else {
        parent.join(".mise/locks")
    };
    if stem == "mise" {
        base
    } else {
        base.join(stem)
    }
}

pub(super) struct TreeFiles {
    pub paths: Vec<PathBuf>,
    pub links: BTreeMap<PathBuf, FileGuard>,
    pub errors: Vec<String>,
}

pub(super) fn tree_files(
    guard: &FileGuard,
    boundary: &Path,
    known_links: Option<&BTreeMap<PathBuf, FileGuard>>,
) -> io::Result<TreeFiles> {
    if !guard.target.starts_with(boundary) {
        return Err(io::Error::other(
            "Mise metadata resolves outside the permitted recovery boundary",
        ));
    }
    guard.check()?;
    let mut scan = TreeFiles {
        paths: vec![],
        links: BTreeMap::new(),
        errors: vec![],
    };
    visit(guard, &mut scan, boundary, known_links);
    scan.paths.sort();
    scan.errors.sort();
    Ok(scan)
}
fn directory_guard(
    path: &Path,
    boundary: &Path,
    known_links: Option<&BTreeMap<PathBuf, FileGuard>>,
    links: &mut BTreeMap<PathBuf, FileGuard>,
) -> io::Result<Option<FileGuard>> {
    let kind = fs::symlink_metadata(path)?;
    if let Some(known_links) = known_links {
        if let Some(original) = known_links.get(path) {
            original.check()?;
        } else if kind.file_type().is_symlink() {
            return Err(io::Error::other(
                "Mise metadata directory link has unknown original target; kept for manual recovery",
            ));
        }
    }
    if !kind.is_dir() && !kind.file_type().is_symlink() {
        return Ok(None);
    }
    let guard = FileGuard::capture(path)?;
    if !guard.target.starts_with(boundary) {
        return Err(io::Error::other(
            "Mise metadata directory resolves outside the permitted recovery boundary",
        ));
    }
    guard.check()?;
    if !fs::metadata(&guard.target)?.is_dir() {
        return Err(io::Error::other(
            "Mise metadata directory link is not a directory",
        ));
    }
    if kind.file_type().is_symlink() {
        links.insert(guard.alias.clone(), guard.clone());
    }
    Ok(Some(guard))
}
fn directories(
    directory: &FileGuard,
    scan: &mut TreeFiles,
    boundary: &Path,
    known_links: Option<&BTreeMap<PathBuf, FileGuard>>,
) -> Vec<FileGuard> {
    let entries = match fs::read_dir(&directory.target) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return vec![],
        Err(error) => {
            scan.errors.push(format!(
                "Cannot enumerate {}: {error}",
                crate::user_path::display(&directory.alias)
            ));
            return vec![];
        }
    };
    let mut guards = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                scan.errors.push(format!(
                    "Cannot read {}: {error}",
                    crate::user_path::display(&directory.alias)
                ));
                continue;
            }
        };
        let path = directory.alias.join(entry.file_name());
        match directory_guard(&path, boundary, known_links, &mut scan.links) {
            Ok(Some(guard)) => guards.push(guard),
            Ok(None) => {}
            Err(error) => scan.errors.push(format!(
                "Cannot inspect {}: {error}",
                crate::user_path::display(&path)
            )),
        }
    }
    guards.sort_by(|left, right| left.alias.cmp(&right.alias));
    guards
}

fn visit(
    directory: &FileGuard,
    scan: &mut TreeFiles,
    boundary: &Path,
    known_links: Option<&BTreeMap<PathBuf, FileGuard>>,
) {
    for tool in directories(directory, scan, boundary, known_links) {
        for version in directories(&tool, scan, boundary, known_links) {
            let path = version.alias;
            // 不在や一部だけのメタデータも含め、判明した NativeGraph の各パスを保存する。
            for name in [
                "uv.lock",
                "pyproject.toml",
                "aube-lock.yaml",
                "package.json",
            ] {
                let metadata = path.join(name);
                match fs::symlink_metadata(&metadata) {
                    Ok(kind) if kind.file_type().is_symlink() => {
                        scan.errors.push(format!(
                            "Mise metadata symlink requires manual recovery: {}",
                            crate::user_path::display(&metadata)
                        ));
                    }
                    Ok(kind) if kind.is_file() => scan.paths.push(metadata),
                    Ok(_) => scan.errors.push(format!(
                        "Unsupported mise metadata file type: {}",
                        crate::user_path::display(&metadata)
                    )),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        scan.paths.push(metadata)
                    }
                    Err(error) => scan.errors.push(format!(
                        "Cannot inspect {}: {error}",
                        crate::user_path::display(&metadata)
                    )),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
