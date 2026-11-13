use super::*;

/// `root` の下に `names` のディレクトリを作り、`identity` で正規化した `root` を返す。
fn tree(root: &Path, names: &[&str]) -> PathBuf {
    for name in names {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    identity(root)
}

/// `expand` の結果を、`root` からの相対パス (区切りは `/`) にする。
fn relative(root: &Path, paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .components()
                .map(|part| part.as_os_str().to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

/// `root` に uv の workspace の root の pyproject.toml を書く。
fn uv_workspace(root: &Path, workspace: &str) {
    fs::write(
        root.join("pyproject.toml"),
        format!(
            "[project]\nname = \"root\"\nversion = \"0.1.0\"\n[tool.uv.workspace]\n{workspace}"
        ),
    )
    .unwrap();
}

/// 先頭の `./` は root そのものを指すので、root からの相対パスとして展開する。
/// uv の members・exclude も mise の config_roots も `./packages/*` を root からの相対パスとして扱うが、
/// 先頭の `.` を root の外に出うる部分として断り、install の復元の範囲を決められずに止めていた。
#[test]
fn expand_treats_a_leading_current_directory_as_the_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages/a", "packages/b"]);
    assert_eq!(
        relative(&root, expand(&root, "./packages/*").unwrap()),
        ["packages/a", "packages/b"]
    );
    assert_eq!(
        relative(&root, expand(&root, "./packages/a").unwrap()),
        ["packages/a"]
    );
    // root の外に出うる `..` は今までどおり断る
    assert_eq!(
        expand(&root, "./../outside"),
        Err("Unsafe workspace path in install recovery scope".to_string())
    );
}

/// uv の workspace の members と exclude を `./` で書いても、メンバーの復元の範囲を決められる。
#[test]
fn uv_root_accepts_members_and_excludes_written_from_the_current_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages/a", "packages/b", "packages/c"]);
    uv_workspace(
        &root,
        "members = [\"./packages/*\"]\nexclude = [\"./packages/c\"]\n",
    );
    let scope = uv_root(&root.join("packages/a"), &root).unwrap();
    assert_eq!(scope.root, root);
    assert_eq!(
        scope.members,
        BTreeSet::from([
            root.clone(),
            root.join("packages/a"),
            root.join("packages/b")
        ])
    );
    // exclude で外したプロジェクトは、workspace に入らない単独のプロジェクトになる
    let excluded = root.join("packages/c");
    let scope = uv_root(&excluded, &root).unwrap();
    assert_eq!(scope.root, excluded);
    assert_eq!(scope.members, BTreeSet::from([excluded]));
}
