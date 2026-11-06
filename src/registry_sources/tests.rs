use super::*;

#[test]
fn only_the_public_npm_hosts_are_public() {
    for url in [
        "https://registry.npmjs.org/",
        "https://registry.npmjs.org",
        "http://registry.npmjs.org/",
        "https://registry.yarnpkg.com/",
    ] {
        assert!(is_public_npm_registry(url), "{url}");
    }
    // 似たホスト・ポート・認証情報・サブパス (ミラーや私設の入口) は公開とみなさない
    for url in [
        "https://registry.npmjs.org.example.com/",
        "https://npm.example.com/",
        "https://registry.npmmirror.com/",
        "https://registry.npmjs.org:8443/",
        "https://user@registry.npmjs.org/",
        "https://registry.npmjs.org/mirror/",
        "https://registry.npmjs.org/?token=x",
        "registry.npmjs.org",
        "",
    ] {
        assert!(!is_public_npm_registry(url), "{url}");
    }
}

#[test]
fn only_packagist_itself_is_public() {
    for url in [
        "https://repo.packagist.org",
        "https://repo.packagist.org/",
        "https://packagist.org",
    ] {
        assert!(is_packagist(url), "{url}");
    }
    for url in [
        "https://repo.packagist.com/acme/",
        "https://packagist.org.example.com",
        "https://satis.example.com",
        "https://packagist.org/acme",
    ] {
        assert!(!is_packagist(url), "{url}");
    }
}

#[test]
fn verdicts_become_skip_reasons_except_public() {
    assert_eq!(Verdict::Public.skip_reason(), None);
    assert_eq!(
        Verdict::NonPublic(".npmrc: registry".into()).skip_reason(),
        Some(SkipReason::NonPublicSource(".npmrc: registry".into()))
    );
    assert_eq!(
        Verdict::Unverified("bunfig.toml could not be parsed".into()).skip_reason(),
        Some(SkipReason::SourceUnverified(
            "bunfig.toml could not be parsed".into()
        ))
    );
}
