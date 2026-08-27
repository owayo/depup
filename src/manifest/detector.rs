//! モノレポおよび Tauri 対応のマニフェストファイル検出
//!
//! 機能:
//! - package.json, pyproject.toml, Cargo.toml, go.mod の検出
//! - pnpm-workspace.yaml によるモノレポ検出のサポート
//! - Tauri プロジェクト (src-tauri/Cargo.toml) のサポート

use crate::domain::Language;
use std::path::{Path, PathBuf};

/// 検出されたマニフェストファイルの情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInfo {
    /// マニフェストファイルのパス
    pub path: PathBuf,
    /// マニフェストの言語/エコシステム
    pub language: Language,
    /// ワークスペースルートのマニフェストかどうか
    pub is_workspace_root: bool,
    /// Tauri プロジェクトの src-tauri ディレクトリのマニフェストかどうか
    pub is_tauri_rust: bool,
}

impl ManifestInfo {
    /// 新しい ManifestInfo を作成する
    pub fn new(path: impl Into<PathBuf>, language: Language) -> Self {
        Self {
            path: path.into(),
            language,
            is_workspace_root: false,
            is_tauri_rust: false,
        }
    }

    /// ワークスペースルートとしてマークする
    pub fn with_workspace_root(mut self, is_root: bool) -> Self {
        self.is_workspace_root = is_root;
        self
    }

    /// Tauri Rust プロジェクトとしてマークする
    pub fn with_tauri_rust(mut self, is_tauri: bool) -> Self {
        self.is_tauri_rust = is_tauri;
        self
    }
}

/// 検出されたマニフェストファイルを表す構造体
#[derive(Debug, Clone)]
pub struct ManifestFile {
    /// マニフェストファイルのパス
    pub path: PathBuf,
    /// マニフェストファイルの内容
    pub content: String,
    /// マニフェストの情報
    pub info: ManifestInfo,
}

