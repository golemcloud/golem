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

use super::Worker;
use super::entity_slot::{EntitySlot, EntitySlotRegistration};
use super::owner_lane::OwnerLane;
use super::state_actor::OwnerCommitController;
use crate::durable_host::replay_state::ReplayState;
use crate::durable_host::tool::operation::{DeferredAdmissionTable, OwnerToolOperations};
use crate::model::ExecutionStatus;
use crate::services::active_agents::WorkerComponentCharge;
use crate::services::agent_filesystem::FilesystemGenerationHandle;
use crate::services::oplog::{CommitLevel, Oplog};
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::{HasActiveAgents, HasComponentService, HasWasmtimeEngine};
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::OwnedAgentId;
use golem_common::model::entity::{
    EntityActivation, EntityInvocationScope, ExecutableTarget, FilesystemCapability,
    InvocationExecutionMode, OwnerRuntime,
};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::DeletedRegions;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::component::Component as ComponentMetadata;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tracing::warn;
use wasmtime::component::{Component, Instance};
use wasmtime::{AsContextMut, Store, StoreMemory, UpdateDeadline};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinearMemoryEnumerationError {
    Shared,
    Overflow,
}

pub(super) fn allocated_linear_memory_bytes<T>(
    store: &Store<T>,
) -> Result<u64, LinearMemoryEnumerationError> {
    store
        .linear_memories()
        .iter()
        .try_fold(0u64, |allocated_bytes, memory| match memory {
            StoreMemory::Unshared(memory) => allocated_bytes
                .checked_add(memory.data_size(store) as u64)
                .ok_or(LinearMemoryEnumerationError::Overflow),
            StoreMemory::Shared(_) => Err(LinearMemoryEnumerationError::Shared),
        })
}

struct StoreFuelGuard<Ctx: crate::workerctx::FuelManagement + 'static> {
    store: Option<Store<Ctx>>,
}

impl<Ctx: crate::workerctx::FuelManagement + 'static> StoreFuelGuard<Ctx> {
    fn new(store: Store<Ctx>) -> Self {
        Self { store: Some(store) }
    }

    fn settle(&mut self) {
        if let Some(store) = self.store.as_mut()
            && let Ok(current_fuel_level) = store.get_fuel()
        {
            store.data_mut().settle_fuel(current_fuel_level);
        }
    }

    fn into_inner(mut self) -> Store<Ctx> {
        self.settle();
        self.store.take().unwrap()
    }
}

impl<Ctx: crate::workerctx::FuelManagement + 'static> Deref for StoreFuelGuard<Ctx> {
    type Target = Store<Ctx>;

    fn deref(&self) -> &Self::Target {
        self.store.as_ref().unwrap()
    }
}

impl<Ctx: crate::workerctx::FuelManagement + 'static> DerefMut for StoreFuelGuard<Ctx> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.store.as_mut().unwrap()
    }
}

impl<Ctx: crate::workerctx::FuelManagement + 'static> Drop for StoreFuelGuard<Ctx> {
    fn drop(&mut self) {
        self.settle();
    }
}

/// The durable execution stream shared by every Store belonging to one owner.
///
/// A replay generation is installed by the primary Store and cloned by entity Stores. Cloning a
/// [`ReplayState`] shares its cursor; it does not create another cursor over the same oplog.
pub struct OwnerExecution {
    owner_id: OwnedAgentId,
    oplog: Arc<dyn Oplog>,
    replay: tokio::sync::RwLock<Option<ReplayState>>,
    commit: Arc<OwnerCommitController>,
    lane: OwnerLane,
    primary_tail_work: crate::durable_host::tail_work::TailWorkTracker,
    tool_operations: Arc<OwnerToolOperations>,
    deferred_tool_admission: Arc<DeferredAdmissionTable>,
    reached_oplog_marker: AtomicU64,
    #[cfg(feature = "test-utils")]
    monotonic_clock_now_gate: Mutex<Option<Arc<ClockNowGate>>>,
    #[cfg(feature = "test-utils")]
    wall_clock_now_gate: Mutex<Option<Arc<ClockNowGate>>>,
    #[cfg(feature = "test-utils")]
    skip_monotonic_clock_now_durability: AtomicBool,
    #[cfg(feature = "test-utils")]
    skip_wall_clock_now_durability: AtomicBool,
}

