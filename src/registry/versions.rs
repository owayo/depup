//! 実行全体で共有するバージョン取得、キャッシュ、並行数制御。

use super::RegistryAdapter;
use crate::domain::Language;
use crate::update::VersionInfo;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// バージョン情報のキャッシュ (言語, パッケージ名) をキーとする。
pub type VersionCache = Arc<Mutex<HashMap<(Language, String), Vec<VersionInfo>>>>;
type VersionFetchLocks = Arc<Mutex<HashMap<(Language, String), Arc<Mutex<()>>>>>;

/// 通常チェック・同期・lock 監査で同じ取得状態を共有する。
pub(crate) struct VersionFetcher {
    pub(crate) cache: VersionCache,
    fetch_locks: VersionFetchLocks,
    general_semaphore: Semaphore,
    crates_io_semaphore: Semaphore,
}

impl VersionFetcher {
    pub(crate) fn new(general_concurrency: usize, crates_io_concurrency: usize) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            fetch_locks: Arc::new(Mutex::new(HashMap::new())),
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
        let cache_key = (adapter.language(), package.to_string());

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
            .insert((adapter.language(), package.to_string()), versions.clone());
        Ok(versions)
    }

    /// 監査中に取得済みの版一覧 (レジストリへは問い合わせない)
    pub(crate) async fn cached(
        &self,
        adapter: &(dyn RegistryAdapter + Send + Sync),
        package: &str,
    ) -> Option<Vec<VersionInfo>> {
        let cache = self.cache.lock().await;
        cache
            .get(&(adapter.language(), package.to_string()))
            .cloned()
    }
}
