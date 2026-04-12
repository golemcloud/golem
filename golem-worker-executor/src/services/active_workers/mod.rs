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

pub mod concurrent_agents_scheduler;
pub mod concurrent_agents_semaphore;
pub mod fs_semaphore;
#[cfg(test)]
mod tests;

pub use concurrent_agents_scheduler::{ConcurrentAgentPermit, ConcurrentAgentsScheduler};
pub use concurrent_agents_semaphore::ConcurrentAgentsSemaphore;
pub use fs_semaphore::{
    FILESYSTEM_STORAGE_PERMIT_SIZE_KB, FilesystemStorageSemaphore,
    bytes_to_filesystem_storage_permits, filesystem_storage_bytes_rounded_up,
    filesystem_storage_permits_to_bytes, filesystem_storage_pool_bytes_to_permits,
};

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError};

use tracing::{Instrument, debug};

use crate::services::HasAll;
use crate::services::golem_config::{FilesystemStorageConfig, MemoryConfig};
use crate::services::resource_limits::AtomicResourceEntry;
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

/// Holds the metadata and wasmtime structures of currently active Golem workers
pub struct ActiveWorkers<Ctx: WorkerCtx> {
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    worker_memory: Arc<Semaphore>,
    worker_filesystem_storage: Arc<FilesystemStorageSemaphore>,
    concurrent_agents: Arc<ConcurrentAgentsScheduler>,
    priority_allocation_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
}

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn new(memory_config: &MemoryConfig, storage_config: &FilesystemStorageConfig) -> Self {
        let worker_memory_size = memory_config.worker_memory();
        Self {
            workers: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "active_workers",
            ),
            worker_memory: Arc::new(Semaphore::new(worker_memory_size)),
            worker_filesystem_storage: Arc::new(FilesystemStorageSemaphore::new(
                storage_config.worker_filesystem_storage(),
                storage_config.acquire_retry_delay,
            )),
            concurrent_agents: Arc::new(ConcurrentAgentsScheduler::new()),
            acquire_retry_delay: memory_config.acquire_retry_delay,
            priority_allocation_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn get_or_add<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
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
                    debug!(
                        "Not enough memory to allocate {mem32} (available: {}), trying to free some up",
                        self.worker_memory.available_permits()
                    );
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
            let mut idle_candidates = Vec::new();
            let mut warm_candidates = Vec::new();

            debug!("Collecting memory eviction candidates");
            let pairs = self.workers.iter().await;
            for (agent_id, worker) in pairs {
                if let Some(class) = worker.eviction_class().await
                    && let Ok(mem) = worker.memory_requirement().await
                {
                    let last_changed = worker.last_execution_state_change();
                    let entry = (agent_id, worker, mem, last_changed);
                    match class {
                        crate::worker::EvictionClass::LoadedIdle => {
                            idle_candidates.push(entry);
                        }
                        crate::worker::EvictionClass::WarmRunnable => {
                            warm_candidates.push(entry);
                        }
                    }
                }
            }

            // Sort each bucket by timestamp — newest first so we pop oldest
            idle_candidates
                .sort_by_key(|(_, _, _, ts)| ts.to_millis());
            idle_candidates.reverse();
            warm_candidates
                .sort_by_key(|(_, _, _, ts)| ts.to_millis());
            warm_candidates.reverse();

            let mut freed = 0u64;

            // First evict LoadedIdle workers (cheapest)
            while freed < needed && !idle_candidates.is_empty() {
                let (agent_id, worker, mem, _) = idle_candidates.pop().unwrap();
                debug!("Trying to stop idle {agent_id} to free up memory");
                if worker
                    .stop_if_evictable(crate::worker::EvictionClass::LoadedIdle)
                    .await
                {
                    debug!("Stopped idle {agent_id} to free up {mem} memory");
                    freed += mem;
                }
            }

            // Then evict WarmRunnable workers if still under pressure
            while freed < needed && !warm_candidates.is_empty() {
                let (agent_id, worker, mem, _) = warm_candidates.pop().unwrap();
                debug!("Trying to stop warm-runnable {agent_id} to free up memory");
                if worker
                    .stop_if_evictable(crate::worker::EvictionClass::WarmRunnable)
                    .await
                {
                    debug!("Stopped warm-runnable {agent_id} to free up {mem} memory");
                    freed += mem;
                }
            }

            if freed > 0 {
                debug!("Freed up {freed}");
            }
            freed >= needed
        } else {
            debug!("Memory was freed up in the meantime");
            true
        }
    }

    /// Blocking acquire of storage semaphore permits. Loops until the requested
    /// number of bytes is available, evicting idle workers as needed.
    pub async fn acquire_filesystem_storage(&self, storage_bytes: u64) -> OwnedSemaphorePermit {
        let workers = self.workers.clone();
        self.worker_filesystem_storage
            .acquire(storage_bytes, || {
                let workers = workers.clone();
                async move { Self::try_free_up_filesystem_storage(&workers, storage_bytes).await }
            })
            .await
    }

    /// Non-blocking, priority storage acquire. Grabs the allocation lock to
    /// interrupt any ongoing blocking `acquire_storage` loops, then attempts once.
    ///
    /// Returns `None` if the requested storage is not available even after
    /// interrupting waiting acquires.
    pub async fn try_acquire_filesystem_storage(
        &self,
        storage_bytes: u64,
    ) -> Option<OwnedSemaphorePermit> {
        self.worker_filesystem_storage
            .try_acquire(storage_bytes)
            .await
    }

    pub fn filesystem_storage_semaphore(&self) -> Arc<FilesystemStorageSemaphore> {
        self.worker_filesystem_storage.clone()
    }

    /// Register an account with the per-account concurrent agent semaphore.
    ///
    /// Must be called (from `Worker::new`) before any `acquire_concurrent_agent`
    /// call for the account. Idempotent — safe to call multiple times.
    pub async fn register_account_concurrency(
        &self,
        account_id: AccountId,
        resource_entry: Arc<AtomicResourceEntry>,
    ) {
        self.concurrent_agents
            .register_account(account_id, resource_entry)
            .await;
    }

    /// Blocking acquire of one concurrent-agent permit for `account_id`,
    /// respecting FIFO ordering within the account.
    ///
    /// Only actively running agents hold permits — idle agents release theirs
    /// back to the pool. In the common case permits are already available and
    /// the acquire succeeds immediately.
    ///
    /// If all permits are held by actively running agents, the agent is queued
    /// in the per-account FIFO scheduler and waits until a running agent
    /// finishes and returns its permit. This ensures fairness: a worker that
    /// finishes and re-requests a slot goes to the back of the queue.
    ///
    /// Returns immediately (zero-cost permit) for accounts whose plan limit is
    /// at or above the unlimited sentinel.
    pub async fn acquire_concurrent_agent(
        &self,
        account_id: AccountId,
        agent_id: AgentId,
    ) -> ConcurrentAgentPermit {
        self.concurrent_agents
            .acquire(account_id, agent_id)
            .await
    }

    async fn try_free_up_filesystem_storage(
        workers: &Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
        storage_bytes: u64,
    ) -> bool {
        let mut idle_candidates = Vec::new();
        let mut warm_candidates = Vec::new();

        debug!("Collecting storage eviction candidates");
        for (agent_id, worker) in workers.iter().await {
            if let Some(class) = worker.eviction_class().await
                && let Ok(storage) = worker.filesystem_storage_requirement().await
            {
                let last_changed = worker.last_execution_state_change();
                let entry = (agent_id, worker, storage, last_changed);
                match class {
                    crate::worker::EvictionClass::LoadedIdle => idle_candidates.push(entry),
                    crate::worker::EvictionClass::WarmRunnable => warm_candidates.push(entry),
                }
            }
        }

        // Sort each bucket — newest first so we pop oldest
        idle_candidates.sort_by_key(|(_, _, _, ts)| ts.to_millis());
        idle_candidates.reverse();
        warm_candidates.sort_by_key(|(_, _, _, ts)| ts.to_millis());
        warm_candidates.reverse();

        let mut freed: u64 = 0;

        // First evict LoadedIdle workers
        while freed < storage_bytes && !idle_candidates.is_empty() {
            let (agent_id, worker, storage, _) = idle_candidates.pop().unwrap();
            debug!("Trying to stop idle {agent_id} to free up storage");
            if worker
                .stop_if_evictable(crate::worker::EvictionClass::LoadedIdle)
                .await
            {
                debug!("Stopped idle {agent_id}, freed {storage} bytes of storage");
                freed += storage;
            }
        }

        // Then evict WarmRunnable workers if still under pressure
        while freed < storage_bytes && !warm_candidates.is_empty() {
            let (agent_id, worker, storage, _) = warm_candidates.pop().unwrap();
            debug!("Trying to stop warm-runnable {agent_id} to free up storage");
            if worker
                .stop_if_evictable(crate::worker::EvictionClass::WarmRunnable)
                .await
            {
                debug!("Stopped warm-runnable {agent_id}, freed {storage} bytes of storage");
                freed += storage;
            }
        }

        if freed > 0 {
            debug!("Freed {freed} bytes by evicting worker(s); re-checking availability");
        }
        freed >= storage_bytes
    }
}
