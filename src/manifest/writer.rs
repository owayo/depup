//! マニフェストファイルの書き戻し処理。
//!
//! 提供内容:
//! - マニフェストへバージョン更新を適用する `ManifestWriter`
//! - ファイルを書き換えない dry-run モード
//! - 更新時の書式保持
//! - 失敗時も継続できるエラーハンドリング

use crate::domain::{Dependency, GitReference, Language, ManifestUpdateResult, UpdateResult};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// マニフェストへの更新を書き戻すライター
pub struct ManifestWriter {
    /// dry-run モードで動作するかどうか
    dry_run: bool,
}

/// マニフェスト 1 件への適用結果
#[derive(Debug)]
pub struct WriteResult {
    /// 対象マニフェストのパス
    pub path: std::path::PathBuf,
    /// 実際に反映された更新数 (書き換えても内容が同じだった更新と、
    /// マニフェストを書き換えない git の更新は数えない)
    pub updates_applied: usize,
    /// 書き込めなかった更新 (`ManifestUpdateResult::results` 上の位置の順)
    pub failed_updates: Vec<WriteFailure>,
    /// 実ファイルが変更されたかどうか
    pub file_modified: bool,
    /// 更新中に発生したエラー
    pub errors: Vec<String>,
}

/// 書き込めなかった更新 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFailure {
    /// `ManifestUpdateResult::results` 上の位置
    pub index: usize,
    /// 書き込めなかった理由
    pub reason: String,
}

impl WriteResult {
    /// 新しい `WriteResult` を作る
    fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            updates_applied: 0,
            failed_updates: Vec::new(),
            file_modified: false,
            errors: Vec::new(),
        }
    }

    /// 実際に反映された更新があるかどうか
    pub fn has_updates(&self) -> bool {
        self.updates_applied > 0
    }

    /// 書き込めなかった更新の数
    pub fn updates_failed(&self) -> usize {
        self.failed_updates.len()
    }

    /// エラーがあるかどうか
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// `index` の更新を書き込めなかったものとして記録する
    fn record_failure(&mut self, index: usize, reason: impl Into<String>) {
        self.failed_updates.push(WriteFailure {
            index,
            reason: reason.into(),
        });
    }

    /// マニフェスト全体の失敗 (読み込み・解析・保存の失敗) を記録する。
    ///
    /// マニフェストを書き換える更新はどれも反映されていないので、まだ個別の理由が
    /// 記録されていない更新を `reason` で書き込めなかったものにする。個別の理由
    /// (曖昧として拒否した、など) がある更新は、その理由を残す
    fn record_manifest_failure(&mut self, manifest_result: &ManifestUpdateResult, reason: String) {
        for (index, update) in manifest_result.results.iter().enumerate() {
            if let UpdateResult::Update { dependency, .. } = update
                && rewrites_manifest(dependency)
                && !self
                    .failed_updates
                    .iter()
                    .any(|failure| failure.index == index)
            {
                self.record_failure(index, reason.clone());
            }
        }
        self.failed_updates.sort_by_key(|failure| failure.index);
        self.updates_applied = 0;
        self.file_modified = false;
        self.errors.push(reason);
    }
}

/// 更新がマニフェストを書き換えるかどうかを返す。
///
/// git 依存の branch / rev / 既定ブランチはマニフェストを書き換えない
/// (Cargo.lock 側で commit hash が更新されるのを待つ)
fn rewrites_manifest(dependency: &Dependency) -> bool {
    match &dependency.git_source {
        Some(git) => matches!(git.reference, GitReference::Tag(_)),
        None => true,
    }
}

