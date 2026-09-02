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

//! Durable owner-oplog boundary for transient entity bodies.
//!
//! The tool call surface and middleware-chain dispatcher supply only dispatch behavior. They begin
//! an entity record here, launch the returned invocation scope through `ActiveAgent`, then hand the
//! body handle back to [`EntityInvocationDurability::drive_access`].

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, DurableCallSession, HistoricalReconstruction, LeaveIncompleteOnDrop,
    ReconstructionReplayOutcome, ReplayAccessStartOutcome,
};
use crate::services::oplog::OplogOps;
use crate::worker::entity_invocation::{EntityInvocationHandle, EntityInvocationResources};
use crate::worker::owner_lane::OwnerInvocationId;
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::agent::Principal;
use golem_common::model::entity::{
    AgentEntity, EntityActivation, EntityCallMode, EntityInvocationDescriptor, EntityInvocationId,
    EntityInvocationRequest, EntityInvocationRequestIdentity, EntityInvocationScope,
    InvocationExecutionMode, OwnedAgentEntityId, ToolInvocationClaimIdentity,
};
use golem_common::model::oplog::host_functions::{GolemEntityInvoke, GolemToolInvocationRejected};
use golem_common::model::oplog::payload::types::{
    SerializableEntityBodyExecution, SerializableToolOperationTerminal, SerializableToolRpcError,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestEntityInvocation,
    HostRequestGolemToolInvocationRejected, HostResponseEntityInvocation, OplogEntry, OplogIndex,
};
use golem_common::schema::{IntoTypedSchemaValue, TypedSchemaValue};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, HasData};

pub(crate) enum EntityInvocationDurabilityOutcome {
    Completed(
        Box<HostResponseEntityInvocation>,
        Option<EntityInvocationResources>,
    ),
    Cancelled(
        Box<HostResponseEntityInvocation>,
        Option<EntityInvocationResources>,
    ),
}

pub(crate) struct EntityInvocationDurabilityFailure {
    pub(crate) error: WorkerExecutorError,
    pub(crate) resources: Option<EntityInvocationResources>,
}

impl From<WorkerExecutorError> for EntityInvocationDurabilityFailure {
    fn from(error: WorkerExecutorError) -> Self {
        Self {
            error,
            resources: None,
        }
    }
}

pub enum ToolInvocationReplayOutcome {
    Accepted(Box<EntityInvocationDurability>),
    Rejected(Box<HostResponseEntityInvocation>),
    ReplayEnded,
}

pub enum RecordedEntityTerminal {
    Completed(HostResponseEntityInvocation),
    Cancelled(HostResponseEntityInvocation),
}

pub(crate) enum IncompleteLiveRepairBeforeBody {
    Ready(EntityInvocationDurability),
    Cancelled(EntityInvocationDurability),
}

enum EntityReconstructionResolution<R, H> {
    Replayed(R),
    Cancelled(R),
    Incomplete(H),
    LiveAdmissionCancelled(H),
}

#[derive(Debug)]
enum EntityReconstructionOutcome<R, H> {
    Replayed(R),
    Cancelled(R),
    Incomplete { response: R, handle: H },
    IncompleteCancelled { handle: H },
    IncompleteLiveAdmissionCancelled { handle: H },
}

/// Task-owned durable state for one entity body. Its `Start` index is the entity invocation ID.
/// Dropping before terminal selection leaves the Start incomplete for recovery; only an explicit
/// cancellation appends a cancellation terminal.
pub struct EntityInvocationDurability {
    handle: DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>,
    scope: EntityInvocationScope,
    principal: Principal,
    parent: OwnerInvocationId,
    call_mode: EntityCallMode,
    operation: Option<EntityInvocationDescriptor>,
    input: TypedSchemaValue,
    historical_reconstruction: Option<HistoricalReconstruction>,
}

