//! 公開日を固定した crates.io の代わり
//!
//! `--age` の判定は「公開から何日経ったか」で決まるため、本物の crates.io を使うと
//! 結果が実行日によって変わる。版ごとの公開日をテストの中で決めて返すことで、
//! 実行日にもネットワークにも依存せずに age 監査の分岐を確かめられるようにする。

use crate::domain::Language;
use crate::error::RegistryError;
use crate::registry::RegistryAdapter;
use crate::update::VersionInfo;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 公開日を固定した crates.io の代わり (`RegistryAdapter` を実装する)
pub(crate) struct FakeCratesIo {
    /// crate 名 → 登録された版 (登録順のまま持ち、返すときに並べる)
    releases: HashMap<String, Vec<VersionInfo>>,
    yanked: HashSet<(String, String)>,
    /// 版情報を照会した回数 (未登録の名前で失敗した呼び出しも数える)
    fetch_count: AtomicUsize,
}

impl FakeCratesIo {
    pub fn publisher(
        mut self,
        name: &str,
        version: &str,
        publisher: crate::update::PublisherEvidence,
    ) -> Self {
        self.releases
            .get_mut(name)
            .unwrap()
            .iter_mut()
            .find(|info| info.version == version)
            .unwrap()
            .publisher = publisher;
        self
    }

    /// 版を 1 つも持たない偽のレジストリを作る
    pub fn new() -> Self {
        Self {
            releases: HashMap::new(),
            yanked: HashSet::new(),
            fetch_count: AtomicUsize::new(0),
        }
    }

    /// 版と公開日を登録する (ビルダー)
    ///
    /// crates.io では同じ版を 2 度公開できないため、同じ版の二重登録はテストの
    /// 書き誤りとして panic する (黙って重複させると返す一覧に同じ版が 2 つ並ぶ)。
    pub fn release(mut self, name: &str, version: &str, released_at: DateTime<Utc>) -> Self {
        let versions = self.releases.entry(name.to_string()).or_default();
        assert!(
            versions.iter().all(|v| v.version != version),
            "{name} {version} を 2 回登録している"
        );
        versions.push(VersionInfo::new(version, released_at));
        self
    }

    /// 版情報を照会した回数
    pub fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }

    pub fn yank(mut self, name: &str, version: &str) -> Self {
        assert!(
            self.releases
                .get(name)
                .is_some_and(|versions| { versions.iter().any(|info| info.version == version) })
        );
        self.yanked.insert((name.into(), version.into()));
        self
    }

    fn all_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        let mut versions = self
            .releases
            .get(package)
            .cloned()
            .ok_or_else(|| RegistryError::package_not_found(package, self.registry_name()))?;
        versions.sort();
        Ok(versions)
    }
}

#[async_trait]
impl RegistryAdapter for FakeCratesIo {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn registry_name(&self) -> &'static str {
        "fake-crates-io"
    }

    async fn fetch_versions(&self, package: &str) -> Result<Vec<VersionInfo>, RegistryError> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let mut versions = self.all_versions(package)?;
        versions.retain(|info| {
            !self
                .yanked
                .contains(&(package.into(), info.version.clone()))
        });
        Ok(versions)
    }

    async fn fetch_locked_versions(
        &self,
        package: &str,
        _locked: &[String],
        _publisher_cutoff: Option<DateTime<Utc>>,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        self.all_versions(package)
    }

    async fn fetch_yanked_versions(
        &self,
        package: &str,
    ) -> Result<Vec<VersionInfo>, RegistryError> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let mut versions = self.all_versions(package)?;
        versions.retain(|info| {
            self.yanked
                .contains(&(package.into(), info.version.clone()))
        });
        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn test_adapter_identity() {
        let fake = FakeCratesIo::new();
        assert_eq!(fake.language(), Language::Rust);
        assert_eq!(fake.registry_name(), "fake-crates-io");
    }

    /// 登録した順ではなく版の昇順で返り、公開日は版ごとに保たれる。
    /// 文字列順だと 1.0.10 が 1.0.2 より前に来るので、数値として並ぶことも確かめる
    #[tokio::test]
    async fn test_fetch_versions_returns_ascending_regardless_of_release_order() {
        let fake = FakeCratesIo::new()
            .release("solo", "1.0.10", day(3))
            .release("solo", "0.9.0", day(1))
            .release("solo", "1.0.2", day(2));

        let versions = fake.fetch_versions("solo").await.unwrap();

        let pairs: Vec<(&str, Option<DateTime<Utc>>)> = versions
            .iter()
            .map(|v| (v.version.as_str(), v.released_at))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("0.9.0", Some(day(1))),
                ("1.0.2", Some(day(2))),
                ("1.0.10", Some(day(3)))
            ]
        );
    }

    #[tokio::test]
    async fn test_fetch_versions_unknown_package_is_not_found() {
        let fake = FakeCratesIo::new().release("solo", "1.0.0", day(1));

        let err = fake.fetch_versions("missing").await.unwrap_err();

        match err {
            RegistryError::PackageNotFound { package, registry } => {
                assert_eq!(package, "missing");
                assert_eq!(registry, "fake-crates-io");
            }
            other => panic!("PackageNotFound 以外が返った: {other:?}"),
        }
    }

    /// 成功・失敗に関係なく呼び出しのたびに 1 増える
    #[tokio::test]
    async fn test_fetch_count_counts_every_call() {
        let fake = FakeCratesIo::new().release("solo", "1.0.0", day(1));
        assert_eq!(fake.fetch_count(), 0);

        fake.fetch_versions("solo").await.unwrap();
        assert_eq!(fake.fetch_count(), 1);

        fake.fetch_versions("missing").await.unwrap_err();
        assert_eq!(fake.fetch_count(), 2);
    }

    #[test]
    #[should_panic(expected = "solo 1.0.0 を 2 回登録している")]
    fn test_release_rejects_duplicate_version() {
        let _ = FakeCratesIo::new()
            .release("solo", "1.0.0", day(1))
            .release("solo", "1.0.0", day(2));
    }
}
