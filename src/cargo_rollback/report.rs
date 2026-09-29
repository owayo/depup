//! install 後の Cargo.lock と、画面に出した更新先の版の突き合わせ。
//!
//! depup が表示する更新先は Cargo.toml に書いた版だが、`--install` の `cargo update` は
//! その版要求 (`^0.2.128`) を満たす最新版 (`0.2.129`) を lock に入れる。差し戻しで
//! 揃えられなかった場合や `--max-change` / OSV で古い版を選んだ場合は、表示とビルドに
//! 使われる版が食い違うので、その差を利用者に見せるために使う。

use super::series::same_series;
use crate::manifest::RegistryLockEntries;
use std::cmp::Ordering;

/// 画面に出した 1 件の更新
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayedUpdate {
    /// crates.io 上の crate 名 (リネーム依存でも実パッケージ名)
    pub name: String,
    /// 表示した更新前の版
    pub from: String,
    /// 表示した更新先の版
    pub to: String,
}

/// 更新先の版と Cargo.lock に入った版が食い違った 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedVersionMismatch {
    pub name: String,
    pub from: String,
    pub to: String,
    /// Cargo.lock に入った版
    pub locked: String,
}

/// 更新先と同じ semver 系列で Cargo.lock に入っている版を探し、更新先と違えば返す。
///
/// 同名の crate が複数版 lock されていても (`syn 1.x` と `syn 2.x`)、系列で対応付けるので
/// 取り違えない。系列に該当する版が lock に無いものは突き合わせられないので含めない。
/// 比較は build metadata (`+...`) を無視する。
pub fn lock_mismatches(
    updates: &[DisplayedUpdate],
    entries: &RegistryLockEntries,
) -> Vec<LockedVersionMismatch> {
    updates
        .iter()
        .filter_map(|update| {
            let locked = entries
                .get(&update.name)?
                .iter()
                .find(|version| same_series(version, &update.to))?;
            let differs = match (
                semver::Version::parse(locked),
                semver::Version::parse(&update.to),
            ) {
                (Ok(locked), Ok(to)) => locked.cmp_precedence(&to) != Ordering::Equal,
                _ => locked != &update.to,
            };
            differs.then(|| LockedVersionMismatch {
                name: update.name.clone(),
                from: update.from.clone(),
                to: update.to.clone(),
                locked: locked.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(name: &str, from: &str, to: &str) -> DisplayedUpdate {
        DisplayedUpdate {
            name: name.to_string(),
            from: from.to_string(),
            to: to.to_string(),
        }
    }

    fn entries(pairs: &[(&str, &[&str])]) -> RegistryLockEntries {
        pairs
            .iter()
            .map(|(name, versions)| {
                (
                    (*name).to_string(),
                    versions.iter().map(|v| (*v).to_string()).collect(),
                )
            })
            .collect()
    }

    /// 表示した更新先より新しい版が lock に入ったら注記の対象
    #[test]
    fn test_lock_mismatches_reports_newer_locked_version() {
        let mismatches = lock_mismatches(
            &[update("wasm-bindgen", "0.2.127", "0.2.128")],
            &entries(&[("wasm-bindgen", &["0.2.129"])]),
        );
        assert_eq!(
            mismatches,
            vec![LockedVersionMismatch {
                name: "wasm-bindgen".to_string(),
                from: "0.2.127".to_string(),
                to: "0.2.128".to_string(),
                locked: "0.2.129".to_string(),
            }]
        );
    }

    /// 一致していれば何も出さない
    #[test]
    fn test_lock_mismatches_ignores_matching_version() {
        let mismatches = lock_mismatches(
            &[update("serde", "1.0.200", "1.0.210")],
            &entries(&[("serde", &["1.0.210"])]),
        );
        assert!(mismatches.is_empty());
    }

    /// 同名の別系列の版とは突き合わせない
    #[test]
    fn test_lock_mismatches_matches_by_series() {
        let mismatches = lock_mismatches(
            &[update("syn", "2.0.80", "2.0.90")],
            &entries(&[("syn", &["1.0.109", "2.0.90"])]),
        );
        assert!(mismatches.is_empty());

        let mismatches = lock_mismatches(
            &[update("syn", "2.0.80", "2.0.90")],
            &entries(&[("syn", &["1.0.109", "2.0.95"])]),
        );
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].locked, "2.0.95");
    }

    /// lock に無い crate (その系列の版が無い) は突き合わせられないので含めない
    #[test]
    fn test_lock_mismatches_skips_crates_missing_from_lock() {
        let mismatches = lock_mismatches(
            &[update("tokio", "1.40.0", "1.41.0")],
            &entries(&[("tokio", &["0.2.25"])]),
        );
        assert!(mismatches.is_empty());
    }

    /// build metadata の差だけでは食い違いにしない
    #[test]
    fn test_lock_mismatches_ignores_build_metadata() {
        let mismatches = lock_mismatches(
            &[update("wasi", "0.11.0", "0.11.1+wasi-snapshot-preview1")],
            &entries(&[("wasi", &["0.11.1"])]),
        );
        assert!(mismatches.is_empty());
    }
}
