//! 実行対象内の設定スコープと age の優先順位を解決する。

use crate::cli::CliArgs;
use crate::global_config::{DEFAULT_AGE, GlobalConfig};
use crate::manifest::{BunSettings, MiseSettings, PnpmSettings};
use crate::update::{AgeExemptions, AgePolicy};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) struct ResolvedAge {
    pub policy: AgePolicy,
    project: Option<ProjectAge>,
    cli_label: Option<&'static str>,
    suppressed_exemptions: bool,
}

struct ProjectAge {
    duration: Duration,
    source: PathBuf,
}

/// 対象ディレクトリから実行ルートまでの明示設定を継承し、最も厳しい期間を採る。
/// 兄弟プロジェクトや実行ルートの外側の設定は混ぜない。
pub(super) fn resolve(
    args: &CliArgs,
    config: Option<&GlobalConfig>,
    dir: &Path,
    evaluated_at: DateTime<Utc>,
) -> ResolvedAge {
    let root = project_scope_root(&args.path);
    let dir = absolute(dir);
    let mut project = None;
    if dir.starts_with(&root) {
        for scope in dir.ancestors().take_while(|scope| scope.starts_with(&root)) {
            for candidate in project_ages(scope) {
                if project
                    .as_ref()
                    .is_none_or(|old: &ProjectAge| candidate.duration > old.duration)
                {
                    project = Some(candidate);
                }
            }
        }
    }
    let exemptions = config
        .map(|config| config.age_exempt.clone())
        .unwrap_or_default();
    let suppressed_exemptions = project.is_some() && !exemptions.is_empty();
    let min_age = if let Some(project) = &project {
        Some(project.duration)
    } else if args.no_age {
        None
    } else {
        Some(
            args.age
                .or_else(|| config.and_then(GlobalConfig::age_duration))
                .unwrap_or(DEFAULT_AGE),
        )
    };
    let cli_label = if args.no_age {
        Some("--no-age")
    } else {
        args.age.map(|_| "--age")
    };
    ResolvedAge {
        policy: AgePolicy {
            min_age,
            evaluated_at,
            exemptions: if project.is_some() {
                AgeExemptions::default()
            } else {
                exemptions
            },
        },
        project,
        cli_label,
        suppressed_exemptions,
    }
}

impl ResolvedAge {
    pub(super) fn emit_notice(&self) {
        use colored::Colorize as _;
        if let Some(project) = &self.project {
            let seconds = project.duration.as_secs();
            let duration = if seconds.is_multiple_of(86400) {
                let days = seconds / 86400;
                format!("{days} {}", if days == 1 { "day" } else { "days" })
            } else {
                format!("{seconds}s")
            };
            let source = project.source.display();
            if let Some(cli) = self.cli_label {
                eprintln!("{}", format!("⚠ {cli} ignored: project's minimumReleaseAge ({duration} from {source}) takes precedence").yellow());
            } else {
                eprintln!(
                    "{}",
                    format!("ℹ Using project's minimumReleaseAge ({duration} from {source})")
                        .cyan()
                );
            }
        }
        if self.suppressed_exemptions {
            eprintln!("Warning: project minimumReleaseAge takes precedence over age_exempt");
        }
    }
}

fn project_ages(dir: &Path) -> Vec<ProjectAge> {
    let mut ages = Vec::new();
    // 設定ファイル自体が明示指定の根拠。lockfile の有無には依存しない。
    if let Some((duration, source)) = PnpmSettings::minimum_release_age_with_source(dir) {
        ages.push(ProjectAge {
            duration,
            source: dir.join(source),
        });
    }
    if let Some(duration) = BunSettings::from_dir(dir).minimum_release_age {
        ages.push(ProjectAge {
            duration,
            source: dir.join("bunfig.toml"),
        });
    }
    let mise = MiseSettings::from_dir(dir);
    if let Some(duration) = mise.minimum_release_age {
        ages.push(ProjectAge {
            duration,
            source: dir.join(mise.source.as_deref().unwrap_or("mise.toml")),
        });
    }
    ages
}

