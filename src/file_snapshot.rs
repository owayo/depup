//! File identity and checked atomic writes shared by the writer and install recovery.
use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub(crate) struct FileGuard {
    pub alias: PathBuf,
    pub target: PathBuf,
    links: Vec<(PathBuf, Option<PathBuf>)>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileState {
    pub bytes: Option<Vec<u8>>,
    pub permissions: Option<Permissions>,
}

impl FileState {
    pub fn read(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => Ok(Self {
                bytes: Some(bytes),
                permissions: Some(fs::metadata(path)?.permissions()),
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A dangling link is not an absent file.
                match fs::symlink_metadata(path) {
                    Ok(_) => return Err(error),
                    Err(missing) if missing.kind() == io::ErrorKind::NotFound => {}
                    Err(other) => return Err(other),
                }
                Ok(Self {
                    bytes: None,
                    permissions: None,
                })
            }
            Err(error) => Err(error),
        }
    }
    pub fn matches(&self, other: &Self) -> bool {
        self.bytes == other.bytes
            && permissions_equal(self.permissions.as_ref(), other.permissions.as_ref())
    }
}

fn permissions_equal(a: Option<&Permissions>, b: Option<&Permissions>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                a.mode() == b.mode()
            }
            #[cfg(not(unix))]
            {
                a.readonly() == b.readonly()
            }
        }
        _ => false,
    }
}

fn link_at(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(path).map(Some),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn link_paths(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let mut prefix = PathBuf::new();
    path.components().filter_map(move |part| {
        prefix.push(part);
        // A Windows Prefix is incomplete until its root separator is appended.
        (!matches!(part, Component::Prefix(_))).then(|| prefix.clone())
    })
}

pub(crate) fn resolved_path(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::symlink_metadata(path) {
                Ok(_) => return Err(error),
                Err(missing) if missing.kind() == io::ErrorKind::NotFound => {}
                Err(other) => return Err(other),
            }
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("File has no parent"))?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("File has no name"))?;
            Ok(resolved_path(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}

impl FileGuard {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let alias = std::path::absolute(path)?;
        let target = resolved_path(&alias)?;
        let mut links = Vec::new();
        for prefix in link_paths(&alias) {
            links.push((prefix.clone(), link_at(&prefix)?));
        }
        Ok(Self {
            alias,
            target,
            links,
        })
    }
    pub fn check(&self) -> io::Result<()> {
        for (path, expected) in &self.links {
            if &link_at(path)? != expected {
                return Err(io::Error::other("File or parent symlink changed"));
            }
        }
        if resolved_path(&self.alias)? != self.target {
            return Err(io::Error::other("File target changed"));
        }
        Ok(())
    }
    pub fn read(&self) -> io::Result<FileState> {
        self.check()?;
        FileState::read(&self.target)
    }
}

/// The guard and expected state are rechecked immediately before the rename.
pub(crate) fn write_checked(
    guard: &FileGuard,
    expected: &FileState,
    bytes: &[u8],
    permissions: Option<&Permissions>,
) -> io::Result<()> {
    guard.check()?;
    if !expected.matches(&FileState::read(&guard.target)?) {
        return Err(io::Error::other("File changed before writing"));
    }
    if expected.bytes.is_some() {
        fs::OpenOptions::new().write(true).open(&guard.target)?;
    }
    let parent = guard
        .target
        .parent()
        .ok_or_else(|| io::Error::other("File has no parent"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = parent.join(format!(".depup-tmp-{}-{nonce}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions.clone())?;
        }
        file.sync_all()?;
        guard.check()?;
        if !expected.matches(&FileState::read(&guard.target)?) {
            return Err(io::Error::other("File changed before replacement"));
        }
        fs::rename(&tmp, &guard.target)?;
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_paths_observe_complete_root_and_each_descendant() {
        let alias = std::path::absolute("project/member/Cargo.lock").unwrap();
        let paths: Vec<_> = link_paths(&alias).collect();
        let root = alias.ancestors().last().unwrap();
        assert_eq!(paths.first().unwrap(), root);
        assert_eq!(paths.last().unwrap(), &alias);
        assert!(paths.iter().all(|path| path.is_absolute()));
        assert!(
            paths
                .windows(2)
                .all(|pair| pair[1].parent() == Some(pair[0].as_path()))
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_link_paths_preserve_drive_verbatim_and_unc_roots() {
        for root in [
            r"C:\",
            r"\\?\C:\",
            r"\\server\share\",
            r"\\?\UNC\server\share\",
        ] {
            let root = Path::new(root);
            let directory = root.join("project");
            let file = directory.join("Cargo.lock");
            assert_eq!(
                link_paths(&file).collect::<Vec<_>>(),
                vec![root.to_path_buf(), directory, file]
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_guard_captures_canonical_existing_and_absent_locks() {
        for exists in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let directory = fs::canonicalize(temp.path()).unwrap();
            assert!(matches!(
                directory.components().next(),
                Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim()
            ));
            let path = directory.join("Cargo.lock");
            if exists {
                fs::write(&path, "original lock").unwrap();
            }
            let guard = FileGuard::capture(&path).unwrap();
            assert_eq!(guard.alias, path);
            assert_eq!(guard.target, path);
            assert!(guard.links.iter().all(|(path, _)| path.is_absolute()));
            guard.check().unwrap();
            assert_eq!(
                guard.read().unwrap().bytes,
                exists.then(|| b"original lock".to_vec())
            );
            fs::write(&path, "new lock").unwrap();
            guard.check().unwrap();
            assert_eq!(guard.read().unwrap().bytes, Some(b"new lock".to_vec()));
        }
    }
}
