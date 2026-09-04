//! 依存関係情報の構造体

use super::{GitSource, Language, VersionSpec};
use serde::{Deserialize, Serialize};
use std::fmt;

/// パッケージ依存関係を表す
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// パッケージ名
    pub name: String,
    /// マニフェスト上の依存キー名。Cargo の `package` リネームなどで実パッケージ名と異なる場合に使う
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_name: Option<String>,
    /// バージョン指定
    pub version_spec: VersionSpec,
    /// 開発依存かどうか
    pub is_dev: bool,
    /// この依存関係が属する言語/エコシステム
    pub language: Language,
    /// バージョンが変数で定義されている場合のオプション変数名 (例: Gradle の def/val)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_name: Option<String>,
    /// git 依存 (Cargo.toml の `{ git = "..." }` など) の場合に設定される
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_source: Option<GitSource>,
    /// マニフェスト上の値に前置される接頭辞 (npm alias の `npm:<real>@` など)。
    ///
    /// `version_spec.raw` は制約部分 (`^17.0.0`) だけを保持するため、これが無いと
    /// `--diff` が `"react": "^17.0.0"` → `"^18.0.0"` と表示してしまう。実際に
    /// 書き込まれるのは `"npm:@preact/compat@^18.0.0"` なので、diff だけを見ると
    /// 「alias 宣言が外れる」と読めてレビュー結果と適用結果が食い違う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_prefix: Option<String>,
}

impl Dependency {
    /// 新しい依存関係を作成する
    pub fn new(
        name: impl Into<String>,
        version_spec: VersionSpec,
        is_dev: bool,
        language: Language,
    ) -> Self {
        Self {
            name: name.into(),
            manifest_name: None,
            version_spec,
            is_dev,
            language,
            variable_name: None,
            git_source: None,
            value_prefix: None,
        }
    }

    /// この依存関係の変数名を設定する (ビルダーパターン)
    pub fn with_variable(mut self, var_name: impl Into<String>) -> Self {
        self.variable_name = Some(var_name.into());
        self
    }

    /// マニフェスト上の値に前置される接頭辞を設定する (ビルダーパターン)
    pub fn with_value_prefix(mut self, value_prefix: impl Into<String>) -> Self {
        let value_prefix = value_prefix.into();
        if !value_prefix.is_empty() {
            self.value_prefix = Some(value_prefix);
        }
        self
    }

    /// マニフェストへ書かれる値の表示用文字列を組み立てる。
    ///
    /// npm alias のように制約の前に接頭辞が付く形式では、接頭辞込みでないと
    /// 実ファイルの内容と食い違う。`--diff` の before/after で使う。
    pub fn manifest_value(&self, constraint: &str) -> String {
        match &self.value_prefix {
            Some(prefix) => format!("{prefix}{constraint}"),
            None => constraint.to_string(),
        }
    }

    /// マニフェスト上の依存キー名を設定する (ビルダーパターン)
    pub fn with_manifest_name(mut self, manifest_name: impl Into<String>) -> Self {
        let manifest_name = manifest_name.into();
        if manifest_name != self.name {
            self.manifest_name = Some(manifest_name);
        }
        self
    }

    /// この依存関係に git ソースを設定する (ビルダーパターン)
    pub fn with_git_source(mut self, git_source: GitSource) -> Self {
        self.git_source = Some(git_source);
        self
    }

    /// git 依存かどうかを返す
    pub fn is_git(&self) -> bool {
        self.git_source.is_some()
    }

    /// 新しい本番依存関係を作成する
    pub fn production(
        name: impl Into<String>,
        version_spec: VersionSpec,
        language: Language,
    ) -> Self {
        Self::new(name, version_spec, false, language)
    }

    /// 新しい開発依存関係を作成する
    pub fn development(
        name: impl Into<String>,
        version_spec: VersionSpec,
        language: Language,
    ) -> Self {
        Self::new(name, version_spec, true, language)
    }

    /// この依存関係がピン留めされているかどうかを返す
    ///
    /// git 依存の場合は `rev = "..."` 指定のみ pinned と判定する。
    /// branch/tag/省略形はデフォルトで更新対象。
    pub fn is_pinned(&self) -> bool {
        if let Some(git) = &self.git_source {
            return git.reference.is_pinned();
        }
        self.version_spec.is_pinned()
    }

    /// 現在のバージョン文字列を返す
    pub fn version(&self) -> &str {
        &self.version_spec.version
    }

    /// マニフェストを書き戻すときに使う依存キー名を返す
    pub fn manifest_name(&self) -> &str {
        self.manifest_name.as_deref().unwrap_or(&self.name)
    }
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dev_marker = if self.is_dev { " (dev)" } else { "" };
        write!(
            f,
            "{}@{}{} [{}]",
            self.name, self.version_spec, dev_marker, self.language
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn sample_version_spec() -> VersionSpec {
        VersionSpec::new(VersionSpecKind::Caret, "^1.2.3", "1.2.3").with_prefix("^")
    }

    fn exact_version_spec() -> VersionSpec {
        VersionSpec::new(VersionSpecKind::Exact, "1.2.3", "1.2.3")
    }

    #[test]
    fn test_dependency_new() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert_eq!(dep.name, "lodash");
        assert!(dep.manifest_name.is_none());
        assert!(!dep.is_dev);
        assert_eq!(dep.language, Language::Node);
    }

