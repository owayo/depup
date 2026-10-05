use super::*;
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
struct FakeDates {
    dates: HashMap<String, PublicationDates>,
    requests: Mutex<Vec<String>>,
}

#[async_trait]
impl PublicationProvider for FakeDates {
    async fn fetch_dates(&self, package: &str) -> Result<PublicationDates, String> {
        self.requests.lock().unwrap().push(package.into());
        self.dates
            .get(package)
            .cloned()
            .ok_or_else(|| "registry unavailable".into())
    }
}

fn policy() -> AgePolicy {
    AgePolicy {
        min_age: Some(Duration::from_secs(14 * 86400)),
        evaluated_at: "2026-10-05T00:00:00Z".parse().unwrap(),
        exemptions: Default::default(),
    }
}

fn dates(entries: &[(&str, &str)]) -> PublicationDates {
    entries
        .iter()
        .map(|(version, date)| (version.to_string(), date.parse().unwrap()))
        .collect()
}

fn package(name: &str, version: &str) -> Value {
    let basename = name.rsplit('/').next().unwrap();
    json!({
        "from": name,
        "version": version,
        "resolved": format!("https://registry.npmjs.org/{name}/-/{basename}-{version}.tgz")
    })
}

async fn check(snapshot: LockSnapshot, provider: &impl PublicationProvider) -> NodeLockAuditResult {
    audit_snapshot(
        snapshot,
        policy().cutoff().unwrap(),
        provider,
        Instant::now() + Duration::from_secs(1),
        None,
    )
    .await
}

