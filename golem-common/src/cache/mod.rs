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
    /// Test-only seam: when set, awaited inside the full-cache eviction between
    /// snapshotting the entries to keep and committing the new size, so a test
    /// can deterministically interleave a concurrent insert at that point.
    #[cfg(test)]
    evict_interleave: Arc<Mutex<Option<EvictInterleaveHook>>>,
    /// Test-only seam: when set, awaited inside the full-cache eviction after
    /// snapshotting the entries to keep but before retaining the map, so a test
    /// can deterministically interleave concurrent evictions with stale
    /// snapshots.
    #[cfg(test)]
    evict_before_retain_interleave: Arc<Mutex<Option<EvictInterleaveHook>>>,
    /// Test-only seam: when set, awaited in `get_or_insert_spawned` between
    /// spawning the owner task and subscribing to the result watch, so a test
    /// can deterministically let the spawned owner finish before the caller
    /// subscribes.
    #[cfg(test)]
    spawned_pre_subscribe_interleave: Arc<Mutex<Option<EvictInterleaveHook>>>,
}

#[cfg(test)]
type EvictInterleaveHook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub trait SimpleCache<K, V, E> {
    fn get_or_insert_simple<F>(&self, key: &K, f: F) -> impl Future<Output = Result<V, E>>
    where
        F: AsyncFnOnce() -> Result<V, E>;

    /// Cancellation-safe variant of [`Self::get_or_insert_simple`].
    ///
    /// The owner future is spawned on the Tokio runtime, so dropping the
    /// caller (e.g. a cancelled request) does NOT leave the pending cache
    /// entry stuck forever. The caller subscribes to the same watch channel
    /// as other waiters and receives the spawned future's `Result<V, E>` as
    /// usual.
    ///
    /// Use this variant when the closure captures shared state that must
    /// survive caller cancellation (for example a per-worker read-only
    /// invocation that is already enqueued and is going to complete on the
    /// worker even if the originating gRPC call is cancelled).
    fn get_or_insert_simple_spawned<F, Fut>(
        &self,
        key: &K,
        f: F,
    ) -> impl Future<Output = Result<V, E>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<V, E>> + Send + 'static;
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

    async fn get_or_insert_simple_spawned<F, Fut>(&self, key: &K, f: F) -> Result<V, E>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<V, E>> + Send + 'static,
    {
        self.get_or_insert_spawned(key, || (), move |_| Box::pin(f()))
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
            #[cfg(test)]
            evict_interleave: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            evict_before_retain_interleave: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            spawned_pre_subscribe_interleave: Arc::new(Mutex::new(None)),
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

    /// Test-only: installs a hook awaited inside full-cache eviction between
    /// computing the surviving entry set and committing the new size, so a test
    /// can deterministically interleave a concurrent insert at that point.
    #[cfg(test)]
    fn set_evict_interleave(&self, hook: EvictInterleaveHook) {
        *self.evict_interleave.lock().unwrap() = Some(hook);
    }

    /// Test-only: removes the eviction interleave hook.
    #[cfg(test)]
    fn clear_evict_interleave(&self) {
        *self.evict_interleave.lock().unwrap() = None;
    }

    /// Test-only: installs a hook awaited inside full-cache eviction after the
    /// surviving entry set is computed but before the map is retained.
    #[cfg(test)]
    fn set_evict_before_retain_interleave(&self, hook: EvictInterleaveHook) {
        *self.evict_before_retain_interleave.lock().unwrap() = Some(hook);
    }

    /// Test-only: removes the pre-retain eviction interleave hook.
    #[cfg(test)]
    fn clear_evict_before_retain_interleave(&self) {
        *self.evict_before_retain_interleave.lock().unwrap() = None;
    }

    /// Test-only: installs a hook awaited in `get_or_insert_spawned` between
    /// spawning the owner task and subscribing to the result watch.
    #[cfg(test)]
    fn set_spawned_pre_subscribe_interleave(&self, hook: EvictInterleaveHook) {
        *self.spawned_pre_subscribe_interleave.lock().unwrap() = Some(hook);
    }

    /// Test-only: removes the pre-subscribe interleave hook.
    #[cfg(test)]
    fn clear_spawned_pre_subscribe_interleave(&self) {
        *self.spawned_pre_subscribe_interleave.lock().unwrap() = None;
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
            let own_id = self.state.last_id.fetch_add(1, Ordering::Relaxed);
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
                            let old_count = self.state.count.fetch_add(1, Ordering::Relaxed);
                            let new_count = old_count.saturating_add(1);

                            record_cache_size(self.name, new_count);

                            if self.capacity.is_some_and(|capacity| new_count > capacity) {
                                eviction_needed = true;
                            }
                        } else {
                            self.state.items.remove_async(key).await;
                        }
                        // `send_replace` instead of `send`: a plain `send` fails
                        // without storing the value when the channel has no
                        // receivers, and waiters that found the pending entry but
                        // have not subscribed yet would then wait forever.
                        tx.send_replace(Some(value.clone()));

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

    /// Cancellation-safe variant of [`Self::get_or_insert`].
    ///
    /// Behaves like `get_or_insert`, but the owner future is spawned via
    /// `tokio::task::spawn` instead of being awaited inline. The caller
    /// subscribes to the same `tokio::sync::watch` channel as other waiters,
    /// so if the caller's future is dropped the spawned owner still runs to
    /// completion and resolves the pending entry — either upserting the
    /// cached value (on `Ok`) or removing the pending entry (on `Err`).
    ///
    /// Without this guarantee, a cancelled caller could leave the pending
    /// entry in the map forever, permanently blocking subsequent callers
    /// for the same key.
    pub async fn get_or_insert_spawned<F1, F2>(&self, key: &K, f1: F1, f2: F2) -> Result<V, E>
    where
        F1: FnOnce() -> PV,
        F2: FnOnce(&PV) -> Pin<Box<dyn Future<Output = Result<V, E>> + Send>> + Send + 'static,
    {
        let own_id = self.state.last_id.fetch_add(1, Ordering::Relaxed);
        let result = self.get_or_add_as_pending(key, own_id, f1).await?;
        match result {
            Item::Pending {
                ref tx,
                id,
                pending_value,
            } => {
                if id == own_id {
                    record_cache_miss(self.name);
                    // Owner: spawn the producer so cancellation of the caller
                    // does not abandon the pending entry.
                    let key_clone = key.clone();
                    let tx_clone = tx.clone();
                    let self_clone = self.clone();
                    let mut eviction_needed = false;
                    tokio::task::spawn(
                        async move {
                            let value = f2(&pending_value).await;
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
                                    self_clone.state.count.fetch_add(1, Ordering::Relaxed);
                                let new_count = old_count.saturating_add(1);

                                record_cache_size(self_clone.name, new_count);

                                if self_clone
                                    .capacity
                                    .is_some_and(|capacity| new_count > capacity)
                                {
                                    eviction_needed = true;
                                }
                            } else {
                                self_clone.state.items.remove_async(&key_clone).await;
                            }
                            // `send_replace` instead of `send`: the spawned owner
                            // can finish before the caller subscribes, and a plain
                            // `send` on a receiver-less channel fails without
                            // storing the value, leaving all subsequent
                            // subscribers waiting forever.
                            tx_clone.send_replace(Some(value));
                            if eviction_needed {
                                self_clone.evict().await;
                            }
                        }, // NOT `.in_current_span()`: this owner outlives the caller by
                           // design, so cloning the caller's span would hold it open for
                           // the owner's whole life. Callers that want the work traced
                           // must instrument the future they hand in - see the
                           // `read_only_invocation` span in the worker executor for the
                           // shape: capture a `TraceOrigin` and link, do not nest.
                    );
                } else {
                    record_cache_hit(self.name);
                }

                // Test-only seam: let a test delay the subscription until the
                // spawned owner has already completed and sent its result.
                #[cfg(test)]
                {
                    let hook = self
                        .spawned_pre_subscribe_interleave
                        .lock()
                        .unwrap()
                        .clone();
                    if let Some(hook) = hook {
                        hook().await;
                    }
                }

                // Owner and all waiters subscribe to the same watch and
                // receive the spawned future's `Result<V, E>`.
                let mut rx = tx.subscribe();
                let val = rx
                    .wait_for(|v| v.is_some())
                    .await
                    .expect("cache watch sender dropped without sending");
                val.clone()
                    .expect("watch value must be Some after wait_for")
            }
            Item::Cached { value, .. } => {
                record_cache_hit(self.name);
                self.update_last_access(key).await;
                Ok(value)
            }
        }
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
            let own_id = self.state.last_id.fetch_add(1, Ordering::Relaxed);
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

                        // Same as `get_or_insert_spawned` above: no `.in_current_span()`,
                        // because this owner outlives the caller that started it.
                        tokio::task::spawn(async move {
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
                                    self_clone.state.count.fetch_add(1, Ordering::Relaxed);
                                let new_count = old_count.saturating_add(1);

                                record_cache_size(self_clone.name, new_count);

                                if self_clone
                                    .capacity
                                    .is_some_and(|capacity| new_count > capacity)
                                {
                                    self_clone.evict().await;
                                }
                            } else {
                                self_clone.state.items.remove_async(&key_clone).await;
                            }
                            // `send_replace` instead of `send`: a plain `send`
                            // fails without storing the value when no waiter
                            // has subscribed yet, and later subscribers would
                            // then wait forever.
                            tx_clone.send_replace(Some(value.clone()));
                        });
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

    /// Returns a snapshot of cached entries whose last access was at least
    /// `ttl` ago without refreshing their access time. Pending entries are
    /// excluded.
    pub async fn entries_older_than(&self, ttl: Duration) -> Vec<(K, V)> {
        let mut snapshotted_pairs = vec![];
        self.state
            .items
            .iter_async(|key, item| {
                if let Item::Cached { value, last_access } = item
                    && last_access.elapsed() >= ttl
                {
                    snapshotted_pairs.push((key.clone(), value.clone()));
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
            let count = self.state.count.fetch_sub(1, Ordering::Relaxed);
            record_cache_size(self.name, count.saturating_sub(1));
        }
    }

    /// Removes the cached value for `key` only if the stored `Cached` value
    /// satisfies `predicate`. Pending entries are never removed. The predicate
    /// runs under the atomic remove, so concurrent inserts cannot be removed
    /// by accident.
    pub async fn remove_if_cached<F>(&self, key: &K, predicate: F) -> bool
    where
        F: Fn(&V) -> bool,
    {
        let removed = self
            .state
            .items
            .remove_if_async(key, |item| match item {
                Item::Cached { value, .. } => predicate(value),
                Item::Pending { .. } => false,
            })
            .await
            .is_some();
        if removed {
            let count = self.state.count.fetch_sub(1, Ordering::SeqCst);
            record_cache_size(self.name, count.saturating_sub(1));
        }
        removed
    }

    /// Removes the cached value for `key` only if it has not been accessed for
    /// at least `ttl` and satisfies `predicate`. Age and value are checked in
    /// the same atomic map operation, so a completed access or replacement is
    /// observed before removal. The predicate can additionally reject removal
    /// while a value cloned by an in-progress access is still in use. Pending
    /// entries are never removed.
    pub async fn remove_if_cached_older_than<F>(&self, key: &K, ttl: Duration, predicate: F) -> bool
    where
        F: Fn(&V) -> bool,
    {
        let removed = self
            .state
            .items
            .remove_if_async(key, |item| match item {
                Item::Cached { value, last_access } => {
                    last_access.elapsed() >= ttl && predicate(value)
                }
                Item::Pending { .. } => false,
            })
            .await
            .is_some();
        if removed {
            let count = self.state.count.fetch_sub(1, Ordering::SeqCst);
            record_cache_size(self.name, count.saturating_sub(1));
        }
        removed
    }

    pub async fn contains_key(&self, key: &K) -> bool {
        self.state.items.contains_async(key).await
    }

    pub fn create_weak_remover(&self, key: K) -> impl FnOnce() + use<K, V, PV, E> {
        let weak_state = Arc::downgrade(&self.state);
        let name = self.name;
        move || {
            if let Some(state) = weak_state.upgrade() {
                let removed = state.items.remove_sync(&key).is_some();
                if removed {
                    let count = state.count.fetch_sub(1, Ordering::Relaxed);
                    record_cache_size(name, count.saturating_sub(1));
                }
            }
        }
    }

    pub fn create_weak_conditional_remover<F>(
        &self,
        key: K,
        predicate: F,
    ) -> impl FnOnce() + use<K, V, PV, E, F>
    where
        F: Fn(&V) -> bool,
    {
        let weak_state = Arc::downgrade(&self.state);
        let name = self.name;
        move || {
            if let Some(state) = weak_state.upgrade() {
                let removed = state
                    .items
                    .remove_if_sync(&key, |item| match item {
                        Item::Cached { value, .. } => predicate(value),
                        Item::Pending { .. } => false,
                    })
                    .is_some();
                if removed {
                    let count = state.count.fetch_sub(1, Ordering::Relaxed);
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
        let mut cached = vec![];
        self.state
            .items
            .iter_async(|key, value| {
                if let Item::Cached { last_access, .. } = value {
                    cached.push((key.clone(), last_access.elapsed().as_millis()))
                }
                true
            })
            .await;

        // Sort most-recently-used first (smallest elapsed first) so truncating
        // the tail drops the oldest entries and keeps the newest.
        cached.sort_by_key(|(_, elapsed)| *elapsed);

        // Keep at most `cached_len - count` entries, and never more than the
        // configured capacity, so an over-capacity cache is always trimmed back
        // down to the bound regardless of how far it overshot.
        let cached_len = cached.len();
        let mut keep = cached_len.saturating_sub(count);
        if let Some(capacity) = self.capacity {
            keep = keep.min(capacity);
        }
        cached.truncate(keep);

        #[cfg(test)]
        {
            let hook = self.evict_before_retain_interleave.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook().await;
            }
        }

        let keys_to_keep: HashSet<&K> = cached.iter().map(|(k, _)| k).collect();

        let removed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let removed_in_retain = removed.clone();
        self.state
            .items
            .retain_async(|k, v| match v {
                Item::Cached { .. } => {
                    let keep = keys_to_keep.contains(k);
                    if !keep {
                        removed_in_retain.fetch_add(1, Ordering::Relaxed);
                    }
                    keep
                }
                Item::Pending { .. } => true,
            })
            .await;

        // Test-only seam: let a test interleave a concurrent insert here, after
        // the surviving set has been computed but before the size is committed,
        // to deterministically exercise the count race.
        #[cfg(test)]
        {
            let hook = self.evict_interleave.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook().await;
            }
        }

        // Decrement by the number of cached entries this retain actually
        // removed rather than by the stale snapshot's expected removal count.
        // A blind store would clobber concurrent insert increments, and a
        // snapshot-derived decrement would double-subtract when concurrent
        // evictions try to remove the same entries.
        let removed = removed.load(Ordering::Relaxed);
        let new_count = self
            .state
            .count
            .fetch_sub(removed, Ordering::Relaxed)
            .saturating_sub(removed);
        record_cache_size(self.name, new_count);
    }

    async fn evict_older_than(&self, ttl: Duration) {
        let removed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let removed_in_retain = removed.clone();
        self.state
            .items
            .retain_async(|_, item| match item {
                Item::Cached { last_access, .. } => {
                    let keep = last_access.elapsed() < ttl;
                    if !keep {
                        removed_in_retain.fetch_add(1, Ordering::Relaxed);
                    }
                    keep
                }
                Item::Pending { .. } => true,
            })
            .await;
        // Decrement by the number of cached entries actually removed rather than
        // overwriting the counter, so concurrent insert increments are not lost.
        let removed = removed.load(Ordering::Relaxed);
        let new_count = self
            .state
            .count
            .fetch_sub(removed, Ordering::Relaxed)
            .saturating_sub(removed);
        record_cache_size(self.name, new_count);
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
mod tests;