impl EntityInvocationDurability {
    pub async fn start_live_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        entity: AgentEntity,
        activation: Arc<EntityActivation>,
        calling_principal: Principal,
        principal: Principal,
        call_mode: EntityCallMode,
        operation: Option<EntityInvocationDescriptor>,
        input: TypedSchemaValue,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        if !store.with(|mut access| get_ctx(access.data_mut()).state.is_live()) {
            return Err(WorkerExecutorError::runtime(
                "live entity invocation Start requested during historical replay",
            ));
        }
        let parent_start_index = parent.start_index();
        let metadata = EntityInvocationRequest {
            entity: entity.clone(),
            activation: activation.as_ref().clone(),
            calling_principal: calling_principal.clone(),
            call_mode,
            operation,
            principal: Some(principal),
        };
        let encoded_metadata = desert_rust::serialize_to_byte_vec(&metadata).map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "failed to encode entity invocation metadata: {error}"
            ))
        })?;
        let request = HostRequestEntityInvocation {
            metadata: encoded_metadata,
            input,
        };
        let started_input = request.input.clone();
        let handle =
            DurableCallSession::<GolemEntityInvoke, LeaveIncompleteOnDrop>::start_access_with_options(
                store,
                get_ctx,
                DurableFunctionType::WriteLocal,
                AccessClaimOptions {
                    entity_invocation_identity: Some(entity_request_identity(
                        &metadata,
                        &request.input,
                    )),
                    parent_start_index: Some(parent_start_index),
                    ..AccessClaimOptions::default()
                },
                async move |_| Ok(request),
            )
            .await?;
        Self::from_started_request(store, get_ctx, parent, handle, metadata, started_input).await
    }

    pub async fn replay_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        identity: EntityInvocationRequestIdentity,
    ) -> Result<Option<Self>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let parent_start_index = parent.start_index();
        let handle = match DurableCallSession::<GolemEntityInvoke, LeaveIncompleteOnDrop>::claim_replay_access_with_options(
                store,
                get_ctx,
                DurableFunctionType::WriteLocal,
                AccessClaimOptions {
                    entity_invocation_identity: Some(identity),
                    parent_start_index: Some(parent_start_index),
                    ..AccessClaimOptions::default()
                },
            )
            .await?
        {
            ReplayAccessStartOutcome::Claimed(handle) => handle,
            ReplayAccessStartOutcome::ReplayEnded => return Ok(None),
        };
        if let Some(hook) =
            store.with(|mut access| get_ctx(access.data_mut()).entity_reconstruction_claim_hook())
        {
            hook.after_claim(handle.start_index()).await;
        }
        let request = load_recorded_request(store, get_ctx, handle.start_index()).await?;
        let request: HostRequestEntityInvocation = request.try_into().map_err(|actual| {
            WorkerExecutorError::unexpected_oplog_entry("entity invocation request", actual)
        })?;
        let metadata = desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
            .map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to decode recorded entity invocation metadata: {error}"
                ))
            })?;
        Self::from_started_request(store, get_ctx, parent, handle, metadata, request.input)
            .await
            .map(Some)
    }

    pub async fn replay_tool_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        identity: ToolInvocationClaimIdentity,
    ) -> Result<ToolInvocationReplayOutcome, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let parent_start_index = parent.start_index();
        let mut handle = match DurableCallSession::<GolemEntityInvoke, LeaveIncompleteOnDrop>::claim_replay_access_with_options(
                store,
                get_ctx,
                DurableFunctionType::WriteLocal,
                AccessClaimOptions {
                    tool_invocation_identity: Some((
                        GolemToolInvocationRejected::HOST_FUNCTION_NAME,
                        identity,
                    )),
                    parent_start_index: Some(parent_start_index),
                    ..AccessClaimOptions::default()
                },
            )
            .await?
        {
            ReplayAccessStartOutcome::Claimed(handle) => handle,
            ReplayAccessStartOutcome::ReplayEnded => {
                return Ok(ToolInvocationReplayOutcome::ReplayEnded);
            }
        };
        if let Some(hook) =
            store.with(|mut access| get_ctx(access.data_mut()).entity_reconstruction_claim_hook())
        {
            hook.after_claim(handle.start_index()).await;
        }
        match load_recorded_request(store, get_ctx, handle.start_index()).await? {
            HostRequest::EntityInvocation(request) => {
                let metadata =
                    desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
                        .map_err(|error| {
                            WorkerExecutorError::runtime(format!(
                                "failed to decode recorded entity invocation metadata: {error}"
                            ))
                        })?;
                Ok(ToolInvocationReplayOutcome::Accepted(Box::new(
                    Self::from_started_request(
                        store,
                        get_ctx,
                        parent,
                        handle,
                        metadata,
                        request.input,
                    )
                    .await?,
                )))
            }
            HostRequest::GolemToolInvocationRejected(request) => {
                let mut historical_reconstruction = handle
                    .take_historical_reconstruction()
                    .expect("replayed tool rejection claim must own a reconstruction claim");
                historical_reconstruction.body_settled();
                let expected = skipped_tool_terminal(request.error).await?;
                let response = match handle.replay_reconstruction_access(store, get_ctx).await? {
                    ReconstructionReplayOutcome::Replayed(recorded) => {
                        validate_recorded_rejection_terminal(recorded, &expected)?
                    }
                    ReconstructionReplayOutcome::Cancelled(_) => {
                        return Err(WorkerExecutorError::unexpected_oplog_entry(
                            "completed predispatch rejection terminal",
                            "cancelled rejection terminal",
                        ));
                    }
                    ReconstructionReplayOutcome::Incomplete(live) => live
                        .complete_access(store, get_ctx, expected)
                        .await
                        .map_err(|error| error.source)?,
                    ReconstructionReplayOutcome::LiveAdmissionCancelled(mut live) => {
                        live.abandon_for_trap();
                        return Err(
                            crate::durable_host::tool_attachment_live_admission_cancelled_error(),
                        );
                    }
                };
                Ok(ToolInvocationReplayOutcome::Rejected(Box::new(response)))
            }
            actual => Err(WorkerExecutorError::unexpected_oplog_entry(
                "accepted entity invocation or predispatch tool rejection request",
                format!("{actual:?}"),
            )),
        }
    }

    async fn from_started_request<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        mut handle: DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>,
        metadata: EntityInvocationRequest,
        input: TypedSchemaValue,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let parent_start_index = parent.start_index();
        let owner =
            store.with(|mut access| get_ctx(access.data_mut()).state.owned_agent_id.clone());
        let operation = metadata.operation;
        let historical_reconstruction = if handle.is_live() {
            None
        } else {
            Some(
                handle
                    .take_historical_reconstruction()
                    .expect("replayed entity invocation must own a reconstruction claim"),
            )
        };
        let principal = metadata
            .principal
            .unwrap_or_else(|| metadata.calling_principal.clone());
        let invocation_id = EntityInvocationId::new(
            OwnedAgentEntityId {
                owner,
                entity: metadata.entity,
            },
            handle.start_index(),
        )
        .map_err(WorkerExecutorError::runtime)?;
        let execution_mode = if handle.is_live() {
            InvocationExecutionMode::Live
        } else {
            let replay =
                store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
            if replay.has_visible_terminal(handle.start_index()).await {
                InvocationExecutionMode::ReplayingCompleted
            } else {
                InvocationExecutionMode::ReplayingIncomplete
            }
        };
        let scope = EntityInvocationScope::new(
            invocation_id,
            parent_start_index,
            Arc::new(metadata.activation),
            metadata.calling_principal,
            execution_mode,
        )
        .map_err(WorkerExecutorError::runtime)?;
        Ok(Self {
            handle,
            scope,
            principal,
            parent,
            call_mode: metadata.call_mode,
            operation,
            input,
            historical_reconstruction,
        })
    }

    pub fn scope(&self) -> &EntityInvocationScope {
        &self.scope
    }

    pub(crate) fn historical_reconstruction_hold(&self) -> Option<HistoricalReconstruction> {
        self.historical_reconstruction.clone()
    }

    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    pub fn operation(&self) -> Option<&EntityInvocationDescriptor> {
        self.operation.as_ref()
    }

    pub fn input(&self) -> &TypedSchemaValue {
        &self.input
    }

    pub fn call_mode(&self) -> EntityCallMode {
        self.call_mode
    }

    /// Converts a replayed incomplete Start into its live-repair handle before a body exists.
    /// Filesystem-capable tools can remain in input staging while the primary owner replays later
    /// sibling calls, so retaining their historical reconstruction fence until body dispatch would
    /// deadlock the primary's transition to the live tail.
    pub(crate) async fn enter_incomplete_live_repair_before_body_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<IncompleteLiveRepairBeforeBody, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        if self.scope.mode() != InvocationExecutionMode::ReplayingIncomplete {
            return Ok(IncompleteLiveRepairBeforeBody::Ready(self));
        }
        let replay = store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
        if replay
            .has_visible_scope_descendant(self.handle.start_index())
            .await
        {
            return Ok(IncompleteLiveRepairBeforeBody::Ready(self));
        }

        let Self {
            handle,
            scope,
            principal,
            parent,
            call_mode,
            operation,
            input,
            mut historical_reconstruction,
        } = self;
        let (handle, cancelled) = match handle.replay_reconstruction_access(store, get_ctx).await? {
            ReconstructionReplayOutcome::Incomplete(handle) => (handle, false),
            ReconstructionReplayOutcome::LiveAdmissionCancelled(handle) => (handle, true),
            ReconstructionReplayOutcome::Replayed(_) => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "incomplete entity invocation Start",
                    "completed entity invocation terminal",
                ));
            }
            ReconstructionReplayOutcome::Cancelled(_) => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "incomplete entity invocation Start",
                    "cancelled entity invocation terminal",
                ));
            }
        };
        if let Some(reconstruction) = historical_reconstruction.as_mut() {
            reconstruction.body_settled();
        }
        drop(historical_reconstruction.take());
        let scope = EntityInvocationScope::new(
            scope.invocation_id().clone(),
            scope.parent_start_index(),
            scope.activation().clone(),
            scope.calling_principal().clone(),
            InvocationExecutionMode::Live,
        )
        .map_err(WorkerExecutorError::runtime)?;

        let durability = Self {
            handle,
            scope,
            principal,
            parent,
            call_mode,
            operation,
            input,
            historical_reconstruction: None,
        };
        Ok(if cancelled {
            IncompleteLiveRepairBeforeBody::Cancelled(durability)
        } else {
            IncompleteLiveRepairBeforeBody::Ready(durability)
        })
    }

    pub fn parent(&self) -> &OwnerInvocationId {
        &self.parent
    }

    /// Reads a completed replay's authoritative response without consuming its cursor terminal.
    /// Callers use this only to decide whether a recorded skipped-body operation may bypass Store
    /// construction; terminal consumption and equality validation still happen through the call
    /// handle's normal reconstruction path.
    pub async fn recorded_terminal_access<T, D, Ctx>(
        &self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<Option<RecordedEntityTerminal>, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        if self.scope.mode() != InvocationExecutionMode::ReplayingCompleted {
            return Ok(None);
        }
        let (replay, oplog) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (ctx.state.replay_state.clone(), ctx.state.oplog.clone())
        });
        let entry = replay
            .visible_terminal_entry(self.handle.start_index())
            .await
            .ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    format!(
                        "recorded terminal for completed entity invocation {}",
                        self.scope.invocation_id()
                    ),
                    "no replay-visible terminal",
                )
            })?;
        let (payload, cancelled) = match entry {
            OplogEntry::End {
                response: Some(response),
                ..
            } => (response, false),
            OplogEntry::Cancelled {
                partial: Some(response),
                ..
            } => (response, true),
            actual => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "entity End response or Cancelled partial response",
                    format!("{actual:?}"),
                ));
            }
        };
        let response = oplog.download_payload(payload).await.map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "recorded entity terminal cannot be downloaded: {error}"
            ))
        })?;
        let response: HostResponseEntityInvocation = response.try_into().map_err(|actual| {
            WorkerExecutorError::unexpected_oplog_entry(
                "entity invocation terminal response",
                actual,
            )
        })?;
        Ok(Some(if cancelled {
            RecordedEntityTerminal::Cancelled(response)
        } else {
            RecordedEntityTerminal::Completed(response)
        }))
    }

    /// Completes an eager entity `Start` without constructing a sidecar body. This is used only
    /// after deterministic admission has selected an ordinary terminal (for example attachment
    /// resource exhaustion). Replay consumes the recorded terminal directly.
    pub(crate) async fn complete_without_body_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        response: HostResponseEntityInvocation,
    ) -> Result<EntityInvocationDurabilityOutcome, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let Self {
            handle,
            mut historical_reconstruction,
            ..
        } = self;
        if let Some(reconstruction) = historical_reconstruction.as_mut() {
            reconstruction.body_settled();
        }
        let response = if handle.is_live() {
            handle
                .complete_access(store, get_ctx, response)
                .await
                .map_err(|error| error.source)?
        } else {
            match handle.replay_reconstruction_access(store, get_ctx).await? {
                ReconstructionReplayOutcome::Replayed(recorded) if recorded == response => recorded,
                ReconstructionReplayOutcome::Replayed(recorded) => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "recorded no-body entity terminal",
                        format!("preflight selected {response:?}, replay returned {recorded:?}"),
                    ));
                }
                ReconstructionReplayOutcome::Cancelled(recorded) => {
                    return Ok(EntityInvocationDurabilityOutcome::Cancelled(
                        Box::new(recorded),
                        None,
                    ));
                }
                ReconstructionReplayOutcome::Incomplete(live) => live
                    .complete_access(store, get_ctx, response)
                    .await
                    .map_err(|error| error.source)?,
                ReconstructionReplayOutcome::LiveAdmissionCancelled(mut live) => {
                    live.abandon_for_trap();
                    return Err(
                        crate::durable_host::tool_attachment_live_admission_cancelled_error(),
                    );
                }
            }
        };
        Ok(EntityInvocationDurabilityOutcome::Completed(
            Box::new(response),
            None,
        ))
    }

    /// Selects the generic durable cancellation terminal before a sidecar body exists. A replayed
    /// call must resolve to the same recorded `Cancelled` entry.
    pub(crate) async fn cancel_without_body_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Result<EntityInvocationDurabilityOutcome, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let Self {
            handle,
            mut historical_reconstruction,
            ..
        } = self;
        if let Some(reconstruction) = historical_reconstruction.as_mut() {
            reconstruction.body_settled();
        }
        let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Skipped).await?;
        let response = if handle.is_live() {
            handle
                .cancel_access(store, get_ctx, Some(response.clone()))
                .await
                .map_err(|error| error.source)?;
            response
        } else {
            match handle.replay_reconstruction_access(store, get_ctx).await? {
                ReconstructionReplayOutcome::Cancelled(recorded) => {
                    validate_recorded_cancellation_terminal(recorded, &response)?
                }
                ReconstructionReplayOutcome::Replayed(_) => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "cancelled entity invocation terminal",
                        "completed entity invocation terminal",
                    ));
                }
                ReconstructionReplayOutcome::Incomplete(live)
                | ReconstructionReplayOutcome::LiveAdmissionCancelled(live) => {
                    live.cancel_access(store, get_ctx, Some(response.clone()))
                        .await
                        .map_err(|error| error.source)?;
                    response
                }
            }
        };
        Ok(EntityInvocationDurabilityOutcome::Cancelled(
            Box::new(response),
            None,
        ))
    }

    /// Drives body reconstruction and the outer durable terminal together. A completed replay does
    /// not release its recorded response until the fresh body has reconstructed local effects; a
    /// live cancellation cooperatively unwinds the body while retaining its Store for child
    /// settlement; and an incomplete Start switches to live and is completed under the original
    /// Start index.
    pub(crate) async fn drive_access<
        T,
        D,
        Ctx,
        OnCompletedStarted,
        OnCompletedFailure,
        CompletedFailureFuture,
    >(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        body: EntityInvocationHandle<HostResponseEntityInvocation>,
        cancellation: Option<tokio_util::sync::CancellationToken>,
        on_completed_started: OnCompletedStarted,
        on_completed_failure: OnCompletedFailure,
    ) -> Result<EntityInvocationDurabilityOutcome, EntityInvocationDurabilityFailure>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
        OnCompletedStarted: FnOnce() + Send,
        OnCompletedFailure: FnOnce(WorkerExecutorError) -> CompletedFailureFuture + Send + 'static,
        CompletedFailureFuture: Future<Output = ()> + Send + 'static,
    {
        let Self {
            mut handle,
            scope,
            parent,
            call_mode,
            mut historical_reconstruction,
            ..
        } = self;
        let invocation = scope.invocation_id().clone();
        let abort = body.abort_handle();
        let body_resources = Arc::new(Mutex::new(None));
        let completed_body_resources = body_resources.clone();
        let body = async move {
            let completion = match call_mode {
                EntityCallMode::FireAndForget => body.join_completion().await,
                EntityCallMode::Synchronous | EntityCallMode::Asynchronous => {
                    body.await_completion(&parent).await
                }
            }?;
            let (result, resources) = completion.into_parts();
            *completed_body_resources.lock().unwrap() = Some(resources);
            result
        };

        if scope.mode() == InvocationExecutionMode::ReplayingCompleted {
            let (replay_state, oplog, completion_marker_recorder) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (
                    ctx.state.replay_state.clone(),
                    ctx.state.oplog.clone(),
                    ctx.state.completion_marker_recorder(),
                )
            });
            let active_reconstruction_bodies = replay_state.historical_reconstruction_bodies();
            let terminal = Arc::new(Mutex::new(None));
            let replay_terminal = terminal.clone();
            let structural_replay_state = replay_state.clone();
            let structural_start = invocation.start_index();
            let unconsumed_scope = async move {
                structural_replay_state
                    .await_unconsumed_scope_entry(structural_start, active_reconstruction_bodies)
                    .await
            };
            let replay = async move {
                let (response, terminal) = handle
                    .resolve_completed_reconstruction(
                        replay_state.clone(),
                        oplog,
                        completion_marker_recorder,
                    )
                    .await?;
                let cancelled = terminal.cancelled();
                *replay_terminal.lock().unwrap() = Some(terminal);
                Ok(if cancelled {
                    EntityReconstructionResolution::<
                        _,
                        DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>,
                    >::Cancelled(response)
                } else {
                    EntityReconstructionResolution::Replayed(response)
                })
            };
            let supervisor_body_resources = body_resources.clone();
            let monitor_reconstruction = historical_reconstruction.clone();
            let completed_supervisor = tokio::spawn(async move {
                let mut historical_reconstruction = historical_reconstruction;
                let reconstruction = std::panic::AssertUnwindSafe(async {
                    let reconstruction = coordinate_entity_reconstruction_inner(
                        &invocation,
                        InvocationExecutionMode::ReplayingCompleted,
                        body,
                        replay,
                        unconsumed_scope,
                        || abort.abort(),
                        &mut historical_reconstruction,
                        cancellation.as_ref(),
                    )
                    .await?;
                    let terminal = terminal.lock().unwrap().take().ok_or_else(|| {
                        WorkerExecutorError::runtime(
                            "resolved reconstruction did not retain its accessor terminal",
                        )
                    })?;
                    Ok::<_, WorkerExecutorError>((reconstruction, terminal))
                })
                .catch_unwind()
                .await;
                match reconstruction {
                    Ok(reconstruction) => {
                        reconstruction.map_err(|error| EntityInvocationDurabilityFailure {
                            error,
                            resources: take_entity_resources(&supervisor_body_resources),
                        })
                    }
                    Err(_) => Err(EntityInvocationDurabilityFailure {
                        error: WorkerExecutorError::runtime(
                            "completed entity reconstruction coordinator panicked",
                        ),
                        resources: take_entity_resources(&supervisor_body_resources),
                    }),
                }
            });
            let (completed_tx, completed_rx) = oneshot::channel();
            let monitor_body_resources = body_resources.clone();
            tokio::spawn(async move {
                let completed = match completed_supervisor.await {
                    Ok(completed) => completed,
                    Err(error) => Err(EntityInvocationDurabilityFailure {
                        error: WorkerExecutorError::runtime(format!(
                            "completed entity reconstruction task failed: {error}"
                        )),
                        resources: take_entity_resources(&monitor_body_resources),
                    }),
                };
                let retained_reconstruction = if let Err(failure) = &completed {
                    on_completed_failure(failure.error.clone()).await;
                    drop(monitor_reconstruction);
                    None
                } else {
                    Some(monitor_reconstruction)
                };
                let _ = completed_tx.send((completed, retained_reconstruction));
            });
            on_completed_started();
            let (completed, mut retained_reconstruction) =
                completed_rx
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error: WorkerExecutorError::runtime(format!(
                            "completed entity reconstruction monitor failed: {error}"
                        )),
                        resources: take_entity_resources(&body_resources),
                    })?;
            let (reconstruction, terminal) = completed?;
            if !terminal.is_replay_at_marker() {
                drop(retained_reconstruction.take());
            }
            terminal
                .finish_access(store, get_ctx)
                .await
                .map_err(|error| EntityInvocationDurabilityFailure {
                    error,
                    resources: take_entity_resources(&body_resources),
                })?;
            drop(retained_reconstruction);
            return Ok(match reconstruction {
                EntityReconstructionOutcome::Replayed(recorded) => {
                    EntityInvocationDurabilityOutcome::Completed(
                        Box::new(recorded),
                        take_entity_resources(&body_resources),
                    )
                }
                EntityReconstructionOutcome::Cancelled(recorded) => {
                    EntityInvocationDurabilityOutcome::Cancelled(
                        Box::new(recorded),
                        take_entity_resources(&body_resources),
                    )
                }
                EntityReconstructionOutcome::Incomplete { .. }
                | EntityReconstructionOutcome::IncompleteCancelled { .. }
                | EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled { .. } => {
                    unreachable!("completed reconstruction cannot resolve incomplete")
                }
            });
        }

        tokio::pin!(body);
        if handle.is_live() {
            let body_result = match cancellation {
                Some(cancellation) => {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => None,
                        response = &mut body => Some(response),
                    }
                }
                None => Some(body.as_mut().await),
            };
            let Some(body_result) = body_result else {
                let _ = body.as_mut().await;
                let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Executed)
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error,
                        resources: take_entity_resources(&body_resources),
                    })?;
                handle
                    .cancel_access(store, get_ctx, Some(response.clone()))
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error: error.source,
                        resources: take_entity_resources(&body_resources),
                    })?;
                return Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&body_resources),
                ));
            };
            let response = match body_result {
                Ok(response) => response,
                Err(error) => {
                    let _ = handle.trap(error.clone());
                    return Err(EntityInvocationDurabilityFailure {
                        error,
                        resources: take_entity_resources(&body_resources),
                    });
                }
            };
            let response = handle
                .complete_access(store, get_ctx, response)
                .await
                .map_err(|error| EntityInvocationDurabilityFailure {
                    error: error.source,
                    resources: take_entity_resources(&body_resources),
                })?;
            return Ok(EntityInvocationDurabilityOutcome::Completed(
                Box::new(response),
                take_entity_resources(&body_resources),
            ));
        }

        let replay = async {
            Ok(
                match handle.replay_reconstruction_access(store, get_ctx).await? {
                    ReconstructionReplayOutcome::Replayed(response) => {
                        EntityReconstructionResolution::Replayed(response)
                    }
                    ReconstructionReplayOutcome::Cancelled(recorded) => {
                        EntityReconstructionResolution::Cancelled(recorded)
                    }
                    ReconstructionReplayOutcome::Incomplete(handle) => {
                        EntityReconstructionResolution::Incomplete(handle)
                    }
                    ReconstructionReplayOutcome::LiveAdmissionCancelled(handle) => {
                        EntityReconstructionResolution::LiveAdmissionCancelled(handle)
                    }
                },
            )
        };
        let (replay_state, active_reconstruction_bodies) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.state.replay_state.historical_reconstruction_bodies(),
            )
        });
        let unconsumed_scope = replay_state
            .await_unconsumed_scope_entry(invocation.start_index(), active_reconstruction_bodies);
        let reconstruction = coordinate_entity_reconstruction(
            &invocation,
            scope.mode(),
            body,
            replay,
            unconsumed_scope,
            || abort.abort(),
            historical_reconstruction.take(),
            cancellation.as_ref(),
        )
        .await;
        let reconstruction = match reconstruction {
            Ok(reconstruction) => reconstruction,
            Err(error) => {
                return Err(EntityInvocationDurabilityFailure {
                    error,
                    resources: take_entity_resources(&body_resources),
                });
            }
        };
        match reconstruction {
            EntityReconstructionOutcome::Cancelled(recorded) => {
                Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(recorded),
                    take_entity_resources(&body_resources),
                ))
            }
            EntityReconstructionOutcome::Replayed(recorded) => {
                Ok(EntityInvocationDurabilityOutcome::Completed(
                    Box::new(recorded),
                    take_entity_resources(&body_resources),
                ))
            }
            EntityReconstructionOutcome::Incomplete {
                response,
                handle: live_handle,
            } => {
                let response = live_handle
                    .complete_access(store, get_ctx, response)
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error: error.source,
                        resources: take_entity_resources(&body_resources),
                    })?;
                Ok(EntityInvocationDurabilityOutcome::Completed(
                    Box::new(response),
                    take_entity_resources(&body_resources),
                ))
            }
            EntityReconstructionOutcome::IncompleteCancelled {
                handle: live_handle,
            } => {
                let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Executed)
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error,
                        resources: take_entity_resources(&body_resources),
                    })?;
                live_handle
                    .cancel_access(store, get_ctx, Some(response.clone()))
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error: error.source,
                        resources: take_entity_resources(&body_resources),
                    })?;
                Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&body_resources),
                ))
            }
            EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled {
                handle: live_handle,
            } => {
                let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Skipped)
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error,
                        resources: take_entity_resources(&body_resources),
                    })?;
                live_handle
                    .cancel_access(store, get_ctx, Some(response.clone()))
                    .await
                    .map_err(|error| EntityInvocationDurabilityFailure {
                        error: error.source,
                        resources: take_entity_resources(&body_resources),
                    })?;
                Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&body_resources),
                ))
            }
        }
    }
}