impl ManifestWriter {
    /// 新しい `ManifestWriter` を作る
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }

    /// dry-run 用の `ManifestWriter` を作る
    pub fn dry_run() -> Self {
        Self { dry_run: true }
    }

    /// dry-run モードかどうか
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// `ManifestUpdateResult` の更新をファイルへ適用する。
    ///
    /// 書き込めなかった更新は `WriteResult::failed_updates` に記録する。保存 (I/O) に失敗した
    /// ときも `Ok` を返し、書き換えた更新をすべて書き込めなかったものとして記録する。
    /// `Err` を返すのは、マニフェストを読み込めず、どの更新も試せなかったときだけ
    pub fn apply_updates(
        &self,
        manifest_result: &ManifestUpdateResult,
        parser: &dyn ManifestParser,
    ) -> Result<WriteResult, ManifestError> {
        let path = &manifest_result.path;
        let mut result = WriteResult::new(path);

        // 現在のファイル内容を読む
        let content = fs::read_to_string(path).map_err(|e| ManifestError::ReadError {
            path: path.clone(),
            source: e,
        })?;

        // 現行のパーサAPIは依存名だけで書き換えるため、同じキーが複数箇所にあると
        // 更新対象外の宣言まで変更してしまう。位置情報付き編集へ移行するまでは、
        // 曖昧な更新を拒否してマニフェスト破壊を防ぐ。
        let parsed_dependencies = parser.parse(&content)?;
        let mut declaration_counts = HashMap::new();
        let mut variable_counts = HashMap::new();
        for dependency in &parsed_dependencies {
            *declaration_counts
                .entry(dependency.manifest_name().to_string())
                .or_insert(0usize) += 1;
            if let Some(variable) = &dependency.variable_name {
                *variable_counts.entry(variable.clone()).or_insert(0usize) += 1;
            }
        }
        let ambiguous_declarations: HashSet<String> = declaration_counts
            .into_iter()
            .filter_map(|(name, count)| (count > 1).then_some(name))
            .collect();
        let is_version_catalog = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".versions.toml"));
        let ambiguous_variables: HashSet<String> = variable_counts
            .into_iter()
            .filter_map(|(name, count)| {
                if count <= 1 {
                    return None;
                }
                // Every alias sharing this catalog version must select the same target.
                // A skipped alias or different target keeps the write ambiguous.
                let targets: Vec<&str> = manifest_result
                    .results
                    .iter()
                    .filter_map(|result| match result {
                        UpdateResult::Update {
                            dependency,
                            new_version,
                            ..
                        } if dependency.variable_name.as_deref() == Some(&name) => {
                            Some(new_version.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                let shared_target_is_safe = is_version_catalog
                    && targets.len() == count
                    && targets.iter().all(|target| *target == targets[0]);
                (!shared_target_is_safe).then_some(name)
            })
            .collect();

        // 更新は順番に適用する
        let mut current_content = content.clone();
        // 曖昧として拒否した依存キー。版や取得元の食い違う宣言が別々の更新になっても、
        // 同じ依存のエラーは 1 行だけ出す
        let mut refused_ambiguous: HashSet<&str> = HashSet::new();

        for (index, update) in manifest_result.results.iter().enumerate() {
            if let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = update
            {
                // マニフェストを書き換えない更新 (git 依存の branch/default/rev) は、
                // 同名の宣言が他にあっても巻き添えは起きないので、曖昧判定を当てない
                if !rewrites_manifest(dependency) {
                    continue;
                }

                let ambiguous_variable = dependency
                    .variable_name
                    .as_ref()
                    .is_some_and(|name| ambiguous_variables.contains(name));
                if ambiguous_declarations.contains(dependency.manifest_name()) || ambiguous_variable
                {
                    let reason = format!(
                        "Refusing to update ambiguous dependency '{}' because it has multiple declarations or a shared version target",
                        dependency.manifest_name()
                    );
                    if refused_ambiguous.insert(dependency.manifest_name()) {
                        result.errors.push(reason.clone());
                    }
                    result.record_failure(index, reason);
                    continue;
                }

                // git 依存の tag はマニフェストの tag 文字列を書き換える
                if dependency.git_source.is_some() {
                    match parser.update_git_tag(
                        &current_content,
                        dependency.manifest_name(),
                        new_version,
                    ) {
                        Ok(updated_content) => {
                            if updated_content != current_content {
                                current_content = updated_content;
                                result.updates_applied += 1;
                            }
                        }
                        Err(e) => {
                            let reason =
                                format!("Failed to update git tag for {}: {}", dependency.name, e);
                            result.errors.push(reason.clone());
                            result.record_failure(index, reason);
                        }
                    }
                    continue;
                }

                match parser.update_version(
                    &current_content,
                    dependency.manifest_name(),
                    new_version,
                ) {
                    Ok(updated_content) => {
                        if updated_content != current_content {
                            current_content = updated_content;
                            result.updates_applied += 1;
                        }
                    }
                    Err(e) => {
                        let reason = format!("Failed to update {}: {}", dependency.name, e);
                        result.errors.push(reason.clone());
                        result.record_failure(index, reason);
                    }
                }
            }
        }

        // dry-run でなく、実際に変更がある場合のみ書き戻す
        if result.updates_applied > 0 && !self.dry_run {
            if let Err(source) = write_atomically(path, &current_content) {
                let error = ManifestError::WriteError {
                    path: path.clone(),
                    source,
                };
                result.record_manifest_failure(
                    manifest_result,
                    format!("Failed to process manifest: {}", error),
                );
                return Ok(result);
            }
            result.file_modified = true;
        }

        Ok(result)
    }

    /// 複数のマニフェストへ更新を適用する
    pub fn apply_all_updates(
        &self,
        manifests: &[ManifestUpdateResult],
        get_parser: impl Fn(Language) -> Box<dyn ManifestParser>,
    ) -> Vec<WriteResult> {
        manifests
            .iter()
            // 更新対象があるマニフェストだけ処理する
            .filter(|manifest| manifest.has_judged_updates())
            .map(|manifest| {
                let parser = get_parser(manifest.language);
                self.apply_updates(manifest, parser.as_ref())
                    .unwrap_or_else(|e| {
                        // 読み込み・解析に失敗したので、どの更新も試せていない
                        let mut result = WriteResult::new(&manifest.path);
                        result.record_manifest_failure(
                            manifest,
                            format!("Failed to process manifest: {}", e),
                        );
                        result
                    })
            })
            .collect()
    }
}

/// マニフェストの内容を安全に読み込む
pub fn read_manifest(path: &Path) -> Result<String, ManifestError> {
    fs::read_to_string(path).map_err(|e| ManifestError::ReadError {
        path: path.to_path_buf(),
        source: e,
    })
}

/// マニフェストへ内容を書き込む
pub fn write_manifest(path: &Path, content: &str) -> Result<(), ManifestError> {
    write_atomically(path, content).map_err(|e| ManifestError::WriteError {
        path: path.to_path_buf(),
        source: e,
    })
}

/// 一時ファイル + rename によるアトミック書き込み。
///
/// `fs::write` の truncate→write は途中失敗 (ディスクフル・電源断など) で
/// マニフェストを部分内容のまま破壊しうるため、同一ディレクトリの一時ファイルへ
/// 書き切ってから rename で置き換える。既存ファイルのパーミッションは引き継ぐ。
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    // path 自体が symlink の場合はリンク先の実体を更新対象にする。
    // rename(2) は path が symlink のときリンク先ではなく symlink そのものを置き換えるため、
    // そのまま rename すると symlink が通常ファイルに化け、リンク先 (共有マニフェスト等) が
    // 古いまま取り残される (従来の `fs::write` は symlink を辿って実体を truncate→write していた)。
    // canonicalize で実パスへ解決し、その実体に対してアトミック置換を行うことで symlink 構造を
    // 保ったまま中身だけを更新する。tmp も実体側のディレクトリに作るため rename は同一
    // ファイルシステム内に収まり EXDEV にならない。
    let resolved;
    let target = if path
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        resolved = fs::canonicalize(path)?;
        resolved.as_path()
    } else {
        path
    };

    // 既存ファイルが書き込み不可なら従来の `fs::write` と同じくエラーにする。
    // rename はディレクトリ権限だけで成功し、読み取り専用による保護を
    // 迂回してしまうため、先に書き込み権限を確認する。
    if target.exists() {
        fs::OpenOptions::new().write(true).open(target)?;
    }

    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("manifest");
    // 一時ファイル名は PID だけだと予測可能で、同じ名前の symlink を先に置かれると
    // `File::create` (O_CREAT|O_TRUNC、O_NOFOLLOW なし) がリンク先へ書き込んでしまう。
    // ナノ秒を混ぜて推測を難しくし、`create_new` で「既存 (symlink 含む) があれば
    // EEXIST で失敗」させることで、リンク先の実体を破壊する経路を塞ぐ。
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp_path = dir.join(format!(
        ".{}.depup-tmp-{}-{}",
        file_name,
        std::process::id(),
        nonce
    ));

    let result = (|| -> std::io::Result<()> {
        {
            // rename の前にデータを永続化する。`fs::write` はページキャッシュへ書くだけなので、
            // fsync しないと rename のメタデータだけが先に永続化され、電源断で
            // ゼロ長または部分内容のマニフェストが残りうる (元の内容も失われる)。
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        if let Ok(metadata) = fs::metadata(target) {
            let _ = fs::set_permissions(&tmp_path, metadata.permissions());
        }
        fs::rename(&tmp_path, target)?;
        // rename エントリ自体の永続化 (best-effort。ディレクトリを開けない
        // プラットフォームでは無視する)
        if let Ok(dir_handle) = fs::File::open(dir) {
            let _ = dir_handle.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, GitSource, VersionSpec, VersionSpecKind};
    use crate::manifest::ManifestParser;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_write_manifest_is_atomic_and_cleans_tmp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("package.json");
        fs::write(&path, "old").unwrap();

        write_manifest(&path, "new content").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");

        // 一時ファイルが残っていないこと
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("depup-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "一時ファイルが残留: {:?}", leftovers);
    }

    #[cfg(unix)]
    #[test]
    fn test_write_manifest_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Cargo.toml");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        write_manifest(&path, "new").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "既存ファイルのパーミッションを引き継ぐべき");
    }

    #[cfg(unix)]
    #[test]
    fn test_write_manifest_through_symlink_updates_target() {
        // マニフェストが symlink の場合、rename でリンクを通常ファイルに化けさせず、
        // リンク先の実体を更新して symlink 構造を維持すること (アトミック化前の
        // `fs::write` 挙動との互換)。モノレポで共有マニフェストを symlink 参照する構成を想定。
        let dir = TempDir::new().unwrap();
        let real_dir = dir.path().join("shared");
        fs::create_dir(&real_dir).unwrap();
        let target = real_dir.join("package.json");
        fs::write(&target, "old").unwrap();

        let link = dir.path().join("package.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_manifest(&link, "new content").unwrap();

        // symlink はそのまま symlink として残る
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink が通常ファイルに置き換わってはならない"
        );
        // リンク先の実体が更新されている
        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
        // symlink 経由でも新内容が読める
        assert_eq!(fs::read_to_string(&link).unwrap(), "new content");

        // 一時ファイルが実体側ディレクトリに残っていないこと
        let leftovers: Vec<_> = fs::read_dir(&real_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("depup-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "一時ファイルが残留: {:?}", leftovers);
    }

    struct NoOpParser;

    impl ManifestParser for NoOpParser {
        fn parse(&self, _content: &str) -> Result<Vec<Dependency>, ManifestError> {
            Ok(Vec::new())
        }

        fn language(&self) -> Language {
            Language::Node
        }

        fn update_version(
            &self,
            content: &str,
            _package: &str,
            _new_version: &str,
        ) -> Result<String, ManifestError> {
            Ok(content.to_string())
        }
    }

    fn sample_dependency(name: &str, version: &str, language: Language) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, false, language)
    }

    fn create_temp_package_json(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("package.json");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    fn create_temp_cargo_toml(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("Cargo.toml");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_manifest_writer_new() {
        let writer = ManifestWriter::new(false);
        assert!(!writer.is_dry_run());

        let writer = ManifestWriter::new(true);
        assert!(writer.is_dry_run());
    }

    #[test]
    fn test_manifest_writer_dry_run_constructor() {
        let writer = ManifestWriter::dry_run();
        assert!(writer.is_dry_run());
    }

    #[test]
    fn test_write_result_new() {
        let result = WriteResult::new("/path/to/file");
        assert_eq!(result.path, std::path::PathBuf::from("/path/to/file"));
        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 0);
        assert!(!result.file_modified);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_write_result_has_updates() {
        let mut result = WriteResult::new("/path/to/file");
        assert!(!result.has_updates());

        result.updates_applied = 1;
        assert!(result.has_updates());
    }

    #[test]
    fn test_write_result_has_errors() {
        let mut result = WriteResult::new("/path/to/file");
        assert!(!result.has_errors());

        result.errors.push("error".to_string());
        assert!(result.has_errors());
    }

    #[test]
    fn test_apply_updates_dry_run() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::dry_run();
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert!(!result.file_modified); // dry-run では書き換えない

        // ファイル内容は変わらない
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("4.17.21"));
        assert!(!content.contains("4.18.0"));
    }

    #[test]
    fn test_apply_updates_actual_write() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert!(result.file_modified);

        // ファイル内容が更新されることを確認する
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("^4.18.0"));
        assert!(!content.contains("4.17.21"));
    }

    #[test]
    fn test_apply_updates_uses_manifest_name() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"[dependencies]
tokio_v1 = { package = "tokio", version = "1.0", features = ["rt"] }
"#;
        let path = create_temp_cargo_toml(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Rust);
        let dep = sample_dependency("tokio", "1.0", Language::Rust).with_manifest_name("tokio_v1");
        manifest_result.add_result(UpdateResult::update(dep, "1.45.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::CargoTomlParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"tokio_v1 = { package = "tokio", version = "1.45.0""#));
    }

    #[test]
    fn test_apply_updates_multiple_packages() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21",
    "express": "^4.18.0"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        let dep1 = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep1, "4.18.0"));

        let dep2 = sample_dependency("express", "4.18.0", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep2, "4.19.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 2);
        assert!(result.file_modified);

        // 両方の依存が更新されることを確認する
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("^4.18.0")); // lodash が更新される
        assert!(content.contains("^4.19.0")); // express が更新される
    }

    #[test]
    fn test_apply_updates_refuses_ambiguous_duplicate_dependency() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "shared": "^1.0.0"
  },
  "devDependencies": {
    "shared": "2.0.0"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);
        let parser = crate::manifest::PackageJsonParser;
        let dependencies = parser.parse(original_content).unwrap();
        let production = dependencies
            .iter()
            .find(|dependency| !dependency.is_dev)
            .unwrap()
            .clone();
        let development = dependencies
            .iter()
            .find(|dependency| dependency.is_dev)
            .unwrap()
            .clone();

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        manifest_result.add_result(UpdateResult::update(production, "3.0.0"));
        manifest_result.add_result(UpdateResult::skip_pinned(development));

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 1);
        assert!(!result.file_modified);
        assert!(result.errors[0].contains("ambiguous dependency"));
        assert_eq!(fs::read_to_string(path).unwrap(), original_content);
    }

    #[test]
    fn test_apply_updates_refuses_shared_version_catalog_reference() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"[versions]