#[cfg(feature = "test-utils")]
struct ClockNowGate {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Semaphore,
}

#[cfg(feature = "test-utils")]
pub struct ClockNowGateHandle {
    entered: tokio::sync::oneshot::Receiver<()>,
    gate: Arc<ClockNowGate>,
}

#[cfg(feature = "test-utils")]
impl ClockNowGateHandle {
    pub async fn entered(&mut self) {
        (&mut self.entered)
            .await
            .expect("clock now gate was dropped without firing");
    }

    pub fn release(&self) {
        self.gate.release.add_permits(1);
    }
}

#[cfg(feature = "test-utils")]
impl Drop for ClockNowGateHandle {
    fn drop(&mut self) {
        self.gate.release.add_permits(1);
    }
}

impl OwnerExecution {
    pub(crate) fn new(
        owner_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        commit: Arc<OwnerCommitController>,
    ) -> Self {
        let lane = OwnerLane::new(owner_id.clone());
        let primary_tail_work = crate::durable_host::tail_work::TailWorkTracker::new();
        Self {
            owner_id,
            oplog,
            replay: tokio::sync::RwLock::new(None),
            commit,
            lane,
            tool_operations: OwnerToolOperations::new(),
            primary_tail_work,
            deferred_tool_admission: Arc::new(DeferredAdmissionTable::default()),
            reached_oplog_marker: AtomicU64::new(OplogIndex::NONE.into()),
            #[cfg(feature = "test-utils")]
            monotonic_clock_now_gate: Mutex::new(None),
            #[cfg(feature = "test-utils")]
            wall_clock_now_gate: Mutex::new(None),
            #[cfg(feature = "test-utils")]
            skip_monotonic_clock_now_durability: AtomicBool::new(false),
            #[cfg(feature = "test-utils")]
            skip_wall_clock_now_durability: AtomicBool::new(false),
        }
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        &self.owner_id
    }

    pub fn oplog(&self) -> Arc<dyn Oplog> {
        self.oplog.clone()
    }

    pub fn lane(&self) -> OwnerLane {
        self.lane.clone()
    }

    pub fn tool_operation_metadata(&self) -> crate::durable_host::tool::ToolOperationSetMetadata {
        self.tool_operations.metadata()
    }

    pub(crate) fn tool_operations(&self) -> Arc<OwnerToolOperations> {
        self.tool_operations.clone()
    }

    pub(crate) fn primary_tail_work_tracker(
        &self,
    ) -> crate::durable_host::tail_work::TailWorkTracker {
        self.primary_tail_work.clone()
    }

    pub(crate) fn deferred_tool_admission(&self) -> Arc<DeferredAdmissionTable> {
        self.deferred_tool_admission.clone()
    }

    pub(crate) fn mark_reached_oplog_marker(&self, marker: OplogIndex) {
        self.reached_oplog_marker
            .store(marker.into(), Ordering::Release);
    }

    pub fn reached_oplog_marker(&self) -> Option<OplogIndex> {
        let marker = OplogIndex::from_u64(self.reached_oplog_marker.load(Ordering::Acquire));
        (marker != OplogIndex::NONE).then_some(marker)
    }

    pub(crate) async fn begin_replay_generation(
        &self,
        deleted_regions: DeletedRegions,
        initial_snapshot_skip_end: Option<OplogIndex>,
    ) -> Result<ReplayState, WorkerExecutorError> {
        self.install_replay_generation(deleted_regions, initial_snapshot_skip_end)
            .await?;
        self.replay().await
    }

    /// Installs a fresh owner replay cursor. Lifecycle recovery uses the returned cursor through
    /// [`Self::replay`]; exposing the installation step also lets integration tests reconstruct
    /// transient entity Stores without inventing a persistent child runtime.
    pub async fn install_replay_generation(
        &self,
        deleted_regions: DeletedRegions,
        initial_snapshot_skip_end: Option<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        if let Some(replay) = self.replay.read().await.as_ref() {
            replay.ensure_reconstruction_claims_empty()?;
        }
        self.tool_operations.begin_generation()?;
        self.deferred_tool_admission.begin_generation()?;
        self.reached_oplog_marker
            .store(OplogIndex::NONE.into(), Ordering::Release);
        let replay = ReplayState::new_for_owner(
            self.owner_id.clone(),
            self.oplog.clone(),
            deleted_regions,
            initial_snapshot_skip_end,
            self.tool_operations.clone(),
        )
        .await?;
        *self.replay.write().await = Some(replay.clone());
        Ok(())
    }