fn take_entity_resources(
    resources: &Arc<Mutex<Option<EntityInvocationResources>>>,
) -> Option<EntityInvocationResources> {
    resources.lock().unwrap().take()
}

pub async fn record_tool_rejection_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    parent: OwnerInvocationId,
    request: HostRequestGolemToolInvocationRejected,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let response = skipped_tool_terminal(request.error.clone()).await?;
    let handle = DurableCallSession::<GolemToolInvocationRejected, LeaveIncompleteOnDrop>::start_access_with_options(
        store,
        get_ctx,
        DurableFunctionType::WriteLocal,
        AccessClaimOptions {
            parent_start_index: Some(parent.start_index()),
            ..AccessClaimOptions::default()
        },
        async move |_| Ok(request),
    )
    .await?;
    handle
        .complete_access(store, get_ctx, response)
        .await
        .map_err(|error| error.source)
}

pub(crate) async fn encode_tool_terminal(
    terminal: SerializableToolOperationTerminal,
    context: &'static str,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    let result = tokio::task::spawn_blocking(move || terminal.into_typed_schema_value())
        .await
        .map_err(|error| WorkerExecutorError::runtime(format!("{context} task failed: {error}")))?
        .map_err(|error| WorkerExecutorError::runtime(format!("{context}: {error}")))?;
    Ok(HostResponseEntityInvocation { result: Ok(result) })
}

