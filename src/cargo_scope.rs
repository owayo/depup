//! Cargo 自身の読み取り専用 workspace 判定を install と事前確認で共有する。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CargoAnchor {
    pub cwd: PathBuf,
    pub program: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct LocatedCargoRoot {
    pub root: PathBuf,
    pub anchor: CargoAnchor,
}

pub(crate) trait CargoScopeResolver: Sync {
    fn resolve(&self, manifest: &Path, boundary: &Path) -> Result<LocatedCargoRoot, String>;
    fn resolve_from(
        &self,
        anchor: &CargoAnchor,
        manifest: &Path,
        boundary: &Path,
    ) -> Result<PathBuf, String>;
}

pub(crate) struct SystemCargoScopeResolver;

impl CargoScopeResolver for SystemCargoScopeResolver {
    fn resolve(&self, manifest: &Path, boundary: &Path) -> Result<LocatedCargoRoot, String> {
        let manifest = std::fs::canonicalize(manifest)
            .map_err(|error| format!("Cannot locate Cargo workspace: {error}"))?;
        let parent = manifest
            .parent()
            .ok_or("Cargo manifest has no parent directory")?;
        let program = which::which_in("cargo", std::env::var_os("PATH"), parent)
            .map_err(|error| format!("Cannot locate Cargo executable: {error}"))?;
        let anchor = CargoAnchor {
            cwd: parent.to_path_buf(),
            program,
        };
        let root = run_locate(
            &anchor,
            Path::new(
                manifest
                    .file_name()
                    .ok_or("Cargo manifest has no filename")?,
            ),
            boundary,
        )?;
        Ok(LocatedCargoRoot { root, anchor })
    }

    fn resolve_from(
        &self,
        anchor: &CargoAnchor,
        manifest: &Path,
        boundary: &Path,
    ) -> Result<PathBuf, String> {
        let manifest = std::fs::canonicalize(manifest)
            .map_err(|error| format!("Cannot read Cargo manifest for recheck: {error}"))?;
        let argument = relative_manifest_arg(&anchor.cwd, &manifest)
            .ok_or("Cannot form a relative Cargo manifest path from the successful context")?;
        run_locate(anchor, &argument, boundary)
    }
}

pub(crate) fn rust_resolution_directory(
    manifest: &Path,
    boundary: &Path,
) -> Result<PathBuf, String> {
    SystemCargoScopeResolver
        .resolve(manifest, boundary)
        .map(|located| located.root)
}

fn run_locate(
    anchor: &CargoAnchor,
    manifest_arg: &Path,
    boundary: &Path,
) -> Result<PathBuf, String> {
    // Cargo accounts for exclusions, implicit path members and package.workspace.
    // locate-project only reads manifests; it never resolves or writes a lock.
    let output = std::process::Command::new(&anchor.program)
        .args([
            "locate-project",
            "--workspace",
            "--message-format",
            "json",
            "--manifest-path",
        ])
        // Keep canonical Windows prefixes out of argv. Rechecks keep the exact
        // program and cwd that succeeded, including their toolchain selection.
        .arg(manifest_arg)
        .current_dir(&anchor.cwd)
        .env("CARGO_NET_OFFLINE", "true")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()
        .map_err(|error| format!("Cannot run cargo locate-project: {error}"))?;
    if !output.status.success() {
        let message = crate::package_manager::redact_install_output(
            &String::from_utf8_lossy(&output.stderr),
            &[],
        );
        return Err(format!("Cannot locate Cargo workspace: {}", message.trim()));
    }
    let location: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Cannot parse cargo locate-project output")?;
    let root = location
        .get("root")
        .and_then(serde_json::Value::as_str)
        .ok_or("cargo locate-project returned no root")?;
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("Cannot read located Cargo root: {error}"))?;
    let directory = root
        .parent()
        .ok_or("Located Cargo root has no parent directory")?;
    let boundary = std::fs::canonicalize(boundary)
        .map_err(|error| format!("Cannot read Cargo execution boundary: {error}"))?;
    if !directory.starts_with(boundary) {
        return Err("Cargo resolved a workspace outside the execution boundary".into());
    }
    Ok(directory.to_path_buf())
}

