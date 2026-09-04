//! 変更内容を unified diff 風に表示するフォーマッタ。
//!
//! 提供内容:
//! - unified diff 形式の表示
//! - 更新前後バージョンの比較

use crate::domain::{GitReference, ManifestUpdateResult, UpdateResult, UpdateSummary};
use crate::orchestrator::OrchestratorResult;
use crate::output::OutputFormatter;
use std::io::Write;

/// バージョン変更を diff 形式で表示するフォーマッタ
pub struct DiffFormatter {
    /// dry-run かどうか
    dry_run: bool,
}

impl DiffFormatter {
    /// 新しい `DiffFormatter` を作る
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }

    /// 必要なら dry-run 接頭辞を返す
    fn dry_run_prefix(&self) -> &'static str {
        if self.dry_run { "(dry-run) " } else { "" }
    }

    /// マニフェスト 1 件分の diff (ヘッダ + hunk) を書く。
    ///
    /// 実際にマニフェストへ書き込まれる更新だけを対象にする。git 依存の
    /// branch / default / rev はマニフェストを書き換えない (Cargo.lock 側の
    /// 更新のみ) ため、偽の差分として表示しない。表示対象がなければ何も書かない。
    fn write_manifest_diff(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<bool> {
        let prefix = self.dry_run_prefix();

        let displayable: Vec<_> = manifest
            .updates()
            .filter_map(|result| match result {
                UpdateResult::Update {
                    dependency,
                    new_version,
                    ..
                } => {
                    if let Some(git) = &dependency.git_source
                        && !matches!(git.reference, GitReference::Tag(_))
                    {
                        return None;
                    }
                    Some((dependency, new_version))
                }
                _ => None,
            })
            .collect();

        if displayable.is_empty() {
            return Ok(false);
        }

        // diff ヘッダを書く
        writeln!(writer, "{}--- a/{}", prefix, manifest.path.display())?;
        writeln!(writer, "{}+++ b/{}", prefix, manifest.path.display())?;

        // 各更新を diff hunk として書く
        for (dependency, new_version) in displayable {
            let new_constraint = dependency
                .version_spec
                .try_format_updated(new_version)
                .unwrap_or_else(|| dependency.version_spec.raw.clone());
            // 値には接頭辞も含める。npm alias では `version_spec.raw` が制約部分
            // (`^17.0.0`) しか持たないため、そのまま出すと「alias 宣言が外れて素の
            // `^18.0.0` になる」ように見え、実際の書き込み
            // (`"npm:@preact/compat@^18.0.0"`) と食い違う
            let old_version = dependency.manifest_value(&dependency.version_spec.raw);
            let new_formatted = dependency.manifest_value(&new_constraint);

            // 表示にはマニフェスト上の依存キーを使う。`dependency.name` はレジストリ上の
            // 実パッケージ名なので、npm alias (`"react": "npm:@preact/compat@^17"`) や
            // Cargo の `package = "..."` リネーム依存では実ファイルに存在しないキーになる
            let key = dependency.manifest_name();
            writeln!(writer, "@@ {} @@", key)?;
            writeln!(writer, "-  \"{}\": \"{}\"", key, old_version)?;
            writeln!(writer, "+  \"{}\": \"{}\"", key, new_formatted)?;
        }

        Ok(true)
    }
}

impl OutputFormatter for DiffFormatter {
    fn format(&self, result: &OrchestratorResult, writer: &mut dyn Write) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();

        for manifest in &result.summary.manifests {
            if self.write_manifest_diff(manifest, writer)? {
                writeln!(writer)?;
            }
        }

        // 最後にサマリを書く
        let updates = result.summary.total_updates();
        writeln!(
            writer,
            "{}# {} package(s) would be updated",
            prefix, updates
        )?;