/// 指定ディレクトリ内の全マニフェストファイルを検出する
///
/// この関数の処理:
/// 1. 標準的なマニフェストファイル (package.json, pyproject.toml, Cargo.toml, go.mod, build.gradle) を探す
/// 2. pnpm-workspace.yaml の存在をチェックしてモノレポを検出する
/// 3. src-tauri/Cargo.toml の存在をチェックして Tauri プロジェクトを検出する
/// 4. build.gradle.kts (Kotlin DSL) の存在をチェックして Gradle プロジェクトを検出する
/// 5. gradle/*.versions.toml の Gradle version catalog を検出する
pub fn detect_manifests(dir: &Path) -> Vec<ManifestInfo> {
    let mut manifests = Vec::new();

    // pnpm ワークスペースかどうかを確認する
    let is_pnpm_workspace = dir.join("pnpm-workspace.yaml").exists();

    // 各マニフェストタイプを検出する
    for language in Language::all() {
        let manifest_name = language.manifest_filename();
        let manifest_path = dir.join(manifest_name);

        if manifest_path.exists() {
            let mut info = ManifestInfo::new(&manifest_path, *language);

            // pnpm-workspace.yaml が存在し package.json であればワークスペースルートとしてマークする
            if *language == Language::Node && is_pnpm_workspace {
                info = info.with_workspace_root(true);
            }

            manifests.push(info);
        }

        // Java 向け Kotlin DSL バリアント (build.gradle.kts) を確認する
        if *language == Language::Java {
            let kts_path = dir.join("build.gradle.kts");
            if kts_path.exists() && !manifest_path.exists() {
                // build.gradle が存在しない場合のみ .kts を追加する (Groovy を Kotlin DSL より優先)
                manifests.push(ManifestInfo::new(&kts_path, Language::Java));
            }
        }
    }

    // Cargo ワークスペースメンバーを確認する
    let cargo_toml_path = dir.join("Cargo.toml");
    if cargo_toml_path.exists()
        && let Ok(content) = std::fs::read_to_string(&cargo_toml_path)
    {
        for member_dir in detect_cargo_workspace_members(dir, &content) {
            let member_cargo = member_dir.join("Cargo.toml");
            if member_cargo.exists() && !manifests.iter().any(|m| m.path == member_cargo) {
                manifests.push(ManifestInfo::new(&member_cargo, Language::Rust));
            }
        }
    }

    // Tauri プロジェクト (src-tauri/Cargo.toml) を確認する
    let tauri_cargo_path = dir.join("src-tauri").join("Cargo.toml");
    if tauri_cargo_path.exists() {
        // ワークスペースメンバー検出で既に追加済みなら、Tauri フラグだけ設定する
        if let Some(existing) = manifests
            .iter_mut()
            .find(|m| m.language == Language::Rust && m.path == tauri_cargo_path)
        {
            existing.is_tauri_rust = true;
        } else {
            let tauri_info =
                ManifestInfo::new(&tauri_cargo_path, Language::Rust).with_tauri_rust(true);
            manifests.push(tauri_info);
        }
    }

    // pnpm-workspace.yaml が存在する場合、pnpm ワークスペースパッケージを確認する
    if is_pnpm_workspace && let Ok(workspace_packages) = detect_pnpm_workspace_packages(dir) {
        for package_path in workspace_packages {
            let package_json_path = package_path.join("package.json");
            // ルートの package.json と既出のマニフェストは重複追加しない
            if package_json_path.exists()
                && package_json_path != dir.join("package.json")
                && !manifests.iter().any(|m| m.path == package_json_path)
            {
                manifests.push(ManifestInfo::new(&package_json_path, Language::Node));
            }
        }
    }

    // Gradle version catalog は Java/Gradle 依存の別マニフェストとして扱う。
    for catalog_path in detect_gradle_version_catalogs(dir) {
        if !manifests.iter().any(|m| m.path == catalog_path) {
            manifests.push(ManifestInfo::new(catalog_path, Language::Java));
        }
    }

    // mise の設定ファイルを検出する。`mise.toml` は Language::all() のループで
    // 拾えるが、mise は同じディレクトリの複数ファイルを読むので残りも足す。
    for mise_path in detect_mise_manifests(dir) {
        if !manifests.iter().any(|m| m.path == mise_path) {
            manifests.push(ManifestInfo::new(mise_path, Language::Mise));
        }
    }

    manifests
}

/// ディレクトリ直下の mise 設定ファイルを検出する。
///
/// mise が読むファイルのうち、depup が更新対象にするのは次の 2 系統:
/// - TOML 形式 (`MISE_CONFIG_FILENAMES`)
/// - asdf 互換の `.tool-versions`
///
/// `mise.local.toml` / `.mise.local.toml` は個人のローカル上書き
/// (通常 gitignore 対象) なので更新しない。`mise.<env>.toml` も特定環境だけの
/// overlay なので、意図しない環境の書き換えを避けて対象外にする。
fn detect_mise_manifests(dir: &Path) -> Vec<PathBuf> {
    let mut paths = super::mise_settings::mise_config_paths(dir);
    let tool_versions = dir.join(super::tool_versions::TOOL_VERSIONS_FILENAME);
    if tool_versions.is_file() {
        paths.push(tool_versions);
    }
    paths
}

/// gradle ディレクトリ直下の version catalog を検出する
fn detect_gradle_version_catalogs(dir: &Path) -> Vec<PathBuf> {
    let gradle_dir = dir.join("gradle");
    let Ok(entries) = std::fs::read_dir(&gradle_dir) else {
        return Vec::new();
    };

    let mut catalogs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if file_name.ends_with(".versions.toml") {
            catalogs.push(path);
        }
    }

    catalogs.sort();
    catalogs
}