    pub(crate) async fn replay(&self) -> Result<ReplayState, WorkerExecutorError> {
        self.replay.read().await.clone().ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "Owner execution for {} has no active replay generation",
                self.owner_id
            ))
        })
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_drain_terminal_clamp_then_reconstruction_barrier(
        &self,
        start_index: OplogIndex,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>, WorkerExecutorError> {
        let replay = self.replay().await?;
        replay
            .test_drain_terminal_clamp_then_reconstruction_barrier(start_index)
            .await
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_drain_reconstruction_terminal(
        &self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.replay()
            .await?
            .test_drain_reconstruction_terminal(start_index)
            .await
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_clamp_after_claim(
        &self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.replay()
            .await?
            .test_clamp_after_claim(start_index)
            .await
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_replay_is_live(&self) -> Result<bool, WorkerExecutorError> {
        Ok(self.replay().await?.is_live_published())
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_replay_is_settling(&self) -> Result<bool, WorkerExecutorError> {
        Ok(self.replay().await?.test_is_settling())
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub async fn test_wait_for_tool_owner_failure(
        &self,
    ) -> crate::durable_host::tool::ToolOperationSetMetadata {
        self.tool_operations.wait_for_owner_failure().await;
        self.tool_operation_metadata()
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub fn test_gate_next_monotonic_clock_now(&self) -> ClockNowGateHandle {
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let gate = Arc::new(ClockNowGate {
            entered: Mutex::new(Some(entered_tx)),
            release: tokio::sync::Semaphore::new(0),
        });
        *self.monotonic_clock_now_gate.lock().unwrap() = Some(gate.clone());
        ClockNowGateHandle { entered, gate }
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub fn test_gate_next_wall_clock_now(&self) -> ClockNowGateHandle {
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let gate = Arc::new(ClockNowGate {
            entered: Mutex::new(Some(entered_tx)),
            release: tokio::sync::Semaphore::new(0),
        });
        *self.wall_clock_now_gate.lock().unwrap() = Some(gate.clone());
        ClockNowGateHandle { entered, gate }
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub fn test_skip_next_monotonic_clock_now_durability(&self) {
        self.skip_monotonic_clock_now_durability
            .store(true, Ordering::Release);
    }

    #[cfg(feature = "test-utils")]
    #[doc(hidden)]
    pub fn test_skip_next_wall_clock_now_durability(&self) {
        self.skip_wall_clock_now_durability
            .store(true, Ordering::Release);
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn test_should_skip_monotonic_clock_now_durability(&self) -> bool {
        self.skip_monotonic_clock_now_durability
            .swap(false, Ordering::AcqRel)
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn test_should_skip_wall_clock_now_durability(&self) -> bool {
        self.skip_wall_clock_now_durability
            .swap(false, Ordering::AcqRel)
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn test_before_monotonic_clock_now(&self) {
        let gate = self.monotonic_clock_now_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            if let Some(entered) = gate.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            gate.release
                .acquire()
                .await
                .expect("monotonic-clock now gate was closed")
                .forget();
        }
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn test_before_wall_clock_now(&self) {
        let gate = self.wall_clock_now_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            if let Some(entered) = gate.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            gate.release
                .acquire()
                .await
                .expect("wall-clock now gate was closed")
                .forget();
        }
    }

    pub async fn commit(&self, level: CommitLevel) -> OplogIndex {
        self.commit.commit_and_update_state(level).await.0
    }

    pub async fn add_and_commit(&self, entry: OplogEntry) -> OplogIndex {
        let index = self.oplog.add(entry).await;
        self.commit(CommitLevel::Always).await;
        index
    }
}

/// Owner-scoped runtime resources reused by primary and entity Store construction.
pub struct OwnerRuntimeResources {
    resource_limits: Arc<AtomicResourceEntry>,
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    // This weak lifecycle handle lets entity Stores attach during reconstruction or residence
    // without sharing or owning the AgentFilesystem itself.
    filesystem_generation: Mutex<Option<FilesystemGenerationHandle>>,
}

impl OwnerRuntimeResources {
    pub(crate) fn new(
        resource_limits: Arc<AtomicResourceEntry>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    ) -> Self {
        Self {
            resource_limits,
            execution_status,
            filesystem_generation: Mutex::new(None),
        }
    }

    pub fn resource_limits(&self) -> Arc<AtomicResourceEntry> {
        self.resource_limits.clone()
    }

    pub fn execution_status(&self) -> Arc<std::sync::RwLock<ExecutionStatus>> {
        self.execution_status.clone()
    }

    pub(crate) fn activate_filesystem_generation(&self, generation: FilesystemGenerationHandle) {
        *self.filesystem_generation.lock().unwrap() = Some(generation);
    }

    pub(crate) fn filesystem_generation_handle(
        &self,
    ) -> Result<FilesystemGenerationHandle, WorkerExecutorError> {
        self.filesystem_generation
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                WorkerExecutorError::runtime("Owner filesystem has no active generation")
            })
    }

    pub(crate) fn fence_filesystem_generation(&self) {
        self.filesystem_generation.lock().unwrap().take();
    }
}

/// Shared Store-driving layer used by the primary Worker and transient entity instances.
pub struct InstanceHost<Ctx: WorkerCtx> {
    owner_id: OwnedAgentId,
    runtime: OwnerRuntime,
    executable: ExecutableTarget,
    filesystem: FilesystemCapability,
    owner_execution: Arc<OwnerExecution>,
    owner_resources: Arc<OwnerRuntimeResources>,
    owner: Weak<Worker<Ctx>>,
    slot: Option<Arc<EntitySlot>>,
    activation: Option<Arc<EntityActivation>>,
    owner_component_metadata: Option<Arc<ComponentMetadata>>,
}

impl<Ctx: WorkerCtx> InstanceHost<Ctx> {
    pub(crate) fn new(
        owner: &Arc<Worker<Ctx>>,
        runtime: OwnerRuntime,
        executable: ExecutableTarget,
    ) -> Result<Self, WorkerExecutorError> {
        if runtime != OwnerRuntime::Agent {
            return Err(WorkerExecutorError::runtime(
                "Entity instance hosts require a pinned activation and entity slot",
            ));
        }
        if runtime == OwnerRuntime::Agent && executable.component_id != owner.component_id() {
            return Err(WorkerExecutorError::runtime(
                "Primary instance executable must be the owner's component",
            ));
        }
        Ok(Self {
            owner_id: owner.owned_agent_id().clone(),
            runtime,
            executable,
            filesystem: FilesystemCapability::Capable,
            owner_execution: owner.owner_execution(),
            owner_resources: owner.owner_runtime_resources(),
            owner: Arc::downgrade(owner),
            slot: None,
            activation: None,
            owner_component_metadata: None,
        })
    }

    pub(crate) fn new_entity(
        owner: &Arc<Worker<Ctx>>,
        activation: &EntityActivation,
        slot: Arc<EntitySlot>,
        owner_component_metadata: Arc<ComponentMetadata>,
    ) -> Result<Self, WorkerExecutorError> {
        if slot.entity_id().owner_id() != owner.owned_agent_id() {
            return Err(WorkerExecutorError::runtime(
                "Entity slot does not belong to the instance owner",
            ));
        }
        Ok(Self {
            owner_id: owner.owned_agent_id().clone(),
            runtime: OwnerRuntime::Entity(slot.entity().clone()),
            executable: activation.executable().clone(),
            filesystem: activation.filesystem(),
            owner_execution: owner.owner_execution(),
            owner_resources: owner.owner_runtime_resources(),
            owner: Arc::downgrade(owner),
            slot: Some(slot),
            activation: Some(Arc::new(activation.clone())),
            owner_component_metadata: Some(owner_component_metadata),
        })
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        &self.owner_id
    }

    pub fn runtime(&self) -> &OwnerRuntime {
        &self.runtime
    }

    pub fn executable(&self) -> &ExecutableTarget {
        &self.executable
    }

    pub fn filesystem(&self) -> FilesystemCapability {
        self.filesystem
    }

    pub fn owner_execution(&self) -> Arc<OwnerExecution> {
        self.owner_execution.clone()
    }

    pub fn owner_resources(&self) -> Arc<OwnerRuntimeResources> {
        self.owner_resources.clone()
    }

    pub(crate) fn owner(&self) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        self.owner.upgrade().ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "Owner {} was dropped while constructing an instance",
                self.owner_id
            ))
        })
    }

    /// Resolves the executable pinned by this host. Entity hosts can point at a component and
    /// revision unrelated to the owner's routing component.
    pub async fn activate(&self) -> Result<(Component, ComponentMetadata), WorkerExecutorError> {
        let owner = self.owner()?;
        owner
            .component_service()
            .get(
                &owner.engine(),
                self.executable.component_id,
                self.executable.component_revision,
            )
            .await
    }

    pub(crate) async fn instantiate(
        &self,
        context: Ctx,
        component: &Component,
    ) -> Result<HostedInstance<Ctx>, WorkerExecutorError> {
        let owner = self.owner()?;
        let engine = owner.engine();
        let mut store = Store::new(&engine, context);
        store.set_epoch_deadline(0);
        store.epoch_deadline_callback(move |mut store| {
            let current_level = store.get_fuel().unwrap_or(0);
            let data_mut = store.data_mut();
            if let Err(error) = data_mut.ensure_fuel(current_level) {
                if data_mut.agent_mode() == golem_common::model::agent::AgentMode::Ephemeral {
                    warn!(error = ?error, "Could not borrow more fuel for ephemeral agent");
                    return Err(WorkerExecutorError::InvocationFailed {
                        error,
                        stderr: String::new(),
                    }
                    .into());
                }
                warn!("Could not borrow more fuel, suspending");
                return Err(
                    InterruptKind::Suspend(golem_common::model::Timestamp::now_utc()).into(),
                );
            }

            match data_mut.check_interrupt() {
                Some(kind) => Err(kind.into()),
                None => Ok(UpdateDeadline::YieldCustom(
                    1,
                    tokio::task::yield_now().boxed(),
                )),
            }
        });
        store
            .set_fuel(u64::MAX)
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        store.limiter_async(|ctx| ctx.resource_limiter());
        let mut store = StoreFuelGuard::new(store);

        let linker = (*owner.linker()).clone();
        let instance_pre = linker
            .instantiate_pre(component)
            .map_err(|error| self.creation_error(error.into()))?;
        let instance = instance_pre
            .instantiate_async(&mut *store)
            .await
            .map_err(|error| {
                if let Some(kind) = error.root_cause().downcast_ref::<InterruptKind>() {
                    WorkerExecutorError::Interrupted { kind: *kind }
                } else {
                    self.creation_error(error.into())
                }
            })?;

        Ok(HostedInstance {
            instance,
            store,
            runtime: self.runtime.clone(),
            executable: self.executable.clone(),
            filesystem: self.filesystem,
            owner_id: self.owner_id.clone(),
            slot: self.slot.clone(),
            _component_charge: None,
        })
    }

    pub(super) async fn reconcile_linear_memories(
        &self,
        hosted: &mut HostedInstance<Ctx>,
    ) -> Result<(), WorkerExecutorError> {
        let owner = self.owner()?;
        let durable = hosted.store.data().durable_ctx();
        let tracker = durable.linear_memory_tracker();
        let limit = durable.max_linear_memory_size();
        let allocated_bytes =
            allocated_linear_memory_bytes(&hosted.store).map_err(|error| match error {
                LinearMemoryEnumerationError::Shared => super::shared_linear_memory_error(&owner),
                LinearMemoryEnumerationError::Overflow => {
                    WorkerExecutorError::runtime("linear-memory allocation total overflowed")
                }
            })?;

        let required_grant_bytes = tracker.reconciliation_grant_bytes(allocated_bytes);
        if required_grant_bytes > limit {
            return Err(WorkerExecutorError::worker_creation_failed(
                self.owner_id.agent_id(),
                format!(
                    "Linear memories require {required_grant_bytes} bytes, exceeding the per-worker limit of {limit} bytes"
                ),
            ));
        }

        let retained_grant = tracker.retained_growth_grant();
        let (grant_is_tracked, granted_bytes) = {
            let grant = retained_grant.lock().unwrap();
            (grant.is_tracked(), grant.bytes())
        };
        if grant_is_tracked && required_grant_bytes > granted_bytes {
            let additional_grant = owner
                .active_agents()
                .acquire_memory(required_grant_bytes - granted_bytes)
                .await;
            retained_grant.lock().unwrap().merge(additional_grant);
        }
        if grant_is_tracked {
            retained_grant
                .lock()
                .unwrap()
                .shrink_to(required_grant_bytes);
        }

        if self.runtime == OwnerRuntime::Agent {
            owner
                .memory_limit_interrupt_queued
                .store(false, Ordering::Release);
        }

        let live_instantiation_growth =
            tracker.reconcile(allocated_bytes, std::time::Instant::now());
        crate::metrics::wasm::record_worker_allocated_linear_memory(allocated_bytes);
        if self.runtime == OwnerRuntime::Agent && live_instantiation_growth > 0 {
            // Commit the growth oplog entry before publishing the primary instance. Otherwise a
            // process crash can replay and persist the same instantiation growth again.
            owner
                .add_and_commit_oplog(OplogEntry::grow_memory(live_instantiation_growth))
                .await;
            owner
                .startup_linear_memory_bytes
                .store(allocated_bytes, Ordering::Release);
        }

        Ok(())
    }

    pub async fn instantiate_entity(&self) -> Result<HostedInstance<Ctx>, WorkerExecutorError> {
        self.instantiate_entity_with_scope(None).await
    }

    pub(crate) async fn instantiate_entity_scoped(
        &self,
        scope: &EntityInvocationScope,
    ) -> Result<HostedInstance<Ctx>, WorkerExecutorError> {
        self.instantiate_entity_with_scope(Some(scope)).await
    }

    async fn instantiate_entity_with_scope(
        &self,
        scope: Option<&EntityInvocationScope>,
    ) -> Result<HostedInstance<Ctx>, WorkerExecutorError> {
        if !matches!(&self.runtime, OwnerRuntime::Entity(_)) {
            return Err(WorkerExecutorError::runtime(
                "Entity instantiation requires an entity instance host",
            ));
        }
        if let Some(scope) = scope {
            let OwnerRuntime::Entity(entity) = &self.runtime else {
                unreachable!("validated above")
            };
            if scope.owner_id() != &self.owner_id
                || scope.invocation_id().entity() != entity
                || scope.activation().executable() != &self.executable
                || scope.activation().filesystem() != self.filesystem
            {
                return Err(WorkerExecutorError::runtime(
                    "Entity invocation scope does not match its instance host",
                ));
            }
        }
        let owner = self.owner()?;
        let (component, component_metadata) = self.activate().await?;
        let component_charge = owner
            .active_agents()
            .acquire_component_charge(
                component_metadata.id,
                component_metadata.revision,
                component_metadata.component_size,
            )
            .await;
        let mut context = owner
            .create_entity_context(
                self.runtime.clone(),
                self.filesystem,
                component_metadata,
                self.activation.clone(),
                self.owner_component_metadata
                    .clone()
                    .expect("Entity instance host must pin its owner component metadata"),
            )
            .await?;
        if let Some(scope) = scope {
            context.set_entity_invocation_scope(Some(scope.clone()))?;
        }
        let mut hosted = self.instantiate(context, &component).await?;
        self.reconcile_linear_memories(&mut hosted).await?;
        hosted._component_charge = Some(component_charge);
        Ok(hosted)
    }

    fn creation_error(&self, error: anyhow::Error) -> WorkerExecutorError {
        WorkerExecutorError::worker_creation_failed(
            self.owner_id.agent_id(),
            format!(
                "Failed to instantiate {} executable {}@{}: {error:#}",
                match &self.runtime {
                    OwnerRuntime::Agent => "primary",
                    OwnerRuntime::Entity(entity) =>
                        return WorkerExecutorError::worker_creation_failed(
                            self.owner_id.agent_id(),
                            format!(
                                "Failed to instantiate entity {entity} executable {}@{}: {error:#}",
                                self.executable.component_id, self.executable.component_revision
                            ),
                        ),
                },
                self.executable.component_id,
                self.executable.component_revision
            ),
        )
    }
}

/// One instantiated component and its private Store.
pub struct HostedInstance<Ctx: WorkerCtx> {
    instance: Instance,
    store: StoreFuelGuard<Ctx>,
    runtime: OwnerRuntime,
    executable: ExecutableTarget,
    filesystem: FilesystemCapability,
    owner_id: OwnedAgentId,
    slot: Option<Arc<EntitySlot>>,
    _component_charge: Option<WorkerComponentCharge>,
}

pub trait EntityInvocationBody<Ctx: WorkerCtx, R>: Send + 'static {
    fn invoke<'a>(
        self,
        instance: &'a Instance,
        store: &'a mut Store<Ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>>;
}

pub(crate) struct ClosureEntityInvocationBody<F>(pub(crate) F);

impl<Ctx, R, F> EntityInvocationBody<Ctx, R> for ClosureEntityInvocationBody<F>
where
    Ctx: WorkerCtx,
    F: Send + 'static,
    F: for<'a> FnOnce(
        &'a Instance,
        &'a mut Store<Ctx>,
    )
        -> Pin<Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>>,
{
    fn invoke<'a>(
        self,
        instance: &'a Instance,
        store: &'a mut Store<Ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>> {
        self.0(instance, store)
    }
}

