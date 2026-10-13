//! Static membership for managers whose command resolves a shared project lock.
use super::*;
use std::path::Component;

pub(super) fn expand(root: &Path, pattern: &str) -> Result<Vec<PathBuf>, String> {
    let root = identity(root);
    let mut paths = vec![root.clone()];
    for part in Path::new(pattern).components() {
        let Component::Normal(part) = part else {
            return Err("Unsafe workspace path in install recovery scope".into());
        };
        let part = part.to_str().ok_or("Workspace path is not UTF-8")?;
        if part.contains(['?', '[', ']', '{', '}'])
            || part.contains("**")
            || part.matches('*').count() > 1
        {
            return Err("Cannot safely resolve this workspace pattern for install recovery".into());
        }
        let mut next = Vec::new();
        for path in paths {
            if let Some((prefix, suffix)) = part.split_once('*') {
                let entries = match fs::read_dir(&path) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(format!("Cannot enumerate workspace scope: {error}")),
                };
                for entry in entries {
                    let entry =
                        entry.map_err(|error| format!("Cannot read workspace scope: {error}"))?;
                    let name = entry.file_name();
                    let name = name
                        .to_str()
                        .ok_or("Workspace directory name is not UTF-8")?;
                    if name.len() >= prefix.len() + suffix.len()
                        && name.starts_with(prefix)
                        && name.ends_with(suffix)
                    {
                        next.push(entry.path());
                    }
                }
            } else {
                next.push(path.join(part));
            }
        }
        paths = Vec::new();
        for path in next {
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {
                    if !resolved_path(&path)
                        .map_err(|error| format!("Cannot resolve workspace scope: {error}"))?
                        .starts_with(&root)
                    {
                        return Err("Workspace scope resolves outside its root".into());
                    }
                    paths.push(path);
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Cannot inspect workspace scope: {error}")),
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
fn patterns(value: Option<&toml::Value>) -> Result<Vec<String>, String> {
    match value {
        None => Ok(vec![]),
        Some(value) => value
            .as_array()
            .ok_or("Invalid workspace patterns")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or("Invalid workspace pattern".into())
            })
            .collect(),
    }
}
#[derive(Clone)]
pub(super) struct UvScope {
    pub root: PathBuf,
    pub members: BTreeSet<PathBuf>,
}
impl UvScope {
    fn standalone(directory: PathBuf) -> Self {
        Self {
            members: BTreeSet::from([directory.clone()]),
            root: directory,
        }
    }
}
pub(super) fn uv_root(directory: &Path, boundary: &Path) -> Result<UvScope, String> {
    let directory = identity(directory);
    let boundary = identity(boundary);
    // This permission boundary is not evidence that uv stops searching above it.
    for ancestor in directory.ancestors() {
        let content = match fs::read_to_string(ancestor.join("pyproject.toml")) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Cannot read uv workspace scope: {error}")),
        };
        let config: toml::Value =
            toml::from_str(&content).map_err(|_| "Cannot parse uv workspace scope")?;
        let Some(workspace) = config
            .get("tool")
            .and_then(|tool| tool.get("uv"))
            .and_then(|uv| uv.get("workspace"))
        else {
            continue;
        };
        let mut excluded = BTreeSet::new();
        for pattern in patterns(workspace.get("exclude"))? {
            excluded.extend(
                expand(ancestor, &pattern)?
                    .into_iter()
                    .map(|path| identity(&path)),
            );
        }
        if ancestor != directory && excluded.contains(&directory) {
            return Ok(UvScope::standalone(directory));
        }
        let mut members = BTreeSet::new();
        for pattern in patterns(workspace.get("members"))? {
            members.extend(
                expand(ancestor, &pattern)?
                    .into_iter()
                    .map(|path| identity(&path)),
            );
        }
        members.retain(|member| !excluded.contains(member));
        members.insert(ancestor.to_path_buf());
        return if members.contains(&directory) {
            if ancestor.starts_with(&boundary) {
                Ok(UvScope {
                    root: ancestor.into(),
                    members,
                })
            } else {
                Err("Confirmed uv workspace root is outside the recovery boundary; run from the workspace root".into())
            }
        } else {
            Err("Cannot confirm the selected uv project's workspace membership".into())
        };
    }
    Ok(UvScope::standalone(directory))
}
