//! depup の統合テスト
//!
//! 次の項目を検証する:
//! - 複数言語のマニフェスト検出
//! - マニフェスト更新時の書式保持
//! - レジストリ応答の解析

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// テスト用ディレクトリを作成するヘルパー
fn create_test_dir() -> TempDir {
    tempfile::tempdir().expect("Failed to create temp directory")
}

mod manifest_detection {
    use super::*;

    /// 単一ディレクトリにある複数マニフェストの検出をテストする
    #[test]
    fn test_detect_multiple_languages() {
        let temp_dir = create_test_dir();

        // package.json（Node.js）を作成する
        let package_json = r#"{
            "name": "test-package",
            "dependencies": {
                "lodash": "^4.17.21"
            }
        }"#;
        fs::write(temp_dir.path().join("package.json"), package_json).unwrap();

        // pyproject.toml（Python）を作成する
        let pyproject = r#"[project]
name = "test-package"
dependencies = [
    "requests>=2.28.0"
]
"#;
        fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

        // Cargo.toml（Rust）を作成する
        let cargo_toml = r#"[package]
name = "test-package"
version = "0.1.0"

[dependencies]
serde = "1.0"
"#;
        fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

        // go.mod（Go）を作成する
        let go_mod = r#"module example.com/test

go 1.21

require github.com/gin-gonic/gin v1.9.0
"#;
        fs::write(temp_dir.path().join("go.mod"), go_mod).unwrap();

        // マニフェスト検出関数を使う
        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        // 4個すべてのマニフェストを検出する
        assert_eq!(manifests.len(), 4, "Should detect 4 manifest files");

        // 各言語が含まれることを確認する
        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(
            languages.contains(&depup::domain::Language::Node),
            "Should detect Node.js manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Python),
            "Should detect Python manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Rust),
            "Should detect Rust manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Go),
            "Should detect Go manifest"
        );
    }

    /// Ruby と PHP のマニフェスト検出をテストする
    #[test]
    fn test_detect_ruby_php_manifests() {
        let temp_dir = create_test_dir();

        // Gemfile（Ruby）を作成する
        let gemfile = r#"source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem 'pg', '~> 1.5'
"#;
        fs::write(temp_dir.path().join("Gemfile"), gemfile).unwrap();

        // composer.json（PHP）を作成する
        let composer_json = r#"{
    "require": {
        "laravel/framework": "^10.0",
        "monolog/monolog": "^3.0"
    }
}"#;
        fs::write(temp_dir.path().join("composer.json"), composer_json).unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 2, "Should detect 2 manifest files");

        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(
            languages.contains(&depup::domain::Language::Ruby),
            "Should detect Ruby manifest"
        );
        assert!(
            languages.contains(&depup::domain::Language::Php),
            "Should detect PHP manifest"
        );
    }

    /// 6言語すべての同時検出をテストする
    #[test]
    fn test_detect_all_six_languages() {
        let temp_dir = create_test_dir();

        // Node.js 用マニフェスト
        fs::write(
            temp_dir.path().join("package.json"),
            r#"{"dependencies": {"lodash": "^4.17.21"}}"#,
        )
        .unwrap();

        // Python 用マニフェスト
        fs::write(
            temp_dir.path().join("pyproject.toml"),
            r#"[project]
dependencies = ["requests>=2.28.0"]
"#,
        )
        .unwrap();

        // Rust 用マニフェスト
        fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0"
"#,
        )
        .unwrap();

        // Go 言語
        fs::write(
            temp_dir.path().join("go.mod"),
            r#"module example.com/test

go 1.21

require github.com/gin-gonic/gin v1.9.0
"#,
        )
        .unwrap();

        // Ruby 用マニフェスト
        fs::write(temp_dir.path().join("Gemfile"), r#"gem 'rails', '~> 7.0'"#).unwrap();

        // PHP 用マニフェスト
        fs::write(
            temp_dir.path().join("composer.json"),
            r#"{"require": {"laravel/framework": "^10.0"}}"#,
        )
        .unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 6, "Should detect all 6 manifest files");
    }

    /// 一部の言語だけにマニフェストがある場合をテストする
    #[test]
    fn test_detect_partial_manifests() {
        let temp_dir = create_test_dir();

        // Node.js と Python のマニフェストだけを作成する
        let package_json = r#"{"name": "test", "dependencies": {"express": "^4.18.0"}}"#;
        fs::write(temp_dir.path().join("package.json"), package_json).unwrap();

        let pyproject = r#"[project]
name = "test"
dependencies = ["flask>=2.0.0"]
"#;
        fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 2, "Should detect 2 manifest files");
    }

    /// 空のディレクトリをテストする
    #[test]
    fn test_detect_empty_directory() {
        let temp_dir = create_test_dir();
        let manifests = depup::manifest::detect_manifests(temp_dir.path());
        assert!(
            manifests.is_empty(),
            "Should detect no manifests in empty directory"
        );
    }

    /// 存在しないディレクトリをテストする
    #[test]
    fn test_detect_nonexistent_directory() {
        let manifests = depup::manifest::detect_manifests(&PathBuf::from("/nonexistent/path"));
        assert!(
            manifests.is_empty(),
            "Should return empty for non-existent directory"
        );
    }

    /// Java/Gradle マニフェスト（build.gradle と build.gradle.kts）の検出をテストする
    #[test]
    fn test_detect_gradle_manifests() {
        let temp_dir = create_test_dir();

        // build.gradle（Groovy DSL）を作成する
        let build_gradle = r#"plugins {
    id 'java'
}

dependencies {
    implementation 'com.google.guava:guava:33.0.0-jre'
    testImplementation 'junit:junit:4.13.2'
}
"#;
        fs::write(temp_dir.path().join("build.gradle"), build_gradle).unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 1, "Should detect build.gradle");
        assert_eq!(
            manifests[0].language,
            depup::domain::Language::Java,
            "Should detect Java language"
        );
    }

    /// go.work のメンバーモジュールが検出されることをテストする
    ///
    /// ルートに go.mod が無い構成では、展開しないと Go 依存が 1 件も更新されず
    /// 「更新なし」と報告してしまう (無言の no-op)。
    #[test]
    fn test_detect_go_work_member_modules() {
        let temp_dir = create_test_dir();
        let root = temp_dir.path();

        fs::write(
            root.join("go.work"),
            "go 1.23.0\n\nuse (\n\t./svc-a\n\t./svc-b\n)\n",
        )
        .unwrap();
        for member in ["svc-a", "svc-b"] {
            fs::create_dir_all(root.join(member)).unwrap();
            fs::write(
                root.join(member).join("go.mod"),
                format!("module example.com/{member}\n\ngo 1.23.0\n"),
            )
            .unwrap();
        }

        let manifests = depup::manifest::detect_manifests(root);
        let go_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == depup::domain::Language::Go)
            .collect();

        assert_eq!(
            go_manifests.len(),
            2,
            "go.work の use に挙げた 2 モジュールが検出されるべき: {manifests:?}"
        );
        for member in ["svc-a", "svc-b"] {
            assert!(
                go_manifests
                    .iter()
                    .any(|m| m.path == root.join(member).join("go.mod")),
                "{member}/go.mod が検出されるべき"
            );
        }
    }

    /// go.work が参照するモジュールとルートの go.mod が重複しないことをテストする
    #[test]
    fn test_detect_go_work_does_not_duplicate_root_module() {
        let temp_dir = create_test_dir();
        let root = temp_dir.path();

        fs::write(root.join("go.work"), "go 1.23.0\n\nuse .\n").unwrap();
        fs::write(
            root.join("go.mod"),
            "module example.com/root\n\ngo 1.23.0\n",
        )
        .unwrap();

        let manifests = depup::manifest::detect_manifests(root);
        let go_count = manifests
            .iter()
            .filter(|m| m.language == depup::domain::Language::Go)
            .count();

        assert_eq!(go_count, 1, "ルートの go.mod は 1 件だけであるべき");
    }

    /// settings.gradle の include からサブプロジェクトが検出されることをテストする
    ///
    /// 依存宣言の大半はサブプロジェクト側にあるため、ルートだけ見ていると
    /// 「更新なし」と報告してしまう。
    #[test]
    fn test_detect_gradle_subprojects_from_settings() {
        let temp_dir = create_test_dir();
        let root = temp_dir.path();

        fs::write(
            root.join("settings.gradle"),
            "rootProject.name = 'demo'\ninclude ':app', ':core'\n",
        )
        .unwrap();
        for module in ["app", "core"] {
            fs::create_dir_all(root.join(module)).unwrap();
            fs::write(
                root.join(module).join("build.gradle"),
                "dependencies {\n    implementation 'com.google.guava:guava:33.0.0-jre'\n}\n",
            )
            .unwrap();
        }

        let manifests = depup::manifest::detect_manifests(root);
        let java_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == depup::domain::Language::Java)
            .collect();

        assert_eq!(
            java_manifests.len(),
            2,
            "include に挙げた 2 サブプロジェクトが検出されるべき: {manifests:?}"
        );
        for module in ["app", "core"] {
            assert!(
                java_manifests
                    .iter()
                    .any(|m| m.path == root.join(module).join("build.gradle")),
                "{module}/build.gradle が検出されるべき"
            );
        }
    }

    /// Kotlin DSL の settings.gradle.kts でもサブプロジェクトを検出することをテストする
    #[test]
    fn test_detect_gradle_subprojects_kotlin_dsl() {
        let temp_dir = create_test_dir();
        let root = temp_dir.path();

        fs::write(
            root.join("settings.gradle.kts"),
            "rootProject.name = \"demo\"\ninclude(\n    \":app\",\n    \":core:api\",\n)\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(
            root.join("app").join("build.gradle.kts"),
            "dependencies {\n    implementation(\"com.google.guava:guava:33.0.0-jre\")\n}\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("core").join("api")).unwrap();
        fs::write(
            root.join("core").join("api").join("build.gradle"),
            "dependencies {\n    implementation 'junit:junit:4.13.2'\n}\n",
        )
        .unwrap();

        let manifests = depup::manifest::detect_manifests(root);
        let java_paths: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == depup::domain::Language::Java)
            .map(|m| m.path.clone())
            .collect();

        assert!(
            java_paths.contains(&root.join("app").join("build.gradle.kts")),
            "app/build.gradle.kts が検出されるべき: {java_paths:?}"
        );
        assert!(
            java_paths.contains(&root.join("core").join("api").join("build.gradle")),
            "core/api/build.gradle が検出されるべき: {java_paths:?}"
        );
    }

    /// build.gradle.kts（Kotlin DSL）の検出をテストする
    #[test]
    fn test_detect_gradle_kts_manifest() {
        let temp_dir = create_test_dir();

        let build_gradle_kts = r#"plugins {
    java
}

