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
mod entity_execution;
mod eviction;
mod invocation_loops;
pub mod memory_probe;
#[cfg(test)]
mod tests;

#[cfg(test)]
use eviction::{
    acquire_memory_and_component_charge, admission_wait_warns, component_charge_bytes,
    credit_eviction_stop, is_loaded_idle_filesystem_pressure_candidate,
};
pub(crate) use eviction::{
    eligible_loaded_idle_filesystem_pressure_victims, request_loaded_idle_filesystem_unload,
};
#[cfg(test)]
pub(crate) use eviction::{plan_memory_eviction_stops, stop_loaded_idle_if_eligible};

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
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use tracing::{Instrument, debug, info, warn};

use crate::durable_host::tool::operation::OwnerFailureWinner;
use crate::services::HasAll;
use crate::services::agent_filesystem::{AgentFilesystems, FilesystemStorageError};
use crate::services::card_interest::{
    CardAuthorityRecoveryEpoch, CardAuthorityRecoveryFinalize, CardInterestIndex,
};
use crate::services::golem_config::{
    ActiveAgentsConfig, AgentStatusFlushConfig, FilesystemStorageConfig, MemoryConfig,
};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::worker::entity_invocation::{
    EntityInvocationHandle, RetainedEntityStore, start_entity_invocation,
    start_native_entity_invocation, start_pre_acquired_entity_invocation,
    start_registered_entity_invocation, start_registered_native_entity_invocation,
};
use crate::worker::entity_slot::ActiveEntityInvocationMetadata;
use crate::worker::entity_slot::EntitySlot;
use crate::worker::instance::{
    EntityInvocationBody, InstanceHost, OwnerExecution, OwnerRuntimeResources,
};
use crate::worker::owner_lane::{EntityCallMode, OwnerInvocationId, OwnerInvocationTicket};
use crate::worker::status_flusher::AgentStatusFlushQueue;
use crate::worker::{
    EvictionClass, EvictionStopOutcome, FilesystemPressureEligibility, UnloadRequest,
};
use crate::worker::{RetirementReason, Worker, WorkerCreationMode};
use crate::workerctx::WorkerCtx;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::{InvocationFreshnessDisposition, OwnerKind, Principal};
use golem_common::model::card::CardId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::entity::{
    AgentEntity, EntityActivation, EntityInvocationScope, OwnedAgentEntityId,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AgentFingerprint, AgentId, IdempotencyKey, OwnedAgentId, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use wasmtime::Store;
use wasmtime::component::Instance;

/// Capability proving that per-account concurrent-agent state has been registered
/// in this executor and can be used for subsequent permit acquires.
#[doc(hidden)]
#[derive(Clone)]
pub struct RegisteredConcurrentAccount {
    scheduler: Arc<ConcurrentAgentsScheduler>,
    account_id: AccountId,
}

impl RegisteredConcurrentAccount {
    pub async fn acquire(&self, agent_id: AgentId) -> ConcurrentAgentPermit {
        self.scheduler.acquire(self.account_id, agent_id).await
    }
}

/// One owner-routed runtime group. Entity instances share its execution and resources but never
/// become independently routable workers.
pub struct ActiveAgent<Ctx: WorkerCtx> {
    owner_id: OwnedAgentId,
    primary: Arc<Worker<Ctx>>,
    executor_tasks: InvocationLoops,
    entities: Mutex<HashMap<AgentEntity, Arc<EntitySlot>>>,
    accepting_entities: AtomicBool,
    entity_fence_generation: AtomicU64,
    metrics: OnceLock<OwnerGroupMetricsGuard>,
}

struct OwnerGroupMetricsGuard;

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
    pub lane: crate::worker::owner_lane::OwnerLaneMetadata,
    pub tool_operations: crate::durable_host::tool::ToolOperationSetMetadata,
}

pub(crate) struct EntityFence<'a> {
    pub operations: &'a crate::durable_host::tool::operation::OwnerToolOperations,
    pub entities: &'a Mutex<HashMap<AgentEntity, Arc<EntitySlot>>>,
    pub accepting: &'a AtomicBool,
    pub generation: &'a AtomicU64,
    pub resources: &'a OwnerRuntimeResources,
}

