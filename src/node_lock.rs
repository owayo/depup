//! Node lock の実際の解決版を、実行開始時に固定した age cutoff で検査する。
//!
//! pnpm は公式 CLI の lockfile-only graph、npm は shrinkwrap/package-lock v2/v3 を読む。
//! マニフェストの候補を選ぶフィルタと分け、既存版・推移依存も検査する。

use crate::registry::{HttpClient, NpmAdapter};
use crate::update::AgePolicy;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use indicatif::ProgressBar;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;

const NODE_AUDIT_BUDGET: Duration = Duration::from_secs(180);
const PNPM_LIST_TIMEOUT: Duration = Duration::from_secs(60);
const PUBLICATION_CONCURRENCY: usize = 8;

type PublicationDates = HashMap<String, DateTime<Utc>>;

#[derive(Debug, Clone)]
pub struct NodeAgeViolation {
    pub name: String,
    pub version: String,
    pub released_at: DateTime<Utc>,
    pub cutoff: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeUnverifiedDependency {
    pub name: String,
    pub version: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct NodeLockAuditResult {
    pub checked: usize,
    pub violations: Vec<NodeAgeViolation>,
    pub unverified: Vec<NodeUnverifiedDependency>,
    pub errors: Vec<String>,
}

impl NodeLockAuditResult {
    pub fn has_failures(&self) -> bool {
        !self.violations.is_empty() || !self.unverified.is_empty() || !self.errors.is_empty()
    }

    /// verbose の有無に関わらず、残った版と確認できなかった理由を表示する。
    pub fn failure_messages(&self) -> Vec<String> {
        self.violations
            .iter()
            .map(|item| {
                format!(
                    "{} {} violates --age: published {}, cutoff {} (lockfile was not changed)",
                    item.name,
                    item.version,
                    item.released_at.to_rfc3339(),
                    item.cutoff.to_rfc3339()
                )
            })
            .chain(self.unverified.iter().map(|item| {
                format!(
                    "{} {} could not be checked against --age: {}",
                    item.name, item.version, item.reason
                )
            }))
            .chain(self.errors.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Default)]
struct LockSnapshot {
    packages: BTreeSet<(String, String)>,
    unverified: BTreeSet<NodeUnverifiedDependency>,
}

#[async_trait]
trait PublicationProvider: Sync {
    async fn fetch_dates(&self, package: &str) -> Result<PublicationDates, String>;
}

#[async_trait]
impl PublicationProvider for NpmAdapter {
    async fn fetch_dates(&self, package: &str) -> Result<PublicationDates, String> {
        self.fetch_publication_dates(package)
            .await
            .map_err(|error| error.to_string())
    }
}

/// age 無効時は lock を読まず、CLI も通信も起動しない。
pub async fn audit_node_lock(
    directory: &Path,
    package_manager: &str,
    policy: &AgePolicy,
    bar: Option<&ProgressBar>,
) -> NodeLockAuditResult {
    let Some(cutoff) = policy.cutoff() else {
        return NodeLockAuditResult::default();
    };
    let deadline = Instant::now() + NODE_AUDIT_BUDGET;
    let lock_name = match package_manager {
        "pnpm" => "pnpm-lock.yaml",
        "npm" => npm_lock_name(directory),
        _ => {
            return NodeLockAuditResult {
                errors: vec![format!(
                    "{package_manager} lockfile could not be checked against --age: lockfile audit is not supported"
                )],
                ..Default::default()
            };
        }
    };
    if let Some(bar) = bar {
        bar.set_message(format!("Reading {lock_name}"));
    }
    let lock_path = directory.join(lock_name);
    let original = match read_lock(&lock_path, deadline).await {
        Ok(bytes) => bytes,
        Err(error) => return audit_error(error),
    };
    let snapshot = match package_manager {
        "pnpm" => {
            let program = which::which_in("pnpm", std::env::var_os("PATH"), directory)
                .unwrap_or_else(|_| PathBuf::from("pnpm"));
            async {
                let mut snapshot = read_pnpm_graph(directory, &program, deadline).await?;
                verify_pnpm_registry_sources(directory, &program, &mut snapshot, deadline).await?;
                Ok::<_, String>(snapshot)
            }
            .await
        }
        _ => serde_json::from_slice::<Value>(&original)
            .map_err(|error| format!("invalid {lock_name}: {error}"))
            .and_then(|value| {
                let snapshot = parse_npm_snapshot(&value)?;
                verify_npm_manifest_dependencies(directory, &value)?;
                Ok(snapshot)
            }),
    };
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => return audit_error(error),
    };
    let client = match HttpClient::new() {
        Ok(client) => client,
        Err(error) => return audit_error(error.to_string()),
    };
    let mut result =
        audit_snapshot(snapshot, cutoff, &NpmAdapter::new(client), deadline, bar).await;
    match read_lock(&lock_path, deadline).await {
        Ok(bytes) if bytes == original => {}
        Ok(_) => result.errors.push(format!(
            "{lock_name} changed while --age was being checked; the final lockfile is unverified"
        )),
        Err(error) => result.errors.push(error),
    }
    result
}

fn npm_lock_name(directory: &Path) -> &'static str {
    match std::fs::symlink_metadata(directory.join("npm-shrinkwrap.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "package-lock.json",
        _ => "npm-shrinkwrap.json",
    }
}

/// list が integrity-only lock から合成する URL は取得元の証拠にならない。
/// 明示 tarball は graph の URL で検査し、合成 URL の取得元は実効 registry で検査する。
async fn verify_pnpm_registry_sources(
    directory: &Path,
    program: &Path,
    snapshot: &mut LockSnapshot,
    deadline: Instant,
) -> Result<(), String> {
    if snapshot.packages.is_empty() {
        return Ok(());
    }
    let default = read_pnpm_registry(directory, program, "registry", deadline)
        .await
        .and_then(|value| value.ok_or_else(|| "effective npm registry is unavailable".to_string()));
    let scopes: BTreeSet<String> = snapshot
        .packages
        .iter()
        .filter_map(|(name, _)| {
            name.strip_prefix('@')?
                .split_once('/')
                .map(|(scope, _)| format!("@{scope}:registry"))
        })
        .collect();
    let mut evidence = BTreeMap::new();
    for scope in scopes {
        let registry = read_pnpm_registry(directory, program, &scope, deadline)
            .await
            .and_then(|value| value.map(Ok).unwrap_or_else(|| default.clone()));
        evidence.insert(
            scope,
            registry.map(|registry| is_public_npm_registry(&registry)),
        );
    }
    let default = default.map(|registry| is_public_npm_registry(&registry));
    for (name, version) in std::mem::take(&mut snapshot.packages) {
        let source = name
            .strip_prefix('@')
            .and_then(|rest| rest.split_once('/'))
            .map(|(scope, _)| &evidence[&format!("@{scope}:registry")])
            .unwrap_or(&default);
        match source {
            Ok(true) => {
                snapshot.packages.insert((name, version));
            }
            Ok(false) => {
                snapshot.unverified.insert(NodeUnverifiedDependency {
                    name,
                    version,
                    reason: "effective registry is not the public npm registry".into(),
                });
            }
            Err(error) => {
                snapshot.unverified.insert(NodeUnverifiedDependency {
                    name,
                    version,
                    reason: error.clone(),
                });
            }
        }
    }
    Ok(())
}

async fn read_pnpm_registry(
    directory: &Path,
    program: &Path,
    key: &str,
    deadline: Instant,
) -> Result<Option<String>, String> {
    let mut command = tokio::process::Command::new(program);
    command
        .current_dir(directory)
        .args(["config", "get", key, "--json"])
        .kill_on_drop(true);
    let output = tokio::time::timeout_at(
        deadline.min(Instant::now() + Duration::from_secs(10)),
        command.output(),
    )
    .await
    .map_err(|_| "effective registry verification timed out".to_string())?
    .map_err(|error| format!("effective registry verification failed: {error}"))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err("effective registry configuration could not be verified".into());
    }
    match serde_json::from_slice::<Value>(&output.stdout)
        .map_err(|_| "effective registry configuration was not valid JSON".to_string())?
    {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err("effective registry configuration had an unsupported value".into()),
    }
}

fn is_public_npm_registry(registry: &str) -> bool {
    reqwest::Url::parse(registry).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && url.host_str() == Some("registry.npmjs.org")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn audit_error(error: String) -> NodeLockAuditResult {
    NodeLockAuditResult {
        errors: vec![error],
        ..Default::default()
    }
}

async fn read_lock(path: &Path, deadline: Instant) -> Result<Vec<u8>, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match tokio::time::timeout_at(deadline, tokio::fs::read(path)).await {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(format!("could not read {name} for --age audit: {error}")),
        Err(_) => Err(format!(
            "{name} --age audit timed out; the lockfile is unverified"
        )),
    }
}

async fn read_pnpm_graph(
    directory: &Path,
    program: &Path,
    deadline: Instant,
) -> Result<LockSnapshot, String> {
    let mut command = tokio::process::Command::new(program);
    command.current_dir(directory).args([
        "list",
        "--depth",
        "Infinity",
        "--lockfile-only",
        "--json",
    ]);
    if directory.join("pnpm-workspace.yaml").exists() {
        command.args(["--recursive", "--include-workspace-root"]);
    }
    command.kill_on_drop(true);
    let command_deadline = deadline.min(Instant::now() + PNPM_LIST_TIMEOUT);
    let output = tokio::time::timeout_at(command_deadline, command.output())
        .await
        .map_err(|_| "pnpm list --lockfile-only timed out; the lockfile is unverified".to_string())?
        .map_err(|error| format!("pnpm list --lockfile-only failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "pnpm list --lockfile-only failed ({}): {}",
            output.status,
            redact_command_output(&String::from_utf8_lossy(&output.stderr)).trim()
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid pnpm lockfile graph: {error}"))?;
    let snapshot = parse_pnpm_snapshot(&value)?;
    verify_pnpm_manifest_dependencies(directory, &value)?;
    if !output.stderr.is_empty() {
        return Err(format!(
            "pnpm list --lockfile-only emitted a warning; the lockfile is unverified: {}",
            redact_command_output(&String::from_utf8_lossy(&output.stderr)).trim()
        ));
    }
    Ok(snapshot)
}

/// list は manifest と食い違う空 importer にも exit 0 を返すため、直接依存の欠落を検査する。
/// optionalDependencies は platform 等の理由で無いことがあるので必須とはしない。
fn verify_pnpm_manifest_dependencies(directory: &Path, value: &Value) -> Result<(), String> {
    let projects = value
        .as_array()
        .ok_or_else(|| "invalid pnpm project graph".to_string())?;
    let root = directory
        .canonicalize()
        .map_err(|error| format!("could not locate pnpm audit directory: {error}"))?;
    let mut saw_root = false;
    for project in projects {
        let project_path = match project.get("path").and_then(Value::as_str) {
            Some(path) => PathBuf::from(path),
            None if projects.len() == 1 => root.clone(),
            None => return Err(
                "pnpm workspace graph omitted a project path; completeness could not be checked"
                    .into(),
            ),
        };
        let project_path = project_path
            .canonicalize()
            .map_err(|error| format!("could not locate pnpm graph project: {error}"))?;
        if !project_path.starts_with(&root) {
            return Err("pnpm graph references a project outside the audited workspace".into());
        }
        saw_root |= project_path == root;
        let manifest = std::fs::read(project_path.join("package.json"))
            .map_err(|error| format!("could not read pnpm graph project package.json: {error}"))?;
        let manifest: Value = serde_json::from_slice(&manifest)
            .map_err(|error| format!("invalid pnpm graph project package.json: {error}"))?;
        for section in ["dependencies", "devDependencies"] {
            let Some(required) = manifest.get(section) else {
                continue;
            };
            let required = required
                .as_object()
                .ok_or_else(|| format!("package.json has invalid {section}"))?;
            for name in required.keys() {
                if manifest
                    .get("optionalDependencies")
                    .and_then(Value::as_object)
                    .is_some_and(|optional| optional.contains_key(name))
                {
                    continue;
                }
                if !["dependencies", "devDependencies", "optionalDependencies"]
                    .iter()
                    .any(|section| {
                        project
                            .get(section)
                            .and_then(Value::as_object)
                            .is_some_and(|dependencies| dependencies.contains_key(name))
                    })
                {
                    return Err(format!(
                        "pnpm lockfile graph omitted required dependency {name}; the lockfile may be stale or unsupported"
                    ));
                }
            }
        }
    }
    if !saw_root {
        return Err(
            "pnpm graph omitted the workspace root; completeness could not be checked".into(),
        );
    }
    Ok(())
}

fn verify_npm_manifest_dependencies(directory: &Path, value: &Value) -> Result<(), String> {
    let manifest = std::fs::read(directory.join("package.json"))
        .map_err(|error| format!("could not read package.json for npm lockfile audit: {error}"))?;
    let manifest: Value = serde_json::from_slice(&manifest)
        .map_err(|error| format!("invalid package.json for npm lockfile audit: {error}"))?;
    let packages = value
        .get("packages")
        .and_then(Value::as_object)
        .ok_or_else(|| "npm lockfile has no valid packages table".to_string())?;
    verify_npm_required_dependencies("", &manifest, packages, true)?;
    for (path, package) in packages {
        if !package.is_object() {
            return Err(format!(
                "npm lockfile has an invalid package record at {path}"
            ));
        }
        if package.get("link").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        verify_npm_required_dependencies(
            path,
            package,
            packages,
            !path.split('/').any(|part| part == "node_modules"),
        )?;
    }
    Ok(())
}

fn verify_npm_required_dependencies(
    issuer: &str,
    package: &Value,
    packages: &serde_json::Map<String, Value>,
    include_dev: bool,
) -> Result<(), String> {
    for section in ["dependencies", "devDependencies"] {
        if section == "devDependencies" && !include_dev {
            continue;
        }
        let Some(required) = package.get(section) else {
            continue;
        };
        let required = required
            .as_object()
            .ok_or_else(|| format!("npm lockfile package {issuer} has invalid {section}"))?;
        for name in required.keys() {
            if package
                .get("optionalDependencies")
                .and_then(Value::as_object)
                .is_some_and(|optional| optional.contains_key(name))
            {
                continue;
            }
            resolve_npm_dependency(issuer, name, packages).ok_or_else(|| {
                format!("npm lockfile omitted required dependency {name} of {issuer}; the lockfile may be stale")
            })?;
            // npm owns range, override, and alias resolution. The audit checks the
            // actual lock package identity and version when collecting its source.
        }
    }
    Ok(())
}

/// Node resolution searches the closest node_modules first, then its ancestors.
/// Workspace links refer to another record in the same packages table.
fn resolve_npm_dependency<'a>(
    issuer: &str,
    name: &str,
    packages: &'a serde_json::Map<String, Value>,
) -> Option<&'a Value> {
    let mut current = issuer;
    loop {
        if current.rsplit('/').next() != Some("node_modules") {
            let candidate = if current.is_empty() {
                format!("node_modules/{name}")
            } else {
                format!("{current}/node_modules/{name}")
            };
            if let Some(mut dependency) = packages.get(&candidate) {
                let mut seen = BTreeSet::new();
                while dependency.get("link").and_then(Value::as_bool) == Some(true) {
                    let target = dependency.get("resolved").and_then(Value::as_str)?;
                    if !seen.insert(target) {
                        return None;
                    }
                    dependency = packages.get(target)?;
                }
                return Some(dependency);
            }
        }
        if current.is_empty() {
            return None;
        }
        current = current
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
    }
}

fn redact_command_output(output: &str) -> String {
    static URLS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s\"'<>]+"#).unwrap()
    });
    URLS.replace_all(output, |captures: &regex::Captures<'_>| {
        crate::registry::redact_url(&captures[0])
    })
    .into_owned()
}

