use super::*;

mod retry_regressions {
    use super::*;
    use crate::test_support::fake_crates_io::FakeCratesIo;
    use crate::test_support::local_registry::{LocalRegistry, TestProject};
    use chrono::TimeZone;

    fn fixture(
        pin: bool,
    ) -> (
        LocalRegistry,
        TestProject,
        RegistryLockEntries,
        FakeCratesIo,
        CargoCommand,
    ) {
        fixture_with_middle(pin, false)
    }

    fn fixture_with_middle(
        pin: bool,
        middle: bool,
    ) -> (
        LocalRegistry,
        TestProject,
        RegistryLockEntries,
        FakeCratesIo,
        CargoCommand,
    ) {
        let registry = LocalRegistry::new();
        registry
            .publish("a-dep", "1.1.0", &[])
            .publish("a-dep", "1.2.0", &[]);
        registry
            .publish("middle", "1.1.0", &[("a-dep", "^1.1"), ("old-only", "1")])
            .publish("middle", "1.2.0", &[("a-dep", "^1.2"), ("new-only", "1")])
            .publish("old-only", "1.0.0", &[])
            .publish("new-only", "1.0.0", &[]);
        registry
            .publish(
                "z-top",
                "1.1.0",
                if middle {
                    &[("middle", "=1.1.0")]
                } else {
                    &[("a-dep", "^1.1")]
                },
            )
            .publish(
                "z-top",
                "1.2.0",
                if middle {
                    &[("middle", "=1.2.0")]
                } else {
                    &[("a-dep", "^1.2")]
                },
            );
        registry
            .publish("s-solo", "1.0.0", &[])
            .publish("s-solo", "1.1.0", &[]);
        registry.publish("y-pin", "1.0.0", &[("a-dep", "^1.2")]);
        let manifest = |old: bool| {
            format!(
                "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nz-top = '{}'\ns-solo = '{}'\n{}",
                if old { "=1.1.0" } else { "1" },
                if old { "=1.0.0" } else { "1" },
                if !old && pin { "y-pin = '1'\n" } else { "" }
            )
        };
        let project = TestProject::new(&registry, &manifest(true));
        project.cargo_ok(&["generate-lockfile"]);
        project.cargo_ok(&["update", "-p", "a-dep", "--precise", "1.1.0"]);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.write("Cargo.toml", &manifest(false));
        project.cargo_ok(&["update"]);
        let old = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let young = chrono::Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap();
        let dates = FakeCratesIo::new()
            .release("a-dep", "1.1.0", old)
            .release("a-dep", "1.2.0", young)
            .release("z-top", "1.1.0", old)
            .release("z-top", "1.2.0", young)
            .release("s-solo", "1.0.0", old)
            .release("s-solo", "1.1.0", young)
            .release("y-pin", "1.0.0", old)
            .release("middle", "1.1.0", old)
            .release("middle", "1.2.0", old)
            .release("old-only", "1.0.0", old)
            .release("new-only", "1.0.0", old);
        let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
        for (key, value) in project.cargo_envs() {
            cargo = cargo.env(key, value);
        }
        (registry, project, baseline, dates, cargo)
    }

