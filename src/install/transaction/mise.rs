//! プロジェクト内の mise の lock と依存メタデータ。
use super::*;

pub(super) struct MiseScope {
    pub paths: Vec<PathBuf>,
    pub configs: BTreeSet<PathBuf>,
    pub locks: Vec<super::mise_lock::LockContext>,
}

pub(super) fn managed_paths(
    directory: &Path,
    manifests: &[PathBuf],
    boundary: &Path,
) -> Result<MiseScope, String> {
    let boundary = identity(boundary);
    let directory = identity(directory);
    let mut environments: Vec<_> = std::env::var("MISE_ENV")
        .unwrap_or_default()
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
    if ["MISE_CONFIG_FILE", "MISE_CONFIG_DIR"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return Err("Cannot safely prepare mise recovery with custom config paths".into());
    }
    check_parent_scope(&directory, &boundary, &environments)?;
    let mut configs = BTreeSet::new();
    let mut monorepo = None;
    for ancestor in directory
        .ancestors()
        .take_while(|path| path.starts_with(&boundary))
    {
        let mut layers = Vec::new();
        let suffixes = std::iter::once(String::new())
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
            );
        for suffix in suffixes {
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
        for name in [
            "mise/config.toml",
            ".mise/config.toml",
            ".config/mise.toml",
            ".config/mise/config.toml",
        ] {
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
        // 同じルートの設定は優先度順に重なる。root が実効的に true の最寄りの場所を採る。
        if monorepo.is_some()
            && layers
                .iter()
                .filter_map(|layer| layer.get("monorepo_root"))
                .next_back()
                .and_then(toml::Value::as_bool)
                == Some(true)
        {
            return Err("Cannot safely prepare recovery across nested mise monorepo roots".into());
        }
        if monorepo.is_none() {
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
                monorepo = Some((
                    ancestor.to_path_buf(),
                    shared == Some(true),
                    config_roots.unwrap_or_default(),
                ));
            }
        }
    }
    if let Some((root, true, config_roots)) = &monorepo {
        for pattern in config_roots {
            for project in scope::expand(root, pattern)? {
                for name in crate::manifest::MISE_CONFIG_FILENAMES {
                    let config = project.join(name);
                    match fs::symlink_metadata(&config) {
                        Ok(_) => {
                            let content = fs::read_to_string(&config).map_err(|error| {
                                format!("Cannot read mise monorepo config: {error}")
                            })?;
                            let value: toml::Value = toml::from_str(&content)
                                .map_err(|_| "Cannot parse mise monorepo config")?;
                            if value.get("monorepo_root").and_then(toml::Value::as_bool)
                                == Some(true)
                            {
                                return Err("Cannot safely prepare recovery for a nested mise monorepo config root".into());
                            }
                            configs.insert(config);
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(format!(
                                "Cannot inspect mise monorepo lock scope: {error}"
                            ));
                        }
                    }
                }
            }
        }
    }
    configs.extend(
        manifests
            .iter()
            .filter(|manifest| !manifest.ends_with(".tool-versions"))
            .cloned(),
    );
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
        let destination = if let Some((root, true, _)) = &monorepo {
            root.join(&name)
        } else {
            colocated.clone()
        };
        paths.insert(colocated.clone());
        paths.insert(destination.clone());
        // 別の設定用ディレクトリは、それを含むプロジェクトの設定を読む。
        let project = [
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
        .unwrap_or(parent);
        locks.push(super::mise_lock::LockContext {
            source: identity(&colocated),
            destination: identity(&destination),
            directory: identity(if let Some((root, true, _)) = &monorepo {
                root
            } else {
                project
            }),
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

/// mise は、Cargo から得た許可境界より上の親プロジェクト設定を読むことがある。
fn check_parent_scope(
    directory: &Path,
    boundary: &Path,
    environments: &[String],
) -> Result<(), String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| identity(&path));
    for ancestor in directory
        .ancestors()
        .filter(|ancestor| !ancestor.starts_with(boundary))
    {
        let suffixes = std::iter::once(String::new())
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
            );
        let mut names: Vec<_> = suffixes
            .flat_map(|suffix| [format!("mise{suffix}.toml"), format!(".mise{suffix}.toml")])
            .collect();
        names.extend(
            [
                "mise/config.toml",
                ".mise/config.toml",
                ".config/mise.toml",
                ".config/mise/config.toml",
            ]
            .into_iter()
            .map(str::to_string),
        );
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
            let global = home.as_deref() == Some(ancestor) && name.starts_with(".config/mise");
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
            if parent_monorepo || (!global && (value.get("tools").is_some() || existing_lock)) {
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
                directory.alias.display()
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
                    directory.alias.display()
                ));
                continue;
            }
        };
        let path = directory.alias.join(entry.file_name());
        match directory_guard(&path, boundary, known_links, &mut scan.links) {
            Ok(Some(guard)) => guards.push(guard),
            Ok(None) => {}
            Err(error) => scan
                .errors
                .push(format!("Cannot inspect {}: {error}", path.display())),
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
                            metadata.display()
                        ));
                    }
                    Ok(kind) if kind.is_file() => scan.paths.push(metadata),
                    Ok(_) => scan.errors.push(format!(
                        "Unsupported mise metadata file type: {}",
                        metadata.display()
                    )),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        scan.paths.push(metadata)
                    }
                    Err(error) => scan
                        .errors
                        .push(format!("Cannot inspect {}: {error}", metadata.display())),
                }
            }
        }
    }
}