fn parse_pnpm_snapshot(value: &Value) -> Result<LockSnapshot, String> {
    let projects = value
        .as_array()
        .filter(|projects| !projects.is_empty())
        .ok_or_else(|| "pnpm lockfile graph contains no project records".to_string())?;
    let mut snapshot = LockSnapshot::default();
    let mut pending: Vec<&Value> = projects.iter().collect();
    while let Some(parent) = pending.pop() {
        if !parent.is_object() {
            return Err("pnpm lockfile graph has an invalid project or dependency record".into());
        }
        for section in ["dependencies", "devDependencies", "optionalDependencies"] {
            let Some(dependencies) = parent.get(section) else {
                continue;
            };
            let dependencies = dependencies
                .as_object()
                .ok_or_else(|| format!("pnpm lockfile graph has invalid {section}"))?;
            for (alias, dependency) in dependencies {
                collect_package(&mut snapshot, alias, dependency);
                pending.push(dependency);
            }
        }
    }
    Ok(snapshot)
}

fn parse_npm_snapshot(value: &Value) -> Result<LockSnapshot, String> {
    if !matches!(
        value.get("lockfileVersion").and_then(Value::as_u64),
        Some(2 | 3)
    ) {
        return Err("npm lockfile --age audit requires lockfileVersion 2 or 3".into());
    }
    let packages = value
        .get("packages")
        .and_then(Value::as_object)
        .ok_or_else(|| "npm lockfile has no valid packages table".to_string())?;
    if !packages.get("").is_some_and(Value::is_object) {
        return Err("npm lockfile omitted its root package record".into());
    }
    let mut snapshot = LockSnapshot::default();
    for (path, package) in packages {
        let Some((_, alias)) = path.rsplit_once("node_modules/") else {
            continue;
        };
        if package.get("link").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        collect_package(&mut snapshot, alias, package);
    }
    Ok(snapshot)
}