dependencies {
    implementation("com.google.guava:guava:33.0.0-jre")
    testImplementation("junit:junit:4.13.2")
}
"#;
        fs::write(temp_dir.path().join("build.gradle.kts"), build_gradle_kts).unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 1, "Should detect build.gradle.kts");
        assert_eq!(
            manifests[0].language,
            depup::domain::Language::Java,
            "Should detect Java language from .kts"
        );
    }

    /// Gradle version catalog (gradle/libs.versions.toml) の検出とパースの統合テスト
    #[test]
    fn test_detect_and_parse_gradle_version_catalog() {
        let temp_dir = create_test_dir();
        fs::create_dir_all(temp_dir.path().join("gradle")).unwrap();

        let catalog = r#"[versions]
guava = "33.0.0-jre"

[libraries]
guava = { module = "com.google.guava:guava", version.ref = "guava" }
junit = "junit:junit:4.13.2"
commons-lang3 = { group = "org.apache.commons", name = "commons-lang3", version = "3.14.0" }

[plugins]
spotless = { id = "com.diffplug.spotless", version = "6.25.0" }
"#;
        let catalog_path = temp_dir.path().join("gradle").join("libs.versions.toml");
        fs::write(&catalog_path, catalog).unwrap();

        // 検出: version catalog は Java マニフェストとして扱われる
        let manifests = depup::manifest::detect_manifests(temp_dir.path());
        let catalog_manifest = manifests
            .iter()
            .find(|m| m.path == catalog_path)
            .expect("version catalog が検出されるべき");
        assert_eq!(catalog_manifest.language, depup::domain::Language::Java);

        // パース: libraries は Maven 座標で抽出し、plugins は除外する
        let parser = depup::manifest::get_parser(depup::domain::Language::Java);
        let deps = parser.parse(catalog).unwrap();

        assert!(
            deps.iter()
                .any(|d| d.name == "com.google.guava:guava"
                    && d.version_spec.version == "33.0.0-jre")
        );
        assert!(
            deps.iter()
                .any(|d| d.name == "junit:junit" && d.version_spec.version == "4.13.2")
        );
        assert!(
            deps.iter()
                .any(|d| d.name == "org.apache.commons:commons-lang3"
                    && d.version_spec.version == "3.14.0")
        );
        // plugins は Maven 座標と一致しないため更新対象から除外される
        assert!(
            !deps
                .iter()
                .any(|d| d.name.contains("spotless") || d.name.contains("com.diffplug"))
        );
        assert_eq!(deps.len(), 3, "libraries 3 件のみ (plugins は除外)");
    }

    /// Package.swift ファイルの検出テスト
    #[test]
    fn test_detect_swift_manifest() {
        let temp_dir = create_test_dir();

        // Package.swift（Swift）を作成する
        let package_swift = r#"// swift-tools-version:5.9
import PackageDescription
let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.0.0"),
    ],
    targets: [.target(name: "MyApp")]
)
"#;
        fs::write(temp_dir.path().join("Package.swift"), package_swift).unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 1, "Package.swift を検出すべき");
        assert_eq!(
            manifests[0].language,
            depup::domain::Language::Swift,
            "Swift 言語として検出されるべき"
        );
    }

    /// Java を含む7言語すべての検出をテストする
    #[test]
    fn test_detect_all_seven_languages() {
        let temp_dir = create_test_dir();

        // Node.js 用マニフェスト
        fs::write(
            temp_dir.path().join("package.json"),
            r#"{"dependencies": {"lodash": "^4.17.21"}}"#,
        )
        .unwrap();

        // Python 用マニフェスト
        fs::write(
            temp_dir.path().join("pyproject.toml"),
            "[project]\ndependencies = [\"requests>=2.28.0\"]\n",
        )
        .unwrap();

        // Rust 用マニフェスト
        fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n",
        )
        .unwrap();

        // Go 言語
        fs::write(
            temp_dir.path().join("go.mod"),
            "module example.com/test\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.0\n",
        )
        .unwrap();

        // Ruby 用マニフェスト
        fs::write(temp_dir.path().join("Gemfile"), "gem 'rails', '~> 7.0'\n").unwrap();

        // PHP 用マニフェスト
        fs::write(
            temp_dir.path().join("composer.json"),
            r#"{"require": {"laravel/framework": "^10.0"}}"#,
        )
        .unwrap();

        // Java 用マニフェスト
        fs::write(
            temp_dir.path().join("build.gradle"),
            "dependencies {\n    implementation 'com.google.guava:guava:33.0.0-jre'\n}\n",
        )
        .unwrap();

        let manifests = depup::manifest::detect_manifests(temp_dir.path());

        assert_eq!(manifests.len(), 7, "Should detect all 7 manifest files");

        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(
            languages.contains(&depup::domain::Language::Java),
            "Should detect Java manifest"
        );
    }
}

mod manifest_update_format_preservation {
    use depup::domain::{Language, VersionSpecKind};
    use depup::manifest::get_parser;