shared = "1.0.0"

[libraries]
alpha = { module = "com.example:alpha", version.ref = "shared" }
beta = { module = "com.example:beta", version.ref = "shared" }
"#;
        let path = temp_dir.path().join("libs.versions.toml");
        fs::write(&path, original_content).unwrap();

        let parser = crate::manifest::GradleParser;
        let dependency = parser
            .parse(original_content)
            .unwrap()
            .into_iter()
            .find(|dependency| dependency.name == "com.example:alpha")
            .unwrap();
        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Java);
        manifest_result.add_result(UpdateResult::update(dependency, "2.0.0"));

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 1);
        assert!(!result.file_modified);
        assert_eq!(fs::read_to_string(&path).unwrap(), original_content);
    }

    #[test]
    fn test_apply_updates_shared_plugin_reference_when_all_targets_agree() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"[versions]
agp = "9.3.1"

[plugins]
android-application = { id = "com.android.application", version.ref = "agp" }
android-library = { id = "com.android.library", version.ref = "agp" }
"#;
        let path = temp_dir.path().join("libs.versions.toml");
        fs::write(&path, content).unwrap();

        let parser = crate::manifest::GradleParser;
        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Java);
        for dependency in parser.parse(content).unwrap() {
            manifest_result.add_result(UpdateResult::update(dependency, "9.4.1"));
        }
        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();
        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed(), 0);
        assert!(result.file_modified);
        let updated = fs::read_to_string(path).unwrap();
        assert!(updated.contains("agp = \"9.4.1\""));
        assert!(updated.contains("version.ref = \"agp\""));
    }

    #[test]
    fn test_apply_updates_gradle_same_coordinate_in_multiple_configurations() {
        // Lombok 公式セットアップ (同一座標を 4 つの configuration へ宣言する正式な手順) は
        // 曖昧扱いで拒否せず、全宣言を更新すること。
        // GradleParser の update_version は文字列記法の全出現を書き換えるため、
        // parse が 1 依存へ畳んだ上で 1 回適用すれば 4 行すべてが更新される。
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"dependencies {
    compileOnly 'org.projectlombok:lombok:1.18.30'
    annotationProcessor 'org.projectlombok:lombok:1.18.30'
    testCompileOnly 'org.projectlombok:lombok:1.18.30'
    testAnnotationProcessor 'org.projectlombok:lombok:1.18.30'
}
"#;
        let path = temp_dir.path().join("build.gradle");
        fs::write(&path, original_content).unwrap();

        let parser = crate::manifest::GradleParser;
        let dependencies = parser.parse(original_content).unwrap();
        assert_eq!(
            dependencies.len(),
            1,
            "同一座標の 4 宣言は 1 依存として扱う"
        );

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Java);
        manifest_result.add_result(UpdateResult::update(dependencies[0].clone(), "1.18.42"));

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed(), 0);
        assert!(!result.has_errors(), "errors: {:?}", result.errors);
        assert!(result.file_modified);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(
            content
                .matches("'org.projectlombok:lombok:1.18.42'")
                .count(),
            4,
            "4 宣言すべてが更新されるべき: {content}"
        );
        assert!(!content.contains("1.18.30"));
    }

    #[test]
    fn test_apply_updates_refuses_shared_gradle_version_variable() {
        // 別々の依存が同じバージョン変数を共有する場合は従来どおり拒否する
        // (変数定義を書き換えると、更新対象でない依存の版まで動いてしまう)
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"def sharedVersion = '31.0'

dependencies {
    implementation "com.google.guava:guava:$sharedVersion"
    implementation "com.example:other-lib:$sharedVersion"
}
"#;
        let path = temp_dir.path().join("build.gradle");
        fs::write(&path, original_content).unwrap();

        let parser = crate::manifest::GradleParser;
        let dependency = parser
            .parse(original_content)
            .unwrap()
            .into_iter()
            .find(|dependency| dependency.name == "com.google.guava:guava")
            .unwrap();

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Java);
        manifest_result.add_result(UpdateResult::update(dependency, "33.0.0-jre"));

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 1);
        assert!(!result.file_modified);
        assert!(result.errors[0].contains("ambiguous dependency"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original_content);
    }

    #[test]
    fn test_apply_updates_refuses_gradle_declarations_with_mismatched_versions() {
        // 同名でもバージョン生表記が食い違う宣言は 1 依存へ畳めないため、
        // 従来どおり曖昧として拒否する
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"dependencies {
    compileOnly 'org.projectlombok:lombok:1.18.28'
    annotationProcessor "org.projectlombok:lombok:1.18.30!!"
}
"#;
        let path = temp_dir.path().join("build.gradle");
        fs::write(&path, original_content).unwrap();

        let parser = crate::manifest::GradleParser;
        let dependencies = parser.parse(original_content).unwrap();
        assert_eq!(dependencies.len(), 2, "生表記が違う宣言は畳まない");

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Java);
        manifest_result.add_result(UpdateResult::update(dependencies[0].clone(), "1.18.42"));

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 1);
        assert!(!result.file_modified);
        assert!(result.errors[0].contains("ambiguous dependency"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original_content);
    }

    #[test]
    fn test_apply_updates_handles_failed_update() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        // 正常な更新
        let dep1 = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep1, "4.18.0"));

        // 失敗する更新（対象パッケージが存在しない）
        let dep2 = sample_dependency("nonexistent", "1.0.0", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep2, "2.0.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed(), 1);
        assert!(result.has_errors());
        assert!(result.file_modified); // 成功分は書き戻される
    }

    #[test]
    fn test_apply_updates_no_updates() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        // 更新対象がないケース
        let manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 0);
        assert!(!result.file_modified);
    }

    #[test]
    fn test_apply_updates_file_not_found() {
        let manifest_result =
            ManifestUpdateResult::new("/nonexistent/path/package.json", Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        let mut manifest_result = manifest_result;
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser);

        assert!(result.is_err());
    }

    #[test]
    fn test_apply_updates_no_op_is_not_counted() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::new(false);
        let result = writer.apply_updates(&manifest_result, &NoOpParser).unwrap();

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 0);
        assert!(!result.file_modified);
        assert!(!result.has_errors());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original_content);
    }

    #[test]
    fn test_read_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let content = r#"{"name": "test"}"#;
        let path = create_temp_package_json(&temp_dir, content);

        let result = read_manifest(&path).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_read_manifest_not_found() {
        let result = read_manifest(Path::new("/nonexistent/path/file.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_write_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.json");
        let content = r#"{"name": "test"}"#;

        write_manifest(&path, content).unwrap();

        let result = fs::read_to_string(&path).unwrap();
        assert_eq!(result, content);
    }

    #[cfg(unix)]
    #[test]
    fn test_apply_updates_write_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);

        // 読み取り専用にする
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o444);
        fs::set_permissions(&path, perms).unwrap();

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));
        // 保存の前に個別の理由で失敗する更新
        let missing = sample_dependency("nonexistent", "1.0.0", Language::Node);
        manifest_result.add_result(UpdateResult::update(missing, "2.0.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser);

        // 後始末のため権限を戻す
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        // 保存に失敗しても Err にせず、書き換えた更新を書き込めなかったものとして記録する。
        // 保存の前に個別の理由で失敗した更新は、その理由を残す
        let result = result.unwrap();
        assert_eq!(result.updates_applied, 0);
        assert!(!result.file_modified);
        assert_eq!(result.updates_failed(), 2);
        assert_eq!(result.failed_updates[0].index, 0);
        assert!(
            result.failed_updates[0]
                .reason
                .contains("failed to write manifest file"),
            "{:?}",
            result.failed_updates
        );
        assert_eq!(result.failed_updates[1].index, 1);
        assert!(
            result.failed_updates[1]
                .reason
                .starts_with("Failed to update nonexistent"),
            "{:?}",
            result.failed_updates
        );
        assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.starts_with("Failed to process manifest"))
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original_content);
    }

    #[test]
    fn test_apply_updates_git_tag_updates_file() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"[dependencies]