    fn cutoff() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap()
    }

    struct AdvancingAgeBudget {
        inner: FakeCratesIo,
        epoch: Instant,
        offset: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }

    #[async_trait::async_trait]
    impl RegistryAdapter for AdvancingAgeBudget {
        fn language(&self) -> Language {
            Language::Rust
        }
        fn registry_name(&self) -> &'static str {
            "fake-crates-io"
        }
        async fn fetch_versions(
            &self,
            package: &str,
        ) -> Result<Vec<VersionInfo>, crate::error::RegistryError> {
            if package == "c-next" {
                let shift = Duration::from_secs(41).saturating_sub(self.epoch.elapsed());
                self.offset.store(
                    u64::try_from(shift.as_millis()).unwrap(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
            self.inner.fetch_versions(package).await
        }
    }

    #[tokio::test]
    async fn review_normal_pass_hands_reserved_candidates_to_single_retries() {
        let registry = LocalRegistry::new();
        registry
            .publish("a-middle", "1.1.0", &[])
            .publish("a-middle", "1.2.0", &[("support", "=1.1.0")])
            .publish("a-middle", "1.3.0", &[]);
        registry
            .publish("b-top", "1.1.0", &[])
            .publish("b-top", "1.2.0", &[("support", "=1.2.0")]);
        registry
            .publish("support", "1.1.0", &[])
            .publish("support", "1.2.0", &[]);
        registry
            .publish("c-next", "1.0.0", &[])
            .publish("c-next", "1.1.0", &[]);
        let project = TestProject::new(
            &registry,
            "[package]\nname='example'\nversion='0.1.0'\nedition='2024'\n[dependencies]\na-middle='=1.1.0'\nb-top='=1.1.0'\nc-next='=1.0.0'\n",
        );
        project.cargo_ok(&["generate-lockfile"]);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.write("Cargo.toml", "[package]\nname='example'\nversion='0.1.0'\nedition='2024'\n[dependencies]\na-middle='1'\nb-top='1'\nc-next='1'\n");
        project.cargo_ok(&["update"]);
        let old = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let young = chrono::Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap();
        let dates = FakeCratesIo::new()
            .release("a-middle", "1.1.0", old)
            .release("a-middle", "1.2.0", old)
            .release("a-middle", "1.3.0", young)
            .release("b-top", "1.1.0", old)
            .release("b-top", "1.2.0", young)
            .release("support", "1.1.0", old)
            .release("support", "1.2.0", old)
            .release("c-next", "1.0.0", old)
            .release("c-next", "1.1.0", young);
        let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
        for (key, value) in project.cargo_envs() {
            cargo = cargo.env(key, value);
        }
        let offset = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let dates = AdvancingAgeBudget {
            inner: dates,
            epoch: Instant::now(),
            offset: offset.clone(),
        };
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(60),
        };
        let versions = VersionFetcher::new(4, 1);
        let mut auditor = LockAgeAuditor::new(&input, &versions, false);
        auditor.elapsed_offset = Some(offset);
        let result = auditor.run(None).await;
        assert_eq!(project.locked_versions("a-middle"), ["1.2.0"], "{result:?}");
        assert_eq!(project.locked_versions("b-top"), ["1.1.0"]);
        assert_eq!(project.locked_versions("c-next"), ["1.0.0"]);
        assert!(!result.has_unresolved(), "{result:?}");
        assert!(result.problems.is_empty(), "{result:?}");
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_new_dependency_edges_allow_restoring_automatic_moves() {
        new_edge_case(false).await;
    }

    #[tokio::test]
    async fn review_new_edge_source_dependency_closures_are_restored() {
        new_edge_case(true).await;
    }

    #[tokio::test]
    async fn review_same_version_feature_changes_return_to_install_time_dependencies() {
        use crate::test_support::local_registry::IndexDep;
        let registry = LocalRegistry::new();
        registry
            .publish("old-only", "1.0.0", &[])
            .publish("new-only", "1.0.0", &[])
            .publish("unresolved", "1.0.0", &[]);
        let mut old_dep = IndexDep::normal("old-only", "1");
        old_dep.optional = true;
        let mut new_dep = IndexDep::normal("new-only", "1");
        new_dep.optional = true;
        registry.publish_with("middle", "1.0.0", &[old_dep, new_dep]);
        // Add the feature definitions to this fixture's index, without changing the
        // shared registry builder or requiring any downloadable crate contents.
        let index_path = registry.path().join("index/mi/dd/middle");
        let mut entry: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&index_path).unwrap()).unwrap();
        entry["features"] = serde_json::json!({"old": ["old-only"], "new": ["new-only"]});
        std::fs::write(index_path, format!("{entry}\n")).unwrap();
        let mut old_middle = IndexDep::normal("middle", "=1.0.0");
        old_middle.features = vec!["old".into()];
        let mut new_middle = IndexDep::normal("middle", "=1.0.0");
        new_middle.features = vec!["new".into()];
        registry
            .publish("b-top", "1.0.0", &[])
            .publish_with(
                "b-top",
                "1.1.0",
                &[old_middle, IndexDep::normal("unresolved", "1")],
            )
            .publish_with("b-top", "1.2.0", &[new_middle]);
        let project = TestProject::new(
            &registry,
            "[package]\nname='example'\nversion='0.1.0'\nedition='2024'\n[dependencies]\nb-top='=1.0.0'\n",
        );
        project.cargo_ok(&["generate-lockfile"]);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.write("Cargo.toml", "[package]\nname='example'\nversion='0.1.0'\nedition='2024'\n[dependencies]\nb-top='1'\nunresolved='1'\n");
        project.cargo_ok(&["update"]);
        assert_eq!(project.locked_versions("middle"), ["1.0.0"]);
        assert_eq!(project.locked_versions("new-only"), ["1.0.0"]);
        assert!(project.locked_versions("old-only").is_empty());
        let old = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let young = chrono::Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap();
        let dates = FakeCratesIo::new()
            .release("b-top", "1.0.0", old)
            .release("b-top", "1.1.0", old)
            .release("b-top", "1.2.0", young)
            .release("middle", "1.0.0", old)
            .release("old-only", "1.0.0", old)
            .release("new-only", "1.0.0", old)
            .release("unresolved", "1.0.0", young);
        let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
        for (key, value) in project.cargo_envs() {
            cargo = cargo.env(key, value);
        }
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: LOCK_AGE_AUDIT_BUDGET,
        };
        let versions = VersionFetcher::new(4, 1);
        let result = LockAgeAuditor::new(&input, &versions, false)
            .run(None)
            .await;
        assert!(result.problems.is_empty(), "{result:?}");
        assert_eq!(project.locked_versions("b-top"), ["1.2.0"], "{result:?}");
        assert_eq!(project.locked_versions("middle"), ["1.0.0"]);
        assert_eq!(project.locked_versions("new-only"), ["1.0.0"]);
        assert!(project.locked_versions("old-only").is_empty());
        assert!(result.has_unresolved());
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    async fn new_edge_case(with_support: bool) {
        let registry = LocalRegistry::new();
        registry
            .publish("a-middle", "1.1.0", &[])
            .publish(
                "a-middle",
                "1.2.0",
                if with_support {
                    &[("support", "=1.1.0")]
                } else {
                    &[]
                },
            )
            .publish(
                "a-middle",
                "1.3.0",
                if with_support {
                    &[("support", "=1.2.0")]
                } else {
                    &[]
                },
            );
        registry
            .publish("support", "1.1.0", &[])
            .publish("support", "1.2.0", &[]);
        registry
            .publish("b-top", "1.0.0", &[])
            .publish(
                "b-top",
                "1.1.0",
                &[("a-middle", "=1.1.0"), ("unresolved", "1")],
            )
            .publish("b-top", "1.2.0", &[]);
        registry.publish("unresolved", "1.0.0", &[]);
        let project = TestProject::new(
            &registry,
            "[package]\nname='example'\nversion='0.1.0'\nedition='2024'\n[dependencies]\na-middle='=1.1.0'\nb-top='=1.0.0'\n",
        );
        project.cargo_ok(&["generate-lockfile"]);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.write("Cargo.toml", "[package]\nname='example'\nversion='0.1.0'\nedition='2024'\n[dependencies]\na-middle='1'\nb-top='1'\nunresolved='1'\n");
        project.cargo_ok(&["update"]);
        let old = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let young = chrono::Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap();
        let dates = FakeCratesIo::new()
            .release("a-middle", "1.1.0", old)
            .release("a-middle", "1.2.0", old)
            .release("a-middle", "1.3.0", young)
            .release("b-top", "1.0.0", old)
            .release("b-top", "1.1.0", old)
            .release("b-top", "1.2.0", young)
            .release("support", "1.1.0", old)
            .release("support", "1.2.0", old)
            .release("unresolved", "1.0.0", young);
        let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
        for (key, value) in project.cargo_envs() {
            cargo = cargo.env(key, value);
        }
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: LOCK_AGE_AUDIT_BUDGET,
        };
        let versions = VersionFetcher::new(4, 1);
        let result = LockAgeAuditor::new(&input, &versions, false)
            .run(None)
            .await;
        assert!(result.problems.is_empty(), "{result:?}");
        assert_eq!(project.locked_versions("a-middle"), ["1.3.0"], "{result:?}");
        assert_eq!(project.locked_versions("b-top"), ["1.2.0"]);
        if with_support {
            assert_eq!(project.locked_versions("support"), ["1.2.0"]);
        }
        assert!(result.has_unresolved());
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_automatic_dependency_changes_return_to_the_install_time_combination() {
        let (_registry, project, baseline, dates, cargo) = fixture_with_middle(true, true);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(19),
        };
        let versions = VersionFetcher::new(4, 1);
        let result = LockAgeAuditor::new(&input, &versions, false)
            .run(None)
            .await;
        assert_eq!(project.locked_versions("z-top"), ["1.2.0"], "{result:?}");
        assert_eq!(project.locked_versions("middle"), ["1.2.0"]);
        assert_eq!(project.locked_versions("new-only"), ["1.0.0"]);
        assert!(project.locked_versions("old-only").is_empty());
        assert_eq!(project.locked_versions("s-solo"), ["1.0.0"]);
        assert!(result.has_unresolved());
        assert!(result.problems.is_empty(), "{result:?}");
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_external_changes_to_automatic_dependencies_are_preserved() {
        let (registry, project, baseline, dates, cargo) = fixture_with_middle(true, true);
        registry.publish("old-only", "1.1.0", &[]);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(19),
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let mut state = AuditState::new(input.budget);
        state.installed_lock = project.read("Cargo.lock");
        auditor.run_pass(&mut state, 0, false, None).await.unwrap();
        assert_eq!(project.locked_versions("middle"), ["1.1.0"]);
        project.cargo_ok(&["update", "-p", "old-only@1.1.0", "--precise", "1.0.0"]);
        auditor.restore_mixed(&mut state).await;
        assert_eq!(project.locked_versions("old-only"), ["1.0.0"]);
        assert_eq!(project.locked_versions("z-top"), ["1.1.0"]);
        assert_eq!(project.locked_versions("s-solo"), ["1.0.0"]);
        assert!(
            state
                .problems
                .iter()
                .any(|problem| problem.contains("external change") && problem.contains("old-only")),
            "{:?}",
            state.problems
        );
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_joint_resolution_receipts_track_automatic_additions_and_removals() {
        let (_registry, project, baseline, dates, cargo) = fixture_with_middle(true, true);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: LOCK_AGE_AUDIT_BUDGET,
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let mut state = AuditState::new(input.budget);
        state.installed_lock = project.read("Cargo.lock");
        state.changes.observe(&state.installed_lock);
        auditor
            .rollback_one(
                RollbackCandidate {
                    name: "s-solo".into(),
                    current: "1.1.0".into(),
                    target: "1.0.0".into(),
                    minimum: None,
                },
                &mut state,
                &mut AuditPass::default(),
                None,
            )
            .await
            .unwrap();
        let mut pass = AuditPass {
            together: ["z-top", "middle"]
                .into_iter()
                .map(|name| RollbackCandidate {
                    name: name.into(),
                    current: "1.2.0".into(),
                    target: "1.1.0".into(),
                    minimum: None,
                })
                .collect(),
            ..Default::default()
        };
        auditor
            .resolve_conflicts(&mut state, &mut pass, None)
            .await
            .unwrap();
        assert_eq!(project.locked_versions("middle"), ["1.1.0"]);
        assert_eq!(project.locked_versions("old-only"), ["1.0.0"]);
        assert!(project.locked_versions("new-only").is_empty());
        auditor.restore_mixed(&mut state).await;
        assert!(state.problems.is_empty(), "{:?}", state.problems);
        assert_eq!(project.locked_versions("z-top"), ["1.2.0"]);
        assert_eq!(project.locked_versions("middle"), ["1.2.0"]);
        assert!(project.locked_versions("old-only").is_empty());
        assert_eq!(project.locked_versions("new-only"), ["1.0.0"]);
        assert_eq!(project.locked_versions("s-solo"), ["1.0.0"]);
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_pending_retries_prioritize_every_candidate_over_the_together_reserve() {
        let (registry, project, baseline, dates, cargo) = fixture(false);
        registry
            .publish("a-block", "1.1.0", &[])
            .publish("a-block", "1.2.0", &[]);
        let manifest = format!("{}a-block = '=1.2.0'\n", project.read("Cargo.toml"));
        project.write("Cargo.toml", &manifest);
        project.cargo_ok(&["update", "--workspace"]);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(19),
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let mut state = AuditState::new(input.budget);
        let blocked = RollbackCandidate {
            name: "a-block".into(),
            current: "1.2.0".into(),
            target: "1.1.0".into(),
            minimum: None,
        };
        let child = RollbackCandidate {
            name: "a-dep".into(),
            current: "1.2.0".into(),
            target: "1.1.0".into(),
            minimum: None,
        };
        for candidate in [&blocked, &child] {
            auditor
                .rollback_one(
                    candidate.clone(),
                    &mut state,
                    &mut AuditPass::default(),
                    None,
                )
                .await
                .unwrap();
        }
        assert_eq!(state.alone_failures.len(), 2);
        project.cargo_ok(&["update", "-p", "z-top@1.2.0", "--precise", "1.1.0"]);
        state.started = Instant::now();
        state.deadline = state.started + input.budget;
        let mut pass = AuditPass {
            together: vec![blocked, child],
            ..Default::default()
        };
        auditor
            .retry_pending(&mut state, &mut pass, None)
            .await
            .unwrap();
        assert_eq!(project.locked_versions("a-dep"), ["1.1.0"]);
        assert_eq!(project.locked_versions("a-block"), ["1.2.0"]);
        assert!(!pass.budget_exhausted);
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_lock_changes_after_planning_abort_restoration_without_writing() {
        use crate::cargo_rollback::batch::{RestorationScope, RestoreCandidate, restore_installed};
        let (_registry, project, _baseline, _dates, cargo) = fixture_with_middle(true, true);
        let expected = project.read("Cargo.lock");
        project.cargo_ok(&["update", "-p", "s-solo@1.1.0", "--precise", "1.0.0"]);
        let external = project.read("Cargo.lock");
        let members = [RestoreCandidate {
            name: "z-top".into(),
            current: Some("1.2.0".into()),
            installed: "1.2.0".into(),
        }];
        let scope = RestorationScope {
            members: &members,
            discarded: &[],
            installed: &expected,
            expected_lock: &expected,
        };
        let error = restore_installed(
            project.path(),
            scope,
            &cargo,
            Instant::now() + Duration::from_secs(60),
            CARGO_UPDATE_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(error.contains("changed after planning"), "{error}");
        assert_eq!(project.read("Cargo.lock"), external);
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn review_successful_joint_receipts_match_the_committed_lock() {
        let (_registry, project, _baseline, _dates, cargo) = fixture_with_middle(true, true);
        let before = project.read("Cargo.lock");
        let candidates: Vec<_> = ["z-top", "middle"]
            .into_iter()
            .map(|name| RollbackCandidate {
                name: name.into(),
                current: "1.2.0".into(),
                target: "1.1.0".into(),
                minimum: None,
            })
            .collect();
        let (report, changes) = resolve_together_tracked(
            project.path(),
            &candidates,
            &cargo,
            Instant::now() + Duration::from_secs(60),
            CARGO_UPDATE_TIMEOUT,
        )
        .await;
        assert!(report.problems.is_empty(), "{report:?}");
        assert!(
            report
                .outcomes
                .iter()
                .all(|(_, outcome)| matches!(outcome, TogetherOutcome::Resolved(_))),
            "{report:?}"
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before, before);
        assert_eq!(changes[0].after, project.read("Cargo.lock"));
        assert_ne!(changes[0].before, changes[0].after);
        assert_eq!(
            changes[0].selected,
            candidates
                .iter()
                .map(|candidate| (candidate.name.clone(), candidate.current.clone()))
                .collect::<Vec<_>>()
        );
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn dependency_order_works_without_a_together_budget() {
        let (_registry, project, baseline, dates, cargo) = fixture(false);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(19),
        };
        let versions = VersionFetcher::new(4, 1);
        let result = LockAgeAuditor::new(&input, &versions, false)
            .run(None)
            .await;
        assert_eq!(project.locked_versions("a-dep"), ["1.1.0"], "{result:?}");
        assert_eq!(project.locked_versions("z-top"), ["1.1.0"], "{result:?}");
        assert!(!result.has_unresolved(), "{result:?}");
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn independent_series_roll_back_through_an_unselected_intermediate() {
        let registry = LocalRegistry::new();
        for version in ["1.1.0", "1.2.0", "2.1.0", "2.2.0"] {
            registry.publish("a-dep", version, &[]);
        }
        registry
            .publish("middle", "1.1.0", &[("a-dep", "^1.1")])
            .publish("middle", "1.2.0", &[("a-dep", "^1.2")]);
        registry
            .publish("z-top", "1.1.0", &[("middle", "=1.1.0")])
            .publish("z-top", "1.2.0", &[("middle", "=1.2.0")])
            .publish("z-top", "2.1.0", &[("a-dep", "^2.1")])
            .publish("z-top", "2.2.0", &[("a-dep", "^2.2")]);
        let manifest = |old: bool| {
            format!(
                "[package]\nname = 'example'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nfirst = {{ package = 'z-top', version = '{}' }}\nsecond = {{ package = 'z-top', version = '{}' }}\n",
                if old { "=1.1.0" } else { "1" },
                if old { "=2.1.0" } else { "2" }
            )
        };
        let project = TestProject::new(&registry, &manifest(true));
        project.cargo_ok(&["generate-lockfile"]);
        project.cargo_ok(&["update", "-p", "a-dep@1.2.0", "--precise", "1.1.0"]);
        project.cargo_ok(&["update", "-p", "a-dep@2.2.0", "--precise", "2.1.0"]);
        let baseline = parse_registry_entries(&project.read("Cargo.lock"));
        project.write("Cargo.toml", &manifest(false));
        project.cargo_ok(&["update"]);
        let old = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let young = chrono::Utc.with_ymd_and_hms(2026, 1, 29, 0, 0, 0).unwrap();
        let mut dates = FakeCratesIo::new();
        for name in ["a-dep", "z-top"] {
            for version in ["1.1.0", "2.1.0"] {
                dates = dates.release(name, version, old);
            }
            for version in ["1.2.0", "2.2.0"] {
                dates = dates.release(name, version, young);
            }
        }
        dates = dates
            .release("middle", "1.1.0", old)
            .release("middle", "1.2.0", old);
        let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
        for (key, value) in project.cargo_envs() {
            cargo = cargo.env(key, value);
        }
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(19),
        };
        let versions = VersionFetcher::new(4, 1);
        let result = LockAgeAuditor::new(&input, &versions, false)
            .run(None)
            .await;
        assert_eq!(
            project.locked_versions("a-dep"),
            ["1.1.0", "2.1.0"],
            "{result:?}"
        );
        assert_eq!(
            project.locked_versions("z-top"),
            ["1.1.0", "2.1.0"],
            "{result:?}"
        );
        assert!(!result.has_unresolved(), "{result:?}");
        // Multiple locked versions of a name share one metadata request.
        assert_eq!(dates.fetch_count(), 3);
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }

    #[tokio::test]
    async fn unresolved_related_rollbacks_return_to_installed_versions() {
        for budget in [Duration::from_secs(19), LOCK_AGE_AUDIT_BUDGET] {
            let (_registry, project, baseline, dates, cargo) = fixture(true);
            let input = LockAgeAudit {
                project_dir: project.path(),
                cutoff: cutoff(),
                exemptions: &Default::default(),
                baseline: &baseline,
                preferred: &PreferredVersions::new(),
                adapter: &dates,
                cargo: &cargo,
                budget,
            };
            let versions = VersionFetcher::new(4, 1);
            let result = LockAgeAuditor::new(&input, &versions, false)
                .run(None)
                .await;
            assert_eq!(project.locked_versions("z-top"), ["1.2.0"], "{result:?}");
            assert_eq!(project.locked_versions("a-dep"), ["1.2.0"]);
            assert_eq!(project.locked_versions("s-solo"), ["1.0.0"]);
            assert!(result.has_unresolved());
            assert!(result.problems.is_empty(), "{result:?}");
            assert!(
                result
                    .adjustments
                    .iter()
                    .any(|adjustment| adjustment.name == "z-top"
                        && matches!(adjustment.status, LockAgeStatus::Reverted(_)))
            );
            project.cargo_ok(&["update", "--workspace", "--locked"]);
        }
    }

    #[tokio::test]
    async fn singles_retry_only_after_the_lock_changes() {
        let (_registry, project, baseline, dates, cargo) = fixture(false);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(60),
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let candidate = RollbackCandidate {
            name: "a-dep".into(),
            current: "1.2.0".into(),
            target: "1.1.0".into(),
            minimum: Some("1.1.0".into()),
        };
        let key = (candidate.name.clone(), candidate.current.clone());
        let mut state = AuditState::new(input.budget);
        let mut pass = AuditPass::default();
        auditor
            .rollback_one(candidate.clone(), &mut state, &mut pass, None)
            .await
            .unwrap();
        assert!(state.alone_failures.contains_key(&key));
        state.alone_failures.get_mut(&key).unwrap().message = "same-lock sentinel".into();
        auditor
            .rollback_one(candidate.clone(), &mut state, &mut pass, None)
            .await
            .unwrap();
        assert_eq!(state.alone_failures[&key].message, "same-lock sentinel");
        state.alone_failures.get_mut(&key).unwrap().target = "1.0.0".into();
        auditor
            .rollback_one(candidate, &mut state, &mut pass, None)
            .await
            .unwrap();
        assert_eq!(state.alone_failures[&key].target, "1.1.0");
        assert_ne!(state.alone_failures[&key].message, "same-lock sentinel");
        project.cargo_ok(&["update", "-p", "z-top@1.2.0", "--precise", "1.1.0"]);
        auditor
            .retry_pending(&mut state, &mut pass, None)
            .await
            .unwrap();
        assert_eq!(project.locked_versions("a-dep"), ["1.1.0"]);
        assert!(!state.alone_failures.contains_key(&key));
        assert!(pass.together.is_empty());
        assert_eq!(pass.alone_successes, 1);
    }

    #[tokio::test]
    async fn stale_current_is_recorded_without_starting_cargo() {
        let (_registry, project, baseline, dates, _cargo) = fixture(false);
        project.cargo_ok(&["update", "-p", "z-top@1.2.0", "--precise", "1.1.0"]);
        let unavailable = CargoCommand::new().program("depup-no-such-test-cargo");
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &unavailable,
            budget: Duration::ZERO,
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let mut state = AuditState::new(input.budget);
        let mut pass = AuditPass::default();
        auditor
            .rollback_one(
                RollbackCandidate {
                    name: "z-top".into(),
                    current: "1.2.0".into(),
                    target: "1.1.0".into(),
                    minimum: None,
                },
                &mut state,
                &mut pass,
                None,
            )
            .await
            .unwrap();
        assert!(pass.any_downgraded);
        assert_eq!(
            state.log.entries[&("z-top".into(), "1.2.0".into())].status,
            LockAgeStatus::Downgraded
        );
    }

    #[tokio::test]
    async fn unchanged_timed_out_attempt_is_not_restarted_or_sent_to_together() {
        let (_registry, project, baseline, dates, _cargo) = fixture(false);
        let unavailable = CargoCommand::new().program("depup-no-such-test-cargo");
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &unavailable,
            budget: Duration::from_secs(60),
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let candidate = RollbackCandidate {
            name: "a-dep".into(),
            current: "1.2.0".into(),
            target: "1.1.0".into(),
            minimum: None,
        };
        let key = (candidate.name.clone(), candidate.current.clone());
        let mut state = AuditState::new(input.budget);
        let message = "cargo update timed out".to_string();
        state.log.record(
            &candidate.name,
            &candidate.current,
            Some(candidate.target.clone()),
            LockAgeStatus::NotAttempted(message.clone()),
        );
        state.alone_failures.insert(
            key.clone(),
            AloneFailure {
                target: candidate.target.clone(),
                lock: together_fingerprint(&project.read("Cargo.lock"), &[]),
                message: message.clone(),
                conflict: false,
            },
        );
        let mut pass = AuditPass::default();
        auditor
            .rollback_one(candidate, &mut state, &mut pass, None)
            .await
            .unwrap();
        assert_eq!(
            state.log.entries[&key].status,
            LockAgeStatus::NotAttempted(message)
        );
        assert!(pass.together.is_empty());
    }

    #[tokio::test]
    async fn restoration_has_its_own_budget_and_preserves_external_edits() {
        let (_registry, project, baseline, dates, cargo) = fixture(true);
        let input = LockAgeAudit {
            project_dir: project.path(),
            cutoff: cutoff(),
            exemptions: &Default::default(),
            baseline: &baseline,
            preferred: &PreferredVersions::new(),
            adapter: &dates,
            cargo: &cargo,
            budget: Duration::from_secs(19),
        };
        let versions = VersionFetcher::new(4, 1);
        let auditor = LockAgeAuditor::new(&input, &versions, false);
        let mut state = AuditState::new(input.budget);
        state.installed_lock = project.read("Cargo.lock");
        auditor.run_pass(&mut state, 0, false, None).await.unwrap();
        assert_eq!(project.locked_versions("z-top"), ["1.1.0"]);
        assert_eq!(project.locked_versions("s-solo"), ["1.0.0"]);
        project.cargo_ok(&["update", "-p", "s-solo@1.0.0", "--precise", "1.1.0"]);
        state.started = Instant::now() - Duration::from_secs(20);
        state.deadline = Instant::now();
        auditor.restore_mixed(&mut state).await;
        assert!(state.problems.is_empty(), "{:?}", state.problems);
        assert_eq!(project.locked_versions("z-top"), ["1.2.0"]);
        assert_eq!(project.locked_versions("s-solo"), ["1.1.0"]);
        project.cargo_ok(&["update", "--workspace", "--locked"]);
    }
}

#[test]
fn unreadable_or_invalid_lock_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Cargo.lock");
    assert!(read_audit_lock(&path).is_err());
    for content in [
        "",
        "broken {{{",
        "package = 1",
        "[[package]]\nname = 'library'\n",
    ] {
        std::fs::write(&path, content).unwrap();
        assert!(read_audit_lock(&path).is_err(), "{content}");
    }
    std::fs::write(&path, "version = 4\n").unwrap();
    assert!(read_audit_lock(&path).unwrap().1.is_empty());
}

fn version_at(version: &str, days_ago: i64) -> VersionInfo {
    VersionInfo::new(
        version,
        chrono::Utc::now() - chrono::Duration::days(days_ago),
    )
}

fn lock_entries(entries: &[(&str, &[&str])]) -> RegistryLockEntries {
    entries
        .iter()
        .map(|(name, versions)| {
            (
                (*name).to_string(),
                versions.iter().map(|v| (*v).to_string()).collect(),
            )
        })
        .collect()
}

/// install 前と同じバージョンで lock されている依存は監査対象に含めない。
/// crates.io は 1 リクエスト/秒 なので、lock 全体を舐めると数百件 = 数分かかる。
#[test]
fn test_changed_entries_excludes_unchanged() {
    let baseline = lock_entries(&[("serde", &["1.0.200"]), ("tokio", &["1.40.0"])]);
    let current = lock_entries(&[("serde", &["1.0.200"]), ("tokio", &["1.41.0"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(
        targets,
        vec![("tokio".to_string(), vec!["1.41.0".to_string()])]
    );
}

/// install で新しく lock に入った依存は監査対象になる
#[test]
fn test_changed_entries_includes_newly_added() {
    let baseline = lock_entries(&[("serde", &["1.0.200"])]);
    let current = lock_entries(&[("serde", &["1.0.200"]), ("anyhow", &["1.0.90"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(
        targets,
        vec![("anyhow".to_string(), vec!["1.0.90".to_string()])]
    );
}

/// 同名クレートが複数バージョン lock されている場合、新しく増えた版だけを対象にする
#[test]
fn test_changed_entries_picks_only_new_versions_of_same_crate() {
    let baseline = lock_entries(&[("syn", &["1.0.109"])]);
    let current = lock_entries(&[("syn", &["1.0.109", "2.0.90"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(
        targets,
        vec![("syn".to_string(), vec!["2.0.90".to_string()])]
    );
}

/// install 前に Cargo.lock が無かった場合 (baseline が空) は全件が対象
#[test]
fn test_changed_entries_empty_baseline_audits_everything() {
    let baseline = RegistryLockEntries::new();
    let current = lock_entries(&[("serde", &["1.0.200"]), ("tokio", &["1.41.0"])]);

    let targets = changed_entries(&current, &baseline);

    assert_eq!(targets.len(), 2);
}

/// 進捗表示と報告順を安定させるため、対象は名前順で返る
/// (`HashMap` の反復順は非決定的)
#[test]
fn test_changed_entries_is_sorted_by_name() {
    let baseline = RegistryLockEntries::new();
    let current = lock_entries(&[("zerocopy", &["0.7.0"]), ("anyhow", &["1.0.90"])]);

    let targets = changed_entries(&current, &baseline);

    let names: Vec<&str> = targets.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["anyhow", "zerocopy"]);
}

/// lock が install 前後で変わっていなければ監査対象は空 (レジストリ照会ゼロ)
#[test]
fn test_changed_entries_no_changes_is_empty() {
    let baseline = lock_entries(&[("serde", &["1.0.200"])]);
    let current = lock_entries(&[("serde", &["1.0.200"])]);

    assert!(changed_entries(&current, &baseline).is_empty());
}

#[test]
fn full_audit_includes_unchanged_versions_after_changed_crates() {
    let baseline = lock_entries(&[("alpha", &["1.0.0"]), ("zeta", &["1.0.0"])]);
    let current = lock_entries(&[("alpha", &["1.0.0"]), ("zeta", &["1.0.0", "1.0.1"])]);
    assert_eq!(
        audit_entries(&current, &baseline),
        vec![
            ("zeta".into(), vec!["1.0.0".into(), "1.0.1".into()]),
            ("alpha".into(), vec!["1.0.0".into()]),
        ]
    );
}

#[test]
fn test_pick_older_within_age_basic() {
    // age cutoff = 14 日前。現在 lock = 1.5.0 (3 日前) は age 違反。
    // 1.4.9 (30 日前) が最新の「古くて age 内」候補。
    let versions = vec![
        version_at("1.4.0", 100),
        version_at("1.4.5", 60),
        version_at("1.4.9", 30),
        version_at("1.5.0", 3), // age 違反の現在 lock
        version_at("1.6.0", 1), // 新しすぎる
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

#[test]
fn test_pick_older_within_age_returns_none_when_all_newer() {
    // 全候補が age 違反または現バージョン以上
    let versions = vec![version_at("1.5.0", 3), version_at("1.6.0", 1)];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert!(pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).is_none());
}

#[test]
fn test_pick_older_within_age_skips_prereleases() {
    // プレリリースは候補から除外
    let versions = vec![
        version_at("1.4.9", 30),
        version_at("1.5.0-beta.1", 60), // プレリリースなので除外
        version_at("1.5.0", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

#[test]
fn test_pick_older_within_age_picks_latest_eligible() {
    // 複数の age 内候補があれば semver 最大を選ぶ
    let versions = vec![
        version_at("1.3.0", 200),
        version_at("1.4.0", 100),
        version_at("1.4.9", 30),
        version_at("2.0.0", 5), // 新しすぎる
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "2.0.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

#[test]
fn test_pick_older_within_age_ignores_same_version() {
    // current と同じバージョンは候補外 (downgrade できない)
    let versions = vec![version_at("1.4.0", 30), version_at("1.5.0", 3)];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        pick_older_within_age(&versions, "1.5.0", cutoff, &Default::default()).as_deref(),
        Some("1.4.0"),
    );
}

/// judge がこの実行で選んだ版 (画面に出した更新先) を差し戻し先の第一候補にする。
/// judge が OSV で 1.4.9 を退けて 1.4.5 を選んでいれば、差し戻しでも 1.4.9 を選び直さない
#[test]
fn test_rollback_target_prefers_judge_choice() {
    let versions = vec![
        version_at("1.4.5", 60),
        version_at("1.4.9", 30),
        version_at("1.5.0", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let preferred = vec!["1.4.5".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.5.0",
            cutoff,
            Some(&preferred),
            None,
            &Default::default()
        )
        .as_deref(),
        Some("1.4.5"),
    );
}

/// judge の版が使えない (期間内・今の版以上・別系列) ときは期間を満たす最新の古い版
#[test]
fn test_rollback_target_falls_back_when_judge_choice_is_unusable() {
    let versions = vec![
        version_at("0.9.0", 200),
        version_at("1.4.9", 30),
        version_at("1.5.0", 3),
        version_at("1.5.1", 1),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    for preferred in [
        // 期間内 (新しすぎる)
        vec!["1.5.1".to_string()],
        // 今の版と同じ
        vec!["1.5.0".to_string()],
        // 別の semver 系列
        vec!["0.9.0".to_string()],
    ] {
        assert_eq!(
            rollback_target(
                &versions,
                "1.5.0",
                cutoff,
                Some(&preferred),
                None,
                &Default::default()
            )
            .as_deref(),
            Some("1.4.9"),
            "preferred = {preferred:?}",
        );
    }
    assert_eq!(
        rollback_target(&versions, "1.5.0", cutoff, None, None, &Default::default()).as_deref(),
        Some("1.4.9"),
    );
}

/// 同じ crate を member ごとに別の版へ更新した場合は、使えるもののうち最大を採る
#[test]
fn test_rollback_target_picks_highest_usable_judge_choice() {
    let versions = vec![
        version_at("1.4.0", 90),
        version_at("1.4.5", 60),
        version_at("1.5.0", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let preferred = vec!["1.4.0".to_string(), "1.4.5".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.5.0",
            cutoff,
            Some(&preferred),
            None,
            &Default::default()
        )
        .as_deref(),
        Some("1.4.5"),
    );
}

/// install 前の lock にもっと新しい版があったなら、judge の版 (manifest の古い要求から
/// `--max-change` で選んだ版など) まで下げない。下げると install 前に使えていた API が消える
#[test]
fn test_rollback_target_does_not_go_below_version_locked_before_install() {
    let versions = vec![
        version_at("1.40.5", 300),
        version_at("1.50.0", 90),
        version_at("1.52.0", 30),
        version_at("1.53.1", 3),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let preferred = vec!["1.40.5".to_string()];
    let before = vec!["1.50.0".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.53.1",
            cutoff,
            Some(&preferred),
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.52.0"),
    );
    // 別系列の install 前の版は下限にしない
    let before = vec!["0.9.0".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.53.1",
            cutoff,
            Some(&preferred),
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.40.5"),
    );
}

fn entries_of(pairs: &[(&str, &[&str])]) -> RegistryLockEntries {
    lock_entries(pairs)
}

/// 「差し戻した」と記録した組を最終の lock と突き合わせ、実態に合わせて直す
#[test]
fn test_adjustment_log_reconciles_with_final_lock() {
    let mut log = AdjustmentLog::default();
    // cargo は成功を返したが lock は変わっていない
    log.record(
        "still",
        "1.0.2",
        Some("1.0.1".into()),
        LockAgeStatus::Downgraded,
    );
    // 後の差し戻しでさらに古い版へ動いた
    log.record(
        "moved",
        "1.0.2",
        Some("1.0.1".into()),
        LockAgeStatus::Downgraded,
    );
    // lock から外れた
    log.record("gone", "1.0.2", None, LockAgeStatus::Removed);
    // 今の版より新しい版に動いている
    log.record(
        "newer",
        "1.0.2",
        Some("1.0.1".into()),
        LockAgeStatus::Downgraded,
    );
    // 差し戻していない記録は触らない
    log.record("young", "0.1.0", None, LockAgeStatus::NoOlderCandidate);

    log.reconcile_with_lock(&entries_of(&[
        ("still", &["1.0.2"]),
        ("moved", &["1.0.0"]),
        ("gone", &["0.9.0"]),
        ("newer", &["1.0.3"]),
        ("young", &["0.1.0"]),
    ]));
    let adjustments = log.into_adjustments();
    let by_name = |name: &str| adjustments.iter().find(|a| a.name == name).unwrap();

    assert!(matches!(
        by_name("still").status,
        LockAgeStatus::UpdateCommandFailed(ref m) if m.contains("still has still 1.0.2")
    ));
    assert_eq!(by_name("still").to, None);
    assert_eq!(by_name("still").target.as_deref(), Some("1.0.1"));
    assert_eq!(by_name("moved").status, LockAgeStatus::Downgraded);
    assert_eq!(by_name("moved").to.as_deref(), Some("1.0.0"));
    assert_eq!(by_name("gone").status, LockAgeStatus::Removed);
    assert!(matches!(
        by_name("newer").status,
        LockAgeStatus::UpdateCommandFailed(ref m) if m.contains("not older than 1.0.2")
    ));
    assert_eq!(by_name("young").status, LockAgeStatus::NoOlderCandidate);
}

/// まとめ解きの指紋は lock の内容と候補の集合の両方で変わり、並び順には左右されない
#[test]
fn test_together_fingerprint_depends_on_lock_and_candidates() {
    let candidate = |name: &str| RollbackCandidate {
        name: name.to_string(),
        current: "1.0.2".to_string(),
        target: "1.0.1".to_string(),
        minimum: None,
    };
    let a_b = [candidate("a"), candidate("b")];
    let b_a = [candidate("b"), candidate("a")];
    let a = [candidate("a")];
    assert_eq!(
        together_fingerprint("lock", &a_b),
        together_fingerprint("lock", &b_a)
    );
    assert_ne!(
        together_fingerprint("lock", &a_b),
        together_fingerprint("lock", &a)
    );
    assert_ne!(
        together_fingerprint("lock", &a_b),
        together_fingerprint("other", &a_b)
    );
}

/// 差し戻しの後の lock で、今の版が外れたかと行き先を調べる
#[test]
fn test_locked_after_rollback() {
    let entries = entries_of(&[("syn", &["1.0.109", "2.0.80"])]);
    assert_eq!(
        locked_after_rollback(&entries, "syn", "2.0.90"),
        Some(Some("2.0.80".to_string()))
    );
    assert_eq!(locked_after_rollback(&entries, "syn", "2.0.80"), None);
    assert_eq!(locked_after_rollback(&entries, "syn", "3.0.1"), Some(None));
    assert_eq!(
        locked_after_rollback(&entries, "tokio", "1.0.0"),
        Some(None)
    );
}

/// 期間を満たす古い版が install 前の版より古い (または無い) なら、install 前の版へ戻す。
/// install 前から期間内の版が入っていた場合でも、depup が入れた変更だけを取り消す
#[test]
fn test_rollback_target_returns_to_version_locked_before_install() {
    let versions = vec![
        version_at("1.49.0", 90),
        version_at("1.50.0", 5),
        version_at("1.50.1", 2),
    ];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    let before = vec!["1.50.0".to_string()];
    assert_eq!(
        rollback_target(
            &versions,
            "1.50.1",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.50.0"),
    );

    let only_young = vec![version_at("1.50.0", 5), version_at("1.50.1", 2)];
    assert_eq!(
        rollback_target(
            &only_young,
            "1.50.1",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("1.50.0"),
    );
}

#[test]
fn an_unavailable_baseline_does_not_allow_another_semver_series() {
    let versions = vec![version_at("0.7.9", 90), version_at("0.8.4", 2)];
    let before = vec!["0.8.3".to_string()];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert!(
        rollback_target(
            &versions,
            "0.8.4",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .is_none()
    );
}

#[test]
fn a_new_crate_is_not_rolled_back_across_semver_series() {
    let versions = vec![version_at("0.7.9", 90), version_at("0.8.4", 2)];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert!(rollback_target(&versions, "0.8.4", cutoff, None, None, &Default::default()).is_none());
}

#[test]
fn an_available_baseline_still_prevents_going_below_the_preinstall_version() {
    let versions = vec![
        version_at("0.8.1", 90),
        version_at("0.8.2", 5),
        version_at("0.8.4", 2),
    ];
    let before = vec!["0.8.2".to_string(), "0.8.3".to_string()];
    let cutoff = chrono::Utc::now() - chrono::Duration::days(14);
    assert_eq!(
        rollback_target(
            &versions,
            "0.8.4",
            cutoff,
            None,
            Some(&before),
            &Default::default()
        )
        .as_deref(),
        Some("0.8.2")
    );
}
