//! Swift Package Manager プロジェクト向けの `Package.swift` パーサ。
//!
//! 対応対象:
//! - `.package(url:, from:)` / `.package(name:, url:, from:)` → キャレット
//! - `.package(url:, .upToNextMajor(from:))` → キャレット
//! - `.package(url:, .upToNextMinor(from:))` → チルダ
//! - `.package(url:, exact:)` / `.package(url:, .exact())` → Exact (固定)
//! - `.package(url:, "V1"..<"V2")` → レンジ
//! - `.package(url:, "V1"..."V2")` → レンジ
//! - `.package(path:)` → スキップ (ローカル依存)
//! - `branch:` / `revision:` / `.branch()` / `.revision()` → スキップ (バージョンなし)
//! - 行コメント (`//`) とブロックコメント (`/* ... */`) はスキップ
//! - 複数行の `.package()` 宣言に対応

use crate::domain::{Dependency, Language, VersionSpec, VersionSpecKind};
use crate::error::ManifestError;
use crate::manifest::ManifestParser;
use crate::parser::{SwiftVersionParser, VersionParser};
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// `Package.swift` 用パーサ
pub struct PackageSwiftParser;

/// オプションの `name:` パラメータ接頭辞 (Swift 5.2+ で追加、5.5+ で非推奨)
const NAME_OPT: &str = r#"(?:name:\s*"[^"]+"\s*,\s*)?"#;

// .package([name:,] url: "...", from: "VERSION") にマッチする
static FROM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*from:\s*"([^"]+)"\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", .upToNextMajor(from: "VERSION")) にマッチする
static UP_TO_NEXT_MAJOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*\.upToNextMajor\(\s*from:\s*"([^"]+)"\s*\)\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", .upToNextMinor(from: "VERSION")) にマッチする
static UP_TO_NEXT_MINOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*\.upToNextMinor\(\s*from:\s*"([^"]+)"\s*\)\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", exact: "VERSION") にマッチする
static EXACT_KEYWORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*exact:\s*"([^"]+)"\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", .exact("VERSION")) にマッチする
static EXACT_METHOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*\.exact\(\s*"([^"]+)"\s*\)\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", "V1"..<"V2") にマッチする — 半開区間
static RANGE_HALF_OPEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\.\.<\s*"([^"]+)"\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

// .package([name:,] url: "...", "V1"..."V2") にマッチする — 閉区間
static RANGE_CLOSED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\.package\(\s*{}url:\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\.\.\.\s*"([^"]+)"\s*[,)]"#,
        NAME_OPT
    ))
    .unwrap()
});

/// GitHub URL から owner/repo を抽出する
///
/// 対応形式:
/// - URL 例: https://github.com/owner/repo.git
/// - URL 例: https://github.com/owner/repo
/// - SSH URL 例: git@github.com:owner/repo.git
/// - SSH URL 例: ssh://git@github.com/owner/repo.git
/// - SSH over 443 URL 例: ssh://git@ssh.github.com:443/owner/repo.git
fn extract_github_owner_repo(url: &str) -> Option<String> {
    // owner/repo に `?` `#` 空白等の不正文字が混ざると、後段の GitHub API URL 構築で
    // クエリ汚染やパストラバーサルを誘発しうるため、ここで文字種を GitHub 準拠に限定する。
    // 判定はレジストリ層と同じ実装を共有し、二層防御が同じ穴を持たないようにする
    // (以前は両方が個別実装で、どちらも `..` を通していた)。
    use crate::registry::is_valid_registry_id_segment as is_valid_segment;

    // ホスト名は大小文字を区別しないため `https://GitHub.com/...` も GitHub として扱う
    // (区別すると非 GitHub URL 扱いになり無言でスキップされる)
    fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
        s.get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .and_then(|_| s.get(prefix.len()..))
    }

    // HTTPS URL パターン
    fn owner_repo_from_path(path: &str) -> Option<String> {
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.len() >= 2 && is_valid_segment(parts[0]) && is_valid_segment(parts[1]) {
            return Some(format!("{}/{}", parts[0], parts[1]));
        }
        None
    }

    if let Some(rest) = strip_prefix_ignore_ascii_case(url, "https://github.com/") {
        return owner_repo_from_path(rest);
    }

    // Git の標準 SSH URL 形式。GitHub の SSH over 443 用ホストも同じリポジトリへ対応する。
    if let Some(rest) = strip_prefix_ignore_ascii_case(url, "ssh://")
        && let Some((authority, path)) = rest.split_once('/')
    {
        let host_with_port = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        let host = host_with_port
            .split_once(':')
            .map(|(host, _)| host)
            .unwrap_or(host_with_port);

        if host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("ssh.github.com") {
            return owner_repo_from_path(path);
        }
    }

    // SSH URL パターン
    if let Some(rest) = strip_prefix_ignore_ascii_case(url, "git@github.com:") {
        return owner_repo_from_path(rest);
    }

    None
}

fn is_valid_swift_version(version: &str) -> bool {
    SwiftVersionParser.parse(version).is_some()
}