my-crate = { git = "https://github.com/example/my-crate.git", tag = "v1.2.3" }
"#;
        let path = temp_dir.path().join("Cargo.toml");
        fs::write(&path, original_content).unwrap();

        // tag 指定の git 依存
        let spec = VersionSpec::new(VersionSpecKind::Exact, "v1.2.3", "v1.2.3");
        let dep = Dependency::new("my-crate", spec, false, Language::Rust).with_git_source(
            GitSource::new(
                "https://github.com/example/my-crate.git",
                GitReference::Tag("v1.2.3".to_string()),
            ),
        );

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Rust);
        manifest_result.add_result(UpdateResult::update(dep, "v1.3.0"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::CargoTomlParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        assert_eq!(result.updates_applied, 1);
        assert!(result.file_modified);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(r#"tag = "v1.3.0""#));
        assert!(!content.contains(r#"tag = "v1.2.3""#));
    }

    #[test]
    fn test_apply_updates_git_branch_does_not_modify_file() {
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"[dependencies]
my-crate = { git = "https://github.com/example/my-crate.git", branch = "main" }
"#;
        let path = temp_dir.path().join("Cargo.toml");
        fs::write(&path, original_content).unwrap();

        // branch 指定の git 依存
        let spec = VersionSpec::new(VersionSpecKind::Exact, "main", "main");
        let dep = Dependency::new("my-crate", spec, false, Language::Rust).with_git_source(
            GitSource::new(
                "https://github.com/example/my-crate.git",
                GitReference::Branch("main".to_string()),
            )
            .with_current_commit("abc1234"),
        );

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Rust);
        manifest_result.add_result(UpdateResult::update(dep, "def5678"));

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::CargoTomlParser;
        let result = writer.apply_updates(&manifest_result, &parser).unwrap();

        // branch 更新は Cargo.toml を書き換えない (Cargo.lock で反映される)
        assert_eq!(result.updates_applied, 0);
        assert!(!result.file_modified);
        assert!(!result.has_errors());

        // ファイル内容が元のままであることを確認
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original_content);
    }

    /// Cargo.toml を parse し、`(依存名, git 依存か, 更新先)` ごとに該当する依存を
    /// 更新として書き込む。parse から書き込みまでを通すための補助で、
    /// 書き込みの結果と書き込み後のファイル内容を返す
    fn apply_cargo_updates(content: &str, updates: &[(&str, bool, &str)]) -> (WriteResult, String) {
        let temp_dir = TempDir::new().unwrap();
        let path = create_temp_cargo_toml(&temp_dir, content);
        let parser = crate::manifest::CargoTomlParser;
        let dependencies = parser.parse(content).unwrap();

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Rust);
        for (name, is_git, new_version) in updates {
            let dependency = dependencies
                .iter()
                .find(|dependency| dependency.name == *name && dependency.is_git() == *is_git)
                .unwrap_or_else(|| panic!("{name} が parse されない: {dependencies:?}"))
                .clone();
            manifest_result.add_result(UpdateResult::update(dependency, *new_version));
        }

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();
        let written = fs::read_to_string(&path).unwrap();
        (result, written)
    }

    #[test]
    fn test_apply_updates_rust_same_git_tag_in_dependencies_and_dev_dependencies() {
        // 回帰 (#20): 同じ git の tag 依存を [dependencies] と [dev-dependencies] に書いた
        // 構成は曖昧として拒否せず、1 回の更新で両方の tag を書き換える
        let content = r#"[dependencies]
serde_json = { git = "https://github.com/example/json", tag = "v1.0.100" }

[dev-dependencies]
serde_json = { git = "https://github.com/example/json", tag = "v1.0.100", features = ["preserve_order"] }
"#;
        let (result, written) = apply_cargo_updates(content, &[("serde_json", true, "v1.0.151")]);

        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed(), 0);
        assert!(!result.has_errors(), "errors: {:?}", result.errors);
        assert!(result.file_modified);
        assert_eq!(
            written.matches(r#"tag = "v1.0.151""#).count(),
            2,
            "{written}"
        );
        assert!(written.contains(r#"features = ["preserve_order"]"#));
    }

    #[test]
    fn test_apply_updates_rust_same_registry_version_in_dependencies_and_dev_dependencies() {
        // 回帰 (#20): crates.io の依存でも、同じ版の指定の宣言は 1 回の更新で全部書き換える
        let content = r#"[dependencies]
anyhow = "1.0.0"

[dev-dependencies]
anyhow = { version = "1.0.0", features = ["backtrace"] }
"#;
        let (result, written) = apply_cargo_updates(content, &[("anyhow", false, "1.0.100")]);

        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed(), 0);
        assert!(!result.has_errors(), "errors: {:?}", result.errors);
        assert!(written.contains(r#"anyhow = "1.0.100""#), "{written}");
        assert!(
            written.contains(r#"anyhow = { version = "1.0.100", features = ["backtrace"] }"#),
            "{written}"
        );
    }

    #[test]
    fn test_apply_updates_rust_same_git_branch_in_dependencies_and_dev_dependencies() {
        // 回帰 (#20): branch の git 依存は Cargo.toml を書き換えない更新なので、
        // 宣言が複数あってもエラーにしない
        let content = r#"[dependencies]
serde_json = { git = "https://github.com/example/json", branch = "master" }

[dev-dependencies]
serde_json = { git = "https://github.com/example/json", branch = "master", features = ["preserve_order"] }
"#;
        let (result, written) = apply_cargo_updates(content, &[("serde_json", true, "def5678")]);

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 0);
        assert!(!result.has_errors(), "errors: {:?}", result.errors);
        assert!(!result.file_modified);
        assert_eq!(written, content);
    }

    #[test]
    fn test_apply_updates_rust_unwritten_git_update_is_not_refused_beside_registry_declaration() {
        // branch / rev / 既定ブランチの git 依存と同名の crates.io の宣言が並ぶとき、
        // Cargo.toml を書き換えない git の更新は通し、書き換える crates.io の更新だけを
        // 曖昧として拒否する
        for git_reference in [r#", branch = "main""#, r#", rev = "abc1234""#, ""] {
            let content = format!(
                r#"[dependencies]
tracked = {{ git = "https://github.com/example/tracked"{git_reference} }}

[dev-dependencies]
tracked = "1.0.0"
"#
            );

            let (result, written) = apply_cargo_updates(&content, &[("tracked", true, "def5678")]);
            assert_eq!(result.updates_failed(), 0, "{git_reference}");
            assert!(!result.has_errors(), "{git_reference}: {:?}", result.errors);
            assert_eq!(written, content);

            let (result, written) = apply_cargo_updates(
                &content,
                &[("tracked", true, "def5678"), ("tracked", false, "1.1.0")],
            );
            assert_eq!(result.updates_applied, 0, "{git_reference}");
            assert_eq!(result.updates_failed(), 1, "{git_reference}");
            assert_eq!(
                result.errors.len(),
                1,
                "{git_reference}: {:?}",
                result.errors
            );
            assert!(result.errors[0].contains("ambiguous dependency 'tracked'"));
            assert_eq!(written, content);
        }
    }

    #[test]
    fn test_apply_updates_rust_mismatched_declarations_report_one_error() {
        // 版の指定が食い違う宣言は今のとおり拒否する。どちらの宣言も更新になっても、
        // 同じ依存のエラーは 1 行にまとめる
        let content = r#"[dependencies]
shared = "^1.0.0"

[dev-dependencies]
shared = "=2.0.0"
"#;
        let temp_dir = TempDir::new().unwrap();
        let path = create_temp_cargo_toml(&temp_dir, content);
        let parser = crate::manifest::CargoTomlParser;
        let dependencies = parser.parse(content).unwrap();
        assert_eq!(dependencies.len(), 2, "{dependencies:?}");

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Rust);
        for dependency in dependencies {
            manifest_result.add_result(UpdateResult::update(dependency, "3.0.0"));
        }
        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_applied, 0);
        assert_eq!(result.updates_failed(), 2);
        assert_eq!(result.errors.len(), 1, "errors: {:?}", result.errors);
        assert!(result.errors[0].contains("ambiguous dependency 'shared'"));
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }

    #[test]
    fn test_apply_all_updates_empty() {
        let writer = ManifestWriter::new(false);
        let results =
            writer.apply_all_updates(&[], |_| Box::new(crate::manifest::PackageJsonParser));
        assert!(results.is_empty());
    }

    #[test]
    fn test_apply_all_updates_skips_no_updates() {
        let temp_dir = TempDir::new().unwrap();
        let path = create_temp_package_json(&temp_dir, r#"{"dependencies": {}}"#);

        // 更新がない `ManifestUpdateResult`
        let manifest_result = ManifestUpdateResult::new(&path, Language::Node);

        let writer = ManifestWriter::new(false);
        let results = writer.apply_all_updates(&[manifest_result], |_| {
            Box::new(crate::manifest::PackageJsonParser)
        });

        // 更新がないマニフェストは返さない
        assert!(results.is_empty());
    }

    #[test]
    fn test_apply_all_updates_handles_missing_file() {
        let mut manifest_result =
            ManifestUpdateResult::new("/nonexistent/path/package.json", Language::Node);
        let dep = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(dep, "4.18.0"));

        let writer = ManifestWriter::new(false);
        let results = writer.apply_all_updates(&[manifest_result], |_| {
            Box::new(crate::manifest::PackageJsonParser)
        });

        assert_eq!(results.len(), 1);
        assert!(results[0].has_errors());
        // 読み込めないマニフェストの更新は、どれも書き込めなかったものとして記録する
        assert_eq!(results[0].updates_failed(), 1);
        assert_eq!(results[0].failed_updates[0].index, 0);
        assert!(
            results[0].failed_updates[0]
                .reason
                .starts_with("Failed to process manifest")
        );
    }

    #[test]
    fn test_apply_updates_records_failed_update_positions() {
        // 回帰 (#21): 書き込めなかった更新は、`results` 上の位置と理由で記録する。
        // 出力はこれを使って、書き込めた更新と書き込めなかった更新を分けて数える
        let temp_dir = TempDir::new().unwrap();
        let path = create_temp_package_json(
            &temp_dir,
            r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#,
        );

        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        let express = sample_dependency("express", "4.18.0", Language::Node);
        manifest_result.add_result(UpdateResult::skip_already_latest(express));
        let lodash = sample_dependency("lodash", "4.17.21", Language::Node);
        manifest_result.add_result(UpdateResult::update(lodash, "4.18.0"));
        let missing = sample_dependency("nonexistent", "1.0.0", Language::Node);
        manifest_result.add_result(UpdateResult::update(missing, "2.0.0"));

        let result = ManifestWriter::new(false)
            .apply_updates(&manifest_result, &crate::manifest::PackageJsonParser)
            .unwrap();

        assert_eq!(result.updates_applied, 1);
        assert_eq!(result.updates_failed(), 1);
        assert_eq!(result.failed_updates[0].index, 2);
        assert!(
            result.failed_updates[0]
                .reason
                .starts_with("Failed to update nonexistent"),
            "{:?}",
            result.failed_updates
        );
        assert_eq!(result.errors, vec![result.failed_updates[0].reason.clone()]);
    }

    #[test]
    fn test_apply_updates_dry_run_records_refused_updates() {
        // dry-run でも、書き込めない見込みの更新を記録する (ファイルは変えない)
        let temp_dir = TempDir::new().unwrap();
        let original_content = r#"{
  "dependencies": {
    "shared": "^1.0.0"
  },
  "devDependencies": {
    "shared": "~1.0.0"
  }
}"#;
        let path = create_temp_package_json(&temp_dir, original_content);
        let parser = crate::manifest::PackageJsonParser;
        let mut manifest_result = ManifestUpdateResult::new(&path, Language::Node);
        for dependency in parser.parse(original_content).unwrap() {
            manifest_result.add_result(UpdateResult::update(dependency, "1.5.0"));
        }

        let result = ManifestWriter::dry_run()
            .apply_updates(&manifest_result, &parser)
            .unwrap();

        assert_eq!(result.updates_failed(), 2);
        assert_eq!(
            result
                .failed_updates
                .iter()
                .map(|failure| failure.index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
        assert_eq!(fs::read_to_string(&path).unwrap(), original_content);
    }

    #[test]
    fn test_apply_all_updates_unreadable_manifest_keeps_unwritten_git_updates() {
        // マニフェストを読み込めなくても、マニフェストを書き換えない git の更新
        // (Cargo.lock 側で上がる branch の依存) は書き込めなかったものにしない
        let mut manifest_result =
            ManifestUpdateResult::new("/nonexistent/path/Cargo.toml", Language::Rust);
        let branch = Dependency::new(
            "tracked",
            VersionSpec::new(VersionSpecKind::Exact, "main", "main"),
            false,
            Language::Rust,
        )
        .with_git_source(GitSource::new(
            "https://github.com/example/tracked",
            GitReference::Branch("main".to_string()),
        ));
        manifest_result.add_result(UpdateResult::update(branch, "def5678"));
        let tagged = Dependency::new(
            "tagged",
            VersionSpec::new(VersionSpecKind::Exact, "v1.0.0", "v1.0.0"),
            false,
            Language::Rust,
        )
        .with_git_source(GitSource::new(
            "https://github.com/example/tagged",
            GitReference::Tag("v1.0.0".to_string()),
        ));
        manifest_result.add_result(UpdateResult::update(tagged, "v1.1.0"));

        let results = ManifestWriter::new(false).apply_all_updates(&[manifest_result], |_| {
            Box::new(crate::manifest::CargoTomlParser)
        });

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0]
                .failed_updates
                .iter()
                .map(|failure| failure.index)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }
}
