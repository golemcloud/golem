// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashSet;
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;

use crate::metrics::caching::{
    record_cache_capacity, record_cache_eviction, record_cache_hit, record_cache_miss,
    record_cache_size,
};

/// Cache supporting concurrent access including ensuring that the async function
/// computing the cached value is only executed once for each key if multiple fibers are requesting it.
///
/// Cached elements that get evicted are immediately dropped.
///
/// An intermediate pending value of type PV can be returned while the async function is running.
///
/// Eviction happens in two ways:
/// - when the cache is full and a new element is added, at least one element is evicted (the least recently used ones)
/// - optionally a periodic background task evicts some elements, either the N oldest one or all the items older than a given duration
#[derive(Clone)]
pub struct Cache<K, PV, V, E> {
    state: Arc<CacheState<K, PV, V, E>>,
    capacity: Option<usize>,
    full_cache_eviction: FullCacheEvictionMode,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    name: &'static str,
}

pub trait SimpleCache<K, V, E> {
    fn get_or_insert_simple<F>(&self, key: &K, f: F) -> impl Future<Output = Result<V, E>>
    where
        F: AsyncFnOnce() -> Result<V, E>;
}

struct CacheState<K, PV, V, E> {
    items: scc::HashMap<K, Item<V, PV, E>>,
    last_id: std::sync::atomic::AtomicU64,
    count: std::sync::atomic::AtomicUsize,
}

impl<
        K: Eq + Hash + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        E: Clone + Send + Sync + 'static,
    > SimpleCache<K, V, E> for Cache<K, (), V, E>
{
    /// Gets a cached value for a given key, or inserts a new one with the given async function. If a value is pending,
    /// it is awaited instead of recreating it.
    async fn get_or_insert_simple<F>(&self, key: &K, f: F) -> Result<V, E>
    where
        F: AsyncFnOnce() -> Result<V, E>,
    {
        self.get_or_insert(key, || (), async |_| f().await).await
    }
}