async fn skipped_tool_terminal(
    error: SerializableToolRpcError,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        SerializableToolOperationTerminal {
            body_execution: SerializableEntityBodyExecution::Skipped,
            result: Err(error),
        },
        "failed to encode skipped tool terminal",
    )
    .await
}

async fn cancelled_tool_terminal(
    body_execution: SerializableEntityBodyExecution,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        SerializableToolOperationTerminal {
            body_execution,
            result: Err(SerializableToolRpcError::Cancelled),
        },
        "failed to encode entity cancellation terminal",
    )
    .await
}

fn validate_recorded_rejection_terminal(
    recorded: HostResponseEntityInvocation,
    expected: &HostResponseEntityInvocation,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    if &recorded == expected {
        Ok(recorded)
    } else {
        Err(WorkerExecutorError::unexpected_oplog_entry(
            "predispatch rejection terminal matching its recorded Start decision",
            "completed rejection terminal carried a different response",
        ))
    }
}

fn validate_recorded_cancellation_terminal(
    recorded: HostResponseEntityInvocation,
    expected: &HostResponseEntityInvocation,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    if &recorded == expected {
        Ok(recorded)
    } else {
        Err(WorkerExecutorError::unexpected_oplog_entry(
            "recorded skipped-body tool cancellation terminal",
            "cancelled entity terminal carried a different partial response",
        ))
    }
}

