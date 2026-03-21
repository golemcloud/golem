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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError};

use tracing::{debug, Instrument};

use crate::services::golem_config::{MemoryConfig, StorageConfig};
use crate::services::HasAll;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::agent::Principal;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::worker::WorkerAgentConfigEntry;
use golem_common::model::{AgentId, OwnedAgentId};
use golem_service_base::error::worker_executor::WorkerExecutorError;

/// Executor-wide storage semaphore. One permit = `STORAGE_PERMIT_SIZE_KB` KB.
///
/// Extracted as a standalone struct so it can be unit-tested independently of
/// the `WorkerCtx`-generic `ActiveWorkers`.
pub struct StorageSemaphore {
    semaphore: Arc<Semaphore>,
    /// Held during non-blocking priority acquires to interrupt any in-progress
    /// blocking `acquire` loops, preventing starvation of high-priority callers.
    priority_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
}

impl StorageSemaphore {
    pub(crate) fn new(pool_bytes: usize, acquire_retry_delay: Duration) -> Self {
        let permits = storage_pool_bytes_to_permits(pool_bytes);
        Self {
            semaphore: Arc::new(Semaphore::new(permits)),
            priority_lock: Arc::new(Mutex::new(())),
            acquire_retry_delay,
        }
    }

    /// Available bytes remaining in the pool (rounded down to KB boundary).
    #[cfg(test)]
    pub(crate) fn available_bytes(&self) -> u64 {
        self.semaphore.available_permits() as u64 * STORAGE_PERMIT_SIZE_KB * 1024
    }

    /// Blocking acquire. Loops until `storage_bytes` are available, calling
    /// `try_free_up` each time permits are exhausted. If `try_free_up` returns
    /// `false` (nothing to evict), sleeps `acquire_retry_delay` before retrying.
    pub(crate) async fn acquire<F, Fut>(
        &self,
        storage_bytes: u64,
        try_free_up: F,
    ) -> OwnedSemaphorePermit
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let permits = bytes_to_storage_permits(storage_bytes);
        loop {
            let available = self.semaphore.available_permits();
            let lock = self.priority_lock.lock().await;
            let result = self.semaphore.clone().try_acquire_many_owned(permits);
            drop(lock);
            match result {
                Ok(permit) => {
                    debug!(
                        "Acquired {} storage permits ({} bytes) of {}, new available: {}, permit size: {}",
                        permits,
                        storage_bytes,
                        available,
                        self.semaphore.available_permits(),
                        permit.num_permits()
                    );
                    break permit;
                }
                Err(TryAcquireError::Closed) => panic!("worker storage semaphore has been closed"),
                Err(TryAcquireError::NoPermits) => {
                    debug!(
                        "Not enough storage to allocate {} permits (available: {}), trying to free some up",
                        permits,
                        self.semaphore.available_permits()
                    );
                    if try_free_up().await {
                        debug!("Freed up some storage, retrying");
                        continue;
                    } else {
                        debug!("Could not free up storage, retrying after some time");
                        tokio::time::sleep(self.acquire_retry_delay).await;
                    }
                }
            }
        }
    }

    /// Non-blocking priority acquire. Grabs the priority lock to interrupt any
    /// in-progress blocking `acquire` loops, then attempts once.
    ///
    /// Returns `None` if `storage_bytes` are not available even after
    /// interrupting waiting acquires.
    pub(crate) async fn try_acquire(&self, storage_bytes: u64) -> Option<OwnedSemaphorePermit> {
        let permits = bytes_to_storage_permits(storage_bytes);
        let mut lock = None;
        loop {
            match self.semaphore.clone().try_acquire_many_owned(permits) {
                Ok(permit) => {
                    debug!(
                        "Acquired {} storage permits ({} bytes), available now: {}",
                        permits,
                        storage_bytes,
                        self.semaphore.available_permits()
                    );
                    break Some(permit);
                }
                Err(TryAcquireError::Closed) => panic!("worker storage semaphore has been closed"),
                Err(TryAcquireError::NoPermits) => {
                    if lock.is_none() {
                        debug!(
                            "Not enough storage to acquire {} permits (available: {}), cancelling waiting acquires and retry",
                            permits,
                            self.semaphore.available_permits()
                        );
                        lock = Some(self.priority_lock.lock().await);
                        continue;
                    } else {
                        debug!(
                            "Not enough storage to acquire {} permits (available: {})",
                            permits,
                            self.semaphore.available_permits()
                        );
                        break None;
                    }
                }
            }
        }
    }

    /// Release a number of bytes back to the pool without dropping the entire
    /// permit. Used when a file is partially freed (e.g. truncation).
    ///
    /// Adds `storage_bytes` worth of permits back to the semaphore directly.
    /// The caller is responsible for ensuring they don't release more than they
    /// acquired (no underflow protection at this level).
    pub(crate) fn release(&self, storage_bytes: u64) {
        let permits = bytes_to_storage_permits(storage_bytes);
        if permits > 0 {
            self.semaphore.add_permits(permits as usize);
        }
    }
}