impl<
        K: Eq + Hash + Clone + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        PV: Clone + Send + Sync + 'static,
        E: Clone + Send + Sync + 'static,
    > Cache<K, PV, V, E>
{
    pub fn new(
        capacity: Option<usize>,
        full_cache_eviction: FullCacheEvictionMode,
        background_eviction: BackgroundEvictionMode,
        name: &'static str,
    ) -> Self {
        match full_cache_eviction {
            FullCacheEvictionMode::LeastRecentlyUsed(count) => {
                assert!(count >= 1);
            }
            FullCacheEvictionMode::None => {}
        }

        let state = Arc::new(CacheState {
            items: match capacity {
                Some(capacity) => scc::HashMap::with_capacity(capacity),
                None => scc::HashMap::new(),
            },
            last_id: std::sync::atomic::AtomicU64::new(0),
            count: std::sync::atomic::AtomicUsize::new(0),
        });
        let cache = Self {
            state,
            capacity,
            full_cache_eviction,
            background_handle: Arc::new(Mutex::new(None)),
            name,
        };

        if let Some(capacity) = capacity {
            record_cache_capacity(name, capacity);
        }
        record_cache_size(name, 0);

        let background_handle = match background_eviction {
            BackgroundEvictionMode::LeastRecentlyUsed { count, period } => {
                assert!(count >= 1);
                let cache_clone = cache.clone();
                let eviction = background_eviction;
                Some(tokio::task::spawn(async move {
                    loop {
                        tokio::time::sleep(period).await;
                        cache_clone.background_evict(&eviction).await;
                    }
                }))
            }
            BackgroundEvictionMode::OlderThan { period, .. } => {
                let cache_clone = cache.clone();
                let eviction = background_eviction;
                Some(tokio::task::spawn(async move {
                    loop {
                        tokio::time::sleep(period).await;
                        cache_clone.background_evict(&eviction).await;
                    }
                }))
            }
            BackgroundEvictionMode::None => None,
        };
        *cache.background_handle.lock().unwrap() = background_handle;

        cache
    }

    /// Tries to get a cached value for the given key. If the value is missing or is pending, it returns None.
    pub async fn try_get(&self, key: &K) -> Option<V> {
        let result = self
            .state
            .items
            .read_async(key, |_, item| match item {
                Item::Pending { .. } => None,
                Item::Cached { value, .. } => Some(value.clone()),
            })
            .await
            .flatten();

        if result.is_some() {
            self.update_last_access(key).await;
        }
        result
    }

    /// Gets a cached value for the given key. If the value is pending, it awaits it.
    /// If the pending value fails, it returns None.
    pub async fn get(&self, key: &K) -> Option<V> {
        let entry = self
            .state
            .items
            .read_async(key, |_, item| match item {
                Item::Pending { tx, .. } => Err(tx.subscribe()),
                Item::Cached { value, .. } => Ok(value.clone()),
            })
            .await;

        let result = match entry {
            Some(Ok(value)) => Some(value),
            Some(Err(mut rx)) => rx
                .wait_for(|v| v.is_some())
                .await
                .ok()
                .and_then(|val| val.clone())
                .and_then(|r| r.ok()),
            None => None,
        };

        if result.is_some() {
            self.update_last_access(key).await;
        }

        result
    }

    /// Gets a cached value for a given key, or inserts a new one with the given async function. If a value is pending,
    /// it is awaited instead of recreating it.
    pub async fn get_or_insert<F1, F2>(&self, key: &K, f1: F1, f2: F2) -> Result<V, E>
    where
        F1: FnOnce() -> PV,
        F2: AsyncFnOnce(&PV) -> Result<V, E>,
    {
        let mut eviction_needed = false;
        let result = {
            let own_id = self.state.last_id.fetch_add(1, Ordering::SeqCst);
            let result = self.get_or_add_as_pending(key, own_id, f1).await?;
            match result {
                Item::Pending {
                    ref tx,
                    id,
                    pending_value,
                } => {
                    if id == own_id {
                        record_cache_miss(self.name);

                        let value = f2(&pending_value).await;
                        if let Ok(success_value) = &value {
                            self.state
                                .items
                                .upsert_async(
                                    key.clone(),
                                    Item::Cached {
                                        value: success_value.clone(),
                                        last_access: Instant::now(),
                                    },
                                )
                                .await;
                            let old_count = self.state.count.fetch_add(1, Ordering::SeqCst);

                            record_cache_size(self.name, old_count.saturating_add(1));

                            if Some(old_count) == self.capacity {
                                eviction_needed = true;
                            }
                        } else {
                            self.state.items.remove_async(key).await;
                        }
                        let _ = tx.send(Some(value.clone()));

                        value
                    } else {
                        record_cache_hit(self.name);

                        let mut rx = tx.subscribe();
                        let val = rx
                            .wait_for(|v| v.is_some())
                            .await
                            .expect("cache watch sender dropped without sending");
                        val.clone().unwrap()
                    }
                }
                Item::Cached { value, .. } => {
                    record_cache_hit(self.name);

                    self.update_last_access(key).await;
                    Ok(value)
                }
            }
        };

        if eviction_needed {
            self.evict().await;
        }

        result
    }

    /// Gets a cached value for a given key, or inserts a new one with the given async function but immediately
    /// returns the pending value. If a value is pending, it's pending value is returned immediately.
    pub async fn get_or_insert_pending<F1, F2>(
        &self,
        key: &K,
        f1: F1,
        f2: F2,
    ) -> Result<PendingOrFinal<PV, V>, E>
    where
        F1: FnOnce() -> PV,
        F2: FnOnce(&PV) -> Pin<Box<dyn Future<Output = Result<V, E>> + Send>> + Send + 'static,
    {
        {
            let own_id = self.state.last_id.fetch_add(1, Ordering::SeqCst);
            let result = self.get_or_add_as_pending(key, own_id, f1).await?;
            match result {
                Item::Pending {
                    ref tx,
                    id,
                    pending_value,
                } => {
                    if id == own_id {
                        record_cache_miss(self.name);

                        let key_clone = key.clone();
                        let tx_clone = tx.clone();
                        let pending_value_clone = pending_value.clone();
                        let self_clone = self.clone();

                        tokio::task::spawn(
                            async move {
                                let value = f2(&pending_value_clone).await;
                                if let Ok(success_value) = &value {
                                    self_clone
                                        .state
                                        .items
                                        .upsert_async(
                                            key_clone.clone(),
                                            Item::Cached {
                                                value: success_value.clone(),
                                                last_access: Instant::now(),
                                            },
                                        )
                                        .await;
                                    let old_count =
                                        self_clone.state.count.fetch_add(1, Ordering::SeqCst);

                                    record_cache_size(self_clone.name, old_count.saturating_add(1));

                                    if Some(old_count) == self_clone.capacity {
                                        self_clone.evict().await;
                                    }
                                } else {
                                    self_clone.state.items.remove_async(&key_clone).await;
                                }
                                let _ = tx_clone.send(Some(value.clone()));
                            }
                            .in_current_span(),
                        );
                    }

                    Ok(PendingOrFinal::Pending(pending_value))
                }
                Item::Cached { value, .. } => {
                    record_cache_hit(self.name);

                    self.update_last_access(key).await;
                    Ok(PendingOrFinal::Final(value))
                }
            }
        }
    }

    pub async fn iter(&self) -> Vec<(K, V)> {
        let mut snapshotted_pairs = vec![];
        self.state
            .items
            .iter_async(|key, value| {
                match value {
                    Item::Cached { value, .. } => {
                        snapshotted_pairs.push((key.clone(), value.clone()));
                    }
                    Item::Pending { .. } => {}
                }
                true
            })
            .await;

        snapshotted_pairs
    }

    pub async fn keys(&self) -> Vec<K> {
        let mut keys = vec![];
        self.state
            .items
            .iter_async(|key, _| {
                keys.push(key.clone());
                true
            })
            .await;
        keys
    }

    pub async fn remove(&self, key: &K) {
        let removed = self.state.items.remove_async(key).await.is_some();
        if removed {
            let count = self.state.count.fetch_sub(1, Ordering::SeqCst);
            record_cache_size(self.name, count.saturating_sub(1));
        }
    }

    pub async fn contains_key(&self, key: &K) -> bool {
        self.state.items.contains_async(key).await
    }

    pub fn create_weak_remover(&self, key: K) -> impl FnOnce() {
        let weak_state = Arc::downgrade(&self.state);
        let name = self.name;
        move || {
            if let Some(state) = weak_state.upgrade() {
                let removed = state.items.remove_sync(&key).is_some();
                if removed {
                    let count = state.count.fetch_sub(1, Ordering::SeqCst);
                    record_cache_size(name, count.saturating_sub(1));
                }
            }
        }
    }

    async fn evict(&self) {
        record_cache_eviction(self.name, "full");
        match self.full_cache_eviction {
            FullCacheEvictionMode::None => {}
            FullCacheEvictionMode::LeastRecentlyUsed(count) => {
                self.evict_least_recently_used(count).await;
            }
        }
    }

    async fn background_evict(&self, mode: &BackgroundEvictionMode) {
        record_cache_eviction(self.name, "background");
        match mode {
            BackgroundEvictionMode::None => {}
            BackgroundEvictionMode::LeastRecentlyUsed { count, .. } => {
                self.evict_least_recently_used(*count).await
            }
            BackgroundEvictionMode::OlderThan { ttl, .. } => self.evict_older_than(*ttl).await,
        }
    }

    async fn evict_least_recently_used(&self, count: usize) {
        let mut keys_to_keep = vec![];
        self.state
            .items
            .iter_async(|key, value| {
                if let Item::Cached { last_access, .. } = value {
                    keys_to_keep.push((key.clone(), last_access.elapsed().as_millis()))
                }
                true
            })
            .await;

        keys_to_keep.sort_by_key(|(_, v)| *v);
        keys_to_keep.truncate(keys_to_keep.len().saturating_sub(count));
        let keys_to_keep: HashSet<&K> = keys_to_keep.iter().map(|(k, _)| k).collect();

        self.state
            .items
            .retain_async(|k, v| match v {
                Item::Cached { .. } => keys_to_keep.contains(k),
                Item::Pending { .. } => true,
            })
            .await;
        self.state.count.store(keys_to_keep.len(), Ordering::SeqCst);
        record_cache_size(self.name, keys_to_keep.len());
    }

    async fn evict_older_than(&self, ttl: Duration) {
        self.state
            .items
            .retain_async(|_, item| match item {
                Item::Cached { last_access, .. } => last_access.elapsed() < ttl,
                Item::Pending { .. } => true,
            })
            .await;
        let count = self.state.items.len();
        self.state.count.store(count, Ordering::SeqCst);
        record_cache_size(self.name, count);
    }

    async fn update_last_access(&self, key: &K) {
        self.state
            .items
            .update_async(key, |_, item| {
                if let Item::Cached { last_access, .. } = item {
                    *last_access = Instant::now()
                }
            })
            .await;
    }

    async fn get_or_add_as_pending<F>(
        &self,
        key: &K,
        own_id: u64,
        f: F,
    ) -> Result<Item<V, PV, E>, E>
    where
        F: FnOnce() -> PV,
    {
        Ok(self
            .state
            .items
            .entry_async(key.clone())
            .await
            .or_insert_with(|| {
                let pending_value = f();
                let (tx, _) = tokio::sync::watch::channel(None);
                Item::Pending {
                    tx: Arc::new(tx),
                    id: own_id,
                    pending_value,
                }
            })
            .get()
            .clone())
    }
}