/// The worker invocation loops spawned by one executor.
///
/// Every loop is bound to the executor's lifetime. Shutdown fences producer mutations before
/// abandoning the loop and drains admitted writes before reporting its exit. The oplog can then
/// be reopened without racing writes from the old owner. Cloning shares the same set of loops;
/// a clone does not keep any task alive.
#[derive(Clone)]
pub struct InvocationLoops {
    shutdown_token: CancellationToken,
    task_shutdown_token: CancellationToken,
    tracker: TaskTracker,
    owner_oplogs: Arc<Mutex<Vec<RegisteredOwnerOplog>>>,
    construction_admission: Arc<Mutex<bool>>,
    _watcher_lifetime: Arc<InvocationLoopWatcherLifetime>,
}

struct InvocationLoopWatcherLifetime(CancellationToken);

#[derive(Clone)]
struct RegisteredOwnerOplog {
    oplog: Weak<dyn crate::services::oplog::Oplog>,
    tasks: Option<crate::worker::tasks::WorkerTasks>,
    shutdown: crate::services::oplog::OplogShutdownHandle,
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

/// Identifies a compiled component for module-charge accounting.
type ComponentChargeKey = (ComponentId, ComponentRevision);

/// Guard held by a resident worker keeping its component's module charge alive.
pub type WorkerComponentCharge = ComponentChargeGuard<ComponentChargeKey, GateChargeSource>;

impl<Ctx: WorkerCtx> ActiveAgents<Ctx> {
    #[doc(hidden)]
    pub async fn worker_is_loaded_for_test(&self, owned_agent_id: &OwnedAgentId) -> bool {
        match self.try_get(owned_agent_id).await {
            Some(worker) => worker.is_loaded().await,
            None => false,
        }
    }

    #[doc(hidden)]
    pub async fn worker_has_pending_startup_for_test(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.try_get(owned_agent_id)
            .await
            .is_some_and(|worker| worker.pending_startup_attempt().is_some())
    }

    pub async fn new(
        active_agents_config: &ActiveAgentsConfig,
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
        initial_files_service: Arc<InitialAgentFilesService>,
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
            initial_files_service,
            agent_status_flush_config,
            shutdown_token,
        )
        .await
    }

    /// Like [`Self::new`] but with an explicitly provided memory probe instead of
    /// the one derived from the config. The in-process test harness uses this to
    /// supply a probe with a pinned limit and current usage, so the gate's
    /// decision is deterministic and isolated from the shared test process's RSS.
    pub async fn new_with_probe(
        probe: Box<dyn MemoryProbe>,
        active_agents_config: &ActiveAgentsConfig,
        memory_config: &MemoryConfig,
        storage_config: &FilesystemStorageConfig,
        initial_files_service: Arc<InitialAgentFilesService>,
        agent_status_flush_config: &AgentStatusFlushConfig,
        shutdown_token: CancellationToken,
    ) -> Result<Self, FilesystemStorageError> {
        let agent_filesystems =
            Arc::new(AgentFilesystems::new(storage_config, initial_files_service).await?);
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
        OwnerKind::ComponentAgent
            .validate_instance_name(&owned_agent_id.agent_id.agent_id)
            .map_err(WorkerExecutorError::invalid_request)?;
        let active_agent = self.get_or_add_unresolved(deps, owned_agent_id).await?;
        let worker = active_agent.primary.clone();
        worker
            .ensure_created(
                worker_env,
                worker_agent_config,
                component_revision,
                parent,
                invocation_context_stack.clone(),
                principal,
                freshness_disposition,
                WorkerCreationMode::ComponentAgent,
            )
            .in_current_span()
            .await?;
        Ok(active_agent.primary())
    }

    pub async fn get_or_add_ephemeral_external_tool<T>(
        &self,
        deps: &T,
        component_id: ComponentId,
        environment_id: EnvironmentId,
        idempotency_key: &IdempotencyKey,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let component = deps
            .component_service()
            .get_metadata(component_id, None)
            .await?;
        self.get_or_add_ephemeral_external_tool_pinned(
            deps,
            component_id,
            environment_id,
            idempotency_key,
            component.revision,
            invocation_context_stack,
            principal,
        )
        .await
    }