impl<Ctx: WorkerCtx> HostedInstance<Ctx> {
    pub(crate) fn into_parts(self) -> (Instance, Store<Ctx>) {
        (self.instance, self.store.into_inner())
    }

    pub(crate) async fn prepare_tool_parent_end(
        &mut self,
        parent: crate::worker::owner_lane::OwnerInvocationId,
    ) -> Result<(), WorkerExecutorError> {
        crate::durable_host::tool::prepare_tool_parent_end(&mut self.store.as_context_mut(), parent)
            .await
    }

    pub(crate) async fn settle_tool_children(
        &mut self,
        parent: crate::worker::owner_lane::OwnerInvocationId,
    ) -> Result<(), WorkerExecutorError> {
        crate::durable_host::tool::settle_tool_children(&mut self.store.as_context_mut(), parent)
            .await
    }

    /// Runs one entity export with an installed invocation scope and then destroys its Store.
    /// The owned task keeps an already-launched invocation running when its caller is cancelled;
    /// every completion path settles fuel and drops the transient Store.
    pub async fn invoke_scoped<R, F>(
        self,
        scope: EntityInvocationScope,
        invoke: F,
    ) -> Result<R, WorkerExecutorError>
    where
        R: Send + 'static,
        F: Send + 'static,
        F: for<'a> FnOnce(
            &'a Instance,
            &'a mut Store<Ctx>,
        ) -> Pin<
            Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>,
        >,
    {
        tokio::spawn(async move {
            let registration = self
                .slot
                .as_ref()
                .ok_or_else(|| WorkerExecutorError::runtime("Entity instance has no slot"))?
                .register(&scope)?;
            self.invoke_scoped_inner(scope, &registration, ClosureEntityInvocationBody(invoke))
                .await
        })
        .await
        .map_err(|error| {
            WorkerExecutorError::runtime(if error.is_panic() {
                "Entity invocation task panicked".to_string()
            } else {
                format!("Entity invocation task was cancelled: {error}")
            })
        })?
    }

    pub(crate) async fn invoke_scoped_registered_retained<R, F>(
        mut self,
        scope: EntityInvocationScope,
        registration: &EntitySlotRegistration,
        invoke: F,
    ) -> (Result<R, WorkerExecutorError>, Self)
    where
        R: Send + 'static,
        F: EntityInvocationBody<Ctx, R>,
    {
        let result = self
            .invoke_scoped_inner_retained(scope, registration, invoke)
            .await;
        (result, self)
    }

    async fn invoke_scoped_inner<R, F>(
        mut self,
        scope: EntityInvocationScope,
        registration: &EntitySlotRegistration,
        invoke: F,
    ) -> Result<R, WorkerExecutorError>
    where
        R: Send + 'static,
        F: EntityInvocationBody<Ctx, R>,
    {
        self.invoke_scoped_inner_retained(scope, registration, invoke)
            .await
    }

    async fn invoke_scoped_inner_retained<R, F>(
        &mut self,
        scope: EntityInvocationScope,
        registration: &EntitySlotRegistration,
        invoke: F,
    ) -> Result<R, WorkerExecutorError>
    where
        R: Send + 'static,
        F: EntityInvocationBody<Ctx, R>,
    {
        let OwnerRuntime::Entity(entity) = &self.runtime else {
            return Err(WorkerExecutorError::runtime(
                "invoke_scoped can only be used with an entity instance",
            ));
        };
        if scope.owner_id() != &self.owner_id
            || scope.invocation_id().entity() != entity
            || scope.activation().executable() != &self.executable
            || scope.activation().filesystem() != self.filesystem
        {
            return Err(WorkerExecutorError::runtime(
                "Entity invocation scope does not match its instance host",
            ));
        }

        registration
            .attach_linear_memory(self.store.data().durable_ctx().linear_memory_tracker())?;
        let execution_mode = scope.mode();
        let body_hook = self.store.data().entity_invocation_body_hook();

        match self.store.data().entity_invocation_scope() {
            Some(installed) if installed != &scope => {
                return Err(WorkerExecutorError::runtime(
                    "Entity invocation scope does not match the scope installed during instantiation",
                ));
            }
            Some(_) => {}
            None => self
                .store
                .data_mut()
                .set_entity_invocation_scope(Some(scope))?,
        }
        if let Some(hook) = body_hook.as_ref() {
            hook.before_invocation(execution_mode).await;
        }
        let mut result =
            std::panic::AssertUnwindSafe(invoke.invoke(&self.instance, &mut self.store))
                .catch_unwind()
                .await;
        if result.is_ok()
            && let Some(hook) = body_hook.as_ref()
        {
            hook.before_completion(execution_mode).await;
        }
        if execution_mode == InvocationExecutionMode::ReplayingCompleted
            && let Ok(Ok(response)) = &mut result
            && let Some(response) = (response as &mut dyn std::any::Any)
                .downcast_mut::<golem_common::model::oplog::HostResponseEntityInvocation>(
            )
            && let Some(hook) = body_hook
        {
            hook.mutate_completed_reconstruction_response(response);
        }
        let cleanup = self.finish_scoped_invocation();

        match result {
            Ok(result) => {
                cleanup?;
                result
            }
            Err(payload) => {
                let _ = cleanup;
                let details = payload
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic payload".to_string());
                Err(WorkerExecutorError::runtime(format!(
                    "Entity invocation body panicked: {details}"
                )))
            }
        }
    }

    fn finish_scoped_invocation(&mut self) -> Result<(), WorkerExecutorError> {
        self.store.data_mut().set_entity_invocation_scope(None)
    }
}