/// pnpm-workspace.yaml をパースしてパッケージディレクトリを返す
fn detect_pnpm_workspace_packages(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let workspace_file = dir.join("pnpm-workspace.yaml");
    let content = std::fs::read_to_string(&workspace_file)?;

    let mut includes = Vec::new();
    let mut excludes: Vec<PathBuf> = Vec::new();

    // packages 配列の簡易 YAML パース
    // 形式: packages:
    //          - 'packages/*' というワークスペース設定例
    //          - 'apps/*' というワークスペース設定例
    let mut in_packages = false;
    // flow-style 配列 (`packages: ['a/*', 'b/*']`) を複数行に跨って蓄積するバッファ
    let mut flow_buffer: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();

        // flow-style 配列を複数行で読んでいる途中なら `]` が現れるまで蓄積する
        if let Some(buffer) = flow_buffer.as_mut() {
            buffer.push(' ');
            buffer.push_str(trimmed);
            if let Some(end) = buffer.find(']') {
                let inner = buffer[..end].to_string();
                flow_buffer = None;
                add_flow_workspace_patterns(&inner, dir, &mut includes, &mut excludes);
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("packages:") {
            let rest = rest.trim();
            if let Some(after_bracket) = rest.strip_prefix('[') {
                // フロー形式: `packages: ['packages/*', 'apps/*']`
                if let Some(end) = after_bracket.find(']') {
                    add_flow_workspace_patterns(
                        &after_bracket[..end],
                        dir,
                        &mut includes,
                        &mut excludes,
                    );
                } else {
                    // `]` が次行以降にある複数行 flow-style
                    flow_buffer = Some(after_bracket.to_string());
                }
            } else {
                // block-style: 次行以降の `- 'pattern'` を読む
                in_packages = true;
            }
            continue;
        }

        if in_packages {
            // 新しいセクションに移ったかどうかを確認する
            if !trimmed.is_empty() && !trimmed.starts_with('-') && !trimmed.starts_with('#') {
                break;
            }

            // パッケージの glob パターンをパースする
            if let Some(item) = trimmed.strip_prefix('-')
                && let Some(pattern) = parse_yaml_list_value(item)
            {
                add_workspace_pattern(&pattern, dir, &mut includes, &mut excludes);
            }
        }
    }

    let mut packages = Vec::new();
    for path in includes {
        if !excludes.contains(&path) && !packages.contains(&path) {
            packages.push(path);
        }
    }

    Ok(packages)
}

/// YAML リスト要素からインラインコメントとクォートを取り除いた値を返す
fn parse_yaml_list_value(item: &str) -> Option<String> {
    let item = item.trim();
    let mut chars = item.chars();
    let first = chars.next()?;
    if first == '\'' || first == '"' {
        // クォート付き: 閉じクォートまでが値 (以降はインラインコメント扱い)
        let rest = chars.as_str();
        let end = rest.find(first)?;
        Some(rest[..end].to_string())
    } else {
        // クォートなし: 空白 + '#' 以降はインラインコメント
        let value = match item.find(" #") {
            Some(pos) => &item[..pos],
            None => item,
        };
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}

/// glob パターンを include / exclude へ振り分ける (`!pattern` は除外指定)。
fn add_workspace_pattern(
    pattern: &str,
    dir: &Path,
    includes: &mut Vec<PathBuf>,
    excludes: &mut Vec<PathBuf>,
) {
    // `!pattern` は除外指定 (pnpm 公式サポート)
    if let Some(negated) = pattern.strip_prefix('!') {
        excludes.extend(expand_workspace_pattern(dir, negated));
    } else {
        includes.extend(expand_workspace_pattern(dir, pattern));
    }
}

/// flow-style 配列の中身 (`'a/*', 'b/*'`) をカンマ区切りで分解して振り分ける。
fn add_flow_workspace_patterns(
    inner: &str,
    dir: &Path,
    includes: &mut Vec<PathBuf>,
    excludes: &mut Vec<PathBuf>,
) {
    for raw_item in inner.split(',') {
        let item = raw_item.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(pattern) = parse_yaml_list_value(item) {
            add_workspace_pattern(&pattern, dir, includes, excludes);
        }
    }
}

/// ワークスペースの glob パターンをディレクトリ一覧へ展開する。
///
/// 対応形式: 直接パス / 末尾 `/*` / 末尾 `/**` (第 1 階層のみ) /
/// 末尾セグメントの単純ワイルドカード (`crates/util-*`)。
fn expand_workspace_pattern(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    if let Some(base) = pattern.strip_suffix("/*") {
        list_subdirectories(&dir.join(base))
    } else if let Some(base) = pattern.strip_suffix("/**") {
        // ** パターンは現時点では第 1 階層のみを対象にする
        list_subdirectories(&dir.join(base))
    } else if !pattern.contains('*') {
        // glob なしの直接パス
        let pkg_path = dir.join(pattern);
        if pkg_path.exists() {
            vec![pkg_path]
        } else {
            Vec::new()
        }
    } else {
        // 末尾セグメントの単純ワイルドカード (`crates/util-*`) を展開する
        let path = Path::new(pattern);
        if let (Some(parent), Some(file)) =
            (path.parent(), path.file_name().and_then(|f| f.to_str()))
            && !parent.to_string_lossy().contains('*')
            && let Some((prefix, suffix)) = file.split_once('*')
            && !suffix.contains('*')
        {
            return list_subdirectories(&dir.join(parent))
                .into_iter()
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
                })
                .collect();
        }
        Vec::new()
    }
}