#[tokio::test]
async fn pnpm_snapshot_checks_direct_and_transitive_releases() {
    let mut direct = package("example-parent", "1.0.0");
    direct["dependencies"] = json!({"example-child": package("example-child", "1.0.1")});
    let snapshot = parse_pnpm_snapshot(&json!([{
        "name": "example-app", "dependencies": {"example-parent": direct}
    }]))
    .unwrap();
    let provider = FakeDates {
        dates: HashMap::from([
            (
                "example-parent".into(),
                dates(&[("1.0.0", "2026-09-01T00:00:00Z")]),
            ),
            (
                "example-child".into(),
                dates(&[("1.0.1", "2026-09-30T00:00:00Z")]),
            ),
        ]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert_eq!(result.checked, 2);
    assert!(result.has_failures());
    assert_eq!(result.violations.len(), 1);
    assert_eq!(result.violations[0].name, "example-child");
    assert_eq!(result.violations[0].version, "1.0.1");
    assert!(result.failure_messages()[0].contains("2026-09-30"));
}

#[tokio::test]
async fn pnpm_deep_graph_preserves_alias_and_optional_releases() {
    let mut chain = package("chain-15", "1.0.0");
    chain["optionalDependencies"] = json!({"optional-deep": package("optional-deep", "1.0.0")});
    for index in (1..15).rev() {
        let mut parent = package(&format!("chain-{index}"), "1.0.0");
        parent["dependencies"] = json!({format!("chain-{}", index + 1): chain});
        chain = parent;
    }
    let graph = json!([{
        "dependencies": {"chain-1": chain, "alias": package("@example/real", "1.0.0")},
        "optionalDependencies": {"optional-root": package("optional-root", "1.0.0")}
    }]);
    let snapshot = parse_pnpm_snapshot(&graph).unwrap();
    assert_eq!(snapshot.packages.len(), 18);
    assert!(snapshot.unverified.is_empty());
    let mut provider = FakeDates::default();
    for (name, version) in &snapshot.packages {
        provider
            .dates
            .insert(name.clone(), dates(&[(version, "2026-09-01T00:00:00Z")]));
    }
    for name in ["optional-deep", "optional-root", "@example/real"] {
        provider
            .dates
            .insert(name.into(), dates(&[("1.0.0", "2026-09-30T00:00:00Z")]));
    }
    let result = check(snapshot, &provider).await;
    assert_eq!(result.checked, 18);
    assert_eq!(result.violations.len(), 3);
    assert_eq!(
        result
            .violations
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        ["@example/real", "optional-deep", "optional-root"]
    );
    assert!(
        !provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "alias")
    );
}

#[tokio::test]
async fn npm_aliases_use_tarball_identity_and_deduplicate_fetches() {
    let snapshot = parse_npm_snapshot(&json!({
        "lockfileVersion": 3,
        "packages": {
            "": {"name": "example-app", "version": "1.0.0"},
            "node_modules/alias": package("@example/actual", "1.0.0"),
            "node_modules/parent/node_modules/alias": package("@example/actual", "1.0.0"),
            "node_modules/another": package("@example/actual", "1.1.0")
        }
    }))
    .unwrap();
    let provider = FakeDates {
        dates: HashMap::from([(
            "@example/actual".into(),
            dates(&[
                ("1.0.0", "2026-09-01T00:00:00Z"),
                ("1.1.0", "2026-09-22T00:00:00Z"),
            ]),
        )]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert_eq!(*provider.requests.lock().unwrap(), ["@example/actual"]);
    assert_eq!(result.checked, 2);
    assert_eq!(result.violations[0].version, "1.1.0");
}

#[tokio::test]
async fn private_and_unknown_sources_are_never_queried_on_public_npm() {
    let snapshot = parse_pnpm_snapshot(&json!([{
        "dependencies": {
            "example-private": {"from": "example-private", "version": "1.0.0",
                "resolved": "https://registry.example.com/example-private/-/example-private-1.0.0.tgz"},
            "unknown": {"version": "1.0.0"},
            "local": {"version": "link:../local", "resolved": "link:../local"},
            "git": {"version": "git+https://example.com/library.git#abc",
                "resolved": "git+https://example.com/library.git#abc"}
        }
    }]))
    .unwrap();
    let provider = FakeDates::default();
    let result = check(snapshot, &provider).await;
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(result.unverified.len(), 2);
    assert!(result.has_failures());
}

#[tokio::test]
async fn missing_dates_and_fetch_errors_are_failures() {
    let snapshot = parse_pnpm_snapshot(&json!([{"dependencies": {
        "no-date": package("no-date", "1.0.0"),
        "unavailable": package("unavailable", "1.0.0")
    }}]))
    .unwrap();
    let provider = FakeDates {
        dates: HashMap::from([("no-date".into(), PublicationDates::new())]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert!(result.has_failures());
    assert_eq!(result.checked, 0);
    assert_eq!(result.unverified.len(), 2);
    assert!(
        result
            .failure_messages()
            .iter()
            .any(|line| line.contains("registry unavailable"))
    );
}

#[tokio::test]
async fn unchanged_old_versions_and_cutoff_boundary_are_safe() {
    let snapshot = parse_pnpm_snapshot(&json!([{"dependencies": {
        "boundary": package("boundary", "1.0.0")
    }}]))
    .unwrap();
    let provider = FakeDates {
        dates: HashMap::from([(
            "boundary".into(),
            dates(&[("1.0.0", "2026-09-21T00:00:00Z")]),
        )]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert_eq!(result.checked, 1);
    assert!(!result.has_failures());
}

#[tokio::test]
async fn node_age_disabled_never_reads_a_lock() {
    let policy = AgePolicy {
        min_age: None,
        ..policy()
    };
    let result = audit_node_lock(Path::new("missing-project"), "pnpm", &policy, None).await;
    assert_eq!(result.checked, 0);
    assert!(!result.has_failures());
}

#[tokio::test]
async fn absent_invalid_and_unsupported_locks_report_failure_without_fetching() {
    let directory = tempfile::tempdir().unwrap();
    let absent = audit_node_lock(directory.path(), "npm", &policy(), None).await;
    assert!(absent.has_failures());
    assert_eq!(absent.checked, 0);
    std::fs::write(directory.path().join("package-lock.json"), "invalid JSON").unwrap();
    let invalid = audit_node_lock(directory.path(), "npm", &policy(), None).await;
    assert!(invalid.has_failures());
    assert!(invalid.errors[0].contains("invalid package-lock.json"));
    for manager in ["yarn", "bun"] {
        let result = audit_node_lock(directory.path(), manager, &policy(), None).await;
        assert!(result.has_failures());
        assert!(result.errors[0].contains("not supported"));
    }
}

#[test]
fn npm_v2_links_are_skipped_but_registry_packages_remain_auditable() {
    let snapshot = parse_npm_snapshot(&json!({
        "lockfileVersion": 2,
        "packages": {
            "": {"name": "example-app"},
            "packages/local": {"name": "local", "version": "1.0.0"},
            "node_modules/local": {"link": true, "resolved": "packages/local"},
            "node_modules/public": package("public", "1.0.0")
        }
    }))
    .unwrap();
    assert_eq!(snapshot.packages.len(), 1);
    assert!(snapshot.unverified.is_empty());
}

#[tokio::test]
async fn npm_empty_stale_lock_is_unverified_without_network_requests() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("package.json"),
        r#"{"dependencies":{"required":"1.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(
        directory.path().join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"":{}}}"#,
    )
    .unwrap();
    let result = audit_node_lock(directory.path(), "npm", &policy(), None).await;
    assert!(result.has_failures());
    assert!(result.errors[0].contains("required"));
    assert_eq!(result.checked, 0);
}

#[tokio::test]
async fn npm_shrinkwrap_takes_priority_over_package_lock() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("package.json"), "{}").unwrap();
    std::fs::write(
        directory.path().join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"":{}}}"#,
    )
    .unwrap();
    std::fs::write(directory.path().join("npm-shrinkwrap.json"), "invalid JSON").unwrap();
    let result = audit_node_lock(directory.path(), "npm", &policy(), None).await;
    assert!(result.has_failures());
    assert!(result.errors[0].contains("invalid npm-shrinkwrap.json"));
    assert_eq!(result.checked, 0);
    let shrinkwrap = json!({"lockfileVersion":2,"packages":{"":{},"node_modules/young":package("young","1.0.0")}});
    std::fs::write(
        directory.path().join("npm-shrinkwrap.json"),
        shrinkwrap.to_string(),
    )
    .unwrap();
    assert_eq!(npm_lock_name(directory.path()), "npm-shrinkwrap.json");
    let snapshot = parse_npm_snapshot(&shrinkwrap).unwrap();
    let provider = FakeDates {
        dates: HashMap::from([("young".into(), dates(&[("1.0.0", "2026-09-30T00:00:00Z")]))]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert_eq!(result.violations.len(), 1);
}

#[tokio::test]
async fn npm_missing_transitive_dependency_is_not_a_success() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("package.json"),
        r#"{"dependencies":{"parent":"1.0.0"}}"#,
    )
    .unwrap();
    let mut parent = package("parent", "1.0.0");
    parent["dependencies"] = json!({"missing-child":"^1.0.0"});
    let lock = json!({"lockfileVersion":3,"packages":{"":{},"node_modules/parent":parent}});
    std::fs::write(directory.path().join("package-lock.json"), lock.to_string()).unwrap();
    let result = audit_node_lock(directory.path(), "npm", &policy(), None).await;
    assert!(result.has_failures());
    assert_eq!(result.checked, 0);
    assert!(result.errors[0].contains("missing-child"));
}