    /// caret バージョンを持つ package.json の書式保持をテストする
    #[test]
    fn test_package_json_caret_preservation() {
        let content = r#"{
  "name": "test",
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#;

        let parser = get_parser(Language::Node);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "lodash");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);

        // バージョンを更新する
        let updated = parser.update_version(content, "lodash", "4.18.0").unwrap();
        assert!(
            updated.contains("\"^4.18.0\""),
            "Should preserve caret prefix: {}",
            updated
        );
    }

    /// tilde バージョンを持つ package.json の書式保持をテストする
    #[test]
    fn test_package_json_tilde_preservation() {
        let content = r#"{
  "dependencies": {
    "express": "~4.18.0"
  }
}"#;

        let parser = get_parser(Language::Node);
        let updated = parser.update_version(content, "express", "4.19.0").unwrap();
        assert!(
            updated.contains("\"~4.19.0\""),
            "Should preserve tilde prefix: {}",
            updated
        );
    }

    /// 回帰テスト: 部分指定の tilde はマニフェスト書き換え経路でもセグメント数を保つ。
    ///
    /// Node のパーサは比較用バージョンを 3 セグメントへ 0 埋め正規化するため、
    /// セグメント数を比較用バージョンから数えていた頃は Node だけ保持が効かず、
    /// `~1` (= `>=1.0.0 <2.0.0`) が `~2.5.3` (= `>=2.5.3 <2.6.0`) へ狭まっていた。
    #[test]
    fn test_package_json_partial_tilde_preserves_segment_count() {
        let content = r#"{
  "dependencies": {
    "glob": "~10.3",
    "chalk": "~4"
  }
}"#;

        let parser = get_parser(Language::Node);

        let updated = parser.update_version(content, "glob", "13.0.6").unwrap();
        assert!(
            updated.contains(r#""glob": "~13.0""#),
            "2 セグメントの tilde はセグメント数を保つべき: {}",
            updated
        );
        assert!(
            !updated.contains("~13.0.6"),
            "セグメント数を増やしてはいけない: {}",
            updated
        );

        let updated = parser.update_version(content, "chalk", "5.6.2").unwrap();
        assert!(
            updated.contains(r#""chalk": "~5""#),
            "1 セグメントの tilde は major 幅を保つべき: {}",
            updated
        );
    }

    /// pyproject.toml の書式保持をテストする
    #[test]
    fn test_pyproject_toml_gte_preservation() {
        let content = r#"[project]
dependencies = [
    "requests>=2.28.0",
]
"#;

        let parser = get_parser(Language::Python);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::GreaterOrEqual);

        let updated = parser
            .update_version(content, "requests", "2.31.0")
            .unwrap();
        assert!(
            updated.contains(">=2.31.0"),
            "Should preserve >= prefix: {}",
            updated
        );
    }

    /// Cargo.toml の書式保持をテストする
    #[test]
    fn test_cargo_toml_bare_version_preservation() {
        let content = r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0.190"
"#;

        let parser = get_parser(Language::Rust);
        let deps = parser.parse(content).unwrap();

        assert!(deps.iter().any(|d| d.name == "serde"));

        let updated = parser.update_version(content, "serde", "1.0.195").unwrap();
        // Cargo の演算子なしバージョンを維持する（接頭辞なし）
        assert!(
            updated.contains("\"1.0.195\""),
            "Should update bare version: {}",
            updated
        );
    }

    /// Cargo.toml のインラインテーブル書式保持をテストする
    #[test]
    fn test_cargo_toml_inline_table_preservation() {
        let content = r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
tokio = { version = "1.35", features = ["full"] }
"#;

        let parser = get_parser(Language::Rust);
        let updated = parser.update_version(content, "tokio", "1.40").unwrap();

        // インラインテーブルの書式を維持する
        assert!(
            updated.contains("{ version = \"1.40\"") || updated.contains("{version = \"1.40\""),
            "Should preserve inline table: {}",
            updated
        );
        assert!(
            updated.contains("features = [\"full\"]"),
            "Should preserve features: {}",
            updated
        );
    }

    /// go.mod の書式保持をテストする
    #[test]
    fn test_go_mod_v_prefix_preservation() {
        let content = r#"module example.com/test

go 1.21

require github.com/gin-gonic/gin v1.9.0
"#;

        let parser = get_parser(Language::Go);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/gin-gonic/gin");

        let updated = parser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(
            updated.contains("v1.10.0"),
            "Should preserve v prefix: {}",
            updated
        );
    }

    /// go.mod のコメント保持をテストする
    #[test]
    fn test_go_mod_comment_preservation() {
        let content = r#"module example.com/test

go 1.21

require (
    github.com/gin-gonic/gin v1.9.0 // indirect
    github.com/stretchr/testify v1.8.0 // pinned
)
"#;

        let parser = get_parser(Language::Go);
        let deps = parser.parse(content).unwrap();

        // stretchr/testify は固定指定として扱われる
        let testify = deps.iter().find(|d| d.name.contains("testify"));
        assert!(testify.is_some());
        assert!(
            testify.unwrap().version_spec.is_pinned(),
            "Should detect pinned comment"
        );

        // gin を更新する
        let updated = parser
            .update_version(content, "github.com/gin-gonic/gin", "v1.10.0")
            .unwrap();
        assert!(
            updated.contains("// indirect"),
            "Should preserve comments: {}",
            updated
        );
    }

    /// Gemfile の悲観的制約（~>）保持をテストする
    #[test]
    fn test_gemfile_pessimistic_preservation() {
        let content = r#"source 'https://rubygems.org'

gem 'rails', '~> 7.0'
gem 'pg', '~> 1.5'
"#;

        let parser = get_parser(Language::Ruby);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 2);

        let rails = deps.iter().find(|d| d.name == "rails").unwrap();
        assert_eq!(rails.version_spec.kind, VersionSpecKind::Tilde);

        let updated = parser.update_version(content, "rails", "7.1.0").unwrap();
        // `~> 7.0` は `>= 7.0, < 8.0`。許容幅を保つためセグメント数も維持する
        // (`~> 7.1.0` にすると上限が `< 7.2` へ黙って縮まる)
        assert!(
            updated.contains("'~> 7.1'"),
            "悲観的制約の演算子とセグメント数を保持すべき: {}",
            updated
        );
        assert!(
            !updated.contains("'~> 7.1.0'"),
            "セグメント数を増やして許容幅を狭めてはいけない: {}",
            updated
        );
    }

    /// Gemfile の固定バージョン保持をテストする
    #[test]
    fn test_gemfile_exact_version_preservation() {
        let content = r#"gem 'bcrypt', '3.1.18'"#;

        let parser = get_parser(Language::Ruby);
        let updated = parser.update_version(content, "bcrypt", "3.1.20").unwrap();
        assert!(
            updated.contains("'3.1.20'"),
            "Should preserve exact version format: {}",
            updated
        );
    }

    /// Gemfile の二重引用符保持をテストする
    #[test]
    fn test_gemfile_double_quotes_preservation() {
        let content = r#"gem "rails", "~> 7.0""#;

        let parser = get_parser(Language::Ruby);
        let updated = parser.update_version(content, "rails", "7.1.0").unwrap();
        // 二重引用符と併せて `~>` のセグメント数も維持する
        assert!(
            updated.contains("\"~> 7.1\""),
            "二重引用符を保持すべき: {}",
            updated
        );
    }

    /// composer.json の caret 保持をテストする
    #[test]
    fn test_composer_json_caret_preservation() {
        let content = r#"{
  "require": {
    "laravel/framework": "^10.0"
  }
}"#;

        let parser = get_parser(Language::Php);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "laravel/framework");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);

        let updated = parser
            .update_version(content, "laravel/framework", "10.5.0")
            .unwrap();
        assert!(
            updated.contains("\"^10.5.0\""),
            "Should preserve caret prefix: {}",
            updated
        );
    }

    /// composer.json の tilde 保持をテストする
    #[test]
    fn test_composer_json_tilde_preservation() {
        let content = r#"{
  "require": {
    "symfony/console": "~6.0"
  }
}"#;

        let parser = get_parser(Language::Php);
        let updated = parser
            .update_version(content, "symfony/console", "6.4.0")
            .unwrap();
        // Composer の `~6.0` は `>=6.0 <7.0`。セグメント数を保って `~6.4` にする
        // (`~6.4.0` にすると上限が `<6.5.0` へ縮まってしまう)
        assert!(
            updated.contains("\"~6.4\""),
            "tilde 接頭辞とセグメント数を保持すべき: {}",
            updated
        );
        assert!(
            !updated.contains("\"~6.4.0\""),
            "セグメント数を増やして許容幅を狭めてはいけない: {}",
            updated
        );
    }

    /// composer.json のワイルドカード保持をテストする
    #[test]
    fn test_composer_json_wildcard_preservation() {
        let content = r#"{
  "require": {
    "vendor/package": "1.2.*"
  }
}"#;

        let parser = get_parser(Language::Php);
        let updated = parser
            .update_version(content, "vendor/package", "1.3.4")
            .unwrap();
        assert!(
            updated.contains("\"1.3.*\""),
            "Should preserve wildcard format: {}",
            updated
        );
    }

    /// Gradle の文字列記法の解析と更新をテストする
    #[test]
    fn test_gradle_string_notation_preservation() {
        let content = r#"plugins {
    id 'java'
}

dependencies {
    implementation 'com.google.guava:guava:33.0.0-jre'
    testImplementation 'junit:junit:4.13.2'
}
"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        assert!(
            deps.iter().any(|d| d.name == "com.google.guava:guava"),
            "Should parse Gradle string notation dependency"
        );

        let updated = parser
            .update_version(content, "com.google.guava:guava", "33.1.0-jre")
            .unwrap();
        assert!(
            updated.contains("33.1.0-jre"),
            "Should update Gradle version: {}",
            updated
        );
    }

    /// Gradle Kotlin DSL の文字列記法をテストする
    #[test]
    fn test_gradle_kts_string_notation_preservation() {
        let content = r#"plugins {
    java
}