fn swift_dependency(
    url: &str,
    kind: VersionSpecKind,
    raw: String,
    version: &str,
    additional_versions: &[&str],
) -> Option<Dependency> {
    if !is_valid_swift_version(version)
        || additional_versions
            .iter()
            .any(|candidate| !is_valid_swift_version(candidate))
    {
        return None;
    }

    let name = extract_github_owner_repo(url)?;
    let spec = VersionSpec::new(kind, raw, version);
    Some(Dependency::production(name, spec, Language::Swift))
}

/// コメントを空白に置き換え、元のバイト位置を保ったまま検索しやすくする
fn mask_comments(content: &str) -> String {
    fn push_masked(masked: &mut String, ch: char) {
        if matches!(ch, '\n' | '\r') {
            masked.push(ch);
        } else {
            for _ in 0..ch.len_utf8() {
                masked.push(' ');
            }
        }
    }

    let mut masked = String::with_capacity(content.len());
    let mut index = 0;
    let bytes = content.as_bytes();
    let mut in_string = false;
    let mut block_depth = 0usize;

    while index < bytes.len() {
        if block_depth > 0 {
            if bytes[index..].starts_with(b"/*") {
                masked.push_str("  ");
                block_depth += 1;
                index += 2;
                continue;
            }

            if bytes[index..].starts_with(b"*/") {
                masked.push_str("  ");
                block_depth -= 1;
                index += 2;
                continue;
            }

            let ch = content[index..]
                .chars()
                .next()
                .expect("有効な UTF-8 文字を読む");
            push_masked(&mut masked, ch);
            index += ch.len_utf8();
            continue;
        }

        if in_string {
            if bytes[index] == b'\\' {
                masked.push('\\');
                index += 1;
                if index < bytes.len() {
                    let ch = content[index..]
                        .chars()
                        .next()
                        .expect("有効な UTF-8 文字を読む");
                    masked.push(ch);
                    index += ch.len_utf8();
                }
                continue;
            }

            let ch = content[index..]
                .chars()
                .next()
                .expect("有効な UTF-8 文字を読む");
            masked.push(ch);
            index += ch.len_utf8();

            if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if bytes[index..].starts_with(b"//") {
            masked.push_str("  ");
            index += 2;
            while index < bytes.len() {
                let ch = content[index..]
                    .chars()
                    .next()
                    .expect("有効な UTF-8 文字を読む");
                if matches!(ch, '\n' | '\r') {
                    masked.push(ch);
                    index += ch.len_utf8();
                    break;
                }
                push_masked(&mut masked, ch);
                index += ch.len_utf8();
            }
            continue;
        }

        if bytes[index..].starts_with(b"/*") {
            masked.push_str("  ");
            block_depth = 1;
            index += 2;
            continue;
        }

        let ch = content[index..]
            .chars()
            .next()
            .expect("有効な UTF-8 文字を読む");
        masked.push(ch);
        index += ch.len_utf8();

        if ch == '"' {
            in_string = true;
        }
    }

    masked
}

impl ManifestParser for PackageSwiftParser {
    fn parse(&self, content: &str) -> Result<Vec<Dependency>, ManifestError> {
        // コメントをマスクし、URL やバージョン文字列の位置を保ったまま全体を走査する
        let clean = mask_comments(content);
        let mut found: Vec<(usize, Dependency)> = Vec::new();

        // より具体的なパターンを先に、汎用の FROM_RE を最後に試す
        for caps in UP_TO_NEXT_MAJOR_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(dep) = swift_dependency(
                url,
                VersionSpecKind::Caret,
                version.to_string(),
                version,
                &[],
            ) {
                found.push((pos, dep));
            }
        }

        for caps in UP_TO_NEXT_MINOR_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(dep) = swift_dependency(
                url,
                VersionSpecKind::Tilde,
                version.to_string(),
                version,
                &[],
            ) {
                found.push((pos, dep));
            }
        }

        for caps in EXACT_KEYWORD_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(dep) = swift_dependency(
                url,
                VersionSpecKind::Exact,
                version.to_string(),
                version,
                &[],
            ) {
                found.push((pos, dep));
            }
        }

        for caps in EXACT_METHOD_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(dep) = swift_dependency(
                url,
                VersionSpecKind::Exact,
                version.to_string(),
                version,
                &[],
            ) {
                found.push((pos, dep));
            }
        }

        for caps in RANGE_HALF_OPEN_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let lower = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let raw = format!("{}..<{}", lower, upper);
            if let Some(dep) = swift_dependency(url, VersionSpecKind::Range, raw, lower, &[upper]) {
                found.push((pos, dep));
            }
        }

        for caps in RANGE_CLOSED_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let lower = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let upper = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let raw = format!("{}...{}", lower, upper);
            if let Some(dep) = swift_dependency(url, VersionSpecKind::Range, raw, lower, &[upper]) {
                found.push((pos, dep));
            }
        }

        // FROM_RE を最後に (最も汎用的なパターン)
        for caps in FROM_RE.captures_iter(&clean) {
            let pos = caps.get(0).unwrap().start();
            let url = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(dep) = swift_dependency(
                url,
                VersionSpecKind::Caret,
                version.to_string(),
                version,
                &[],
            ) {
                found.push((pos, dep));
            }
        }

        // 元の順序を保つため位置でソートする
        found.sort_by_key(|(pos, _)| *pos);

        // パッケージ名で重複を除去する
        let mut seen = std::collections::HashSet::new();
        let dependencies = found
            .into_iter()
            .filter(|(_, dep)| seen.insert(dep.name.clone()))
            .map(|(_, dep)| dep)
            .collect();

        Ok(dependencies)
    }

    fn language(&self) -> Language {
        Language::Swift
    }

    fn update_version(
        &self,
        content: &str,
        package: &str,
        new_version: &str,
    ) -> Result<String, ManifestError> {
        // 書き込もうとするバージョンが SPM の Version (semver 2.0.0) として妥当か検証する。
        // SPM は厳格パースなので、`2024.01.15` のような先頭ゼロ付きの CalVer タグを
        // そのまま書き込むと `swift build` が `Invalid semantic version string` で
        // `Package.swift` ごと読み込みに失敗し、プロジェクト全体がビルド不能になる。
        // レジストリ側 (GitHub Tags) の取りこぼしに備えた最後の防波堤として、
        // ファイルを書き換える前に弾く。
        if !is_valid_swift_version(new_version) {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: new_version.to_string(),
                message: "not a valid Swift Package Manager version (semver 2.0.0 requires three numeric segments without leading zeros)".to_string(),
            });
        }

        let escaped_package = regex::escape(package);
        // URL は Swift 文字列リテラル内にあるため、リポジトリ名の直後には必ず閉じ引用符
        // (`"`) かパス区切り等の境界文字が続く。境界文字を要求することで、
        // `grpc/grpc-swift` の更新が先に宣言された `grpc/grpc-swift-nio` の URL へ
        // 前方一致して別依存を書き換えるのを防ぐ (前方一致は境界で弾かれるため、
        // 最初のマッチが境界一致した正しい宣言になる)。
        // ホスト名は大小文字を区別しないため `(?i:...)` で囲む。`extract_github_owner_repo`
        // が `https://GitHub.com/...` を受理する一方でここが大小区別のままだと、parse は
        // 依存として surface するのに書き換え先が見つからず report/apply が矛盾する。
        // owner/repo 側まで大小無視にすると別リポジトリへ誤爆するため、ホスト部だけを囲む。
        // `extract_github_owner_repo` は GitHub の SSH over 443
        // (`ssh://git@ssh.github.com:443/owner/repo.git`) も正規形として受理するため、
        // ホスト部に `ssh.` 接頭辞とポート番号を許容しないと parse だけ通って
        // 書き換え先が見つからず report/apply が矛盾する
        let url_pattern = format!(
            r#"((?i:(?:ssh\.)?github\.com)(?::\d+)?[/:]{}(?:\.git)?)["'/?#\s)]"#,
            escaped_package
        );
        let masked = mask_comments(content);

        let url_re = Regex::new(&url_pattern).map_err(|e| ManifestError::InvalidVersionSpec {
            path: PathBuf::from("Package.swift"),
            spec: package.to_string(),
            message: format!("invalid regex pattern: {}", e),
        })?;

        // SPM は semver 2.0.0 準拠のためプレリリース識別子とビルドメタデータも許容する
        let version_re = Regex::new(
            r#""((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)""#,
        )
            .map_err(|e| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: format!("invalid regex pattern: {}", e),
            })?;

        // 全体から URL を検索する (複数行宣言に対応)。
        // 境界文字込みでマッチさせ、以降の位置計算には URL 本体 (グループ1) の範囲を使う
        let url_match = url_re
            .captures(&masked)
            .and_then(|caps| caps.get(1))
            .ok_or_else(|| ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            })?;

        // 囲む .package() 宣言を見つける
        let prefix = &masked[..url_match.start()];
        let pkg_start =
            prefix
                .rfind(".package(")
                .ok_or_else(|| ManifestError::InvalidVersionSpec {
                    path: PathBuf::from("Package.swift"),
                    spec: package.to_string(),
                    message: "package not found or version could not be updated".to_string(),
                })?;

        // .package( から URL 末尾までの括弧深度を数える
        let mut depth: i32 = 0;
        for c in masked[pkg_start..url_match.end()].chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }

        if depth <= 0 {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        }

        // 対応する閉じ括弧を見つける
        let mut end_pos = content.len();
        for (i, c) in masked[url_match.end()..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end_pos = url_match.end() + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        // URL 末尾からパッケージ宣言末尾までの実バージョン文字列だけを差し替える
        let masked_section = &masked[url_match.end()..end_pos];
        let Some(version_match) = version_re.find(masked_section) else {
            return Err(ManifestError::InvalidVersionSpec {
                path: PathBuf::from("Package.swift"),
                spec: package.to_string(),
                message: "package not found or version could not be updated".to_string(),
            });
        };

        let replace_start = url_match.end() + version_match.start();
        let replace_end = url_match.end() + version_match.end();

        Ok(format!(
            "{}\"{}\"{}",
            &content[..replace_start],
            new_version,
            &content[replace_end..]
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::VersionSpecKind;

    fn parse(content: &str) -> Result<Vec<Dependency>, ManifestError> {
        PackageSwiftParser.parse(content)
    }

    #[test]
    fn test_parse_from_version() {
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_up_to_next_major() {
        let content =
            r#".package(url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.0.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "4.0.0");
    }

    #[test]
    fn test_parse_up_to_next_minor() {
        let content =
            r#".package(url: "https://github.com/vapor/vapor.git", .upToNextMinor(from: "4.5.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(deps[0].version_spec.version, "4.5.0");
    }

    #[test]
    fn test_parse_exact_keyword() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert!(deps[0].is_pinned());
    }

    #[test]
    fn test_parse_exact_method() {
        let content =
            r#".package(url: "https://github.com/apple/swift-nio.git", .exact("2.40.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    #[test]
    fn test_parse_range_half_open() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", "4.0.0"..<"5.0.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
        assert_eq!(deps[0].version_spec.version, "4.0.0");
    }

    #[test]
    fn test_parse_range_closed() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", "4.0.0"..."4.9.9")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Range);
    }

    #[test]
    fn test_skip_branch() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", branch: "main")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_revision() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", revision: "abc123")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_comments() {
        let content = r#"
        // .package(url: "https://github.com/vapor/vapor.git", from: "4.0.0")
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.0.0")
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
    }

    #[test]
    fn test_skip_non_github_url() {
        let content = r#".package(url: "https://gitlab.com/some/repo.git", from: "1.0.0")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_multiple_dependencies() {
        let content = r#"
let package = Package(
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
        .package(url: "https://github.com/vapor/vapor.git", from: "4.0.0"),
        .package(url: "https://github.com/apple/swift-nio.git", .upToNextMinor(from: "2.40.0")),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[1].name, "vapor/vapor");
        assert_eq!(deps[2].name, "apple/swift-nio");
    }

    #[test]
    fn test_parse_empty() {
        let deps = parse("").unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_extract_github_owner_repo_https() {
        assert_eq!(
            extract_github_owner_repo("https://github.com/apple/swift-argument-parser.git"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_https_no_git() {
        assert_eq!(
            extract_github_owner_repo("https://github.com/apple/swift-argument-parser"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_ssh() {
        assert_eq!(
            extract_github_owner_repo("git@github.com:apple/swift-argument-parser.git"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_standard_ssh_url() {
        assert_eq!(
            extract_github_owner_repo("ssh://git@github.com/apple/swift-argument-parser.git"),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    #[test]
    fn test_extract_github_owner_repo_ssh_over_443_url() {
        assert_eq!(
            extract_github_owner_repo(
                "ssh://git@ssh.github.com:443/apple/swift-argument-parser.git"
            ),
            Some("apple/swift-argument-parser".to_string())
        );
    }

    /// parse できる URL 形式はすべて update_version でも書き換えられること。
    /// parse (正規化ベース) と update (生 URL の正規表現) が二重定義になっているため、
    /// 片方だけ対応した形式があると「更新あり」と報告してから書き込みが失敗する
    #[test]
    fn test_update_version_supports_every_parsable_github_url_form() {
        for url in [
            "https://github.com/apple/swift-log.git",
            "https://github.com/apple/swift-log",
            "git@github.com:apple/swift-log.git",
            "ssh://git@github.com/apple/swift-log.git",
            "ssh://git@ssh.github.com:443/apple/swift-log.git",
        ] {
            let content = format!(
                "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(\n    name: \"X\",\n    dependencies: [\n        .package(url: \"{url}\", from: \"1.0.0\"),\n    ]\n)\n"
            );

            let deps = PackageSwiftParser.parse(&content).unwrap();
            assert_eq!(deps.len(), 1, "parse できるべき: {url}");
            assert_eq!(deps[0].name, "apple/swift-log", "{url}");

            let updated = PackageSwiftParser
                .update_version(&content, "apple/swift-log", "1.5.0")
                .unwrap_or_else(|e| panic!("update できるべき: {url} ({e})"));
            assert!(updated.contains("from: \"1.5.0\""), "{url}: {updated}");
            // URL 自体は書き換えない
            assert!(updated.contains(url), "{url}: {updated}");
        }
    }

    #[test]
    fn test_parse_standard_ssh_url_dependency() {
        let content = r#"
            .package(
                url: "ssh://git@github.com/apple/swift-argument-parser.git",
                from: "1.2.0"
            )
        "#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.raw, "1.2.0");
    }

    #[test]
    fn test_extract_github_owner_repo_rejects_lookalike_ssh_host() {
        assert_eq!(
            extract_github_owner_repo("ssh://git@github.com.example/owner/repo.git"),
            None
        );
    }

    #[test]
    fn test_extract_github_owner_repo_non_github() {
        assert_eq!(
            extract_github_owner_repo("https://gitlab.com/some/repo.git"),
            None
        );
    }

    #[test]
    fn test_update_version_from() {
        let content = r#"
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
        .package(url: "https://github.com/vapor/vapor.git", from: "4.0.0"),
"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-argument-parser", "1.3.0")
            .unwrap();
        assert!(result.contains(r#"from: "1.3.0""#));
        // 他のパッケージは変更されない
        assert!(result.contains(r#"from: "4.0.0""#));
    }

    #[test]
    fn test_update_version_exact() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.41.0")
            .unwrap();
        assert!(result.contains(r#"exact: "2.41.0""#));
    }

    #[test]
    fn test_update_version_not_found() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#;
        let result = PackageSwiftParser.update_version(content, "nonexistent/repo", "1.0.0");
        assert!(result.is_err());
    }

    /// バグ回帰テスト: SPM が読めないバージョンは書き込む前に弾く。
    ///
    /// 以前は `new_version` を検証せず `"{}"` に埋めていたため、GitHub の CalVer タグ
    /// (`2024.01.15`) が候補として採用されると、`swift build` が
    /// `Invalid semantic version string` で `Package.swift` ごと読めなくなる版を
    /// 書き込んでいた。
    #[test]
    fn test_update_version_rejects_invalid_swift_version() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#;
        for invalid in [
            "2024.01.15", // 先頭ゼロを含む CalVer タグ
            "1.02.0",
            "1.2.03",
            "1.2.3-01", // 先頭ゼロの数値プレリリース識別子
            "1.2",      // 2 セグメント
            "v1.2.3",   // v 接頭辞は SPM のバージョン文字列に付かない
            "1.2.3.4",  // 4 セグメント
            "not-a-version",
            "",
        ] {
            let result = PackageSwiftParser.update_version(content, "apple/swift-nio", invalid);
            assert!(
                result.is_err(),
                "expected error for invalid version: {:?}",
                invalid
            );
        }
        // 元ファイルは書き換えられない (エラーを返すだけ)
        assert!(content.contains(r#"from: "2.40.0""#));
    }

    #[test]
    fn test_update_version_accepts_valid_semver_forms() {
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#;
        for valid in [
            "2.41.0",
            "1.0.0-beta.1",
            "1.0.0+build.123",
            "1.0.0-rc.1+sha.abc",
        ] {
            let result = PackageSwiftParser
                .update_version(content, "apple/swift-nio", valid)
                .unwrap_or_else(|e| panic!("expected success for {}: {}", valid, e));
            assert!(result.contains(&format!(r#"from: "{}""#, valid)));
        }
    }

    #[test]
    fn test_update_version_url_prefix_collision_longer_first() {
        // 回帰テスト: 名前プレフィックスを共有する別依存 (grpc-swift-nio) が先に
        // 宣言されていても、grpc-swift の更新が nio 側の URL に前方一致して
        // nio 側の from を書き換えない
        let content = r#"
let package = Package(
    dependencies: [
        .package(url: "https://github.com/grpc/grpc-swift-nio.git", from: "1.0.0"),
        .package(url: "https://github.com/grpc/grpc-swift.git", from: "1.10.0"),
    ]
)
"#;
        let result = PackageSwiftParser
            .update_version(content, "grpc/grpc-swift", "1.11.0")
            .unwrap();
        assert!(
            result.contains(
                r#".package(url: "https://github.com/grpc/grpc-swift.git", from: "1.11.0")"#
            ),
            "grpc-swift should be updated, but got:\n{}",
            result
        );
        assert!(
            result.contains(
                r#".package(url: "https://github.com/grpc/grpc-swift-nio.git", from: "1.0.0")"#
            ),
            "grpc-swift-nio should not be modified, but got:\n{}",
            result
        );
    }

    #[test]
    fn test_update_version_url_prefix_collision_shorter_first() {
        // 逆順 (本体が先、nio が後) でも正しい宣言だけを更新する
        let content = r#"
let package = Package(
    dependencies: [
        .package(url: "https://github.com/grpc/grpc-swift.git", from: "1.10.0"),
        .package(url: "https://github.com/grpc/grpc-swift-nio.git", from: "1.0.0"),
    ]
)
"#;
        let result = PackageSwiftParser
            .update_version(content, "grpc/grpc-swift", "1.11.0")
            .unwrap();
        assert!(result.contains(r#"grpc-swift.git", from: "1.11.0""#));
        assert!(result.contains(r#"grpc-swift-nio.git", from: "1.0.0""#));

        // 長い方 (grpc-swift-nio) の更新も自身の宣言だけに当たる
        let result = PackageSwiftParser
            .update_version(content, "grpc/grpc-swift-nio", "1.2.0")
            .unwrap();
        assert!(result.contains(r#"grpc-swift-nio.git", from: "1.2.0""#));
        assert!(result.contains(r#"grpc-swift.git", from: "1.10.0""#));
    }

    #[test]
    fn test_update_version_url_prefix_collision_without_git_extension() {
        // `.git` 拡張なしの URL (閉じ引用符が直後に続く) でも境界判定が機能する
        let content = r#"
        .package(url: "https://github.com/grpc/grpc-swift-nio", from: "1.0.0"),
        .package(url: "https://github.com/grpc/grpc-swift", from: "1.10.0"),
"#;
        let result = PackageSwiftParser
            .update_version(content, "grpc/grpc-swift", "1.11.0")
            .unwrap();
        assert!(result.contains(r#"grpc-swift", from: "1.11.0""#));
        assert!(result.contains(r#"grpc-swift-nio", from: "1.0.0""#));
    }

    #[test]
    fn test_parser_language() {
        assert_eq!(PackageSwiftParser.language(), Language::Swift);
    }

    #[test]
    fn test_parse_url_without_git_extension() {
        let content = r#".package(url: "https://github.com/apple/swift-log", from: "1.0.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-log");
    }

    // --- name: パラメータのサポート ---

    #[test]
    fn test_parse_with_name_parameter_from() {
        let content = r#".package(name: "ArgumentParser", url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_with_name_up_to_next_major() {
        let content = r#".package(name: "Vapor", url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.0.0"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_with_name_exact() {
        let content = r#".package(name: "SwiftNIO", url: "https://github.com/apple/swift-nio.git", exact: "2.40.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
    }

    // --- path: 依存 (スキップされるべき) ---

    #[test]
    fn test_skip_path_dependency() {
        let content = r#".package(path: "../some-local-package")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_path_dependency_with_name() {
        let content = r#".package(name: "LocalLib", path: "../local-lib")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    // --- .branch() / .revision() メソッド構文 ---

    #[test]
    fn test_skip_branch_method_syntax() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", .branch("main"))"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_skip_revision_method_syntax() {
        let content = r#".package(url: "https://github.com/vapor/vapor.git", .revision("abc123"))"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    // --- 複数行宣言 ---

    #[test]
    fn test_parse_multiline_from() {
        let content = ".package(\n    url: \"https://github.com/apple/swift-argument-parser.git\",\n    from: \"1.2.0\"\n)";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_parse_multiline_up_to_next_major() {
        let content = ".package(\n    url: \"https://github.com/vapor/vapor.git\",\n    .upToNextMajor(from: \"4.0.0\")\n)";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "vapor/vapor");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
    }

    #[test]
    fn test_parse_multiline_with_name() {
        let content = ".package(\n    name: \"ArgumentParser\",\n    url: \"https://github.com/apple/swift-argument-parser.git\",\n    from: \"1.2.0\"\n)";
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.version, "1.2.0");
    }

    #[test]
    fn test_update_version_multiline() {
        let content = ".package(\n    url: \"https://github.com/apple/swift-argument-parser.git\",\n    from: \"1.2.0\"\n)";
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-argument-parser", "1.3.0")
            .unwrap();
        assert!(result.contains("from: \"1.3.0\""));
        assert!(result.contains(".package(\n"));
    }

    #[test]
    fn test_update_version_with_name_parameter() {
        let content = r#".package(name: "ArgumentParser", url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-argument-parser", "1.3.0")
            .unwrap();
        assert!(result.contains(r#"from: "1.3.0""#));
        assert!(result.contains(r#"name: "ArgumentParser""#));
    }

    // --- 実運用に近い Package.swift ---

    #[test]
    fn test_update_version_range_preserves_upper_bound() {
        // レンジ構文で上限が誤って置換されないことを確認
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio.git", "1.0.0"..<"2.0.0"),
    ]
)
"#;

        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "1.5.0")
            .unwrap();
        assert!(result.contains(r#""1.5.0"..<"2.0.0""#));
    }

    #[test]
    fn test_update_version_closed_range_preserves_upper_bound() {
        // 閉区間 `...` でも下限のみ更新し、上限を保持することを確認
        // (半開 `..<` と共通の version_re.find() 経路だが、閉区間の直接検証がなかった)
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/apple/swift-nio.git", "1.0.0"..."2.0.0"),
    ]
)
"#;

        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "1.5.0")
            .unwrap();
        assert!(
            result.contains(r#""1.5.0"..."2.0.0""#),
            "閉区間の下限のみ更新し上限を保持できていない: {result}"
        );
    }

    #[test]
    fn test_parse_realistic_package_swift() {
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    platforms: [
        .macOS(.v13),
        .iOS(.v16)
    ],
    dependencies: [
        .package(
            url: "https://github.com/apple/swift-argument-parser.git",
            from: "1.2.0"
        ),
        .package(url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.89.0")),
        .package(name: "SwiftNIO", url: "https://github.com/apple/swift-nio.git", exact: "2.40.0"),
        // .package(url: "https://github.com/old/dep.git", from: "0.1.0"),
        .package(url: "https://github.com/grpc/grpc-swift.git", branch: "main"),
        .package(path: "../my-local-lib"),
    ],
    targets: [
        .target(name: "MyApp", dependencies: [
            .product(name: "ArgumentParser", package: "swift-argument-parser"),
            .product(name: "Vapor", package: "vapor"),
        ]),
    ]
)"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "apple/swift-argument-parser");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.2.0");
        assert_eq!(deps[1].name, "vapor/vapor");
        assert_eq!(deps[1].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[1].version_spec.version, "4.89.0");
        assert_eq!(deps[2].name, "apple/swift-nio");
        assert_eq!(deps[2].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[2].version_spec.version, "2.40.0");
    }

    // --- 追加エッジケーステスト ---

    #[test]
    fn test_parse_url_without_dot_git_from() {
        // .git 拡張なしの URL でも正しくパースされること
        let content = r#".package(url: "https://github.com/owner/repo", from: "1.0.0")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "owner/repo");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_v_prefix_in_version() {
        // Git タグ名では `v1.0.0` を認識するが、Package.swift の requirement 文字列は
        // SPM の Version として semver 2.0.0 形式 (`1.0.0`) でなければならない。
        let content = r#".package(url: "https://github.com/owner/repo.git", from: "v1.0.0")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_multiple_mixed_constraint_types() {
        // 複数の依存関係が異なる制約タイプを使用する Package.swift
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MixedDeps",
    dependencies: [
        .package(url: "https://github.com/apple/swift-log", from: "1.5.0"),
        .package(url: "https://github.com/vapor/vapor.git", .upToNextMinor(from: "4.89.0")),
        .package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0"),
        .package(url: "https://github.com/swift-server/async-http-client.git", "1.0.0"..<"2.0.0"),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4);

        // from: 指定は Caret として扱う
        let swift_log = deps.iter().find(|d| d.name == "apple/swift-log").unwrap();
        assert_eq!(swift_log.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(swift_log.version_spec.version, "1.5.0");

        // upToNextMinor 指定は Tilde として扱う
        let vapor = deps.iter().find(|d| d.name == "vapor/vapor").unwrap();
        assert_eq!(vapor.version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(vapor.version_spec.version, "4.89.0");

        // exact: 指定は Exact として扱う
        let nio = deps.iter().find(|d| d.name == "apple/swift-nio").unwrap();
        assert_eq!(nio.version_spec.kind, VersionSpecKind::Exact);

        // ..< 指定は Range として扱う
        let http_client = deps
            .iter()
            .find(|d| d.name == "swift-server/async-http-client")
            .unwrap();
        assert_eq!(http_client.version_spec.kind, VersionSpecKind::Range);
        assert_eq!(http_client.version_spec.version, "1.0.0");
    }

    #[test]
    fn test_parse_name_parameter() {
        // name: パラメータ付きの各種制約タイプが正しくパースされること
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(name: "ArgumentParser", url: "https://github.com/apple/swift-argument-parser.git", from: "1.2.0"),
        .package(name: "Vapor", url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.0.0")),
        .package(name: "SwiftNIO", url: "https://github.com/apple/swift-nio.git", .upToNextMinor(from: "2.40.0")),
        .package(name: "GRPC", url: "https://github.com/grpc/grpc-swift.git", exact: "1.0.0"),
        .package(name: "GRPCMethod", url: "https://github.com/grpc/grpc-swift-nio.git", .exact("2.0.0")),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 5);

        // name: パラメータは無視され、URL からパッケージ名が抽出される
        let parser_dep = deps
            .iter()
            .find(|d| d.name == "apple/swift-argument-parser")
            .unwrap();
        assert_eq!(parser_dep.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(parser_dep.version_spec.version, "1.2.0");

        let vapor = deps.iter().find(|d| d.name == "vapor/vapor").unwrap();
        assert_eq!(vapor.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(vapor.version_spec.version, "4.0.0");

        let nio = deps.iter().find(|d| d.name == "apple/swift-nio").unwrap();
        assert_eq!(nio.version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(nio.version_spec.version, "2.40.0");

        let grpc = deps.iter().find(|d| d.name == "grpc/grpc-swift").unwrap();
        assert_eq!(grpc.version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(grpc.version_spec.version, "1.0.0");

        let grpc_method = deps
            .iter()
            .find(|d| d.name == "grpc/grpc-swift-nio")
            .unwrap();
        assert_eq!(grpc_method.version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(grpc_method.version_spec.version, "2.0.0");
    }

    #[test]
    fn test_parse_multiline_dependency() {
        // 複数行にまたがる依存宣言が正しくパースされること
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(
            name: "ArgumentParser",
            url: "https://github.com/apple/swift-argument-parser.git",
            from: "1.2.0"
        ),
        .package(
            url: "https://github.com/vapor/vapor.git",
            .upToNextMajor(
                from: "4.89.0"
            )
        ),
        .package(
            url: "https://github.com/apple/swift-nio.git",
            .upToNextMinor(from: "2.40.0")
        ),
        .package(
            name: "GRPC",
            url: "https://github.com/grpc/grpc-swift.git",
            .exact("1.5.0")
        ),
    ]
)
"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 4, "マルチライン宣言が全てパースされるべき");

        let parser_dep = deps
            .iter()
            .find(|d| d.name == "apple/swift-argument-parser")
            .unwrap();
        assert_eq!(parser_dep.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(parser_dep.version_spec.version, "1.2.0");

        let vapor = deps.iter().find(|d| d.name == "vapor/vapor").unwrap();
        assert_eq!(vapor.version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(vapor.version_spec.version, "4.89.0");

        let nio = deps.iter().find(|d| d.name == "apple/swift-nio").unwrap();
        assert_eq!(nio.version_spec.kind, VersionSpecKind::Tilde);
        assert_eq!(nio.version_spec.version, "2.40.0");

        let grpc = deps.iter().find(|d| d.name == "grpc/grpc-swift").unwrap();
        assert_eq!(grpc.version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(grpc.version_spec.version, "1.5.0");
    }

    #[test]
    fn test_update_version_v_prefix() {
        // v プレフィックス付き requirement は SPM の Version として無効なので更新対象外
        let content = r#"
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyApp",
    dependencies: [
        .package(url: "https://github.com/owner/repo.git", from: "v1.0.0"),
    ]
)
"#;
        let result = PackageSwiftParser.update_version(content, "owner/repo", "1.2.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_skips_block_comments() {
        let content = r#"
/*
.package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")
*/
.package(url: "https://github.com/apple/swift-log.git", from: "1.5.0")
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-log");
    }

    #[test]
    fn test_parse_skips_nested_block_comments() {
        let content = r#"
/*
.package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")
/*
.package(url: "https://github.com/grpc/grpc-swift.git", from: "1.5.0")
*/
*/
.package(url: "https://github.com/apple/swift-log.git", from: "1.5.0")
"#;

        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-log");
    }

    #[test]
    fn test_update_version_ignores_block_commented_dependency() {
        let content = r#"
/*
.package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")
*/
.package(url: "https://github.com/apple/swift-nio.git", from: "2.41.0")
"#;

        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.42.0")
            .unwrap();

        assert!(result.contains(
            r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.42.0")"#
        ));
        assert!(result.contains(
            r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#
        ));
    }

    // --- semver 2.0.0 のプレリリース / ビルドメタデータ対応 ---

    #[test]
    fn test_parse_prerelease_from_version() {
        // from: にプレリリース付きバージョンを指定するケース
        let content =
            r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0-beta.1")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "2.40.0-beta.1");
    }

    #[test]
    fn test_parse_rejects_non_semver_versions() {
        // SPM の Version は semver 2.0.0 準拠なので、2/4 セグメントや先頭ゼロは対象外
        let content = r#"
.package(url: "https://github.com/apple/swift-nio.git", from: "2.40")
.package(url: "https://github.com/vapor/vapor.git", from: "4.0.0.1")
.package(url: "https://github.com/apple/swift-argument-parser.git", from: "01.2.3")
"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_rejects_non_semver_range_bound() {
        let content =
            r#".package(url: "https://github.com/apple/swift-nio.git", "2.40.0"..<"3.0")"#;
        let deps = parse(content).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_prerelease_up_to_next_major() {
        // .upToNextMajor(from:) にプレリリース付きバージョンを指定するケース
        let content = r#".package(url: "https://github.com/vapor/vapor.git", .upToNextMajor(from: "4.0.0-rc.1"))"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Caret);
        assert_eq!(deps[0].version_spec.version, "4.0.0-rc.1");
    }

    #[test]
    fn test_parse_prerelease_exact() {
        // exact: にプレリリース付きバージョンを指定するケース
        let content =
            r#".package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0-alpha.1")"#;
        let deps = parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].version_spec.kind, VersionSpecKind::Exact);
        assert_eq!(deps[0].version_spec.version, "2.40.0-alpha.1");
    }

    #[test]
    fn test_update_version_prerelease_to_stable() {
        // プレリリースから安定版への更新が正しく行われる
        let content =
            r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0-beta.1")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.40.0")
            .unwrap();
        assert!(result.contains(r#"from: "2.40.0""#));
    }

    #[test]
    fn test_update_version_stable_to_prerelease() {
        // 安定版からプレリリースへの更新も正しく行える (新しい RC 等)
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.41.0-rc.1")
            .unwrap();
        assert!(result.contains(r#"from: "2.41.0-rc.1""#));
    }

    #[test]
    fn test_update_version_build_metadata() {
        // ビルドメタデータ付きバージョンへの更新
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0")"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.41.0+build.123")
            .unwrap();
        assert!(result.contains(r#"from: "2.41.0+build.123""#));
    }

    #[test]
    fn test_parse_from_with_traits() {
        // SPM 6.1 の traits: 引数が末尾に付いても version requirement を解析できる
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0", traits: ["FeatureX"])"#;
        let deps = PackageSwiftParser.parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
        assert_eq!(deps[0].version_spec.version, "2.40.0");
    }

    #[test]
    fn test_parse_exact_with_module_aliases() {
        // moduleAliases: 引数が末尾に付いても version requirement を解析できる
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", exact: "2.40.0", moduleAliases: ["NIO": "AppNIO"])"#;
        let deps = PackageSwiftParser.parse(content).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "apple/swift-nio");
        assert_eq!(deps[0].version_spec.version, "2.40.0");
    }

    #[test]
    fn test_update_version_from_with_traits_preserved() {
        // traits: 引数を保ったまま version requirement だけ更新する
        let content = r#".package(url: "https://github.com/apple/swift-nio.git", from: "2.40.0", traits: ["FeatureX"])"#;
        let result = PackageSwiftParser
            .update_version(content, "apple/swift-nio", "2.41.0")
            .unwrap();
        assert!(result.contains(r#"from: "2.41.0""#));
        assert!(result.contains(r#"traits: ["FeatureX"]"#));
    }

    #[test]
    fn test_extract_github_owner_repo_rejects_invalid_chars() {
        // owner/repo に不正文字 (? 空白等) が混ざる URL は None を返す (URL インジェクション防止)
        assert_eq!(
            extract_github_owner_repo("https://github.com/owner/repo?evil=1"),
            None
        );
        assert_eq!(
            extract_github_owner_repo("https://github.com/own er/rep o"),
            None
        );
        // 正常な URL は owner/repo を返す
        assert_eq!(
            extract_github_owner_repo("https://github.com/apple/swift-nio.git"),
            Some("apple/swift-nio".to_string())
        );
        assert_eq!(
            extract_github_owner_repo("git@github.com:apple/swift-nio.git"),
            Some("apple/swift-nio".to_string())
        );
    }

    #[test]
    fn test_parse_registry_id_dependency_not_yet_supported() {
        // Swift Package Registry の id: 依存 (.package(id: "scope.name", ...)) は現状未対応で
        // 検出されない (既知の制限、README 参照)。registry API アダプタが未実装のため、
        // 現時点では GitHub URL 依存のみを対象とする。
        let content = r#"
let package = Package(
    name: "x",
    dependencies: [
        .package(id: "apple.swift-nio", from: "2.40.0"),
    ]
)
"#;
        let deps = PackageSwiftParser.parse(content).unwrap();
        assert_eq!(deps.len(), 0);
    }
}