#[cfg(test)]
mod tests {
    use super::StoreFuelGuard;
    use crate::workerctx::FuelManagement;
    use golem_common::model::oplog::AgentError;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use test_r::test;
    use wasmtime::{Config, Engine, Store};

    struct FuelTestContext {
        borrowed: bool,
        returned_at: Arc<AtomicU64>,
    }

    impl FuelManagement for FuelTestContext {
        fn fuel_metering_enabled(&self) -> bool {
            true
        }

        fn ensure_fuel(&mut self, _current_level: u64) -> Result<(), AgentError> {
            self.borrowed = true;
            Ok(())
        }

        fn return_fuel(&mut self, current_level: u64) -> u64 {
            self.settle_fuel(current_level);
            0
        }

        fn settle_fuel(&mut self, current_level: u64) {
            assert!(self.borrowed);
            self.returned_at.store(current_level, Ordering::Release);
        }
    }

    #[test]
    fn dropping_guard_settles_fuel_borrowed_before_invocation() -> anyhow::Result<()> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        let returned_at = Arc::new(AtomicU64::new(0));
        let mut store = Store::new(
            &engine,
            FuelTestContext {
                borrowed: false,
                returned_at: returned_at.clone(),
            },
        );
        store.set_fuel(123)?;
        store
            .data_mut()
            .ensure_fuel(123)
            .expect("test fuel borrow must succeed");

        drop(StoreFuelGuard::new(store));

        assert_eq!(returned_at.load(Ordering::Acquire), 123);
        Ok(())
    }
}