fn entity_request_identity(
    request: &EntityInvocationRequest,
    input: &TypedSchemaValue,
) -> EntityInvocationRequestIdentity {
    EntityInvocationRequestIdentity {
        entity: request.entity.clone(),
        calling_principal: request.calling_principal.clone(),
        call_mode: request.call_mode,
        operation: request.operation.as_ref().map(Into::into),
        input: input.clone(),
    }
}

async fn load_recorded_request<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
) -> Result<HostRequest, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let oplog = store.with(|mut access| get_ctx(access.data_mut()).state.oplog.clone());
    let entry = oplog.read(start_index).await;
    let OplogEntry::Start {
        request: Some(request),
        ..
    } = entry
    else {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            format!("entity invocation Start with request at {start_index}"),
            format!("{entry:?}"),
        ));
    };
    oplog.download_payload(request).await.map_err(|error| {
        WorkerExecutorError::runtime(format!(
            "failed to load entity invocation request at {start_index}: {error}"
        ))
    })
}

trait ReconstructionGuard {
    fn body_settled(&mut self);
}

impl ReconstructionGuard for () {
    fn body_settled(&mut self) {}
}

impl ReconstructionGuard for HistoricalReconstruction {
    fn body_settled(&mut self) {
        HistoricalReconstruction::body_settled(self);
    }
}

