//! install の解決結果を変えず、既存の lock の対象範囲を維持する。
use super::*;

pub(super) struct LockContext {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub directory: PathBuf,
    pub config: PathBuf,
    pub environment: Option<String>,
    pub local: bool,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExecutionContext {
    directory: PathBuf,
    environment: Option<String>,
    local: bool,
}

#[derive(Clone, Debug)]
struct Entry {
    tool: String,
    resolution: toml::Value,
    platforms: BTreeMap<String, (bool, bool)>,
    metadata: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq)]
struct Lock {
    format: Option<toml::Value>,
    resolutions: Vec<(String, toml::Value)>,
    metadata: Vec<BTreeMap<String, toml::Value>>,
}

struct Required {
    source: PathBuf,
    destination: PathBuf,
    contexts: BTreeSet<ExecutionContext>,
    entry: Entry,
}

pub(super) struct Preservation {
    required: Vec<Required>,
    formats: BTreeMap<PathBuf, Option<toml::Value>>,
    paths: BTreeSet<PathBuf>,
    migrations: BTreeMap<PathBuf, PathBuf>,
}

pub(super) struct Repair {
    pub commands: Vec<PreparedInstall>,
    baseline: BTreeMap<PathBuf, Lock>,
    required: Vec<(PathBuf, Entry)>,
    targets: BTreeSet<(PathBuf, String)>,
}

#[derive(Default)]
struct PlatformRequest {
    platforms: BTreeSet<String>,
    missing: bool,
    tool: String,
}

fn parse(bytes: Option<&[u8]>) -> Result<(Lock, Vec<Entry>), String> {
    let content =
        std::str::from_utf8(bytes.unwrap_or_default()).map_err(|_| "Mise lock is not UTF-8")?;
    let value: toml::Value =
        toml::from_str(content).map_err(|_| "Cannot parse mise lock for platform preservation")?;
    let mut entries = Vec::new();
    if let Some(tools) = value.get("tools") {
        for (tool, versions) in tools.as_table().ok_or("Invalid mise lock tools")? {
            if tool.is_empty() || tool.starts_with('-') || tool.chars().any(char::is_control) {
                return Err("Invalid tool name in mise lock".into());
            }
            let versions: Vec<_> = match versions {
                toml::Value::Array(versions) => versions.iter().collect(),
                toml::Value::Table(_) => vec![versions],
                _ => return Err("Invalid mise lock tool versions".into()),
            };
            for version in versions {
                let mut resolution = version.as_table().ok_or("Invalid mise lock entry")?.clone();
                if !resolution
                    .get("version")
                    .is_some_and(|value| value.as_str().is_some_and(|s| !s.is_empty()))
                {
                    return Err("Mise lock entry has no resolved version".into());
                }
                let mut platforms = BTreeMap::new();
                let mut metadata_values = BTreeMap::new();
                let keys: Vec<_> = resolution
                    .keys()
                    .filter(|key| key.starts_with("platforms.") || *key == "platforms")
                    .cloned()
                    .collect();
                for key in keys {
                    let value = resolution.remove(&key).unwrap();
                    if key == "platforms" {
                        for (platform, metadata) in
                            value.as_table().ok_or("Invalid mise lock platforms")?
                        {
                            validate_platform(platform)?;
                            platforms.insert(platform.clone(), metadata_requirements(metadata)?);
                            metadata_values.insert(platform.clone(), metadata.clone());
                        }
                    } else {
                        let platform = key.strip_prefix("platforms.").unwrap();
                        validate_platform(platform)?;
                        platforms.insert(platform.into(), metadata_requirements(&value)?);
                        metadata_values.insert(platform.into(), value);
                    }
                }
                entries.push(Entry {
                    tool: tool.clone(),
                    resolution: toml::Value::Table(resolution),
                    platforms,
                    metadata: metadata_values,
                });
            }
        }
    }
    let resolutions = entries
        .iter()
        .map(|entry| (entry.tool.clone(), entry.resolution.clone()))
        .collect();
    Ok((
        Lock {
            format: value.get("lockfile_version").cloned(),
            resolutions,
            metadata: entries.iter().map(|entry| entry.metadata.clone()).collect(),
        },
        entries,
    ))
}