/// Both paths are canonical. Different drives or prefix kinds cannot be retried.
fn relative_manifest_arg(cwd: &Path, manifest: &Path) -> Option<PathBuf> {
    if !cwd.is_absolute() || !manifest.is_absolute() {
        return None;
    }
    let from: Vec<_> = cwd.components().collect();
    let to: Vec<_> = manifest.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0
        || from[common..]
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        || to[common..]
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in &from[common..] {
        relative.push("..");
    }
    for part in &to[common..] {
        relative.push(part.as_os_str());
    }
    Some(relative)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CargoRoot {
    Known(PathBuf),
    Unknown(String),
}

/// One lazy snapshot serves the preflight gate and its write/install exclusions.
pub(crate) struct CargoMembership<'a> {
    manifests: BTreeMap<PathBuf, PathBuf>,
    boundary: &'a Path,
    resolver: &'a dyn CargoScopeResolver,
    roots: OnceLock<BTreeMap<PathBuf, CargoRoot>>,
}

impl<'a> CargoMembership<'a> {
    pub fn new(
        manifests: impl IntoIterator<Item = PathBuf>,
        boundary: &'a Path,
        resolver: &'a dyn CargoScopeResolver,
    ) -> Self {
        Self {
            manifests: manifests
                .into_iter()
                .map(|path| {
                    let target = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    (path, target)
                })
                .collect(),
            boundary,
            resolver,
            roots: OnceLock::new(),
        }
    }

    pub fn roots(&self) -> &BTreeMap<PathBuf, CargoRoot> {
        self.roots.get_or_init(|| self.classify())
    }

    pub fn identity(&self, manifest: &Path) -> &Path {
        &self.manifests[manifest]
    }

