//! マニフェストファイルの書き戻し処理。
//!
//! 提供内容:
//! - マニフェストへバージョン更新を適用する `ManifestWriter`
//! - ファイルを書き換えない dry-run モード
//! - 更新時の書式保持
//! - 失敗時も継続できるエラーハンドリング

use crate::domain::{GitReference, Language, ManifestUpdateResult, UpdateResult};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use std::fs;
use std::path::Path;

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
    /// 実際に反映された更新数
    pub updates_applied: usize,
    /// 失敗した更新数
    pub updates_failed: usize,
    /// 実ファイルが変更されたかどうか
    pub file_modified: bool,
    /// 更新中に発生したエラー
    pub errors: Vec<String>,
}

impl WriteResult {
    /// 新しい `WriteResult` を作る
    fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            updates_applied: 0,
            updates_failed: 0,
            file_modified: false,
            errors: Vec::new(),
        }
    }

    /// 実際に反映された更新があるかどうか
    pub fn has_updates(&self) -> bool {
        self.updates_applied > 0
    }

    /// エラーがあるかどうか
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
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

    /// `ManifestUpdateResult` の更新をファイルへ適用する
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

        // 更新は順番に適用する
        let mut current_content = content.clone();

        for update in manifest_result.results.iter() {
            if let UpdateResult::Update {
                dependency,
                new_version,
                ..
            } = update
            {
                // git 依存の場合は参照種別で挙動が変わる。
                //   - tag: マニフェストの tag 文字列を書き換える
                //   - branch/default/rev: マニフェストを書き換えない
                //     (Cargo.lock 側で commit hash が更新されるのを待つ)
                if let Some(git) = &dependency.git_source {
                    if let GitReference::Tag(_) = &git.reference {
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
                                result.updates_failed += 1;
                                result.errors.push(format!(
                                    "Failed to update git tag for {}: {}",
                                    dependency.name, e
                                ));
                            }
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
                        result.updates_failed += 1;
                        result
                            .errors
                            .push(format!("Failed to update {}: {}", dependency.name, e));
                    }
                }
            }
        }

        // dry-run でなく、実際に変更がある場合のみ書き戻す
        if result.updates_applied > 0 && !self.dry_run {
            fs::write(path, &current_content).map_err(|e| ManifestError::WriteError {
                path: path.clone(),
                source: e,
            })?;
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
            .filter_map(|manifest| {
                // 更新対象があるマニフェストだけ処理する
                if !manifest.has_updates() {
                    return None;
                }

                let parser = get_parser(manifest.language);
                match self.apply_updates(manifest, parser.as_ref()) {
                    Ok(result) => Some(result),
                    Err(e) => {
                        let mut result = WriteResult::new(&manifest.path);
                        result
                            .errors
                            .push(format!("Failed to process manifest: {}", e));
                        Some(result)
                    }
                }
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
    fs::write(path, content).map_err(|e| ManifestError::WriteError {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, GitSource, VersionSpec, VersionSpecKind};
    use crate::manifest::ManifestParser;
    use std::io::Write;
    use tempfile::TempDir;

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
        assert_eq!(result.updates_failed, 0);
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
        assert_eq!(result.updates_failed, 1);
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
        assert_eq!(result.updates_failed, 0);
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

        let writer = ManifestWriter::new(false);
        let parser = crate::manifest::PackageJsonParser;
        let result = writer.apply_updates(&manifest_result, &parser);

        // 後始末のため権限を戻す
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        assert!(result.is_err());
        match result.unwrap_err() {
            ManifestError::WriteError { .. } => {}
            e => panic!("Expected WriteError, got: {:?}", e),
        }
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
    }
}
