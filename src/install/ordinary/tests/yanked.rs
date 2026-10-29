use super::*;

/// A yanked pre-install transitive version must not cancel an independent mature update.
#[tokio::test]
async fn yanked_preinstall_transitive_preserves_age_eligible_updates() {
    let registry = LocalRegistry::new();
    registry
        .publish("top", "1.0.0", &[("middle", "^0.7")])
        .publish("middle", "0.7.5", &[("leaf", "^0.4.3")])
        .publish("leaf", "0.4.3", &[])
        .publish("independent", "1.0.0", &[]);
    let manifest = "[package]\nname = 'app'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\ntop = '1.0.0'\nindependent = '1.0.0'\n";
    let project = TestProject::new(&registry, manifest);
    project.cargo_ok(&["generate-lockfile"]);
    let lock = project.read("Cargo.lock");
    registry
        .yank("middle", "0.7.5")
        .publish("middle", "0.7.10", &[("leaf", "^0.4.6")])
        .publish("leaf", "0.4.7", &[])
        .publish("independent", "1.1.0", &[])
        .publish("independent", "1.2.0", &[]);
    let independent = CargoTomlParser
        .parse(manifest)
        .unwrap()
        .into_iter()
        .find(|dependency| dependency.name == "independent")
        .unwrap();
    let mut update = ManifestUpdateResult::new(project.path().join("Cargo.toml"), Language::Rust);
    update.add_result(UpdateResult::update(independent, "1.1.0"));
    let write = ManifestWriter::new(false)
        .apply_updates(&update, &CargoTomlParser)
        .unwrap();
    let written = project.read("Cargo.toml");
    let mut summary = UpdateSummary::new(false);
    summary.add_manifest(update);
    let mut cargo = CargoCommand::new().program(TestProject::cargo_program());
    for (key, value) in project.cargo_envs() {
        cargo = cargo.env(key, value);
    }
    let dates = FakeCratesIo::new()
        .release("top", "1.0.0", old())
        .release("middle", "0.7.5", old())
        .yank("middle", "0.7.5")
        .release("middle", "0.7.10", young())
        .release("leaf", "0.4.3", old())
        .release("leaf", "0.4.7", young())
        .release("independent", "1.0.0", old())
        .release("independent", "1.1.0", old())
        .release("independent", "1.2.0", young());
    let mut run = RustRun {
        _registry: registry,
        project,
        manifest: manifest.into(),
        lock,
        result: OrchestratorResult {
            summary,
            write_results: vec![write],
            errors: vec![],
        },
        checks: Fixed {
            dates,
            cargo,
            node_baseline: NodeBaseline::Known(BTreeSet::new()),
            node_audit: Mutex::new(None),
            edit_after_audit: false,
        },
    };
    let outcome = run.install().await;
    assert!(!outcome.install_failed && !outcome.audit_failed);
    assert!(!run.canceled());
    assert_eq!(run.result.summary.total_updates(), 1);
    assert_eq!(run.project.read("Cargo.toml"), written);
    assert_eq!(run.project.locked_versions("independent"), ["1.1.0"]);
    assert_eq!(run.project.locked_versions("middle"), ["0.7.5"]);
    assert_eq!(run.project.locked_versions("leaf"), ["0.4.3"]);
    run.project.cargo_ok(&["update", "--workspace", "--locked"]);
}