dependencies {
    implementation("com.google.guava:guava:33.0.0-jre")
    testImplementation("junit:junit:4.13.2")
}
"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        assert!(
            deps.iter().any(|d| d.name == "com.google.guava:guava"),
            "Should parse Kotlin DSL string notation"
        );
    }

    /// Gradle の変数によるバージョン定義をテストする
    #[test]
    fn test_gradle_variable_version() {
        let content = r#"
def guavaVersion = '33.0.0-jre'

dependencies {
    implementation "com.google.guava:guava:$guavaVersion"
}
"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        let guava = deps.iter().find(|d| d.name == "com.google.guava:guava");
        assert!(guava.is_some(), "Should parse variable-based version");

        if let Some(dep) = guava {
            assert_eq!(dep.version_spec.version, "33.0.0-jre");
        }
    }

    /// Gradle の開発依存検出をテストする
    #[test]
    fn test_gradle_dev_dependency_detection() {
        let content = r#"dependencies {
    implementation 'com.google.guava:guava:33.0.0-jre'
    testImplementation 'junit:junit:4.13.2'
}
"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        let guava = deps
            .iter()
            .find(|d| d.name == "com.google.guava:guava")
            .unwrap();
        assert!(!guava.is_dev, "implementation should not be dev dependency");

        let junit = deps.iter().find(|d| d.name == "junit:junit").unwrap();
        assert!(junit.is_dev, "testImplementation should be dev dependency");
    }

    /// composer.json の require-dev 解析をテストする
    #[test]
    fn test_composer_json_require_dev() {
        let content = r#"{
  "require": {
    "laravel/framework": "^10.0"
  },
  "require-dev": {
    "phpunit/phpunit": "^10.0"
  }
}"#;

        let parser = get_parser(Language::Php);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 2);

        let phpunit = deps.iter().find(|d| d.name == "phpunit/phpunit").unwrap();
        assert!(phpunit.is_dev, "Should mark require-dev as dev dependency");

        let laravel = deps.iter().find(|d| d.name == "laravel/framework").unwrap();
        assert!(!laravel.is_dev, "Should mark require as non-dev dependency");
    }

    /// Package.swift の半開区間 (..<) で下限更新と上限保持をテストする
    #[test]
    fn test_package_swift_half_open_range_preservation() {
        let content = r#"// swift-tools-version:5.9
import PackageDescription
let package = Package(
    name: "Test",
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio.git", "1.0.0"..<"2.0.0"),
    ],
    targets: [.target(name: "Test")]
)
"#;

        let parser = get_parser(Language::Swift);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);

        let updated = parser
            .update_version(content, "apple/swift-nio", "1.5.0")
            .unwrap();
        assert!(
            updated.contains(r#""1.5.0"..<"2.0.0""#),
            "半開区間の上限が保持されるべき: {}",
            updated
        );
    }

    /// Package.swift の閉区間 (...) で下限更新と上限保持をテストする
    #[test]
    fn test_package_swift_closed_range_preservation() {
        let content = r#"// swift-tools-version:5.9
import PackageDescription
let package = Package(
    name: "Test",
    dependencies: [
        .package(url: "https://github.com/apple/swift-log.git", "1.0.0"..."2.0.0"),
    ],
    targets: [.target(name: "Test")]
)
"#;

        let parser = get_parser(Language::Swift);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-log");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);

        let updated = parser
            .update_version(content, "apple/swift-log", "1.5.0")
            .unwrap();
        assert!(
            updated.contains(r#""1.5.0"..."2.0.0""#),
            "閉区間の上限が保持されるべき: {}",
            updated
        );
    }

    /// npm alias (npm:@scope/pkg@^1.0) のパースと更新をテストする
    #[test]
    fn test_package_json_npm_alias_preservation() {
        let content = r#"{
  "name": "test",
  "dependencies": {
    "my-lodash": "npm:lodash@^4.17.21"
  }
}"#;

        let parser = get_parser(Language::Node);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        // npm alias の場合、レジストリ照会は実パッケージ名、書き戻しはキー名
        assert_eq!(deps[0].name, "lodash");
        assert_eq!(deps[0].manifest_name(), "my-lodash");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);

        let updated = parser
            .update_version(content, "my-lodash", "4.18.0")
            .unwrap();
        assert!(
            updated.contains("npm:lodash@^4.18.0"),
            "npm alias プレフィックスが保持されるべき: {}",
            updated
        );
    }

    /// Maven 半開区間の下限更新をテストする
    #[test]
    fn test_gradle_maven_range_lower_bound_update() {
        let content = r#"dependencies {
    implementation("org.example:mylib:[1.0,2.0)")
}"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);

        let updated = parser
            .update_version(content, "org.example:mylib", "1.5.0")
            .unwrap();
        assert!(
            updated.contains("[1.5.0,2.0)"),
            "Maven レンジの下限が更新されるべき: {}",
            updated
        );
    }

    /// Maven Hard requirement (`[1.0]`) は Exact と同義として扱われ、ブラケットを保持して更新される
    #[test]
    fn test_gradle_maven_hard_requirement_update() {
        let content = r#"dependencies {
    implementation("org.example:mylib:[1.0.0]")
}"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "1.0.0");
        // Hard requirement は固定指定なので pinned 扱い
        assert!(deps[0].version_spec.is_pinned());

        let updated = parser
            .update_version(content, "org.example:mylib", "2.0.0")
            .unwrap();
        assert!(
            updated.contains("[2.0.0]"),
            "Maven Hard requirement のブラケット表記が保持されるべき: {}",
            updated
        );
    }

    /// Maven Hard requirement (Kotlin DSL での書き方も含む)
    #[test]
    fn test_gradle_kts_maven_hard_requirement_update() {
        let content = r#"dependencies {
    implementation("org.springframework:spring-core:[5.3.8]")
}"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "5.3.8");

        let updated = parser
            .update_version(content, "org.springframework:spring-core", "5.4.0")
            .unwrap();
        assert!(
            updated.contains("[5.4.0]"),
            "Hard requirement のブラケット表記が更新後も保持されるべき: {}",
            updated
        );
    }
}

mod registry_response_parsing {
    use chrono::{TimeZone, Utc};
    use depup::update::VersionInfo;

    /// npm の JSON 応答解析をテストする
    #[test]
    fn test_npm_response_structure() {
        // npm レジストリの応答構造を再現する
        let npm_response = r#"{
            "time": {
                "4.17.21": "2021-02-20T15:30:00.000Z",
                "4.17.20": "2021-01-12T10:00:00.000Z"
            },
            "versions": {
                "4.17.21": {},
                "4.17.20": {}
            }
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(npm_response).unwrap();

        let time = parsed.get("time").unwrap().as_object().unwrap();
        assert_eq!(time.len(), 2);
        assert!(time.contains_key("4.17.21"));

        let versions = parsed.get("versions").unwrap().as_object().unwrap();
        assert_eq!(versions.len(), 2);
    }

    /// PyPI の JSON 応答解析をテストする
    #[test]
    fn test_pypi_response_structure() {
        // PyPI JSON API の応答構造を再現する
        let pypi_response = r#"{
            "releases": {
                "2.28.0": [
                    {"upload_time_iso_8601": "2022-06-14T15:00:00.000Z"}
                ],
                "2.31.0": [
                    {"upload_time_iso_8601": "2023-05-22T15:00:00.000Z"}
                ]
            }
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(pypi_response).unwrap();

        let releases = parsed.get("releases").unwrap().as_object().unwrap();
        assert_eq!(releases.len(), 2);

        let v2_31 = releases.get("2.31.0").unwrap().as_array().unwrap();
        assert!(!v2_31.is_empty());
        assert!(v2_31[0].get("upload_time_iso_8601").is_some());
    }

    /// crates.io の JSON 応答解析をテストする
    #[test]
    fn test_crates_io_response_structure() {
        // crates.io API の応答構造を再現する
        let crates_response = r#"{
            "versions": [
                {"num": "1.0.195", "created_at": "2024-01-15T10:00:00.000Z"},
                {"num": "1.0.194", "created_at": "2024-01-10T10:00:00.000Z"}
            ]
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(crates_response).unwrap();

        let versions = parsed.get("versions").unwrap().as_array().unwrap();
        assert_eq!(versions.len(), 2);

        let v195 = &versions[0];
        assert_eq!(v195.get("num").unwrap().as_str().unwrap(), "1.0.195");
        assert!(v195.get("created_at").is_some());
    }

