use super::ManifestJournal;
use crate::domain::{ChangeLevel, Dependency, ManifestUpdateResult, UpdateResult, VersionSpecKind};
use crate::file_snapshot::{FileGuard, FileState, resolved_path, write_checked};
use crate::manifest::json_sections::unique_top_level_string_property;
use crate::manifest::mise::count_mise_tool_declarations;
use crate::manifest::{
    ManifestInfo, TOOL_VERSIONS_FILENAME, native_age::mise_config_layers, split_utf8_bom,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct PackageManagerSync {
    pub path: PathBuf,
    pub from: String,
    pub to: String,
    pub source_index: usize,
    pub journal: Option<ManifestJournal>,
}

pub(super) struct SyncContext<'a> {
    pub blocked: &'a BTreeSet<PathBuf>,
    pub detected: &'a [ManifestInfo],
    pub initial_pins: &'a BTreeMap<PathBuf, String>,
    pub max_change: Option<ChangeLevel>,
}

pub(super) struct PendingSync {
    pub report: PackageManagerSync,
    guard: FileGuard,
    before: FileState,
    written: Vec<u8>,
}

fn manager_name(tool: &str) -> Option<&'static str> {
    match tool.split_once('[').map_or(tool, |(name, _)| name) {
        "npm" | "aqua:npm/cli" | "npm:npm" => Some("npm"),
        "pnpm" | "aqua:pnpm/pnpm" | "npm:pnpm" => Some("pnpm"),
        "yarn" | "aqua:yarnpkg/berry" | "npm:@yarnpkg/cli-dist" | "npm:yarn" => Some("yarn"),
        _ => None,
    }
}

pub(crate) fn is_mise_package_manager(tool: &str) -> bool {
    manager_name(tool).is_some()
}

pub(crate) fn capture_package_manager_pins<'a>(
    projects: impl Iterator<Item = &'a Path>,
) -> BTreeMap<PathBuf, String> {
    projects
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|project| {
            let path = project.join("package.json");
            let before = read_package_manifest(project, &path).ok()?.1;
            let content = std::str::from_utf8(before.bytes.as_deref()?).ok()?;
            let (_, body) = split_utf8_bom(content);
            serde_json::from_str::<serde_json::Value>(body).ok()?;
            let (_, value) = unique_top_level_string_property(body, "packageManager").ok()??;
            Some((project.to_path_buf(), value.into_owned()))
        })
        .collect()
}

fn read_package_manifest(project: &Path, path: &Path) -> Result<(FileGuard, FileState), String> {
    let guard = FileGuard::capture(path).map_err(|error| error.to_string())?;
    let real_project = resolved_path(project).map_err(|error| error.to_string())?;
    if guard.target != real_project.join("package.json") {
        return Err("package.json resolves to a different file".into());
    }
    let state = guard.read().map_err(|error| error.to_string())?;
    Ok((guard, state))
}

fn competing_declarations(project: &Path, manager: &str) -> Result<bool, String> {
    let environments = std::env::var("MISE_ENV").unwrap_or_default();
    let mut paths: BTreeSet<_> = mise_config_layers(project, &environments)
        .into_iter()
        .collect();
    paths.insert(project.join(TOOL_VERSIONS_FILENAME));
    let mut count = 0;
    for path in paths {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read {}: {error}",
                    crate::user_path::display(&path)
                ));
            }
        };
        count += count_mise_tool_declarations(split_utf8_bom(&content).1, |tool| {
            manager_name(tool) == Some(manager)
        })
        .map_err(|error| format!("cannot parse {}: {error}", crate::user_path::display(&path)))?;
        if count > 1 {
            return Ok(true);
        }
    }
    Ok(false)
}

