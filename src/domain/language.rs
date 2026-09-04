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
    /// mise ツールチェーン (mise.toml, .tool-versions)
    Mise,
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
            Language::Mise => "mise.toml",
        }
    }

    /// この言語のロックファイル名を返す
    pub fn lock_filenames(&self) -> &'static [&'static str] {
        match self {
            Language::Node => &[
                "package-lock.json",
                "pnpm-lock.yaml",
                "yarn.lock",
                "bun.lock",
                "bun.lockb",
            ],
            Language::Python => &["uv.lock", "requirements.lock", "poetry.lock"],
            Language::Rust => &["Cargo.lock"],
            Language::Go => &["go.sum"],
            Language::Ruby => &["Gemfile.lock"],
            Language::Php => &["composer.lock"],
            Language::Java => &["gradle.lockfile"],
            Language::Swift => &["Package.resolved"],
            Language::Mise => &["mise.lock"],
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
            Language::Mise => "mise",
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
            Language::Mise,
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
    ///
    /// mise は `node = "26.7.0"` のような完全ピンが標準的な書き方で、
    /// 部分指定 (`node = "26"`) も「26 系の最新を都度解決する」プレフィックス指定に留まる。
    /// ツールチェーンを固定する用途のファイルであり、ピンを更新しないと
    /// 何も更新できないため `--include-pinned` を暗黙的に有効にする。
    pub fn always_pinned(&self) -> bool {
        matches!(self, Language::Go | Language::Mise)
    }

    /// この言語に対応する OSV.dev の ecosystem 名を返す。
    ///
    /// `None` の場合は OSV による脆弱性チェックの対象外。
    /// Swift は GitHub URL ベースで OSV に問い合わせる必要があり、本機能のスコープ外。
    ///
    /// mise はツール (node / python / terraform 等) を扱い、バックエンドごとに
    /// バージョン体系も名前空間も異なるため、単一の OSV ecosystem へは対応付けられない。
    ///
    /// 公式リスト: <https://ossf.github.io/osv-schema/#defined-ecosystems>
    pub fn osv_ecosystem(&self) -> Option<&'static str> {
        match self {
            Language::Node => Some("npm"),
            Language::Python => Some("PyPI"),
            Language::Rust => Some("crates.io"),
            Language::Go => Some("Go"),
            Language::Ruby => Some("RubyGems"),
            Language::Php => Some("Packagist"),
            Language::Java => Some("Maven"),
            Language::Swift | Language::Mise => None,
        }
    }

    /// `--age` を transitive (推移) 依存にも効かせられるパッケージマネージャを返す。
    ///
    /// 対応は**言語単位ではなく PM 単位**である点が重要。ここに載っていない PM が
    /// 選ばれた場合は direct 依存にしか age 制約がかからないため、`--verbose` で
    /// その旨を通知する。内訳:
    /// - pnpm (Node): v10.16+ の `npm_config_minimum_release_age` env var。
    ///   npm / yarn / bun には同等の手段が無い
    /// - uv (Python): `--exclude-newer`。pip / poetry / rye / pipenv には無い
    /// - cargo (Rust): `cargo update` 後の Cargo.lock 監査 (post-install audit)
    /// - mise: `mise install` へ渡す `MISE_MINIMUM_RELEASE_AGE` env var
    ///   (そもそも mise のツールは互いに推移依存を持たず、マニフェストに書かれた
    ///   ツールがそのまま解決対象なので「direct のみ」という取りこぼしが起きない)
    ///
    /// 一覧を `Language` 側に置くことで、言語を追加したときに通知漏れが起きない
    /// ようにする (match の網羅性検査が効く)。
    pub fn native_transitive_age_package_managers(&self) -> &'static [&'static str] {
        match self {
            Language::Node => &["pnpm"],
            Language::Python => &["uv"],
            Language::Rust => &["cargo"],
            Language::Mise => &["mise"],
            Language::Go | Language::Ruby | Language::Php | Language::Java | Language::Swift => &[],
        }
    }

    /// 指定のパッケージマネージャが transitive 依存へ age をネイティブ適用できるか。
    pub fn pm_has_native_transitive_age_support(&self, package_manager: &str) -> bool {
        self.native_transitive_age_package_managers()
            .contains(&package_manager)
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
            &[
                "package-lock.json",
                "pnpm-lock.yaml",
                "yarn.lock",
                "bun.lock",
                "bun.lockb"
            ]
        );
        assert_eq!(
            Language::Python.lock_filenames(),
            &["uv.lock", "requirements.lock", "poetry.lock"]
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
        assert_eq!(all.len(), 9);
        assert!(all.contains(&Language::Node));
        assert!(all.contains(&Language::Python));
        assert!(all.contains(&Language::Rust));
        assert!(all.contains(&Language::Go));
        assert!(all.contains(&Language::Ruby));
        assert!(all.contains(&Language::Php));
        assert!(all.contains(&Language::Java));
        assert!(all.contains(&Language::Swift));
        assert!(all.contains(&Language::Mise));
    }

    #[test]
    fn test_mise_language_metadata() {
        assert_eq!(Language::Mise.manifest_filename(), "mise.toml");
        assert_eq!(Language::Mise.lock_filenames(), &["mise.lock"]);
        assert_eq!(Language::Mise.display_name(), "mise");
        // mise はバージョン固定が標準の書き方なので --include-pinned なしで更新する
        assert!(Language::Mise.always_pinned());
        // ツールは OSV の単一 ecosystem に対応付けられない
        assert_eq!(Language::Mise.osv_ecosystem(), None);
        // `mise install` へ MISE_MINIMUM_RELEASE_AGE を渡せる
        assert!(Language::Mise.pm_has_native_transitive_age_support("mise"));
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
    fn test_osv_ecosystem() {
        assert_eq!(Language::Node.osv_ecosystem(), Some("npm"));
        assert_eq!(Language::Python.osv_ecosystem(), Some("PyPI"));
        assert_eq!(Language::Rust.osv_ecosystem(), Some("crates.io"));
        assert_eq!(Language::Go.osv_ecosystem(), Some("Go"));
        assert_eq!(Language::Ruby.osv_ecosystem(), Some("RubyGems"));
        assert_eq!(Language::Php.osv_ecosystem(), Some("Packagist"));
        assert_eq!(Language::Java.osv_ecosystem(), Some("Maven"));
        // Swift は GitHub URL ベースで対象外
        assert_eq!(Language::Swift.osv_ecosystem(), None);
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

    #[test]
    fn test_native_transitive_age_package_managers() {
        // ネイティブ手段あり: pnpm の env var / uv の --exclude-newer /
        // Rust の post-install lock 監査 / mise の env var
        assert!(Language::Node.pm_has_native_transitive_age_support("pnpm"));
        assert!(Language::Python.pm_has_native_transitive_age_support("uv"));
        assert!(Language::Rust.pm_has_native_transitive_age_support("cargo"));
        assert!(Language::Mise.pm_has_native_transitive_age_support("mise"));

        // 同じ言語でも PM が違えば direct 依存にしか age がかからない。
        // 言語単位で判定していたときは npm / yarn / bun / pip / poetry などでも
        // 「transitive にも効く」と誤って扱われ、通知が出ていなかった
        for pm in ["npm", "yarn", "bun"] {
            assert!(
                !Language::Node.pm_has_native_transitive_age_support(pm),
                "{pm} は transitive age 非対応"
            );
        }
        for pm in ["pip", "poetry", "rye", "pipenv"] {
            assert!(
                !Language::Python.pm_has_native_transitive_age_support(pm),
                "{pm} は transitive age 非対応"
            );
        }

        // ネイティブ手段が無い言語は PM を問わず非対応
        for language in [
            Language::Go,
            Language::Ruby,
            Language::Php,
            Language::Java,
            Language::Swift,
        ] {
            assert!(language.native_transitive_age_package_managers().is_empty());
        }

        // 通知文には display_name を使うため、全言語で空でないこと
        for language in Language::all() {
            assert!(!language.display_name().is_empty());
        }
    }
}