    /// Go Proxy のプレーンテキスト応答解析をテストする
    #[test]
    fn test_go_proxy_list_response() {
        // Go Proxy の /@v/list プレーンテキスト応答を再現する
        let go_list_response = "v1.9.0\nv1.9.1\nv1.10.0\n";

        let versions: Vec<&str> = go_list_response.lines().collect();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0], "v1.9.0");
        assert_eq!(versions[2], "v1.10.0");
    }

    /// Go Proxy の .info 応答をテストする
    #[test]
    fn test_go_proxy_info_response() {
        // Go Proxy の /@v/version.info 応答を再現する
        let go_info_response = r#"{
            "Version": "v1.10.0",
            "Time": "2024-01-20T15:00:00Z"
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(go_info_response).unwrap();
        assert_eq!(parsed.get("Version").unwrap().as_str().unwrap(), "v1.10.0");
        assert!(parsed.get("Time").is_some());
    }

    /// VersionInfo の並べ替えをテストする
    #[test]
    fn test_version_info_sorting() {
        let v1 = VersionInfo::new("1.0.0", Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        let v2 = VersionInfo::new("1.0.1", Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap());
        let v3 = VersionInfo::new("1.1.0", Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap());

        let mut versions = [v3.clone(), v1.clone(), v2.clone()];
        versions.sort();

        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[1].version, "1.0.1");
        assert_eq!(versions[2].version, "1.1.0");
    }

    /// バージョン比較のエッジケースをテストする
    #[test]
    fn test_version_comparison_edge_cases() {
        let now = Utc::now();

        let prerelease = VersionInfo::new("1.0.0-alpha", now);
        let stable = VersionInfo::new("1.0.0", now);
        let patch = VersionInfo::new("1.0.1", now);

        // semver 仕様: 数値コアが等しい場合、プレリリース付きは安定版より小さい。
        // プレリリースは同じコアの安定版より古い
        assert!(prerelease < stable);
        // 安定版は次のパッチ版より古い
        assert!(stable < patch);
        // プレリリースも次のパッチ版より古い
        assert!(prerelease < patch);
    }

    /// RubyGems の JSON 応答解析をテストする
    #[test]
    fn test_rubygems_response_structure() {
        // RubyGems API の応答構造を再現する
        let rubygems_response = r#"[
            {"number": "7.1.0", "created_at": "2023-10-05T12:00:00Z", "platform": "ruby", "yanked": false},
            {"number": "7.0.8", "created_at": "2023-09-01T12:00:00Z", "platform": "ruby", "yanked": false},
            {"number": "7.0.7", "created_at": "2023-08-15T12:00:00Z", "platform": "ruby", "yanked": true}
        ]"#;

        let parsed: Vec<serde_json::Value> = serde_json::from_str(rubygems_response).unwrap();

        assert_eq!(parsed.len(), 3);

        let v710 = &parsed[0];
        assert_eq!(v710.get("number").unwrap().as_str().unwrap(), "7.1.0");
        assert!(!v710.get("yanked").unwrap().as_bool().unwrap());
        assert!(v710.get("created_at").is_some());

        // 3番目のバージョンは yanked 扱い
        let v707 = &parsed[2];
        assert!(v707.get("yanked").unwrap().as_bool().unwrap());
    }

    /// Packagist の JSON 応答解析をテストする
    #[test]
    fn test_packagist_response_structure() {
        // Packagist p2 API の応答構造を再現する
        let packagist_response = r#"{
            "packages": {
                "laravel/framework": [
                    {"version": "v10.0.0", "version_normalized": "10.0.0.0", "time": "2023-02-14T15:00:00+00:00"},
                    {"version": "v9.0.0", "version_normalized": "9.0.0.0", "time": "2022-02-08T15:00:00+00:00"},
                    {"version": "dev-master", "version_normalized": "dev-master", "time": "2024-01-01T00:00:00+00:00"}
                ]
            }
        }"#;

        let parsed: serde_json::Value = serde_json::from_str(packagist_response).unwrap();

        let packages = parsed.get("packages").unwrap().as_object().unwrap();
        assert!(packages.contains_key("laravel/framework"));

        let versions = packages
            .get("laravel/framework")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(versions.len(), 3);

        let v10 = &versions[0];
        assert_eq!(v10.get("version").unwrap().as_str().unwrap(), "v10.0.0");
        assert!(v10.get("time").is_some());

        // 実際の実装では開発版を除外する
        let dev = &versions[2];
        assert!(
            dev.get("version")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("dev")
        );
    }

    /// RubyGems の yanked バージョン除外をテストする
    #[test]
    fn test_rubygems_yanked_filtering_logic() {
        let versions = vec![
            ("7.1.0", false), // not yanked
            ("7.0.8", false), // not yanked
            ("7.0.7", true),  // yanked - should be excluded
        ];

        let active_versions: Vec<_> = versions
            .into_iter()
            .filter(|(_, yanked)| !yanked)
            .map(|(v, _)| v)
            .collect();

        assert_eq!(active_versions.len(), 2);
        assert!(active_versions.contains(&"7.1.0"));
        assert!(active_versions.contains(&"7.0.8"));
        assert!(!active_versions.contains(&"7.0.7"));
    }

    /// Packagist の開発版除外ロジックをテストする
    #[test]
    fn test_packagist_dev_version_filtering_logic() {
        let versions = vec!["v10.0.0", "v9.0.0", "dev-master", "dev-main", "1.0.x-dev"];

        let stable_versions: Vec<_> = versions
            .into_iter()
            .filter(|v| {
                let lower = v.to_lowercase();
                !lower.contains("dev") && !lower.contains("-dev")
            })
            .collect();

        assert_eq!(stable_versions.len(), 2);
        assert!(stable_versions.contains(&"v10.0.0"));
        assert!(stable_versions.contains(&"v9.0.0"));
    }

    /// Packagist のバージョン正規化（v 接頭辞の除去）をテストする
    #[test]
    fn test_packagist_version_normalization() {
        let normalize =
            |version: &str| -> String { version.strip_prefix('v').unwrap_or(version).to_string() };

        assert_eq!(normalize("v10.0.0"), "10.0.0");
        assert_eq!(normalize("v1.2.3"), "1.2.3");
        assert_eq!(normalize("1.0.0"), "1.0.0"); // no v prefix
    }
}

mod monorepo_config {
    use super::*;
    use depup::config::DepupConfig;

    /// 有効なディレクトリを持つ .depup 設定の解析をテストする
    #[test]
    fn test_parse_config_with_valid_dirs() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("gui")).unwrap();
        fs::create_dir(dir.path().join("web")).unwrap();
        fs::create_dir(dir.path().join("cli")).unwrap();

        let content = "gui\nweb\ncli\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 3);
    }

    /// モノレポ内の複数ディレクトリでマニフェスト検出が動作することをテストする
    #[test]
    fn test_monorepo_manifest_detection() {
        let dir = create_test_dir();

        // マニフェストを持つサブディレクトリを作成する
        let gui_dir = dir.path().join("gui");
        let web_dir = dir.path().join("web");
        fs::create_dir(&gui_dir).unwrap();
        fs::create_dir(&web_dir).unwrap();

        // gui に package.json を配置する
        fs::write(
            gui_dir.join("package.json"),
            r#"{"dependencies": {"react": "^18.0.0"}}"#,
        )
        .unwrap();

        // web に package.json と pyproject.toml を配置する
        fs::write(
            web_dir.join("package.json"),
            r#"{"dependencies": {"express": "^4.18.0"}}"#,
        )
        .unwrap();
        fs::write(
            web_dir.join("pyproject.toml"),
            "[project]\ndependencies = [\"flask>=2.0.0\"]\n",
        )
        .unwrap();

        // ディレクトリごとにマニフェストを検出する
        let gui_manifests = depup::manifest::detect_manifests(&gui_dir);
        let web_manifests = depup::manifest::detect_manifests(&web_dir);

        assert_eq!(gui_manifests.len(), 1);
        assert_eq!(web_manifests.len(), 2);

        // 検出結果を結合する
        let total = gui_manifests.len() + web_manifests.len();
        assert_eq!(total, 3);
    }

    /// コメントと行末コメントを含む .depup をテストする
    #[test]
    fn test_config_with_comments() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("app")).unwrap();
        fs::create_dir(dir.path().join("lib")).unwrap();

        let content = "\
# Main application
app  # frontend

