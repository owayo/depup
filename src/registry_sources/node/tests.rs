use super::*;
use std::fs;

/// `files` を置いたディレクトリで、`packages` の取得元を判定する。
fn verdicts(files: &[(&str, &str)], packages: &[&str]) -> Vec<Verdict> {
    let temp = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let path = temp.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let sources = NodeSources::load(temp.path(), temp.path());
    packages
        .iter()
        .map(|package| sources.verdict(package))
        .collect()
}

fn non_public(origin: &str) -> Verdict {
    Verdict::NonPublic(origin.into())
}

#[test]
fn without_settings_every_package_is_public() {
    assert_eq!(
        verdicts(&[], &["react", "@example/ui"]),
        [Verdict::Public, Verdict::Public]
    );
}

/// Issue #91: `.npmrc` の scope のレジストリは、その scope のパッケージだけを外す。
#[test]
fn npmrc_scope_registry_excludes_only_that_scope() {
    let npmrc = "; comment\n# comment\n@example:registry=https://npm.example.com/\n//npm.example.com/:_authToken=${NPM_TOKEN}\n";
    assert_eq!(
        verdicts(&[(".npmrc", npmrc)], &["@example/ui", "@other/ui", "react"]),
        [
            non_public(".npmrc: @example:registry"),
            Verdict::Public,
            Verdict::Public
        ]
    );
}

/// 既定のレジストリの差し替えは、scope の設定がない依存をすべて外す。公開を明示した scope は残す。
/// ミラー (registry.npmmirror.com など) も私設と区別できないので外す。
#[test]
fn npmrc_default_registry_excludes_packages_without_a_public_scope() {
    let npmrc = "registry = \"https://registry.npmmirror.com/\"\n@types:registry=https://registry.npmjs.org/\n";
    assert_eq!(
        verdicts(
            &[(".npmrc", npmrc)],
            &["react", "@types/node", "@example/ui"]
        ),
        [
            non_public(".npmrc: registry"),
            Verdict::Public,
            non_public(".npmrc: registry")
        ]
    );
    // 公開レジストリを既定に書いても外さない
    assert_eq!(
        verdicts(
            &[(".npmrc", "registry=https://registry.npmjs.org/\n")],
            &["react"]
        ),
        [Verdict::Public]
    );
}

#[test]
fn environment_variables_and_empty_values_cannot_be_verified() {
    assert!(matches!(
        &verdicts(&[(".npmrc", "registry=${NPM_REGISTRY}\n")], &["react"])[0],
        Verdict::Unverified(reason) if reason.contains("environment variable")
    ));
    assert!(matches!(
        &verdicts(&[(".npmrc", "@example:registry=\n")], &["@example/ui"])[0],
        Verdict::Unverified(reason) if reason.contains("empty")
    ));
}

#[test]
fn yarn_classic_rc_is_read() {
    let yarnrc = "# yarn 1\nregistry \"https://npm.example.com/\"\n\"@example:registry\" \"https://npm.example.com/\"\n";
    assert_eq!(
        verdicts(&[(".yarnrc", yarnrc)], &["react", "@example/ui"]),
        [
            non_public(".yarnrc: registry"),
            non_public(".yarnrc: @example:registry")
        ]
    );
}

#[test]
fn yarn_berry_rc_is_read() {
    let yarnrc = "nodeLinker: node-modules\nnpmScopes:\n  example:\n    npmAlwaysAuth: true\n    npmRegistryServer: \"https://npm.example.com\"\n  public:\n    npmRegistryServer: https://registry.yarnpkg.com\n";
    assert_eq!(
        verdicts(
            &[(".yarnrc.yml", yarnrc)],
            &["@example/ui", "@public/ui", "react"]
        ),
        [
            non_public(".yarnrc.yml: npmScopes.example.npmRegistryServer"),
            Verdict::Public,
            Verdict::Public
        ]
    );
    assert_eq!(
        verdicts(
            &[(
                ".yarnrc.yml",
                "npmRegistryServer: 'https://npm.example.com' # private\n"
            )],
            &["react"]
        ),
        [non_public(".yarnrc.yml: npmRegistryServer")]
    );
    // フロー形式は解釈しない
    assert!(matches!(
        &verdicts(
            &[(
                ".yarnrc.yml",
                "npmScopes: { example: { npmRegistryServer: x } }\n"
            )],
            &["react"]
        )[0],
        Verdict::Unverified(_)
    ));
}