#[test]
fn npm_required_dependencies_resolve_hoisting_workspaces_and_aliases() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("package.json"),
        r#"{"dependencies":{"parent":"^1.0.0","member":"file:packages/member"}}"#,
    )
    .unwrap();
    let mut parent = package("parent", "1.2.0");
    parent["dependencies"] = json!({"child":"~1.2.0","alias":"npm:@example/actual@^1.0.0","alias-latest":"npm:@example/actual","optional":"1.0.0"});
    parent["optionalDependencies"] = json!({"optional":"1.0.0"});
    parent["devDependencies"] = json!({"not-installed":"1.0.0"});
    let lock = json!({"packages":{
        "":{}, "node_modules/parent":parent,
        "node_modules/child":package("child","1.2.3"),
        "node_modules/alias":package("@example/actual","1.1.0"),
        "node_modules/alias-latest":package("@example/actual","1.1.0"),
        "node_modules/member":{"link":true,"resolved":"packages/member"},
        "packages/member":{"name":"member","version":"1.0.0","devDependencies":{"dev":"1.0.0"}},
        "node_modules/dev":package("dev","1.0.0")
    }});
    verify_npm_manifest_dependencies(directory.path(), &lock).unwrap();
    let mut missing_dev = lock.clone();
    missing_dev["packages"]
        .as_object_mut()
        .unwrap()
        .remove("node_modules/dev");
    assert!(
        verify_npm_manifest_dependencies(directory.path(), &missing_dev)
            .unwrap_err()
            .contains("dev")
    );
}