# Shared libraries
lib
# skipped: tests
";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 2);
    }

    /// .depup にある存在しないディレクトリが除外されることをテストする
    #[test]
    fn test_config_skips_nonexistent() {
        let dir = create_test_dir();
        fs::create_dir(dir.path().join("exists")).unwrap();

        let content = "exists\nmissing\n";
        let config = DepupConfig::parse(content, dir.path()).unwrap();
        assert_eq!(config.directories.len(), 1);
    }

    /// .depup ファイルがある場合とない場合の from_dir をテストする
    #[test]
    fn test_from_dir_presence() {
        let dir = create_test_dir();

        // .depup ファイルがない場合
        assert!(DepupConfig::from_dir(dir.path()).is_none());

        // 有効なディレクトリを持つ .depup を作成する
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join(".depup"), "sub\n").unwrap();

        let config = DepupConfig::from_dir(dir.path()).unwrap();
        assert_eq!(config.directories.len(), 1);
    }
}

mod pipeline_tests {
    use super::*;
    use chrono::Utc;
    use depup::domain::ManifestUpdateResult;
    use depup::manifest::{ManifestWriter, detect_manifests, get_parser};
    use depup::update::{UpdateFilter, UpdateJudge, VersionInfo};

    /// 一連の処理（検出→解析→判定→書き込み）をネットワークなしでテストする
    #[test]
    fn test_pipeline_updates_manifest_file() {
        let dir = create_test_dir();
        let path = dir.path().join("package.json");
        fs::write(
            &path,
            r#"{
  "dependencies": {
    "lodash": "^4.17.21"
  }
}"#,
        )
        .unwrap();

