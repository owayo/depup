//! Tauri バージョン同期
//!
//! Tauri の npm パッケージと tauri クレートのメジャー.マイナーバージョンを
//! 一致させ、ビルドエラーを防止する。
//!
//! Tauri の要件:
//! - npm パッケージ: @tauri-apps/api, @tauri-apps/cli
//! - crates.io クレート: tauri
//!   全てが同じメジャー.マイナーバージョンである必要がある (例: 2.10.x)

use crate::domain::{Language, UpdateResult};
use crate::update::VersionInfo;
use std::cmp::Ordering;

/// Tauri npm パッケージのパッケージ名
pub const TAURI_NPM_PACKAGES: &[&str] = &["@tauri-apps/api", "@tauri-apps/cli"];

/// Tauri クレートのパッケージ名
pub const TAURI_CRATE: &str = "tauri";

/// バージョン文字列からメジャー.マイナーを抽出する
/// 例: "2.10.1" -> Some((2, 10))
pub fn extract_major_minor(version: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        Some((major, minor))
    } else {
        None
    }
}

/// メジャー.マイナーバージョンを比較する
/// 一致する場合は true を返す
pub fn versions_match(v1: &str, v2: &str) -> bool {
    match (extract_major_minor(v1), extract_major_minor(v2)) {
        (Some((m1, n1)), Some((m2, n2))) => m1 == m2 && n1 == n2,
        _ => false,
    }
}

/// 利用可能なバージョンから、ターゲットバージョンと同じメジャー.マイナーを持つ
/// 最適なバージョンを見つける
pub fn find_matching_version(
    target_major_minor: (u32, u32),
    available_versions: &[VersionInfo],
) -> Option<&VersionInfo> {
    available_versions
        .iter()
        .filter(|v| !v.is_prerelease())
        .filter(|v| {
            if let Some((major, minor)) = extract_major_minor(&v.version) {
                major == target_major_minor.0 && minor == target_major_minor.1
            } else {
                false
            }
        })
        .max() // メジャー.マイナー内の最新パッチバージョンを取得
}

/// npm と crates.io の両方で利用可能な最高のメジャー.マイナーバージョンを見つける
pub fn find_common_major_minor(
    npm_versions: &[VersionInfo],
    crate_versions: &[VersionInfo],
) -> Option<(u32, u32)> {
    // npm からメジャー.マイナーペアを収集 (安定版のみ)
    let npm_pairs: std::collections::HashSet<(u32, u32)> = npm_versions
        .iter()
        .filter(|v| !v.is_prerelease())
        .filter_map(|v| extract_major_minor(&v.version))
        .collect();

    // crates.io からメジャー.マイナーペアを収集 (安定版のみ)
    let crate_pairs: std::collections::HashSet<(u32, u32)> = crate_versions
        .iter()
        .filter(|v| !v.is_prerelease())
        .filter_map(|v| extract_major_minor(&v.version))
        .collect();

    // 共通ペアを見つける
    let common: Vec<(u32, u32)> = npm_pairs.intersection(&crate_pairs).copied().collect();

    // 最高の共通メジャー.マイナーを返す
    common.into_iter().max_by(|a, b| match a.0.cmp(&b.0) {
        Ordering::Equal => a.1.cmp(&b.1),
        ord => ord,
    })
}

/// Tauri パッケージバージョンの同期
///
/// 全ての Tauri パッケージのメジャー.マイナーバージョンが一致することを保証する。
pub struct TauriVersionSync {
    /// @tauri-apps/api の利用可能な npm バージョン (全 npm パッケージに使用)
    npm_versions: Vec<VersionInfo>,
    /// tauri の利用可能な crates.io バージョン
    crate_versions: Vec<VersionInfo>,
}

impl TauriVersionSync {
    /// 利用可能なバージョンで新しい TauriVersionSync を作成する
    pub fn new(npm_versions: Vec<VersionInfo>, crate_versions: Vec<VersionInfo>) -> Self {
        Self {
            npm_versions,
            crate_versions,
        }
    }