        Ok(())
    }

    fn format_summary(
        &self,
        summary: &UpdateSummary,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        let prefix = self.dry_run_prefix();
        let updates = summary.total_updates();
        let skips = summary.total_skips();

        writeln!(
            writer,
            "{}# {} package(s) updated, {} skipped",
            prefix, updates, skips
        )?;

        Ok(())
    }

    fn format_manifest(
        &self,
        manifest: &ManifestUpdateResult,
        writer: &mut dyn Write,
    ) -> std::io::Result<()> {
        self.write_manifest_diff(manifest, writer)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
    use std::path::PathBuf;

    fn sample_dependency(name: &str, version: &str) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, false, Language::Node)
    }

    fn create_test_result() -> OrchestratorResult {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let dep1 = sample_dependency("lodash", "4.17.21");
        manifest.add_result(UpdateResult::update(dep1, "4.18.0"));

        summary.add_manifest(manifest);

        OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn test_diff_formatter_new() {
        let formatter = DiffFormatter::new(false);
        assert!(!formatter.dry_run);
    }

    /// rename 依存では diff にマニフェスト上のキーを出す。
    /// レジストリ上の実パッケージ名を出すと、実ファイルに存在しないキーが並ぶ
    #[test]
    fn test_format_diff_uses_manifest_key_for_renamed_dependency() {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("Cargo.toml"), Language::Rust);

        // `mylib = { package = "actual-crate", version = "^17.0.0" }` 相当。
        // Cargo の rename では値の中に実クレート名が現れないので、キーだけを差し替える
        let dep = sample_dependency("actual-crate", "17.0.0").with_manifest_name("mylib");
        manifest.add_result(UpdateResult::update(dep, "18.0.0"));
        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let formatter = DiffFormatter::new(false);
        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("@@ mylib @@"), "{text}");
        assert!(text.contains(r#"-  "mylib": "^17.0.0""#), "{text}");
        assert!(text.contains(r#"+  "mylib": "^18.0.0""#), "{text}");
        assert!(
            !text.contains("actual-crate"),
            "マニフェストに存在しないキーを出さない: {text}"
        );
    }

    /// 回帰テスト: npm alias では値の `npm:<real>@` 接頭辞も diff に出す。
    ///
    /// `version_spec.raw` は制約部分 (`^17.0.0`) しか持たないため、接頭辞を
    /// 落とすと diff が「alias 宣言が外れて素の `^18.0.0` になる」ように見える。
    /// 実際に書き込まれるのは `"npm:@preact/compat@^18.0.0"` なので、レビュー
    /// 内容と適用結果が食い違っていた。
    #[test]
    fn test_format_diff_keeps_npm_alias_prefix_in_values() {
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        // "react": "npm:@preact/compat@^17.0.0" 相当
        let dep = sample_dependency("@preact/compat", "17.0.0")
            .with_manifest_name("react")
            .with_value_prefix("npm:@preact/compat@");
        manifest.add_result(UpdateResult::update(dep, "18.0.0"));
        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let formatter = DiffFormatter::new(false);
        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        // キーはマニフェスト上の alias 名
        assert!(text.contains("@@ react @@"), "{text}");
        // 値は接頭辞込みで実ファイルと一致する
        assert!(
            text.contains(r#"-  "react": "npm:@preact/compat@^17.0.0""#),
            "{text}"
        );
        assert!(
            text.contains(r#"+  "react": "npm:@preact/compat@^18.0.0""#),
            "{text}"
        );
    }

    #[test]
    fn test_format_diff_skips_non_tag_git_dependencies() {
        use crate::domain::{GitReference, GitSource};

        // branch 参照の git 依存はマニフェストを書き換えないため diff に出さない
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("Cargo.toml"), Language::Rust);

        let spec = VersionSpec::new(VersionSpecKind::Exact, "main", "main");
        let git_dep = Dependency::new("my-crate", spec, false, Language::Rust).with_git_source(
            GitSource::new(
                "https://github.com/example/my-crate",
                GitReference::Branch("main".to_string()),
            ),
        );
        manifest.add_result(UpdateResult::update(
            git_dep,
            "0123456789abcdef0123456789abcdef01234567",
        ));
        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let formatter = DiffFormatter::new(false);
        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(
            !output_str.contains("my-crate"),
            "branch git 依存は diff に出ないべき: {}",
            output_str
        );
        assert!(
            !output_str.contains("--- a/Cargo.toml"),
            "表示対象がないマニフェストはヘッダも出ないべき: {}",
            output_str
        );
    }

    #[test]
    fn test_format_diff_shows_tag_git_dependencies() {
        use crate::domain::{GitReference, GitSource};

        // tag 参照の git 依存はマニフェストの tag 文字列が書き換わるため diff に出す
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("Cargo.toml"), Language::Rust);

        let spec = VersionSpec::new(VersionSpecKind::Exact, "v1.2.3", "v1.2.3");
        let git_dep = Dependency::new("my-crate", spec, false, Language::Rust).with_git_source(
            GitSource::new(
                "https://github.com/example/my-crate",
                GitReference::Tag("v1.2.3".to_string()),
            ),
        );
        manifest.add_result(UpdateResult::update(git_dep, "v2.0.0"));
        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let formatter = DiffFormatter::new(false);
        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("--- a/Cargo.toml"));
        assert!(output_str.contains("-  \"my-crate\": \"v1.2.3\""));
        assert!(output_str.contains("+  \"my-crate\": \"v2.0.0\""));
    }

    #[test]
    fn test_dry_run_prefix() {
        let formatter = DiffFormatter::new(true);
        assert_eq!(formatter.dry_run_prefix(), "(dry-run) ");

        let formatter = DiffFormatter::new(false);
        assert_eq!(formatter.dry_run_prefix(), "");
    }

    #[test]
    fn test_format_diff() {
        let formatter = DiffFormatter::new(false);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // diff 形式で出力されることを確認する
        assert!(output_str.contains("--- a/package.json"));
        assert!(output_str.contains("+++ b/package.json"));
        assert!(output_str.contains("@@ lodash @@"));
        assert!(output_str.contains("-  \"lodash\": \"^4.17.21\""));
        assert!(output_str.contains("+  \"lodash\": \"^4.18.0\""));
        assert!(output_str.contains("# 1 package(s) would be updated"));
    }

    #[test]
    fn test_format_diff_dry_run() {
        let formatter = DiffFormatter::new(true);
        let result = create_test_result();
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("(dry-run)"));
    }

    #[test]
    fn test_format_diff_no_updates() {
        let formatter = DiffFormatter::new(false);
        let summary = UpdateSummary::new(false);
        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };
        let mut output = Vec::new();

        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // サマリ行だけが出力される
        assert!(output_str.contains("# 0 package(s) would be updated"));
        assert!(!output_str.contains("---"));
    }

    #[test]
    fn test_format_manifest() {
        let formatter = DiffFormatter::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);
        let dep = sample_dependency("lodash", "4.17.21");
        manifest.add_result(UpdateResult::update(dep, "4.18.0"));

        let mut output = Vec::new();
        formatter.format_manifest(&manifest, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("--- a/package.json"));
        assert!(output_str.contains("lodash"));
    }

    #[test]
    fn test_format_summary() {
        let formatter = DiffFormatter::new(false);
        let summary = UpdateSummary::new(false);
        let mut output = Vec::new();

        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("# 0 package(s) updated"));
    }

    #[test]
    fn test_format_summary_with_skips() {
        // スキップ数もサマリに含まれる
        let formatter = DiffFormatter::new(false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let dep = sample_dependency("express", "4.18.0");
        manifest.add_result(UpdateResult::skip(
            dep,
            crate::domain::SkipReason::AlreadyLatest,
        ));
        summary.add_manifest(manifest);

        let mut output = Vec::new();
        formatter.format_summary(&summary, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("0 package(s) updated"));
        assert!(output_str.contains("1 skipped"));
    }

    #[test]
    fn test_format_manifest_no_updates() {
        // 更新なしのマニフェストは出力されない
        let formatter = DiffFormatter::new(false);
        let manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let mut output = Vec::new();
        formatter.format_manifest(&manifest, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.is_empty());
    }

    #[test]
    fn test_format_diff_multiple_updates() {
        // 複数パッケージの更新
        let formatter = DiffFormatter::new(false);
        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);

        let dep1 = sample_dependency("lodash", "4.17.21");
        manifest.add_result(UpdateResult::update(dep1, "4.18.0"));

        let dep2 = sample_dependency("axios", "1.6.0");
        manifest.add_result(UpdateResult::update(dep2, "1.7.0"));

        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("@@ lodash @@"));
        assert!(output_str.contains("@@ axios @@"));
        assert!(output_str.contains("# 2 package(s) would be updated"));
    }

    #[test]
    fn test_format_diff_multiple_manifests() {
        // 複数マニフェストファイルの更新
        let formatter = DiffFormatter::new(false);
        let mut summary = UpdateSummary::new(false);

        let mut manifest1 =
            ManifestUpdateResult::new(PathBuf::from("package.json"), Language::Node);
        let dep1 = sample_dependency("lodash", "4.17.21");
        manifest1.add_result(UpdateResult::update(dep1, "4.18.0"));

        let mut manifest2 = ManifestUpdateResult::new(PathBuf::from("Cargo.toml"), Language::Rust);
        let spec = VersionSpec::new(VersionSpecKind::Caret, "^1.0.0".to_string(), "1.0.0")
            .with_prefix("^");
        let dep2 = Dependency::new("serde", spec, false, Language::Rust);
        manifest2.add_result(UpdateResult::update(dep2, "1.1.0"));

        summary.add_manifest(manifest1);
        summary.add_manifest(manifest2);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(output_str.contains("--- a/package.json"));
        assert!(output_str.contains("--- a/Cargo.toml"));
    }

    /// 回帰テスト: Go の `VersionSpec` は `v` を prefix に持ち、Go Proxy は `v` 込みの
    /// バージョンを返すため、素朴に連結すると diff が `vv1.9.1` という go.mod として
    /// 無効な文字列を表示していた。実書き込み (`go_mod::update_version`) と一致させる。
    #[test]
    fn test_go_diff_does_not_duplicate_v_prefix() {
        let spec = VersionSpec::new(VersionSpecKind::Exact, "v1.8.0", "1.8.0").with_prefix("v");
        let dep = Dependency::new("github.com/spf13/cobra", spec, false, Language::Go);

        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("go.mod"), Language::Go);
        manifest.add_result(UpdateResult::update(dep, "v1.9.1"));
        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let formatter = DiffFormatter::new(false);
        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(
            output_str.contains(r#"+  "github.com/spf13/cobra": "v1.9.1""#),
            "{}",
            output_str
        );
        assert!(!output_str.contains("vv1.9.1"), "{}", output_str);
    }

    /// `+incompatible` を suffix に持つ Go 依存でも二重付与しない。
    #[test]
    fn test_go_diff_does_not_duplicate_incompatible_suffix() {
        let spec = VersionSpec::new(VersionSpecKind::Exact, "v2.0.0+incompatible", "2.0.0")
            .with_prefix("v")
            .with_suffix("+incompatible");
        let dep = Dependency::new("example.com/mod", spec, false, Language::Go);

        let mut summary = UpdateSummary::new(false);
        let mut manifest = ManifestUpdateResult::new(PathBuf::from("go.mod"), Language::Go);
        manifest.add_result(UpdateResult::update(dep, "v2.1.0+incompatible"));
        summary.add_manifest(manifest);

        let result = OrchestratorResult {
            summary,
            write_results: Vec::new(),
            errors: Vec::new(),
        };

        let formatter = DiffFormatter::new(false);
        let mut output = Vec::new();
        formatter.format(&result, &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();

        assert!(
            output_str.contains(r#"+  "example.com/mod": "v2.1.0+incompatible""#),
            "{}",
            output_str
        );
    }
}