#[test]
fn bunfig_registry_and_scopes_are_read_in_both_forms() {
    let bunfig = "[install]\nregistry = { url = \"https://npm.example.com/\", token = \"$TOKEN\" }\n\n[install.scopes]\n\"@example\" = \"https://npm.example.com/\"\npublic = { url = \"https://registry.npmjs.org/\" }\n";
    assert_eq!(
        verdicts(
            &[("bunfig.toml", bunfig)],
            &["react", "@example/ui", "@public/ui"]
        ),
        [
            non_public("bunfig.toml: install.registry"),
            non_public("bunfig.toml: install.scopes.@example"),
            Verdict::Public
        ]
    );
    assert!(matches!(
        &verdicts(&[("bunfig.toml", "[install\n")], &["react"])[0],
        Verdict::Unverified(reason) if reason.contains("bunfig.toml could not be parsed")
    ));
    assert!(matches!(
        &verdicts(
            &[("bunfig.toml", "[install]\nregistry = \"$BUN_REGISTRY\"\n")],
            &["react"]
        )[0],
        Verdict::Unverified(_)
    ));
}

#[test]
fn pnpm_workspace_registries_are_read() {
    let workspace = "packages:\n  - 'apps/*'\nregistries:\n  default: https://registry.npmjs.org/\n  '@example': https://npm.example.com/\n";
    assert_eq!(
        verdicts(
            &[("pnpm-workspace.yaml", workspace)],
            &["react", "@example/ui"]
        ),
        [
            Verdict::Public,
            non_public("pnpm-workspace.yaml: registries.@example")
        ]
    );
    assert_eq!(
        verdicts(
            &[(
                "pnpm-workspace.yaml",
                "registry: https://npm.example.com/\n"
            )],
            &["react"]
        ),
        [non_public("pnpm-workspace.yaml: registry")]
    );
}

/// workspace のルート (実行のルート) の設定は、メンバーのマニフェストにも効く。
/// 実行のルートより上 (ここではテンポラリのディレクトリの外) は読まない。
#[test]
fn settings_from_the_member_up_to_the_root_apply() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let member = root.join("packages/app");
    fs::create_dir_all(&member).unwrap();
    fs::write(
        temp.path().join(".npmrc"),
        "registry=https://outside.example.com/\n",
    )
    .unwrap();
    fs::write(
        root.join(".npmrc"),
        "@example:registry=https://npm.example.com/\n",
    )
    .unwrap();
    let sources = NodeSources::load(&member, &root);
    assert_eq!(
        sources.verdict("@example/ui"),
        non_public(".npmrc: @example:registry")
    );
    assert_eq!(sources.verdict("react"), Verdict::Public);
    // メンバーの設定も読む (どちらかが私設なら外す)
    fs::write(member.join(".npmrc"), "registry=https://npm.example.com/\n").unwrap();
    let sources = NodeSources::load(&member, &root);
    assert_eq!(sources.verdict("react"), non_public(".npmrc: registry"));
}

/// リポジトリのルート (`.git` のあるディレクトリ) までを読み、その上は読まない。
#[test]
fn settings_stop_at_the_repository_root() {
    let temp = tempfile::tempdir().unwrap();
    let repository = temp.path().join("repo");
    let member = repository.join("packages/app");
    fs::create_dir_all(&member).unwrap();
    fs::create_dir(repository.join(".git")).unwrap();
    fs::write(
        temp.path().join(".npmrc"),
        "registry=https://outside.example.com/\n",
    )
    .unwrap();
    fs::write(
        repository.join(".npmrc"),
        "@example:registry=https://npm.example.com/\n",
    )
    .unwrap();
    // 実行のルートがリポジトリより上でも、リポジトリの外の設定は読まない
    let sources = NodeSources::load(&member, temp.path());
    assert_eq!(sources.verdict("react"), Verdict::Public);
    assert_eq!(
        sources.verdict("@example/ui"),
        non_public(".npmrc: @example:registry")
    );
}

#[test]
fn an_unreadable_setting_file_leaves_every_package_unverified() {
    let temp = tempfile::tempdir().unwrap();
    // ディレクトリはファイルとして読めない
    fs::create_dir(temp.path().join(".npmrc")).unwrap();
    let sources = NodeSources::load(temp.path(), temp.path());
    assert!(matches!(
        sources.verdict("react"),
        Verdict::Unverified(reason) if reason.contains(".npmrc could not be read")
    ));
}