async fn coordinate_entity_reconstruction<R, H, B, F, S, A, G>(
    invocation: &EntityInvocationId,
    execution_mode: InvocationExecutionMode,
    body: B,
    replay: F,
    structural_stall: S,
    abort: A,
    mut historical_reconstruction: Option<G>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<EntityReconstructionOutcome<R, H>, WorkerExecutorError>
where
    R: PartialEq,
    B: Future<Output = Result<R, WorkerExecutorError>>,
    F: Future<Output = Result<EntityReconstructionResolution<R, H>, WorkerExecutorError>>,
    S: Future<Output = Result<OplogIndex, WorkerExecutorError>>,
    A: FnOnce(),
    G: ReconstructionGuard,
{
    coordinate_entity_reconstruction_inner(
        invocation,
        execution_mode,
        body,
        replay,
        structural_stall,
        abort,
        &mut historical_reconstruction,
        cancellation,
    )
    .await
}

async fn coordinate_entity_reconstruction_inner<R, H, B, F, S, A, G>(
    invocation: &EntityInvocationId,
    execution_mode: InvocationExecutionMode,
    body: B,
    replay: F,
    structural_stall: S,
    abort: A,
    historical_reconstruction: &mut Option<G>,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<EntityReconstructionOutcome<R, H>, WorkerExecutorError>
where
    R: PartialEq,
    B: Future<Output = Result<R, WorkerExecutorError>>,
    F: Future<Output = Result<EntityReconstructionResolution<R, H>, WorkerExecutorError>>,
    S: Future<Output = Result<OplogIndex, WorkerExecutorError>>,
    A: FnOnce(),
    G: ReconstructionGuard,
{
    tokio::pin!(body);
    tokio::pin!(replay);
    tokio::pin!(structural_stall);

    enum First<B, R> {
        Body(B),
        Replay(R),
    }
    let first = tokio::select! {
        body_result = &mut body => First::Body(body_result),
        replay_result = &mut replay => First::Replay(replay_result),
    };
    let (body_result, replay_result) = match first {
        First::Replay(replay_result) => (None, replay_result),
        First::Body(body_result) => {
            if let Some(reconstruction) = historical_reconstruction.as_mut() {
                reconstruction.body_settled();
            }
            match body_result {
                Err(error) => {
                    let cancellation_interruption = execution_mode
                        == InvocationExecutionMode::ReplayingIncomplete
                        && cancellation.is_some_and(|token| token.is_cancelled());
                    match replay.as_mut().now_or_never() {
                        Some(replay_result) => (Some(Err(error)), replay_result),
                        None if cancellation_interruption => {
                            let replay_result = tokio::select! {
                                biased;
                                replay_result = &mut replay => replay_result,
                                stalled_at = &mut structural_stall => {
                                    let stalled_at = stalled_at?;
                                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                                        format!("cancelled replay body for {invocation}"),
                                        format!("entity body was cancelled before consuming its recorded descendant at {stalled_at}"),
                                    ));
                                }
                            };
                            (Some(Err(error)), replay_result)
                        }
                        None => {
                            return Err(match execution_mode {
                                InvocationExecutionMode::ReplayingCompleted => {
                                    replay_body_failure(invocation, error)
                                }
                                InvocationExecutionMode::ReplayingIncomplete
                                | InvocationExecutionMode::Live => error,
                            });
                        }
                    }
                }
                Ok(response) => {
                    let replay_result = tokio::select! {
                        biased;
                        replay_result = &mut replay => replay_result,
                        stalled_at = &mut structural_stall => {
                            let stalled_at = stalled_at?;
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                format!("completed replay body for {invocation}"),
                                format!("entity body returned before consuming its recorded descendant at {stalled_at}"),
                            ));
                        }
                    };
                    (Some(Ok(response)), replay_result)
                }
            }
        }
    };
    let mut abort = Some(abort);
    let replay_result = match replay_result {
        Ok(result) => result,
        Err(error) => {
            abort.take().unwrap()();
            if body_result.is_none() {
                let _ = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
            }
            return Err(error);
        }
    };

    match replay_result {
        EntityReconstructionResolution::Cancelled(recorded) => {
            abort.take().unwrap()();
            if body_result.is_none() {
                let _ = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
            }
            Ok(EntityReconstructionOutcome::Cancelled(recorded))
        }
        EntityReconstructionResolution::Replayed(recorded) => {
            let reconstructed = match body_result {
                Some(response) => {
                    response.map_err(|error| replay_body_failure(invocation, error))?
                }
                None => {
                    let response = body.await;
                    if let Some(reconstruction) = historical_reconstruction.as_mut() {
                        reconstruction.body_settled();
                    }
                    response.map_err(|error| replay_body_failure(invocation, error))?
                }
            };
            if reconstructed != recorded {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!("recorded entity invocation result for {invocation}"),
                    format!("reconstructed a different result for {invocation}"),
                ));
            }
            Ok(EntityReconstructionOutcome::Replayed(recorded))
        }
        EntityReconstructionResolution::LiveAdmissionCancelled(handle) => {
            abort.take().unwrap()();
            if body_result.is_none() {
                let _ = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
            }
            Ok(EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled { handle })
        }
        EntityReconstructionResolution::Incomplete(handle) => {
            // The resolver already released the historical fence when replay reached the live
            // tail. Keep body membership until the body actually settles so structural-stall
            // detection can still identify its owned descendants during live repair.
            if cancellation.is_some_and(|token| token.is_cancelled()) {
                if body_result.is_none() {
                    let _ = body.await;
                    if let Some(reconstruction) = historical_reconstruction.as_mut() {
                        reconstruction.body_settled();
                    }
                }
                return Ok(EntityReconstructionOutcome::IncompleteCancelled { handle });
            }
            let response = match body_result {
                Some(response) => response?,
                None => match body.await {
                    Ok(response) => {
                        if let Some(reconstruction) = historical_reconstruction.as_mut() {
                            reconstruction.body_settled();
                        }
                        response
                    }
                    Err(_) if cancellation.is_some_and(|token| token.is_cancelled()) => {
                        if let Some(reconstruction) = historical_reconstruction.as_mut() {
                            reconstruction.body_settled();
                        }
                        return Ok(EntityReconstructionOutcome::IncompleteCancelled { handle });
                    }
                    Err(error) => {
                        if let Some(reconstruction) = historical_reconstruction.as_mut() {
                            reconstruction.body_settled();
                        }
                        return Err(error);
                    }
                },
            };
            Ok(EntityReconstructionOutcome::Incomplete { response, handle })
        }
    }
}