fn validate_platform(platform: &str) -> Result<(), String> {
    if platform.is_empty()
        || platform
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        return Err("Invalid mise lock platform name".into());
    }
    Ok(())
}

fn metadata_requirements(value: &toml::Value) -> Result<(bool, bool), String> {
    let table = value
        .as_table()
        .ok_or("Invalid mise lock platform metadata")?;
    let present = |key| {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    };
    Ok((present("url"), present("checksum")))
}

fn snapshot(
    files: &BTreeMap<PathBuf, FileRecord>,
    path: &Path,
) -> Result<(Lock, Vec<Entry>), String> {
    let record = files.get(path).ok_or("Mise lock has no guarded snapshot")?;
    parse(record.expected.bytes.as_deref()).map_err(|error| format!("{}: {error}", path.display()))
}

fn identity_fields(entry: &Entry) -> toml::Value {
    let mut value = entry.resolution.as_table().unwrap().clone();
    value.remove("version");
    value.remove("specifiers");
    if value
        .get("options")
        .and_then(toml::Value::as_table)
        .is_some_and(|options| options.is_empty())
    {
        value.remove("options");
    }
    toml::Value::Table(value)
}

fn matching<'a>(old: &Entry, entries: &'a [Entry]) -> Result<&'a Entry, String> {
    let candidates: Vec<_> = entries
        .iter()
        .filter(|entry| entry.tool == old.tool && identity_fields(entry) == identity_fields(old))
        .collect();
    let exact: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|entry| entry.resolution == old.resolution)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    let specifiers: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|entry| entry.resolution.get("specifiers") == old.resolution.get("specifiers"))
        .collect();
    if specifiers.len() == 1 {
        return Ok(specifiers[0]);
    }
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    Err(format!(
        "Cannot unambiguously preserve mise lock platforms for {}",
        old.tool
    ))
}

fn covers(actual: &Entry, required: &Entry) -> bool {
    required
        .platforms
        .iter()
        .all(|(platform, &(url, checksum))| {
            actual
                .platforms
                .get(platform)
                .is_some_and(|&(actual_url, actual_checksum)| {
                    (!url || actual_url) && (!checksum || actual_checksum)
                })
        })
}

impl Preservation {
    pub fn capture(
        contexts: Vec<LockContext>,
        files: &BTreeMap<PathBuf, FileRecord>,
    ) -> Result<Self, String> {
        let paths: BTreeSet<_> = contexts
            .iter()
            .flat_map(|context| [context.source.clone(), context.destination.clone()])
            .collect();
        let mut formats = BTreeMap::new();
        let mut required = Vec::new();
        let mut migrations = BTreeMap::new();
        for context in &contexts {
            if let Some(previous) =
                migrations.insert(context.source.clone(), context.destination.clone())
                && previous != context.destination
            {
                return Err(
                    "Cannot associate mise lock with a single migration destination".into(),
                );
            }
        }
        for source in &paths {
            let (lock, entries) = snapshot(files, source)?;
            let destination = migrations.get(source).unwrap_or(source).clone();
            if files[source]
                .expected
                .bytes
                .as_ref()
                .is_some_and(|bytes| !bytes.is_empty())
            {
                for path in [source, &destination] {
                    if formats
                        .get(path)
                        .is_some_and(|format| format != &lock.format)
                    {
                        return Err(
                            "Cannot migrate mise locks with different lockfile versions".into()
                        );
                    }
                    formats.insert(path.clone(), lock.format.clone());
                }
            }
            for entry in entries
                .into_iter()
                .filter(|entry| !entry.platforms.is_empty())
            {
                let related: Vec<_> = contexts
                    .iter()
                    .filter(|context| &context.source == source || &context.destination == source)
                    .collect();
                let destinations: BTreeSet<_> = related
                    .iter()
                    .map(|context| context.destination.clone())
                    .collect();
                if destinations.len() != 1 {
                    return Err(
                        "Cannot associate mise lock with a single migration destination".into(),
                    );
                }
                let destination = destinations.into_iter().next().unwrap();
                let mut execution_contexts = BTreeSet::new();
                for context in related {
                    let config = files
                        .get(&context.config)
                        .ok_or("Mise config has no guarded snapshot")?;
                    let config: toml::Value = toml::from_str(
                        std::str::from_utf8(config.expected.bytes.as_deref().unwrap_or_default())
                            .map_err(|_| "Mise config is not UTF-8")?,
                    )
                    .map_err(|_| "Cannot parse mise config for lock context")?;
                    if config
                        .get("tools")
                        .and_then(|tools| tools.get(&entry.tool))
                        .is_some()
                    {
                        execution_contexts.insert(ExecutionContext {
                            directory: context.directory.clone(),
                            environment: context.environment.clone(),
                            local: context.local,
                        });
                    }
                }
                if execution_contexts.is_empty() {
                    return Err(format!(
                        "Cannot confirm the execution context for mise tool {}",
                        entry.tool
                    ));
                }
                required.push(Required {
                    source: source.clone(),
                    destination,
                    contexts: execution_contexts,
                    entry,
                });
            }
        }
        Ok(Self {
            required,
            formats,
            paths,
            migrations,
        })
    }

