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

pub mod admission;
pub mod component_charge;
pub mod concurrent_agents_scheduler;
pub mod concurrent_agents_semaphore;
pub mod fs_semaphore;
pub mod memory_probe;
#[cfg(test)]
mod tests;

pub use concurrent_agents_scheduler::{ConcurrentAgentPermit, ConcurrentAgentsScheduler};
pub use concurrent_agents_semaphore::ConcurrentAgentsSemaphore;
pub use fs_semaphore::{
    FILESYSTEM_STORAGE_PERMIT_SIZE_KB, FilesystemStoragePermit, FilesystemStorageSemaphore,
    bytes_to_filesystem_storage_permits, filesystem_storage_bytes_rounded_up,
    filesystem_storage_permits_to_bytes, filesystem_storage_pool_bytes_to_permits,
};

use admission::{AdmissionController, AdmissionDecision, EvictionPriority, EvictionSource};
use async_trait::async_trait;
pub use component_charge::HeldComponentCharge;
use component_charge::{ChargeSource, ComponentChargeGuard, ComponentChargeRegistry};
use memory_probe::default_probe;
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
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentId, OwnedAgentId, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::error::worker_executor::WorkerExecutorError;

/// Capability proving that per-account concurrent-agent state has been registered
/// in this executor and can be used for subsequent permit acquires.
#[derive(Clone)]
pub(crate) struct RegisteredConcurrentAccount {
    scheduler: Arc<ConcurrentAgentsScheduler>,
    account_id: AccountId,
}

impl RegisteredConcurrentAccount {
    pub(crate) async fn acquire(&self, agent_id: AgentId) -> ConcurrentAgentPermit {
        self.scheduler.acquire(self.account_id, agent_id).await
    }
}

/// Holds the metadata and wasmtime structures of currently active Golem workers
pub struct ActiveWorkers<Ctx: WorkerCtx> {
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    worker_memory: Arc<Semaphore>,
    worker_filesystem_storage: Arc<FilesystemStorageSemaphore>,
    concurrent_agents: Arc<ConcurrentAgentsScheduler>,
    priority_allocation_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
    /// Authoritative measured-headroom admission gate. Decides whether real
    /// memory headroom permits a new acquisition, evicting via the worker set
    /// when short, and is what refuses admission in normal operation. The
    /// estimate-based `worker_memory` semaphore is the second line of defence
    /// behind it: its atomic permit acquisition catches the concurrent
    /// admissions the lockless gate can let through on the same snapshot. `None`
    /// when measured admission is disabled (e.g. shared test environments) —
    /// admission then relies on the estimate semaphore alone.
    admission: Option<AdmissionController>,
    /// Charges each resident component's compiled module size to the estimate
    /// pool exactly once (shared across all its workers) rather than per worker.
    component_charges:
        Arc<ComponentChargeRegistry<ComponentChargeKey, MemoryPoolChargeSource<Ctx>>>,
    /// Multiplier applied to a component's `component_size` when sizing its
    /// module charge permit.
    component_size_coefficient: f64,
}

/// Identifies a compiled component for module-charge accounting.
type ComponentChargeKey = (ComponentId, ComponentRevision);

/// Guard held by a resident worker keeping its component's module charge alive.
pub type WorkerComponentCharge<Ctx> =
    ComponentChargeGuard<ComponentChargeKey, MemoryPoolChargeSource<Ctx>>;

#[derive(Debug)]
pub struct WorkerMemoryPermit {
    permit: Option<OwnedSemaphorePermit>,
}

impl WorkerMemoryPermit {
    fn new(permit: OwnedSemaphorePermit) -> Self {
        crate::metrics::workers::record_memory_permit_acquired(permit.num_permits());
        Self {
            permit: Some(permit),
        }
    }

    pub fn num_permits(&self) -> usize {
        self.permit
            .as_ref()
            .map_or(0, |permit| permit.num_permits())
    }

    pub fn merge(&mut self, mut other: Self) {
        if let Some(other_permit) = other.permit.take() {
            match &mut self.permit {
                Some(permit) => permit.merge(other_permit),
                None => self.permit = Some(other_permit),
            }
        }
    }
}