fn replay_body_failure(
    invocation: &EntityInvocationId,
    error: WorkerExecutorError,
) -> WorkerExecutorError {
    WorkerExecutorError::unexpected_oplog_entry(
        format!("reconstructable entity invocation body for {invocation}"),
        format!("entity replay body failed before its recorded terminal: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::tool::ToolName;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;
    use tokio::sync::oneshot;

    fn no_structural_stall() -> impl Future<Output = Result<OplogIndex, WorkerExecutorError>> {
        std::future::pending()
    }

    fn invocation() -> EntityInvocationId {
        let owner = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "owner".to_string(),
            },
        );
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner,
                entity: AgentEntity::Tool(ToolName::try_from("entity").unwrap()),
            },
            OplogIndex::from_u64(2),
        )
        .unwrap()
    }

    #[test]
    async fn recorded_tool_terminals_must_match_their_authoritative_payload() {
        let cancellation = skipped_tool_terminal(SerializableToolRpcError::Cancelled)
            .await
            .unwrap();
        let rejection = skipped_tool_terminal(SerializableToolRpcError::Denied(
            "recorded decision".to_string(),
        ))
        .await
        .unwrap();

        assert_eq!(
            validate_recorded_cancellation_terminal(cancellation.clone(), &cancellation).unwrap(),
            cancellation.clone()
        );
        assert!(validate_recorded_cancellation_terminal(rejection.clone(), &cancellation).is_err());
        assert_eq!(
            validate_recorded_rejection_terminal(rejection.clone(), &rejection).unwrap(),
            rejection.clone()
        );
        assert!(validate_recorded_rejection_terminal(cancellation, &rejection).is_err());
    }

    #[test]
    async fn cancellation_after_body_completion_does_not_poll_body_twice() {
        let (body_completed, wait_for_body) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = body_completed.send(());
                Ok::<_, WorkerExecutorError>(7)
            },
            async move {
                let _ = wait_for_body.await;
                Ok(EntityReconstructionResolution::<u64, ()>::Cancelled(9))
            },
            no_structural_stall(),
            move || abort_flag.store(true, Ordering::Release),
            None::<()>,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(result, EntityReconstructionOutcome::Cancelled(9)));
        assert!(aborted.load(Ordering::Acquire));
    }

    #[test]
    async fn cancellation_aborts_and_drains_a_running_body() {
        let (body_cancelled, wait_for_cancellation) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = wait_for_cancellation.await;
                Err::<u64, _>(WorkerExecutorError::runtime("body was cancelled"))
            },
            async { Ok(EntityReconstructionResolution::<u64, ()>::Cancelled(9)) },
            no_structural_stall(),
            move || {
                abort_flag.store(true, Ordering::Release);
                let _ = body_cancelled.send(());
            },
            None::<()>,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(result, EntityReconstructionOutcome::Cancelled(9)));
        assert!(aborted.load(Ordering::Acquire));
    }

    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    impl ReconstructionGuard for DropProbe {
        fn body_settled(&mut self) {}
    }

    struct BodySettlementProbe(Arc<AtomicBool>);

    impl ReconstructionGuard for BodySettlementProbe {
        fn body_settled(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    async fn replay_failure_aborts_and_drains_body_and_releases_fence() {
        let (body_cancelled, wait_for_cancellation) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let fence_dropped = Arc::new(AtomicBool::new(false));
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = wait_for_cancellation.await;
                Err::<u64, _>(WorkerExecutorError::runtime("body was cancelled"))
            },
            async {
                Err::<EntityReconstructionResolution<u64, ()>, _>(WorkerExecutorError::runtime(
                    "replay failed",
                ))
            },
            no_structural_stall(),
            move || {
                abort_flag.store(true, Ordering::Release);
                let _ = body_cancelled.send(());
            },
            Some(DropProbe(fence_dropped.clone())),
            None,
        )
        .await
        .expect_err("replay failure must terminate reconstruction");

        assert!(error.to_string().contains("replay failed"));
        assert!(aborted.load(Ordering::Acquire));
        assert!(fence_dropped.load(Ordering::Acquire));
    }

    #[test]
    async fn incomplete_replay_uses_resolver_fence_release_before_waiting_for_body() {
        let (fence_released, wait_for_fence) = oneshot::channel();
        let body_settled = Arc::new(AtomicBool::new(false));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingIncomplete,
                async move {
                    let _ = wait_for_fence.await;
                    Ok::<_, WorkerExecutorError>(9)
                },
                async move {
                    let _ = fence_released.send(());
                    Ok(EntityReconstructionResolution::<u64, ()>::Incomplete(()))
                },
                no_structural_stall(),
                || {},
                Some(BodySettlementProbe(body_settled.clone())),
                None,
            ),
        )
        .await
        .expect("the live continuation must not deadlock behind its historical fence")
        .unwrap();

        assert!(matches!(
            result,
            EntityReconstructionOutcome::Incomplete {
                response: 9,
                handle: ()
            }
        ));
        assert!(body_settled.load(Ordering::Acquire));
    }

    #[test]
    async fn cancelled_activation_drains_body() {
        let (body_cancelled, wait_for_cancellation) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let body_settled = Arc::new(AtomicBool::new(false));
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingIncomplete,
            async move {
                let _ = wait_for_cancellation.await;
                Err::<u64, _>(WorkerExecutorError::runtime("body was cancelled"))
            },
            async { Ok(EntityReconstructionResolution::<u64, ()>::LiveAdmissionCancelled(())) },
            no_structural_stall(),
            move || {
                abort_flag.store(true, Ordering::Release);
                let _ = body_cancelled.send(());
            },
            Some(BodySettlementProbe(body_settled.clone())),
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            result,
            EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled { handle: () }
        ));
        assert!(aborted.load(Ordering::Acquire));
        assert!(body_settled.load(Ordering::Acquire));
    }

    #[test]
    async fn replayed_response_divergence_is_permanent() {
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async { Ok::<_, WorkerExecutorError>(7) },
            async { Ok(EntityReconstructionResolution::<u64, ()>::Replayed(8)) },
            no_structural_stall(),
            || {},
            None::<()>,
            None,
        )
        .await
        .expect_err("a reconstructed result must match the recorded outer response");

        assert!(
            error
                .to_string()
                .contains("reconstructed a different result")
        );
    }

    #[test]
    async fn replayed_response_waits_for_and_agrees_with_body() {
        let (body_completed, wait_for_body) = oneshot::channel();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = body_completed.send(());
                Ok::<_, WorkerExecutorError>(7)
            },
            async move {
                let _ = wait_for_body.await;
                Ok(EntityReconstructionResolution::<u64, ()>::Replayed(7))
            },
            no_structural_stall(),
            || {},
            None::<()>,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(result, EntityReconstructionOutcome::Replayed(7)));
    }

    #[test]
    async fn body_failure_before_replayed_terminal_is_not_hidden() {
        let (body_completed, wait_for_body) = oneshot::channel();
        let fence_dropped = Arc::new(AtomicBool::new(false));
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = body_completed.send(());
                Err::<u64, _>(WorkerExecutorError::runtime("body replay failed"))
            },
            async move {
                let _ = wait_for_body.await;
                Ok(EntityReconstructionResolution::<u64, ()>::Replayed(7))
            },
            no_structural_stall(),
            || {},
            Some(DropProbe(fence_dropped.clone())),
            None,
        )
        .await
        .expect_err("a replayed terminal must not hide body failure");

        assert!(error.to_string().contains("body replay failed"));
        assert!(fence_dropped.load(Ordering::Acquire));
    }

    #[test]
    async fn completed_body_failure_does_not_wait_for_blocked_replay() {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async { Err::<u64, _>(WorkerExecutorError::runtime("body replay failed")) },
                std::future::pending::<
                    Result<EntityReconstructionResolution<u64, ()>, WorkerExecutorError>,
                >(),
                no_structural_stall(),
                || {},
                None::<()>,
                None,
            ),
        )
        .await
        .expect("body failure must not wait for an outer terminal blocked by that body")
        .expect_err("completed reconstruction body failure is permanent");

        assert!(error.to_string().contains("body replay failed"));
    }

    #[test]
    async fn completed_body_underconsumption_is_structural_divergence() {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async { Ok::<_, WorkerExecutorError>(7) },
                std::future::pending::<
                    Result<EntityReconstructionResolution<u64, ()>, WorkerExecutorError>,
                >(),
                async { Ok(OplogIndex::from_u64(3)) },
                || {},
                None::<()>,
                None,
            ),
        )
        .await
        .expect("structural divergence must not wait for an unreachable outer terminal")
        .expect_err("underconsumed recorded descendants are permanent divergence");

        assert!(error.to_string().contains("recorded descendant at 3"));
    }

    #[test]
    async fn incomplete_body_failure_remains_a_live_failure() {
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingIncomplete,
            async { Err::<u64, _>(WorkerExecutorError::runtime("live continuation failed")) },
            async { Ok(EntityReconstructionResolution::<u64, ()>::Incomplete(())) },
            no_structural_stall(),
            || {},
            None::<()>,
            None,
        )
        .await
        .expect_err("the live continuation failure must propagate");

        let message = error.to_string();
        assert!(message.contains("live continuation failed"));
        assert!(!message.contains("reconstructable entity invocation body"));
    }

    #[test]
    async fn cancellation_during_incomplete_replay_becomes_a_live_cancellation() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let (body_failed, wait_for_body_failure) = oneshot::channel();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingIncomplete,
            async move {
                let _ = body_failed.send(());
                Err::<u64, _>(WorkerExecutorError::runtime("cancellation interruption"))
            },
            async move {
                let _ = wait_for_body_failure.await;
                tokio::task::yield_now().await;
                Ok(EntityReconstructionResolution::<u64, ()>::Incomplete(()))
            },
            no_structural_stall(),
            || {},
            None::<()>,
            Some(&cancellation),
        )
        .await
        .expect("an explicit cancellation must not become an infrastructure failure");

        assert!(matches!(
            result,
            EntityReconstructionOutcome::IncompleteCancelled { handle: () }
        ));
    }
}
