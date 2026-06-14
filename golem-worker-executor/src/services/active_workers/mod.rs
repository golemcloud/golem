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

pub(crate) use admission::MemoryGrant;
use admission::{AdmissionController, EvictionPriority, EvictionSource};
use async_trait::async_trait;
pub use component_charge::HeldComponentCharge;
use component_charge::{ChargeSource, ComponentChargeGuard, ComponentChargeRegistry};
use memory_probe::{MemoryProbe, default_probe};
use std::sync::Arc;
use std::time::Duration;

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
    worker_filesystem_storage: Arc<FilesystemStorageSemaphore>,
    concurrent_agents: Arc<ConcurrentAgentsScheduler>,
    acquire_retry_delay: Duration,
    /// Authoritative measured-headroom admission gate, and the sole admission
    /// authority. Decides whether real memory headroom permits a new
    /// acquisition, evicting via the worker set when short. `None` when measured
    /// admission is disabled (e.g. shared test environments), in which case
    /// acquisition always proceeds.
    admission: Option<Arc<AdmissionController>>,
    /// Reserves each resident component's compiled module size with the gate
    /// exactly once (shared across all its workers) rather than per worker, so
    /// the module's resident cost is accounted before it faults into memory.
    component_charges: Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    /// Multiplier applied to a component's `component_size` when sizing its
    /// module charge.
    component_size_coefficient: f64,
}

/// Identifies a compiled component for module-charge accounting.
type ComponentChargeKey = (ComponentId, ComponentRevision);