impl<K, V, PV, E> Drop for Cache<K, V, PV, E> {
    fn drop(&mut self) {
        if let Some(handle) = self.background_handle.lock().unwrap().take() {
            handle.abort();
        }
    }
}

#[derive(Clone)]
enum Item<V, PV, E> {
    Pending {
        tx: Arc<tokio::sync::watch::Sender<Option<Result<V, E>>>>,
        id: u64,
        pending_value: PV,
    },
    Cached {
        value: V,
        last_access: Instant,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FullCacheEvictionMode {
    None,
    LeastRecentlyUsed(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(unused)]
pub enum BackgroundEvictionMode {
    None,
    LeastRecentlyUsed { count: usize, period: Duration },
    OlderThan { ttl: Duration, period: Duration },
}

pub enum PendingOrFinal<PV, V> {
    Pending(PV),
    Final(V),
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::join_all;
    use std::sync::atomic::AtomicU64;
    use test_r::test;

    fn test_cache(name: &'static str) -> Cache<u64, (), u64, String> {
        Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            name,
        )
    }

    fn bounded_cache(capacity: usize, name: &'static str) -> Cache<u64, (), u64, String> {
        Cache::new(
            Some(capacity),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::None,
            name,
        )
    }

    // ---- Race condition proof tests ----

    #[test]
    async fn broadcast_late_subscribe_misses_message() {
        let (tx, _rx) = tokio::sync::broadcast::channel::<Result<u64, String>>(1);
        let tx_clone = tx.clone();

        let _ = tx.send(Ok(42));

        let mut rx = tx_clone.subscribe();
        let result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err(),
            "broadcast late subscriber misses the message (proving the original bug)"
        );
    }

    #[test]
    async fn watch_late_subscribe_sees_message() {
        let (tx, _rx) = tokio::sync::watch::channel::<Option<Result<u64, String>>>(None);

        let _ = tx.send(Some(Ok(42)));

        let mut rx = tx.subscribe();
        let result =
            tokio::time::timeout(Duration::from_millis(200), rx.wait_for(|v| v.is_some())).await;
        assert!(result.is_ok(), "watch late subscriber sees the message");
        let val = result.unwrap().unwrap().clone().unwrap();
        assert_eq!(val, Ok(42));
    }

    // ---- Basic operations ----

    #[test]
    async fn get_or_insert_simple_inserts_and_returns_value() {
        let cache = test_cache("basic_insert");
        let result = cache.get_or_insert_simple(&1, || async { Ok(42u64) }).await;
        assert_eq!(result, Ok(42));
    }

    #[test]
    async fn get_or_insert_simple_returns_cached_value_on_second_call() {
        let cache = test_cache("cached_second");
        let call_count = Arc::new(AtomicU64::new(0));

        let cc = call_count.clone();
        let r1 = cache
            .get_or_insert_simple(&1, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(42u64)
            })
            .await;

        let cc = call_count.clone();
        let r2 = cache
            .get_or_insert_simple(&1, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(99u64)
            })
            .await;

