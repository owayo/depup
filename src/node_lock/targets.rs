//! 更新した各宣言を、最終的な pnpm importer の対応する辺と正確に照合する。
use super::*;
use crate::domain::{Language, UpdateResult, UpdateSummary};

pub(crate) type DirectEdges = BTreeMap<PathBuf, BTreeMap<String, (String, String)>>;

pub(crate) struct TargetFailure {
    pub path: PathBuf,
    pub index: usize,
    pub message: String,
}

pub(crate) fn target_failures(
    summary: &UpdateSummary,
    directory: &Path,
    edges: &DirectEdges,
) -> Vec<TargetFailure> {
    evaluate_targets(summary, directory, Ok(edges))
}

fn evaluate_targets(
    summary: &UpdateSummary,
    directory: &Path,
    edges: Result<&DirectEdges, &str>,
) -> Vec<TargetFailure> {
    let mut failures = Vec::new();
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    for manifest in summary.by_language(Language::Node).filter(|manifest| {
        crate::node_scope::node_resolution_directory(&manifest.path) == directory
    }) {
        let importer = manifest
            .path
            .parent()
            .and_then(|path| path.canonicalize().ok());
        let declaration = std::fs::read(&manifest.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        for (index, update) in manifest.results.iter().enumerate() {
            let UpdateResult::Update {
                dependency,
                new_version,
                write_error: None,
                ..
            } = update
            else {
                continue;
            };
            let key = dependency.manifest_name();
            // peer 要件しかない場合は、ローカルへの install を要求しない。
            let peer_only = declaration.as_ref().is_some_and(|value| {
                value
                    .get("peerDependencies")
                    .and_then(Value::as_object)
                    .is_some_and(|peers| peers.contains_key(key))
                    && !["dependencies", "devDependencies", "optionalDependencies"]
                        .iter()
                        .any(|section| {
                            value
                                .get(section)
                                .and_then(Value::as_object)
                                .is_some_and(|dependencies| dependencies.contains_key(key))
                        })
            });
            if peer_only {
                continue;
            }
            let checked = (|| {
                if declaration.is_none() {
                    return Err(
                        "package.json could not be read for final resolution verification"
                            .to_string(),
                    );
                }
                let edges = edges.map_err(str::to_string)?;
                let (name, resolved) = importer
                    .as_ref()
                    .and_then(|path| edges.get(path))
                    .and_then(|dependencies| dependencies.get(key))
                    .ok_or_else(|| "the exact importer dependency edge is missing".to_string())?;
                if name != &dependency.name {
                    return Err("the resolved package identity is ambiguous or differs from the declaration".into());
                }
                let parse = |version: &str| {
                    semver::Version::parse(version.strip_prefix('v').unwrap_or(version))
                };
                let resolved_version = parse(resolved)
                    .map_err(|_| "the resolved version is invalid or ambiguous".to_string())?;
                let target = parse(new_version)
                    .map_err(|_| "the update target is not a valid npm version".to_string())?;
                if resolved_version.cmp_precedence(&target).is_lt() {
                    return Err(format!(
                        "resolved {resolved}, below the reported target {new_version}"
                    ));
                }
                Ok(())
            })();
            if let Err(reason) = checked {
                failures.push(TargetFailure {
                    path: manifest.path.clone(),
                    index,
                    message: format!("pnpm did not verify {key} at target {new_version}: {reason}"),
                });
            }
        }
    }
    failures
}

pub async fn verify_pnpm_update_targets(
    directory: &Path,
    summary: &mut UpdateSummary,
) -> Vec<String> {
    let program = which::which_in("pnpm", std::env::var_os("PATH"), directory)
        .unwrap_or_else(|_| "pnpm".into());
    verify_with(directory, summary, &program).await
}

async fn verify_with(directory: &Path, summary: &mut UpdateSummary, program: &Path) -> Vec<String> {
    if target_failures(summary, directory, &BTreeMap::new()).is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + NODE_AUDIT_BUDGET;
    let checked = async {
        let path = directory.join("pnpm-lock.yaml");
        let before = read_target_lock(&path, deadline).await?;
        let snapshot = read_pnpm_graph(directory, program, deadline).await?;
        if read_target_lock(&path, deadline).await? != before {
            return Err("pnpm-lock.yaml changed during final target verification".into());
        }
        Ok::<_, String>(snapshot)
    }
    .await;
    let failures = evaluate_targets(
        summary,
        directory,
        checked
            .as_ref()
            .map(|snapshot| &snapshot.direct_edges)
            .map_err(String::as_str),
    );
    failures
        .into_iter()
        .map(|failure| {
            let message = format!(
                "{}; manifest and lockfile changes were kept for inspection",
                failure.message
            );
            summary.record_write_error(&failure.path, failure.index, &message);
            message
        })
        .collect()
}

async fn read_target_lock(path: &Path, deadline: Instant) -> Result<Vec<u8>, String> {
    tokio::time::timeout_at(deadline, tokio::fs::read(path))
        .await
        .map_err(|_| "pnpm-lock.yaml read timed out during final target verification".to_string())?
        .map_err(|_| "could not read pnpm-lock.yaml for final target verification".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, ManifestUpdateResult, VersionSpec, VersionSpecKind};

    #[cfg(unix)]
    #[tokio::test]
    async fn pnpm_final_verification_marks_failures_and_keeps_written_files_without_age() {
        for graph in [
            r#"[{"dependencies":{"example":{"version":"1.1.0"}}}]"#,
            r#"[{"dependencies":{}}]"#,
            "invalid",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let path = root.join("package.json");
            let content = r#"{"dependencies":{"example":"^1.2.0"}}"#;
            std::fs::write(&path, content).unwrap();
            std::fs::write(root.join("pnpm-lock.yaml"), "lock bytes").unwrap();
            let program = root.join("fake-pnpm");
            crate::test_support::exec::write_script(
                &program,
                &format!("#!/bin/sh\nprintf '%s' '{graph}'\n"),
                0o755,
            );
            let mut manifest = ManifestUpdateResult::new(&path, Language::Node);
            manifest.add_result(UpdateResult::update(
                Dependency::new(
                    "example",
                    VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0"),
                    false,
                    Language::Node,
                ),
                "1.2.0",
            ));
            let mut summary = UpdateSummary::default();
            summary.add_manifest(manifest);
            let failures = verify_with(&root, &mut summary, &program).await;
            assert_eq!(failures.len(), 1);
            assert_eq!(summary.total_updates(), 0);
            assert_eq!(summary.total_failed(), 1);
            assert!(failures[0].contains("changes were kept"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
            assert_eq!(
                std::fs::read_to_string(root.join("pnpm-lock.yaml")).unwrap(),
                "lock bytes"
            );
        }
    }

    #[test]
    fn pnpm_final_targets_require_each_exact_alias_edge_and_accept_newer_versions() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"alias":"npm:actual@^1.2.0"}}"#,
        )
        .unwrap();
        let mut manifest = ManifestUpdateResult::new(root.join("package.json"), Language::Node);
        let dependency = Dependency::new(
            "actual",
            VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0"),
            false,
            Language::Node,
        )
        .with_manifest_name("alias");
        manifest.add_result(UpdateResult::update(dependency, "1.2.0"));
        let mut summary = UpdateSummary::default();
        summary.add_manifest(manifest);
        for (name, version, failures) in [
            ("actual", "1.1.0", 1),
            ("actual", "1.2.0", 0),
            ("actual", "1.3.0", 0),
            ("actual", "invalid", 1),
            ("different", "2.0.0", 1),
        ] {
            let edges = BTreeMap::from([(
                root.clone(),
                BTreeMap::from([("alias".into(), (name.into(), version.into()))]),
            )]);
            assert_eq!(
                target_failures(&summary, &root, &edges).len(),
                failures,
                "{name} {version}"
            );
        }
        let wrong_alias = BTreeMap::from([(
            root.clone(),
            BTreeMap::from([("other".into(), ("actual".into(), "2.0.0".into()))]),
        )]);
        assert_eq!(target_failures(&summary, &root, &wrong_alias).len(), 1);
        assert_eq!(target_failures(&summary, &root, &BTreeMap::new()).len(), 1);
        summary.manifests[0].record_write_error(0, "already failed");
        assert!(target_failures(&summary, &root, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn pnpm_final_targets_do_not_borrow_another_importer_and_exempt_peer_only_requirements() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut summary = UpdateSummary::default();
        let mut manifest = ManifestUpdateResult::new(root.join("package.json"), Language::Node);
        manifest.add_result(UpdateResult::update(
            Dependency::new(
                "example",
                VersionSpec::new(VersionSpecKind::Caret, "^1.0.0", "1.0.0"),
                false,
                Language::Node,
            ),
            "1.2.0",
        ));
        summary.add_manifest(manifest);
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"example":"^1.2.0"}}"#,
        )
        .unwrap();
        let another_importer = BTreeMap::from([(
            root.join("other"),
            BTreeMap::from([("example".into(), ("example".into(), "2.0.0".into()))]),
        )]);
        assert_eq!(target_failures(&summary, &root, &another_importer).len(), 1);
        std::fs::write(
            root.join("package.json"),
            r#"{"peerDependencies":{"example":"^1.2.0"}}"#,
        )
        .unwrap();
        assert!(target_failures(&summary, &root, &BTreeMap::new()).is_empty());
        std::fs::write(
            root.join("package.json"),
            r#"{"optionalDependencies":{"example":"^1.2.0"}}"#,
        )
        .unwrap();
        assert_eq!(target_failures(&summary, &root, &BTreeMap::new()).len(), 1);
        summary.manifests[0].results.clear();
        assert!(target_failures(&summary, &root, &BTreeMap::new()).is_empty());
    }
}