pub(super) fn project_scope_root(path: &Path) -> PathBuf {
    let root = absolute(path);
    for parent in root.ancestors() {
        let cargo = has_workspace_table(&parent.join("Cargo.toml"), &["workspace"]);
        let uv = has_workspace_table(&parent.join("pyproject.toml"), &["tool", "uv", "workspace"]);
        if parent.join("pnpm-workspace.yaml").is_file() || cargo || uv {
            return parent.to_path_buf();
        }
    }
    root
}

pub(super) fn cargo_workspace_root(path: &Path) -> PathBuf {
    let root = absolute(path);
    if has_workspace_table(&root.join("Cargo.toml"), &["workspace"]) {
        return root;
    }
    root.ancestors()
        .skip(1)
        .find(|parent| has_workspace_table(&parent.join("Cargo.toml"), &["workspace"]))
        .map(Path::to_path_buf)
        .unwrap_or(root)
}

fn has_workspace_table(path: &Path, keys: &[&str]) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(document) = toml::from_str::<toml::Value>(&content) else {
        return false;
    };
    keys.iter()
        .try_fold(&document, |value, key| value.get(*key))
        .is_some_and(toml::Value::is_table)
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn running_inside_a_workspace_keeps_parent_policy_and_lock_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let member = root.join("member");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = ['member']\n",
        )
        .unwrap();
        std::fs::write(root.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
        let mut args = CliArgs::parse_from(["depup", "--no-age"]);
        args.path = member.clone();
        assert_eq!(project_scope_root(&member), absolute(&root));
        assert_eq!(project_scope_root(&root), absolute(&root));
        assert_eq!(cargo_workspace_root(&member), absolute(&root));
        assert_eq!(
            resolve(&args, None, &member, Utc::now()).policy.min_age,
            Some(Duration::from_secs(30 * 86400))
        );
    }

    #[test]
    fn scopes_inherit_strictest_setting_without_crossing_siblings_or_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let app = root.join("app");
        let sibling = root.join("other");
        std::fs::create_dir_all(app.join("nested")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(temp.path().join(".npmrc"), "minimum-release-age=90d\n").unwrap();
        std::fs::write(app.join(".npmrc"), "minimum-release-age=30d\n").unwrap();
        let mut args = CliArgs::parse_from(["depup", "--no-age"]);
        args.path = root.clone();
        let config = GlobalConfig {
            age_exempt: AgeExemptions {
                github: vec!["example-dev".into()],
            },
            ..Default::default()
        };
        let scoped = resolve(&args, Some(&config), &app.join("nested"), Utc::now()).policy;
        assert_eq!(scoped.min_age, Some(Duration::from_secs(30 * 86400)));
        assert!(scoped.exemptions.is_empty());
        assert_eq!(
            resolve(&args, Some(&config), &sibling, Utc::now())
                .policy
                .min_age,
            None
        );
        assert!(
            !resolve(&args, Some(&config), &sibling, Utc::now())
                .policy
                .exemptions
                .is_empty()
        );
        std::fs::write(
            root.join("bunfig.toml"),
            "[install]\nminimumReleaseAge = 3456000\n",
        )
        .unwrap();
        assert_eq!(
            resolve(&args, Some(&config), &app, Utc::now())
                .policy
                .min_age,
            Some(Duration::from_secs(40 * 86400))
        );
    }

    #[test]
    fn explicit_pnpm_package_settings_do_not_require_a_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"pnpm":{"minimumReleaseAge":43200}}"#,
        )
        .unwrap();
        let mut args = CliArgs::parse_from(["depup", "--age", "1d"]);
        args.path = dir.path().into();
        assert_eq!(
            resolve(&args, None, dir.path(), Utc::now()).policy.min_age,
            Some(Duration::from_secs(30 * 86400))
        );
    }
}
