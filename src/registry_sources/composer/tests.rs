use super::*;
use std::fs;

fn verdicts(composer: &str, packages: &[&str]) -> Vec<Verdict> {
    let sources = ComposerSources::from_manifest(Path::new("composer.json"), composer);
    packages
        .iter()
        .map(|package| sources.verdict(package))
        .collect()
}

fn non_public(origin: &str) -> Verdict {
    Verdict::NonPublic(format!("composer.json: {origin}"))
}

#[test]
fn without_repositories_or_with_packagist_itself_everything_is_public() {
    for composer in [
        r#"{"require":{"monolog/monolog":"^3.0"}}"#,
        r#"{"repositories":[]}"#,
        r#"{"repositories":[{"type":"composer","url":"https://repo.packagist.org"}]}"#,
    ] {
        assert_eq!(
            verdicts(composer, &["monolog/monolog"]),
            [Verdict::Public],
            "{composer}"
        );
    }
}

/// Issue #91: Packagist を無効にしたプロジェクトは、どの依存も Packagist の版で判定しない。
#[test]
fn disabling_packagist_excludes_every_package_in_both_forms() {
    for composer in [
        r#"{"repositories":[{"packagist.org":false}]}"#,
        r#"{"repositories":[{"packagist":false}]}"#,
        r#"{"repositories":{"packagist.org":false}}"#,
    ] {
        assert_eq!(
            verdicts(composer, &["monolog/monolog"]),
            [Verdict::NonPublic(
                "composer.json: Packagist is disabled".into()
            )],
            "{composer}"
        );
    }
}

/// 持つパッケージが分からないリポジトリ (私設の composer / vcs) は、`only` / `exclude` が
/// なければすべての依存を外す。`canonical: false` でも外す (私設の版も候補になる)。
#[test]
fn repositories_with_unknown_contents_follow_only_and_exclude() {
    assert_eq!(
        verdicts(
            r#"{"repositories":[{"type":"composer","url":"https://satis.example.com","canonical":false}]}"#,
            &["monolog/monolog"]
        ),
        [non_public("repositories[0] (composer)")]
    );
    assert_eq!(
        verdicts(
            r#"{"repositories":{"private":{"type":"vcs","url":"https://git.example.com/acme/ui.git","only":["acme/*"]}}}"#,
            &["acme/ui", "ACME/Forms", "monolog/monolog"]
        ),
        [
            non_public("repositories.private (vcs)"),
            non_public("repositories.private (vcs)"),
            Verdict::Public
        ]
    );
    assert_eq!(
        verdicts(
            r#"{"repositories":[{"type":"composer","url":"https://satis.example.com","exclude":["monolog/*"]}]}"#,
            &["acme/ui", "monolog/monolog"]
        ),
        [non_public("repositories[0] (composer)"), Verdict::Public]
    );
}

#[test]
fn package_repositories_exclude_the_named_packages() {
    let composer = r#"{"repositories":[{"type":"package","package":[
        {"name":"acme/ui","version":"1.0.0","replace":{"acme/legacy-ui":"self.version"}},
        {"name":"acme/forms","version":"1.0.0"}
    ]}]}"#;
    assert_eq!(
        verdicts(
            composer,
            &["acme/ui", "acme/legacy-ui", "acme/forms", "monolog/monolog"]
        ),
        [
            non_public("repositories[0] (package)"),
            non_public("repositories[0] (package)"),
            non_public("repositories[0] (package)"),
            Verdict::Public
        ]
    );
    assert!(matches!(
        &verdicts(
            r#"{"repositories":[{"type":"package","package":{"version":"1.0.0"}}]}"#,
            &["monolog/monolog"]
        )[0],
        Verdict::Unverified(reason) if reason.contains("without a name")
    ));
}

#[test]
fn path_repositories_exclude_the_local_package_names() {
    let temp = tempfile::tempdir().unwrap();
    for (directory, name) in [
        ("packages/ui", "acme/ui"),
        ("packages/forms", "acme/forms"),
        ("lib/core", "acme/core"),
    ] {
        let directory = temp.path().join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("composer.json"),
            format!(r#"{{"name":"{name}","provide":{{"{name}-implementation":"1.0"}}}}"#),
        )
        .unwrap();
    }
    let manifest = temp.path().join("composer.json");
    let composer = r#"{"repositories":[
        {"type":"path","url":"packages/*"},
        {"type":"path","url":"./lib/core"}
    ]}"#;
    let sources = ComposerSources::from_manifest(&manifest, composer);
    for package in [
        "acme/ui",
        "acme/forms",
        "acme/core",
        "acme/core-implementation",
    ] {
        assert!(
            matches!(sources.verdict(package), Verdict::NonPublic(_)),
            "{package}"
        );
    }
    assert_eq!(sources.verdict("monolog/monolog"), Verdict::Public);
    // 名前を確かめられない path は、どの依存も確かめられない
    let sources = ComposerSources::from_manifest(
        &manifest,
        r#"{"repositories":[{"type":"path","url":"missing"}]}"#,
    );
    assert!(matches!(
        sources.verdict("monolog/monolog"),
        Verdict::Unverified(reason) if reason.contains("could not read")
    ));
}

#[test]
fn malformed_repositories_cannot_be_verified() {
    for composer in [
        r#"{"repositories":"https://satis.example.com"}"#,
        r#"{"repositories":["https://satis.example.com"]}"#,
        "{",
    ] {
        assert!(
            matches!(
                &verdicts(composer, &["monolog/monolog"])[0],
                Verdict::Unverified(_)
            ),
            "{composer}"
        );
    }
}

#[test]
fn patterns_match_like_composer_filters() {
    assert!(glob("acme/*", "acme/ui"));
    assert!(glob("acme/ui-*", "acme/ui-forms"));
    assert!(glob("*", "monolog/monolog"));
    assert!(glob("acme/u?", "acme/ui"));
    assert!(!glob("acme/*", "acmecorp/ui"));
    assert!(!glob("acme/ui", "acme/ui-forms"));
}
