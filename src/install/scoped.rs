//! 適用した依存更新と変更前の lockfile から、選択的な更新を計画する。

use super::*;
use crate::cargo_scope::rust_resolution_directory;
use crate::domain::{Dependency, GitReference, ManifestUpdateResult};
use crate::manifest::{LockPackage, is_crates_io_source, parse_lock_packages};

/// 全体を解き直す意味を保ちつつ、検出によって停止済みのルートを復活させない。
pub(super) fn replace_full_rust_jobs(
    jobs: &mut Vec<InstallJob>,
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
) {
    jobs.retain(|job| job.language != Language::Rust);
    let boundary = orchestrator.rust_lock_boundary();
    let stopped_paths = stopped_cargo_paths(result);
    let stopped_roots = stopped_cargo_roots(result);
    let mut locations = BTreeMap::new();
    let mut roots = BTreeSet::new();
    for manifest in result
        .summary
        .manifests
        .iter()
        .filter(|manifest| manifest.language == Language::Rust && manifest.has_updates())
    {
        let key = std::fs::canonicalize(&manifest.path).unwrap_or_else(|_| manifest.path.clone());
        let location = locations
            .entry(key.clone())
            .or_insert_with(|| rust_resolution_directory(&manifest.path, &boundary));
        let (directory, scope) = match location {
            Ok(root) => (
                root.clone(),
                if stopped_roots.contains(root) || stopped_paths.contains(&key) {
                    Err("Cargo root was stopped by the pnpm preflight".into())
                } else {
                    Ok(InstallScope::CargoFull)
                },
            ),
            Err(error) => (
                manifest.path.parent().unwrap_or(Path::new(".")).into(),
                Err(error.clone()),
            ),
        };
        if !roots.insert(directory.clone()) {
            continue;
        }
        jobs.push(InstallJob {
            manifests: result
                .summary
                .manifests
                .iter()
                .filter(|manifest| {
                    manifest.language == Language::Rust
                        && locations
                            .entry(
                                std::fs::canonicalize(&manifest.path)
                                    .unwrap_or_else(|_| manifest.path.clone()),
                            )
                            .or_insert_with(|| rust_resolution_directory(&manifest.path, &boundary))
                            .as_ref()
                            .is_ok_and(|root| root == &directory)
                })
                .map(|manifest| manifest.path.clone())
                .collect(),
            policy: install_age_policy(orchestrator, result, &directory, Language::Rust),
            directory,
            language: Language::Rust,
            scope,
        });
    }
    jobs.sort_by(|left, right| {
        left.directory
            .cmp(&right.directory)
            .then_with(|| left.language.to_string().cmp(&right.language.to_string()))
    });
}

pub(super) fn replace_scoped_jobs(
    jobs: &mut Vec<InstallJob>,
    orchestrator: &Orchestrator,
    result: &OrchestratorResult,
) {
    jobs.retain(|job| !matches!(job.language, Language::Rust | Language::Php));
    let boundary = orchestrator.rust_lock_boundary();
    let mut groups: BTreeMap<(PathBuf, String), Vec<&ManifestUpdateResult>> = BTreeMap::new();
    let mut rust_roots: BTreeMap<PathBuf, Result<PathBuf, String>> = BTreeMap::new();
    for manifest in &result.summary.manifests {
        if !manifest.has_updates() || !matches!(manifest.language, Language::Rust | Language::Php) {
            continue;
        }
        let parent = manifest.path.parent().unwrap_or(Path::new("."));
        let directory = if manifest.language == Language::Rust {
            let key =
                std::fs::canonicalize(&manifest.path).unwrap_or_else(|_| manifest.path.clone());
            match rust_roots
                .entry(key)
                .or_insert_with(|| rust_resolution_directory(&manifest.path, &boundary))
            {
                Ok(directory) => directory.clone(),
                Err(error) => {
                    jobs.push(InstallJob {
                        manifests: vec![manifest.path.clone()],
                        directory: parent.to_path_buf(),
                        language: Language::Rust,
                        policy: install_age_policy(orchestrator, result, parent, Language::Rust),
                        scope: Err(error.clone()),
                    });
                    continue;
                }
            }
        } else {
            parent.to_path_buf()
        };
        groups
            .entry((directory, manifest.language.to_string()))
            .or_default()
            .push(manifest);
    }
    for ((directory, _), manifests) in groups {
        let language = manifests[0].language;
        let scope = match language {
            Language::Rust => cargo_scope(&directory, &manifests),
            Language::Php => composer_scope(&directory, &manifests),
            _ => unreachable!(),
        };
        jobs.push(InstallJob {
            manifests: manifests
                .iter()
                .map(|manifest| manifest.path.clone())
                .collect(),
            policy: install_age_policy(orchestrator, result, &directory, language),
            directory,
            language,
            scope,
        });
    }
    jobs.sort_by(|left, right| {
        left.directory
            .cmp(&right.directory)
            .then_with(|| left.language.to_string().cmp(&right.language.to_string()))
    });
}

fn composer_scope(
    directory: &Path,
    manifests: &[&ManifestUpdateResult],
) -> Result<InstallScope, String> {
    let has_lock = directory
        .join("composer.lock")
        .try_exists()
        .map_err(|error| format!("Cannot inspect composer.lock: {error}"))?;
    if !has_lock {
        return Ok(InstallScope::Full);
    }
    let names: BTreeSet<_> = manifests
        .iter()
        .flat_map(|manifest| manifest.updates())
        .map(|update| update.dependency().name.to_ascii_lowercase())
        .collect();
    Ok(InstallScope::Packages(names.into_iter().collect()))
}