    /// 全パッケージの同期済みターゲットバージョンを取得する
    ///
    /// ターゲットのクレートバージョンを受け取り、全パッケージが使用すべき
    /// メジャー.マイナーを返す。戻り値は (npm_target_version, crate_target_version)。
    pub fn get_synchronized_versions(
        &self,
        crate_target_version: &str,
    ) -> Option<(String, String)> {
        let target_mm = extract_major_minor(crate_target_version)?;

        // このメジャー.マイナーが npm で利用可能かチェック
        let npm_version = find_matching_version(target_mm, &self.npm_versions)?;
        let crate_version = find_matching_version(target_mm, &self.crate_versions)?;

        Some((npm_version.version.clone(), crate_version.version.clone()))
    }

    /// 両方のレジストリで利用可能な最高の共通バージョンを取得する
    pub fn get_latest_common_version(&self) -> Option<(String, String)> {
        let common_mm = find_common_major_minor(&self.npm_versions, &self.crate_versions)?;

        let npm_version = find_matching_version(common_mm, &self.npm_versions)?;
        let crate_version = find_matching_version(common_mm, &self.crate_versions)?;

        Some((npm_version.version.clone(), crate_version.version.clone()))
    }

    /// パッケージが Tauri npm パッケージかどうかをチェックする
    pub fn is_tauri_npm_package(name: &str) -> bool {
        TAURI_NPM_PACKAGES.contains(&name)
    }

    /// パッケージが Tauri クレートかどうかをチェックする
    pub fn is_tauri_crate(name: &str) -> bool {
        name == TAURI_CRATE
    }

    /// パッケージが何らかの Tauri パッケージかどうかをチェックする
    pub fn is_tauri_package(name: &str, language: Language) -> bool {
        match language {
            Language::Node => Self::is_tauri_npm_package(name),
            Language::Rust => Self::is_tauri_crate(name),
            _ => false,
        }
    }