impl Drop for WorkerMemoryPermit {
    fn drop(&mut self) {
        crate::metrics::workers::record_memory_permit_released(self.num_permits());
    }
}

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn new(memory_config: &MemoryConfig, storage_config: &FilesystemStorageConfig) -> Self {
        // Build the probe once and size both admission layers from its reported
        // limit, so the estimate semaphore and the measured-headroom gate share
        // a single basis (the pod's cgroup limit when constrained, not host RAM).
        let probe = default_probe(memory_config.system_memory_override);
        let worker_memory_size = memory_config.worker_memory_for_limit(probe.limit_bytes());
        let admission = memory_config
            .enable_measured_admission
            .then(|| AdmissionController::new(probe, memory_config.admission_policy()));
        let workers = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            "active_workers",
        );
        let worker_memory = Arc::new(Semaphore::new(worker_memory_size));
        let priority_allocation_lock = Arc::new(Mutex::new(()));
        let component_charges = ComponentChargeRegistry::new(MemoryPoolChargeSource {
            worker_memory: worker_memory.clone(),
            workers: workers.clone(),
            priority_allocation_lock: priority_allocation_lock.clone(),
            acquire_retry_delay: memory_config.acquire_retry_delay,
        });
        let active_workers = Self {
            workers,
            worker_memory,
            worker_filesystem_storage: Arc::new(FilesystemStorageSemaphore::new(
                storage_config.worker_filesystem_storage(),
                storage_config.acquire_retry_delay,
            )),
            concurrent_agents: Arc::new(ConcurrentAgentsScheduler::new()),
            acquire_retry_delay: memory_config.acquire_retry_delay,
            priority_allocation_lock,
            admission,
            component_charges,
            component_size_coefficient: memory_config.component_size_coefficient,
        };
        active_workers.initialize_metrics(worker_memory_size);
        active_workers
    }

    /// Acquire (or share) the per-component module charge for a worker of the
    /// given component. The first resident worker of the component pays its
    /// compiled-module size (scaled by `component_size_coefficient`) into the
    /// estimate pool; subsequent workers share the same charge. The returned
    /// guard releases residency on drop, and the charge is freed when the last
    /// worker of the component unloads.
    pub async fn acquire_component_charge(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component_module_bytes: u64,
    ) -> WorkerComponentCharge<Ctx> {
        let charge_bytes = (self.component_size_coefficient * component_module_bytes as f64) as u64;
        self.component_charges
            .acquire((component_id, component_revision), charge_bytes)
            .await
    }

    pub async fn get_or_add<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
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
                    let worker = Arc::new(
                        Worker::new(
                            &deps,
                            owned_agent_id,
                            worker_env,
                            worker_agent_config,
                            component_revision,
                            parent,
                            &invocation_context_stack,
                            principal,
                        )
                        .in_current_span()
                        .await?,
                    );
                    Ok(worker)
                })
            })
            .await
    }

    pub async fn try_get(&self, owned_agent_id: &OwnedAgentId) -> Option<Arc<Worker<Ctx>>> {
        let agent_id = owned_agent_id.agent_id();
        self.workers.get(&agent_id).await
    }

    pub async fn remove(&self, agent_id: &AgentId) {
        self.workers.remove(agent_id).await
    }

    pub async fn snapshot(&self) -> Vec<(AgentId, Arc<Worker<Ctx>>)> {
        self.workers.iter().await
    }

    /// Interrupts and unloads all in-memory workers whose environment matches
    /// `environment_id`.  Called when the environment is deleted so that
    /// running workers stop promptly.
    pub async fn unload_environment(&self, environment_id: EnvironmentId) {
        for (_agent_id, worker) in self.snapshot().await {
            if worker.get_initial_worker_metadata().environment_id == environment_id {
                if let Some(mut await_interrupted) = worker
                    .set_interrupting(InterruptKind::Interrupt(Timestamp::now_utc()))
                    .await
                {
                    await_interrupted.recv().await.unwrap();
                }
                self.remove(&worker.agent_id()).await;
            }
        }
    }

    pub async fn acquire(&self, memory: u64) -> WorkerMemoryPermit {
        let mem32: u32 = memory
            .try_into()
            .expect("requested memory size is too large");

        loop {
            // Blocking acquire: retry until the request can be admitted. A
            // rejection here is transient, not terminal. The gate reads resident
            // memory from the probe, which lags real usage (cgroup
            // `memory.current` only counts already-touched pages), so a worker
            // admitted earlier may not yet be fully resident; pressure eases as
            // its pages settle and as other workers finish and release pool
            // permits. Each iteration backs off, re-reads the gate, and re-tries
            // the pool, so the caller eventually proceeds once headroom recovers
            // rather than failing under momentary pressure.
            // Authoritative measured-headroom gate (when enabled). Evicts
            // idle-then-warm when real headroom is short; rejects (and we back
            // off) when it cannot make room rather than risking the limit.
            if let Some(admission) = &self.admission
                && admission.try_admit(memory, &self.eviction_source()).await
                    == AdmissionDecision::Reject
            {
                debug!("Measured headroom insufficient for {mem32}, backing off and retrying");
                tokio::time::sleep(self.acquire_retry_delay).await;
                continue;
            }

            // Estimate-semaphore pool: the second line of defence behind the
            // gate. Its atomic permit acquisition catches the concurrent
            // admissions the lockless gate can let through on the same snapshot.
            // Sized above the gate ceiling (but clamped below the limit), so it
            // rarely binds first — the gate refuses in normal operation.
            if let Some(permit) = acquire_pool_permit(
                &self.worker_memory,
                &self.workers,
                &self.priority_allocation_lock,
                self.acquire_retry_delay,
                mem32,
                memory,
            )
            .await
            {
                break permit;
            }
            // Pool could not satisfy the estimate even after eviction; loop and
            // re-run the gate before trying again.
        }
    }

    /// Builds an [`EvictionSource`] view over the live worker set for the
    /// admission controller to reclaim memory through.
    fn eviction_source(&self) -> WorkerEvictionSource<Ctx> {
        WorkerEvictionSource {
            workers: self.workers.clone(),
        }
    }

    pub async fn try_acquire(&self, memory: u64) -> Option<WorkerMemoryPermit> {
        let mem32: u32 = memory
            .try_into()
            .expect("requested memory size is too large");

        // Authoritative measured-headroom gate (when enabled). Single attempt
        // (this is the non-blocking path): if real headroom is insufficient even
        // after eviction, do not admit.
        if let Some(admission) = &self.admission
            && admission.try_admit(memory, &self.eviction_source()).await
                == AdmissionDecision::Reject
        {
            debug!("Measured headroom insufficient for {mem32}, not admitting");
            return None;
        }

        let mut lock = None;
        loop {
            match self.worker_memory.clone().try_acquire_many_owned(mem32) {
                Ok(permit) => {
                    debug!(
                        "Acquired {} memory of {}",
                        mem32,
                        self.worker_memory.available_permits()
                    );
                    break Some(WorkerMemoryPermit::new(permit));
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

    /// Blocking acquire of storage semaphore permits. Loops until the requested
    /// number of bytes is available, evicting idle workers as needed.
    pub async fn acquire_filesystem_storage(&self, storage_bytes: u64) -> FilesystemStoragePermit {
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
    ) -> Option<FilesystemStoragePermit> {
        self.worker_filesystem_storage
            .try_acquire(storage_bytes)
            .await
    }

    pub fn filesystem_storage_semaphore(&self) -> Arc<FilesystemStorageSemaphore> {
        self.worker_filesystem_storage.clone()
    }

    /// Register an account with the per-account concurrent agent semaphore.
    ///
    /// Must be called (from `Worker::new`) before any concurrent-agent permit
    /// acquire for the account. Idempotent — safe to call multiple times.
    pub(crate) async fn register_account_concurrency(
        &self,
        account_id: AccountId,
        resource_entry: Arc<AtomicResourceEntry>,
    ) -> RegisteredConcurrentAccount {
        self.concurrent_agents
            .register_account(account_id, resource_entry)
            .await;

        RegisteredConcurrentAccount {
            scheduler: self.concurrent_agents.clone(),
            account_id,
        }
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
                crate::metrics::workers::record_worker_eviction("LoadedIdle");
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
                crate::metrics::workers::record_worker_eviction("WarmRunnable");
                freed += storage;
            }
        }

        if freed > 0 {
            debug!("Freed {freed} bytes by evicting worker(s); re-checking availability");
        }
        freed >= storage_bytes
    }

    /// Initializes worker gauges. Subsequent changes are recorded inline at the mutation sites.
    fn initialize_metrics(&self, worker_memory_size: usize) {
        crate::metrics::workers::initialize_worker_metrics();
        crate::metrics::workers::set_filesystem_semaphore_available(
            self.worker_filesystem_storage.available_bytes(),
        );
        crate::metrics::storage::record_worker_memory_pool_total(worker_memory_size as u64);
    }
}

impl From<EvictionPriority> for crate::worker::EvictionClass {
    fn from(priority: EvictionPriority) -> Self {
        match priority {
            EvictionPriority::Idle => crate::worker::EvictionClass::LoadedIdle,
            EvictionPriority::Warm => crate::worker::EvictionClass::WarmRunnable,
        }
    }
}

/// Evicts resident workers at a single priority tier, oldest-first, stopping
/// once at least `needed_bytes` have been freed or the tier is exhausted.
/// Returns the bytes actually reclaimed.
async fn evict_at_most_memory<Ctx: WorkerCtx>(
    workers: &Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    priority: EvictionPriority,
    needed_bytes: u64,
) -> u64 {
    let target_class: crate::worker::EvictionClass = priority.into();

    let mut candidates = Vec::new();
    for (agent_id, worker) in workers.iter().await {
        if let Some(class) = worker.eviction_class().await
            && class == target_class
            && let Ok(mem) = worker.memory_requirement().await
        {
            let last_changed = worker.last_execution_state_change();
            candidates.push((agent_id, worker, mem, last_changed));
        }
    }

    // Sort by timestamp newest-first so we pop the oldest first.
    candidates.sort_by_key(|(_, _, _, ts)| ts.to_millis());
    candidates.reverse();

    let mut freed = 0u64;
    while freed < needed_bytes && !candidates.is_empty() {
        let (agent_id, worker, mem, _) = candidates.pop().unwrap();
        debug!("Trying to stop {target_class:?} {agent_id} to free up memory");
        if worker.stop_if_evictable(target_class).await {
            debug!("Stopped {target_class:?} {agent_id} to free up {mem} memory");
            crate::metrics::workers::record_worker_eviction(match priority {
                EvictionPriority::Idle => "LoadedIdle",
                EvictionPriority::Warm => "WarmRunnable",
            });
            freed += mem;
        }
    }
    freed
}

/// Frees up to `memory` estimate-permit bytes by evicting idle-then-warm
/// workers, accounting for permits already available. Returns true when enough
/// is (or was already) free.
async fn try_free_up_pool_memory<Ctx: WorkerCtx>(
    worker_memory: &Semaphore,
    workers: &Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    memory: u64,
) -> bool {
    let current_avail = worker_memory.available_permits();
    let needed = memory.saturating_sub(current_avail as u64);
    if needed == 0 {
        return true;
    }

    let mut freed = 0u64;
    for priority in [EvictionPriority::Idle, EvictionPriority::Warm] {
        if freed >= needed {
            break;
        }
        freed += evict_at_most_memory(workers, priority, needed - freed).await;
    }
    freed >= needed
}

/// Single estimate-semaphore acquisition attempt with eviction. Returns the
/// permit on success, or `None` when the pool cannot satisfy `mem32` even after
/// evicting idle/warm workers (caller decides whether to retry). Shared by
/// `ActiveWorkers::acquire` and the per-component charge source so there is one
/// pool-acquire implementation.
async fn acquire_pool_permit<Ctx: WorkerCtx>(
    worker_memory: &Arc<Semaphore>,
    workers: &Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    priority_allocation_lock: &Mutex<()>,
    acquire_retry_delay: Duration,
    mem32: u32,
    memory: u64,
) -> Option<WorkerMemoryPermit> {
    let lock = priority_allocation_lock.lock().await; // Block trying until a priority request is retrying once
    let result = worker_memory.clone().try_acquire_many_owned(mem32);
    drop(lock);
    match result {
        Ok(permit) => Some(WorkerMemoryPermit::new(permit)),
        Err(TryAcquireError::Closed) => panic!("worker memory semaphore has been closed"),
        Err(TryAcquireError::NoPermits) => {
            if try_free_up_pool_memory(worker_memory, workers, memory).await {
                // Freed enough; signal the caller to retry the acquire.
                None
            } else {
                // Could not free enough; wait before the caller retries.
                tokio::time::sleep(acquire_retry_delay).await;
                None
            }
        }
    }
}

struct WorkerEvictionSource<Ctx: WorkerCtx> {
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
}

#[async_trait]
impl<Ctx: WorkerCtx> EvictionSource for WorkerEvictionSource<Ctx> {
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64 {
        evict_at_most_memory(&self.workers, priority, needed_bytes).await
    }
}

/// Production [`ChargeSource`] for the per-component module charge. Takes
/// estimate-semaphore permits via the same pool acquire+evict path as worker
/// memory (the measured-headroom gate already accounts for the resident module
/// via real RSS, so the charge does not pass through it).
pub struct MemoryPoolChargeSource<Ctx: WorkerCtx> {
    worker_memory: Arc<Semaphore>,
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    priority_allocation_lock: Arc<Mutex<()>>,
    acquire_retry_delay: Duration,
}

#[async_trait]
impl<Ctx: WorkerCtx> ChargeSource for MemoryPoolChargeSource<Ctx> {
    type Charge = WorkerMemoryPermit;

    async fn acquire_charge(&self, bytes: u64) -> WorkerMemoryPermit {
        let mem32: u32 = bytes.try_into().expect("component charge size too large");
        loop {
            if let Some(permit) = acquire_pool_permit(
                &self.worker_memory,
                &self.workers,
                &self.priority_allocation_lock,
                self.acquire_retry_delay,
                mem32,
                bytes,
            )
            .await
            {
                break permit;
            }
        }
    }
}