#[tokio::test]
async fn npm_overrides_alias_replacements_and_empty_or_audit_actual_lock_versions() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = json!({
        "dependencies":{"parent":"1.0.0","empty-or":"1.0.0 ||"},
        "overrides":{"child":"2.0.0","original":"npm:replacement@2.0.0"}
    });
    std::fs::write(directory.path().join("package.json"), manifest.to_string()).unwrap();
    let mut parent = package("parent", "1.0.0");
    parent["dependencies"] = json!({"child":"^1.0.0","original":"^1.0.0"});
    let lock = json!({"lockfileVersion":3,"packages":{
        "":manifest,
        "node_modules/parent":parent,
        "node_modules/child":package("child","2.0.0"),
        "node_modules/original":package("replacement","2.0.0"),
        "node_modules/empty-or":package("empty-or","2.0.0")
    }});
    verify_npm_manifest_dependencies(directory.path(), &lock).unwrap();
    let snapshot = parse_npm_snapshot(&lock).unwrap();
    let provider = FakeDates {
        dates: HashMap::from([
            ("parent".into(), dates(&[("1.0.0", "2026-09-01T00:00:00Z")])),
            ("child".into(), dates(&[("2.0.0", "2026-09-30T00:00:00Z")])),
            (
                "replacement".into(),
                dates(&[("2.0.0", "2026-09-30T00:00:00Z")]),
            ),
            (
                "empty-or".into(),
                dates(&[("2.0.0", "2026-09-30T00:00:00Z")]),
            ),
        ]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert_eq!(result.checked, 4);
    assert_eq!(result.violations.len(), 3);
    assert!(result.unverified.is_empty());
    assert_eq!(
        result
            .violations
            .iter()
            .map(|item| (item.name.as_str(), item.version.as_str()))
            .collect::<Vec<_>>(),
        [
            ("child", "2.0.0"),
            ("empty-or", "2.0.0"),
            ("replacement", "2.0.0")
        ]
    );
    let mut requests = provider.requests.lock().unwrap().clone();
    requests.sort();
    assert_eq!(requests, ["child", "empty-or", "parent", "replacement"]);
}

#[test]
fn malformed_and_unsupported_lockfiles_are_not_empty_successes() {
    assert!(parse_npm_snapshot(&json!({"lockfileVersion": 1, "dependencies": {}})).is_err());
    assert!(parse_npm_snapshot(&json!({"lockfileVersion": 3})).is_err());
    assert!(parse_npm_snapshot(&json!({"lockfileVersion": 3,"packages":{}})).is_err());
    assert!(parse_pnpm_snapshot(&json!({})).is_err());
    assert!(parse_pnpm_snapshot(&json!([])).is_err());
    assert!(parse_pnpm_snapshot(&json!([{"dependencies": "not-an-object"}])).is_err());
}

#[test]
fn pnpm_workspace_graphs_cover_root_members_and_all_dependency_sections() {
    let snapshot = parse_pnpm_snapshot(&json!([
        {"dependencies": {"root": package("root", "1.0.0")}},
        {"devDependencies": {"dev": package("dev", "1.0.0")},
         "optionalDependencies": {"optional": package("optional", "1.0.0")}}
    ]))
    .unwrap();
    assert_eq!(snapshot.packages.len(), 3);
}

#[test]
fn public_registry_source_requires_an_exact_host_and_package_tarball() {
    assert_eq!(
        public_tarball_package(
            "https://registry.npmjs.org/@example/pkg/-/pkg-1.0.0.tgz",
            "1.0.0"
        ),
        Some("@example/pkg".into())
    );
    for source in [
        "https://registry.npmjs.org.example.com/pkg/-/pkg-1.0.0.tgz",
        "https://registry.npmjs.org:8443/pkg/-/pkg-1.0.0.tgz",
        "https://registry.npmjs.org/pkg/-/other-1.0.0.tgz",
        "https://registry.npmjs.org/pkg/-/pkg-2.0.0.tgz",
    ] {
        assert!(
            public_tarball_package(source, "1.0.0").is_none(),
            "{source}"
        );
    }
    let mut source = reqwest::Url::parse("https://registry.npmjs.org/pkg/-/pkg-1.0.0.tgz").unwrap();
    source.set_username("example").unwrap();
    source.set_password(Some("placeholder")).unwrap();
    assert!(public_tarball_package(source.as_str(), "1.0.0").is_none());
}

#[test]
fn pnpm_errors_redact_credentials_in_registry_urls() {
    let mut source = reqwest::Url::parse("https://registry.example.com/pkg").unwrap();
    source.set_username("example").unwrap();
    source.set_password(Some("placeholder")).unwrap();
    let error = redact_command_output(&format!("unable to read {source}"));
    assert!(error.contains("https://***@registry.example.com/pkg"));
    assert!(!error.contains("placeholder"));
}

#[tokio::test]
async fn audit_deadline_keeps_unchecked_versions_as_failures() {
    let snapshot =
        parse_pnpm_snapshot(&json!([{"dependencies": {"pkg": package("pkg", "1.0.0")}}])).unwrap();
    let provider = FakeDates::default();
    let result = audit_snapshot(
        snapshot,
        policy().cutoff().unwrap(),
        &provider,
        Instant::now(),
        None,
    )
    .await;
    assert!(result.has_failures());
    assert_eq!(result.unverified.len(), 1);
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn metadata_wait_is_bounded_by_the_remaining_audit_budget() {
    struct SlowDates;
    #[async_trait]
    impl PublicationProvider for SlowDates {
        async fn fetch_dates(&self, _: &str) -> Result<PublicationDates, String> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            unreachable!("the audit budget must cancel this request");
        }
    }
    let snapshot = parse_pnpm_snapshot(&json!([{"dependencies": {
        "pkg": package("pkg", "1.0.0")
    }}]))
    .unwrap();
    let result = audit_snapshot(
        snapshot,
        policy().cutoff().unwrap(),
        &SlowDates,
        Instant::now() + Duration::from_millis(20),
        None,
    )
    .await;
    assert_eq!(result.checked, 0);
    assert_eq!(result.unverified.len(), 1);
    assert!(result.unverified[0].reason.contains("timed out"));
}

#[cfg(unix)]
fn fake_pnpm(directory: &Path, script: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let program = directory.join("fake-pnpm");
    std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

#[cfg(unix)]
#[tokio::test]
async fn pnpm_synthesized_public_urls_require_effective_public_registry() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_pnpm(
        directory.path(),
        "printf '\"https://registry.example.com/\"'",
    );
    let mut snapshot = parse_pnpm_snapshot(
        &json!([{"dependencies":{"private-name":package("private-name","1.0.0")}}]),
    )
    .unwrap();
    verify_pnpm_registry_sources(
        directory.path(),
        &program,
        &mut snapshot,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap();
    let provider = FakeDates::default();
    let result = check(snapshot, &provider).await;
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(result.unverified.len(), 1);
    assert!(result.unverified[0].reason.contains("effective registry"));
}

#[cfg(unix)]
#[tokio::test]
async fn pnpm_scoped_registry_overrides_default_and_private_tarballs_remain_unverified() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_pnpm(
        directory.path(),
        "case \"$3\" in registry) printf '\"https://registry.npmjs.org/\"';; '@private:registry') printf '\"https://registry.example.com/\"';; *) printf null;; esac",
    );
    let mut private = package("explicit-private", "1.0.0");
    private["resolved"] =
        json!("https://registry.example.com/explicit-private/-/explicit-private-1.0.0.tgz");
    let mut snapshot = parse_pnpm_snapshot(&json!([{"dependencies":{
        "@private/name":package("@private/name","1.0.0"),
        "@example/public":package("@example/public","1.0.0"),
        "explicit-public":package("explicit-public","1.0.0"),
        "explicit-private":private
    }}]))
    .unwrap();
    verify_pnpm_registry_sources(
        directory.path(),
        &program,
        &mut snapshot,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap();
    let provider = FakeDates {
        dates: HashMap::from([
            (
                "@example/public".into(),
                dates(&[("1.0.0", "2026-09-01T00:00:00Z")]),
            ),
            (
                "explicit-public".into(),
                dates(&[("1.0.0", "2026-09-01T00:00:00Z")]),
            ),
        ]),
        ..Default::default()
    };
    let result = check(snapshot, &provider).await;
    assert_eq!(result.checked, 2);
    assert_eq!(result.unverified.len(), 2);
    let requests = provider.requests.lock().unwrap();
    assert!(!requests.iter().any(|name| name.contains("private")));
}

#[cfg(unix)]
#[tokio::test]
async fn pnpm_unreadable_registry_config_never_sends_names_to_public_npm() {
    let directory = tempfile::tempdir().unwrap();
    for script in [
        "exit 7",
        "printf 'not JSON'",
        "printf null",
        "printf '\"https://registry.npmjs.org/\"'; printf warning >&2",
    ] {
        let program = fake_pnpm(directory.path(), script);
        let mut snapshot =
            parse_pnpm_snapshot(&json!([{"dependencies":{"unknown":package("unknown","1.0.0")}}]))
                .unwrap();
        verify_pnpm_registry_sources(
            directory.path(),
            &program,
            &mut snapshot,
            Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
        let provider = FakeDates::default();
        let result = check(snapshot, &provider).await;
        assert!(provider.requests.lock().unwrap().is_empty());
        assert_eq!(result.unverified.len(), 1);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn pnpm_list_reads_every_workspace_from_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("package.json"), "{}").unwrap();
    std::fs::write(
        directory.path().join("pnpm-workspace.yaml"),
        "packages: []\n",
    )
    .unwrap();
    let program = fake_pnpm(
        directory.path(),
        "printf '%s\\n' \"$@\" > arguments.txt\nprintf '[{\"dependencies\":{}}]'",
    );
    let snapshot = read_pnpm_graph(
        directory.path(),
        &program,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert!(snapshot.packages.is_empty());
    let arguments = std::fs::read_to_string(directory.path().join("arguments.txt")).unwrap();
    assert_eq!(
        arguments.lines().collect::<Vec<_>>(),
        [
            "list",
            "--depth",
            "Infinity",
            "--lockfile-only",
            "--json",
            "--recursive",
            "--include-workspace-root"
        ]
    );
}

#[test]
fn pnpm_empty_stale_importer_is_unverified_even_when_list_exits_successfully() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("package.json"),
        r#"{"dependencies":{"required":"^1.0.0"},"optionalDependencies":{"optional":"1.0.0"}}"#,
    )
    .unwrap();
    let stale = json!([{"path": directory.path(), "dependencies": {}}]);
    assert!(
        verify_pnpm_manifest_dependencies(directory.path(), &stale)
            .unwrap_err()
            .contains("required")
    );
    let complete = json!([{"path": directory.path(), "dependencies": {"required": package("required", "1.0.0")}}]);
    assert!(verify_pnpm_manifest_dependencies(directory.path(), &complete).is_ok());
}

#[test]
fn pnpm_workspace_importer_validation_includes_member_dev_dependencies_and_root() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("packages/member");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(directory.path().join("package.json"), "{}").unwrap();
    std::fs::write(
        member.join("package.json"),
        r#"{"devDependencies":{"dev-required":"1.0.0"}}"#,
    )
    .unwrap();
    let stale = json!([{"path": directory.path()}, {"path": member}]);
    assert!(
        verify_pnpm_manifest_dependencies(directory.path(), &stale)
            .unwrap_err()
            .contains("dev-required")
    );
    let complete_member = json!({"path": member, "devDependencies": {"dev-required": package("dev-required", "1.0.0")}});
    let missing_root = json!([complete_member]);
    assert!(
        verify_pnpm_manifest_dependencies(directory.path(), &missing_root)
            .unwrap_err()
            .contains("root")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn pnpm_successful_list_with_lock_warnings_is_unverified() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("package.json"), "{}").unwrap();
    let program = fake_pnpm(
        directory.path(),
        "printf 'Ignoring unsupported lockfile' >&2\nprintf '[{\"dependencies\":{}}]'",
    );
    let error = read_pnpm_graph(
        directory.path(),
        &program,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap_err();
    assert!(error.contains("Ignoring unsupported lockfile"));
    assert!(error.contains("unverified"));
}

#[cfg(unix)]
#[tokio::test]
async fn pnpm_list_command_failures_and_timeouts_are_not_successes() {
    let directory = tempfile::tempdir().unwrap();
    let program = fake_pnpm(directory.path(), "printf 'lock parse failed' >&2\nexit 7");
    let error = read_pnpm_graph(
        directory.path(),
        &program,
        Instant::now() + Duration::from_secs(2),
    )
    .await
    .unwrap_err();
    assert!(error.contains("lock parse failed"));
    assert!(error.contains('7'));

    let program = fake_pnpm(directory.path(), "exec sleep 2");
    let error = read_pnpm_graph(
        directory.path(),
        &program,
        Instant::now() + Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert!(error.contains("timed out"));
}
