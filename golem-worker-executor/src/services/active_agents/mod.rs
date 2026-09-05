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
pub mod memory_probe;
#[cfg(test)]
mod tests;

pub(crate) use admission::MemoryGrant;
use admission::{AdmissionController, EvictionPriority, EvictionSource};
use async_trait::async_trait;
pub use component_charge::HeldComponentCharge;
use component_charge::{ChargeSource, ComponentChargeGuard, ComponentChargeRegistry};
pub use concurrent_agents_scheduler::{ConcurrentAgentPermit, ConcurrentAgentsScheduler};
pub use concurrent_agents_semaphore::ConcurrentAgentsSemaphore;
use memory_probe::{MemoryProbe, default_probe};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use tracing::{Instrument, debug};

use crate::services::HasAll;
use crate::services::agent_filesystem::{AgentFilesystems, FilesystemStorageError};
use crate::services::card_interest::{
    CardAuthorityRecoveryEpoch, CardAuthorityRecoveryFinalize, CardInterestIndex,
};
use crate::services::golem_config::{
    ActiveAgentsConfig, AgentStatusFlushConfig, FilesystemStorageConfig, MemoryConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::worker::Worker;
use crate::worker::entity_invocation::{EntityInvocationHandle, start_entity_invocation};
use crate::worker::entity_slot::ActiveEntityInvocationMetadata;
use crate::worker::entity_slot::EntitySlot;
use crate::worker::instance::{InstanceHost, OwnerExecution, OwnerRuntimeResources};
use crate::worker::owner_lane::{EntityCallMode, OwnerInvocationId};
use crate::worker::status_flusher::AgentStatusFlushQueue;
use crate::worker::{
    EvictionClass, EvictionStopOutcome, FilesystemPressureEligibility, UnloadRequest,
};
use crate::workerctx::WorkerCtx;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{InvocationFreshnessDisposition, Principal};
use golem_common::model::card::CardId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::entity::{
    AgentEntity, EntityActivation, EntityInvocationScope, OwnedAgentEntityId,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentId, OwnedAgentId, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::Store;
use wasmtime::component::Instance;

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

/// One owner-routed runtime group. Entity instances share its execution and resources but never
/// become independently routable workers.
pub struct ActiveAgent<Ctx: WorkerCtx> {
    owner_id: OwnedAgentId,
    primary: Arc<Worker<Ctx>>,
    execution: Arc<OwnerExecution>,
    resources: Arc<OwnerRuntimeResources>,
    entities: Mutex<HashMap<AgentEntity, Arc<EntitySlot>>>,
    accepting_entities: AtomicBool,
    entity_fence_generation: AtomicU64,
    _metrics: OwnerGroupMetricsGuard,
}

struct OwnerGroupMetricsGuard;

impl OwnerGroupMetricsGuard {
    fn new() -> Self {
        crate::metrics::workers::inc_owner_group_alive();
        Self
    }
}

impl Drop for OwnerGroupMetricsGuard {
    fn drop(&mut self) {
        crate::metrics::workers::dec_owner_group_alive();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveEntitySlotMetadata {
    pub entity_id: OwnedAgentEntityId,
    pub accepting: bool,
    pub invocations: Vec<ActiveEntityInvocationMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveAgentEntityMetadata {
    pub owner_id: OwnedAgentId,
    pub accepting_entities: bool,
    pub slots: Vec<ActiveEntitySlotMetadata>,
}

impl<Ctx: WorkerCtx> ActiveAgent<Ctx> {
    fn new(primary: Arc<Worker<Ctx>>) -> Self {
        Self {
            owner_id: primary.owned_agent_id().clone(),
            execution: primary.owner_execution(),
            resources: primary.owner_runtime_resources(),
            entities: Mutex::new(HashMap::new()),
            accepting_entities: AtomicBool::new(true),
            entity_fence_generation: AtomicU64::new(0),
            _metrics: OwnerGroupMetricsGuard::new(),
            primary,
        }
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        &self.owner_id
    }

    pub fn primary(&self) -> Arc<Worker<Ctx>> {
        self.primary.clone()
    }

    pub fn execution(&self) -> Arc<OwnerExecution> {
        self.execution.clone()
    }

    pub fn resources(&self) -> Arc<OwnerRuntimeResources> {
        self.resources.clone()
    }

    pub fn entity_slot(&self, entity: &AgentEntity) -> Arc<EntitySlot> {
        self.entities
            .lock()
            .unwrap()
            .entry(entity.clone())
            .or_insert_with(|| {
                Arc::new(EntitySlot::new(OwnedAgentEntityId {
                    owner: self.owner_id.clone(),
                    entity: entity.clone(),
                }))
            })
            .clone()
    }

    fn entity_slot_if_accepting(
        &self,
        entity: &AgentEntity,
    ) -> Result<Arc<EntitySlot>, WorkerExecutorError> {
        let mut entities = self.entities.lock().unwrap();
        if !self.accepting_entities.load(Ordering::Acquire) {
            return Err(WorkerExecutorError::runtime(
                "Entity admission is fenced by owner lifecycle",
            ));
        }
        Ok(entities
            .entry(entity.clone())
            .or_insert_with(|| {
                Arc::new(EntitySlot::new(OwnedAgentEntityId {
                    owner: self.owner_id.clone(),
                    entity: entity.clone(),
                }))
            })
            .clone())
    }

    pub fn entity_slots(&self) -> Vec<Arc<EntitySlot>> {
        self.entities.lock().unwrap().values().cloned().collect()
    }

    pub fn entity_metadata(&self) -> ActiveAgentEntityMetadata {
        let mut slots = self
            .entity_slots()
            .into_iter()
            .map(|slot| ActiveEntitySlotMetadata {
                entity_id: slot.entity_id().clone(),
                accepting: slot.is_accepting(),
                invocations: slot.active_invocations(),
            })
            .collect::<Vec<_>>();
        slots.sort_by(|left, right| left.entity_id.entity.cmp(&right.entity_id.entity));
        ActiveAgentEntityMetadata {
            owner_id: self.owner_id.clone(),
            accepting_entities: self.accepting_entities.load(Ordering::Acquire),
            slots,
        }
    }

    /// Returns transient metadata for one entity without creating a slot. Historical metadata is
    /// queried from the owner oplog; this view exists only while the owner group is active.
    pub fn entity_slot_metadata(
        &self,
        entity_id: &OwnedAgentEntityId,
    ) -> Option<ActiveEntitySlotMetadata> {
        if entity_id.owner_id() != &self.owner_id {
            return None;
        }
        self.entities
            .lock()
            .unwrap()
            .get(&entity_id.entity)
            .map(|slot| ActiveEntitySlotMetadata {
                entity_id: slot.entity_id().clone(),
                accepting: slot.is_accepting(),
                invocations: slot.active_invocations(),
            })
    }

    pub(crate) fn fence_entity_bodies(&self) {
        let entities = self.entities.lock().unwrap();
        self.entity_fence_generation.fetch_add(1, Ordering::AcqRel);
        self.accepting_entities.store(false, Ordering::Release);
        for slot in entities.values() {
            slot.fence();
        }
        drop(entities);
        self.resources.fence_filesystem_generation();
    }

    /// Closes admission for an eviction decision if no entity invocation is active.
    ///
    /// Holding the slot map while closing admission makes this atomic with slot lookup. A caller
    /// that already obtained a slot is rejected by the per-slot fence below if it has not yet
    /// registered; a caller that already registered makes this attempt fail. The inner result is a
    /// generation token with which a failed stop may reopen admission without overriding a newer
    /// lifecycle fence.
    pub(crate) fn try_fence_idle_entity_bodies(&self) -> Option<Option<u64>> {
        let entities = self.entities.lock().unwrap();
        if entities
            .values()
            .any(|slot| slot.active_invocation_count() != 0)
        {
            return None;
        }
        let reopen_generation = self
            .accepting_entities
            .swap(false, Ordering::AcqRel)
            .then(|| self.entity_fence_generation.fetch_add(1, Ordering::AcqRel) + 1);
        for slot in entities.values() {
            slot.fence();
        }
        Some(reopen_generation)
    }

    pub(crate) fn entity_fence_generation(&self) -> u64 {
        self.entity_fence_generation.load(Ordering::Acquire)
    }

    pub(crate) fn reopen_entity_admission_if_generation(&self, generation: u64) -> bool {
        let entities = self.entities.lock().unwrap();
        if self.entity_fence_generation.load(Ordering::Acquire) != generation {
            return false;
        }
        for slot in entities.values() {
            slot.reopen();
        }
        self.accepting_entities.store(true, Ordering::Release);
        true
    }

    pub fn entity_instance_host(
        &self,
        activation: &EntityActivation,
        owner_component_metadata: Arc<golem_service_base::model::component::Component>,
    ) -> Result<InstanceHost<Ctx>, WorkerExecutorError> {
        let entity = activation.entity();
        let slot = self.entity_slot(&entity);
        InstanceHost::new_entity(&self.primary, activation, slot, owner_component_metadata)
    }

    /// Starts one already-durable entity invocation in a fresh Store.
    ///
    /// Call-surface adapters resolve and pin the activation, append the owner-oplog Start, and then
    /// enter here with its invocation scope. Middleware chain dispatch uses the same hook
    /// recursively for each layer and the underlying tool; it does not instantiate Stores, acquire
    /// the owner lane, or register slots itself.
    pub fn start_entity_invocation<R, F, Finalize, Finalized>(
        &self,
        parent: OwnerInvocationId,
        scope: EntityInvocationScope,
        owner_component_metadata: Arc<golem_service_base::model::component::Component>,
        mode: EntityCallMode,
        invoke: F,
        finalize: Finalize,
    ) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
    where
        R: Send + 'static,
        F: Send + 'static,
        F: for<'a> FnOnce(
            &'a Instance,
            &'a mut Store<Ctx>,
        ) -> Pin<
            Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>,
        >,
        Finalize: FnOnce(Result<R, WorkerExecutorError>) -> Finalized + Send + 'static,
        Finalized: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
    {
        if !self.accepting_entities.load(Ordering::Acquire) {
            return Err(WorkerExecutorError::runtime(
                "Entity admission is fenced by owner lifecycle",
            ));
        }
        if scope.owner_id() != &self.owner_id {
            return Err(WorkerExecutorError::runtime(
                "Entity invocation scope does not belong to the active owner",
            ));
        }
        let slot = self.entity_slot_if_accepting(scope.invocation_id().entity())?;
        let host = InstanceHost::new_entity(
            &self.primary,
            scope.activation(),
            slot.clone(),
            owner_component_metadata,
        )?;
        start_entity_invocation(
            host,
            slot,
            self.execution.lane(),
            parent,
            scope,
            mode,
            invoke,
            finalize,
        )
    }
}

const INVOCATION_LOOP_DROP_STACK_SIZE: usize = 8 * 1024 * 1024;

/// The worker invocation loops spawned by one executor.
///
/// Every loop is bound to the executor's lifetime: when the executor's shutdown token is
/// cancelled, the loop task is abandoned at its next await point, which stops the worker from
/// touching storage exactly as if the executor process had died there. The oplog is designed to
/// be reopened after such an interruption. Cloning shares the same set of loops; a clone does not
/// keep any task alive.
#[derive(Clone, Debug)]
pub struct InvocationLoops {
    shutdown_token: CancellationToken,
    tracker: TaskTracker,
}

impl InvocationLoops {
    pub fn new(shutdown_token: CancellationToken) -> Self {
        Self {
            shutdown_token,
            tracker: TaskTracker::new(),
        }
    }

    pub(crate) fn spawn(
        &self,
        invocation_loop: impl Future<Output = ()> + Send + 'static,
    ) -> JoinHandle<()> {
        let shutdown_token = self.shutdown_token.clone();
        self.tracker.spawn(async move {
            let mut invocation_loop = Box::pin(invocation_loop);
            tokio::select! {
                biased;
                _ = shutdown_token.cancelled() => {
                    // Suspended Wasmtime calls form a deeply nested future tree whose destructor
                    // can exhaust Tokio's default worker-thread stack.
                    stacker::grow(INVOCATION_LOOP_DROP_STACK_SIZE, move || drop(invocation_loop));
                }
                _ = &mut invocation_loop => {}
            }
        })
    }

    /// Whether the owning executor has been shut down. Once true, no new loop makes progress and
    /// [`Self::wait_for_exit`] resolves as soon as the already running ones have exited.
    pub fn is_shut_down(&self) -> bool {
        self.shutdown_token.is_cancelled()
    }

    /// Resolves once every invocation loop of the executor has exited. Only meaningful after the
    /// executor was shut down; a loop of a live executor runs until its worker is unloaded.
    ///
    /// A loop that is executing guest code without reaching an await point cannot be cancelled
    /// once the executor's epoch ticker stopped, so callers should bound this wait.
    pub async fn wait_for_exit(&self) {
        self.tracker.close();
        self.tracker.wait().await;
    }
}

/// Holds owner-keyed active agent groups.
pub struct ActiveAgents<Ctx: WorkerCtx> {
    _unloaded_worker_eviction: UnloadedWorkerEvictionTask,
    agents: Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    card_interest_index: Arc<CardInterestIndex>,
    agent_filesystems: Arc<AgentFilesystems>,
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
    invocation_loops: InvocationLoops,
}

struct UnloadedWorkerEvictionTask(JoinHandle<()>);

impl UnloadedWorkerEvictionTask {
    fn start<Ctx: WorkerCtx>(
        agents: Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
        card_interest_index: Arc<CardInterestIndex>,
        ttl: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        const MAX_SWEEP_PERIOD: Duration = Duration::from_secs(60);
        const ZERO_TTL_SWEEP_PERIOD: Duration = Duration::from_secs(1);

        let period = if ttl.is_zero() {
            ZERO_TTL_SWEEP_PERIOD
        } else {
            ttl.min(MAX_SWEEP_PERIOD)
        };
        Self(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => break,
                    _ = tokio::time::sleep(period) => {}
                }
                evict_expired_unloaded_agents(&agents, &card_interest_index, ttl).await;
            }
        }))
    }
}

impl Drop for UnloadedWorkerEvictionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Identifies a compiled component for module-charge accounting.
type ComponentChargeKey = (ComponentId, ComponentRevision);

/// Guard held by a resident worker keeping its component's module charge alive.
pub type WorkerComponentCharge = ComponentChargeGuard<ComponentChargeKey, GateChargeSource>;

impl<Ctx: WorkerCtx> ActiveAgents<Ctx> {
    pub fn new(
        active_agents_config: &ActiveAgentsConfig,
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
        agent_status_flush_config: &AgentStatusFlushConfig,
        shutdown_token: CancellationToken,
    ) -> Result<Self, FilesystemStorageError> {
        // Build the probe once and hand it to the measured-headroom gate, which
        // bases its decision on the pod's cgroup limit when constrained (not host
        // RAM).
        let probe = default_probe(memory_config.system_memory_override);
        Self::new_with_probe(
            probe,
            active_agents_config,
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
        active_agents_config: &ActiveAgentsConfig,
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
        agent_status_flush_config: &AgentStatusFlushConfig,
        shutdown_token: CancellationToken,
    ) -> Result<Self, FilesystemStorageError> {
        let agent_filesystems = Arc::new(AgentFilesystems::new(storage_config)?);
        let admission = memory_config.enable_measured_admission.then(|| {
            Arc::new(AdmissionController::new(
                probe,
                memory_config.admission_policy(),
            ))
        });
        let agents = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::None,
            "active_agents",
        );
        let component_charges = ComponentChargeRegistry::new(GateChargeSource {
            admission: admission.clone(),
        });
        let card_interest_index = Arc::new(CardInterestIndex::new());
        let active_agents = Self {
            _unloaded_worker_eviction: UnloadedWorkerEvictionTask::start(
                agents.clone(),
                card_interest_index.clone(),
                active_agents_config.ttl,
                shutdown_token.clone(),
            ),
            agents,
            card_interest_index,
            agent_filesystems,
            concurrent_agents: Arc::new(ConcurrentAgentsScheduler::new()),
            acquire_retry_delay: memory_config.acquire_retry_delay,
            admission,
            component_charges,
            component_size_coefficient: memory_config.component_size_coefficient,
            status_flush_queue: AgentStatusFlushQueue::new(
                agent_status_flush_config.interval,
                agent_status_flush_config.max_concurrency,
                shutdown_token.clone(),
            ),
            invocation_loops: InvocationLoops::new(shutdown_token),
        };
        active_agents.initialize_metrics();
        Ok(active_agents)
    }

