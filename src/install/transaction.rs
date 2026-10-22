//! 管理するファイルを共有するジョブの単位で、install の失敗から復元する。
use super::{InstallPlan, RustAuditPlan};
use crate::domain::{Language, UpdateSummary};
use crate::file_snapshot::{
    FileGuard, FileState, resolved_path, save_recovery_copy as backup, write_checked,
};
use crate::manifest::detect_manifests;
use crate::node_scope::node_resolution_directory;
use crate::orchestrator::OrchestratorResult;
use crate::package_manager::{InstallResult, InstallScope, PreparedInstall, SystemPackageManager};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

struct FileRecord {
    guards: Vec<FileGuard>,
    before: FileState,
    expected: FileState,
    conflict: Option<String>,
    parent_permissions: Vec<(PathBuf, fs::Permissions)>,
}
struct Task {
    command: Result<Option<PreparedInstall>, String>,
    label: PathBuf,
    files: BTreeSet<PathBuf>,
    scope_paths: BTreeSet<PathBuf>,
    managed: BTreeSet<PathBuf>,
    summaries: BTreeSet<usize>,
    rust_root: Option<PathBuf>,
    node_root: Option<PathBuf>,
    trees: Vec<FileGuard>,
    tree_links: BTreeMap<PathBuf, FileGuard>,
    mise_locks: Option<mise_lock::Preservation>,
}
struct Unit {
    files: BTreeSet<PathBuf>,
    summaries: BTreeSet<usize>,
    directories: BTreeSet<PathBuf>,
    failure: Option<String>,
    policy_failure: bool,
}