/// Holds the metadata and wasmtime structures of currently active Golem workers
pub struct ActiveWorkers<Ctx: WorkerCtx> {
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    worker_memory: Arc<Semaphore>,
    worker_storage: Arc<StorageSemaphore>,
    priority_allocation_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
}

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn new(memory_config: &MemoryConfig, storage_config: &StorageConfig) -> Self {
        let worker_memory_size = memory_config.worker_memory();
        Self {
            workers: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "active_workers",
            ),
            worker_memory: Arc::new(Semaphore::new(worker_memory_size)),
            worker_storage: Arc::new(StorageSemaphore::new(
                storage_config.worker_storage(),
                storage_config.acquire_retry_delay,
            )),
            acquire_retry_delay: memory_config.acquire_retry_delay,
            priority_allocation_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn get_or_add<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        account_id: AccountId,
        worker_env: Option<Vec<(String, String)>>,
        worker_config_vars: Option<BTreeMap<String, String>>,
        worker_agent_config: Vec<WorkerAgentConfigEntry>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let agent_id = owned_agent_id.agent_id();

        let owned_agent_id = owned_agent_id.clone();
        let deps = deps.clone();
        let invocation_context_stack = invocation_context_stack.clone();
        self.workers
            .get_or_insert_simple(&agent_id, || {
                Box::pin(async move {
                    Ok(Arc::new(
                        Worker::new(
                            &deps,
                            &account_id,
                            owned_agent_id,
                            worker_env,
                            worker_config_vars,
                            worker_agent_config,
                            component_revision,
                            parent,
                            &invocation_context_stack,
                            principal,
                        )
                        .in_current_span()
                        .await?,
                    ))
                })
            })
            .await
    }

    pub async fn try_get(&self, owned_agent_id: &OwnedAgentId) -> Option<Arc<Worker<Ctx>>> {
        let agent_id = owned_agent_id.agent_id();
        self.workers.get(&agent_id).await
    }

    pub async fn remove(&self, agent_id: &AgentId) {
        self.workers.remove(agent_id).await;
    }

    pub async fn snapshot(&self) -> Vec<(AgentId, Arc<Worker<Ctx>>)> {
        self.workers.iter().await
    }

    pub async fn acquire(&self, memory: u64) -> OwnedSemaphorePermit {
        let mem32: u32 = memory
            .try_into()
            .expect("requested memory size is too large");

        loop {
            let available = self.worker_memory.available_permits();
            let lock = self.priority_allocation_lock.lock().await; // Block trying until a priority request is retrying once
            let result = self.worker_memory.clone().try_acquire_many_owned(mem32);
            drop(lock);
            match result {
                Ok(permit) => {
                    debug!(
                        "Acquired {} memory of {}, new available: {}, permit size: {}",
                        mem32,
                        available,
                        self.worker_memory.available_permits(),
                        permit.num_permits()
                    );
                    break permit;
                }
                Err(TryAcquireError::Closed) => panic!("worker memory semaphore has been closed"),
                Err(TryAcquireError::NoPermits) => {
                    debug!("Not enough memory to allocate {mem32} (available: {}), trying to free some up", self.worker_memory.available_permits());
                    if self.try_free_up_memory(memory).await {
                        debug!("Freed up some memory, retrying");
                        // We have enough memory unless another worker has taken it in the meantime,
                        // so retry the loop
                        continue;
                    } else {
                        debug!(
                            "Could not free up memory, retrying asking for permits after some time"
                        );
                        // Could not free up enough memory, so waiting for permits to be available.
                        // We cannot use acquire_many() to wait for the permits because it eagerly preallocates
                        // the available permits, and by that causing deadlocks. So we sleep and retry.

                        tokio::time::sleep(self.acquire_retry_delay).await;
                    }
                }
            }
        }
    }

    pub async fn try_acquire(&self, memory: u64) -> Option<OwnedSemaphorePermit> {
        let mem32: u32 = memory
            .try_into()
            .expect("requested memory size is too large");
        let mut lock = None;
        loop {
            match self.worker_memory.clone().try_acquire_many_owned(mem32) {
                Ok(permit) => {
                    debug!(
                        "Acquired {} memory of {}",
                        mem32,
                        self.worker_memory.available_permits()
                    );
                    break Some(permit);
                }
                Err(TryAcquireError::Closed) => panic!("worker memory semaphore has been closed"),
                Err(TryAcquireError::NoPermits) => {
                    if lock.is_none() {
                        debug!(
                            "Not enough available memory to acquire {mem32} (available: {}), cancelling waiting acquires and retry",
                            self.worker_memory.available_permits()
                        );
                        lock = Some(self.priority_allocation_lock.lock().await);
                        continue;
                    } else {
                        debug!(
                            "Not enough available memory to acquire {mem32} (available: {})",
                            self.worker_memory.available_permits()
                        );
                        break None;
                    }
                }
            }
        }
    }

    async fn try_free_up_memory(&self, memory: u64) -> bool {
        let current_avail = self.worker_memory.available_permits();
        let needed = memory.saturating_sub(current_avail as u64);

        if needed > 0 {
            let mut possibilities = Vec::new();

            debug!("Collecting possibilities");
            // Collecting the workers which are currently idle but loaded into memory
            let pairs = self.workers.iter().await;
            for (agent_id, worker) in pairs {
                if worker.is_currently_idle_but_running().await {
                    if let Ok(mem) = worker.memory_requirement().await {
                        let last_changed = worker.last_execution_state_change();
                        possibilities.push((agent_id, worker, mem, last_changed));
                    }
                }
            }

            // Sorting them by last time they changed their status - newest first
            possibilities
                .sort_by_key(|(_agent_id, _worker, _mem, last_changed)| last_changed.to_millis());
            possibilities.reverse();

            let mut freed = 0;

            // Dropping the oldest ones until we have enough memory available - rechecking the idle status before
            while freed < needed && !possibilities.is_empty() {
                let (agent_id, worker, mem, _) = possibilities.pop().unwrap();

                debug!("Trying to stop {agent_id} to free up memory");
                if worker.stop_if_idle().await {
                    debug!("Stopped {agent_id} to free up {mem} memory");
                    freed += mem;
                }
            }

            if freed > 0 {
                debug!("Freed up {freed}");
            }
            freed >= needed
        } else {
            debug!("Memory was freed up in the meantime");
            // Memory was freed up in the meantime, we can retry
            true
        }
    }

    /// Blocking acquire of storage semaphore permits. Loops until the requested
    /// number of bytes is available, evicting idle workers as needed.
    pub async fn acquire_storage(&self, storage_bytes: u64) -> OwnedSemaphorePermit {
        let workers = self.workers.clone();
        self.worker_storage
            .acquire(storage_bytes, || {
                let workers = workers.clone();
                async move { Self::try_free_up_storage_inner(&workers, storage_bytes).await }
            })
            .await
    }

    /// Non-blocking, priority storage acquire. Grabs the allocation lock to
    /// interrupt any ongoing blocking `acquire_storage` loops, then attempts once.
    ///
    /// Returns `None` if the requested storage is not available even after
    /// interrupting waiting acquires.
    pub async fn try_acquire_storage(&self, storage_bytes: u64) -> Option<OwnedSemaphorePermit> {
        self.worker_storage.try_acquire(storage_bytes).await
    }

    /// Return `freed_bytes` to the storage pool without dropping the whole permit.
    /// Used when a file is deleted or truncated
    pub fn release_storage(&self, freed_bytes: u64) {
        self.worker_storage.release(freed_bytes);
    }

    pub fn storage_semaphore(&self) -> Arc<StorageSemaphore> {
        self.worker_storage.clone()
    }

    async fn try_free_up_storage_inner(
        workers: &Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
        storage_bytes: u64,
    ) -> bool {
        let permits_needed = bytes_to_storage_permits(storage_bytes) as u64;
        let mut possibilities = Vec::new();

        debug!("Collecting storage eviction possibilities");
        for (agent_id, worker) in workers.iter().await {
            if worker.is_currently_idle_but_running().await {
                if let Ok(memory) = worker.memory_requirement().await {
                    let last_changed = worker.last_execution_state_change();
                    possibilities.push((agent_id, worker, memory, last_changed));
                }
            }
        }

        // Evict oldest-idle first
        possibilities
            .sort_by_key(|(_agent_id, _worker, _memory, last_changed)| last_changed.to_millis());
        possibilities.reverse();

        let mut evicted = 0u64;
        while evicted < permits_needed && !possibilities.is_empty() {
            let (agent_id, worker, _memory, _) = possibilities.pop().unwrap();
            debug!("Trying to stop {agent_id} to free up storage");
            if worker.stop_if_idle().await {
                debug!("Stopped {agent_id} to free up storage");
                evicted += 1;
            }
        }

        if evicted > 0 {
            debug!("Evicted {evicted} worker(s) to free storage; re-checking availability");
        }
        // Actual freed bytes come from storage_permit being dropped on stop
        evicted >= permits_needed
    }
}