    /// The per-executor queue used to batch cached agent status blob writes in the background.
    pub fn status_flush_queue(&self) -> Arc<AgentStatusFlushQueue> {
        self.status_flush_queue.clone()
    }

    /// The invocation loops of this executor's workers, bound to the executor's shutdown token.
    pub fn invocation_loops(&self) -> InvocationLoops {
        self.invocation_loops.clone()
    }

    pub(crate) fn agent_filesystems(&self) -> Arc<AgentFilesystems> {
        Arc::clone(&self.agent_filesystems)
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
        self.get_or_add_with_freshness(
            deps,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context_stack,
            principal,
            InvocationFreshnessDisposition::MayExist,
        )
        .await
    }

    pub async fn get_or_add_with_freshness<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let owned_agent_id = owned_agent_id.clone();
        let cache_key = owned_agent_id.clone();
        let deps = deps.clone();
        let invocation_context_stack = invocation_context_stack.clone();
        let active_agent = self
            .agents
            .get_or_insert_simple(&cache_key, || {
                Box::pin(async move {
                    let worker = Worker::new(
                        &deps,
                        self.card_interest_index.clone(),
                        owned_agent_id.clone(),
                        worker_env,
                        worker_agent_config,
                        component_revision,
                        parent,
                        &invocation_context_stack,
                        principal,
                        freshness_disposition,
                    )
                    .in_current_span()
                    .await;

                    worker.map(|worker| {
                        let worker = Arc::new(worker);
                        Worker::start_durable_stream_attachment_reconciler(&worker);
                        Arc::new(ActiveAgent::new(worker))
                    })
                })
            })
            .await?;
        Ok(active_agent.primary())
    }

    pub async fn try_get(&self, owned_agent_id: &OwnedAgentId) -> Option<Arc<Worker<Ctx>>> {
        self.try_get_active_agent(owned_agent_id)
            .await
            .map(|active_agent| active_agent.primary())
    }

    pub async fn try_get_active_agent(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<Arc<ActiveAgent<Ctx>>> {
        self.agents.get(owned_agent_id).await
    }

    /// Checks whether an owner group is cached without refreshing its TTL.
    pub async fn contains_cached_agent(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.agents.contains_key(owned_agent_id).await
    }

    /// Inspects all currently known entity slots for an active owner. No durable child status is
    /// created when the owner or a completed invocation is absent.
    pub async fn entity_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<ActiveAgentEntityMetadata> {
        self.try_get_active_agent(owned_agent_id)
            .await
            .map(|active_agent| active_agent.entity_metadata())
    }

    /// Owner-routed local inspection for one entity slot. Looking up an unknown selector does not
    /// create it and never treats the entity identity as a routing or shard key.
    pub async fn entity_slot_metadata(
        &self,
        entity_id: &OwnedAgentEntityId,
    ) -> Option<ActiveEntitySlotMetadata> {
        self.try_get_active_agent(entity_id.owner_id())
            .await
            .and_then(|active_agent| active_agent.entity_slot_metadata(entity_id))
    }

    pub async fn remove(&self, owned_agent_id: &OwnedAgentId) {
        if let Some(active_agent) = self.agents.get(owned_agent_id).await {
            active_agent.fence_entity_bodies();
            let worker = active_agent.primary();
            self.card_interest_index
                .set_card_interest(worker.owned_agent_id().clone(), &[])
                .await;
        }
        self.agents.remove(owned_agent_id).await
    }

    pub async fn tracked_card_ids(&self) -> Vec<CardId> {
        self.card_interest_index.tracked_card_ids().await
    }

    pub(crate) fn close_card_authority(&self) -> CardAuthorityRecoveryEpoch {
        self.card_interest_index.close_authority()
    }

    pub(crate) fn is_current_card_authority_recovery(
        &self,
        epoch: CardAuthorityRecoveryEpoch,
    ) -> bool {
        self.card_interest_index.is_current_recovery(epoch)
    }

    pub(crate) async fn tracked_card_ids_with_revision(&self) -> (u64, Vec<CardId>) {
        self.card_interest_index
            .tracked_card_ids_with_revision()
            .await
    }

    pub(crate) async fn finalize_card_authority_recovery(
        &self,
        epoch: CardAuthorityRecoveryEpoch,
        expected_interest_revision: u64,
    ) -> CardAuthorityRecoveryFinalize {
        self.card_interest_index
            .finalize_recovery(epoch, expected_interest_revision)
            .await
    }

    pub async fn notify_revoked_cards(&self, card_ids: &[CardId]) {
        let affected_agent_cards = self.card_interest_index.interested_agents(card_ids).await;

        for (owned_agent_id, affected_card_ids) in affected_agent_cards {
            let Some(worker) = self.try_get(&owned_agent_id).await else {
                continue;
            };

            worker.queue_card_revocations(&affected_card_ids).await;
        }
    }

    pub async fn snapshot(&self) -> Vec<(AgentId, Arc<Worker<Ctx>>)> {
        self.agents
            .iter()
            .await
            .into_iter()
            .map(|(_, active_agent)| {
                let primary = active_agent.primary();
                (primary.agent_id(), primary)
            })
            .collect()
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
                self.remove(worker.owned_agent_id()).await;
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
    pub(crate) async fn acquire_memory(&self, memory: u64) -> MemoryGrant {
        let Some(admission) = &self.admission else {
            return MemoryGrant::inert(memory);
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
            agents: self.agents.clone(),
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
            return Some(MemoryGrant::inert(memory));
        };
        match admission.admit(memory, &self.eviction_source()).await {
            Some(grant) => Some(grant),
            None => {
                debug!("Measured headroom insufficient for {memory}, not admitting");
                None
            }
        }
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

    /// Initializes worker gauges. Subsequent changes are recorded inline at the mutation sites.
    fn initialize_metrics(&self) {
        crate::metrics::workers::initialize_worker_metrics();
    }
}

async fn evict_expired_unloaded_agents<Ctx: WorkerCtx>(
    agents: &Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    card_interest_index: &CardInterestIndex,
    ttl: Duration,
) {
    for (owned_agent_id, active_agent) in agents.entries_older_than(ttl).await {
        let worker = &active_agent.primary;
        let Some(retirement) = worker.try_begin_cache_retirement().await else {
            continue;
        };
        if Arc::strong_count(&active_agent) != 2 || Arc::strong_count(worker) != 1 {
            continue;
        }

        let removed = card_interest_index
            .clear_agent_interest_if(
                &owned_agent_id,
                agents.remove_if_cached_older_than(&owned_agent_id, ttl, |current| {
                    Arc::ptr_eq(current, &active_agent)
                        // `entries_older_than` owns the only reference besides the cache.
                        && Arc::strong_count(current) == 2
                        // The cached ActiveAgent must be the Worker's only strong owner.
                        && Arc::strong_count(&current.primary) == 1
                        // Fence entity work only after all final removal checks pass while
                        // concurrent cache lookups are excluded by the cache entry lock.
                        && current.try_fence_idle_entity_bodies().is_some()
                }),
            )
            .await;

        if removed {
            retirement.commit();
        }
    }
}

pub(crate) struct FilesystemPressureVictim<Ctx: WorkerCtx> {
    stable_agent_id: String,
    eligible_since: u64,
    worker: Arc<Worker<Ctx>>,
    eligibility: FilesystemPressureEligibility,
}

impl<Ctx: WorkerCtx> FilesystemPressureVictim<Ctx> {
    pub(crate) fn stable_agent_id(&self) -> &str {
        &self.stable_agent_id
    }

    pub(crate) fn eligible_since(&self) -> u64 {
        self.eligible_since
    }
}

pub(crate) async fn eligible_loaded_idle_filesystem_pressure_victims<Ctx: WorkerCtx>(
    active_agents: &ActiveAgents<Ctx>,
) -> Vec<FilesystemPressureVictim<Ctx>> {
    let mut candidates = Vec::new();
    for (agent_id, active_agent) in active_agents.agents.iter().await {
        let worker = active_agent.primary();
        if is_loaded_idle_filesystem_pressure_candidate(worker.eviction_class().await) {
            let Some(eligibility) = worker.filesystem_pressure_eligibility().await else {
                continue;
            };
            candidates.push(FilesystemPressureVictim {
                stable_agent_id: agent_id.to_string(),
                eligible_since: Worker::<Ctx>::filesystem_pressure_eligible_since(eligibility),
                worker,
                eligibility,
            });
        }
    }
    candidates
}

fn is_loaded_idle_filesystem_pressure_candidate(class: Option<EvictionClass>) -> bool {
    class == Some(EvictionClass::LoadedIdle)
}

pub(crate) fn request_loaded_idle_filesystem_unload<Ctx: WorkerCtx>(
    candidate: FilesystemPressureVictim<Ctx>,
    unload_request: UnloadRequest,
) -> tokio::task::JoinHandle<EvictionStopOutcome> {
    tokio::spawn(async move {
        stop_loaded_idle_if_eligible(
            candidate.eligibility,
            unload_request,
            move |target_class, eligibility, unload_request| async move {
                candidate
                    .worker
                    .stop_if_evictable_with_outcome(target_class, eligibility, unload_request)
                    .await
            },
        )
        .await
    })
}

pub(crate) async fn stop_loaded_idle_if_eligible<T>(
    eligibility: FilesystemPressureEligibility,
    unload_request: UnloadRequest,
    stop: impl AsyncFnOnce(EvictionClass, Option<FilesystemPressureEligibility>, UnloadRequest) -> T,
) -> T {
    stop(EvictionClass::LoadedIdle, Some(eligibility), unload_request).await
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
    agents: &Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    component_charges: &Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    component_size_coefficient: f64,
    priority: EvictionPriority,
    needed_bytes: u64,
) -> u64 {
    let target_class: crate::worker::EvictionClass = priority.into();

    let mut candidates = Vec::new();
    for (owned_agent_id, active_agent) in agents.iter().await {
        let worker = active_agent.primary();
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
            candidates.push((
                owned_agent_id,
                worker,
                mem,
                component,
                charge_bytes,
                last_changed,
            ));
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
    agents: Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    component_charges: Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    component_size_coefficient: f64,
}

#[async_trait]
impl<Ctx: WorkerCtx> EvictionSource for WorkerEvictionSource<Ctx> {
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64 {
        evict_at_most_memory(
            &self.agents,
            &self.component_charges,
            self.component_size_coefficient,
            priority,
            needed_bytes,
        )
        .await
    }
}

/// Single attempt of the charge-first admission ordering used by
/// [`ActiveAgents::acquire_with_component_charge`]: reserve the component's
/// shared module, then admit the worker's own memory once.
///
/// Returns the worker's [`MemoryGrant`] and its [`WorkerComponentCharge`], or
/// `None` if the memory admission is refused (in which case dropping the charge
/// releases the module again). Exists so the composition of the admission gate
/// and the component-charge registry — the heart of the first-worker
/// memory + module gating — can be exercised without constructing a full
/// `ActiveAgents<Ctx>`. The production method runs this same ordering with the
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