    fn classify(&self) -> BTreeMap<PathBuf, CargoRoot> {
        let mut roots = BTreeMap::new();
        let mut anchors: BTreeMap<PathBuf, CargoAnchor> = BTreeMap::new();
        let mut failed = BTreeMap::new();
        let targets: BTreeSet<_> = self.manifests.values().cloned().collect();
        for manifest in &targets {
            match self.resolver.resolve(manifest, self.boundary) {
                Ok(located) => {
                    anchors
                        .entry(located.root.clone())
                        .and_modify(|anchor| {
                            if located.anchor < *anchor {
                                *anchor = located.anchor.clone();
                            }
                        })
                        .or_insert(located.anchor);
                    roots.insert(manifest.clone(), CargoRoot::Known(located.root));
                }
                Err(error) => {
                    failed.insert(manifest.clone(), error);
                }
            }
        }
        let mut cache = BTreeMap::new();
        for (manifest, primary_error) in failed {
            let mut answers = BTreeSet::new();
            let mut reasons = vec![format!("primary: {primary_error}")];
            for anchor in anchors.values() {
                let answer = cache
                    .entry((anchor.cwd.clone(), manifest.clone()))
                    .or_insert_with(|| {
                        self.resolver.resolve_from(anchor, &manifest, self.boundary)
                    });
                match answer {
                    Ok(root) => {
                        answers.insert(root.clone());
                    }
                    Err(error) => {
                        reasons.push(format!("rechecked from {}: {error}", anchor.cwd.display()))
                    }
                }
            }
            let root = if answers.len() == 1 {
                CargoRoot::Known(answers.pop_first().unwrap())
            } else {
                if !answers.is_empty() {
                    reasons.push(format!(
                        "successful Cargo anchors disagree: {}",
                        answers
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                CargoRoot::Unknown(reasons.join("; "))
            };
            roots.insert(manifest, root);
        }
        roots
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct TableResolver {
        pub primary: BTreeMap<PathBuf, Result<LocatedCargoRoot, String>>,
        pub secondary: BTreeMap<(PathBuf, PathBuf), Result<PathBuf, String>>,
        pub calls: Mutex<Vec<(Option<PathBuf>, PathBuf)>>,
    }

    impl TableResolver {
        pub fn known(&mut self, manifest: &Path, root: &Path) {
            let manifest = std::fs::canonicalize(manifest).unwrap();
            self.primary.insert(
                manifest.clone(),
                Ok(LocatedCargoRoot {
                    root: std::fs::canonicalize(root).unwrap(),
                    anchor: CargoAnchor {
                        cwd: manifest.parent().unwrap().into(),
                        program: "cargo".into(),
                    },
                }),
            );
        }

        pub fn fail(&mut self, manifest: &Path) {
            self.primary.insert(
                std::fs::canonicalize(manifest).unwrap(),
                Err("simulated member toolchain failure".into()),
            );
        }

        pub fn answer(&mut self, manifest: &Path, answer: Result<PathBuf, String>) {
            let manifest = std::fs::canonicalize(manifest).unwrap();
            for located in self
                .primary
                .values()
                .filter_map(|answer| answer.as_ref().ok())
            {
                self.secondary.insert(
                    (located.anchor.cwd.clone(), manifest.clone()),
                    answer.clone(),
                );
            }
        }

        pub fn assert_cached(&self) {
            let calls = self.calls.lock().unwrap();
            assert!(!calls.is_empty());
            assert_eq!(calls.len(), calls.iter().collect::<BTreeSet<_>>().len());
        }
    }

    impl CargoScopeResolver for TableResolver {
        fn resolve(&self, manifest: &Path, _boundary: &Path) -> Result<LocatedCargoRoot, String> {
            self.calls.lock().unwrap().push((None, manifest.into()));
            self.primary[manifest].clone()
        }

        fn resolve_from(
            &self,
            anchor: &CargoAnchor,
            manifest: &Path,
            _boundary: &Path,
        ) -> Result<PathBuf, String> {
            self.calls
                .lock()
                .unwrap()
                .push((Some(anchor.cwd.clone()), manifest.into()));
            self.secondary
                .get(&(anchor.cwd.clone(), manifest.into()))
                .cloned()
                .unwrap_or_else(|| Err("simulated recheck failure".into()))
        }
    }

    fn manifest(root: &Path, name: &str) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir_all(directory.join("src")).unwrap();
        std::fs::write(directory.join("src/lib.rs"), "").unwrap();
        let path = directory.join("Cargo.toml");
        std::fs::write(
            &path,
            format!("[package]\nname = '{name}'\nversion = '0.1.0'\n"),
        )
        .unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    #[test]
    fn membership_is_lazy_uses_one_fixed_anchor_per_root_and_caches_agreement() {
        let temp = tempfile::tempdir().unwrap();
        let a = manifest(temp.path(), "a");
        let z = manifest(temp.path(), "z");
        let b = manifest(temp.path(), "b");
        let failed = manifest(temp.path(), "failed");
        let root_a = a.parent().unwrap();
        let root_b = b.parent().unwrap();
        let mut resolver = TableResolver::default();
        resolver.known(&a, root_a);
        resolver.known(&z, root_a);
        resolver.known(&b, root_b);
        resolver.fail(&failed);
        resolver.answer(&failed, Ok(root_a.into()));
        let membership = CargoMembership::new(
            [z.clone(), a.clone(), a.clone(), b.clone(), failed.clone()],
            temp.path(),
            &resolver,
        );
        assert!(resolver.calls.lock().unwrap().is_empty());
        assert_eq!(membership.roots()[&failed], CargoRoot::Known(root_a.into()));
        assert_eq!(membership.roots()[&failed], CargoRoot::Known(root_a.into()));
        resolver.assert_cached();
        let calls = resolver.calls.lock().unwrap();
        assert_eq!(calls.len(), 6);
        assert!(calls.contains(&(Some(root_a.into()), failed.clone())));
        assert!(!calls.contains(&(Some(z.parent().unwrap().into()), failed)));
    }

    #[test]
    fn membership_disagreement_keeps_primary_error_and_conflicting_roots() {
        let temp = tempfile::tempdir().unwrap();
        let a = manifest(temp.path(), "a");
        let b = manifest(temp.path(), "b");
        let failed = manifest(temp.path(), "failed");
        let mut resolver = TableResolver::default();
        for path in [&a, &b] {
            resolver.known(path, path.parent().unwrap());
        }
        resolver.fail(&failed);
        for path in [&a, &b] {
            resolver.secondary.insert(
                (path.parent().unwrap().into(), failed.clone()),
                Ok(path.parent().unwrap().into()),
            );
        }
        let membership = CargoMembership::new(
            [a.clone(), b.clone(), failed.clone()],
            temp.path(),
            &resolver,
        );
        let CargoRoot::Unknown(reason) = &membership.roots()[&failed] else {
            panic!("conflicting roots were accepted");
        };
        assert!(reason.contains("simulated member toolchain failure"));
        assert!(reason.contains("anchors disagree"));
        for path in [&a, &b] {
            assert!(reason.contains(&path.parent().unwrap().display().to_string()));
        }
        resolver.assert_cached();
    }

    #[test]
    fn membership_retains_primary_and_recheck_errors_when_no_context_recovers() {
        let temp = tempfile::tempdir().unwrap();
        let a = manifest(temp.path(), "a");
        let failed = manifest(temp.path(), "failed");
        for all_failed in [false, true] {
            let mut resolver = TableResolver::default();
            resolver.fail(&failed);
            if all_failed {
                resolver.fail(&a);
            } else {
                resolver.known(&a, a.parent().unwrap());
            }
            let membership =
                CargoMembership::new([a.clone(), failed.clone()], temp.path(), &resolver);
            let CargoRoot::Unknown(reason) = &membership.roots()[&failed] else {
                panic!("unverified root was accepted");
            };
            assert!(reason.contains("primary: simulated member toolchain failure"));
            assert_eq!(reason.contains("rechecked from"), !all_failed);
            assert_eq!(reason.contains("simulated recheck failure"), !all_failed);
        }
    }

    #[test]
    fn relative_manifest_arguments_cover_siblings_and_deep_paths() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(
            relative_manifest_arg(&base.path().join("a"), &base.path().join("b/Cargo.toml")),
            Some(PathBuf::from("../b/Cargo.toml"))
        );
        assert_eq!(
            relative_manifest_arg(
                &base.path().join("apps/a/src-tauri"),
                &base.path().join("helpers/deep/Cargo.toml")
            ),
            Some(PathBuf::from("../../../helpers/deep/Cargo.toml"))
        );
        assert_eq!(
            relative_manifest_arg(base.path(), &base.path().join("Cargo.toml")),
            Some(PathBuf::from("Cargo.toml"))
        );
        assert!(relative_manifest_arg(Path::new("relative"), base.path()).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn relative_manifest_arguments_keep_verbatim_prefixes_out_of_argv() {
        assert_eq!(
            relative_manifest_arg(
                Path::new(r"\\?\C:\work\a"),
                Path::new(r"\\?\C:\work\b\Cargo.toml")
            ),
            Some(PathBuf::from(r"..\b\Cargo.toml"))
        );
        assert!(
            relative_manifest_arg(
                Path::new(r"\\?\C:\work\a"),
                Path::new(r"\\?\D:\work\Cargo.toml")
            )
            .is_none()
        );
        assert!(
            relative_manifest_arg(
                Path::new(r"C:\work\a"),
                Path::new(r"\\?\C:\work\Cargo.toml")
            )
            .is_none()
        );
        assert!(
            relative_manifest_arg(
                Path::new(r"\\?\UNC\host\share\a"),
                Path::new(r"\\?\UNC\other\share\Cargo.toml")
            )
            .is_none()
        );
    }

    #[test]
    fn successful_context_rechecks_real_shared_and_excluded_roots_without_writes() {
        let temp = tempfile::tempdir().unwrap();
        let a = manifest(temp.path(), "a");
        let b = manifest(temp.path(), "b");
        let excluded = manifest(temp.path(), "excluded");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers=['a', 'b']\nexclude=['excluded']\nresolver='3'\n",
        )
        .unwrap();
        let lock = temp.path().join("Cargo.lock");
        std::fs::write(&lock, "unchanged lock sentinel\n").unwrap();
        let located = SystemCargoScopeResolver.resolve(&a, temp.path()).unwrap();
        assert_eq!(
            SystemCargoScopeResolver
                .resolve_from(&located.anchor, &b, temp.path())
                .unwrap(),
            std::fs::canonicalize(temp.path()).unwrap()
        );
        assert_eq!(
            SystemCargoScopeResolver
                .resolve_from(&located.anchor, &excluded, temp.path())
                .unwrap(),
            excluded.parent().unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(&lock).unwrap(),
            "unchanged lock sentinel\n"
        );
        assert!(!temp.path().join("target").exists());
    }

    #[cfg(unix)]
    #[test]
    fn membership_uses_one_canonical_identity_for_manifest_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let target = manifest(temp.path(), "target");
        let alias = temp.path().join("alias.toml");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let mut resolver = TableResolver::default();
        resolver.fail(&target);
        let membership =
            CargoMembership::new([target.clone(), alias.clone()], temp.path(), &resolver);
        assert_eq!(membership.identity(&alias), membership.identity(&target));
        assert_eq!(membership.roots().len(), 1);
        assert!(matches!(membership.roots()[&target], CargoRoot::Unknown(_)));
        assert_eq!(resolver.calls.lock().unwrap().len(), 1);
    }
}