        assert_eq!(r1, Ok(42));
        assert_eq!(r2, Ok(42));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn try_get_returns_none_for_missing_key() {
        let cache = test_cache("try_get_miss");
        assert_eq!(cache.try_get(&1).await, None);
    }

    #[test]
    async fn try_get_returns_value_for_cached_key() {
        let cache = test_cache("try_get_hit");
        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();
        assert_eq!(cache.try_get(&1).await, Some(42));
    }

    #[test]
    async fn get_returns_none_for_missing_key() {
        let cache = test_cache("get_miss");
        assert_eq!(cache.get(&1).await, None);
    }

    #[test]
    async fn get_returns_value_for_cached_key() {
        let cache = test_cache("get_hit");
        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();
        assert_eq!(cache.get(&1).await, Some(42));
    }

    #[test]
    async fn remove_deletes_cached_value() {
        let cache = test_cache("remove_test");
        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();
        assert!(cache.contains_key(&1).await);

        cache.remove(&1).await;

        assert!(!cache.contains_key(&1).await);
        assert_eq!(cache.get(&1).await, None);
    }

    #[test]
    async fn remove_nonexistent_key_is_noop() {
        let cache = test_cache("remove_noop");
        cache.remove(&999).await;
        assert!(!cache.contains_key(&999).await);
    }

    #[test]
    async fn contains_key_reflects_state() {
        let cache = test_cache("contains_key");
        assert!(!cache.contains_key(&1).await);

        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();
        assert!(cache.contains_key(&1).await);

        cache.remove(&1).await;
        assert!(!cache.contains_key(&1).await);
    }

    #[test]
    async fn iter_returns_all_cached_pairs() {
        let cache = test_cache("iter_test");
        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();
        cache
            .get_or_insert_simple(&2, || async { Ok(20u64) })
            .await
            .unwrap();
        cache
            .get_or_insert_simple(&3, || async { Ok(30u64) })
            .await
            .unwrap();

        let mut pairs = cache.iter().await;
        pairs.sort_by_key(|(k, _)| *k);
        assert_eq!(pairs, vec![(1, 10), (2, 20), (3, 30)]);
    }

    #[test]
    async fn keys_returns_all_keys() {
        let cache = test_cache("keys_test");
        cache
            .get_or_insert_simple(&5, || async { Ok(50u64) })
            .await
            .unwrap();
        cache
            .get_or_insert_simple(&10, || async { Ok(100u64) })
            .await
            .unwrap();

        let mut keys = cache.keys().await;
        keys.sort();
        assert_eq!(keys, vec![5, 10]);
    }

    // ---- Error handling ----

    #[test]
    async fn get_or_insert_propagates_error() {
        let cache = test_cache("error_propagate");
        let result: Result<u64, String> = cache
            .get_or_insert_simple(&1, || async { Err("boom".to_string()) })
            .await;
        assert_eq!(result, Err("boom".to_string()));
    }

    #[test]
    async fn failed_insert_does_not_cache_allows_retry() {
        let cache = test_cache("error_retry");
        let call_count = Arc::new(AtomicU64::new(0));

        let cc = call_count.clone();
        let r1 = cache
            .get_or_insert_simple(&1, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Err::<u64, _>("fail".to_string())
            })
            .await;
        assert!(r1.is_err());