fn collect_package(snapshot: &mut LockSnapshot, alias: &str, package: &Value) {
    let version = package
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let resolved = package.get("resolved").and_then(Value::as_str);
    if resolved.is_some_and(is_non_registry_source) || is_non_registry_source(version) {
        return;
    }
    if let Some(name) = resolved.and_then(|source| public_tarball_package(source, version)) {
        snapshot.packages.insert((name, version.to_string()));
    } else {
        snapshot.unverified.insert(NodeUnverifiedDependency {
            name: alias.to_string(),
            version: version.to_string(),
            reason: "release source is not a verified public npm registry tarball".into(),
        });
    }
}

fn is_non_registry_source(source: &str) -> bool {
    [
        "link:",
        "file:",
        "workspace:",
        "git+",
        "git:",
        "github:",
        "git@",
    ]
    .iter()
    .any(|prefix| source.starts_with(prefix))
}

/// 名前は alias/from ではなく、公開 npm の tarball URL と完全版の一致から決める。
fn public_tarball_package(source: &str, version: &str) -> Option<String> {
    semver::Version::parse(version).ok()?;
    let url = reqwest::Url::parse(source).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("registry.npmjs.org")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let path = url
        .path()
        .replace("%2f", "/")
        .replace("%2F", "/")
        .replace("%40", "@")
        .replace("%2B", "+")
        .replace("%2b", "+");
    let (name, tarball) = path.strip_prefix('/')?.split_once("/-/")?;
    let basename = name.rsplit('/').next()?;
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~')
            })
    };
    let valid_name = match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, name)| valid_segment(scope) && valid_segment(name)),
        None => valid_segment(name),
    };
    (valid_name && tarball == format!("{basename}-{version}.tgz")).then(|| name.to_string())
}

