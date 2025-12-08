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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError};

use tracing::{debug, Instrument};

use crate::services::golem_config::MemoryConfig;
use crate::services::HasAll;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{OwnedWorkerId, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;

/// Holds the metadata and wasmtime structures of currently active Golem workers
pub struct ActiveWorkers<Ctx: WorkerCtx> {
    workers: Cache<WorkerId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    worker_memory: Arc<Semaphore>,
    priority_allocation_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
}

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn new(memory_config: &MemoryConfig) -> Self {
        let worker_memory_size = memory_config.worker_memory();
        Self {
            workers: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "active_workers",
            ),
            worker_memory: Arc::new(Semaphore::new(worker_memory_size)),
            acquire_retry_delay: memory_config.acquire_retry_delay,
            priority_allocation_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn get_or_add<T>(
        &self,
        deps: &T,
        owned_worker_id: &OwnedWorkerId,
        account_id: &AccountId,
        worker_env: Option<Vec<(String, String)>>,
        worker_wasi_config_vars: Option<BTreeMap<String, String>>,
        component_version: Option<ComponentRevision>,
        parent: Option<WorkerId>,
        invocation_context_stack: &InvocationContextStack,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let worker_id = owned_worker_id.worker_id();

        let owned_worker_id = owned_worker_id.clone();
        let account_id = *account_id;
        let deps = deps.clone();
        let invocation_context_stack = invocation_context_stack.clone();
        self.workers
            .get_or_insert_simple(&worker_id, || {
                Box::pin(async move {
                    Ok(Arc::new(
                        Worker::new(
                            &deps,
                            &account_id,
                            owned_worker_id,
                            worker_env,
                            worker_wasi_config_vars,
                            component_version,
                            parent,
                            &invocation_context_stack,
                        )
                        .in_current_span()
                        .await?,
                    ))
                })
            })
            .await
    }

    pub async fn try_get(&self, owned_worker_id: &OwnedWorkerId) -> Option<Arc<Worker<Ctx>>> {
        let worker_id = owned_worker_id.worker_id();
        self.workers.get(&worker_id).await
    }

    pub async fn remove(&self, worker_id: &WorkerId) {
        self.workers.remove(worker_id).await;
    }

    pub async fn snapshot(&self) -> Vec<(WorkerId, Arc<Worker<Ctx>>)> {
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
            for (worker_id, worker) in pairs {
                if worker.is_currently_idle_but_running().await {
                    if let Ok(mem) = worker.memory_requirement().await {
                        let last_changed = worker.last_execution_state_change();
                        possibilities.push((worker_id, worker, mem, last_changed));
                    }
                }
            }

            // Sorting them by last time they changed their status - newest first
            possibilities
                .sort_by_key(|(_worker_id, _worker, _mem, last_changed)| last_changed.to_millis());
            possibilities.reverse();

            let mut freed = 0;

            // Dropping the oldest ones until we have enough memory available - rechecking the idle status before
            while freed < needed && !possibilities.is_empty() {
                let (worker_id, worker, mem, _) = possibilities.pop().unwrap();

                debug!("Trying to stop {worker_id} to free up memory");
                if worker.stop_if_idle().await {
                    debug!("Stopped {worker_id} to free up {mem} memory");
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
}