    pub fn prepare(
        &self,
        files: &BTreeMap<PathBuf, FileRecord>,
        prepared: &PreparedInstall,
    ) -> Result<Repair, String> {
        let mut baseline = BTreeMap::new();
        let mut entries = BTreeMap::new();
        for path in &self.paths {
            let (lock, versions) = snapshot(files, path)?;
            // 同じ場所の lock を削除してよいのは、移動先が確定したときだけ。
            let migrated = self
                .migrations
                .get(path)
                .is_some_and(|destination| destination != path)
                && files[path].expected.bytes.is_none();
            if let Some(format) = self.formats.get(path)
                && !migrated
                && &lock.format != format
            {
                return Err("Mise lockfile_version changed during install".into());
            }
            baseline.insert(path.clone(), lock);
            entries.insert(path.clone(), versions);
        }
        let mut groups: BTreeMap<(ExecutionContext, Vec<String>), BTreeSet<String>> =
            BTreeMap::new();
        let mut requests: BTreeMap<(PathBuf, usize, ExecutionContext), PlatformRequest> =
            BTreeMap::new();
        let mut required_entries = Vec::new();
        let mut matched = BTreeSet::new();
        let mut assignments = BTreeMap::new();
        // 変更した要求を照合する前に、変更していない記録の対応先を予約する。
        // これにより、同じバックエンドの複数版が互いの対応先を奪わないようにする。
        for phase in 0..3 {
            for (index, required) in self.required.iter().enumerate() {
                if assignments.contains_key(&index) {
                    continue;
                }
                let candidates: Vec<_> = entries[&required.destination]
                    .iter()
                    .enumerate()
                    .filter(|(entry_index, actual)| {
                        !matched.contains(&(
                            required.source.clone(),
                            required.destination.clone(),
                            *entry_index,
                        )) && actual.tool == required.entry.tool
                            && identity_fields(actual) == identity_fields(&required.entry)
                            && match phase {
                                0 => actual.resolution == required.entry.resolution,
                                1 => {
                                    actual.resolution.get("specifiers")
                                        == required.entry.resolution.get("specifiers")
                                }
                                _ => true,
                            }
                    })
                    .map(|(entry_index, _)| entry_index)
                    .collect();
                if candidates.len() == 1 {
                    matched.insert((
                        required.source.clone(),
                        required.destination.clone(),
                        candidates[0],
                    ));
                    assignments.insert(index, candidates[0]);
                }
            }
        }
        for (index, required) in self.required.iter().enumerate() {
            let entry_index = if let Some(&entry_index) = assignments.get(&index) {
                entry_index
            } else {
                // 複数の古い要求が同じ版へまとまること自体は正当。
                // 一致する識別情報の全体で候補が1つだけの場合に限り、
                // 別の要求が予約した対応先を共有できる。
                let candidates: Vec<_> = entries[&required.destination]
                    .iter()
                    .enumerate()
                    .filter(|(_, actual)| {
                        actual.tool == required.entry.tool
                            && identity_fields(actual) == identity_fields(&required.entry)
                    })
                    .map(|(entry_index, _)| entry_index)
                    .collect();
                if candidates.len() != 1 {
                    return Err(format!(
                        "Cannot unambiguously preserve mise lock platforms for {}",
                        required.entry.tool
                    ));
                }
                candidates[0]
            };
            let actual = &entries[&required.destination][entry_index];
            let mut expected = actual.clone();
            expected.platforms = required.entry.platforms.clone();
            let missing = !covers(actual, &expected);
            if missing && required.contexts.len() != 1 {
                return Err(format!(
                    "Cannot unambiguously regenerate mise tool {} across multiple config contexts",
                    actual.tool
                ));
            }
            if let Some(context) = required
                .contexts
                .iter()
                .next()
                .filter(|_| required.contexts.len() == 1)
            {
                let request = requests
                    .entry((required.destination.clone(), entry_index, context.clone()))
                    .or_default();
                request.platforms.extend(expected.platforms.keys().cloned());
                request.missing |= missing;
                request.tool = actual.tool.clone();
            }
            required_entries.push((required.destination.clone(), expected));
        }
        let mut targets = BTreeSet::new();
        for ((path, _, context), request) in requests {
            if !request.missing {
                continue;
            }
            targets.insert((path, request.tool.clone()));
            groups
                .entry((context, request.platforms.into_iter().collect()))
                .or_default()
                .insert(request.tool);
        }
        let commands = groups
            .into_iter()
            .map(|((context, platforms), tools)| {
                let mut command = prepared.clone();
                command.directory = context.directory;
                command.argv = vec![prepared.argv[0].clone(), "lock".into()];
                if let Some(environment) = context.environment {
                    command.argv.extend(["--env".into(), environment]);
                }
                if context.local {
                    command.argv.push("--local".into());
                }
                command
                    .argv
                    .extend(["--platform".into(), platforms.join(","), "--".into()]);
                command.argv.extend(tools);
                command
            })
            .collect();
        Ok(Repair {
            commands,
            baseline,
            required: required_entries,
            targets,
        })
    }
}

