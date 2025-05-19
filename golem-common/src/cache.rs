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
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::try_result::TryResult::{Absent, Locked, Present};
use dashmap::DashMap;
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
    items: DashMap<K, Item<V, PV, E>>,
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
        self.get_or_insert(key, || Ok(()), async |_| f().await)
            .await
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
                Some(capacity) => DashMap::with_capacity(capacity),
                None => DashMap::new(),
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
                        cache_clone.background_evict(&eviction);
                    }
                }))
            }
            BackgroundEvictionMode::OlderThan { period, .. } => {
                let cache_clone = cache.clone();
                let eviction = background_eviction;
                Some(tokio::task::spawn(async move {
                    loop {
                        tokio::time::sleep(period).await;
                        cache_clone.background_evict(&eviction);
                    }
                }))
            }
            BackgroundEvictionMode::None => None,
        };
        *cache.background_handle.lock().unwrap() = background_handle;

        cache
    }

    /// Tries to get a cached value for the given key. If the value is missing or is pending, it returns None.
    #[allow(unused)]
    pub fn try_get(&self, key: &K) -> Option<V> {
        let result = match self.state.items.try_get(key) {
            Present(item) => match item.deref() {
                Item::Pending { .. } => None,
                Item::Cached { value, .. } => Some(value.clone()),
            },
            Absent | Locked => None,
        };
        if result.is_some() {
            self.update_last_access(key);
        }
        result
    }

    /// Gets a cached value for the given key. If the value is pending, it awaits it.
    /// If the pending value fails, it returns None.
    pub async fn get(&self, key: &K) -> Option<V> {
        let result = match self.state.items.get(key) {
            Some(item) => match item.deref() {
                Item::Pending { tx, .. } => {
                    let mut rx = tx.subscribe();
                    rx.recv().await.ok().and_then(|r| r.ok())
                }
                Item::Cached { value, .. } => Some(value.clone()),
            },
            None => None,
        };

        if result.is_some() {
            self.update_last_access(key);
        }

        result
    }

    /// Gets a cached value for a given key, or inserts a new one with the given async function. If a value is pending,
    /// it is awaited instead of recreating it.
    pub async fn get_or_insert<F1, F2>(&self, key: &K, f1: F1, f2: F2) -> Result<V, E>
    where
        F1: FnOnce() -> Result<PV, E>,
        F2: AsyncFnOnce(&PV) -> Result<V, E>,
    {
        let mut eviction_needed = false;
        let result = {
            let own_id = self.state.last_id.fetch_add(1, Ordering::SeqCst);
            let result = self.get_or_add_as_pending(key, own_id, f1)?;
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
                            self.state.items.insert(
                                key.clone(),
                                Item::Cached {
                                    value: success_value.clone(),
                                    last_access: Instant::now(),
                                },
                            );
                            let old_count = self.state.count.fetch_add(1, Ordering::SeqCst);

                            record_cache_size(self.name, old_count.saturating_add(1));

                            if Some(old_count) == self.capacity {
                                eviction_needed = true;
                            }
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

                    self.update_last_access(key);
                    Ok(value)
                }
            }
        };

        if eviction_needed {
            self.evict();
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
        F1: FnOnce() -> Result<PV, E>,
        F2: FnOnce(&PV) -> Pin<Box<dyn Future<Output = Result<V, E>> + Send>> + Send + 'static,
    {
        {
            let own_id = self.state.last_id.fetch_add(1, Ordering::SeqCst);
            let result = self.get_or_add_as_pending(key, own_id, f1)?;
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
                                    self_clone.state.items.insert(
                                        key_clone,
                                        Item::Cached {
                                            value: success_value.clone(),
                                            last_access: Instant::now(),
                                        },
                                    );
                                    let old_count =
                                        self_clone.state.count.fetch_add(1, Ordering::SeqCst);

                                    record_cache_size(self_clone.name, old_count.saturating_add(1));

                                    if Some(old_count) == self_clone.capacity {
                                        self_clone.evict();
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

                    self.update_last_access(key);
                    Ok(PendingOrFinal::Final(value))
                }
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (K, V)> + '_ {
        self.state.items.iter().filter_map(|r| match r.deref() {
            Item::Pending { .. } => None,
            Item::Cached { value, .. } => Some((r.key().clone(), value.clone())),
        })
    }

    pub fn remove(&self, key: &K) {
        let removed = self.state.items.remove(key).is_some();
        if removed {
            let count = self.state.count.fetch_sub(1, Ordering::SeqCst);
            record_cache_size(self.name, count.saturating_sub(1));
        }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.state.items.contains_key(key)
    }

    pub fn create_weak_remover(&self, key: K) -> impl FnOnce() {
        let weak_state = Arc::downgrade(&self.state);
        let name = self.name;
        move || {
            if let Some(state) = weak_state.upgrade() {
                let removed = state.items.remove(&key).is_some();
                if removed {
                    let count = state.count.fetch_sub(1, Ordering::SeqCst);
                    record_cache_size(name, count.saturating_sub(1));
                }
            }
        }
    }

    fn evict(&self) {
        record_cache_eviction(self.name, "full");
        match self.full_cache_eviction {
            FullCacheEvictionMode::None => {}
            FullCacheEvictionMode::LeastRecentlyUsed(count) => {
                self.evict_least_recently_used(count);
            }
        }
    }

    fn background_evict(&self, mode: &BackgroundEvictionMode) {
        record_cache_eviction(self.name, "background");
        match mode {
            BackgroundEvictionMode::None => {}
            BackgroundEvictionMode::LeastRecentlyUsed { count, .. } => {
                self.evict_least_recently_used(*count)
            }
            BackgroundEvictionMode::OlderThan { ttl, .. } => self.evict_older_than(*ttl),
        }
    }

    fn evict_least_recently_used(&self, count: usize) {
        let mut keys_to_keep: Vec<(K, u128)> = self
            .state
            .items
            .iter()
            .filter_map(|item| {
                let k = item.key().clone();
                match item.value() {
                    Item::Cached { last_access, .. } => {
                        Some((k, last_access.elapsed().as_millis()))
                    }
                    _ => None,
                }
            })
            .collect();
        keys_to_keep.sort_by_key(|(_, v)| *v);
        keys_to_keep.truncate(keys_to_keep.len() - count);
        let keys_to_keep: HashSet<&K> = keys_to_keep.iter().map(|(k, _)| k).collect();

        self.state.items.retain(|k, v| match v {
            Item::Cached { .. } => keys_to_keep.contains(k),
            Item::Pending { .. } => true,
        });
        self.state.count.store(keys_to_keep.len(), Ordering::SeqCst);
        record_cache_size(self.name, keys_to_keep.len());
    }

    fn evict_older_than(&self, ttl: Duration) {
        self.state.items.retain(|_, item| match item {
            Item::Cached { last_access, .. } => last_access.elapsed() < ttl,
            Item::Pending { .. } => true,
        });
        let count = self.state.items.len();
        self.state.count.store(count, Ordering::SeqCst);
        record_cache_size(self.name, count);
    }

    fn update_last_access(&self, key: &K) {
        self.state.items.entry(key.clone()).and_modify(|item| {
            if let Item::Cached { last_access, .. } = item {
                *last_access = Instant::now()
            }
        });
    }

    fn get_or_add_as_pending<F>(&self, key: &K, own_id: u64, f: F) -> Result<Item<V, PV, E>, E>
    where
        F: FnOnce() -> Result<PV, E>,
    {
        Ok(self
            .state
            .items
            .entry(key.clone())
            .or_try_insert_with(|| {
                f().map(|pending_value| {
                    let (tx, _) = tokio::sync::broadcast::channel(1);
                    Item::Pending {
                        tx,
                        id: own_id,
                        pending_value,
                    }
                })
            })?
            .value()
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