    /// Tauri パッケージの更新結果を同期する
    ///
    /// 現在のバージョンと更新結果を受け取り、バージョンの調整が必要かを判定する。
    /// 戻り値は (npm_target, crate_target) で、Some の場合はバージョンの更新/調整が必要。
    pub fn synchronize_with_current(
        &self,
        npm_current: Option<&str>,
        npm_update: Option<&UpdateResult>,
        crate_current: Option<&str>,
        crate_update: Option<&UpdateResult>,
    ) -> (Option<String>, Option<String>) {
        // 実効ターゲットバージョンを決定
        let npm_target = npm_update
            .and_then(|r| match r {
                UpdateResult::Update { new_version, .. } => Some(new_version.as_str()),
                _ => None,
            })
            .or(npm_current);

        let crate_target = crate_update
            .and_then(|r| match r {
                UpdateResult::Update { new_version, .. } => Some(new_version.as_str()),
                _ => None,
            })
            .or(crate_current);

        // 両方が揃わなければ同期できない
        let (npm_v, crate_v) = match (npm_target, crate_target) {
            (Some(n), Some(c)) => (n, c),
            _ => return (None, None),
        };

        // 既に一致しているかチェック
        if versions_match(npm_v, crate_v) {
            return (None, None);
        }

        // 一致しない場合、最高の共通バージョンを見つける
        let (npm_sync, crate_sync) = match self.get_latest_common_version() {
            Some(v) => v,
            None => return (None, None),
        };

        // 設定予定のバージョンから変更が必要な場合のみバージョンを返す
        let npm_result = if npm_update.map(|r| r.is_update()).unwrap_or(false) {
            // npm に更新がある場合、調整が必要かチェック
            let planned = match npm_update.unwrap() {
                UpdateResult::Update { new_version, .. } => new_version.as_str(),
                _ => npm_v,
            };
            if planned != npm_sync {
                Some(npm_sync.clone())
            } else {
                None
            }
        } else {
            // npm に更新がない場合、現在のバージョンが異なれば追加が必要
            if npm_current
                .map(|v| !versions_match(v, &crate_sync))
                .unwrap_or(false)
            {
                Some(npm_sync.clone())
            } else {
                None
            }
        };

        let crate_result = if crate_update.map(|r| r.is_update()).unwrap_or(false) {
            // クレートに更新がある場合、調整が必要かチェック
            let planned = match crate_update.unwrap() {
                UpdateResult::Update { new_version, .. } => new_version.as_str(),
                _ => crate_v,
            };
            if planned != crate_sync {
                Some(crate_sync.clone())
            } else {
                None
            }
        } else {
            // クレートに更新がない場合、現在のバージョンが異なれば追加が必要
            if crate_current
                .map(|v| !versions_match(v, &npm_sync))
                .unwrap_or(false)
            {
                Some(crate_sync)
            } else {
                None
            }
        };

        (npm_result, crate_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Dependency, VersionSpec, VersionSpecKind};
    use chrono::Utc;

    fn make_version_info(version: &str) -> VersionInfo {
        VersionInfo::new(version, Utc::now())
    }

    fn make_dependency(name: &str, version: &str, language: Language) -> Dependency {
        let spec = VersionSpec::new(VersionSpecKind::Caret, format!("^{}", version), version)
            .with_prefix("^");
        Dependency::new(name, spec, false, language)
    }

    #[test]
    fn test_extract_major_minor() {
        assert_eq!(extract_major_minor("2.10.1"), Some((2, 10)));
        assert_eq!(extract_major_minor("1.0.0"), Some((1, 0)));
        assert_eq!(extract_major_minor("2.9"), Some((2, 9)));
        assert_eq!(extract_major_minor("invalid"), None);
    }

    #[test]
    fn test_versions_match() {
        assert!(versions_match("2.10.1", "2.10.0"));
        assert!(versions_match("2.10.1", "2.10.5"));
        assert!(!versions_match("2.10.1", "2.9.1"));
        assert!(!versions_match("2.10.1", "3.10.1"));
    }

    #[test]
    fn test_find_common_major_minor() {
        let npm_versions = vec![
            make_version_info("2.8.0"),
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.2"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let common = find_common_major_minor(&npm_versions, &crate_versions);
        // 最高の共通バージョンは 2.10
        assert_eq!(common, Some((2, 10)));
    }

    #[test]
    fn test_find_common_major_minor_no_overlap() {
        let npm_versions = vec![make_version_info("2.8.0"), make_version_info("2.9.0")];
        let crate_versions = vec![make_version_info("2.10.0"), make_version_info("2.11.0")];

        let common = find_common_major_minor(&npm_versions, &crate_versions);
        assert_eq!(common, None);
    }

    #[test]
    fn test_find_matching_version() {
        let versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let matching = find_matching_version((2, 10), &versions);
        assert!(matching.is_some());
        assert_eq!(matching.unwrap().version, "2.10.1"); // 最新のパッチ
    }

    #[test]
    fn test_is_tauri_package() {
        assert!(TauriVersionSync::is_tauri_package(
            "@tauri-apps/api",
            Language::Node
        ));
        assert!(TauriVersionSync::is_tauri_package(
            "@tauri-apps/cli",
            Language::Node
        ));
        assert!(TauriVersionSync::is_tauri_package("tauri", Language::Rust));
        assert!(!TauriVersionSync::is_tauri_package("tauri", Language::Node));
        assert!(!TauriVersionSync::is_tauri_package(
            "@tauri-apps/api",
            Language::Rust
        ));
        assert!(!TauriVersionSync::is_tauri_package(
            "lodash",
            Language::Node
        ));
    }

    #[test]
    fn test_is_tauri_npm_package() {
        assert!(TauriVersionSync::is_tauri_npm_package("@tauri-apps/api"));
        assert!(TauriVersionSync::is_tauri_npm_package("@tauri-apps/cli"));
        assert!(!TauriVersionSync::is_tauri_npm_package("tauri"));
        assert!(!TauriVersionSync::is_tauri_npm_package("lodash"));
    }

    #[test]
    fn test_synchronize_crate_only_updating() {
        // npm は 2.9.1、クレートは 2.10.1 に更新中
        // npm も 2.10.x に更新して一致させる必要がある
        let npm_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.5"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let crate_dep = make_dependency(TAURI_CRATE, "2.9.5", Language::Rust);
        let crate_result = UpdateResult::update(crate_dep, "2.10.1");

        let (npm_adj, crate_adj) = sync.synchronize_with_current(
            Some("2.9.1"), // npm の現在のバージョン
            None,          // npm は更新されていない
            Some("2.9.5"), // クレートの現在のバージョン
            Some(&crate_result),
        );

        // npm はクレートに合わせて 2.10.1 に更新されるべき
        assert_eq!(npm_adj, Some("2.10.1".to_string()));
        // クレートは既に 2.10.1 に向かっているので調整不要
        assert_eq!(crate_adj, None);
    }

    #[test]
    fn test_synchronize_both_updating_mismatch() {
        let npm_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.0"),
            make_version_info("2.9.2"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let npm_dep = make_dependency("@tauri-apps/api", "2.8.0", Language::Node);
        let crate_dep = make_dependency(TAURI_CRATE, "2.8.0", Language::Rust);

        let npm_result = UpdateResult::update(npm_dep, "2.9.1");
        let crate_result = UpdateResult::update(crate_dep, "2.10.1");

        let (npm_adj, crate_adj) = sync.synchronize_with_current(
            Some("2.8.0"),
            Some(&npm_result),
            Some("2.8.0"),
            Some(&crate_result),
        );

        // 最高の共通バージョン (2.10) に調整されるべき
        assert_eq!(npm_adj, Some("2.10.0".to_string()));
        assert_eq!(crate_adj, None); // クレートは既に 2.10.1
    }

    #[test]
    fn test_synchronize_already_matching() {
        let npm_versions = vec![make_version_info("2.10.0")];
        let crate_versions = vec![make_version_info("2.10.1")];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let npm_dep = make_dependency("@tauri-apps/api", "2.9.0", Language::Node);
        let crate_dep = make_dependency(TAURI_CRATE, "2.9.0", Language::Rust);

        let npm_result = UpdateResult::update(npm_dep, "2.10.0");
        let crate_result = UpdateResult::update(crate_dep, "2.10.1");

        let (npm_adj, crate_adj) = sync.synchronize_with_current(
            Some("2.9.0"),
            Some(&npm_result),
            Some("2.9.0"),
            Some(&crate_result),
        );

        // メジャー.マイナーが既に一致しているため調整不要
        assert_eq!(npm_adj, None);
        assert_eq!(crate_adj, None);
    }

    #[test]
    fn test_find_matching_version_no_match() {
        let versions = vec![make_version_info("2.9.0"), make_version_info("2.9.1")];
        // 2.10.x のバージョンがない
        let result = find_matching_version((2, 10), &versions);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_matching_version_skips_prerelease() {
        let versions = vec![
            make_version_info("2.10.0-beta.1"),
            make_version_info("2.10.0-alpha.2"),
        ];
        // プレリリースバージョンのみの場合、マッチしないはず
        let result = find_matching_version((2, 10), &versions);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_major_minor_single_part() {
        // パーツが1つだけでは不十分
        assert_eq!(extract_major_minor("1"), None);
        assert_eq!(extract_major_minor(""), None);
    }

    #[test]
    fn test_find_common_major_minor_empty_lists() {
        let npm_versions: Vec<VersionInfo> = vec![];
        let crate_versions: Vec<VersionInfo> = vec![];
        let common = find_common_major_minor(&npm_versions, &crate_versions);
        assert_eq!(common, None);
    }

    #[test]
    fn test_get_synchronized_versions() {
        let npm_versions = vec![
            make_version_info("2.9.1"),
            make_version_info("2.10.0"),
            make_version_info("2.10.1"),
        ];
        let crate_versions = vec![
            make_version_info("2.9.5"),
            make_version_info("2.10.1"),
            make_version_info("2.10.2"),
        ];

        let sync = TauriVersionSync::new(npm_versions, crate_versions);

        let result = sync.get_synchronized_versions("2.10.1");
        assert!(result.is_some());
        let (npm, crate_v) = result.unwrap();
        assert_eq!(npm, "2.10.1");
        assert_eq!(crate_v, "2.10.2"); // 2.10 内の最新パッチ
    }
}
