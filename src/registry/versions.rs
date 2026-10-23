//! 実行全体で共有するバージョン取得、キャッシュ、並行数制御。

use super::RegistryAdapter;
use crate::domain::Language;
use crate::update::VersionInfo;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// バージョン情報のキャッシュ (言語, パッケージ名) をキーとする。
pub type VersionCache = Arc<Mutex<HashMap<(Language, String), Vec<VersionInfo>>>>;
type VersionFetchLocks = Arc<Mutex<HashMap<(Language, String), Arc<Mutex<()>>>>>;
/// (言語, パッケージのキャッシュキー, 版) → 公開日 (公開日が無いと分かった版は `None`)
type ReleaseDates = Mutex<HashMap<(Language, String, String), Option<DateTime<Utc>>>>;

/// 通常チェック・同期・lock 監査で同じ取得状態を共有する。
pub(crate) struct VersionFetcher {
    pub(crate) cache: VersionCache,
    fetch_locks: VersionFetchLocks,
    /// 解決した公開日。レジストリが公開日を持たないと答えた版 (`None`) も覚え、問い合わせ直さない
    release_dates: ReleaseDates,
    general_semaphore: Semaphore,
    crates_io_semaphore: Semaphore,
}

impl VersionFetcher {
    fn cache_key(
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> (Language, String) {
        let scope = adapter.cache_scope();
        if scope.is_empty() {
            (adapter.language(), package.to_string())
        } else {
            (adapter.language(), format!("{scope}\0{package}"))
        }
    }

    pub(crate) fn new(general_concurrency: usize, crates_io_concurrency: usize) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            fetch_locks: Arc::new(Mutex::new(HashMap::new())),
            release_dates: Mutex::new(HashMap::new()),
            general_semaphore: Semaphore::new(general_concurrency),
            crates_io_semaphore: Semaphore::new(crates_io_concurrency),
        }
    }

    /// 同時実行制御とキャッシュ付きでレジストリからバージョンを取得する
    pub(crate) async fn fetch(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        // mise は現在ディレクトリと native age により一覧自体が変わる。
        // その一覧を別の age スコープへ再利用しない (mise 自身のリモートキャッシュは使える)。
        if adapter.language() == Language::Mise {
            let _permit = self.general_semaphore.acquire().await.unwrap();
            return adapter
                .fetch_versions(package)
                .await
                .map_err(|error| error.to_string());
        }
        let cache_key = Self::cache_key(adapter, package);

        // まずキャッシュを確認
        {
            let cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // 同一キーの取得だけを直列化する。異なるパッケージの並列性は維持する。
        let fetch_lock = {
            let mut locks = self.fetch_locks.lock().await;
            Arc::clone(
                locks
                    .entry(cache_key.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _fetch_guard = fetch_lock.lock().await;

        // キー単位ロックの待機中に先行取得が完了している場合はキャッシュを返す。
        {
            let cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // レジストリに応じて適切なセマフォを使用
        let semaphore = if adapter.language() == Language::Rust {
            &self.crates_io_semaphore
        } else {
            &self.general_semaphore
        };

        let _permit = semaphore.acquire().await.unwrap();

        let result = adapter
            .fetch_versions(package)
            .await
            .map_err(|e| e.to_string())?;

        // キャッシュに保存
        {
            let mut cache = self.cache.lock().await;
            cache.insert(cache_key, result.clone());
        }

        Ok(result)
    }

    /// この実行で既に解決した公開日。まだ問い合わせていなければ `None`、
    /// 公開日が無いと分かっていれば `Some(None)`。
    pub(crate) async fn cached_release_date(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
        version: &str,
    ) -> Option<Option<DateTime<Utc>>> {
        let package_key = Self::cache_key(adapter, package);
        let key = (package_key.0, package_key.1, version.to_string());
        self.release_dates.lock().await.get(&key).copied()
    }

    /// 同じ実行内のマニフェスト間で、解決済みの公開日を共有する。
    pub(crate) async fn release_date(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
        version: &str,
    ) -> Result<Option<DateTime<Utc>>, String> {
        let package_key = Self::cache_key(adapter, package);
        let key = (package_key.0, package_key.1.clone(), version.to_string());
        if let Some(date) = self.release_dates.lock().await.get(&key) {
            return Ok(*date);
        }
        let fetch_lock = {
            let mut locks = self.fetch_locks.lock().await;
            Arc::clone(
                locks
                    .entry(package_key)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = fetch_lock.lock().await;
        if let Some(date) = self.release_dates.lock().await.get(&key) {
            return Ok(*date);
        }
        let date = adapter
            .fetch_release_date(package, version)
            .await
            .map_err(|error| error.to_string())?;
        self.release_dates.lock().await.insert(key, date);
        Ok(date)
    }

    /// install がキャッシュにない版を選んだ場合に、共有レート制限付きで一覧を更新する。
    pub(crate) async fn refresh(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Result<Vec<VersionInfo>, String> {
        let _permit = self
            .crates_io_semaphore
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        let versions = adapter
            .fetch_versions(package)
            .await
            .map_err(|error| error.to_string())?;
        self.cache
            .lock()
            .await
            .insert(Self::cache_key(adapter, package), versions.clone());
        Ok(versions)
    }

    /// 監査中に取得済みの版一覧 (レジストリへは問い合わせない)
    pub(crate) async fn cached(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Option<Vec<VersionInfo>> {
        let cache = self.cache.lock().await;
        cache.get(&Self::cache_key(adapter, package)).cloned()
    }
}
