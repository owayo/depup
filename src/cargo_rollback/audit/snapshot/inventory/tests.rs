use super::*;

fn chars(pattern: &str) -> Vec<char> {
    pattern.chars().collect()
}

/// `members` に `patterns` を持つ workspace のルート
fn workspace(patterns: &[&str]) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let members: Vec<String> = patterns
        .iter()
        .map(|pattern| format!("'{pattern}'"))
        .collect();
    fs::write(
        temp.path().join("Cargo.toml"),
        format!("[workspace]\nmembers = [{}]\n", members.join(", ")),
    )
    .unwrap();
    temp
}

fn member(root: &Path, relative: &str) {
    let directory = root.join(relative);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = 'member'\nversion = '0.1.0'\n",
    )
    .unwrap();
}

/// 集合のメンバーのディレクトリを、ルートからの相対パス (区切りは `/`) で並べる。
fn listed(inventory: &Inventory, root: &Path) -> Vec<String> {
    let root = fs::canonicalize(root).unwrap();
    inventory
        .iter()
        .map(|(path, _)| {
            path.parent()
                .unwrap()
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn components_match_like_the_glob_crate() {
    let cases = [
        ("crate-*", "crate-a", true),
        ("crate-*", "crate-", true),
        ("crate-*", "crate", false),
        ("crate-*", "my-crate-a", false),
        // glob crate の既定では、先頭のドットにも `*` が一致する
        ("*", ".git", true),
        ("a*b*c", "aXbYbZc", true),
        ("a*b*c", "aXbYbZ", false),
        ("?", "a", true),
        ("?", "ab", false),
        ("?", "é", true),
        ("[ab]x", "bx", true),
        ("[ab]x", "cx", false),
        ("[!ab]x", "cx", true),
        ("[!ab]x", "ax", false),
        ("v[0-9]", "v7", true),
        ("v[0-9]", "va", false),
        ("[]]", "]", true),
        ("[!]]", "a", true),
        ("[!]]", "]", false),
        ("[a-]", "-", true),
        ("[-a]", "-", true),
        ("[?]", "?", true),
        ("[?]", "a", false),
        // 大文字と小文字を区別する
        ("Crate-*", "crate-a", false),
    ];
    for (pattern, name, expected) in cases {
        assert_eq!(
            matches(&chars(pattern), name),
            expected,
            "{pattern} vs {name}"
        );
    }
}

#[test]
fn invalid_patterns_are_rejected_and_braces_are_literal() {
    assert!(classify("a**".into()).is_err());
    assert!(classify("[ab".into()).is_err());
    assert!(matches!(classify("{a,b}".into()), Ok(Part::Literal(_))));
    assert!(matches!(classify("**".into()), Ok(Part::Recursive)));
    assert!(matches!(classify("crate-*".into()), Ok(Part::Pattern(_))));
    assert!(parse("../sibling").is_err());
}

/// Issue #90: glob が最初の要素にあっても、パターンに一致しえない `target/` などは読まない。
/// 以前は接頭辞 (ここではルート) の配下をすべて辿り、メンバーでない Cargo.toml も集め、
/// 大きな `target/` では上限を超えて監査が失敗していた。
#[test]
fn unrelated_trees_below_a_leading_glob_are_not_read() {
    let temp = workspace(&["crate-*"]);
    member(temp.path(), "crate-a");
    member(temp.path(), "crate-b");
    for index in 0..50 {
        fs::create_dir_all(temp.path().join(format!("target/debug/build/dep-{index}"))).unwrap();
    }
    member(temp.path(), "target/package/crate-a-0.1.0");
    fs::create_dir_all(temp.path().join(".git/objects")).unwrap();
    let inventory = inventory_within(temp.path(), 10).unwrap();
    assert_eq!(listed(&inventory, temp.path()), ["crate-a", "crate-b"]);
}

/// Issue #90: Tauri の複数アプリの構成で、`apps/*/node_modules` の中は読まない。
#[test]
fn only_the_matched_paths_below_a_glob_are_followed() {
    let temp = workspace(&["apps/*/src-tauri"]);
    member(temp.path(), "apps/desktop/src-tauri");
    member(temp.path(), "apps/mobile/src-tauri");
    for index in 0..50 {
        member(
            temp.path(),
            &format!("apps/desktop/node_modules/pkg-{index}"),
        );
    }
    fs::write(temp.path().join("apps/README.md"), "").unwrap();
    let inventory = inventory_within(temp.path(), 10).unwrap();
    assert_eq!(
        listed(&inventory, temp.path()),
        ["apps/desktop/src-tauri", "apps/mobile/src-tauri"]
    );
}

/// Issue #90: パターンに一致しない、プロジェクトの外を指すジャンクションは辿らない。
#[cfg(windows)]
#[test]
fn an_outside_junction_that_cannot_match_does_not_fail() {
    let temp = workspace(&["crate-*"]);
    member(temp.path(), "crate-core");
    let outside = tempfile::tempdir().unwrap();
    member(outside.path(), "lib");
    // ジャンクションは symlink と違い、作るのに特別な権限が要らない。
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(temp.path().join("vendor"))
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let inventory = inventory_within(temp.path(), 100).unwrap();
    assert_eq!(listed(&inventory, temp.path()), ["crate-core"]);
}

/// Issue #90: パターンに一致しない、読めないディレクトリやプロジェクトの外を指す symlink では
/// 失敗しない。
#[cfg(unix)]
#[test]
fn unreadable_or_outside_entries_that_cannot_match_do_not_fail() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = workspace(&["crate-*"]);
    member(temp.path(), "crate-core");
    let outside = tempfile::tempdir().unwrap();
    member(outside.path(), "lib");
    let secret = temp.path().join("secret");
    fs::create_dir(&secret).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    symlink(outside.path(), temp.path().join("vendor")).unwrap();
    let inventory = inventory_within(temp.path(), 100);
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(listed(&inventory.unwrap(), temp.path()), ["crate-core"]);
}

/// 照合に要るディレクトリを読めなければ、欠けた集合で成功させない (Cargo も失敗する)。
#[cfg(unix)]
#[test]
fn an_unreadable_directory_that_the_pattern_needs_fails() {
    use std::os::unix::fs::PermissionsExt;
    let temp = workspace(&["crates/*"]);
    member(temp.path(), "crates/core");
    let crates = temp.path().join("crates");
    fs::set_permissions(&crates, fs::Permissions::from_mode(0o000)).unwrap();
    // root などで権限が効かない環境では確かめられない。
    let readable = fs::read_dir(&crates).is_ok();
    let inventory = inventory_within(temp.path(), 100);
    fs::set_permissions(&crates, fs::Permissions::from_mode(0o700)).unwrap();
    if !readable {
        let error = inventory.unwrap_err();
        assert!(error.contains("could not read"), "{error}");
    }
}

#[test]
fn exceeding_the_entry_limit_on_a_needed_directory_fails() {
    let temp = workspace(&["crates/*"]);
    for index in 0..20 {
        member(temp.path(), &format!("crates/c{index}"));
    }
    let error = inventory_within(temp.path(), 10).unwrap_err();
    assert!(error.contains("exceeded 10 entries"), "{error}");
    assert_eq!(inventory_within(temp.path(), 100).unwrap().len(), 20);
}

#[test]
fn recursive_wildcards_match_zero_or_more_directories() {
    let temp = workspace(&["member/**/lib", "tools/**"]);
    member(temp.path(), "member/lib");
    member(temp.path(), "member/a/lib");
    member(temp.path(), "member/a/b/lib");
    member(temp.path(), "member/a/b/other");
    member(temp.path(), "tools");
    member(temp.path(), "tools/x/y");
    let inventory = inventory_within(temp.path(), 100).unwrap();
    assert_eq!(
        listed(&inventory, temp.path()),
        [
            "member/a/b/lib",
            "member/a/lib",
            "member/lib",
            "tools",
            "tools/x/y"
        ]
    );
}

#[test]
fn consecutive_wildcards_and_overlapping_patterns_list_each_member_once() {
    let temp = workspace(&["crates/**/**", "crates/*"]);
    member(temp.path(), "crates/a");
    member(temp.path(), "crates/a/nested");
    let inventory = inventory_within(temp.path(), 100).unwrap();
    assert_eq!(
        listed(&inventory, temp.path()),
        ["crates/a", "crates/a/nested"]
    );
}

/// glob のないメンバーも入れる。何にも一致しないパターンは、Cargo と同じく書かれたパスで調べる。
#[test]
fn literal_members_and_patterns_without_matches_use_the_written_path() {
    let temp = workspace(&["app", "missing", "crate-[x]"]);
    member(temp.path(), "app");
    member(temp.path(), "crate-[x]");
    let inventory = inventory_within(temp.path(), 100).unwrap();
    assert_eq!(listed(&inventory, temp.path()), ["app", "crate-[x]"]);
    // パターンに一致するディレクトリができると、書かれたパスは使わない。
    member(temp.path(), "crate-x");
    let inventory = inventory_within(temp.path(), 100).unwrap();
    assert_eq!(listed(&inventory, temp.path()), ["app", "crate-x"]);
}

/// 同じ実体への別名が増えても、集合が変わったと分かる。
#[cfg(unix)]
#[test]
fn an_alias_of_an_existing_member_changes_the_inventory() {
    use std::os::unix::fs::symlink;
    let temp = workspace(&["crates/*"]);
    member(temp.path(), "crates/a");
    let before = inventory_within(temp.path(), 100).unwrap();
    symlink(temp.path().join("crates/a"), temp.path().join("crates/b")).unwrap();
    let after = inventory_within(temp.path(), 100).unwrap();
    assert_eq!(listed(&after, temp.path()), ["crates/a", "crates/b"]);
    assert_eq!(after[0].1, after[1].1);
    assert_ne!(before, after);
}

#[cfg(unix)]
#[test]
fn a_symlink_cycle_under_a_recursive_wildcard_terminates() {
    use std::os::unix::fs::symlink;
    let temp = workspace(&["crates/**"]);
    member(temp.path(), "crates/a");
    symlink(
        temp.path().join("crates"),
        temp.path().join("crates/a/loop"),
    )
    .unwrap();
    let inventory = inventory_within(temp.path(), 1000).unwrap();
    assert_eq!(listed(&inventory, temp.path()), ["crates/a"]);
}
