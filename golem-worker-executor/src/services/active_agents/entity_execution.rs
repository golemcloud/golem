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

use super::*;

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

impl EntityFence<'_> {
    pub(crate) async fn begin(&self, failure: OwnerFailureWinner) {
        self.operations.select_owner_failure(failure).await;
        self.close();
    }

    pub(crate) fn close(&self) {
        self.operations.close_failed_attachments();
        {
            let entities = self.entities.lock().unwrap();
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.accepting.store(false, Ordering::Release);
            for slot in entities.values() {
                slot.fence();
            }
        }
        self.resources.fence_filesystem_generation();
    }

    pub(crate) async fn drain(&self, owner_id: &OwnedAgentId) {
        let entities = self
            .entities
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        debug!(%owner_id, entity_slot_count = entities.len(), "Draining fenced entity bodies");
        for slot in entities {
            slot.wait_drained().await;
        }
        self.operations.drain_owner_failure_lanes().await;
        debug!(%owner_id, "Fenced entity bodies drained");
    }
}

impl<Ctx: WorkerCtx> ActiveAgent<Ctx> {
    pub(super) fn new_unresolved(
        owner_id: OwnedAgentId,
        primary: Arc<Worker<Ctx>>,
        executor_tasks: InvocationLoops,
    ) -> Self {
        Self {
            owner_id,
            primary,
            executor_tasks,
            entities: Mutex::new(HashMap::new()),
            accepting_entities: AtomicBool::new(true),
            entity_fence_generation: AtomicU64::new(0),
            metrics: OnceLock::new(),
        }
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        &self.owner_id
    }

    pub fn primary(&self) -> Arc<Worker<Ctx>> {
        assert!(self.primary.is_resolved(), "active agent is not resolved");
        self.metrics.get_or_init(OwnerGroupMetricsGuard::new);
        self.primary.clone()
    }

    pub(super) fn resolved_primary(&self) -> Option<Arc<Worker<Ctx>>> {
        self.primary.is_resolved().then(|| self.primary())
    }

    pub fn execution(&self) -> Arc<OwnerExecution> {
        self.primary().owner_execution()
    }

    pub fn resources(&self) -> Arc<OwnerRuntimeResources> {
        self.primary().owner_runtime_resources()
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
            lane: self.execution().lane().metadata(),
            tool_operations: self.execution().tool_operation_metadata(),
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

    pub(crate) async fn begin_fence_entity_bodies(&self, failure: OwnerFailureWinner) {
        let tool_operations = self.execution().tool_operations();
        debug!(
            owner_id = %self.owner_id,
            failure_kind = failure.kind_label(),
            "Selecting owner failure for entity fence"
        );
        let resources = self.resources();
        EntityFence {
            operations: &tool_operations,
            entities: &self.entities,
            accepting: &self.accepting_entities,
            generation: &self.entity_fence_generation,
            resources: &resources,
        }
        .begin(failure)
        .await;
        debug!(owner_id = %self.owner_id, "Entity fence selected and resources closed");
    }

    pub(crate) async fn fence_entity_bodies_for_unload(&self) {
        self.execution().tool_operations().fence_for_unload().await;
        self.close_entity_admission();
        self.drain_fenced_entity_bodies().await;
    }

    fn close_entity_admission(&self) {
        let operations = self.execution().tool_operations();
        let resources = self.resources();
        EntityFence {
            operations: &operations,
            entities: &self.entities,
            accepting: &self.accepting_entities,
            generation: &self.entity_fence_generation,
            resources: &resources,
        }
        .close();
        debug!(owner_id = %self.owner_id, "Entity fence selected and resources closed");
    }

    pub(crate) async fn drain_fenced_entity_bodies(&self) {
        let operations = self.execution().tool_operations();
        let resources = self.resources();
        EntityFence {
            operations: &operations,
            entities: &self.entities,
            accepting: &self.accepting_entities,
            generation: &self.entity_fence_generation,
            resources: &resources,
        }
        .drain(&self.owner_id)
        .await;
    }

    pub(crate) async fn fence_entity_bodies(&self, failure: OwnerFailureWinner) {
        self.begin_fence_entity_bodies(failure).await;
        self.drain_fenced_entity_bodies().await;
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
        if self.execution().tool_operations().has_active_operations()
            || entities
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
        InstanceHost::new_entity(&self.primary(), activation, slot, owner_component_metadata)
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
        Finalize:
            FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
        Finalized:
            Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
            &self.primary(),
            scope.activation(),
            slot.clone(),
            owner_component_metadata,
        )?;
        start_entity_invocation(
            host,
            slot,
            self.execution().lane(),
            self.execution().suspension(),
            self.executor_tasks.clone(),
            parent,
            scope,
            mode,
            invoke,
            finalize,
        )
    }