        // 手順1: マニフェストを検出する
        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);

        // 手順2: 依存関係を解析する
        let parser = get_parser(manifests[0].language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();
        assert_eq!(deps.len(), 1);

        // 手順3: ネットワークなしでバージョン情報を用意する
        let versions = vec![
            VersionInfo::new("4.17.21", Utc::now() - chrono::Duration::days(100)),
            VersionInfo::new("4.18.0", Utc::now() - chrono::Duration::days(10)),
        ];

        // 手順4: 更新を判定する
        let judge = UpdateJudge::new(UpdateFilter::new());
        let result = judge.judge(&deps[0], &versions);

        // 手順5: 更新を適用する
        let mut manifest_result = ManifestUpdateResult::new(&path, manifests[0].language);
        manifest_result.add_result(result);

        let writer = ManifestWriter::new(false);
        let write_result = writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();

        assert_eq!(write_result.updates_applied, 1);
        assert!(write_result.file_modified);

        // ファイル内容が変更されたことを確認する
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("^4.18.0"));
    }

    /// 回帰テスト: x-range 端点のハイフンレンジで上限が守られることを
    /// judge → writer の実経路で確認する。
    ///
    /// 上限抽出がワイルドカード端点を読めなかったときは、judge がレンジ外の
    /// 最新版を選び writer が下限だけを置換して `4.18.x - 2.3.x`
    /// (= `>=4.18.0 <2.4.0-0`、空レンジ) を書き込み、`npm install` が
    /// 必ず失敗するマニフェストを生成していた。
    #[test]
    fn test_pipeline_hyphen_range_with_wildcard_endpoints_respects_upper_bound() {
        let dir = create_test_dir();
        let path = dir.path().join("package.json");
        fs::write(
            &path,
            r#"{
  "dependencies": {
    "lodash": "1.2.x - 2.3.x"
  }
}"#,
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let parser = get_parser(manifests[0].language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();
        assert_eq!(deps.len(), 1);

        // 上限 `2.3.x` (= <2.4.0) を跨ぐ候補を含める
        let versions = vec![
            VersionInfo::new("1.2.0", Utc::now() - chrono::Duration::days(400)),
            VersionInfo::new("2.3.0", Utc::now() - chrono::Duration::days(300)),
            VersionInfo::new("2.4.0", Utc::now() - chrono::Duration::days(200)),
            VersionInfo::new("4.18.1", Utc::now() - chrono::Duration::days(100)),
        ];

        let judge = UpdateJudge::new(UpdateFilter::new());
        let result = judge.judge(&deps[0], &versions);

        let mut manifest_result = ManifestUpdateResult::new(&path, manifests[0].language);
        manifest_result.add_result(result);

        let writer = ManifestWriter::new(false);
        writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();

        let updated = fs::read_to_string(&path).unwrap();
        assert!(
            updated.contains("2.3.x - 2.3.x"),
            "上限内の最新版へ更新されるべき: {updated}"
        );
        assert!(
            !updated.contains("4.18"),
            "上限を超える候補を選んではならない: {updated}"
        );
    }

    /// 回帰テスト: Gemfile の複合制約が judge → writer の実経路で更新できる。
    ///
    /// 以前は judge が Update を返すのに writer が「複合バージョン制約は安全に
    /// 書き換えられません」で失敗し、サマリだけ「更新した」と表示していた。
    #[test]
    fn test_pipeline_updates_gemfile_compound_constraint() {
        let dir = create_test_dir();
        let path = dir.path().join("Gemfile");
        fs::write(
            &path,
            "source \"https://rubygems.org\"\ngem \"pg\", \">= 0.18\", \"< 2.0\"\n",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let parser = get_parser(manifests[0].language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(
            deps[0].version_spec.version, "0.18",
            "比較基準は包含下限であるべき"
        );

        let versions = vec![
            VersionInfo::new("0.18.0", Utc::now() - chrono::Duration::days(400)),
            VersionInfo::new("1.5.9", Utc::now() - chrono::Duration::days(100)),
            // 上限 `< 2.0` を超える候補は選ばれてはならない
            VersionInfo::new("2.1.0", Utc::now() - chrono::Duration::days(50)),
        ];

        let judge = UpdateJudge::new(UpdateFilter::new());
        let result = judge.judge(&deps[0], &versions);

        let mut manifest_result = ManifestUpdateResult::new(&path, manifests[0].language);
        manifest_result.add_result(result);

        let writer = ManifestWriter::new(false);
        let write_result = writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();

        assert_eq!(
            write_result.updates_applied, 1,
            "複合制約も書き込めるべき: {write_result:?}"
        );
        let updated = fs::read_to_string(&path).unwrap();
        assert!(
            updated.contains(r#"gem "pg", ">= 1.5.9", "< 2.0""#),
            "包含下限だけを進め、引数の形を保つべき: {updated}"
        );
    }

    /// mise の一連の処理 (検出→解析→判定→書き込み) をネットワークなしでテストする。
    /// ベンダー接頭辞の保持と前方一致指定のセグメント数保持が、実際の
    /// judge → writer 経路で成立することを確認する。
    #[test]
    fn test_pipeline_updates_mise_manifest() {
        let dir = create_test_dir();
        let path = dir.path().join("mise.toml");
        fs::write(
            &path,
            "[tools]\nnode = \"26.7.0\"\njava = \"temurin-21.0.5\"\npython = \"3.13\"\ngh = \"latest\"\n",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let mise = manifests
            .iter()
            .find(|m| m.language == depup::domain::Language::Mise)
            .expect("mise manifest detected");

        let parser = get_parser(mise.language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();
        // `gh = "latest"` は浮動指定なので依存として surface しない
        assert_eq!(deps.len(), 3);

        // `mise ls-remote <tool>` の出力を模したツール別バージョン一覧。
        // java だけは同じツールの中でベンダーが混ざる (mise の実挙動)。
        let versions_for = |tool: &str| -> Vec<VersionInfo> {
            match tool {
                "node" => vec![
                    VersionInfo::new("26.8.1", Utc::now() - chrono::Duration::days(30)),
                    VersionInfo::new("27.1.0", Utc::now() - chrono::Duration::days(20)),
                ],
                "java" => vec![
                    VersionInfo::new("temurin-21.0.9", Utc::now() - chrono::Duration::days(40)),
                    VersionInfo::new("zulu-27.0.0", Utc::now() - chrono::Duration::days(20)),
                    VersionInfo::new("27.0.0", Utc::now() - chrono::Duration::days(20)),
                ],
                "python" => vec![VersionInfo::new(
                    "3.14.7",
                    Utc::now() - chrono::Duration::days(25),
                )],
                other => panic!("unexpected tool {other}"),
            }
        };

        let judge = UpdateJudge::new(UpdateFilter::new());
        let mut manifest_result = ManifestUpdateResult::new(&path, mise.language);
        for dep in &deps {
            manifest_result.add_result(judge.judge(dep, &versions_for(&dep.name)));
        }

        let writer = ManifestWriter::new(false);
        let write_result = writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();
        assert_eq!(write_result.updates_applied, 3);

        let updated = fs::read_to_string(&path).unwrap();
        // node は完全一致指定なので完全版へ
        assert!(updated.contains("node = \"27.1.0\""), "{updated}");
        // java は temurin 系に留まり、別ベンダー (zulu) へ飛ばない
        assert!(updated.contains("java = \"temurin-21.0.9\""), "{updated}");
        // python は前方一致指定なのでセグメント数を保つ
        assert!(updated.contains("python = \"3.14\""), "{updated}");
        // 浮動指定は書き換えない
        assert!(updated.contains("gh = \"latest\""), "{updated}");
    }

    /// `.tool-versions` も同じ経路で更新できる
    #[test]
    fn test_pipeline_updates_tool_versions_file() {
        let dir = create_test_dir();
        let path = dir.path().join(".tool-versions");
        fs::write(&path, "node    26.7.0   # CI 用\nshellcheck latest\n").unwrap();

        let manifests = detect_manifests(dir.path());
        let mise = manifests
            .iter()
            .find(|m| m.language == depup::domain::Language::Mise)
            .expect("tool-versions manifest detected");

        let parser = get_parser(mise.language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();
        assert_eq!(deps.len(), 1);

        let versions = vec![VersionInfo::new(
            "26.8.1",
            Utc::now() - chrono::Duration::days(30),
        )];
        let judge = UpdateJudge::new(UpdateFilter::new());
        let mut manifest_result = ManifestUpdateResult::new(&path, mise.language);
        manifest_result.add_result(judge.judge(&deps[0], &versions));

        let writer = ManifestWriter::new(false);
        let write_result = writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();
        assert_eq!(write_result.updates_applied, 1);

        // 空白の並びと行末コメントを保つ
        let updated = fs::read_to_string(&path).unwrap();
        assert_eq!(updated, "node    26.8.1   # CI 用\nshellcheck latest\n");
    }

    /// 複数言語を含む処理をテストする
    #[test]
    fn test_pipeline_multi_language() {
        let dir = create_test_dir();

        // package.json を作成する
        fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies": {"lodash": "^4.17.21"}}"#,
        )
        .unwrap();

        // Cargo.toml を作成する
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0.0"
"#,
        )
        .unwrap();

        // すべてのマニフェストを検出する
        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 2);

        // 両方の言語が検出されたことを確認する
        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(languages.contains(&depup::domain::Language::Node));
        assert!(languages.contains(&depup::domain::Language::Rust));
    }

    /// 一連の処理でファイル書式が維持されることをテストする
    #[test]
    fn test_pipeline_preserves_formatting() {
        let dir = create_test_dir();
        let path = dir.path().join("package.json");

        // 特定の書式を持つ元の内容
        let original_content = r#"{
  "name": "test-package",
  "version": "1.0.0",
  "dependencies": {
    "zod": "^3.0.0",
    "axios": "^1.0.0",
    "lodash": "^4.17.21"
  }
}"#;
        fs::write(&path, original_content).unwrap();

        let manifests = detect_manifests(dir.path());
        let parser = get_parser(manifests[0].language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();

        // lodash の依存関係を探す
        let lodash = deps.iter().find(|d| d.name == "lodash").unwrap();

        let versions = vec![
            VersionInfo::new("4.17.21", Utc::now() - chrono::Duration::days(100)),
            VersionInfo::new("4.18.0", Utc::now() - chrono::Duration::days(10)),
        ];

        let judge = UpdateJudge::new(UpdateFilter::new());
        let result = judge.judge(lodash, &versions);

        let mut manifest_result = ManifestUpdateResult::new(&path, manifests[0].language);
        manifest_result.add_result(result);

        let writer = ManifestWriter::new(false);
        writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();

        let updated = fs::read_to_string(&path).unwrap();

        // キー順（zod、axios、lodash）が維持されることを確認する
        let zod_pos = updated.find("\"zod\"").unwrap();
        let axios_pos = updated.find("\"axios\"").unwrap();
        let lodash_pos = updated.find("\"lodash\"").unwrap();
        assert!(zod_pos < axios_pos, "zod should come before axios");
        assert!(axios_pos < lodash_pos, "axios should come before lodash");
    }

    /// ドライランでファイルが変更されないことをテストする
    #[test]
    fn test_pipeline_dry_run_no_modification() {
        let dir = create_test_dir();
        let path = dir.path().join("package.json");
        let original_content = r#"{"dependencies": {"lodash": "^4.17.21"}}"#;
        fs::write(&path, original_content).unwrap();

        let manifests = detect_manifests(dir.path());
        let parser = get_parser(manifests[0].language);
        let content = fs::read_to_string(&path).unwrap();
        let deps = parser.parse(&content).unwrap();

        let versions = vec![
            VersionInfo::new("4.17.21", Utc::now() - chrono::Duration::days(100)),
            VersionInfo::new("4.18.0", Utc::now() - chrono::Duration::days(10)),
        ];

        let judge = UpdateJudge::new(UpdateFilter::new());
        let result = judge.judge(&deps[0], &versions);

        let mut manifest_result = ManifestUpdateResult::new(&path, manifests[0].language);
        manifest_result.add_result(result);

        // ドライランモードを使う
        let writer = ManifestWriter::dry_run();
        let write_result = writer
            .apply_updates(&manifest_result, parser.as_ref())
            .unwrap();

        // 更新件数には含めるがファイルは変更しない
        assert_eq!(write_result.updates_applied, 1);
        assert!(!write_result.file_modified);

        // ファイルが変更されていないことを確認する
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, original_content);
    }
}

mod dependency_parser_edge_cases {
    use depup::domain::{Language, VersionSpecKind};
    use depup::manifest::get_parser;

    /// npm エイリアス依存 (npm:パッケージ名@バージョン) が正しくパースされることを確認
    #[test]
    fn test_node_npm_alias_parsing() {
        let content = r#"{
  "dependencies": {
    "custom-lodash": "npm:lodash@^4.17.21",
    "my-react": "npm:react@~18.2.0"
  }
}"#;

        let parser = get_parser(Language::Node);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 2, "npm エイリアス依存が2つパースされるべき");

        // レジストリ照会には実パッケージ名、書き戻しにはエイリアスキーを使う
        let lodash = deps.iter().find(|d| d.name == "lodash").unwrap();
        assert_eq!(lodash.manifest_name(), "custom-lodash");
        assert_eq!(
            lodash.version_spec.kind,
            VersionSpecKind::Caret,
            "キャレットプレフィックスが検出されるべき"
        );
        assert_eq!(lodash.version_spec.version, "4.17.21");

        let react = deps.iter().find(|d| d.name == "react").unwrap();
        assert_eq!(react.manifest_name(), "my-react");
        assert_eq!(
            react.version_spec.kind,
            VersionSpecKind::Tilde,
            "チルダプレフィックスが検出されるべき"
        );
        assert_eq!(react.version_spec.version, "18.2.0");
    }

    /// pyproject.toml の extras 付き依存 (例: httpx[http2]>=0.24.0) がパースされることを確認
    #[test]
    fn test_python_extras_in_dependency() {
        let content = r#"[project]
name = "my-project"
dependencies = [
    "httpx[http2]>=0.24.0",
    "boto3[crt]>=1.28.0",
]
"#;

        let parser = get_parser(Language::Python);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 2, "extras 付き依存が2つパースされるべき");

        // extras 部分はパッケージ名から除去される
        let httpx = deps.iter().find(|d| d.name == "httpx").unwrap();
        assert_eq!(
            httpx.version_spec.kind,
            VersionSpecKind::GreaterOrEqual,
            ">= 制約が検出されるべき"
        );
        assert_eq!(httpx.version_spec.version, "0.24.0");

        let boto3 = deps.iter().find(|d| d.name == "boto3").unwrap();
        assert_eq!(boto3.version_spec.version, "1.28.0");
    }

    /// Gemfile の group ブロック内依存が正しく開発依存として認識されることを確認
    #[test]
    fn test_ruby_group_block_parsing() {
        let content = r#"source 'https://rubygems.org'

gem 'rails', '~> 7.0'

group :development do
  gem 'web-console', '>= 4.1.0'
  gem 'debug', '~> 1.0'
end

group :test do
  gem 'rspec-rails', '~> 6.0'
end

gem 'pg', '~> 1.5'
"#;

        let parser = get_parser(Language::Ruby);
        let deps = parser.parse(content).unwrap();

        // group 外の gem は本番依存
        let rails = deps.iter().find(|d| d.name == "rails").unwrap();
        assert!(!rails.is_dev, "rails は本番依存であるべき");

        let pg = deps.iter().find(|d| d.name == "pg").unwrap();
        assert!(!pg.is_dev, "pg は group ブロック外なので本番依存であるべき");

        // group :development 内の gem は開発依存
        let web_console = deps.iter().find(|d| d.name == "web-console").unwrap();
        assert!(web_console.is_dev, "web-console は開発依存であるべき");

        let debug = deps.iter().find(|d| d.name == "debug").unwrap();
        assert!(debug.is_dev, "debug は開発依存であるべき");

        // group :test 内の gem も開発依存
        let rspec = deps.iter().find(|d| d.name == "rspec-rails").unwrap();
        assert!(
            rspec.is_dev,
            "rspec-rails はテスト依存（開発依存）であるべき"
        );
    }

    /// build.gradle の変数展開 ($variable) によるバージョン定義がパースされることを確認
    #[test]
    fn test_java_variable_expansion() {
        let content = r#"
def guavaVersion = '33.0.0-jre'
def junitVersion = '4.13.2'

dependencies {
    implementation "com.google.guava:guava:$guavaVersion"
    testImplementation "junit:junit:$junitVersion"
}
"#;

        let parser = get_parser(Language::Java);
        let deps = parser.parse(content).unwrap();

        // 変数が展開された状態でバージョンが取得される
        let guava = deps
            .iter()
            .find(|d| d.name == "com.google.guava:guava")
            .expect("guava が検出されるべき");
        assert_eq!(
            guava.version_spec.version, "33.0.0-jre",
            "変数 $guavaVersion が展開されるべき"
        );

        let junit = deps
            .iter()
            .find(|d| d.name == "junit:junit")
            .expect("junit が検出されるべき");
        assert_eq!(
            junit.version_spec.version, "4.13.2",
            "変数 $junitVersion が展開されるべき"
        );
        assert!(junit.is_dev, "testImplementation は開発依存であるべき");
    }

    /// composer.json の安定性フラグ付き依存 (@dev, @stable 等) がパースされることを確認
    #[test]
    fn test_composer_stability_flag() {
        let content = r#"{
  "require": {
    "vendor/package-a": "^1.0@dev",
    "vendor/package-b": "^2.0@stable",
    "vendor/package-c": "~3.0@beta"
  }
}"#;

        let parser = get_parser(Language::Php);
        let deps = parser.parse(content).unwrap();

        assert_eq!(deps.len(), 3, "安定性フラグ付き依存が3つパースされるべき");

        // @dev フラグは除去されてバージョン制約のみが解釈される
        let pkg_a = deps.iter().find(|d| d.name == "vendor/package-a").unwrap();
        assert_eq!(
            pkg_a.version_spec.kind,
            VersionSpecKind::Caret,
            "@dev フラグ除去後にキャレット制約が検出されるべき"
        );
        assert_eq!(pkg_a.version_spec.version, "1.0");

        // @stable フラグも同様に除去される
        let pkg_b = deps.iter().find(|d| d.name == "vendor/package-b").unwrap();
        assert_eq!(pkg_b.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(pkg_b.version_spec.version, "2.0");

        // @beta フラグも除去され、チルダ制約として解釈される
        let pkg_c = deps.iter().find(|d| d.name == "vendor/package-c").unwrap();
        assert_eq!(
            pkg_c.version_spec.kind,
            VersionSpecKind::Tilde,
            "@beta フラグ除去後にチルダ制約が検出されるべき"
        );
        assert_eq!(pkg_c.version_spec.version, "3.0");
    }
}