async fn audit_snapshot(
    snapshot: LockSnapshot,
    cutoff: DateTime<Utc>,
    provider: &impl PublicationProvider,
    deadline: Instant,
    bar: Option<&ProgressBar>,
) -> NodeLockAuditResult {
    let mut result = NodeLockAuditResult {
        unverified: snapshot.unverified.into_iter().collect(),
        ..Default::default()
    };
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, version) in snapshot.packages {
        groups.entry(name).or_default().push(version);
    }
    if let Some(bar) = bar {
        bar.set_length(groups.len() as u64);
        bar.set_position(0);
    }
    let mut requests = stream::iter(groups)
        .map(|(name, versions)| async move {
            if let Some(bar) = bar {
                bar.set_message(format!("Checking npm {name}"));
            }
            let dates = if Instant::now() >= deadline {
                Err("lockfile age audit timed out".into())
            } else {
                tokio::time::timeout_at(deadline, provider.fetch_dates(&name))
                    .await
                    .unwrap_or_else(|_| Err("lockfile age audit timed out".into()))
            };
            (name, versions, dates)
        })
        .buffer_unordered(PUBLICATION_CONCURRENCY);
    while let Some((name, versions, dates)) = requests.next().await {
        for version in versions {
            match &dates {
                Ok(dates) => match dates.get(&version) {
                    Some(&released_at) => {
                        result.checked += 1;
                        if released_at > cutoff {
                            result.violations.push(NodeAgeViolation {
                                name: name.clone(),
                                version,
                                released_at,
                                cutoff,
                            });
                        }
                    }
                    None => result.unverified.push(NodeUnverifiedDependency {
                        name: name.clone(),
                        version,
                        reason: "release date unavailable".into(),
                    }),
                },
                Err(error) => result.unverified.push(NodeUnverifiedDependency {
                    name: name.clone(),
                    version,
                    reason: error.clone(),
                }),
            }
        }
        if let Some(bar) = bar {
            bar.inc(1);
        }
    }
    result
        .violations
        .sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    result.unverified.sort();
    result
}

#[cfg(test)]
mod tests;