/// コマンドの準備とスナップショットの作成は、PM の実行前に一度だけ行う。
pub struct InstallTransactions {
    tasks: Vec<Task>,
    units: Vec<Unit>,
    task_units: Vec<usize>,
    files: BTreeMap<PathBuf, FileRecord>,
    node_managers: BTreeMap<PathBuf, (PathBuf, &'static str)>,
    recovery_boundary: PathBuf,
}

#[derive(Debug)]
pub enum FileOutcome {
    Unchanged,
    Restored,
    Removed,
    Conflict {
        reason: String,
        backup: Result<PathBuf, String>,
    },
    Failed {
        reason: String,
        backup: Result<PathBuf, String>,
    },
}
impl FileOutcome {
    fn restored(&self) -> bool {
        matches!(self, Self::Unchanged | Self::Restored | Self::Removed)
    }
}
struct FailedUnit {
    directories: BTreeSet<PathBuf>,
    summaries: BTreeSet<usize>,
    reason: String,
    files: BTreeMap<PathBuf, FileOutcome>,
    policy_failure: bool,
}
impl FailedUnit {
    /// 管理するファイルがすべて実行前の状態へ戻ったかを表す。
    fn recovered(&self) -> bool {
        self.files.values().all(FileOutcome::restored)
    }
}

#[derive(Default)]
pub struct RollbackReport {
    units: Vec<FailedUnit>,
    node_managers: BTreeMap<PathBuf, (PathBuf, &'static str)>,
}

impl InstallTransactions {
    pub fn begin(plan: &InstallPlan, result: &OrchestratorResult, boundary: &Path) -> Self {
        let pm = SystemPackageManager::new();
        let mut tx = Self {
            tasks: vec![],
            units: vec![],
            task_units: vec![],
            files: BTreeMap::new(),
            node_managers: BTreeMap::new(),
            recovery_boundary: identity(boundary),
        };
        for job in &plan.node_audits {
            if let Some(manager) = pm.resolve_package_manager(job.language, &job.directory) {
                tx.node_managers.insert(identity(&job.directory), manager);
            }
        }
        // 後で保存する更新済みのマニフェストの状態を、書き込み記録で上書きする。
        for write in &result.write_results {
            if let Some(journal) = &write.journal {
                let before = FileState {
                    bytes: Some(journal.before.clone()),
                    permissions: Some(journal.permissions.clone()),
                };
                let expected = FileState {
                    bytes: Some(journal.written.clone()),
                    permissions: Some(journal.permissions.clone()),
                };
                tx.files
                    .entry(journal.target.clone())
                    .and_modify(|record| {
                        if !record.expected.matches(&before) {
                            record.conflict = Some("Manifest changed between writes".into());
                        }
                        record.expected = expected.clone();
                        record.guards.push(journal.guard.clone());
                    })
                    .or_insert(FileRecord {
                        parent_permissions: parent_permissions(&journal.target),
                        guards: vec![journal.guard.clone()],
                        before,
                        expected,
                        conflict: None,
                    });
            }
        }
        for (directory, version) in &plan.wrapper_updates {
            // 単位は共有しつつ、各回で個別に観測の境界を置く。
            for _ in 0..2 {
                let command = pm.prepare_gradle_wrapper(directory, version).map(Some);
                let mut task = Task::new(directory, command);
                for name in [
                    "gradle/wrapper/gradle-wrapper.properties",
                    "gradle/wrapper/gradle-wrapper.jar",
                    "gradlew",
                    "gradlew.bat",
                ] {
                    tx.add_path(&mut task, &directory.join(name), true);
                }
                task.summaries.extend(
                    result
                        .summary
                        .manifests
                        .iter()
                        .enumerate()
                        .filter(|(_, manifest)| {
                            manifest.path
                                == directory.join("gradle/wrapper/gradle-wrapper.properties")
                        })
                        .map(|(index, _)| index),
                );
                tx.tasks.push(task);
            }
        }
        let mut cargo_roots = BTreeMap::new();
        let mut uv_roots = BTreeMap::new();
        for job in &plan.jobs {
            // 選択結果と確定したファイルの関係は、ポリシーや引数の妥当性に依存しない。
            let selection = match &job.scope {
                Ok(scope) => pm.resolve_scoped_package_manager(job.language, &job.directory, scope),
                Err(_) if job.language != Language::Rust => {
                    pm.resolve_package_manager(job.language, &job.directory)
                }
                Err(_) => None,
            };
            let command = job
                .policy
                .as_ref()
                .and_then(|policy| job.scope.as_ref().map(|scope| (policy, scope)))
                .map_err(Clone::clone)
                .and_then(|(policy, scope)| match &selection {
                    Some((directory, manager)) => {
                        pm.prepare_resolved_install(job.language, directory, manager, policy, scope)
                    }
                    None => Ok(None),
                });
            let mut task = Task::new(&job.directory, command);
            let directory = selection
                .as_ref()
                .map_or(&job.directory, |(directory, _)| directory)
                .clone();
            if job.language == Language::Rust {
                let root = if matches!(job.scope, Ok(InstallScope::Full)) && selection.is_some() {
                    cargo_roots
                        .entry(identity(&directory.join("Cargo.toml")))
                        .or_insert_with(|| {
                            crate::cargo_scope::rust_resolution_directory(
                                &directory.join("Cargo.toml"),
                                boundary,
                            )
                        })
                        .clone()
                } else {
                    Ok(directory.clone())
                };
                match root {
                    Ok(root) => {
                        task.rust_root = Some(root.clone());
                        tx.add_path(&mut task, &root.join("Cargo.lock"), true);
                    }
                    Err(error) => task.command = Err(error),
                }
            }
            if job.language == Language::Node {
                task.node_root = Some(directory.clone());
                if matches!(job.scope, Ok(InstallScope::Refresh)) {
                    tx.add_path(&mut task, &directory.join("pnpm-workspace.yaml"), true);
                }
            }
            for (index, manifest) in result.summary.manifests.iter().enumerate() {
                let applies = if job.language == Language::Rust
                    && task.rust_root.is_some()
                    && matches!(job.scope, Ok(InstallScope::Full))
                {
                    manifest.language == Language::Rust
                        && cargo_roots
                            .entry(identity(&manifest.path))
                            .or_insert_with(|| {
                                crate::cargo_scope::rust_resolution_directory(
                                    &manifest.path,
                                    boundary,
                                )
                            })
                            .as_ref()
                            .is_ok_and(|root| Some(root) == task.rust_root.as_ref())
                } else {
                    job.manifests.contains(&manifest.path)
                };
                if applies {
                    task.summaries.insert(index);
                    tx.add_path(&mut task, &manifest.path, false);
                }
            }
            if let Some((effective_dir, manager)) = selection {
                if job.language == Language::Node {
                    tx.node_managers
                        .insert(identity(&job.directory), (effective_dir.clone(), manager));
                }
                if manager == "uv" {
                    match uv_roots
                        .entry(identity(&effective_dir))
                        .or_insert_with(|| scope::uv_root(&effective_dir, boundary))
                        .clone()
                    {
                        Ok(scope) => {
                            for name in ["pyproject.toml", "uv.lock"] {
                                tx.add_scoped_path(&mut task, &scope.root.join(name));
                            }
                            // uv は、各メンバーで選ばれたコマンドが pip である場合や準備に失敗した場合も含め、
                            // 確定したすべてのメンバーの要件を解決する。
                            for (index, manifest) in result
                                .summary
                                .manifests
                                .iter()
                                .enumerate()
                                .filter(|(_, manifest)| manifest.language == Language::Python)
                            {
                                let Some(parent) = manifest.path.parent() else {
                                    continue;
                                };
                                // 除外や非メンバーと確定した場合、子の設定を解析する必要はない。
                                if !scope.members.contains(&identity(parent)) {
                                    continue;
                                }
                                match uv_roots
                                    .entry(identity(parent))
                                    .or_insert_with(|| scope::uv_root(parent, boundary))
                                    .as_ref()
                                {
                                    Ok(member_scope) if member_scope.root == scope.root => {
                                        task.summaries.insert(index);
                                        tx.add_path(&mut task, &manifest.path, false);
                                    }
                                    Err(error) => {
                                        task.command = Err(error.clone());
                                        task.summaries.insert(index);
                                        tx.add_path(&mut task, &manifest.path, false);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(error) => task.command = Err(error),
                    }
                }
                if manager == "mise" {
                    match mise::managed_paths(&effective_dir, &job.manifests, boundary) {
                        Ok(scope) => {
                            for config in &scope.configs {
                                tx.add_guarded_path(
                                    &mut task,
                                    config,
                                    false,
                                    Some(&identity(boundary)),
                                );
                            }
                            // このマニフェスト自身のジョブの準備に失敗しても、別のジョブによって、
                            // 共有コマンドが読む設定を確定できる場合がある。
                            for (index, manifest) in result.summary.manifests.iter().enumerate() {
                                if manifest.language == Language::Mise
                                    && scope.configs.contains(&identity(&manifest.path))
                                {
                                    task.summaries.insert(index);
                                    tx.add_guarded_path(
                                        &mut task,
                                        &manifest.path,
                                        false,
                                        Some(&tx.recovery_boundary.clone()),
                                    );
                                }
                            }
                            for path in scope.paths {
                                if !tx.add_scoped_path(&mut task, &path) {
                                    continue;
                                }
                                let tree = mise::sidecar(&path);
                                match FileGuard::capture(&tree).and_then(|guard| {
                                    let paths =
                                        mise::tree_files(&guard, &tx.recovery_boundary, None)?;
                                    Ok((guard, paths))
                                }) {
                                    Ok((guard, paths)) => {
                                        if !paths.errors.is_empty() {
                                            task.command = Err(format!(
                                                "Cannot snapshot mise metadata: {}",
                                                paths.errors.join("; ")
                                            ));
                                        }
                                        for path in paths.paths {
                                            tx.add_scoped_path(&mut task, &path);
                                        }
                                        task.tree_links.extend(paths.links);
                                        task.trees.push(guard);
                                    }
                                    Err(error) => {
                                        task.command =
                                            Err(format!("Cannot snapshot mise metadata: {error}"))
                                    }
                                }
                            }
                            match mise_lock::Preservation::capture(scope.locks, &tx.files) {
                                Ok(preservation) => task.mise_locks = Some(preservation),
                                Err(error) => task.command = Err(error),
                            }
                        }
                        Err(error) => task.command = Err(error),
                    }
                }
                if job.language == Language::Java {
                    let root = gradle_root(&effective_dir, boundary);
                    for name in [
                        "gradle/wrapper/gradle-wrapper.properties",
                        "gradle/wrapper/gradle-wrapper.jar",
                        "gradlew",
                        "gradlew.bat",
                    ] {
                        tx.add_path(&mut task, &root.join(name), true);
                    }
                }
                for path in managed_files(manager, &effective_dir) {
                    tx.add_path(&mut task, &path, true);
                }
                if job.language == Language::Node {
                    // 変更のないマニフェストも含め、実際の pnpm メンバーを対象にする。
                    for manifest in detect_manifests(&directory).into_iter().filter(|manifest| {
                        manifest.language == Language::Node
                            && node_resolution_directory(&manifest.path) == directory
                    }) {
                        tx.add_path(&mut task, &manifest.path, true);
                    }
                }
            }
            tx.tasks.push(task);
        }
        tx.group(result);
        tx
    }

    fn add_path(&mut self, task: &mut Task, path: &Path, managed: bool) {
        self.add_guarded_path(task, path, managed, None);
    }

    fn add_guarded_path(
        &mut self,
        task: &mut Task,
        path: &Path,
        managed: bool,
        boundary: Option<&Path>,
    ) -> bool {
        match FileGuard::capture(path).and_then(|guard| {
            if let Some(boundary) = boundary {
                if !guard.target.starts_with(boundary) {
                    return Err(io::Error::other(
                        "Managed file resolves outside the permitted recovery boundary",
                    ));
                }
                task.scope_paths.insert(guard.target.clone());
            }
            let state = guard.read()?;
            Ok((guard, state))
        }) {
            Ok((guard, state)) => {
                let key = guard.target.clone();
                task.files.insert(key.clone());
                if managed {
                    task.managed.insert(key.clone());
                }
                self.files
                    .entry(key)
                    .and_modify(|record| {
                        if !record.guards.iter().any(|known| known.alias == guard.alias) {
                            record.guards.push(guard.clone());
                        }
                        if !record.expected.matches(&state) {
                            record.conflict = Some("File changed after the manifest write".into());
                        }
                    })
                    .or_insert(FileRecord {
                        parent_permissions: parent_permissions(&guard.target),
                        guards: vec![guard],
                        before: state.clone(),
                        expected: state,
                        conflict: None,
                    });
                true
            }
            Err(error) => {
                task.command = Err(format!("Cannot snapshot {}: {error}", path.display()));
                // ほかの管理対象の保存が失敗しても、書き込み記録から復元できる場合がある。
                if let Ok(key) = resolved_path(path)
                    && self.files.contains_key(&key)
                {
                    task.files.insert(key);
                }
                false
            }
        }
    }

    fn add_scoped_path(&mut self, task: &mut Task, path: &Path) -> bool {
        let boundary = self.recovery_boundary.clone();
        let logical = path
            .parent()
            .zip(path.file_name())
            .ok_or_else(|| io::Error::other("Managed file has no parent or filename"))
            .and_then(|(parent, name)| resolved_path(parent).map(|parent| parent.join(name)));
        let logical = match logical {
            Ok(logical) => logical,
            Err(error) => {
                task.command = Err(format!("Cannot locate managed scope: {error}"));
                return false;
            }
        };
        if !logical.starts_with(&boundary) {
            task.command = Err("Managed scope is outside the permitted recovery boundary".into());
            return false;
        }
        // lock の場所の特定や保存に失敗しても、確定した関係は保持する。
        task.scope_paths.insert(logical);
        self.add_guarded_path(task, path, true, Some(&boundary))
    }

    fn group(&mut self, result: &OrchestratorResult) {
        let mut parent: Vec<usize> = (0..self.tasks.len()).collect();
        for left in 0..self.tasks.len() {
            for right in left + 1..self.tasks.len() {
                let a = &self.tasks[left];
                let b = &self.tasks[right];
                let shared = !a.files.is_disjoint(&b.files)
                    || !a.scope_paths.is_disjoint(&b.scope_paths)
                    || !a.summaries.is_disjoint(&b.summaries)
                    || a.rust_root
                        .as_ref()
                        .is_some_and(|root| b.rust_root.as_ref() == Some(root))
                    || a.node_root
                        .as_ref()
                        .is_some_and(|root| b.node_root.as_ref() == Some(root))
                    || a.trees
                        .iter()
                        .any(|tree| b.trees.iter().any(|other| tree.target == other.target));
                let tauri = a.summaries.iter().any(|&a_index| {
                    b.summaries.iter().any(|&b_index| {
                        let a = &result.summary.manifests[a_index];
                        let b = &result.summary.manifests[b_index];
                        tauri_pair(&a.path, a.language, &b.path, b.language)
                    })
                });
                if shared || tauri {
                    let a = root(&parent, left);
                    let b = root(&parent, right);
                    parent[b] = a;
                }
            }
        }
        let mut indices = BTreeMap::new();
        for index in 0..self.tasks.len() {
            let representative = root(&parent, index);
            let unit = *indices.entry(representative).or_insert_with(|| {
                self.units.push(Unit {
                    files: BTreeSet::new(),
                    summaries: BTreeSet::new(),
                    directories: BTreeSet::new(),
                    failure: None,
                    policy_failure: false,
                });
                self.units.len() - 1
            });
            self.task_units.push(unit);
            let task = &self.tasks[index];
            let unit = &mut self.units[unit];
            unit.files.extend(task.files.iter().cloned());
            unit.summaries.extend(&task.summaries);
            unit.directories.insert(identity(&task.label));
            unit.directories
                .insert(std::path::absolute(&task.label).unwrap_or_else(|_| task.label.clone()));
            for &summary in &task.summaries {
                if let Some(parent) = result.summary.manifests[summary].path.parent() {
                    unit.directories.insert(identity(parent));
                    unit.directories
                        .insert(std::path::absolute(parent).unwrap_or_else(|_| parent.into()));
                }
            }
            if let Some(root) = &task.rust_root {
                unit.directories.insert(identity(root));
            }
            if let Some(root) = &task.node_root {
                unit.directories.insert(identity(root));
            }
            if let Err(error) = &task.command {
                unit.failure.get_or_insert(error.clone());
            }
        }
    }

    /// 失敗した単位のコマンドは飛ばし、独立した単位の処理は続ける。
    pub fn commands(&self) -> impl Iterator<Item = &PreparedInstall> {
        self.tasks
            .iter()
            .filter_map(|task| task.command.as_ref().ok().and_then(Option::as_ref))
    }

    pub fn execute(&mut self, mut runner: impl FnMut(&PreparedInstall) -> InstallResult) {
        for index in 0..self.tasks.len() {
            self.execute_task(index, None, &mut runner);
        }
    }

    fn execute_task(
        &mut self,
        index: usize,
        replacement: Option<PreparedInstall>,
        runner: &mut impl FnMut(&PreparedInstall) -> InstallResult,
    ) {
        let unit = self.task_units[index];
        if self.units[unit].failure.is_some() {
            return;
        }
        // パッケージマネージャーの実行が不要でも、変更の所有関係は確認する必要がある。
        self.observe_trees(index, false);
        self.observe(unit, &BTreeSet::new());
        if self.units[unit].failure.is_some() {
            return;
        }
        let Some(command) = self.tasks[index]
            .command
            .as_ref()
            .ok()
            .and_then(|command| command.clone())
            .map(|command| replacement.unwrap_or(command))
        else {
            return;
        };
        let result = runner(&command);
        self.observe_trees(index, true);
        let managed = self.tasks[index].managed.clone();
        self.observe(unit, &managed);
        if !result.success {
            self.units[unit].failure.get_or_insert_with(|| {
                format!("{} failed: {}", result.command, result.stderr.trim())
            });
        }
        if self.units[unit].failure.is_none() {
            self.repair_mise_locks(index, &command, runner);
        }
    }

    pub(super) fn materialize_nodes(
        &mut self,
        mut runner: impl FnMut(&PreparedInstall) -> InstallResult,
    ) {
        for index in 0..self.tasks.len() {
            let Some(command) = self.tasks[index]
                .command
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
            else {
                continue;
            };
            if command.package_manager != "pnpm"
                || command.argv.get(1).map(String::as_str) != Some("update")
            {
                continue;
            }
            let mut command = command.clone();
            command.argv = vec!["pnpm".into(), "install".into(), "--frozen-lockfile".into()];
            self.execute_task(index, Some(command), &mut runner);
        }
    }

    pub(super) fn pending_report(&self) -> RollbackReport {
        RollbackReport {
            units: self
                .units
                .iter()
                .filter_map(|unit| {
                    unit.failure.as_ref().map(|reason| FailedUnit {
                        directories: unit.directories.clone(),
                        summaries: unit.summaries.clone(),
                        reason: reason.clone(),
                        files: BTreeMap::new(),
                        policy_failure: unit.policy_failure,
                    })
                })
                .collect(),
            node_managers: self.node_managers.clone(),
        }
    }

    pub(super) fn declaration_snapshots(&self) -> Vec<(PathBuf, (FileGuard, FileState))> {
        let mut out = Vec::new();
        for unit in &self.units {
            let Some(directory) = unit.directories.first() else {
                continue;
            };
            for key in &unit.files {
                if !key.file_name().is_some_and(|name| {
                    name == "Cargo.toml" || name == "package.json" || name == "pnpm-workspace.yaml"
                }) {
                    continue;
                }
                let record = &self.files[key];
                if let Some(guard) = record.guards.first() {
                    out.push((directory.clone(), (guard.clone(), record.expected.clone())));
                }
            }
        }
        out
    }

    /// ファイルが準備済みの install 単位に含まれるか。監査だけの lock は含まれない。
    pub(super) fn manages(&self, path: &Path) -> bool {
        resolved_path(path).is_ok_and(|key| self.files.contains_key(&key))
    }

    /// この lockfile またはマニフェストを保持する単位を失敗にする。[`Self::fail_verification`] と違い、
    /// 同じディレクトリの独立した install（Cargo の隣の uv や Go）には影響しない。
    pub(super) fn fail_holder(&mut self, path: &Path, reason: String) {
        let key = identity(path);
        for unit in &mut self.units {
            if unit.files.contains(&key) && unit.failure.is_none() {
                unit.failure = Some(reason.clone());
                unit.policy_failure = true;
            }
        }
    }

    pub(super) fn fail_verification(&mut self, directory: &Path, reason: String) {
        let directory = identity(directory);
        for unit in &mut self.units {
            if unit.directories.contains(&directory) && unit.failure.is_none() {
                unit.failure = Some(reason.clone());
                unit.policy_failure = true;
            }
        }
    }

    /// 監査が保証したバイト列だけを採用する。後からディスクを読んだ内容は所有関係の証拠にならない。
    pub(super) fn accept_audit_lock(
        &mut self,
        path: &Path,
        before: &[u8],
        after: &[u8],
    ) -> Result<(), String> {
        let key = resolved_path(path).map_err(|error| error.to_string())?;
        let record = self
            .files
            .get_mut(&key)
            .ok_or("lockfile is outside the prepared transaction")?;
        if record.conflict.is_some() || record.expected.bytes.as_deref() != Some(before) {
            return Err("lockfile changed before the tracked audit".into());
        }
        let expected = FileState {
            bytes: Some(after.to_vec()),
            permissions: record.expected.permissions.clone(),
        };
        let current = read_record(record).map_err(|error| error.to_string())?;
        if !expected.matches(&current) {
            record.conflict = Some("lockfile changed after the tracked audit".into());
            return Err("lockfile changed after the tracked audit".into());
        }
        record.expected = expected;
        Ok(())
    }

    pub(super) fn check_lock(&self, path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
        let key = resolved_path(path).map_err(|error| error.to_string())?;
        let record = self
            .files
            .get(&key)
            .ok_or("lockfile is outside the prepared transaction")?;
        if record.conflict.is_some() || record.expected.bytes.as_deref() != bytes {
            return Err("lockfile differs from the tracked resolution".into());
        }
        let current = read_record(record).map_err(|error| error.to_string())?;
        if !record.expected.matches(&current) {
            return Err("lockfile changed outside a tracked operation".into());
        }
        Ok(())
    }

    fn repair_mise_locks(
        &mut self,
        index: usize,
        command: &PreparedInstall,
        runner: &mut impl FnMut(&PreparedInstall) -> InstallResult,
    ) {
        let Some(preservation) = self.tasks[index].mise_locks.as_ref() else {
            return;
        };
        let unit = self.task_units[index];
        let mut repair = match preservation.prepare(&self.files, command) {
            Ok(repair) => repair,
            Err(error) => {
                self.units[unit].failure = Some(error);
                return;
            }
        };
        for command in repair.commands.clone() {
            self.observe_trees(index, false);
            self.observe(unit, &BTreeSet::new());
            if self.units[unit].failure.is_some() {
                return;
            }
            let result = runner(&command);
            self.observe_trees(index, true);
            let managed = self.tasks[index].managed.clone();
            self.observe(unit, &managed);
            if !result.success {
                self.units[unit].failure.get_or_insert_with(|| {
                    format!(
                        "Mise platform lock regeneration failed: {}",
                        result.stderr.trim()
                    )
                });
            }
            if self.units[unit].failure.is_some() {
                return;
            }
            if let Err(error) = repair.checkpoint(&self.files) {
                self.units[unit].failure = Some(error);
                return;
            }
        }
        if let Err(error) = repair.validate(&self.files) {
            self.units[unit].failure = Some(error);
        }
    }

    fn observe_trees(&mut self, task: usize, owned: bool) {
        let unit = self.task_units[task];
        for tree in self.tasks[task].trees.clone() {
            let paths = match mise::tree_files(
                &tree,
                &self.recovery_boundary,
                Some(&self.tasks[task].tree_links),
            ) {
                Ok(paths) => paths,
                Err(error) => {
                    self.units[unit].failure.get_or_insert(format!(
                        "Mise metadata changed or became unreadable: {error}"
                    ));
                    continue;
                }
            };
            if !paths.errors.is_empty() {
                self.units[unit].failure.get_or_insert_with(|| {
                    format!(
                        "Mise metadata changed or became unreadable: {}",
                        paths.errors.join("; ")
                    )
                });
            }
            for path in paths.paths {
                let key = identity(&path);
                if self.files.contains_key(&key) {
                    continue;
                }
                match FileGuard::capture(&path).and_then(|guard| {
                    if !guard.target.starts_with(&self.recovery_boundary) {
                        return Err(io::Error::other(
                            "Mise metadata resolves outside the permitted recovery boundary",
                        ));
                    }
                    let state = guard.read()?;
                    Ok((guard, state))
                }) {
                    Ok((guard, state)) => {
                        if state.bytes.is_none() {
                            continue;
                        }
                        let conflict = if !owned {
                            Some("Mise metadata was created outside an install command".into())
                        } else if fs::symlink_metadata(&path)
                            .is_ok_and(|metadata| metadata.file_type().is_symlink())
                        {
                            Some("New mise metadata is a symlink; kept for manual recovery".into())
                        } else {
                            None
                        };
                        if conflict.is_some() {
                            self.units[unit]
                                .failure
                                .get_or_insert("External mise metadata change detected".into());
                        }
                        self.files.insert(
                            key.clone(),
                            FileRecord {
                                parent_permissions: vec![],
                                guards: vec![guard],
                                before: FileState {
                                    bytes: None,
                                    permissions: None,
                                },
                                expected: state,
                                conflict,
                            },
                        );
                        self.units[unit].files.insert(key.clone());
                        self.tasks[task].managed.insert(key.clone());
                        self.tasks[task].files.insert(key);
                    }
                    Err(error) => {
                        self.units[unit]
                            .failure
                            .get_or_insert(format!("Cannot observe mise metadata: {error}"));
                    }
                }
            }
        }
    }

    fn observe(&mut self, unit: usize, owned: &BTreeSet<PathBuf>) {
        for key in &self.units[unit].files.clone() {
            let record = self.files.get_mut(key).unwrap();
            if record.conflict.is_some() {
                self.units[unit]
                    .failure
                    .get_or_insert("External file change detected".into());
                continue;
            }
            match read_record(record) {
                Ok(state) if owned.contains(key) => record.expected = state,
                Ok(state) if record.expected.matches(&state) => {}
                Ok(_) => {
                    record.conflict = Some("File changed outside a package-manager command".into());
                }
                Err(error) => {
                    record.conflict = Some(error.to_string());
                }
            }
            if record.conflict.is_some() {
                self.units[unit]
                    .failure
                    .get_or_insert("External file change detected".into());
            }
        }
    }

    pub fn rollback(self) -> RollbackReport {
        self.rollback_with(|_, record| restore_record(record))
    }

    fn rollback_with(
        mut self,
        mut restore: impl FnMut(&Path, &FileRecord) -> FileOutcome,
    ) -> RollbackReport {
        let mut report = RollbackReport::default();
        for index in 0..self.units.len() {
            let Some(reason) = self.units[index].failure.clone() else {
                continue;
            };
            for task in 0..self.tasks.len() {
                if self.task_units[task] == index {
                    self.observe_trees(task, false);
                }
            }
            self.observe(index, &BTreeSet::new());
            let unit = &self.units[index];
            let files = unit
                .files
                .iter()
                .map(|key| {
                    let record = &self.files[key];
                    let outcome = restore(key, record);
                    (key.clone(), outcome)
                })
                .collect();
            report.units.push(FailedUnit {
                reason,
                directories: unit.directories.clone(),
                summaries: unit.summaries.clone(),
                files,
                policy_failure: unit.policy_failure,
            });
        }
        report.node_managers = self.node_managers;
        report
    }
}

impl Task {
    fn new(label: &Path, command: Result<Option<PreparedInstall>, String>) -> Self {
        Self {
            label: label.into(),
            command,
            files: BTreeSet::new(),
            scope_paths: BTreeSet::new(),
            managed: BTreeSet::new(),
            summaries: BTreeSet::new(),
            rust_root: None,
            node_root: None,
            trees: vec![],
            tree_links: BTreeMap::new(),
            mise_locks: None,
        }
    }
}
fn root(parent: &[usize], mut index: usize) -> usize {
    while parent[index] != index {
        index = parent[index];
    }
    index
}
fn identity(path: &Path) -> PathBuf {
    resolved_path(path).unwrap_or_else(|_| path.into())
}
fn tauri_pair(a: &Path, a_language: Language, b: &Path, b_language: Language) -> bool {
    let (node, rust) = match (a_language, b_language) {
        (Language::Node, Language::Rust) => (a, b),
        (Language::Rust, Language::Node) => (b, a),
        _ => return false,
    };
    node.file_name().is_some_and(|name| name == "package.json")
        && node.parent().is_some_and(|directory| {
            identity(&directory.join("src-tauri/Cargo.toml")) == identity(rust)
        })
}

fn gradle_root(directory: &Path, boundary: &Path) -> PathBuf {
    let directory = identity(directory);
    let boundary = identity(boundary);
    for candidate in directory
        .ancestors()
        .take_while(|candidate| candidate.starts_with(&boundary))
    {
        if ["settings.gradle", "settings.gradle.kts"]
            .iter()
            .any(|name| candidate.join(name).is_file())
        {
            if candidate == directory
                || detect_manifests(candidate).iter().any(|manifest| {
                    manifest.language == Language::Java
                        && manifest
                            .path
                            .parent()
                            .is_some_and(|parent| identity(parent) == directory)
                })
            {
                return candidate.into();
            }
            break;
        }
    }
    directory
}

fn managed_files(manager: &str, directory: &Path) -> Vec<PathBuf> {
    let names: &[&str] = match manager {
        "npm" | "pnpm" | "yarn" | "bun" => &[
            "package.json",
            "pnpm-lock.yaml",
            "package-lock.json",
            "npm-shrinkwrap.json",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
        ],
        "uv" => &["pyproject.toml", "uv.lock"],
        "poetry" => &["pyproject.toml", "poetry.lock"],
        "rye" => &[
            "pyproject.toml",
            "requirements.lock",
            "requirements-dev.lock",
        ],
        "pipenv" => &["Pipfile", "Pipfile.lock"],
        "go" => &["go.mod", "go.sum"],
        "bundle" => &["Gemfile", "Gemfile.lock"],
        "composer" => &["composer.json", "composer.lock"],
        "swift" => &["Package.swift", "Package.resolved"],
        "./gradlew" | "./gradlew.bat" => &[
            "gradle/wrapper/gradle-wrapper.properties",
            "gradle/wrapper/gradle-wrapper.jar",
            "gradlew",
            "gradlew.bat",
        ],
        _ => &[],
    };
    let files: Vec<_> = names.iter().map(|name| directory.join(name)).collect();

    files
}

fn parent_permissions(target: &Path) -> Vec<(PathBuf, fs::Permissions)> {
    target
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .filter_map(|path| {
            fs::metadata(path)
                .ok()
                .filter(|metadata| metadata.is_dir())
                .map(|metadata| (path.into(), metadata.permissions()))
        })
        .collect()
}
fn read_record(record: &FileRecord) -> io::Result<FileState> {
    for guard in &record.guards {
        guard.check()?;
    }
    FileState::read(&record.guards[0].target)
}
fn restore_record(record: &FileRecord) -> FileOutcome {
    if let Some(reason) = &record.conflict {
        return FileOutcome::Conflict {
            reason: reason.clone(),
            backup: recovery_copy(record),
        };
    }
    let result = (|| -> io::Result<FileOutcome> {
        let current = read_record(record)?;
        if !record.expected.matches(&current) {
            return Err(io::Error::other("File changed before recovery"));
        }
        if record.before.matches(&current) {
            return Ok(FileOutcome::Unchanged);
        }
        let guard = &record.guards[0];
        match &record.before.bytes {
            Some(bytes) => {
                for (path, permissions) in record.parent_permissions.iter().rev() {
                    match fs::symlink_metadata(path) {
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            guard.check()?;
                            fs::create_dir(path)?;
                            fs::set_permissions(path, permissions.clone())?;
                        }
                        Err(error) => return Err(error),
                        Ok(_) => {}
                    }
                }
                write_checked(guard, &current, bytes, record.before.permissions.as_ref())?;
                if !record.before.matches(&read_record(record)?) {
                    return Err(io::Error::other("File changed after recovery"));
                }
                Ok(FileOutcome::Restored)
            }
            None => {
                guard.check()?;
                if !current.matches(&FileState::read(&guard.target)?) {
                    return Err(io::Error::other("File changed before removal"));
                }
                fs::remove_file(&guard.target)?;
                if !record.before.matches(&read_record(record)?) {
                    return Err(io::Error::other("File changed after removal"));
                }
                Ok(FileOutcome::Removed)
            }
        }
    })();
    match result {
        Ok(outcome) => outcome,
        Err(error) => FileOutcome::Failed {
            reason: error.to_string(),
            backup: recovery_copy(record),
        },
    }
}
fn recovery_copy(record: &FileRecord) -> Result<PathBuf, String> {
    if record.before.bytes.is_none()
        && record.guards.iter().any(
            |guard| match (guard.alias.parent(), guard.target.parent()) {
                (Some(alias), Some(original)) => {
                    resolved_path(alias).map_or(true, |current| current != original)
                }
                _ => true,
            },
        )
    {
        return Err(
            "Current parent has an unobserved target; kept without an absence receipt".into(),
        );
    }
    backup(&record.before)
}

impl RollbackReport {
    pub fn has_install_failures(&self) -> bool {
        self.units.iter().any(|unit| !unit.policy_failure)
    }
    pub fn node_manager(&self, directory: &Path) -> Option<(PathBuf, &'static str)> {
        self.node_managers.get(&identity(directory)).cloned()
    }
    pub fn has_failures(&self) -> bool {
        !self.units.is_empty()
    }
    pub fn covers_directory(&self, directory: &Path) -> bool {
        let absolute = std::path::absolute(directory).unwrap_or_else(|_| directory.into());
        let directory = identity(directory);
        self.units.iter().any(|unit| {
            unit.directories.contains(&directory)
                || unit.directories.contains(&absolute)
                || unit
                    .directories
                    .iter()
                    .any(|alias| identity(alias) == directory)
        })
    }
    pub fn exclude_rust_audits(&self, plan: &mut RustAuditPlan) {
        plan.locks
            .retain(|lock| !self.covers_directory(&lock.directory));
        plan.missing_lock_dirs
            .retain(|directory| !self.covers_directory(directory));
    }
    pub fn mark_summary(&self, summary: &mut UpdateSummary) {
        for unit in &self.units {
            let incomplete = if unit.recovered() {
                ""
            } else {
                "; recovery incomplete: some files were kept for manual recovery"
            };
            for &index in &unit.summaries {
                let manifest = &mut summary.manifests[index];
                let key = identity(&manifest.path);
                if unit.files.get(&key).is_some_and(FileOutcome::restored) {
                    manifest.modified = false;
                }
                for result in &mut manifest.results {
                    if result.is_update() && result.write_error().is_none() {
                        result.record_write_error(format!(
                            "{} failed; update canceled: {}{incomplete}",
                            if unit.policy_failure {
                                "lock verification"
                            } else {
                                "install"
                            },
                            unit.reason
                        ));
                    }
                }
            }
        }
    }
    pub fn print(&self, out: &mut impl Write) -> io::Result<()> {
        for unit in &self.units {
            writeln!(
                out,
                "{} failed; recovering project changes ({})",
                if unit.policy_failure {
                    "Lock verification"
                } else {
                    "Install"
                },
                unit.reason
            )?;
            for (path, outcome) in &unit.files {
                match outcome {
                    FileOutcome::Unchanged => {}
                    FileOutcome::Restored => writeln!(out, "  restored {}", path.display())?,
                    FileOutcome::Removed => {
                        writeln!(out, "  removed {} (created during install)", path.display())?
                    }
                    FileOutcome::Conflict { reason, backup }
                    | FileOutcome::Failed { reason, backup } => {
                        writeln!(
                            out,
                            "  recovery failed for {}: {reason}; current file kept",
                            path.display()
                        )?;
                        match backup {
                            Ok(backup)
                                if backup
                                    .file_name()
                                    .is_some_and(|name| name == "originally-absent") =>
                            {
                                writeln!(
                                    out,
                                    "    Original file was absent (receipt: {}). Review and remove {} after resolving the conflict.",
                                    backup.display(),
                                    path.display()
                                )?
                            }
                            Ok(backup) => writeln!(
                                out,
                                "    Pre-write copy: {}. Restore it to {} after resolving the conflict.",
                                backup.display(),
                                path.display()
                            )?,
                            Err(error) => writeln!(out, "    Recovery copy failed: {error}")?,
                        }
                    }
                }
            }
            if unit.recovered() {
                writeln!(
                    out,
                    "  The project files are back to their state before this run."
                )?;
            } else {
                writeln!(
                    out,
                    "  Recovery is incomplete: the files kept above need manual recovery."
                )?;
            }
            writeln!(
                out,
                "  Install artifacts are retained. Re-run the package manager after recovery{}",
                if unit.policy_failure {
                    "."
                } else {
                    "; age audits for this project were skipped."
                }
            )?;
        }
        Ok(())
    }
}

mod mise;
mod mise_lock;
mod scope;

#[cfg(test)]
mod tests;