/// One storage semaphore permit represents this many kilobytes. Using KB units
/// keeps the permit count within `u32` range while supporting up to ~4 TB of
/// addressable storage space (4_294_967_295 KB ≈ 4 TB).
pub const STORAGE_PERMIT_SIZE_KB: u64 = 1;

/// Convert a byte count to the number of storage semaphore permits needed,
/// rounding up so that partial kilobytes always consume a full permit.
pub fn bytes_to_storage_permits(bytes: u64) -> u32 {
    let kb = bytes.div_ceil(STORAGE_PERMIT_SIZE_KB * 1024);
    kb.min(u32::MAX as u64) as u32
}

/// Convert a storage semaphore pool size in bytes to the number of permits to
/// initialise the semaphore with.
pub fn storage_pool_bytes_to_permits(bytes: usize) -> usize {
    bytes.div_ceil(STORAGE_PERMIT_SIZE_KB as usize * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    test_r::enable!();

    #[test]
    fn bytes_to_permits_exact_kb_boundary() {
        assert_eq!(bytes_to_storage_permits(1024), 1);
    }

    #[test]
    fn bytes_to_permits_rounds_up_partial_kb() {
        assert_eq!(bytes_to_storage_permits(1), 1);
        assert_eq!(bytes_to_storage_permits(1025), 2);
    }

    #[test]
    fn bytes_to_permits_zero_bytes() {
        assert_eq!(bytes_to_storage_permits(0), 0);
    }

    #[test]
    fn bytes_to_permits_1gb() {
        assert_eq!(bytes_to_storage_permits(1024 * 1024 * 1024), 1_048_576);
    }

    #[test]
    fn bytes_to_permits_very_large_saturates_at_u32_max() {
        assert_eq!(bytes_to_storage_permits(u64::MAX), u32::MAX);
    }

    #[test]
    fn bytes_to_permits_just_under_4tb() {
        let just_under: u64 = (u32::MAX as u64) * 1024;
        assert_eq!(bytes_to_storage_permits(just_under), u32::MAX);
    }

    #[test]
    fn storage_pool_permits_10gb() {
        let ten_gb: usize = 10 * 1024 * 1024 * 1024;
        assert_eq!(storage_pool_bytes_to_permits(ten_gb), 10 * 1024 * 1024);
    }

    fn storage_semaphore(pool_bytes: usize) -> StorageSemaphore {
        StorageSemaphore::new(pool_bytes, Duration::from_millis(1))
    }

    #[test]
    async fn try_acquire_succeeds_when_space_available() {
        let storage_semaphore = storage_semaphore(4 * 1024); // 4 KB pool
        let permit = storage_semaphore.try_acquire(2 * 1024).await; // ask for 2 KB
        assert!(permit.is_some());
        assert_eq!(storage_semaphore.available_bytes(), 2 * 1024);
    }

    #[test]
    async fn try_acquire_returns_none_when_pool_exhausted() {
        let storage_semaphore = storage_semaphore(2 * 1024); // 2 KB pool
        let _permit = storage_semaphore.try_acquire(2 * 1024).await.unwrap(); // exhaust it
        let second = storage_semaphore.try_acquire(1024).await; // no space left
        assert!(second.is_none());
    }

    #[test]
    async fn try_acquire_zero_bytes_always_succeeds() {
        let storage_semaphore = storage_semaphore(0); // empty pool — 0 bytes → 0 permits
        let permit = storage_semaphore.try_acquire(0).await;
        assert!(permit.is_some());
    }

    #[test]
    async fn dropping_permit_returns_space_to_pool() {
        let storage_semaphore = storage_semaphore(4 * 1024);
        {
            let _permit = storage_semaphore.try_acquire(4 * 1024).await.unwrap();
            assert_eq!(storage_semaphore.available_bytes(), 0);
        } // permit dropped here
        assert_eq!(storage_semaphore.available_bytes(), 4 * 1024);
    }

    #[test]
    async fn multiple_permits_are_independent() {
        let storage_semaphore = storage_semaphore(6 * 1024); // 6 KB pool
        let p1 = storage_semaphore.try_acquire(2 * 1024).await.unwrap();
        let p2 = storage_semaphore.try_acquire(2 * 1024).await.unwrap();
        assert_eq!(storage_semaphore.available_bytes(), 2 * 1024);
        drop(p1);
        assert_eq!(storage_semaphore.available_bytes(), 4 * 1024);
        drop(p2);
        assert_eq!(storage_semaphore.available_bytes(), 6 * 1024);
    }

    #[test]
    async fn try_acquire_rounds_up_to_kb_boundary() {
        let storage_semaphore = storage_semaphore(2 * 1024); // 2 KB = 2 permits
                                                             // 1 byte rounds up to 1 KB = 1 permit; should leave 1 KB
        let _p = storage_semaphore.try_acquire(1).await.unwrap();
        assert_eq!(storage_semaphore.available_bytes(), 1024);
    }

    #[test]
    async fn release_returns_bytes_without_dropping_permit() {
        let storage_semaphore = storage_semaphore(4 * 1024);
        let _permit = storage_semaphore.try_acquire(4 * 1024).await.unwrap(); // exhaust pool
        assert_eq!(storage_semaphore.available_bytes(), 0);
        storage_semaphore.release(2 * 1024); // release half back
        assert_eq!(storage_semaphore.available_bytes(), 2 * 1024);
    }

    #[test]
    async fn release_zero_is_a_noop() {
        let storage_semaphore = storage_semaphore(4 * 1024);
        let _permit = storage_semaphore.try_acquire(2 * 1024).await.unwrap();
        let before = storage_semaphore.available_bytes();
        storage_semaphore.release(0);
        assert_eq!(storage_semaphore.available_bytes(), before);
    }

    #[test]
    async fn acquire_succeeds_immediately_when_space_available() {
        let storage_semaphore = storage_semaphore(4 * 1024);
        // pool has space so it succeeds on the first try without invoking free_up
        let permit = storage_semaphore
            .acquire(2 * 1024, || async { false })
            .await;
        assert_eq!(permit.num_permits(), 2); // 2 KB = 2 permits
        assert_eq!(storage_semaphore.available_bytes(), 2 * 1024);
    }

    #[test]
    async fn acquire_succeeds_after_free_up_releases_space() {
        let storage_semaphore = storage_semaphore(4 * 1024);
        let _held = storage_semaphore.try_acquire(4 * 1024).await.unwrap(); // exhaust pool

        // Share the inner semaphore Arc with the closure so it can add permits
        // back to simulate a worker releasing its storage on eviction.
        let sem_arc = storage_semaphore.semaphore.clone();
        let released = std::sync::atomic::AtomicBool::new(false);
        let permit = storage_semaphore
            .acquire(2 * 1024, || {
                let sem = sem_arc.clone();
                let already = released.fetch_or(true, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if !already {
                        sem.add_permits(2); // 2 permits = 2 KB freed
                        true
                    } else {
                        false
                    }
                }
            })
            .await;
        assert_eq!(permit.num_permits(), 2);
    }
}
