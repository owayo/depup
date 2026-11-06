//! 利用者に見せるパスの表記。

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};

/// Windows の `\\?\C:\...` を `C:\...` に、`\\?\UNC\server\share\...` を `\\server\share\...` に
/// 戻す (それ以外はそのまま)。
pub(crate) fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let mut plain = match prefix.kind() {
        Prefix::VerbatimDisk(disk) => PathBuf::from(format!("{}:", char::from(disk))),
        Prefix::VerbatimUNC(server, share) => {
            let mut root = OsString::from(r"\\");
            root.push(server);
            root.push(r"\");
            root.push(share);
            PathBuf::from(root)
        }
        _ => return path,
    };
    plain.push(components.as_path());
    plain
}

/// 利用者に見せる (通知・警告・報告に出す) パス。`std::fs::canonicalize` は Windows で `\\?\` 付きの
/// パス (verbatim path) を返すので、表示ではこの接頭辞を外す (#126)。ファイルの操作には元の
/// パスを使う (長いパスは `\\?\` がないと開けないことがある)。
pub(crate) fn for_display(path: &Path) -> Cow<'_, Path> {
    match path.components().next() {
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(..)
            ) =>
        {
            Cow::Owned(strip_verbatim_prefix(path.to_path_buf()))
        }
        _ => Cow::Borrowed(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn test_strip_verbatim_prefix_on_windows() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\work\ws")),
            PathBuf::from(r"C:\work\ws")
        );
        // verbatim の UNC は通常の UNC に戻す
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\ws")),
            PathBuf::from(r"\\server\share\ws")
        );
        // verbatim でないものはそのまま
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\work\ws")),
            PathBuf::from(r"C:\work\ws")
        );
    }

    /// Issue #126: 表示用のパスは `\\?\` を付けない。正規化したパスでも同じ。
    #[test]
    fn displayed_paths_have_no_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let shown = for_display(&canonical).display().to_string();
        assert!(!shown.starts_with(r"\\?\"), "{shown}");
        assert_eq!(
            std::fs::canonicalize(for_display(&canonical)).unwrap(),
            canonical
        );
        // 正規化していないパスはそのまま借りる
        assert!(matches!(for_display(dir.path()), Cow::Borrowed(_)));
    }
}
