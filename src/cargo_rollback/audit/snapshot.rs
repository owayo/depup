//! Last-resort recovery when selective age restoration cannot finish safely.
use super::*;
use crate::cargo_rollback::scratch::workspace_manifest_paths;
use crate::file_snapshot::{FileGuard, FileState, save_recovery_copy, write_checked};
use std::path::PathBuf;

pub(super) struct AuditSnapshot {
    lock: FileGuard,
    start: FileState,
    manifests: Vec<(FileGuard, FileState)>,
    inventory: Vec<PathBuf>,
}

// Inventory every Cargo.toml below each glob's fixed prefix. This conservative
// superset also covers nested/repeated globs without depending on the detector's
// limited pattern expansion or running Cargo again during emergency recovery.
fn manifest_inventory(root: &Path) -> Result<Vec<PathBuf>, String> {
    use std::fs;
    let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let content = match fs::read_to_string(root.join("Cargo.toml")) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let manifest: toml::Value = toml::from_str(&content).map_err(|error| error.to_string())?;
    let mut pending = Vec::new();
    for pattern in manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let pattern = pattern.as_str().ok_or("invalid workspace member pattern")?;
        let mut prefix = root.clone();
        for component in Path::new(pattern).components() {
            match component {
                std::path::Component::Normal(part) => {
                    if part
                        .to_string_lossy()
                        .contains(['*', '?', '[', ']', '{', '}'])
                    {
                        pending.push(prefix);
                        break;
                    }
                    prefix.push(part);
                }
                std::path::Component::CurDir => {}
                _ => return Err("workspace inventory would leave the project root".into()),
            }
        }
    }
    let mut visited = HashSet::new();
    let mut paths = Vec::new();
    let mut examined = 0;
    while let Some(directory) = pending.pop() {
        let directory = match fs::canonicalize(&directory) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !directory.starts_with(&root) {
            return Err("workspace inventory follows a directory outside the project root".into());
        }
        if !visited.insert(directory.clone()) {
            continue;
        }
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            examined += 1;
            if examined > 100_000 {
                return Err("workspace inventory exceeded 100000 entries".into());
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
                pending.push(path);
            } else if entry.file_name() == "Cargo.toml" {
                let path = fs::canonicalize(path).map_err(|error| error.to_string())?;
                if !path.starts_with(&root) {
                    return Err(
                        "workspace inventory follows a manifest outside the project root".into(),
                    );
                }
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

impl AuditSnapshot {
    pub(super) async fn capture(input: &LockAgeAudit<'_>, content: &str) -> Result<Self, String> {
        let lock = FileGuard::capture(&input.project_dir.join("Cargo.lock"))
            .map_err(|error| error.to_string())?;
        let inventory = manifest_inventory(lock.alias.parent().expect("audit lock has a parent"))?;
        let start = lock.read().map_err(|error| error.to_string())?;
        if start.bytes.as_deref() != Some(content.as_bytes()) {
            return Err("Cargo.lock changed while taking the age audit snapshot".into());
        }
        let manifests = async {
            let root = FileGuard::capture(&input.project_dir.join("Cargo.toml"))
                .map_err(|error| error.to_string())?;
            let before = root.read().map_err(|error| error.to_string())?;
            // Read-only audits can also inspect a lock without a manifest.
            let paths = if before.bytes.is_some() {
                workspace_manifest_paths(
                    input.project_dir,
                    input.cargo,
                    input.budget.min(Duration::from_secs(10)),
                )
                .await?
            } else {
                Vec::new()
            };
            let mut files = vec![(root, before)];
            for path in paths {
                let guard = FileGuard::capture(&path).map_err(|error| error.to_string())?;
                let state = guard.read().map_err(|error| error.to_string())?;
                files.push((guard, state));
            }
            Ok::<_, String>(files)
        }
        .await?;
        Ok(Self {
            lock,
            start,
            manifests,
            inventory,
        })
    }

    pub(super) fn restore(&self, state: &mut AuditState) {
        let attempt = (|| -> Result<bool, String> {
            let current = self.lock.read().map_err(|error| error.to_string())?;
            if self.start.matches(&current) {
                return Ok(false);
            }
            if !state
                .changes
                .allows_snapshot_restore(current.bytes.as_deref())
            {
                return Err("Cargo.lock changed outside an audited Cargo operation".into());
            }
            let expected = FileState {
                bytes: current.bytes.clone(),
                permissions: self.start.permissions.clone(),
            };
            if !expected.matches(&current) {
                return Err("Cargo.lock permissions changed during the age audit".into());
            }
            for (guard, before) in &self.manifests {
                if !before.matches(&guard.read().map_err(|error| error.to_string())?) {
                    return Err(format!(
                        "{} changed during the age audit",
                        guard.alias.display()
                    ));
                }
            }
            let root = self.lock.alias.parent().expect("audit lock has a parent");
            if manifest_inventory(root)? != self.inventory {
                return Err("workspace manifest inventory changed during the age audit".into());
            }
            write_checked(
                &self.lock,
                &current,
                self.start.bytes.as_deref().expect("existing audit lock"),
                self.start.permissions.as_ref(),
            )
            .map_err(|error| error.to_string())?;
            if !self
                .start
                .matches(&self.lock.read().map_err(|error| error.to_string())?)
            {
                return Err("Cargo.lock changed after restoring the age audit snapshot".into());
            }
            Ok(true)
        })();
        match attempt {
            Ok(restored) => {
                let reason = "the age audit failed; restored the install-time Cargo.lock snapshot";
                let installed = parse_registry_entries(&state.installed_lock);
                // Entries introduced only by the failed audit no longer describe this lock.
                state.log.entries.retain(|(name, from), _| {
                    installed
                        .get(name)
                        .is_some_and(|versions| versions.contains(from))
                });
                for entry in state.log.entries.values_mut() {
                    if entry.status.is_resolved() || entry.status.is_restored() {
                        entry.to = None;
                        entry.status = LockAgeStatus::Reverted(reason.into());
                    }
                }
                if restored {
                    state.problems.push(reason.into());
                }
            }
            Err(error) => {
                let backup = match save_recovery_copy(&self.start) {
                    Ok(path) => format!(
                        "the install-time Cargo.lock was saved to {} — copy it back to restore",
                        path.display()
                    ),
                    Err(reason) => {
                        format!("saving the install-time Cargo.lock also failed: {reason}")
                    }
                };
                state.problems.push(format!(
                    "could not restore the age audit snapshot ({error}); {backup}"
                ));
            }
        }
    }
}