mod git_dependency_support {
    use super::*;
    use depup::domain::{GitReference, Language};
    use depup::manifest::{
        CargoTomlParser, ManifestParser, detect_manifests, parse_git_entries, read_git_entries,
    };

    /// Cargo.toml に宣言された 4 種類の git 依存 (branch/tag/rev/省略形) を
    /// すべてパースできることを確認する。
    #[test]
    fn test_parse_cargo_toml_mixed_git_dependencies() {
        let content = r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
regular = "1.0"
with-branch = { git = "https://github.com/owner/with-branch.git", branch = "main" }
with-tag = { git = "https://github.com/owner/with-tag.git", tag = "v1.2.3" }
with-rev = { git = "https://github.com/owner/with-rev.git", rev = "abc1234" }
default-ref = { git = "https://github.com/owner/default-ref.git" }
"#;
        let deps = CargoTomlParser.parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        let regular = deps.iter().find(|d| d.name == "regular").unwrap();
        assert!(regular.git_source.is_none());

        let branch = deps.iter().find(|d| d.name == "with-branch").unwrap();
        let b_git = branch.git_source.as_ref().unwrap();
        assert_eq!(b_git.reference, GitReference::Branch("main".to_string()));
        assert!(!branch.is_pinned());

        let tag = deps.iter().find(|d| d.name == "with-tag").unwrap();
        let t_git = tag.git_source.as_ref().unwrap();
        assert_eq!(t_git.reference, GitReference::Tag("v1.2.3".to_string()));
        assert!(!tag.is_pinned());

        let rev = deps.iter().find(|d| d.name == "with-rev").unwrap();
        let r_git = rev.git_source.as_ref().unwrap();
        assert_eq!(r_git.reference, GitReference::Rev("abc1234".to_string()));
        assert!(rev.is_pinned());

        let default = deps.iter().find(|d| d.name == "default-ref").unwrap();
        let d_git = default.git_source.as_ref().unwrap();
        assert_eq!(d_git.reference, GitReference::DefaultBranch);
        assert!(!default.is_pinned());
    }

    /// Cargo.lock から git 依存の現在コミットハッシュを抽出できることを確認する。
    #[test]
    fn test_cargo_lock_extracts_git_commits() {
        let temp_dir = create_test_dir();
        let lock_content = r#"version = 3

[[package]]
name = "registry-dep"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "tree-sitter-xojo"
version = "0.1.0"
source = "git+https://github.com/owayo/tree-sitter-xojo.git?branch=main#045c52a6db5390da14d96c0e4804a6208552dc8f"
"#;
        let lock_path = temp_dir.path().join("Cargo.lock");
        fs::write(&lock_path, lock_content).unwrap();

        let entries = read_git_entries(&lock_path);
        assert_eq!(entries.len(), 1);
        let xojo = &entries.get("tree-sitter-xojo").unwrap()[0];
        assert_eq!(xojo.url, "https://github.com/owayo/tree-sitter-xojo.git");
        assert_eq!(xojo.commit, "045c52a6db5390da14d96c0e4804a6208552dc8f");
    }

    /// Cargo.toml の tag 指定を新バージョンに書き換えられることを確認する。
    #[test]
    fn test_cargo_toml_tag_update_round_trip() {
        let original = r#"[dependencies]
bar = { git = "https://github.com/owner/bar.git", tag = "v1.2.3", features = ["async"] }
serde = "1.0"
"#;
        let updated = CargoTomlParser
            .update_git_tag(original, "bar", "v2.0.0")
            .unwrap();
        assert!(updated.contains(r#"tag = "v2.0.0""#));
        // 他のフィールドが保持される
        assert!(updated.contains(r#"features = ["async"]"#));
        assert!(updated.contains(r#"serde = "1.0""#));
    }

    /// パースした Cargo.lock 文字列から直接 git 依存情報を抽出できる (ファイル I/O なし)
    #[test]
    fn test_parse_git_entries_from_string() {
        let content = r#"[[package]]
name = "foo"
version = "0.1.0"
source = "git+https://example.com/foo.git?tag=v1.0.0#1234567890abcdef1234567890abcdef12345678"
"#;
        let entries = parse_git_entries(content);
        assert_eq!(entries.len(), 1);
        let foo = &entries.get("foo").unwrap()[0];
        assert_eq!(foo.url, "https://example.com/foo.git");
    }

    /// 通常マニフェスト検出パイプラインでも、git 依存を含む Cargo.toml が
    /// 問題なく検出・パースされることを確認する。
    #[test]
    fn test_detect_and_parse_git_dependencies() {
        let temp_dir = create_test_dir();
        let cargo_toml = r#"[package]
name = "example"
version = "0.1.0"

[dependencies]
tree-sitter-xojo = { git = "https://github.com/owayo/tree-sitter-xojo.git", branch = "main" }
"#;
        fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

        let manifests = detect_manifests(temp_dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Rust);

        let content = fs::read_to_string(&manifests[0].path).unwrap();
        let deps = CargoTomlParser.parse(&content).unwrap();
        assert_eq!(deps.len(), 1);
        let git = deps[0].git_source.as_ref().unwrap();
        assert_eq!(git.reference, GitReference::Branch("main".to_string()));
    }
}
