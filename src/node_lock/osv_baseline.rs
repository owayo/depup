//! OSV の比較に使う、公開 npm の直接依存の解決版。
use super::*;

pub(crate) async fn osv_lock_snapshot(
    directory: &Path,
    manager: &str,
    baseline: bool,
) -> Result<(Vec<u8>, BTreeSet<(String, String)>), String> {
    if manager == "pnpm" {
        if baseline {
            let program = which::which_in("pnpm", std::env::var_os("PATH"), directory)
                .unwrap_or_else(|_| "pnpm".into());
            return pnpm_osv_baseline(directory, &program).await;
        }
        let (bytes, snapshot) = refresh_snapshot(directory).await?;
        return Ok((bytes, osv_packages(&snapshot)?));
    }
    if manager != "npm" {
        return Err("unsupported OSV lockfile".into());
    }
    let path = directory.join(npm_lock_name(directory));
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    let lock: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    verify_npm_manifest_dependencies(directory, &lock)?;
    let snapshot = parse_npm_snapshot(&lock)?;
    if std::fs::read(path).map_err(|error| error.to_string())? != bytes {
        return Err("npm lock changed during OSV snapshot".into());
    }
    Ok((bytes, osv_packages(&snapshot)?))
}

async fn pnpm_osv_baseline(
    directory: &Path,
    program: &Path,
) -> Result<(Vec<u8>, BTreeSet<(String, String)>), String> {
    // manifest の書き換え後の警告は、実行前の版の同一性を失わせない。
    let deadline = Instant::now() + NODE_AUDIT_BUDGET;
    let path = directory.join("pnpm-lock.yaml");
    let bytes = read_lock(&path, deadline).await?;
    let (value, _) = list_pnpm_lockfile(directory, program, deadline).await?;
    let mut snapshot = parse_pnpm_snapshot(&value)?;
    verify_pnpm_registry_sources(directory, program, &mut snapshot, deadline).await?;
    if read_lock(&path, deadline).await? != bytes {
        return Err("pnpm lock changed during OSV baseline verification".into());
    }
    Ok((bytes, osv_packages(&snapshot)?))
}

pub(crate) fn osv_packages(snapshot: &LockSnapshot) -> Result<BTreeSet<(String, String)>, String> {
    if let Some(unknown) = snapshot
        .unverified
        .iter()
        .find(|item| item.kind == UnverifiedKind::Unconfirmed)
    {
        return Err(format!(
            "{} {} is unconfirmed for OSV: {}",
            unknown.name, unknown.version, unknown.reason
        ));
    }
    Ok(snapshot.packages.clone())
}

pub(crate) async fn osv_direct_baselines(manifest: &Path) -> BTreeMap<String, (String, String)> {
    read_direct(manifest).await.unwrap_or_default()
}

async fn read_direct(manifest: &Path) -> Result<BTreeMap<String, (String, String)>, String> {
    let parent = manifest.parent().ok_or("manifest has no directory")?;
    let directory = crate::node_scope::node_resolution_directory(manifest);
    let manager = crate::package_manager::SystemPackageManager::new()
        .resolve_package_manager(crate::domain::Language::Node, &directory)
        .map(|(_, manager)| manager);
    if manager == Some("pnpm") && directory.join("pnpm-lock.yaml").is_file() {
        let (_, snapshot) = refresh_snapshot(&directory).await?;
        let importer = parent.canonicalize().map_err(|error| error.to_string())?;
        return Ok(snapshot
            .direct_edges
            .get(&importer)
            .into_iter()
            .flatten()
            .filter(|(_, pair)| {
                snapshot.packages.contains(*pair)
                    && !snapshot
                        .unverified
                        .iter()
                        .any(|unknown| (&unknown.name, &unknown.version) == (&pair.0, &pair.1))
            })
            .map(|(key, pair)| (key.clone(), pair.clone()))
            .collect());
    }
    if manager != Some("npm") {
        return Ok(BTreeMap::new());
    }
    let path = directory.join(npm_lock_name(&directory));
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    let lock: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let _ = parse_npm_snapshot(&lock)?;
    verify_npm_manifest_dependencies(&directory, &lock)?;
    let packages = lock
        .get("packages")
        .and_then(Value::as_object)
        .ok_or("invalid npm lock")?;
    let parent = parent.canonicalize().map_err(|error| error.to_string())?;
    let root = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let issuer = parent
        .strip_prefix(root)
        .map_err(|error| error.to_string())?
        .to_str()
        .ok_or("invalid importer path")?
        .replace('\\', "/");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let mut result = BTreeMap::new();
    for section in ["dependencies", "devDependencies", "optionalDependencies"] {
        for (key, _) in manifest
            .get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if let Some(record) = resolve_npm_dependency(&issuer, key, packages) {
                let mut snapshot = LockSnapshot::default();
                collect_package(&mut snapshot, key, record);
                if snapshot.unverified.is_empty() && snapshot.packages.len() == 1 {
                    result.insert(key.clone(), snapshot.packages.into_iter().next().unwrap());
                }
            }
        }
    }
    if std::fs::read(path).map_err(|error| error.to_string())? != bytes {
        return Err("npm lock changed while reading OSV baseline".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn pnpm_osv_baseline_allows_stale_manifest_warnings_but_requires_public_identity() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();
        let program = temp.path().join("pnpm");
        for (registry, permitted) in [
            ("\"https://registry.npmjs.org/\"", true),
            ("not-json", false),
        ] {
            crate::test_support::exec::write_script(
                &program,
                &format!(
                    r#"#!/bin/sh
if [ "$1" = config ]; then
  printf '%s\n' '{registry}'
else
  printf '%s\n' '[{{"dependencies":{{"example":{{"version":"1.0.0","resolved":"https://registry.npmjs.org/example/-/example-1.0.0.tgz"}}}}}}]'
  printf '%s\n' 'manifest differs from lockfile' >&2
fi
"#
                ),
                0o755,
            );
            let result = pnpm_osv_baseline(temp.path(), &program).await;
            assert_eq!(result.is_ok(), permitted, "{result:?}");
            if let Ok((_, packages)) = result {
                assert!(packages.contains(&("example".into(), "1.0.0".into())));
            }
        }
    }

    #[tokio::test]
    async fn npm_osv_baseline_tracks_aliases_and_excludes_private_sources() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp.path().join("package.json");
        std::fs::write(&manifest, r#"{"dependencies":{"alias":"npm:real@^1.0.0"}}"#).unwrap();
        let path = temp.path().join("package-lock.json");
        for (host, expected) in [
            ("registry.npmjs.org", true),
            ("packages.example.com", false),
        ] {
            let lock = serde_json::json!({"lockfileVersion":3,"packages":{
                "":{"dependencies":{"alias":"npm:real@^1.0.0"}},
                "node_modules/alias":{"name":"real","version":"1.4.0","resolved":format!("https://{host}/real/-/real-1.4.0.tgz")}
            }});
            std::fs::write(&path, serde_json::to_vec(&lock).unwrap()).unwrap();
            let baseline = osv_direct_baselines(&manifest).await;
            if expected {
                assert_eq!(baseline["alias"], ("real".into(), "1.4.0".into()));
            } else {
                assert!(baseline.is_empty());
            }
        }
    }
}
