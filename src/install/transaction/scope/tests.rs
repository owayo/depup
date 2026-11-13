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

/// 1 つの部分の `*` は、その位置のディレクトリにだけ一致する。ファイルと無いパスは範囲に入れず
/// (エラーにもしない)、結果は並べ替える。
#[test]
fn expand_matches_one_wildcard_per_segment_against_directories() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(
        temp.path(),
        &[
            "packages/b",
            "packages/a",
            "packages/pkg-y",
            "packages/pkg-x",
        ],
    );
    fs::write(root.join("packages/pkg-file"), "").unwrap();
    let matched = |pattern| relative(&root, expand(&root, pattern).unwrap());
    assert_eq!(
        matched("packages/*"),
        [
            "packages/a",
            "packages/b",
            "packages/pkg-x",
            "packages/pkg-y"
        ]
    );
    assert_eq!(
        matched("packages/pkg-*"),
        ["packages/pkg-x", "packages/pkg-y"]
    );
    assert_eq!(matched("packages/*-x"), ["packages/pkg-x"]);
    assert_eq!(matched("*/a"), ["packages/a"]);
    assert_eq!(matched("packages/a"), ["packages/a"]);
    assert!(matched("packages/missing").is_empty());
    assert!(matched("missing/*").is_empty());
    assert!(matched("packages/pkg-file").is_empty());
}

/// `*` は空の文字列にも一致するが、接頭辞と接尾辞が重なる短い名前には一致しない。
#[test]
fn expand_does_not_let_the_prefix_and_suffix_of_a_wildcard_overlap() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["aba", "abba"]);
    let matched = |pattern| relative(&root, expand(&root, pattern).unwrap());
    assert_eq!(matched("ab*ba"), ["abba"]);
    assert_eq!(matched("ab*a"), ["aba", "abba"]);
    assert_eq!(matched("aba*"), ["aba"]);
}

/// root の外に出うるパスと、安全に展開できない glob は、範囲を決められないエラーにする。
#[test]
fn expand_rejects_paths_that_can_leave_the_root_and_unsupported_globs() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages/a"]);
    for pattern in ["../outside", "packages/../../outside", "/absolute"] {
        assert_eq!(
            expand(&root, pattern),
            Err("Unsafe workspace path in install recovery scope".to_string()),
            "{pattern}"
        );
    }
    for pattern in [
        "packages/**",
        "packages/*a*",
        "packages/?",
        "packages/[ab]",
        "packages/{a,b}",
    ] {
        assert_eq!(
            expand(&root, pattern),
            Err("Cannot safely resolve this workspace pattern for install recovery".to_string()),
            "{pattern}"
        );
    }
}

/// 一致したディレクトリが symlink で root の外を指すときは、範囲を広げずにエラーにする。
#[cfg(unix)]
#[test]
fn expand_rejects_directories_that_resolve_outside_the_root() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages"]);
    std::os::unix::fs::symlink(outside.path(), root.join("packages/link")).unwrap();
    assert_eq!(
        expand(&root, "packages/*"),
        Err("Workspace scope resolves outside its root".to_string())
    );
}

/// `uv_root` が返すエラーの文言 (範囲を返したらテストを失敗させる)。
fn uv_root_error(directory: &Path, boundary: &Path) -> String {
    match uv_root(directory, boundary) {
        Ok(scope) => panic!(
            "expected an error, got the scope of {}",
            scope.root.display()
        ),
        Err(error) => error,
    }
}

/// 確かめた workspace の root が復元の境界より上なら、境界の中のメンバーからは範囲を決めない。
#[test]
fn uv_root_requires_the_confirmed_workspace_root_inside_the_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages/a", "packages/b"]);
    uv_workspace(&root, "members = [\"packages/*\"]\n");
    let member = root.join("packages/a");
    let scope = uv_root(&member, &root).unwrap();
    assert_eq!(scope.root, root);
    assert_eq!(
        scope.members,
        BTreeSet::from([root.clone(), member.clone(), root.join("packages/b")])
    );
    let error = uv_root_error(&member, &member);
    assert!(error.contains("outside the recovery boundary"), "{error}");
}

/// `[tool.uv.workspace]` の無い pyproject.toml は飛ばして、さらに上の workspace を探す。
/// workspace の root から始めても、同じ範囲になる。
#[test]
fn uv_root_skips_projects_without_a_workspace_table() {
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages/a"]);
    uv_workspace(&root, "members = [\"packages/*\"]\n");
    let member = root.join("packages/a");
    fs::write(
        member.join("pyproject.toml"),
        "[project]\nname = \"a\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    for directory in [&member, &root] {
        let scope = uv_root(directory, &root).unwrap();
        assert_eq!(scope.root, root);
        assert_eq!(
            scope.members,
            BTreeSet::from([root.clone(), member.clone()])
        );
    }
}

/// workspace の設定を読めないときと、members・exclude が文字列の配列でないときは、範囲を決めずにエラーにする。
#[test]
fn uv_root_rejects_workspace_settings_it_cannot_read() {
    for (workspace, expected) in [
        ("members = \"packages/*\"\n", "Invalid workspace patterns"),
        ("members = [1]\n", "Invalid workspace pattern"),
        (
            "members = [\"packages/*\"]\nexclude = \"packages/a\"\n",
            "Invalid workspace patterns",
        ),
        (
            "members = [\"../*\"]\n",
            "Unsafe workspace path in install recovery scope",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = tree(temp.path(), &["packages/a"]);
        uv_workspace(&root, workspace);
        assert_eq!(
            uv_root_error(&root.join("packages/a"), &root),
            expected,
            "{workspace}"
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let root = tree(temp.path(), &["packages/a"]);
    fs::write(root.join("pyproject.toml"), "[tool.uv.workspace\n").unwrap();
    assert_eq!(
        uv_root_error(&root.join("packages/a"), &root),
        "Cannot parse uv workspace scope"
    );
}