/// ベースディレクトリ直下のサブディレクトリを列挙する (順序は名前順で安定)
fn list_subdirectories(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Cargo.toml のワークスペースメンバーをパースしてディレクトリを返す。
///
/// Cargo 公式サポートの glob 形式メンバー (`members = ["crates/*"]`) を展開し、
/// `[workspace] exclude` に挙げられたパスを除外する。
fn detect_cargo_workspace_members(dir: &Path, content: &str) -> Vec<PathBuf> {
    let toml: toml::Value = match toml::from_str(content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let workspace = match toml.get("workspace") {
        Some(w) => w,
        None => return Vec::new(),
    };

    let members = match workspace.get("members").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return Vec::new(),
    };

    let excludes: Vec<PathBuf> = workspace
        .get("exclude")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|p| dir.join(p))
                .collect()
        })
        .unwrap_or_default();

    let mut result = Vec::new();
    for member in members.iter().filter_map(|v| v.as_str()) {
        for path in expand_workspace_pattern(dir, member) {
            if !excludes.contains(&path) && !result.contains(&path) {
                result.push(path);
            }
        }
    }
    result
}

/// ディレクトリが Tauri プロジェクトかどうかを判定する
#[allow(dead_code)]
pub fn is_tauri_project(dir: &Path) -> bool {
    dir.join("src-tauri").exists() && dir.join("src-tauri").join("Cargo.toml").exists()
}