/// Guard held by a resident worker keeping its component's module charge alive.
pub type WorkerComponentCharge = ComponentChargeGuard<ComponentChargeKey, GateChargeSource>;

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn new(memory_config: &MemoryConfig, storage_config: &FilesystemStorageConfig) -> Self {
        // Build the probe once and hand it to the measured-headroom gate, which
        // bases its decision on the pod's cgroup limit when constrained (not host
        // RAM).
        let probe = default_probe(memory_config.system_memory_override);
        Self::new_with_probe(probe, memory_config, storage_config)
    }

    /// Like [`Self::new`] but with an explicitly provided memory probe instead of
    /// the one derived from the config. The in-process test harness uses this to
    /// supply a probe with a pinned limit and current usage, so the gate's
    /// decision is deterministic and isolated from the shared test process's RSS.
    pub fn new_with_probe(
        probe: Box<dyn MemoryProbe>,
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
    ) -> Self {
        let admission = memory_config.enable_measured_admission.then(|| {
            Arc::new(AdmissionController::new(
                probe,
                memory_config.admission_policy(),
            ))
        });
        let workers = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            "active_workers",
        );
        let component_charges = ComponentChargeRegistry::new(GateChargeSource {
            admission: admission.clone(),
        });
        let active_workers = Self {
            workers,
            worker_filesystem_storage: Arc::new(FilesystemStorageSemaphore::new(
                storage_config.worker_filesystem_storage(),
                storage_config.acquire_retry_delay,
            )),
            concurrent_agents: Arc::new(ConcurrentAgentsScheduler::new()),
            acquire_retry_delay: memory_config.acquire_retry_delay,
            admission,
            component_charges,
            component_size_coefficient: memory_config.component_size_coefficient,
        };
        active_workers.initialize_metrics();
        active_workers
    }

    /// Acquire (or share) the per-component module charge for a worker of the
    /// given component. The first resident worker of the component reserves its
    /// compiled-module size (scaled by `component_size_coefficient`) with the
    /// gate; subsequent workers share the same charge. The returned guard
    /// releases the charge when the last worker of the component unloads.
    pub async fn acquire_component_charge(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component_module_bytes: u64,
    ) -> WorkerComponentCharge {
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

    /// Blocking memory admission for a starting worker. Loops until the gate
    /// admits the request, backing off between attempts, and returns a
    /// [`MemoryGrant`] guard owning the reservation: the worker holds it for as
    /// long as it is resident and releases it by dropping the guard, so a start
    /// cancelled before the worker becomes resident cannot leak the reservation.
    ///
    /// A rejection is transient, not terminal. The gate reads resident memory
    /// from the probe, which lags real usage (cgroup `memory.current` only counts
    /// already-touched pages), so a worker admitted earlier may not yet be fully
    /// resident; pressure eases as its pages settle and as other workers finish.
    /// Each iteration backs off and re-reads the gate, so the caller eventually
    /// proceeds once headroom recovers rather than failing under momentary
    /// pressure. With measured admission disabled the worker is admitted
    /// immediately with an inert grant.
    pub(crate) async fn acquire(&self, memory: u64) -> MemoryGrant {
        let Some(admission) = &self.admission else {
            return MemoryGrant::inert();
        };
        loop {
            // Evicts idle-then-warm when real headroom is short; rejects (and we
            // back off) when it cannot make room rather than risking the limit.
            if let Some(grant) = admission.admit(memory, &self.eviction_source()).await {
                return grant;
            }
            debug!("Measured headroom insufficient for {memory}, backing off and retrying");
            tokio::time::sleep(self.acquire_retry_delay).await;
        }
    }

    /// Builds an [`EvictionSource`] view over the live worker set for the
    /// admission controller to reclaim memory through.
    fn eviction_source(&self) -> WorkerEvictionSource<Ctx> {
        WorkerEvictionSource {
            workers: self.workers.clone(),
        }
    }

    /// Non-blocking memory admission for a growing worker. A single gate attempt:
    /// returns the additional [`MemoryGrant`] when the grow is admitted, or `None`
    /// when real headroom is insufficient even after eviction (the caller turns
    /// `None` into a retriable out-of-memory trap). The returned grant should be
    /// merged into the worker's existing grant so its whole reservation is
    /// released together on unload. With measured admission disabled the grow is
    /// always admitted with an inert grant.
    pub(crate) async fn try_acquire(&self, memory: u64) -> Option<MemoryGrant> {
        let Some(admission) = &self.admission else {
            return Some(MemoryGrant::inert());
        };
        match admission.admit(memory, &self.eviction_source()).await {
            Some(grant) => Some(grant),
            None => {
                debug!("Measured headroom insufficient for {memory}, not admitting");
                None
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
    fn initialize_metrics(&self) {
        crate::metrics::workers::initialize_worker_metrics();
        crate::metrics::workers::set_filesystem_semaphore_available(
            self.worker_filesystem_storage.available_bytes(),
        );
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

/// A source of evictable, already-resident memory the gate reclaims through.
struct WorkerEvictionSource<Ctx: WorkerCtx> {
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
}

#[async_trait]
impl<Ctx: WorkerCtx> EvictionSource for WorkerEvictionSource<Ctx> {
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64 {
        evict_at_most_memory(&self.workers, priority, needed_bytes).await
    }
}

/// Production [`ChargeSource`] for the per-component module charge: reserves the
/// module's bytes with the measured-headroom gate. The module is a committed
/// consequence of admitting the first worker of a component (it loads into RAM
/// when that worker becomes resident), so it is reserved rather than admitted —
/// it neither evicts nor can be refused. `None` when measured admission is
/// disabled, in which case the charge is a no-op.
pub struct GateChargeSource {
    admission: Option<Arc<AdmissionController>>,
}

/// Held module charge: releases its reserved bytes from the gate on drop.
pub struct GateCharge {
    admission: Option<Arc<AdmissionController>>,
    bytes: u64,
}

impl Drop for GateCharge {
    fn drop(&mut self) {
        if let Some(admission) = &self.admission {
            admission.release(self.bytes);
        }
    }
}

#[async_trait]
impl ChargeSource for GateChargeSource {
    type Charge = GateCharge;

    async fn acquire_charge(&self, bytes: u64) -> GateCharge {
        if let Some(admission) = &self.admission {
            admission.reserve_committed(bytes);
        }
        GateCharge {
            admission: self.admission.clone(),
            bytes,
        }
    }
}
