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
use tokio_util::sync::CancellationToken;

use tracing::{Instrument, debug};

use crate::services::golem_config::{
    AgentStatusFlushConfig, FilesystemStorageConfig, MemoryConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::{HasAll, HasCardService};
use crate::worker::Worker;
use crate::worker::status_flusher::AgentStatusFlushQueue;
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
    status_flush_queue: Arc<AgentStatusFlushQueue>,
}

/// Identifies a compiled component for module-charge accounting.
type ComponentChargeKey = (ComponentId, ComponentRevision);

/// Guard held by a resident worker keeping its component's module charge alive.
pub type WorkerComponentCharge = ComponentChargeGuard<ComponentChargeKey, GateChargeSource>;

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn new(
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
        agent_status_flush_config: &AgentStatusFlushConfig,
        shutdown_token: CancellationToken,
    ) -> Self {
        // Build the probe once and hand it to the measured-headroom gate, which
        // bases its decision on the pod's cgroup limit when constrained (not host
        // RAM).
        let probe = default_probe(memory_config.system_memory_override);
        Self::new_with_probe(
            probe,
            memory_config,
            storage_config,
            agent_status_flush_config,
            shutdown_token,
        )
    }

    /// Like [`Self::new`] but with an explicitly provided memory probe instead of
    /// the one derived from the config. The in-process test harness uses this to
    /// supply a probe with a pinned limit and current usage, so the gate's
    /// decision is deterministic and isolated from the shared test process's RSS.
    pub fn new_with_probe(
        probe: Box<dyn MemoryProbe>,
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
        agent_status_flush_config: &AgentStatusFlushConfig,
        shutdown_token: CancellationToken,
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
            status_flush_queue: AgentStatusFlushQueue::new(
                agent_status_flush_config.interval,
                agent_status_flush_config.max_concurrency,
                shutdown_token,
            ),
        };
        active_workers.initialize_metrics();
        active_workers
    }

    /// The per-executor queue used to batch cached agent status blob writes in the background.
    pub fn status_flush_queue(&self) -> Arc<AgentStatusFlushQueue> {
        self.status_flush_queue.clone()
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
        T: HasAll<Ctx> + HasCardService + Clone + Send + Sync + 'static,
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
    async fn acquire_memory(&self, memory: u64) -> MemoryGrant {
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
            component_charges: self.component_charges.clone(),
            component_size_coefficient: self.component_size_coefficient,
        }
    }

    /// Blocking admission of a starting worker together with its component's
    /// shared compiled module. Acquires the per-component module charge first —
    /// reserving the module's bytes with the gate for the first worker of the
    /// component, nothing more for later workers — then loops the worker's own
    /// memory admission until the gate admits it, backing off between attempts.
    ///
    /// Acquiring the module charge before admitting the worker's memory is what
    /// makes the first worker of a component gated on its memory *and* its module
    /// together: the memory admission measures headroom against a granted total
    /// that already includes the module, so a first worker is admitted only when
    /// both fit — the gate evicts or backs off rather than over-committing. Both
    /// the returned [`MemoryGrant`] (worker memory) and the
    /// [`WorkerComponentCharge`] (shared module) release their reservations on
    /// drop, so a start cancelled mid-flight returns the whole reservation.
    pub(crate) async fn acquire_with_component_charge(
        &self,
        memory: u64,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component_module_bytes: u64,
    ) -> (MemoryGrant, WorkerComponentCharge) {
        // Reserve the shared module first so the worker's memory admission
        // accounts for it. Held across admission retries and released on drop if
        // the start is cancelled.
        let charge = self
            .acquire_component_charge(component_id, component_revision, component_module_bytes)
            .await;
        let grant = self.acquire_memory(memory).await;
        (grant, charge)
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

/// The cost of stopping one eviction candidate: its own linear memory and the
/// size of its component's shared compiled module (which is only actually freed
/// when the candidate removes the last resident worker of that component).
#[derive(Debug, Clone)]
pub(crate) struct EvictionCandidateCost<K> {
    pub memory: u64,
    pub component: K,
    pub module_bytes: u64,
}

/// Accounts the bytes freed by stopping one eviction candidate, updating the
/// working resident-count map.
///
/// A stop always frees the candidate's own linear `memory`. It additionally
/// frees the component's shared compiled `module_bytes`, but only when this stop
/// removes the *last* resident worker of the component — tracked by decrementing
/// `remaining[component]` and crediting the module when it reaches zero. Shared
/// by both [`plan_memory_eviction_stops`] (advisory planning) and
/// [`evict_at_most_memory`] (the actual stop loop) so the planned and the
/// returned freed totals use identical accounting.
fn credit_eviction_stop<K: Eq + std::hash::Hash + Clone>(
    remaining: &mut std::collections::HashMap<K, usize>,
    component: &K,
    memory: u64,
    module_bytes: u64,
) -> u64 {
    let mut freed = memory;
    let count = remaining.entry(component.clone()).or_insert(0);
    *count = count.saturating_sub(1);
    if *count == 0 {
        freed += module_bytes;
    }
    freed
}

/// Plan how many leading (oldest-first) candidates the memory-eviction loop
/// should attempt to stop to free at least `needed_bytes`.
///
/// Each stop frees the candidate's own memory plus, when it removes the last
/// resident worker of its component, that component's shared module. `refcounts`
/// is the resident-worker count per component across the *whole* live set (not
/// just the candidates), so a component is credited its module only once every
/// resident worker of it — candidate or not — has been accounted as stopped.
///
/// Purely advisory: this decides how many workers to *attempt* to stop, never
/// releasing any bytes. The module charge is released only by the worker's
/// charge guard on drop (covering graceful stop, cancel and abort alike), and
/// the gate re-measures against the probe after eviction, so an imperfect plan
/// can at worst stop scanning slightly early or late.
pub(crate) fn plan_memory_eviction_stops<K: Eq + std::hash::Hash + Clone>(
    candidates: &[EvictionCandidateCost<K>],
    refcounts: &std::collections::HashMap<K, usize>,
    needed_bytes: u64,
) -> usize {
    // Working copy of the resident counts, decremented as we plan each stop, so
    // the module is credited exactly once — to the stop that takes a component's
    // resident count to zero.
    let mut remaining: std::collections::HashMap<K, usize> = refcounts.clone();
    let mut freed = 0u64;
    let mut stops = 0usize;
    for candidate in candidates {
        if freed >= needed_bytes {
            break;
        }
        freed += credit_eviction_stop(
            &mut remaining,
            &candidate.component,
            candidate.memory,
            candidate.module_bytes,
        );
        stops += 1;
    }
    stops
}

/// Evicts resident workers at a single priority tier, oldest-first, stopping
/// once at least `needed_bytes` have been freed or the tier is exhausted.
/// Returns the bytes actually reclaimed.
///
/// How many workers to attempt to stop is decided by
/// [`plan_memory_eviction_stops`], which credits a component's shared module to
/// the stop that removes its last resident worker — so stopping the last worker
/// of a component is correctly counted as freeing its memory *and* its module,
/// rather than memory alone, which would over-evict.
async fn evict_at_most_memory<Ctx: WorkerCtx>(
    workers: &Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    component_charges: &Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    component_size_coefficient: f64,
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
            // Use the currently-loaded module the resident worker actually holds
            // a charge for, not any queued pending-update target: the update has
            // not been applied yet, so the held charge key and size must match the
            // loaded revision for the refcount lookup and freed accounting to be
            // correct.
            let (component_id, component_revision, module_bytes) =
                worker.resident_component_charge_requirement().await;
            let charge_bytes = (component_size_coefficient * module_bytes as f64) as u64;
            let component: ComponentChargeKey = (component_id, component_revision);
            let last_changed = worker.last_execution_state_change();
            candidates.push((agent_id, worker, mem, component, charge_bytes, last_changed));
        }
    }

    // Sort by timestamp oldest-first: the eviction plan and the stop loop both
    // walk candidates oldest-first.
    candidates.sort_by_key(|(_, _, _, _, _, ts)| ts.to_millis());

    // Decide, accounting for last-of-component module releases, how many leading
    // candidates to attempt to stop.
    let refcounts = component_charges.charge_refcounts();
    let costs: Vec<EvictionCandidateCost<ComponentChargeKey>> = candidates
        .iter()
        .map(
            |(_, _, mem, component, charge_bytes, _)| EvictionCandidateCost {
                memory: *mem,
                component: *component,
                module_bytes: *charge_bytes,
            },
        )
        .collect();
    let planned_stops = plan_memory_eviction_stops(&costs, &refcounts, needed_bytes);

    // Working copy of the resident counts, decremented on each successful stop so
    // a component's shared module is credited to `freed` exactly once — to the
    // stop that takes its resident count to zero. This mirrors
    // `plan_memory_eviction_stops`, but counts only stops that actually
    // succeeded, so the returned total reflects the memory genuinely reclaimed
    // (worker linear memory plus released module bytes). The admission gate uses
    // this total to decide whether to escalate to the next priority tier, so
    // omitting the module bytes here would under-report reclaimed headroom and
    // cause unnecessary higher-tier evictions.
    let mut remaining = refcounts;
    let mut freed = 0u64;
    for (agent_id, worker, mem, component, charge_bytes, _) in
        candidates.into_iter().take(planned_stops)
    {
        debug!("Trying to stop {target_class:?} {agent_id} to free up memory");
        if worker.stop_if_evictable(target_class).await {
            debug!("Stopped {target_class:?} {agent_id} to free up {mem} memory");
            crate::metrics::workers::record_worker_eviction(match priority {
                EvictionPriority::Idle => "LoadedIdle",
                EvictionPriority::Warm => "WarmRunnable",
            });
            // Credit the worker's linear memory plus, when this stop removes the
            // last resident worker of its component, the shared module bytes.
            freed += credit_eviction_stop(&mut remaining, &component, mem, charge_bytes);
        }
    }
    freed
}