        let cc = call_count.clone();
        let r2 = cache
            .get_or_insert_simple(&1, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(42u64)
            })
            .await;
        assert_eq!(r2, Ok(42));
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    async fn concurrent_waiters_receive_error_from_owner() {
        let cache = test_cache("error_concurrent");

        let futs: Vec<_> = (0..5)
            .map(|_| {
                let cache = cache.clone();
                async move {
                    cache
                        .get_or_insert_simple(&1, || async { Err::<u64, _>("fail".to_string()) })
                        .await
                }
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
            .await
            .expect("concurrent error test timed out");

        for r in results {
            assert!(r.is_err());
        }
    }

    // ---- Concurrency / deduplication ----

    #[test]
    async fn f2_called_only_once_for_concurrent_requests() {
        let cache = test_cache("dedup");
        let call_count = Arc::new(AtomicU64::new(0));

        let futs: Vec<_> = (0..5)
            .map(|_| {
                let cache = cache.clone();
                let call_count = call_count.clone();
                async move {
                    cache
                        .get_or_insert_simple(&1, || async move {
                            call_count.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            Ok(42u64)
                        })
                        .await
                }
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
            .await
            .expect("dedup test timed out");

        for r in &results {
            assert_eq!(r, &Ok(42));
        }
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "f2 should be called exactly once"
        );
    }

    #[test]
    async fn concurrent_requests_for_different_keys_execute_independently() {
        let cache = test_cache("different_keys");

        let futs: Vec<_> = (0u64..10)
            .map(|i| {
                let cache = cache.clone();
                async move {
                    cache
                        .get_or_insert_simple(&i, || async move { Ok(i * 10) })
                        .await
                }
            })
            .collect();

        let results = join_all(futs).await;
        for (i, r) in results.into_iter().enumerate() {
            assert_eq!(r.unwrap(), (i as u64) * 10);
        }
    }

    #[test]
    async fn get_waits_for_pending_item_to_complete() {
        let cache = test_cache("get_pending");
        let f2_entered = Arc::new(tokio::sync::Notify::new());
        let f2_proceed = Arc::new(tokio::sync::Notify::new());

        let cache_clone = cache.clone();
        let entered = f2_entered.clone();
        let proceed = f2_proceed.clone();
        let producer = tokio::spawn(async move {
            cache_clone
                .get_or_insert_simple(&1, || async move {
                    entered.notify_one();
                    proceed.notified().await;
                    Ok(42u64)
                })
                .await
        });

        // Wait until f2 is actually running (item is pending)
        f2_entered.notified().await;

        // try_get should return None while pending
        assert_eq!(cache.try_get(&1).await, None);
        // contains_key should still be true (pending entry exists)
        assert!(cache.contains_key(&1).await);

        // Start a get that should wait for the pending item
        let cache_clone = cache.clone();
        let getter = tokio::spawn(async move { cache_clone.get(&1).await });

        // Let the getter task start and subscribe
        tokio::task::yield_now().await;

        // Unblock the producer
        f2_proceed.notify_one();

        let producer_result = tokio::time::timeout(Duration::from_secs(5), producer)
            .await
            .expect("producer timed out")
            .unwrap();
        assert_eq!(producer_result, Ok(42));

        let getter_result = tokio::time::timeout(Duration::from_secs(5), getter)
            .await
            .expect("getter timed out")
            .unwrap();
        assert_eq!(getter_result, Some(42));
    }

    #[test]
    async fn get_returns_none_when_pending_item_fails() {
        let cache = test_cache("get_pending_fail");
        let f2_entered = Arc::new(tokio::sync::Notify::new());
        let f2_proceed = Arc::new(tokio::sync::Notify::new());

        let cache_clone = cache.clone();
        let entered = f2_entered.clone();
        let proceed = f2_proceed.clone();
        let producer = tokio::spawn(async move {
            cache_clone
                .get_or_insert_simple(&1, || async move {
                    entered.notify_one();
                    proceed.notified().await;
                    Err::<u64, _>("fail".to_string())
                })
                .await
        });

        f2_entered.notified().await;

        let cache_clone = cache.clone();
        let getter = tokio::spawn(async move { cache_clone.get(&1).await });

        tokio::task::yield_now().await;
        f2_proceed.notify_one();

        let producer_result = producer.await.unwrap();
        assert!(producer_result.is_err());

        let getter_result = tokio::time::timeout(Duration::from_secs(5), getter)
            .await
            .expect("getter timed out")
            .unwrap();
        assert_eq!(getter_result, None);
    }

    #[test]
    async fn concurrent_get_or_insert_with_instant_completion_does_not_hang() {
        for iteration in 0..100 {
            let cache = test_cache("instant_race");

            let futs: Vec<_> = (0..10)
                .map(|_| {
                    let cache = cache.clone();
                    async move {
                        cache
                            .get_or_insert_simple(&1u64, || async { Ok(42u64) })
                            .await
                    }
                })
                .collect();

            let result = tokio::time::timeout(Duration::from_secs(5), join_all(futs)).await;

            match result {
                Ok(results) => {
                    for r in results {
                        assert_eq!(r.unwrap(), 42);
                    }
                }
                Err(_) => {
                    panic!(
                        "Timed out on iteration {iteration} - cache race condition caused a hang"
                    );
                }
            }

            cache.remove(&1u64).await;
        }
    }

    #[test]
    async fn concurrent_insert_then_error_then_retry_succeeds() {
        let cache = test_cache("insert_error_retry");

        // First round: all concurrent requests get an error
        let futs: Vec<_> = (0..5)
            .map(|_| {
                let cache = cache.clone();
                async move {
                    cache
                        .get_or_insert_simple(
                            &1,
                            || async move { Err::<u64, _>("fail".to_string()) },
                        )
                        .await
                }
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
            .await
            .expect("timed out");
        for r in &results {
            assert!(r.is_err());
        }

        // Second round: retry should succeed, key should not be stuck
        let r = cache.get_or_insert_simple(&1, || async { Ok(99u64) }).await;
        assert_eq!(r, Ok(99));
    }

    // ---- Eviction ----

    #[test]
    async fn lru_eviction_when_capacity_exceeded() {
        let cache = bounded_cache(3, "lru_eviction");

        // Insert keys with small gaps so last_access times differ
        cache
            .get_or_insert_simple(&0, || async { Ok(0u64) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        cache
            .get_or_insert_simple(&1, || async { Ok(1u64) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        cache
            .get_or_insert_simple(&2, || async { Ok(2u64) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Touch key 0 to make it the most recently used
        cache.get(&0).await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Insert key 3: capacity exceeded, LRU eviction removes the oldest (key 1)
        cache
            .get_or_insert_simple(&3, || async { Ok(3u64) })
            .await
            .unwrap();

        // Key 0 was touched most recently, should still be present
        assert!(
            cache.contains_key(&0).await,
            "recently touched key 0 should survive eviction"
        );
        // Key 3 was just inserted
        assert!(
            cache.contains_key(&3).await,
            "newly inserted key 3 should be present"
        );
        // One of keys 1 or 2 should be evicted (the least recently used)
        let items = cache.iter().await;
        assert!(items.len() <= 3, "cache should not exceed capacity");
    }

    #[test]
    async fn lru_eviction_evicts_oldest_key() {
        // More targeted: with capacity 2, insert A then B, then C should evict A
        let cache = bounded_cache(2, "lru_eviction_oldest");

        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        cache
            .get_or_insert_simple(&2, || async { Ok(20u64) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Insert key 3 → evicts key 1 (oldest)
        cache
            .get_or_insert_simple(&3, || async { Ok(30u64) })
            .await
            .unwrap();

        assert!(
            !cache.contains_key(&1).await,
            "oldest key 1 should have been evicted"
        );
        assert!(cache.contains_key(&2).await);
        assert!(cache.contains_key(&3).await);
    }

    #[test]
    async fn background_eviction_older_than_ttl() {
        let cache: Cache<u64, (), u64, String> = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::OlderThan {
                ttl: Duration::from_millis(100),
                period: Duration::from_millis(50),
            },
            "bg_eviction_ttl",
        );

        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();
        assert!(cache.contains_key(&1).await);

        // Wait for TTL + a couple eviction periods to ensure background task runs
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert!(
            !cache.contains_key(&1).await,
            "entry should have been evicted by background task"
        );

        // Explicitly drop to abort background task before test ends
        drop(cache);
        tokio::task::yield_now().await;
    }

    #[test]
    async fn background_eviction_lru_mode() {
        let cache: Cache<u64, (), u64, String> = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::LeastRecentlyUsed {
                count: 1,
                period: Duration::from_millis(50),
            },
            "bg_eviction_lru",
        );

        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        cache
            .get_or_insert_simple(&2, || async { Ok(20u64) })
            .await
            .unwrap();

        assert!(cache.contains_key(&1).await);
        assert!(cache.contains_key(&2).await);

        // Wait for background eviction to run — it evicts 1 LRU entry per period
        tokio::time::sleep(Duration::from_millis(150)).await;

        // At least one entry should have been evicted
        let items = cache.iter().await;
        assert!(
            items.len() < 2,
            "background LRU eviction should have removed at least one entry"
        );

        drop(cache);
        tokio::task::yield_now().await;
    }

    #[test]
    async fn background_eviction_preserves_recently_accessed() {
        let cache: Cache<u64, (), u64, String> = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::OlderThan {
                ttl: Duration::from_millis(100),
                period: Duration::from_millis(50),
            },
            "bg_eviction_preserve",
        );

        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();

        // Keep accessing key 1 to prevent it from being evicted
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            assert_eq!(cache.get(&1).await, Some(10));
        }

        // After 200ms of periodic access, key should still be present
        assert!(
            cache.contains_key(&1).await,
            "recently accessed entry should not be evicted"
        );

        drop(cache);
        tokio::task::yield_now().await;
    }

    // ---- get_or_insert_pending ----

    #[test]
    async fn get_or_insert_pending_returns_pending_for_new_key() {
        let cache: Cache<u64, String, u64, String> = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            "pending_new",
        );

        let result = cache
            .get_or_insert_pending(
                &1,
                || "loading".to_string(),
                |_pv| Box::pin(async { Ok(42u64) }),
            )
            .await
            .unwrap();

        match result {
            PendingOrFinal::Pending(pv) => assert_eq!(pv, "loading"),
            PendingOrFinal::Final(_) => panic!("expected Pending"),
        }

        // Wait for background task to complete and cache the value
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(cache.get(&1).await, Some(42));
    }

    #[test]
    async fn get_or_insert_pending_returns_final_for_cached_key() {
        let cache: Cache<u64, String, u64, String> = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            "pending_cached",
        );

        cache
            .get_or_insert(&1, || "loading".to_string(), async |_| Ok(42u64))
            .await
            .unwrap();

        let result = cache
            .get_or_insert_pending(
                &1,
                || "loading".to_string(),
                |_pv| Box::pin(async { Ok(99u64) }),
            )
            .await
            .unwrap();

        match result {
            PendingOrFinal::Final(v) => assert_eq!(v, 42),
            PendingOrFinal::Pending(_) => panic!("expected Final"),
        }
    }

    #[test]
    async fn get_or_insert_pending_error_does_not_leave_stale_pending() {
        let cache: Cache<u64, String, u64, String> = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            "pending_error",
        );

        let result = cache
            .get_or_insert_pending(
                &1,
                || "loading".to_string(),
                |_pv| Box::pin(async { Err::<u64, _>("fail".to_string()) }),
            )
            .await;
        assert!(result.is_ok()); // get_or_insert_pending itself succeeds

        // Wait for background task to run and fail
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Key should NOT be cached (error removes it), retry should work
        let result = cache
            .get_or_insert(&1, || "loading2".to_string(), async |_| Ok(42u64))
            .await;
        assert_eq!(result, Ok(42));
    }

    // ---- create_weak_remover ----

    #[test]
    async fn weak_remover_removes_entry() {
        let cache = test_cache("weak_remover");
        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();

        let remover = cache.create_weak_remover(1);
        remover();

        assert!(!cache.contains_key(&1).await);
    }

    #[test]
    async fn weak_remover_after_cache_drop_is_noop() {
        let cache = test_cache("weak_remover_drop");
        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();

        let remover = cache.create_weak_remover(1);
        drop(cache);

        // Should not panic
        remover();
    }

    // ---- Multiple keys / isolation ----

    #[test]
    async fn multiple_keys_are_independent() {
        let cache = test_cache("multi_key");

        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();
        cache
            .get_or_insert_simple(&2, || async { Ok(20u64) })
            .await
            .unwrap();

        assert_eq!(cache.get(&1).await, Some(10));
        assert_eq!(cache.get(&2).await, Some(20));

        cache.remove(&1).await;
        assert_eq!(cache.get(&1).await, None);
        assert_eq!(cache.get(&2).await, Some(20));
    }

    // ---- Stress tests ----

    #[test]
    async fn stress_many_concurrent_keys() {
        let cache = test_cache("stress_keys");

        let futs: Vec<_> = (0u64..100)
            .map(|i| {
                let cache = cache.clone();
                async move {
                    cache
                        .get_or_insert_simple(&i, || async move {
                            tokio::task::yield_now().await;
                            Ok(i * 100)
                        })
                        .await
                }
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(10), join_all(futs))
            .await
            .expect("stress test timed out");

        for (i, r) in results.into_iter().enumerate() {
            assert_eq!(r.unwrap(), (i as u64) * 100);
        }
    }

    #[test]
    async fn stress_concurrent_mixed_operations() {
        let cache = test_cache("stress_mixed");

        // Pre-populate some keys
        for i in 0u64..10 {
            cache
                .get_or_insert_simple(&i, || async move { Ok(i) })
                .await
                .unwrap();
        }

        // Run readers, inserters, and removers concurrently using join_all
        let mut futs: Vec<Pin<Box<dyn Future<Output = ()>>>> = Vec::new();

        // Concurrent readers
        for i in 0u64..10 {
            let cache = cache.clone();
            futs.push(Box::pin(async move {
                for _ in 0..50 {
                    let _ = cache.get(&i).await;
                    let _ = cache.try_get(&i).await;
                    tokio::task::yield_now().await;
                }
            }));
        }

        // Concurrent inserters for new keys
        for i in 10u64..20 {
            let cache = cache.clone();
            futs.push(Box::pin(async move {
                cache
                    .get_or_insert_simple(&i, || async move { Ok(i * 10) })
                    .await
                    .unwrap();
            }));
        }

        // Concurrent removers
        for i in 0u64..5 {
            let cache = cache.clone();
            futs.push(Box::pin(async move {
                tokio::task::yield_now().await;
                cache.remove(&i).await;
            }));
        }

        tokio::time::timeout(Duration::from_secs(10), join_all(futs))
            .await
            .expect("stress mixed ops timed out");

        // Newly inserted keys (10..20) should still be present (not removed)
        for i in 10u64..20 {
            assert_eq!(
                cache.get(&i).await,
                Some(i * 10),
                "key {i} should still be cached"
            );
        }

        // Removed keys (0..5) should be gone
        for i in 0u64..5 {
            assert_eq!(
                cache.get(&i).await,
                None,
                "key {i} should have been removed"
            );
        }
    }

    // ---- Pending entry semantics: keys() vs iter() ----

    #[test]
    async fn keys_includes_pending_but_iter_excludes_pending() {
        let cache = test_cache("pending_visibility");
        let f2_entered = Arc::new(tokio::sync::Notify::new());
        let f2_proceed = Arc::new(tokio::sync::Notify::new());

        // Insert a cached value for key 1
        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();

        // Start a pending insert for key 2
        let cache_clone = cache.clone();
        let entered = f2_entered.clone();
        let proceed = f2_proceed.clone();
        let _producer = tokio::spawn(async move {
            cache_clone
                .get_or_insert_simple(&2, || async move {
                    entered.notify_one();
                    proceed.notified().await;
                    Ok(20u64)
                })
                .await
        });

        f2_entered.notified().await;

        // keys() should include both cached and pending keys
        let mut keys = cache.keys().await;
        keys.sort();
        assert_eq!(keys, vec![1, 2], "keys() should include pending key 2");

        // iter() should only include cached values, not pending ones
        let pairs = cache.iter().await;
        assert_eq!(pairs, vec![(1, 10)], "iter() should exclude pending key 2");

        f2_proceed.notify_one();
    }

    // ---- Remove while pending ----

    #[test]
    async fn remove_cached_entry_while_other_key_is_pending() {
        let cache = test_cache("remove_cached_while_pending");
        let f2_entered = Arc::new(tokio::sync::Notify::new());
        let f2_proceed = Arc::new(tokio::sync::Notify::new());

        // Cache key 1
        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();

        // Start pending insert for key 2
        let cache_clone = cache.clone();
        let entered = f2_entered.clone();
        let proceed = f2_proceed.clone();
        let producer = tokio::spawn(async move {
            cache_clone
                .get_or_insert_simple(&2, || async move {
                    entered.notify_one();
                    proceed.notified().await;
                    Ok(20u64)
                })
                .await
        });

        f2_entered.notified().await;

        // Remove cached key 1 while key 2 is pending — should not affect key 2
        cache.remove(&1).await;
        assert!(!cache.contains_key(&1).await);

        // Unblock key 2
        f2_proceed.notify_one();

        let result = tokio::time::timeout(Duration::from_secs(5), producer)
            .await
            .expect("producer timed out")
            .unwrap();
        assert_eq!(result, Ok(20));
        assert_eq!(cache.get(&2).await, Some(20));
    }

    // ---- Overwrite after remove ----

    #[test]
    async fn insert_after_remove_gets_new_value() {
        let cache = test_cache("insert_after_remove");

        cache
            .get_or_insert_simple(&1, || async { Ok(10u64) })
            .await
            .unwrap();
        assert_eq!(cache.get(&1).await, Some(10));

        cache.remove(&1).await;
        assert_eq!(cache.get(&1).await, None);

        cache
            .get_or_insert_simple(&1, || async { Ok(99u64) })
            .await
            .unwrap();
        assert_eq!(cache.get(&1).await, Some(99));
    }

    // ---- Clone semantics ----

    #[test]
    async fn cloned_cache_shares_state() {
        let cache = test_cache("clone_shares");
        let clone = cache.clone();

        cache
            .get_or_insert_simple(&1, || async { Ok(42u64) })
            .await
            .unwrap();

        assert_eq!(clone.get(&1).await, Some(42));

        clone.remove(&1).await;
        assert_eq!(cache.get(&1).await, None);
    }

    // ---- Concurrent get_or_insert for the same key with slow f2 ----

    #[test]
    async fn waiters_all_get_result_from_slow_producer() {
        let cache = test_cache("slow_producer");
        let call_count = Arc::new(AtomicU64::new(0));

        let futs: Vec<_> = (0..5)
            .map(|_| {
                let cache = cache.clone();
                let call_count = call_count.clone();
                async move {
                    cache
                        .get_or_insert_simple(&1, || async move {
                            call_count.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            Ok(42u64)
                        })
                        .await
                }
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(5), join_all(futs))
            .await
            .expect("slow producer test timed out");

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        for r in results {
            assert_eq!(r, Ok(42));
        }
    }
}