/// ディレクトリが pnpm ワークスペースかどうかを判定する
#[allow(dead_code)]
pub fn is_pnpm_workspace(dir: &Path) -> bool {
    dir.join("pnpm-workspace.yaml").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_detect_package_json() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Node);
        assert!(!manifests[0].is_workspace_root);
    }

    #[test]
    fn test_detect_multiple_manifests() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        fs::write(dir.path().join("go.mod"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 4);

        let languages: Vec<_> = manifests.iter().map(|m| m.language).collect();
        assert!(languages.contains(&Language::Node));
        assert!(languages.contains(&Language::Rust));
        assert!(languages.contains(&Language::Python));
        assert!(languages.contains(&Language::Go));
    }

    #[test]
    fn test_detect_mise_toml() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("mise.toml"), "[tools]\nnode = \"26.7.0\"\n").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Mise);
        assert_eq!(manifests[0].path, dir.path().join("mise.toml"));
    }

    #[test]
    fn test_detect_mise_alternate_locations() {
        let dir = create_temp_dir();
        fs::create_dir_all(dir.path().join(".config/mise")).unwrap();
        fs::write(dir.path().join(".mise.toml"), "[tools]\n").unwrap();
        fs::write(dir.path().join(".config/mise/config.toml"), "[tools]\n").unwrap();
        fs::write(dir.path().join(".tool-versions"), "node 26.7.0\n").unwrap();

        let paths: Vec<_> = detect_manifests(dir.path())
            .into_iter()
            .filter(|m| m.language == Language::Mise)
            .map(|m| m.path)
            .collect();
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&dir.path().join(".mise.toml")));
        assert!(paths.contains(&dir.path().join(".config/mise/config.toml")));
        assert!(paths.contains(&dir.path().join(".tool-versions")));
    }

    /// `mise.local.toml` は個人のローカル上書き (通常 gitignore) なので更新しない。
    /// `mise.<env>.toml` も特定環境だけの overlay なので対象外。
    #[test]
    fn test_detect_mise_skips_local_and_env_overlays() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("mise.local.toml"), "[tools]\n").unwrap();
        fs::write(dir.path().join(".mise.local.toml"), "[tools]\n").unwrap();
        fs::write(dir.path().join("mise.production.toml"), "[tools]\n").unwrap();

        let manifests = detect_manifests(dir.path());
        assert!(manifests.iter().all(|m| m.language != Language::Mise));
    }

    /// mise.toml は Language::all() のループと mise 専用検出の両方に該当するので、
    /// 二重に登録されていないこと
    #[test]
    fn test_detect_mise_toml_is_not_duplicated() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("mise.toml"), "[tools]\n").unwrap();

        let mise_manifests: Vec<_> = detect_manifests(dir.path())
            .into_iter()
            .filter(|m| m.language == Language::Mise)
            .collect();
        assert_eq!(mise_manifests.len(), 1);
    }

    #[test]
    fn test_detect_pnpm_workspace() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let root = manifests
            .iter()
            .find(|m| m.path == dir.path().join("package.json"))
            .unwrap();
        assert!(root.is_workspace_root);
    }

    #[test]
    fn test_detect_pnpm_workspace_flow_style_packages() {
        // flow-style 配列 (`packages: ['packages/*', 'apps/*']`) でもメンバーを検出する
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: ['packages/*', 'apps/*']\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("packages/a")).unwrap();
        fs::write(dir.path().join("packages/a/package.json"), "{}").unwrap();
        fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        fs::write(dir.path().join("apps/web/package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        assert!(
            manifests
                .iter()
                .any(|m| m.path.ends_with("packages/a/package.json")),
            "flow-style の packages/* メンバーが検出されるべき"
        );
        assert!(
            manifests
                .iter()
                .any(|m| m.path.ends_with("apps/web/package.json")),
            "flow-style の apps/* メンバーが検出されるべき"
        );
    }

    #[test]
    fn test_detect_pnpm_workspace_flow_style_multiline_with_exclude() {
        // 複数行 flow-style と否定パターン (`!`) の組み合わせ
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages: [\n  'packages/*',\n  '!packages/legacy',\n]\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("packages/a")).unwrap();
        fs::write(dir.path().join("packages/a/package.json"), "{}").unwrap();
        fs::create_dir_all(dir.path().join("packages/legacy")).unwrap();
        fs::write(dir.path().join("packages/legacy/package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        assert!(
            manifests
                .iter()
                .any(|m| m.path.ends_with("packages/a/package.json"))
        );
        assert!(
            !manifests
                .iter()
                .any(|m| m.path.ends_with("packages/legacy/package.json")),
            "否定パターンで除外されたメンバーは検出されないべき"
        );
    }

    #[test]
    fn test_detect_tauri_project() {
        let dir = create_temp_dir();
        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        fs::write(dir.path().join("src-tauri").join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 2);

        let tauri_manifest = manifests.iter().find(|m| m.is_tauri_rust).unwrap();
        assert_eq!(tauri_manifest.language, Language::Rust);
        assert!(tauri_manifest.path.ends_with("src-tauri/Cargo.toml"));
    }

    #[test]
    fn test_detect_tauri_with_root_cargo() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        fs::write(dir.path().join("src-tauri").join("Cargo.toml"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        // ルート Cargo.toml と src-tauri/Cargo.toml の両方があるはず
        let rust_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Rust)
            .collect();
        assert_eq!(rust_manifests.len(), 2);

        // 一方は Tauri、もう一方は非 Tauri であるべき
        assert!(rust_manifests.iter().any(|m| m.is_tauri_rust));
        assert!(rust_manifests.iter().any(|m| !m.is_tauri_rust));
    }

    #[test]
    fn test_detect_empty_directory() {
        let dir = create_temp_dir();
        let manifests = detect_manifests(dir.path());
        assert!(manifests.is_empty());
    }

    #[test]
    fn test_is_tauri_project() {
        let dir = create_temp_dir();
        assert!(!is_tauri_project(dir.path()));

        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        assert!(!is_tauri_project(dir.path()));

        fs::write(dir.path().join("src-tauri").join("Cargo.toml"), "").unwrap();
        assert!(is_tauri_project(dir.path()));
    }

    #[test]
    fn test_is_pnpm_workspace() {
        let dir = create_temp_dir();
        assert!(!is_pnpm_workspace(dir.path()));

        fs::write(dir.path().join("pnpm-workspace.yaml"), "").unwrap();
        assert!(is_pnpm_workspace(dir.path()));
    }

    #[test]
    fn test_manifest_info_builder() {
        let info = ManifestInfo::new("/test/package.json", Language::Node)
            .with_workspace_root(true)
            .with_tauri_rust(false);

        assert_eq!(info.path, PathBuf::from("/test/package.json"));
        assert_eq!(info.language, Language::Node);
        assert!(info.is_workspace_root);
        assert!(!info.is_tauri_rust);
    }

    #[test]
    fn test_pnpm_workspace_packages_detection() {
        let dir = create_temp_dir();

        // pnpm-workspace.yaml を作成する
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n  - 'apps/*'\n",
        )
        .unwrap();

        // ルート package.json を作成する
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        // サブパッケージを含む packages ディレクトリを作成する
        fs::create_dir(dir.path().join("packages")).unwrap();
        fs::create_dir(dir.path().join("packages").join("pkg-a")).unwrap();
        fs::write(
            dir.path()
                .join("packages")
                .join("pkg-a")
                .join("package.json"),
            "{}",
        )
        .unwrap();
        fs::create_dir(dir.path().join("packages").join("pkg-b")).unwrap();
        fs::write(
            dir.path()
                .join("packages")
                .join("pkg-b")
                .join("package.json"),
            "{}",
        )
        .unwrap();

        // apps ディレクトリを作成する
        fs::create_dir(dir.path().join("apps")).unwrap();
        fs::create_dir(dir.path().join("apps").join("web")).unwrap();
        fs::write(
            dir.path().join("apps").join("web").join("package.json"),
            "{}",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());

        // ルート package.json + pkg-a + pkg-b + apps/web の 4 件が見つかるはず
        let node_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Node)
            .collect();
        assert_eq!(node_manifests.len(), 4);

        // ルートがワークスペースルートとしてマークされているはず
        let root = node_manifests
            .iter()
            .find(|m| m.path == dir.path().join("package.json"))
            .unwrap();
        assert!(root.is_workspace_root);
    }

    #[test]
    fn test_pnpm_workspace_inline_comment_and_negation() {
        let dir = create_temp_dir();

        // インラインコメント付きパターンと否定パターンを含む pnpm-workspace.yaml
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*' # apps\n  - '!packages/legacy'\n",
        )
        .unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        for name in ["pkg-a", "legacy"] {
            let pkg_dir = dir.path().join("packages").join(name);
            fs::create_dir_all(&pkg_dir).unwrap();
            fs::write(pkg_dir.join("package.json"), "{}").unwrap();
        }

        let manifests = detect_manifests(dir.path());
        let node_paths: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Node)
            .map(|m| m.path.clone())
            .collect();

        // インラインコメントがあっても 'packages/*' が展開される
        assert!(
            node_paths.contains(&dir.path().join("packages/pkg-a/package.json")),
            "コメント付きパターンが展開されるべき: {:?}",
            node_paths
        );
        // 否定パターンのパッケージは除外される
        assert!(
            !node_paths.contains(&dir.path().join("packages/legacy/package.json")),
            "否定パターンは除外されるべき: {:?}",
            node_paths
        );
    }

    #[test]
    fn test_pnpm_workspace_duplicate_patterns_not_duplicated() {
        let dir = create_temp_dir();

        // glob と直接パスが同じパッケージを指す場合に重複しない
        fs::write(
            dir.path().join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n  - 'packages/pkg-a'\n",
        )
        .unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let pkg_dir = dir.path().join("packages").join("pkg-a");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("package.json"), "{}").unwrap();

        let manifests = detect_manifests(dir.path());
        let count = manifests
            .iter()
            .filter(|m| m.path == pkg_dir.join("package.json"))
            .count();
        assert_eq!(count, 1, "同一パッケージは1回だけ検出されるべき");
    }

    #[test]
    fn test_detect_cargo_workspace_members_glob() {
        let dir = create_temp_dir();

        // glob 形式 members + exclude を持つワークスペース
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["crates/*"]
exclude = ["crates/legacy"]
resolver = "2"
"#,
        )
        .unwrap();

        for name in ["core", "cli", "legacy"] {
            let member_dir = dir.path().join("crates").join(name);
            fs::create_dir_all(&member_dir).unwrap();
            fs::write(
                member_dir.join("Cargo.toml"),
                "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
        }

        let manifests = detect_manifests(dir.path());
        let rust_paths: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Rust)
            .map(|m| m.path.clone())
            .collect();

        assert!(
            rust_paths.contains(&dir.path().join("crates/core/Cargo.toml")),
            "glob members が展開されるべき: {:?}",
            rust_paths
        );
        assert!(rust_paths.contains(&dir.path().join("crates/cli/Cargo.toml")));
        assert!(
            !rust_paths.contains(&dir.path().join("crates/legacy/Cargo.toml")),
            "exclude されたメンバーは検出されないべき"
        );
    }

    #[test]
    fn test_detect_cargo_workspace_members_segment_wildcard() {
        let dir = create_temp_dir();

        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/util-*\"]\n",
        )
        .unwrap();

        for name in ["util-a", "other"] {
            let member_dir = dir.path().join("crates").join(name);
            fs::create_dir_all(&member_dir).unwrap();
            fs::write(
                member_dir.join("Cargo.toml"),
                "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();
        }

        let members = detect_cargo_workspace_members(
            dir.path(),
            &fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(),
        );
        assert_eq!(members, vec![dir.path().join("crates/util-a")]);
    }

    #[test]
    fn test_detect_cargo_workspace_members() {
        let dir = create_temp_dir();

        // ワークスペースルートの Cargo.toml を作成する
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["crates/core", "crates/cli"]
resolver = "2"

[workspace.dependencies]
serde = "1"
"#,
        )
        .unwrap();

        // メンバークレートのディレクトリと Cargo.toml を作成する
        fs::create_dir_all(dir.path().join("crates").join("core")).unwrap();
        fs::write(
            dir.path().join("crates").join("core").join("Cargo.toml"),
            r#"[package]
name = "core"
version = "0.1.0"

[dependencies]
tokio = "1"
"#,
        )
        .unwrap();

        fs::create_dir_all(dir.path().join("crates").join("cli")).unwrap();
        fs::write(
            dir.path().join("crates").join("cli").join("Cargo.toml"),
            r#"[package]
name = "cli"
version = "0.1.0"

[dependencies]
clap = "4"
"#,
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let rust_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Rust)
            .collect();

        // ルート Cargo.toml + crates/core + crates/cli の 3 件が見つかるはず
        assert_eq!(rust_manifests.len(), 3);
        assert!(
            rust_manifests
                .iter()
                .any(|m| m.path == dir.path().join("Cargo.toml"))
        );
        assert!(
            rust_manifests
                .iter()
                .any(|m| m.path.ends_with("crates/core/Cargo.toml"))
        );
        assert!(
            rust_manifests
                .iter()
                .any(|m| m.path.ends_with("crates/cli/Cargo.toml"))
        );
    }

    #[test]
    fn test_detect_cargo_workspace_no_duplicate_with_tauri() {
        let dir = create_temp_dir();

        // src-tauri をメンバーとするワークスペースルートを作成する
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["src-tauri"]
"#,
        )
        .unwrap();

        fs::create_dir(dir.path().join("src-tauri")).unwrap();
        fs::write(
            dir.path().join("src-tauri").join("Cargo.toml"),
            r#"[package]
name = "app"
version = "0.1.0"
"#,
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let rust_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Rust)
            .collect();

        // src-tauri は 1 回だけ出現するはず (Tauri として追加、ワークスペースメンバーとの重複なし)
        let tauri_paths: Vec<_> = rust_manifests
            .iter()
            .filter(|m| m.path.ends_with("src-tauri/Cargo.toml"))
            .collect();
        assert_eq!(tauri_paths.len(), 1);
        assert!(tauri_paths[0].is_tauri_rust);
    }

    #[test]
    fn test_detect_cargo_workspace_member_missing_dir() {
        let dir = create_temp_dir();

        // 存在しないメンバーを指定したワークスペースルートを作成する
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"[workspace]
members = ["crates/nonexistent"]
"#,
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        let rust_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Rust)
            .collect();

        // ルート Cargo.toml のみ、存在しないメンバーは無視される
        assert_eq!(rust_manifests.len(), 1);
    }

    #[test]
    fn test_detect_build_gradle() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("build.gradle"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Java);
        assert!(manifests[0].path.ends_with("build.gradle"));
    }

    #[test]
    fn test_detect_build_gradle_kts() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("build.gradle.kts"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Java);
        assert!(manifests[0].path.ends_with("build.gradle.kts"));
    }

    #[test]
    fn test_detect_build_gradle_prefers_groovy_over_kts() {
        let dir = create_temp_dir();
        // Groovy と Kotlin DSL が両方存在する場合、Groovy を優先する
        fs::write(dir.path().join("build.gradle"), "").unwrap();
        fs::write(dir.path().join("build.gradle.kts"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        let java_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Java)
            .collect();
        // 1 件だけ検出されるはず (Groovy)
        assert_eq!(java_manifests.len(), 1);
        assert!(java_manifests[0].path.ends_with("build.gradle"));
    }

    #[test]
    fn test_detect_gradle_version_catalog() {
        let dir = create_temp_dir();
        fs::create_dir(dir.path().join("gradle")).unwrap();
        fs::write(
            dir.path().join("gradle").join("libs.versions.toml"),
            "[libraries]\njunit = \"junit:junit:4.13.2\"\n",
        )
        .unwrap();

        let manifests = detect_manifests(dir.path());
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].language, Language::Java);
        assert!(manifests[0].path.ends_with("gradle/libs.versions.toml"));
    }

    #[test]
    fn test_detect_multiple_gradle_version_catalogs() {
        let dir = create_temp_dir();
        fs::create_dir(dir.path().join("gradle")).unwrap();
        fs::write(dir.path().join("gradle").join("libs.versions.toml"), "").unwrap();
        fs::write(dir.path().join("gradle").join("tools.versions.toml"), "").unwrap();
        fs::write(dir.path().join("gradle").join("not-catalog.toml"), "").unwrap();

        let manifests = detect_manifests(dir.path());
        let java_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.language == Language::Java)
            .collect();

        assert_eq!(java_manifests.len(), 2);
        assert!(
            java_manifests
                .iter()
                .any(|m| m.path.ends_with("gradle/libs.versions.toml"))
        );
        assert!(
            java_manifests
                .iter()
                .any(|m| m.path.ends_with("gradle/tools.versions.toml"))
        );
    }
}
