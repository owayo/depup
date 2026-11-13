//! age の選択的復元を安全に完了できない場合の最終的な復元手段。
use super::*;
use crate::cargo_rollback::scratch::workspace_manifest_paths;
use crate::file_snapshot::{FileGuard, FileState, save_recovery_copy, write_checked};
use inventory::{Inventory, manifest_inventory};

mod inventory;

pub(super) struct AuditSnapshot {
    lock: FileGuard,
    start: FileState,
    manifests: Vec<(FileGuard, FileState)>,
    /// workspace のメンバーの集合。戻す直前に取り直し、監査の間に変わっていれば戻さない。
    inventory: Inventory,
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
            // 読み取り専用の監査では、マニフェストのない lock も確認できる。
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
                let external = state.changes.external_packages();
                return Err(if external.is_empty() {
                    "Cargo.lock changed outside an audited Cargo operation".into()
                } else {
                    format!(
                        "Cargo.lock changed outside an audited Cargo operation: {}",
                        external.join(", ")
                    )
                });
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
                        crate::user_path::display(&guard.alias)
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
                // 失敗した監査だけが追加した記録は、復元後の lock を表さない。
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
                        crate::user_path::display(&path)
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