fn cargo_scope(
    directory: &Path,
    manifests: &[&ManifestUpdateResult],
) -> Result<InstallScope, String> {
    let path = directory.join("Cargo.lock");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstallScope::CargoWorkspace);
        }
        Err(error) => return Err(format!("Cannot read pre-install Cargo.lock: {error}")),
    };
    let lock: toml::Value =
        toml::from_str(&content).map_err(|_| "Cannot parse pre-install Cargo.lock".to_string())?;
    let entries = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or("Pre-install Cargo.lock has no package list")?;
    if entries.iter().any(|entry| {
        entry.get("name").and_then(toml::Value::as_str).is_none()
            || entry.get("version").and_then(toml::Value::as_str).is_none()
    }) {
        return Err("Pre-install Cargo.lock contains an invalid package".into());
    }
    let packages = parse_lock_packages(&content);
    let mut selectors = BTreeSet::new();
    for manifest in manifests {
        for update in manifest.updates() {
            let dependency = update.dependency();
            // タグの変更で新しい取得元IDができる。古い取得元の固定を外さず整合させる。
            if dependency
                .git_source
                .as_ref()
                .is_some_and(|git| matches!(git.reference, GitReference::Tag(_)))
            {
                continue;
            }
            let requirement = if dependency.git_source.is_none() {
                Some(
                    semver::VersionReq::parse(&dependency.version_spec.raw).map_err(|_| {
                        format!(
                            "Cannot identify the old Cargo requirement for {}",
                            dependency.name
                        )
                    })?,
                )
            } else {
                None
            };
            let mut candidates: Vec<_> = packages
                .iter()
                .filter(|package| {
                    package.name == dependency.name
                        && matches_source(package, dependency)
                        && requirement.as_ref().is_none_or(|requirement| {
                            semver::Version::parse(&package.version)
                                .is_ok_and(|version| requirement.matches(&version))
                        })
                })
                .collect();
            if candidates.len() > 1 {
                let direct = direct_packages(&manifest.path, dependency, &packages)?;
                candidates.retain(|candidate| direct.contains(candidate));
            }
            match candidates.as_slice() {
                [package] => {
                    let source = package
                        .source
                        .as_deref()
                        .unwrap()
                        .split('#')
                        .next()
                        .unwrap();
                    selectors.insert(format!("{source}#{}@{}", package.name, package.version));
                }
                [] if dependency.git_source.is_none() => {
                    // 変更後のマニフェストが、まだ lock に入ったことのない依存を参照する場合もある。
                }
                _ => {
                    return Err(format!(
                        "Cannot uniquely identify {} in pre-install Cargo.lock",
                        dependency.name
                    ));
                }
            }
        }
    }
    if selectors.is_empty() {
        Ok(InstallScope::CargoWorkspace)
    } else {
        Ok(InstallScope::Packages(selectors.into_iter().collect()))
    }
}

fn matches_source(package: &LockPackage, dependency: &Dependency) -> bool {
    let Some(source) = package.source.as_deref() else {
        return false;
    };
    let Some(git) = &dependency.git_source else {
        return is_crates_io_source(source);
    };
    let Some(source) = source.strip_prefix("git+") else {
        return false;
    };
    let Ok(mut source) = reqwest::Url::parse(source) else {
        return false;
    };
    let Ok(mut expected) = reqwest::Url::parse(&git.url) else {
        return false;
    };
    let reference_matches = match &git.reference {
        GitReference::Branch(branch) => source
            .query_pairs()
            .any(|(key, value)| key == "branch" && value == branch.as_str()),
        GitReference::DefaultBranch => source.query().is_none(),
        GitReference::Tag(_) | GitReference::Rev(_) => false,
    };
    source.set_query(None);
    source.set_fragment(None);
    expected.set_query(None);
    expected.set_fragment(None);
    reference_matches
        && source
            .as_str()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            == expected
                .as_str()
                .trim_end_matches('/')
                .trim_end_matches(".git")
}

fn read_manifest(path: &Path) -> Result<toml::Value, String> {
    std::fs::read_to_string(path)
        .map_err(|_| "Cannot read Cargo.toml for scoped install".into())
        .and_then(|content| {
            toml::from_str(&content)
                .map_err(|_| "Cannot parse Cargo.toml for scoped install".into())
        })
}

fn direct_packages<'a>(
    path: &Path,
    dependency: &Dependency,
    packages: &'a [LockPackage],
) -> Result<Vec<&'a LockPackage>, String> {
    let manifest = read_manifest(path)?;
    let workspace_dependency = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get(dependency.manifest_name()))
        .is_some();
    let member_name = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str);
    let parents: Vec<_> = packages
        .iter()
        .filter(|package| {
            package.source.is_none()
                && (workspace_dependency || member_name == Some(package.name.as_str()))
        })
        .collect();
    Ok(packages
        .iter()
        .filter(|candidate| {
            parents.iter().any(|parent| {
                parent.dependencies.iter().any(|edge| {
                    edge.name == candidate.name
                        && edge
                            .version
                            .as_ref()
                            .is_none_or(|version| version == &candidate.version)
                        && edge.source.as_ref().is_none_or(|source| {
                            candidate.source.as_ref().is_some_and(|candidate_source| {
                                source
                                    == candidate_source
                                        .split('#')
                                        .next()
                                        .unwrap_or(candidate_source)
                            })
                        })
                })
            })
        })
        .collect())
}

#[cfg(test)]
mod tests;