/// A source of evictable, already-resident memory the gate reclaims through.
struct WorkerEvictionSource<Ctx: WorkerCtx> {
    workers: Cache<AgentId, (), Arc<Worker<Ctx>>, WorkerExecutorError>,
    component_charges: Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    component_size_coefficient: f64,
}

#[async_trait]
impl<Ctx: WorkerCtx> EvictionSource for WorkerEvictionSource<Ctx> {
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64 {
        evict_at_most_memory(
            &self.workers,
            &self.component_charges,
            self.component_size_coefficient,
            priority,
            needed_bytes,
        )
        .await
    }
}

/// Single attempt of the charge-first admission ordering used by
/// [`ActiveWorkers::acquire_with_component_charge`]: reserve the component's
/// shared module, then admit the worker's own memory once.
///
/// Returns the worker's [`MemoryGrant`] and its [`WorkerComponentCharge`], or
/// `None` if the memory admission is refused (in which case dropping the charge
/// releases the module again). Exists so the composition of the admission gate
/// and the component-charge registry — the heart of the first-worker
/// memory + module gating — can be exercised without constructing a full
/// `ActiveWorkers<Ctx>`. The production method runs this same ordering with the
/// memory admission wrapped in its blocking retry loop.
#[cfg(test)]
async fn acquire_memory_and_component_charge(
    admission: &Arc<AdmissionController>,
    component_charges: &Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    source: &dyn EvictionSource,
    memory: u64,
    component: ComponentChargeKey,
    charge_bytes: u64,
) -> Option<(MemoryGrant, WorkerComponentCharge)> {
    // Reserve the component's shared module charge *first*. For the first worker
    // of a component this adds the module bytes to the gate's granted total; for
    // later workers the module is already held and nothing more is reserved.
    // Admitting the worker's own memory afterwards therefore measures headroom
    // against a granted total that already includes this module, so a first
    // worker is admitted only when its memory *and* its module both fit — the
    // gate can evict or reject rather than over-committing. If admission fails,
    // dropping the charge releases the module again, keeping the granted total
    // symmetric.
    let charge = component_charges.acquire(component, charge_bytes).await;
    let grant = admission.admit(memory, source).await?;
    Some((grant, charge))
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
