//! Node の install と lock 監査が共有する解決単位と age ポリシー。

use crate::domain::{Language, ManifestUpdateResult};
use crate::manifest::detect_manifests;
use crate::update::AgePolicy;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) struct NodeScope {
    pub directory: PathBuf,
    pub policy: Result<AgePolicy, String>,
    pub updated: Vec<PathBuf>,
}

/// pnpm の共有 lock は、実際に検出される workspace member だけを結び付ける。
pub(crate) fn node_resolution_directory(manifest: &Path) -> PathBuf {
    let parent = manifest.parent().unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    let manifest = parent.join("package.json");
    for directory in parent.ancestors() {
        if directory == parent {
            if [
                "pnpm-lock.yaml",
                "package-lock.json",
                "npm-shrinkwrap.json",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            ]
            .iter()
            .any(|filename| !matches!(std::fs::symlink_metadata(directory.join(filename)), Err(error) if error.kind() == std::io::ErrorKind::NotFound))
            {
                return directory.to_path_buf();
            }
        } else if directory.join("pnpm-workspace.yaml").is_file()
            && detect_manifests(directory).iter().any(|candidate| {
                candidate.language == Language::Node
                    && std::fs::canonicalize(&candidate.path).is_ok_and(|path| path == manifest)
            })
        {
            return directory.to_path_buf();
        }
    }
    parent
}

/// 更新のない member も含め、共有 lock の最も厳しい policy を採る。
pub(crate) fn node_scopes(
    manifests: &[ManifestUpdateResult],
    policy_for: impl Fn(&Path) -> Result<AgePolicy, String>,
) -> Vec<NodeScope> {
    let mut groups: BTreeMap<PathBuf, (BTreeSet<PathBuf>, Vec<PathBuf>)> = BTreeMap::new();
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.language == Language::Node)
    {
        let (members, updated) = groups
            .entry(node_resolution_directory(&manifest.path))
            .or_default();
        members.insert(manifest.path.clone());
        if manifest.has_updates() {
            updated.push(manifest.path.clone());
        }
    }
    groups
        .into_iter()
        .map(|(directory, (mut members, updated))| {
            members.extend(
                detect_manifests(&directory)
                    .into_iter()
                    .filter(|manifest| manifest.language == Language::Node)
                    .map(|manifest| manifest.path),
            );
            let policy = (|| {
                let mut policy = policy_for(&directory)?;
                for member in members {
                    if let Some(parent) = member.parent() {
                        policy.merge(&policy_for(parent)?);
                    }
                }
                Ok(policy)
            })();
            NodeScope {
                directory,
                policy,
                updated,
            }
        })
        .collect()
}