    pub async fn get_or_add_ephemeral_external_tool_pinned<T>(
        &self,
        deps: &T,
        component_id: ComponentId,
        environment_id: EnvironmentId,
        idempotency_key: &IdempotencyKey,
        component_revision: ComponentRevision,
        invocation_context_stack: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let component = deps
            .component_service()
            .get_metadata(component_id, Some(component_revision))
            .await?;
        if component.environment_id != environment_id {
            return Err(WorkerExecutorError::invalid_request(
                "external tool owner environment does not match the component environment",
            ));
        }
        let owned_agent_id = OwnedAgentId::new(
            environment_id,
            &AgentId {
                component_id,
                agent_id: OwnerKind::external_tool_instance_name(idempotency_key),
            },
        );
        let active_agent = self.get_or_add_unresolved(deps, &owned_agent_id).await?;
        let worker = active_agent.primary.clone();
        worker
            .ensure_created(
                None,
                Vec::new(),
                Some(component_revision),
                None,
                invocation_context_stack.clone(),
                principal,
                InvocationFreshnessDisposition::MayExist,
                WorkerCreationMode::EphemeralExternalTool,
            )
            .in_current_span()
            .await?;
        Ok(active_agent.primary())
    }

    /// Acquires a cached or persisted worker without ever creating a logical agent.
    /// The cache initialization is shared with create-or-load, so absence is decided
    /// inside the single-flight constructor rather than by a racy preflight lookup.
    pub(crate) async fn get_existing<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        self.get_existing_with_fingerprint(deps, owned_agent_id, principal, None)
            .await
    }

    pub(crate) async fn get_existing_with_fingerprint<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
        principal: Principal,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let active_agent = self.get_or_add_unresolved(deps, owned_agent_id).await?;
        let worker = active_agent.primary.clone();
        let result = worker
            .ensure_existing_with_fingerprint(principal, expected_fingerprint)
            .in_current_span()
            .await;
        if matches!(result, Err(WorkerExecutorError::AgentNotFound { .. }))
            && Arc::strong_count(&active_agent) == 2
            && Arc::strong_count(&worker) == 2
        {
            let expected = active_agent.clone();
            self.agents
                .remove_if_cached(owned_agent_id, move |current| {
                    Arc::ptr_eq(current, &expected)
                        && Arc::strong_count(current) == 3
                        && current.primary.can_discard_unresolved()
                })
                .await;
        }
        result.map(|()| active_agent.primary())
    }

    async fn get_or_add_unresolved<T>(
        &self,
        deps: &T,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Arc<ActiveAgent<Ctx>>, WorkerExecutorError>
    where
        T: HasAll<Ctx>,
    {
        let owned_agent_id = owned_agent_id.clone();
        let worker = Worker::unresolved(
            deps,
            self.card_interest_index.clone(),
            owned_agent_id.clone(),
        );
        self.agents
            .get_or_insert_simple(&owned_agent_id.clone(), || {
                Box::pin(async move {
                    Ok(Arc::new(ActiveAgent::new_unresolved(
                        owned_agent_id,
                        worker,
                        self.invocation_loops.clone(),
                    )))
                })
            })
            .await
    }

    pub async fn try_get(&self, owned_agent_id: &OwnedAgentId) -> Option<Arc<Worker<Ctx>>> {
        self.agents
            .get(owned_agent_id)
            .await
            .and_then(|active_agent| active_agent.resolved_primary())
    }

    pub(crate) async fn contains_worker_generation(&self, expected: &Arc<Worker<Ctx>>) -> bool {
        self.agents
            .get(expected.owned_agent_id())
            .await
            .is_some_and(|active_agent| {
                active_agent.primary.is_resolved() && Arc::ptr_eq(&active_agent.primary, expected)
            })
    }

    pub async fn try_get_active_agent(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<Arc<ActiveAgent<Ctx>>> {
        self.agents
            .get(owned_agent_id)
            .await
            .filter(|active_agent| active_agent.primary.is_resolved())
    }

    /// Checks whether an owner group is cached without refreshing its TTL.
    pub async fn contains_cached_agent(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.agents
            .iter()
            .await
            .into_iter()
            .any(|(id, active_agent)| id == *owned_agent_id && active_agent.primary.is_resolved())
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
        if let Some(worker) = self.try_get(owned_agent_id).await {
            self.remove_worker(&worker, false).await;
        }
    }

    /// Removes only the cache generation owned by `expected`. Bookkeeping is cleared only when
    /// that exact generation was still authoritative at the point of removal.
    pub async fn remove_worker(&self, expected: &Arc<Worker<Ctx>>, deletion_owner: bool) -> bool {
        self.remove_worker_with(
            expected,
            deletion_owner,
            OwnerFailureWinner::Lifecycle(InterruptKind::Interrupt(Timestamp::now_utc())),
        )
        .await
    }

    /// [`Self::remove_worker`] with an explicit reason for tearing the agent's entity bodies down.
    /// A lost shard must not report itself as interrupted through the Golem API: it was
    /// not, its shard moved. A deletion owner tears nothing down here, whatever the reason.
    pub(crate) async fn remove_worker_with(
        &self,
        expected: &Arc<Worker<Ctx>>,
        deletion_owner: bool,
        owner_failure: OwnerFailureWinner,
    ) -> bool {
        let owned_agent_id = expected.owned_agent_id().clone();
        let Some(active_agent) = self.agents.get(&owned_agent_id).await else {
            return false;
        };
        if !active_agent.primary.is_resolved() || !Arc::ptr_eq(&active_agent.primary, expected) {
            return false;
        }
        let Some(retirement) = (if deletion_owner {
            expected.try_begin_deletion_cache_retirement().await
        } else {
            expected.try_begin_cache_retirement().await
        }) else {
            return false;
        };
        if !deletion_owner {
            active_agent.fence_entity_bodies(owner_failure).await;
        }
        let expected_active = active_agent.clone();
        let expected_worker = expected.clone();
        let removed = self
            .card_interest_index
            .clear_agent_interest_if(
                &owned_agent_id,
                self.agents
                    .remove_if_cached(&owned_agent_id, move |current| {
                        Arc::ptr_eq(current, &expected_active)
                            && current.primary.is_resolved()
                            && Arc::ptr_eq(&current.primary, &expected_worker)
                    }),
            )
            .await;
        if removed {
            retirement.commit();
        }
        removed
    }

    /// The worker cached for `owned_agent_id`, without waiting on a creation still in progress.
    ///
    /// For callers acting on one particular generation: a pending or still-unresolved entry is a
    /// newer generation being created, never the one they hold, so waiting on it could only delay
    /// them.
    pub(crate) async fn try_get_cached(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<Arc<Worker<Ctx>>> {
        self.agents
            .try_get(owned_agent_id)
            .await
            .and_then(|active_agent| active_agent.resolved_primary())
    }

    /// [`Self::remove_worker_with`] for a caller holding the generation by reference: tears the
    /// entry down and drops it only while it still holds `worker`. Returns whether it did.
    ///
    /// A retiring generation can reach its removal after a newer one is cached under the same id -
    /// its retirement started from a stop through a handle kept past its generation. Keyed by id
    /// alone, such a pass evicts that generation and fences its entity bodies while its loop keeps
    /// running.
    ///
    /// A removal refused while this generation is still cached is retried, unless a deletion owns
    /// its retirement and removes it itself. The only other refusal is the retirement marker held
    /// by a concurrent attempt - an idle expiry, or another pass of this removal - which ends with
    /// the generation removed or the marker rolled back. Without the retry, an agent retired
    /// while an idle expiry happened to be checking it would stay cached here, and a later
    /// re-grant of its shard would find this retired generation instead of opening the oplog at
    /// the new epoch.
    pub(crate) async fn remove_generation(
        &self,
        worker: &Worker<Ctx>,
        owner_failure: OwnerFailureWinner,
    ) -> bool {
        loop {
            let Some(cached) = self.try_get_cached(worker.owned_agent_id()).await else {
                return false;
            };
            if !std::ptr::eq(Arc::as_ptr(&cached), worker) {
                return false;
            }
            if self
                .remove_worker_with(&cached, false, owner_failure.clone())
                .await
            {
                return true;
            }
            if cached.deletion_owns_retirement().await {
                return false;
            }
            drop(cached);
            // The concurrent attempt may be draining entity bodies; poll rather than spin.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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

            let scope = crate::worker::tasks::TaskScope::default();
            if scope.bind(&worker.tasks).is_err() {
                continue;
            }
            scope
                .run(worker.queue_card_revocations(&affected_card_ids))
                .await;
        }
    }

    pub async fn snapshot(&self) -> Vec<(AgentId, Arc<Worker<Ctx>>)> {
        self.agents
            .iter()
            .await
            .into_iter()
            .filter_map(|(_, active_agent)| {
                active_agent
                    .resolved_primary()
                    .map(|primary| (primary.agent_id(), primary))
            })
            .collect()
    }

    /// Retires every agent the predicate selects for its lost shard: stops each one here and drops
    /// it from this executor, so the shard's new owner recovers it.
    ///
    /// Concurrent rather than sequential, unlike [`Self::unload_environment`]: a revoke can name
    /// many agents and each stop waits for that agent's invocation loop to exit. No acknowledgement
    /// channel is awaited either - [`Worker::interrupt_and_retire`] never subscribes to one for a
    /// lost shard - so an agent that is already stopping cannot panic the sweep, which is what the
    /// old `set_interrupting(..).recv().await.unwrap()` shape risked.
    ///
    /// The snapshot includes suspended, loading and already-stopping agents; the stop state
    /// machine has an arm for each, so none is skipped. An agent still being resolved is not in
    /// it: one that read the assignment before the shard left opens its oplog at the epoch it
    /// was granted, and checks the assignment again once it is published - see
    /// `Worker::retire_if_shard_left_during_construction`.
    pub(crate) async fn give_up_matching(
        &self,
        select: impl Fn(&AgentId) -> bool,
        reason: RetirementReason,
    ) {
        let selected: Vec<Arc<Worker<Ctx>>> = self
            .snapshot()
            .await
            .into_iter()
            .filter(|(agent_id, _)| select(agent_id))
            .map(|(_, worker)| worker)
            .collect();

        if !selected.is_empty() {
            info!(
                agents = selected.len(),
                "Retiring agents whose shard has moved"
            );
        }

        // Concurrent, and a failed retirement never stops the sweep: each agent's own answer is
        // the routing miss that sends its callers to the new owner.
        futures::future::join_all(selected.into_iter().map(|worker| {
            let reason = reason.clone();
            async move {
                let _ = worker
                    .interrupt_and_retire(InterruptKind::ShardLost, reason)
                    .await;
            }
        }))
        .await;
    }

    /// Interrupts and unloads all in-memory workers whose environment matches
    /// `environment_id`.  Called when the environment is deleted so that
    /// running workers stop promptly.
    pub async fn unload_environment(&self, environment_id: EnvironmentId) {
        for (_agent_id, worker) in self.snapshot().await {
            if worker.get_initial_worker_metadata().environment_id == environment_id
                && let Err(error) = worker
                    .interrupt_and_retire(
                        InterruptKind::Interrupt(Timestamp::now_utc()),
                        RetirementReason::Requested,
                    )
                    .await
            {
                tracing::error!(
                    agent_id = %worker.owned_agent_id(),
                    error = %error,
                    "Failed to retire worker from deleted environment"
                );
            }
        }
    }

    /// Register an account with the per-account concurrent agent semaphore.
    ///
    /// Must be called (from `Worker::new`) before any concurrent-agent permit
    /// acquire for the account. Idempotent — safe to call multiple times.
    #[doc(hidden)]
    pub async fn register_account_concurrency(
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

pub(crate) struct FilesystemPressureVictim<Ctx: WorkerCtx> {
    stable_agent_id: String,
    eligible_since: u64,
    worker: Arc<Worker<Ctx>>,
    eligibility: FilesystemPressureEligibility,
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

/// A source of evictable, already-resident memory the gate reclaims through.
struct WorkerEvictionSource<Ctx: WorkerCtx> {
    agents: Cache<OwnedAgentId, (), Arc<ActiveAgent<Ctx>>, WorkerExecutorError>,
    component_charges: Arc<ComponentChargeRegistry<ComponentChargeKey, GateChargeSource>>,
    component_size_coefficient: f64,
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
