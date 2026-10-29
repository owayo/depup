use super::*;
use crate::domain::SkipReason;
use std::process::Command;

/// An excluded branch must not pull in its new requirement and cancel mature updates.
#[tokio::test]
async fn excluded_git_dependency_keeps_its_commit_and_age_eligible_updates() {
    for excluded in ["gitdep", "alias"] {
        let repository = tempfile::tempdir().unwrap();
        let git = |argv: &[&str]| {
            let output = Command::new("git")
                .args(argv)
                .current_dir(repository.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--initial-branch=main"]);
        let revision = |requirement: &str| {
            std::fs::create_dir_all(repository.path().join("src")).unwrap();
            std::fs::write(repository.path().join("src/lib.rs"), "").unwrap();
            std::fs::write(
                repository.path().join("Cargo.toml"),
                format!("[package]\nname = 'gitdep'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nleaf = '{requirement}'\n"),
            ).unwrap();
            git(&["add", "Cargo.toml", "src/lib.rs"]);
            git(&[
                "-c",
                "user.name=Example",
                "-c",
                "user.email=you@example.com",
                "commit",
                "-m",
                "test revision",
            ]);
        };
        revision("1.0");
        let url = reqwest::Url::from_directory_path(repository.path()).unwrap();
        let registry = LocalRegistry::new();
        registry
            .publish("selected", "1.0.0", &[])
            .publish("leaf", "1.0.0", &[]);
        let manifest = format!(
            "[package]\nname = 'app'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nselected = '1.0.0'\nalias = {{ package = 'gitdep', git = '{url}', branch = 'main' }}\n"
        );
        let project = TestProject::new(&registry, &manifest);
        let cargo = |argv: &[&str]| {
            // All sources are local; online mode allows Cargo to fetch file:// Git commits.
            let output = Command::new(TestProject::cargo_program())
                .args(argv)
                .current_dir(project.path())
                .envs(project.cargo_envs())
                .env("CARGO_NET_OFFLINE", "false")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        cargo(&["generate-lockfile"]);
        let git_source = |content: &str| {
            crate::manifest::parse_lock_packages(content)
                .into_iter()
                .find(|package| package.name == "gitdep")
                .unwrap()
                .source
                .unwrap()
        };
        let before = git_source(&project.read("Cargo.lock"));
        revision("2.0");
        registry
            .publish("selected", "1.1.0", &[])
            .publish("leaf", "2.0.0", &[]);
        let dependencies = CargoTomlParser.parse(&manifest).unwrap();
        let mut update =
            ManifestUpdateResult::new(project.path().join("Cargo.toml"), Language::Rust);
        for dependency in dependencies {
            update.add_result(if dependency.name == "selected" {
                UpdateResult::update(dependency, "1.1.0")
            } else {
                UpdateResult::skip(dependency, SkipReason::Excluded)
            });
        }
        let write = ManifestWriter::new(false)
            .apply_updates(&update, &CargoTomlParser)
            .unwrap();
        let written = project.read("Cargo.toml");
        let mut summary = UpdateSummary::new(false);
        summary.add_manifest(update);
        let mut result = OrchestratorResult {
            summary,
            write_results: vec![write],
            errors: vec![],
        };
        let mut args = args(project.path());
        args.exclude = vec![excluded.into()];
        let orchestrator = Orchestrator::new(args.clone()).unwrap();
        // Use the same only=false input as the CLI's --exclude-only path.
        let plan = InstallPlan::new_scoped(&orchestrator, &result, &None, project.path(), false);
        let mut checks = Fixed::node(
            NodeBaseline::Known(BTreeSet::new()),
            NodeLockAuditResult::default(),
        );
        checks.dates = FakeCratesIo::new()
            .release("selected", "1.0.0", old())
            .release("selected", "1.1.0", old())
            .release("leaf", "1.0.0", old())
            .release("leaf", "2.0.0", young());
        checks.cargo = CargoCommand::new().program(TestProject::cargo_program());
        for (key, value) in project.cargo_envs() {
            checks.cargo = checks.cargo.env(key, value);
        }
        let outcome = run_with(
            &args,
            &orchestrator,
            &plan,
            &mut result,
            &checks,
            |command| {
                cargo(
                    &command.argv[1..]
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                );
                InstallResult::success(
                    command.language,
                    command.argv.join(" "),
                    String::new(),
                    String::new(),
                )
            },
        )
        .await;
        assert!(
            !outcome.install_failed && !outcome.audit_failed,
            "excluded: {excluded}"
        );
        assert_eq!(project.read("Cargo.toml"), written);
        assert_eq!(git_source(&project.read("Cargo.lock")), before);
        assert_eq!(project.locked_versions("selected"), ["1.1.0"]);
        assert_eq!(project.locked_versions("leaf"), ["1.0.0"]);
        assert_eq!(result.summary.total_updates(), 1);
        cargo(&["update", "--workspace", "--locked"]);
    }
}