    pub(crate) fn start_registered_entity_invocation<R, F, Finalize, Finalized>(
        &self,
        scope: EntityInvocationScope,
        owner_component_metadata: Arc<golem_service_base::model::component::Component>,
        mode: EntityCallMode,
        ticket: OwnerInvocationTicket,
        invoke: F,
        finalize: Finalize,
    ) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
    where
        R: Send + 'static,
        F: EntityInvocationBody<Ctx, R>,
        Finalize:
            FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
        Finalized:
            Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
    {
        let slot = self.entity_slot_if_accepting(scope.invocation_id().entity())?;
        let host = InstanceHost::new_entity(
            &self.primary(),
            scope.activation(),
            slot.clone(),
            owner_component_metadata,
        )?;
        start_registered_entity_invocation(
            host,
            slot,
            self.execution().lane(),
            self.execution().suspension(),
            self.executor_tasks.clone(),
            scope,
            mode,
            ticket,
            invoke,
            finalize,
        )
    }

    /// Starts a sidecar after its operation has already registered and acquired the existing owner
    /// lane node. The operation retains that permit until its durable terminal is committed.
    pub(crate) fn start_pre_acquired_entity_invocation<R, F, Finalize, Finalized>(
        &self,
        scope: EntityInvocationScope,
        owner_component_metadata: Arc<golem_service_base::model::component::Component>,
        mode: EntityCallMode,
        invoke: F,
        finalize: Finalize,
    ) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
    where
        R: Send + 'static,
        F: EntityInvocationBody<Ctx, R>,
        Finalize:
            FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
        Finalized:
            Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
            &self.primary(),
            scope.activation(),
            slot.clone(),
            owner_component_metadata,
        )?;
        start_pre_acquired_entity_invocation(
            host,
            slot,
            self.execution().lane(),
            self.execution().suspension(),
            self.executor_tasks.clone(),
            scope,
            mode,
            invoke,
            finalize,
        )
    }

    pub(crate) fn start_native_entity_invocation<R, Run, Finalize, Finalized>(
        &self,
        parent: Option<OwnerInvocationId>,
        scope: EntityInvocationScope,
        mode: EntityCallMode,
        run: Run,
        finalize: Finalize,
    ) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
    where
        R: Send + 'static,
        Run: Send + 'static,
        Run: for<'a> FnOnce(
            EntityInvocationScope,
            &'a crate::worker::entity_slot::EntitySlotRegistration,
            tokio_util::sync::CancellationToken,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = (
                            Result<EntityBodyOutcome<R>, WorkerExecutorError>,
                            Option<Box<dyn RetainedEntityStore>>,
                        ),
                    > + Send
                    + 'a,
            >,
        >,
        Finalize:
            FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
        Finalized:
            Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
        start_native_entity_invocation(
            slot,
            self.execution().lane(),
            self.execution().suspension(),
            self.executor_tasks.clone(),
            parent,
            scope,
            mode,
            run,
            finalize,
        )
    }

    pub(crate) fn start_registered_native_entity_invocation<R, Run, Finalize, Finalized>(
        &self,
        scope: EntityInvocationScope,
        mode: EntityCallMode,
        ticket: OwnerInvocationTicket,
        run: Run,
        finalize: Finalize,
    ) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
    where
        R: Send + 'static,
        Run: Send + 'static,
        Run: for<'a> FnOnce(
            EntityInvocationScope,
            &'a crate::worker::entity_slot::EntitySlotRegistration,
            tokio_util::sync::CancellationToken,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = (
                            Result<EntityBodyOutcome<R>, WorkerExecutorError>,
                            Option<Box<dyn RetainedEntityStore>>,
                        ),
                    > + Send
                    + 'a,
            >,
        >,
        Finalize:
            FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
        Finalized:
            Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
    {
        let slot = self.entity_slot_if_accepting(scope.invocation_id().entity())?;
        start_registered_native_entity_invocation(
            slot,
            self.execution().lane(),
            self.execution().suspension(),
            self.executor_tasks.clone(),
            scope,
            mode,
            ticket,
            run,
            finalize,
        )
    }
}