impl SyncContext<'_> {
    pub fn project_for(&self, path: &Path) -> Option<&Path> {
        self.detected
            .iter()
            .find(|info| info.path == path)?
            .mise_project
            .as_deref()
    }

    pub fn prepare(
        &self,
        project: &Path,
        manifest: &ManifestUpdateResult,
        applied: &[usize],
        parsed: &[Dependency],
    ) -> Result<Option<PendingSync>, String> {
        let candidates = applied.iter().filter_map(|&index| {
            let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = &manifest.results[index]
            else {
                return None;
            };
            Some((
                index,
                manager_name(&dependency.name)?,
                dependency,
                new_version,
            ))
        });
        if candidates.clone().next().is_none() {
            return Ok(None);
        }

        let path = project.join("package.json");
        let (guard, before) = read_package_manifest(project, &path)?;
        let Some(bytes) = &before.bytes else {
            return Ok(None);
        };
        let content = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let (bom, body) = split_utf8_bom(content);
        serde_json::from_str::<serde_json::Value>(body).map_err(|error| error.to_string())?;
        let Some((range, value)) = unique_top_level_string_property(body, "packageManager")
            .map_err(|()| "multiple top-level packageManager fields")?
        else {
            return Ok(None);
        };
        let Some((manager, old_version)) = value.split_once('@') else {
            return Ok(None);
        };
        let Some((index, _, dependency, new_version)) = candidates
            .into_iter()
            .find(|(_, name, ..)| *name == manager)
        else {
            return Ok(None);
        };
        // 候補を選んでから設定が編集された場合にも、書き込み時に読んだ指定の形を使う。
        let Some(target) = parsed
            .iter()
            .find(|current| current.manifest_name() == dependency.manifest_name())
            .filter(|current| current.version_spec.kind == VersionSpecKind::Exact)
            .and_then(|current| current.version_spec.try_format_updated(new_version))
        else {
            return Err("mise does not specify an exact version for this package manager".into());
        };
        let target = target.strip_prefix('v').unwrap_or(&target);
        let parse =
            |version: &str| semver::Version::parse(version.strip_prefix('v').unwrap_or(version));
        let Ok(new) = parse(target) else {
            return Err("the selected mise version cannot be represented in packageManager".into());
        };
        let Ok(old) = parse(old_version) else {
            return Err("packageManager does not specify an exact semantic version".into());
        };
        let precedence = old.cmp_precedence(&new);
        if precedence.is_eq() && (old.build == new.build || new.build.is_empty()) {
            return Ok(None);
        }
        if !old.build.is_empty() {
            return Err(
                "packageManager has a checksum or build metadata tied to its current version"
                    .into(),
            );
        }
        if precedence.is_gt() {
            return Err("packageManager already specifies a newer version".into());
        }
        if self.initial_pins.get(project).map(String::as_str) != Some(value.as_ref()) {
            return Err("packageManager changed while checking updates".into());
        }
        if let Some(max) = self.max_change
            && ChangeLevel::from_versions(old_version, target).is_some_and(|change| change > max)
        {
            return Err(format!(
                "updating packageManager would exceed --max-change={max}"
            ));
        }
        if competing_declarations(project, manager)? {
            return Err(format!("multiple mise declarations for {manager}"));
        }

        let to = format!("{manager}@{target}");
        let literal = serde_json::to_string(&to).map_err(|error| error.to_string())?;
        let written = format!(
            "{bom}{}{literal}{}",
            &body[..range.start],
            &body[range.end..]
        );
        let report = PackageManagerSync {
            path,
            from: value.into_owned(),
            to,
            source_index: index,
            journal: None,
        };
        Ok(Some(PendingSync {
            report,
            guard,
            before,
            written: written.into_bytes(),
        }))
    }
}

impl PendingSync {
    pub fn commit(&mut self) -> std::io::Result<()> {
        let permissions = self
            .before
            .permissions
            .as_ref()
            .expect("existing package.json");
        write_checked(&self.guard, &self.before, &self.written, Some(permissions))?;
        self.report.journal = Some(ManifestJournal {
            target: self.guard.target.clone(),
            before: self.before.bytes.clone().expect("existing package.json"),
            written: self.written.clone(),
            permissions: permissions.clone(),
            guard: self.guard.clone(),
        });
        Ok(())
    }
}