impl Repair {
    pub fn checkpoint(&mut self, files: &BTreeMap<PathBuf, FileRecord>) -> Result<(), String> {
        self.validate_resolutions(files)?;
        for (path, baseline) in &mut self.baseline {
            // install の解決結果を固定したまま、修復に成功するたびに復元したメタデータを、
            // 後続のコマンドから保護する。
            baseline.metadata = snapshot(files, path)?.0.metadata;
        }
        Ok(())
    }

    pub fn validate_resolutions(
        &self,
        files: &BTreeMap<PathBuf, FileRecord>,
    ) -> Result<(), String> {
        for (path, expected) in &self.baseline {
            let (actual, _) = snapshot(files, path)?;
            if actual.format != expected.format || actual.resolutions != expected.resolutions {
                return Err("Mise lock regeneration changed resolved versions, options, or lockfile_version".into());
            }
            for (index, (tool, _)) in expected.resolutions.iter().enumerate() {
                let preserved = if self.targets.contains(&(path.clone(), tool.clone())) {
                    expected.metadata[index].iter().all(|(platform, original)| {
                        actual.metadata[index].get(platform).is_some_and(|current| {
                            original.as_table().unwrap().iter().all(|(key, value)| {
                                ((key == "url" || key == "checksum")
                                    && value.as_str().is_some_and(|value| value.trim().is_empty()))
                                    || current.get(key) == Some(value)
                            })
                        })
                    })
                } else {
                    actual.metadata[index] == expected.metadata[index]
                };
                if !preserved {
                    return Err(format!(
                        "Mise lock regeneration changed existing platform metadata for {tool}"
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self, files: &BTreeMap<PathBuf, FileRecord>) -> Result<(), String> {
        self.validate_resolutions(files)?;
        for (path, expected) in &self.required {
            let (_, entries) = snapshot(files, path)?;
            let actual = matching(expected, &entries)?;
            if !covers(actual, expected) {
                return Err(format!(
                    "Mise lock regeneration left missing platform URL/checksum metadata for {}",
                    expected.tool
                ));
            }
        }
        Ok(())
    }
}
