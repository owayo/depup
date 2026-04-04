//! 対応パッケージエコシステムの言語型定義

use serde::{Deserialize, Serialize};
use std::fmt;

/// 対応するプログラミング言語/エコシステム
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Node.js エコシステム (package.json)
    Node,
    /// Python エコシステム (pyproject.toml)
    Python,
    /// Rust エコシステム (Cargo.toml)
    Rust,
    /// Go エコシステム (go.mod)
    Go,
    /// Ruby エコシステム (Gemfile)
    Ruby,
    /// PHP エコシステム (composer.json)
    Php,
    /// Java エコシステム (build.gradle, build.gradle.kts)
    Java,
    /// Swift エコシステム (Package.swift)
    Swift,
}

impl Language {
    /// この言語のマニフェストファイル名を返す
    pub fn manifest_filename(&self) -> &'static str {
        match self {
            Language::Node => "package.json",
            Language::Python => "pyproject.toml",
            Language::Rust => "Cargo.toml",
            Language::Go => "go.mod",
            Language::Ruby => "Gemfile",
            Language::Php => "composer.json",
            Language::Java => "build.gradle",
            Language::Swift => "Package.swift",
        }
    }

    /// この言語のロックファイル名を返す
    pub fn lock_filenames(&self) -> &'static [&'static str] {
        match self {
            Language::Node => &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"],
            Language::Python => &["uv.lock", "rye.lock", "poetry.lock"],
            Language::Rust => &["Cargo.lock"],
            Language::Go => &["go.sum"],
            Language::Ruby => &["Gemfile.lock"],
            Language::Php => &["composer.lock"],
            Language::Java => &["gradle.lockfile"],
            Language::Swift => &["Package.resolved"],
        }
    }

    /// この言語の表示名を返す
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::Node => "Node.js",
            Language::Python => "Python",
            Language::Rust => "Rust",
            Language::Go => "Go",
            Language::Ruby => "Ruby",
            Language::Php => "PHP",
            Language::Java => "Java",
            Language::Swift => "Swift",
        }
    }

    /// 対応する全言語を返す
    pub fn all() -> &'static [Language] {
        &[
            Language::Node,
            Language::Python,
            Language::Rust,
            Language::Go,
            Language::Ruby,
            Language::Php,
            Language::Java,
            Language::Swift,
        ]
    }

    /// この言語がピン留め/完全一致バージョンのみ対応するかどうかを返す
    ///
    /// Goはgo.modにレンジ指定子がないため、全バージョンが実質的にピン留めされる。
    /// この言語では `--include-pinned` が暗黙的に有効になるべき。
    ///
    /// 注: Java/Gradle はバージョンレンジに対応 (Maven形式レンジ、
    /// `1.+` のようなプレフィックスバージョン、`latest.release` のような動的バージョン)
    /// するため、ここには含まれない。
    pub fn always_pinned(&self) -> bool {
        matches!(self, Language::Go)
    }

    /// この言語がレジストリとしてGitHub Tags APIを使用するかどうかを返す
    pub fn uses_github_tags(&self) -> bool {
        matches!(self, Language::Swift)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_filenames() {
        assert_eq!(Language::Node.manifest_filename(), "package.json");
        assert_eq!(Language::Python.manifest_filename(), "pyproject.toml");
        assert_eq!(Language::Rust.manifest_filename(), "Cargo.toml");
        assert_eq!(Language::Go.manifest_filename(), "go.mod");
        assert_eq!(Language::Ruby.manifest_filename(), "Gemfile");
        assert_eq!(Language::Php.manifest_filename(), "composer.json");
        assert_eq!(Language::Java.manifest_filename(), "build.gradle");
        assert_eq!(Language::Swift.manifest_filename(), "Package.swift");
    }

    #[test]
    fn test_lock_filenames() {
        assert_eq!(
            Language::Node.lock_filenames(),
            &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"]
        );
        assert_eq!(
            Language::Python.lock_filenames(),
            &["uv.lock", "rye.lock", "poetry.lock"]
        );
        assert_eq!(Language::Rust.lock_filenames(), &["Cargo.lock"]);
        assert_eq!(Language::Go.lock_filenames(), &["go.sum"]);
        assert_eq!(Language::Ruby.lock_filenames(), &["Gemfile.lock"]);
        assert_eq!(Language::Php.lock_filenames(), &["composer.lock"]);
        assert_eq!(Language::Java.lock_filenames(), &["gradle.lockfile"]);
        assert_eq!(Language::Swift.lock_filenames(), &["Package.resolved"]);
    }

    #[test]
    fn test_display_names() {
        assert_eq!(Language::Node.display_name(), "Node.js");
        assert_eq!(Language::Python.display_name(), "Python");
        assert_eq!(Language::Rust.display_name(), "Rust");
        assert_eq!(Language::Go.display_name(), "Go");
        assert_eq!(Language::Ruby.display_name(), "Ruby");
        assert_eq!(Language::Php.display_name(), "PHP");
        assert_eq!(Language::Java.display_name(), "Java");
        assert_eq!(Language::Swift.display_name(), "Swift");
    }

    #[test]
    fn test_display_trait() {
        assert_eq!(format!("{}", Language::Node), "Node.js");
        assert_eq!(format!("{}", Language::Python), "Python");
        assert_eq!(format!("{}", Language::Rust), "Rust");
        assert_eq!(format!("{}", Language::Go), "Go");
        assert_eq!(format!("{}", Language::Ruby), "Ruby");
        assert_eq!(format!("{}", Language::Php), "PHP");
        assert_eq!(format!("{}", Language::Java), "Java");
        assert_eq!(format!("{}", Language::Swift), "Swift");
    }

    #[test]
    fn test_all_languages() {
        let all = Language::all();
        assert_eq!(all.len(), 8);
        assert!(all.contains(&Language::Node));
        assert!(all.contains(&Language::Python));
        assert!(all.contains(&Language::Rust));
        assert!(all.contains(&Language::Go));
        assert!(all.contains(&Language::Ruby));
        assert!(all.contains(&Language::Php));
        assert!(all.contains(&Language::Java));
        assert!(all.contains(&Language::Swift));
    }

    #[test]
    fn test_language_equality() {
        assert_eq!(Language::Node, Language::Node);
        assert_ne!(Language::Node, Language::Python);
    }

    #[test]
    fn test_language_clone() {
        let lang = Language::Rust;
        let cloned = lang;
        assert_eq!(lang, cloned);
    }

    #[test]
    fn test_language_debug() {
        let debug_str = format!("{:?}", Language::Node);
        assert_eq!(debug_str, "Node");
    }

    #[test]
    fn test_serde_serialization() {
        let lang = Language::Node;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"node\"");

        let lang = Language::Python;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"python\"");

        let lang = Language::Ruby;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"ruby\"");

        let lang = Language::Php;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"php\"");

        let lang = Language::Java;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"java\"");
    }

    #[test]
    fn test_serde_deserialization() {
        let lang: Language = serde_json::from_str("\"node\"").unwrap();
        assert_eq!(lang, Language::Node);

        let lang: Language = serde_json::from_str("\"rust\"").unwrap();
        assert_eq!(lang, Language::Rust);

        let lang: Language = serde_json::from_str("\"ruby\"").unwrap();
        assert_eq!(lang, Language::Ruby);

        let lang: Language = serde_json::from_str("\"php\"").unwrap();
        assert_eq!(lang, Language::Php);

        let lang: Language = serde_json::from_str("\"java\"").unwrap();
        assert_eq!(lang, Language::Java);
    }

    #[test]
    fn test_always_pinned() {
        // Goはピン留め/完全一致バージョンのみ対応 (go.modにレンジ構文なし)
        assert!(Language::Go.always_pinned());

        // Java/Gradle はバージョンレンジに対応 (Maven形式、プレフィックスバージョン、動的バージョン)
        assert!(!Language::Java.always_pinned());

        // 他の言語もレンジ指定子に対応
        assert!(!Language::Node.always_pinned());
        assert!(!Language::Python.always_pinned());
        assert!(!Language::Rust.always_pinned());
        assert!(!Language::Ruby.always_pinned());
        assert!(!Language::Php.always_pinned());
        assert!(!Language::Swift.always_pinned());
    }
}
