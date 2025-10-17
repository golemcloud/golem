// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
            Some(Err(mut rx)) => rx.recv().await.ok().and_then(|r| r.ok()),
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
                        if tx.receiver_count() > 0 {
                            let _ = tx.send(value.clone());
                        }

                        value
                    } else {
                        record_cache_hit(self.name);

                        let mut rx = tx.subscribe();
                        rx.recv().await.unwrap()
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
                                            key_clone,
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
                                }
                                if tx_clone.receiver_count() > 0 {
                                    let _ = tx_clone.send(value.clone());
                                }
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
                match value {
                    Item::Cached { last_access, .. } => {
                        keys_to_keep.push((key.clone(), last_access.elapsed().as_millis()))
                    }
                    _ => {}
                }
                true
            })
            .await;

        keys_to_keep.sort_by_key(|(_, v)| *v);
        keys_to_keep.truncate(keys_to_keep.len() - count);
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
                let (tx, _) = tokio::sync::broadcast::channel(1);
                Item::Pending {
                    tx,
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
        tx: tokio::sync::broadcast::Sender<Result<V, E>>,
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