    #[test]
    fn test_dependency_production() {
        let dep = Dependency::production("react", sample_version_spec(), Language::Node);
        assert_eq!(dep.name, "react");
        assert!(!dep.is_dev);
    }

    #[test]
    fn test_dependency_development() {
        let dep = Dependency::development("jest", sample_version_spec(), Language::Node);
        assert_eq!(dep.name, "jest");
        assert!(dep.is_dev);
    }

    #[test]
    fn test_dependency_is_pinned() {
        let pinned = Dependency::new("lodash", exact_version_spec(), false, Language::Node);
        assert!(pinned.is_pinned());

        let not_pinned = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert!(!not_pinned.is_pinned());
    }

    #[test]
    fn test_dependency_version() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert_eq!(dep.version(), "1.2.3");
    }

    #[test]
    fn test_dependency_display_production() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let display = format!("{}", dep);
        assert_eq!(display, "lodash@^1.2.3 [Node.js]");
    }

    #[test]
    fn test_dependency_display_development() {
        let dep = Dependency::new("jest", sample_version_spec(), true, Language::Node);
        let display = format!("{}", dep);
        assert_eq!(display, "jest@^1.2.3 (dev) [Node.js]");
    }

    #[test]
    fn test_dependency_different_languages() {
        let node_dep = Dependency::production("lodash", sample_version_spec(), Language::Node);
        assert_eq!(node_dep.language, Language::Node);

        let python_dep = Dependency::production(
            "requests",
            VersionSpec::new(VersionSpecKind::Caret, "^2.28.0", "2.28.0"),
            Language::Python,
        );
        assert_eq!(python_dep.language, Language::Python);

        let rust_dep = Dependency::production(
            "serde",
            VersionSpec::new(VersionSpecKind::Caret, "1.0", "1.0"),
            Language::Rust,
        );
        assert_eq!(rust_dep.language, Language::Rust);

        let go_dep = Dependency::production(
            "github.com/gin-gonic/gin",
            VersionSpec::new(VersionSpecKind::Exact, "v1.9.0", "1.9.0"),
            Language::Go,
        );
        assert_eq!(go_dep.language, Language::Go);
    }

    #[test]
    fn test_dependency_equality() {
        let dep1 = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let dep2 = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        assert_eq!(dep1, dep2);
    }

    #[test]
    fn test_dependency_clone() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let cloned = dep.clone();
        assert_eq!(dep, cloned);
    }

    #[test]
    fn test_serde_dependency() {
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let json = serde_json::to_string(&dep).unwrap();
        let parsed: Dependency = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dep);
    }

    #[test]
    fn test_dependency_with_variable() {
        let dep = Dependency::new("guava", sample_version_spec(), false, Language::Java)
            .with_variable("guavaVersion");
        assert_eq!(dep.variable_name, Some("guavaVersion".to_string()));
        assert_eq!(dep.name, "guava");
    }

    #[test]
    fn test_dependency_with_manifest_name() {
        let dep = Dependency::new("foo", sample_version_spec(), false, Language::Rust)
            .with_manifest_name("bar");
        assert_eq!(dep.name, "foo");
        assert_eq!(dep.manifest_name, Some("bar".to_string()));
        assert_eq!(dep.manifest_name(), "bar");
    }

    #[test]
    fn test_dependency_manifest_name_defaults_to_package_name() {
        let dep = Dependency::new("foo", sample_version_spec(), false, Language::Rust);
        assert_eq!(dep.manifest_name(), "foo");
    }

    #[test]
    fn test_dependency_with_variable_serde_skip() {
        // variable_name が None -> JSON に出現しないべき
        let dep = Dependency::new("lodash", sample_version_spec(), false, Language::Node);
        let json = serde_json::to_string(&dep).unwrap();
        assert!(!json.contains("variable_name"));

        // variable_name が Some -> JSON に出現するべき
        let dep_with_var = dep.with_variable("ver");
        let json_with_var = serde_json::to_string(&dep_with_var).unwrap();
        assert!(json_with_var.contains("variable_name"));
        assert!(json_with_var.contains("ver"));
    }

    #[test]
    fn test_dependency_display_all_languages() {
        let spec = sample_version_spec();

        let ruby_dep = Dependency::new("rails", spec.clone(), false, Language::Ruby);
        assert!(format!("{}", ruby_dep).contains("[Ruby]"));

        let php_dep = Dependency::new("laravel", spec.clone(), false, Language::Php);
        assert!(format!("{}", php_dep).contains("[PHP]"));

        let java_dep = Dependency::new("guava", spec, false, Language::Java);
        assert!(format!("{}", java_dep).contains("[Java]"));
    }
}
