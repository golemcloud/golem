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
//! body handle back to [`EntityInvocationDurability::drive_owned`].

use crate::durable_host::DurableWorkerCtx;
pub(crate) use crate::durable_host::concurrent::EntitySettlementContext;
use crate::durable_host::concurrent::{
    AccessClaimOptions, DurableCallSession, HistoricalReconstruction, InvocationIdSource,
    LeaveIncompleteOnDrop, ReconstructionReplayOutcome, ReplayAccessStartOutcome,
};
use crate::durable_host::durable_session::strip_typed_streams;
use crate::durable_host::replay_state::ReplayState;
use crate::services::oplog::OplogOps;
use crate::services::shutdown::join_or_park_at_runtime_shutdown;
use crate::worker::entity_invocation::{
    EntityBodyOutcome, EntityInvocationHandle, EntityInvocationResources,
};
use crate::worker::owner_lane::OwnerInvocationId;
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::agent::{AgentPrincipal, Principal};
use golem_common::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationId,
    EntityInvocationPlan, EntityInvocationPlanReference, EntityInvocationRequest,
    EntityInvocationScope, InvocationExecutionMode, OwnedAgentEntityId,
};
use golem_common::model::invocation_context::{AttributeValue, SpanId};
use golem_common::model::oplog::host_functions::{GolemEntityInvoke, GolemToolInvocationRejected};
use golem_common::model::oplog::payload::types::{
    SerializableEntityBodyExecution, SerializableToolAdmissionRefusal,
    SerializableToolOperationTerminal, SerializableToolRpcError,
};
use golem_common::model::oplog::{
    AttributeMap, DurableFunctionType, HostPayloadPair, HostRequest, HostRequestEntityInvocation,
    HostRequestGolemToolInvocationRejected, HostResponseEntityInvocation, OplogEntry, OplogIndex,
    SpanFinished, SpanKind, SpanOutcome, SpanStarted,
};
use golem_common::schema::{IntoTypedSchemaValue, TypedSchemaValue};
use golem_schema::FromSchema;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
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
/// Executor shutdown leaves an unfinished Start incomplete for recovery. Before execution
/// ownership transfers, semantic caller Store disposal instead settles a skipped body.
pub struct EntityInvocationDurability {
    handle: DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>,
    scope: EntityInvocationScope,
    principal: Principal,
    parent: OwnerInvocationId,
    call_mode: EntityCallMode,
    operation: EntityInvocationDescriptor,
    input: TypedSchemaValue,
    resolved_position: ResolvedEntityInvocationPosition,
    historical_reconstruction: Option<HistoricalReconstruction>,
    scope_interest: Arc<crate::services::card_interest::CardInterestContribution>,
}

/// Caller identity captured in guest admission order, before asynchronous dispatch or replay claims.
#[derive(Clone)]
pub(crate) struct EntityInvocationKeyContext {
    caller_key: golem_common::model::IdempotencyKey,
    logical_position: Option<OplogIndex>,
    stream_session_idempotency_key: golem_common::model::IdempotencyKey,
}

impl EntityInvocationKeyContext {
    pub(crate) fn capture<Ctx: WorkerCtx>(
        ctx: &mut DurableWorkerCtx<Ctx>,
        attempt_ordinal: u64,
    ) -> Result<Self, WorkerExecutorError> {
        let caller_key = ctx.state.get_current_idempotency_key().ok_or_else(|| {
            WorkerExecutorError::runtime("entity invocation requires an active caller key")
        })?;
        let logical_position = if ctx
            .entity_invocation_scope()
            .is_some_and(|scope| scope.logical_key_positions())
        {
            Some(
                ctx.state
                    .current_idempotency_key_oplog_index(OplogIndex::NONE),
            )
        } else {
            None
        };
        let stream_parent_key = ctx
            .entity_invocation_scope()
            .map(|scope| scope.stream_session_idempotency_key().clone())
            .unwrap_or_else(|| caller_key.clone());
        let stream_session_idempotency_key =
            derive_entity_stream_session_key(&stream_parent_key, logical_position, attempt_ordinal);
        Ok(Self {
            caller_key,
            logical_position,
            stream_session_idempotency_key,
        })
    }
}

fn derive_entity_stream_session_key(
    parent: &golem_common::model::IdempotencyKey,
    logical_position: Option<OplogIndex>,
    attempt_ordinal: u64,
) -> golem_common::model::IdempotencyKey {
    let mut stream_name = b"golem:entity-stream-session:v1\0".to_vec();
    match logical_position {
        Some(position) => {
            stream_name.extend_from_slice(b"logical\0");
            stream_name.extend_from_slice(&position.as_u64().to_be_bytes());
        }
        None => {
            stream_name.extend_from_slice(b"attempt-ordinal\0");
            stream_name.extend_from_slice(&attempt_ordinal.to_be_bytes());
        }
    }
    golem_common::model::IdempotencyKey::derived_from_bytes(parent, &stream_name)
}

fn entity_span_started(
    trace_id: golem_common::model::invocation_context::TraceId,
    trace_states: Vec<String>,
    parent_span_id: SpanId,
    entity: &AgentEntity,
    operation: &EntityInvocationDescriptor,
) -> SpanStarted {
    let command_path = match operation {
        EntityInvocationDescriptor::Tool(tool) => tool.command_path.join("/"),
    };
    SpanStarted {
        span_id: SpanId::generate(),
        trace_id,
        trace_states,
        parent_span_id: Some(parent_span_id),
        links: Vec::new(),
        started_at: golem_common::model::Timestamp::now_utc(),
        attributes: AttributeMap(HashMap::from([
            (
                "name".to_string(),
                AttributeValue::String("entity-invocation".to_string()),
            ),
            (
                "entity.kind".to_string(),
                AttributeValue::String(entity.kind_label().to_string()),
            ),
            (
                "entity.name".to_string(),
                AttributeValue::String(entity.name().to_string()),
            ),
            (
                "operation.command_path".to_string(),
                AttributeValue::String(command_path),
            ),
        ])),
        kind: SpanKind::Internal,
    }
}

fn tool_terminal_outcome(
    response: &HostResponseEntityInvocation,
) -> Result<SpanOutcome, WorkerExecutorError> {
    let value = response.result.as_ref().map_err(|error| {
        WorkerExecutorError::runtime(format!("entity invocation terminal failed: {error}"))
    })?;
    let terminal =
        SerializableToolOperationTerminal::from_value(value.value()).map_err(|error| {
            WorkerExecutorError::runtime(format!("invalid durable tool terminal: {error}"))
        })?;
    Ok(match terminal.result {
        Ok(_) => SpanOutcome::Completed,
        Err(SerializableToolRpcError::Denied(_)) => SpanOutcome::Denied,
        Err(SerializableToolRpcError::Cancelled) => SpanOutcome::Cancelled,
        Err(_) => SpanOutcome::Failed,
    })
}

fn entity_span_finished(
    span_id: &SpanId,
    response: &HostResponseEntityInvocation,
) -> Result<SpanFinished, WorkerExecutorError> {
    Ok(SpanFinished {
        span_id: span_id.clone(),
        finished_at: golem_common::model::Timestamp::now_utc(),
        outcome: tool_terminal_outcome(response)?,
    })
}

#[derive(Clone)]
pub struct ResolvedEntityInvocationPosition {
    plan: Arc<EntityInvocationPlan>,
    root_start_index: OplogIndex,
    position: u32,
}

impl ResolvedEntityInvocationPosition {
    pub fn plan(&self) -> &Arc<EntityInvocationPlan> {
        &self.plan
    }

    pub fn root_start_index(&self) -> OplogIndex {
        self.root_start_index
    }

    pub fn position(&self) -> u32 {
        self.position
    }

    pub fn layer(&self) -> &golem_common::model::entity::EntityInvocationPlanLayer {
        self.plan
            .layer(self.position)
            .expect("validated plan position")
    }

    pub fn next_reference(&self) -> Option<EntityInvocationPlanReference> {
        ((self.position as usize + 1) < self.plan.len()).then(|| {
            EntityInvocationPlanReference::Descendant {
                root_start_index: self.root_start_index,
                position: self.position + 1,
            }
        })
    }
}

impl EntityInvocationDurability {
    /// Transfers no-body settlement to the independent tool execution owner. Call this only
    /// after that owner has accepted the operation, without an intervening await.
    pub(crate) fn accept_execution_ownership(&mut self) {
        self.handle.accept_entity_execution_ownership();
    }

    pub(crate) async fn start_live_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        key_context: &EntityInvocationKeyContext,
        entity: AgentEntity,
        principal: Principal,
        call_mode: EntityCallMode,
        operation: EntityInvocationDescriptor,
        plan: EntityInvocationPlanReference,
        input: TypedSchemaValue,
        invocation_id: InvocationIdSource,
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
            call_mode,
            operation,
            principal,
            plan,
            authority_wallet: store
                .with(|mut access| get_ctx(access.data_mut()).agent_wallet_cards_snapshot()),
        };
        let encoded_metadata = desert_rust::serialize_to_byte_vec(&metadata).map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "failed to encode entity invocation metadata: {error}"
            ))
        })?;
        let request = HostRequestEntityInvocation {
            metadata: encoded_metadata,
            input: strip_typed_streams(&input),
            stream_session_idempotency_key: key_context.stream_session_idempotency_key.clone(),
        };
        let started_input = request.input.clone();
        let (trace_id, trace_states, parent_span_id) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.invocation_context.trace_id.clone(),
                ctx.state.invocation_context.trace_states.clone(),
                ctx.state.current_span_id.clone(),
            )
        });
        let span_started = entity_span_started(
            trace_id,
            trace_states,
            parent_span_id,
            &entity,
            &metadata.operation,
        );
        let handle = DurableCallSession::<GolemEntityInvoke, LeaveIncompleteOnDrop>::start_access_with_options_and_span(
                store,
                get_ctx,
                DurableFunctionType::WriteLocal,
                AccessClaimOptions {
                    parent_start_index: Some(parent_start_index),
                    incomplete_repair_authorized_by_scope: true,
                    invocation_id,
                    ..AccessClaimOptions::default()
                },
                request,
                span_started.clone(),
            )
            .await?;
        Self::from_started_request(
            store,
            get_ctx,
            parent,
            key_context,
            key_context.stream_session_idempotency_key.clone(),
            handle,
            metadata,
            started_input,
            Some(span_started),
        )
        .await
    }

    pub(crate) async fn replay_tool_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        key_context: &EntityInvocationKeyContext,
        invocation_id: InvocationIdSource,
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
                    tool_rejection_function_name: Some(
                        GolemToolInvocationRejected::HOST_FUNCTION_NAME,
                    ),
                    parent_start_index: Some(parent_start_index),
                    incomplete_repair_authorized_by_scope: true,
                    invocation_id,
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
        if let Some(operation) =
            store.with(|mut access| get_ctx(access.data_mut()).entity_tool_operation())
            && let Some((expected, _)) = operation.expected_admission_refusal()
            && expected
                == (SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                    index: handle.start_index().into(),
                })
        {
            let error = operation.reproduce_admission_refusal(expected).await;
            if !crate::durable_host::is_tool_attachment_live_admission_rejection(&error) {
                return Err(error);
            }
            // The original incomplete-child repair retained the child after refusing its
            // parent's live admission. Reconstruct and validate that child here as well.
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
                        key_context,
                        request.stream_session_idempotency_key,
                        handle,
                        metadata,
                        request.input,
                        None,
                    )
                    .await?,
                )))
            }
            HostRequest::GolemToolInvocationRejected(request) => {
                // This shared claim path can repair a rejection, not only an accepted body.
                // A rejection must complete with its recorded error, never an entity cancellation.
                handle.accept_entity_execution_ownership();
                let span_started = handle
                    .recorded_span_started_access(store, get_ctx)
                    .await?
                    .ok_or_else(|| {
                        WorkerExecutorError::runtime("tool rejection Start has no span metadata")
                    })?;
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
                    ReconstructionReplayOutcome::Incomplete(live) => {
                        let finished = entity_span_finished(&span_started.span_id, &expected)?;
                        live.complete_access_with_span(store, get_ctx, expected, finished)
                            .await
                            .map_err(|error| error.source)?
                    }
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
        key_context: &EntityInvocationKeyContext,
        stream_session_idempotency_key: golem_common::model::IdempotencyKey,
        mut handle: DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>,
        metadata: EntityInvocationRequest,
        input: TypedSchemaValue,
        span_started: Option<SpanStarted>,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let parent_start_index = parent.start_index();
        let (owner, invocation_scope_card, interest) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.owned_agent_id.clone(),
                ctx.current_invocation_scope_card(),
                ctx.state.owner_wallet.interest.clone(),
            )
        });
        let invocation_id = EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner.clone(),
                entity: metadata.entity.clone(),
            },
            handle.start_index(),
        )
        .map_err(WorkerExecutorError::runtime)?;
        let scope_interest = interest.contribute(
            Some(invocation_id.clone()),
            invocation_scope_card
                .as_ref()
                .map(|scope| scope.root_card_ids.as_slice())
                .unwrap_or_default(),
        );
        let calling_principal = Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        });
        let idempotency_key = golem_common::model::IdempotencyKey::derived(
            &key_context.caller_key,
            key_context.logical_position.unwrap_or(handle.start_index()),
        );
        let operation = metadata.operation;
        let resolved_position = resolve_recorded_plan(
            store,
            get_ctx,
            handle.start_index(),
            &metadata.entity,
            metadata.plan,
        )
        .await?;
        let historical_reconstruction = if handle.is_live() {
            None
        } else {
            Some(
                handle
                    .take_historical_reconstruction()
                    .expect("replayed entity invocation must own a reconstruction claim"),
            )
        };
        let principal = metadata.principal;
        let activation = Arc::new(resolved_position.layer().activation().clone());
        let execution_mode = if handle.is_live() {
            InvocationExecutionMode::Live
        } else {
            let replay =
                store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
            if replay.has_visible_terminal(handle.start_index()).await? {
                InvocationExecutionMode::ReplayingCompleted
            } else {
                InvocationExecutionMode::ReplayingIncomplete
            }
        };
        let span_started = match span_started {
            Some(span_started) => span_started,
            None => handle
                .recorded_span_started_access(store, get_ctx)
                .await?
                .ok_or_else(|| {
                    WorkerExecutorError::runtime("entity invocation Start has no span metadata")
                })?,
        };
        handle.close_span_on_cancellation(span_started.span_id.clone());
        let scope = EntityInvocationScope::new(
            invocation_id,
            parent_start_index,
            activation,
            calling_principal,
            execution_mode,
            idempotency_key,
            true,
            stream_session_idempotency_key,
        )
        .map_err(WorkerExecutorError::runtime)?
        .with_authority_wallet(metadata.authority_wallet)
        .with_invocation_scope_card(invocation_scope_card)
        .with_span_started(span_started);
        Ok(Self {
            handle,
            scope,
            principal,
            parent,
            call_mode: metadata.call_mode,
            operation,
            input,
            resolved_position,
            historical_reconstruction,
            scope_interest,
        })
    }

    pub fn scope(&self) -> &EntityInvocationScope {
        &self.scope
    }

    pub(crate) fn historical_reconstruction_hold(&self) -> Option<HistoricalReconstruction> {
        self.historical_reconstruction.clone()
    }

    pub(crate) fn scope_interest(
        &self,
    ) -> Arc<crate::services::card_interest::CardInterestContribution> {
        self.scope_interest.clone()
    }

    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    pub fn operation(&self) -> &EntityInvocationDescriptor {
        &self.operation
    }

    pub fn input(&self) -> &TypedSchemaValue {
        &self.input
    }

    pub fn resolved_position(&self) -> &ResolvedEntityInvocationPosition {
        &self.resolved_position
    }

    pub fn call_mode(&self) -> EntityCallMode {
        self.call_mode
    }

    /// Converts a replayed incomplete Start into its live-repair handle before a body exists.
    /// An empty replay-visible entity scope must release its historical reconstruction fence before
    /// dispatch, so the primary owner can transition to the live tail.
    pub(crate) async fn enter_incomplete_live_repair_before_body_owned<Ctx: WorkerCtx>(
        self,
        context: &EntitySettlementContext<Ctx>,
        ready_for_caller_replay: impl FnOnce(),
    ) -> Result<IncompleteLiveRepairBeforeBody, WorkerExecutorError> {
        if self.scope.mode() != InvocationExecutionMode::ReplayingIncomplete {
            return Ok(IncompleteLiveRepairBeforeBody::Ready(self));
        }
        let replay = &context.replay;
        if replay
            .has_visible_scope_descendant(self.handle.start_index())
            .await?
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
            resolved_position,
            mut historical_reconstruction,
            scope_interest,
        } = self;
        // An empty body has nothing to reconstruct before the caller can replay its later
        // host calls. Those calls must advance the cursor before incomplete repair can finish.
        ready_for_caller_replay();
        let (handle, cancelled) = match handle.replay_entity_owned(context, false).await? {
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
            scope.idempotency_key().clone(),
            scope.logical_key_positions(),
            scope.stream_session_idempotency_key().clone(),
        )
        .map_err(WorkerExecutorError::runtime)?
        .with_authority_wallet(scope.authority_wallet().to_vec())
        .with_invocation_scope_card(scope.invocation_scope_card().cloned())
        .with_span_started(
            scope
                .span_started()
                .expect("entity invocation scope must retain its recorded opening")
                .clone(),
        );

        let durability = Self {
            handle,
            scope,
            principal,
            parent,
            call_mode,
            operation,
            input,
            resolved_position,
            historical_reconstruction: None,
            scope_interest,
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
    pub(crate) async fn recorded_terminal_owned<Ctx: WorkerCtx>(
        &self,
        context: &EntitySettlementContext<Ctx>,
    ) -> Result<Option<RecordedEntityTerminal>, WorkerExecutorError> {
        if self.scope.mode() != InvocationExecutionMode::ReplayingCompleted {
            return Ok(None);
        }
        let (replay, oplog) = (&context.replay, &context.oplog);
        let entry = replay
            .visible_terminal_entry(self.handle.start_index())
            .await?
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
    pub(crate) async fn complete_without_body_owned<Ctx: WorkerCtx>(
        self,
        context: &EntitySettlementContext<Ctx>,
        response: HostResponseEntityInvocation,
    ) -> Result<EntityInvocationDurabilityOutcome, WorkerExecutorError> {
        let completed = self.scope.mode() == InvocationExecutionMode::ReplayingCompleted;
        let span_id = self
            .scope
            .span_started()
            .expect("entity invocation scope must retain its recorded opening")
            .span_id
            .clone();
        let Self {
            handle,
            mut historical_reconstruction,
            scope_interest: _scope_interest,
            ..
        } = self;
        if let Some(reconstruction) = historical_reconstruction.as_mut() {
            reconstruction.body_settled();
        }
        let response = if handle.is_live() {
            let span_finished = entity_span_finished(&span_id, &response)?;
            handle
                .complete_entity_owned(context, response, span_finished)
                .await?
        } else {
            match handle.replay_entity_owned(context, completed).await? {
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
                ReconstructionReplayOutcome::Incomplete(live) => {
                    let span_finished = entity_span_finished(&span_id, &response)?;
                    live.complete_entity_owned(context, response, span_finished)
                        .await?
                }
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
    pub(crate) async fn cancel_without_body_owned<Ctx: WorkerCtx>(
        self,
        context: &EntitySettlementContext<Ctx>,
    ) -> Result<EntityInvocationDurabilityOutcome, WorkerExecutorError> {
        let completed = self.scope.mode() == InvocationExecutionMode::ReplayingCompleted;
        let Self {
            handle,
            mut historical_reconstruction,
            scope_interest: _scope_interest,
            ..
        } = self;
        if let Some(reconstruction) = historical_reconstruction.as_mut() {
            reconstruction.body_settled();
        }
        let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Skipped).await?;
        let response = if handle.is_live() {
            handle
                .cancel_entity_owned(context, response.clone())
                .await?;
            response
        } else {
            match handle.replay_entity_owned(context, completed).await? {
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
                    live.cancel_entity_owned(context, response.clone()).await?;
                    response
                }
            }
        };
        Ok(EntityInvocationDurabilityOutcome::Cancelled(
            Box::new(response),
            None,
        ))
    }

    /// Drives an accepted entity session without borrowing the caller Store. The owner must
    /// retain and join this future through operation settlement, including after a guest trap.
    pub(crate) async fn drive_owned<Ctx, Started, Cancelled, Failed, FailureFuture>(
        mut self,
        context: &EntitySettlementContext<Ctx>,
        body: EntityInvocationHandle<HostResponseEntityInvocation>,
        cancellation: Option<tokio_util::sync::CancellationToken>,
        on_completed_started: Started,
        on_completed_cancelled: Cancelled,
        on_completed_failure: Failed,
    ) -> Result<EntityInvocationDurabilityOutcome, EntityInvocationDurabilityFailure>
    where
        Ctx: WorkerCtx,
        Started: FnOnce() + Send,
        Cancelled: FnOnce() + Send,
        Failed: FnOnce(WorkerExecutorError) -> FailureFuture + Send,
        FailureFuture: Future<Output = ()> + Send,
    {
        self.accept_execution_ownership();
        let span_id = self
            .scope
            .span_started()
            .expect("entity invocation scope must retain its recorded opening")
            .span_id
            .clone();
        let Self {
            mut handle,
            scope,
            parent,
            call_mode,
            historical_reconstruction,
            scope_interest: _scope_interest,
            ..
        } = self;
        let invocation = scope.invocation_id().clone();
        let completed = scope.mode() == InvocationExecutionMode::ReplayingCompleted;
        let abort = body.abort_handle();
        let resources = Arc::new(Mutex::new(None));
        let body_resources = resources.clone();
        let body = async move {
            let completion = match call_mode {
                EntityCallMode::FireAndForget => body.join_completion().await,
                EntityCallMode::Synchronous | EntityCallMode::Asynchronous => {
                    body.await_completion(&parent).await
                }
            }?;
            let (response, resources) = completion.into_parts();
            *body_resources.lock().unwrap() = Some(resources);
            response
        };
        let mut on_cancelled = Some(on_completed_cancelled);
        let fail = |error| EntityInvocationDurabilityFailure {
            error,
            resources: take_entity_resources(&resources),
        };
        tokio::pin!(body);
        if handle.is_live() {
            let response = context
                .coordinate(&invocation, async {
                    match &cancellation {
                        Some(cancellation) => tokio::select! {
                            biased;
                            _ = cancellation.cancelled() => None,
                            response = &mut body => Some(response),
                        },
                        None => Some(body.as_mut().await),
                    }
                })
                .await;
            let Some(response) = response else {
                on_cancelled.take().unwrap()();
                context.coordinate(&invocation, body).await.map_err(fail)?;
                let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Executed)
                    .await
                    .map_err(fail)?;
                handle
                    .cancel_entity_owned(context, response.clone())
                    .await
                    .map_err(fail)?;
                return Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&resources),
                ));
            };
            let response = response.map_err(|error| {
                let _ = handle.trap(error.clone());
                fail(error)
            })?;
            let response = match response {
                EntityBodyOutcome::Completed(response) => response,
                EntityBodyOutcome::Cancelled => {
                    on_cancelled.take().unwrap()();
                    let response =
                        cancelled_tool_terminal(SerializableEntityBodyExecution::Executed)
                            .await
                            .map_err(fail)?;
                    handle
                        .cancel_entity_owned(context, response.clone())
                        .await
                        .map_err(fail)?;
                    return Ok(EntityInvocationDurabilityOutcome::Cancelled(
                        Box::new(response),
                        take_entity_resources(&resources),
                    ));
                }
            };
            let span = entity_span_finished(&span_id, &response).map_err(fail)?;
            let response = handle
                .complete_entity_owned(context, response, span)
                .await
                .map_err(fail)?;
            return Ok(EntityInvocationDurabilityOutcome::Completed(
                Box::new(response),
                take_entity_resources(&resources),
            ));
        }

        // Keep the completed reconstruction fence until failure supervision has acknowledged
        // the error. Cursor exhaustion must not publish live across failed reconstruction.
        let retained_reconstruction = historical_reconstruction.clone();
        if completed {
            on_completed_started();
        }
        let replay_resolved = std::sync::atomic::AtomicBool::new(false);
        let replay = async {
            let resolution = match handle.replay_entity_owned(context, completed).await? {
                ReconstructionReplayOutcome::Replayed(response) => {
                    EntityReconstructionResolution::Replayed(response)
                }
                ReconstructionReplayOutcome::Cancelled(response) => {
                    context
                        .replay
                        .await_tool_attachment_reconstruction(invocation.start_index())
                        .await?;
                    on_cancelled.take().unwrap()();
                    EntityReconstructionResolution::Cancelled(response)
                }
                ReconstructionReplayOutcome::Incomplete(handle) => {
                    if cancellation
                        .as_ref()
                        .is_some_and(|token| token.is_cancelled())
                    {
                        on_cancelled.take().unwrap()();
                    }
                    EntityReconstructionResolution::Incomplete(handle)
                }
                ReconstructionReplayOutcome::LiveAdmissionCancelled(handle) => {
                    EntityReconstructionResolution::LiveAdmissionCancelled(handle)
                }
            };
            replay_resolved.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(resolution)
        };
        let active_bodies = context.replay.historical_reconstruction_bodies();
        let reconstruction = std::panic::AssertUnwindSafe(async {
            let reconstruction = context
                .coordinate_after(
                    &invocation,
                    &replay_resolved,
                    coordinate_entity_reconstruction(
                        &invocation,
                        scope.mode(),
                        body,
                        replay,
                        context
                            .replay
                            .await_unconsumed_scope_entry(invocation.start_index(), active_bodies),
                        || abort.abort(),
                        historical_reconstruction,
                        cancellation.as_ref(),
                    ),
                )
                .await?;
            ensure_body_claimed_retained_descendants(&context.replay, &invocation, &reconstruction)
                .await?;
            Ok::<_, WorkerExecutorError>(reconstruction)
        })
        .catch_unwind()
        .await;
        let reconstruction = match reconstruction {
            Ok(Ok(reconstruction)) => reconstruction,
            failure => {
                let error = match failure {
                    Ok(Err(error)) => error,
                    Err(_) => WorkerExecutorError::runtime(
                        "owned entity reconstruction coordinator panicked",
                    ),
                    Ok(Ok(_)) => unreachable!(),
                };
                if completed {
                    on_completed_failure(error.clone()).await;
                }
                drop(retained_reconstruction);
                return Err(fail(error));
            }
        };
        drop(retained_reconstruction);
        match reconstruction {
            EntityReconstructionOutcome::Replayed(response) => {
                Ok(EntityInvocationDurabilityOutcome::Completed(
                    Box::new(response),
                    take_entity_resources(&resources),
                ))
            }
            EntityReconstructionOutcome::Cancelled(response) => {
                Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&resources),
                ))
            }
            EntityReconstructionOutcome::Incomplete { response, handle } => {
                let span = entity_span_finished(&span_id, &response).map_err(fail)?;
                let response = handle
                    .complete_entity_owned(context, response, span)
                    .await
                    .map_err(fail)?;
                Ok(EntityInvocationDurabilityOutcome::Completed(
                    Box::new(response),
                    take_entity_resources(&resources),
                ))
            }
            EntityReconstructionOutcome::IncompleteCancelled { handle } => {
                let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Executed)
                    .await
                    .map_err(fail)?;
                handle
                    .cancel_entity_owned(context, response.clone())
                    .await
                    .map_err(fail)?;
                Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&resources),
                ))
            }
            EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled { handle } => {
                let response = cancelled_tool_terminal(SerializableEntityBodyExecution::Skipped)
                    .await
                    .map_err(fail)?;
                handle
                    .cancel_entity_owned(context, response.clone())
                    .await
                    .map_err(fail)?;
                Ok(EntityInvocationDurabilityOutcome::Cancelled(
                    Box::new(response),
                    take_entity_resources(&resources),
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

pub(crate) async fn record_tool_rejection_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    parent: OwnerInvocationId,
    mut request: HostRequestGolemToolInvocationRejected,
    invocation_id: InvocationIdSource,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    request.input = request.input.as_ref().map(strip_typed_streams);
    let response = skipped_tool_terminal(request.error.clone()).await?;
    let (trace_id, trace_states, parent_span_id) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.state.invocation_context.trace_id.clone(),
            ctx.state.invocation_context.trace_states.clone(),
            ctx.state.current_span_id.clone(),
        )
    });
    let span_started = SpanStarted {
        span_id: SpanId::generate(),
        trace_id,
        trace_states,
        parent_span_id: Some(parent_span_id),
        links: Vec::new(),
        started_at: golem_common::model::Timestamp::now_utc(),
        attributes: AttributeMap(HashMap::from([
            (
                "name".to_string(),
                AttributeValue::String("entity-invocation".to_string()),
            ),
            (
                "entity.kind".to_string(),
                AttributeValue::String("tool".to_string()),
            ),
            (
                "entity.name".to_string(),
                AttributeValue::String(request.tool_name.clone()),
            ),
            (
                "operation.command_path".to_string(),
                AttributeValue::String(request.command_path.join("/")),
            ),
        ])),
        kind: SpanKind::Internal,
    };
    let span_finished = entity_span_finished(&span_started.span_id, &response)?;
    let handle = DurableCallSession::<GolemToolInvocationRejected, LeaveIncompleteOnDrop>::start_access_with_options_and_span(
        store,
        get_ctx,
        DurableFunctionType::WriteLocal,
        AccessClaimOptions {
            parent_start_index: Some(parent.start_index()),
            incomplete_repair_authorized_by_scope: true,
            invocation_id,
            ..AccessClaimOptions::default()
        },
        request,
        span_started,
    )
    .await?;
    handle
        .complete_access_with_span(store, get_ctx, response, span_finished)
        .await
        .map_err(|error| error.source)
}

pub(crate) async fn encode_tool_terminal(
    terminal: SerializableToolOperationTerminal,
    context: &'static str,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    let result = join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
        terminal.into_typed_schema_value()
    }))
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
            admission_refusal: None,
        },
        "failed to encode skipped tool terminal",
    )
    .await
}

pub(crate) async fn cancelled_tool_terminal(
    body_execution: SerializableEntityBodyExecution,
) -> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        SerializableToolOperationTerminal {
            body_execution,
            result: Err(SerializableToolRpcError::Cancelled),
            admission_refusal: None,
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

async fn resolve_recorded_plan<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    current_start_index: OplogIndex,
    entity: &AgentEntity,
    reference: EntityInvocationPlanReference,
) -> Result<ResolvedEntityInvocationPosition, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let root_metadata = match &reference {
        EntityInvocationPlanReference::Root { .. } => None,
        EntityInvocationPlanReference::Descendant {
            root_start_index, ..
        } => {
            let request = load_recorded_request(store, get_ctx, *root_start_index).await?;
            let HostRequest::EntityInvocation(request) = request else {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "root entity invocation request",
                    format!("{request:?}"),
                ));
            };
            Some(request.metadata)
        }
    };
    resolve_recorded_plan_from_root_payload(
        current_start_index,
        entity,
        reference,
        root_metadata.as_deref(),
    )
}

fn resolve_recorded_plan_from_root_payload(
    current_start_index: OplogIndex,
    entity: &AgentEntity,
    reference: EntityInvocationPlanReference,
    root_metadata: Option<&[u8]>,
) -> Result<ResolvedEntityInvocationPosition, WorkerExecutorError> {
    let (plan, root_start_index, position) = match reference {
        EntityInvocationPlanReference::Root { plan } => (plan, current_start_index, 0),
        EntityInvocationPlanReference::Descendant {
            root_start_index,
            position,
        } => {
            if root_start_index >= current_start_index || position == 0 {
                return Err(WorkerExecutorError::runtime(
                    "invalid descendant entity invocation plan reference",
                ));
            }
            let metadata =
                desert_rust::deserialize::<EntityInvocationRequest>(root_metadata.ok_or_else(
                    || WorkerExecutorError::runtime("missing root entity invocation metadata"),
                )?)
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "failed to decode root entity invocation metadata: {error}"
                    ))
                })?;
            let EntityInvocationPlanReference::Root { plan } = metadata.plan else {
                return Err(WorkerExecutorError::runtime(
                    "descendant entity invocation does not reference a plan root",
                ));
            };
            (plan, root_start_index, position)
        }
    };
    plan.validate().map_err(WorkerExecutorError::runtime)?;
    let layer = plan.layer(position).map_err(WorkerExecutorError::runtime)?;
    if &layer.activation().entity() != entity {
        return Err(WorkerExecutorError::runtime(
            "entity invocation selector does not match its recorded plan position",
        ));
    }
    Ok(ResolvedEntityInvocationPosition {
        plan: Arc::new(plan),
        root_start_index,
        position,
    })
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
    let entry = oplog.read(start_index).await?;
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

/// Fails a settled entity body that returned without claiming one of its recorded descendant
/// `Start`s. The cursor retains unclaimed `Start`s instead of parking on them, so this is the
/// structural divergence a stalled cursor used to surface through
/// [`ReplayState::await_unconsumed_scope_entry`]. Only outcomes whose body ran to completion are
/// checked: an aborted or cancelled body legitimately leaves its recorded subtree unclaimed.
async fn ensure_body_claimed_retained_descendants<R, H>(
    replay_state: &ReplayState,
    invocation: &EntityInvocationId,
    reconstruction: &EntityReconstructionOutcome<R, H>,
) -> Result<(), WorkerExecutorError> {
    let body_completed = match reconstruction {
        EntityReconstructionOutcome::Replayed(_)
        | EntityReconstructionOutcome::Incomplete { .. } => true,
        EntityReconstructionOutcome::Cancelled(_)
        | EntityReconstructionOutcome::IncompleteCancelled { .. }
        | EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled { .. } => false,
    };
    if !body_completed {
        return Ok(());
    }
    let active_bodies = replay_state
        .historical_reconstruction_bodies()
        .borrow()
        .clone();
    match replay_state
        .unclaimed_retained_descendant(invocation.start_index(), active_bodies)
        .await?
    {
        None => Ok(()),
        Some(unclaimed) => Err(WorkerExecutorError::unexpected_oplog_entry(
            format!("completed replay body for {invocation}"),
            format!("entity body returned before consuming its recorded descendant at {unclaimed}"),
        )),
    }
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
    B: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>>,
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
    _execution_mode: InvocationExecutionMode,
    body: B,
    replay: F,
    structural_stall: S,
    abort: A,
    historical_reconstruction: &mut Option<G>,
    _cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Result<EntityReconstructionOutcome<R, H>, WorkerExecutorError>
where
    R: PartialEq,
    B: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>>,
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
                Err(error) => return Err(error),
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
            let joined = if let Some(result) = body_result {
                result
            } else {
                let result = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
                result
            };
            joined?;
            Ok(EntityReconstructionOutcome::Cancelled(recorded))
        }
        EntityReconstructionResolution::Replayed(recorded) => {
            let reconstructed = match body_result {
                Some(response) => response?,
                None => {
                    let response = body.await;
                    if let Some(reconstruction) = historical_reconstruction.as_mut() {
                        reconstruction.body_settled();
                    }
                    response?
                }
            };
            if reconstructed != EntityBodyOutcome::Completed(recorded) {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!("recorded entity invocation result for {invocation}"),
                    format!("reconstructed a different result for {invocation}"),
                ));
            }
            Ok(EntityReconstructionOutcome::Replayed(
                reconstructed.completed()?,
            ))
        }
        EntityReconstructionResolution::LiveAdmissionCancelled(handle) => {
            abort.take().unwrap()();
            let joined = if let Some(result) = body_result {
                result
            } else {
                let result = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
                result
            };
            joined?;
            Ok(EntityReconstructionOutcome::IncompleteLiveAdmissionCancelled { handle })
        }
        EntityReconstructionResolution::Incomplete(handle) => {
            // The resolver already released the historical fence when replay reached the live
            // tail. Keep body membership until the body actually settles so structural-stall
            // detection can still identify its owned descendants during live repair.
            let response = match body_result {
                Some(response) => response?,
                None => match body.await {
                    Ok(response) => {
                        if let Some(reconstruction) = historical_reconstruction.as_mut() {
                            reconstruction.body_settled();
                        }
                        response
                    }
                    Err(error) => {
                        if let Some(reconstruction) = historical_reconstruction.as_mut() {
                            reconstruction.body_settled();
                        }
                        return Err(error);
                    }
                },
            };
            match response {
                EntityBodyOutcome::Completed(response) => {
                    Ok(EntityReconstructionOutcome::Incomplete { response, handle })
                }
                EntityBodyOutcome::Cancelled => {
                    Ok(EntityReconstructionOutcome::IncompleteCancelled { handle })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{AgentPrincipal, AgentTypeName};
    use golem_common::model::component::ComponentId;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::entity::{
        EntityActivation, EntityActivationPolicy, EntityInvocationPlanLayer, ExecutableTarget,
        FilesystemCapability, ToolInvocationDescriptor, ToolMiddlewareName, ToolOutputContract,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::tool::{
        CompiledToolBinding, ConfigKeyScope, SecretKeyScope, ToolBindingOwner,
        ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolSource,
    };
    use golem_common::schema::tool::{CommandTree, Tool};
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
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
    fn entity_span_descriptor_contains_only_safe_identity_fields() {
        let trace_id = golem_common::model::invocation_context::TraceId::generate();
        let parent = SpanId::generate();
        let entity = AgentEntity::Tool(ToolName::try_from("search").unwrap());
        let operation = EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 19,
            command_path: vec!["files".to_string(), "find".to_string()],
            args: Vec::new(),
            has_stdin: true,
            has_stdout: true,
            declares_stdout: true,
            has_stderr: true,
            declares_stderr: true,
            output_contract: ToolOutputContract {
                result: None,
                errors: Vec::new(),
            },
        });

        let started = entity_span_started(
            trace_id.clone(),
            vec!["vendor=value".to_string()],
            parent.clone(),
            &entity,
            &operation,
        );

        assert_eq!(started.trace_id, trace_id);
        assert_eq!(started.parent_span_id, Some(parent));
        assert!(started.links.is_empty());
        assert_eq!(started.attributes.len(), 4);
        assert_eq!(
            started.attributes.get("entity.name"),
            Some(&AttributeValue::String("search".to_string()))
        );
        assert_eq!(
            started.attributes.get("operation.command_path"),
            Some(&AttributeValue::String("files/find".to_string()))
        );
        assert_eq!(entity.name(), "search");
    }

    #[test]
    fn typed_tool_terminal_classifies_span_outcomes() {
        fn response(
            result: Result<
                golem_common::model::oplog::payload::types::SerializableToolStructuredResult,
                SerializableToolRpcError,
            >,
        ) -> HostResponseEntityInvocation {
            HostResponseEntityInvocation {
                result: Ok(SerializableToolOperationTerminal {
                    body_execution: SerializableEntityBodyExecution::Executed,
                    result,
                    admission_refusal: None,
                }
                .into_typed_schema_value()
                .unwrap()),
            }
        }

        assert_eq!(
            tool_terminal_outcome(&response(Ok(
                golem_common::model::oplog::payload::types::SerializableToolStructuredResult {
                    result: None,
                }
            )))
            .unwrap(),
            SpanOutcome::Completed
        );
        assert_eq!(
            tool_terminal_outcome(&response(Err(SerializableToolRpcError::Denied(
                "no".to_string()
            ))))
            .unwrap(),
            SpanOutcome::Denied
        );
        assert_eq!(
            tool_terminal_outcome(&response(Err(SerializableToolRpcError::ProtocolError(
                "bad".to_string()
            ))))
            .unwrap(),
            SpanOutcome::Failed
        );
        assert_eq!(
            tool_terminal_outcome(&response(Err(SerializableToolRpcError::Cancelled))).unwrap(),
            SpanOutcome::Cancelled
        );
    }

    fn tool_definition() -> Tool {
        Tool {
            version: "1.0.0".to_string(),
            requires_filesystem: false,
            commands: CommandTree { nodes: Vec::new() },
            schema: SchemaGraph::empty(),
        }
    }

    fn middleware_activation(name: &str) -> EntityActivation {
        EntityActivation::new(
            ExecutableTarget::new(
                ComponentId::new(),
                golem_common::model::component::ComponentRevision::try_from(1_u64).unwrap(),
            ),
            DeploymentRevision::try_from(9_u64).unwrap(),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name: ToolMiddlewareName::try_from(name).unwrap(),
                provision: ToolProvisionConfig::default(),
                config_keys_readable: ConfigKeyScope::All,
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
            },
            FilesystemCapability::Incapable,
        )
        .unwrap()
    }

    fn tool_activation() -> EntityActivation {
        let component_id = ComponentId::new();
        let component_revision =
            golem_common::model::component::ComponentRevision::try_from(2_u64).unwrap();
        let deployment_revision = DeploymentRevision::try_from(9_u64).unwrap();
        let source = ToolSource::Component {
            component_id,
            component_revision,
            component_name: golem_common::model::component::ComponentName("tool".to_string()),
        };
        let binding = CompiledToolBinding {
            deployment_revision,
            release_id: None,
            owner: ToolBindingOwner::AgentType {
                agent_type_name: AgentTypeName("Owner".to_string()),
            },
            tool_name: ToolName::try_from("entity").unwrap(),
            version: "1.0.0".to_string(),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Default::default(),
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            config_keys_readable: ConfigKeyScope::All,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
            source,
        };
        EntityActivation::new(
            ExecutableTarget::new(component_id, component_revision),
            deployment_revision,
            EntityActivationPolicy::Tool {
                provision: ToolProvisionConfig::default(),
                binding: Box::new(binding),
                mcp_import: None,
            },
            FilesystemCapability::Incapable,
        )
        .unwrap()
    }

    fn recorded_plan_request(plan: EntityInvocationPlanReference) -> Vec<u8> {
        let owner = invocation().owner_id().clone();
        desert_rust::serialize_to_byte_vec(&EntityInvocationRequest {
            entity: AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap()),
            call_mode: EntityCallMode::Synchronous,
            operation: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
                attempt_ordinal: 1,
                command_path: vec!["run".to_string()],
                args: Vec::new(),
                has_stdin: false,
                has_stdout: false,
                declares_stdout: false,
                has_stderr: false,
                declares_stderr: false,
                output_contract: ToolOutputContract {
                    result: None,
                    errors: Vec::new(),
                },
            }),
            principal: Principal::Agent(AgentPrincipal {
                agent_id: owner.agent_id,
            }),
            plan,
            authority_wallet: Vec::new(),
        })
        .unwrap()
    }

    fn three_layer_plan() -> EntityInvocationPlan {
        EntityInvocationPlan::new(vec![
            EntityInvocationPlanLayer::Middleware {
                activation: middleware_activation("audit"),
                parameters: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::string()),
                    SchemaValue::String("outer".to_string()),
                ),
                expected_definition: None,
                presented_definition: None,
                next_effective_definition: tool_definition(),
                compatibility: None,
            },
            EntityInvocationPlanLayer::Middleware {
                activation: middleware_activation("audit"),
                parameters: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::u64()),
                    SchemaValue::U64(7),
                ),
                expected_definition: None,
                presented_definition: None,
                next_effective_definition: tool_definition(),
                compatibility: None,
            },
            EntityInvocationPlanLayer::Tool {
                activation: tool_activation(),
            },
        ])
        .unwrap()
    }

    #[test]
    fn resolve_recorded_plan_descendant_retains_root_and_advances_chain() {
        let root = OplogIndex::from_u64(42);
        let payload = recorded_plan_request(EntityInvocationPlanReference::Root {
            plan: three_layer_plan(),
        });
        let resolved = resolve_recorded_plan_from_root_payload(
            OplogIndex::from_u64(61),
            &AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap()),
            EntityInvocationPlanReference::Descendant {
                root_start_index: root,
                position: 1,
            },
            Some(&payload),
        )
        .unwrap();

        assert_eq!(resolved.root_start_index(), root);
        assert_eq!(resolved.position(), 1);
        assert_eq!(
            resolved.next_reference(),
            Some(EntityInvocationPlanReference::Descendant {
                root_start_index: root,
                position: 2,
            })
        );
        let EntityInvocationPlanLayer::Middleware { parameters, .. } = resolved.layer() else {
            panic!("position one must be middleware")
        };
        assert_eq!(parameters.value(), &SchemaValue::U64(7));

        let leaf = resolve_recorded_plan_from_root_payload(
            OplogIndex::from_u64(62),
            &AgentEntity::Tool(ToolName::try_from("entity").unwrap()),
            EntityInvocationPlanReference::Descendant {
                root_start_index: root,
                position: 2,
            },
            Some(&payload),
        )
        .unwrap();
        assert_eq!(leaf.root_start_index(), root);
        assert!(leaf.next_reference().is_none());
    }

    #[test]
    fn resolve_recorded_plan_descendant_rejects_invalid_recorded_references() {
        let root = OplogIndex::from_u64(42);
        let valid_payload = recorded_plan_request(EntityInvocationPlanReference::Root {
            plan: three_layer_plan(),
        });
        let middleware =
            AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap());
        for (current, position) in [(42, 1), (41, 1), (61, 0), (61, 3)] {
            assert!(
                resolve_recorded_plan_from_root_payload(
                    OplogIndex::from_u64(current),
                    &middleware,
                    EntityInvocationPlanReference::Descendant {
                        root_start_index: root,
                        position,
                    },
                    Some(&valid_payload),
                )
                .is_err()
            );
        }
        assert!(
            resolve_recorded_plan_from_root_payload(
                OplogIndex::from_u64(61),
                &AgentEntity::Tool(ToolName::try_from("entity").unwrap()),
                EntityInvocationPlanReference::Descendant {
                    root_start_index: root,
                    position: 1,
                },
                Some(&valid_payload),
            )
            .is_err()
        );
        let descendant_payload = recorded_plan_request(EntityInvocationPlanReference::Descendant {
            root_start_index: OplogIndex::from_u64(1),
            position: 1,
        });
        assert!(
            resolve_recorded_plan_from_root_payload(
                OplogIndex::from_u64(61),
                &middleware,
                EntityInvocationPlanReference::Descendant {
                    root_start_index: root,
                    position: 1,
                },
                Some(&descendant_payload),
            )
            .is_err()
        );
        assert!(
            resolve_recorded_plan_from_root_payload(
                OplogIndex::from_u64(61),
                &middleware,
                EntityInvocationPlanReference::Descendant {
                    root_start_index: root,
                    position: 1,
                },
                Some(b"malformed plan"),
            )
            .is_err()
        );
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
                Ok::<_, WorkerExecutorError>(EntityBodyOutcome::Completed(7))
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
                Ok::<_, WorkerExecutorError>(EntityBodyOutcome::<u64>::Cancelled)
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
                Ok::<_, WorkerExecutorError>(EntityBodyOutcome::<u64>::Cancelled)
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
                    Ok::<_, WorkerExecutorError>(EntityBodyOutcome::Completed(9))
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
                Ok::<_, WorkerExecutorError>(EntityBodyOutcome::<u64>::Cancelled)
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
            async { Ok::<_, WorkerExecutorError>(EntityBodyOutcome::Completed(7)) },
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
                Ok::<_, WorkerExecutorError>(EntityBodyOutcome::Completed(7))
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
        for failure in [
            WorkerExecutorError::runtime("body replay failed"),
            WorkerExecutorError::recovery_required_from(
                "storage unavailable",
                OplogIndex::from_u64(5),
            ),
            WorkerExecutorError::InvocationFailed {
                error: golem_common::model::oplog::AgentError::OutOfMemory,
                stderr: String::new(),
            },
        ] {
            let expected = failure.clone();
            let (body_completed, wait_for_body) = oneshot::channel();
            let fence_dropped = Arc::new(AtomicBool::new(false));
            let error = coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async move {
                    let _ = body_completed.send(());
                    Err::<EntityBodyOutcome<u64>, _>(failure)
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
            .expect_err("a replayed terminal must not hide or reclassify body failure");

            assert_eq!(error, expected);
            assert!(fence_dropped.load(Ordering::Acquire));
        }
    }

    #[test]
    async fn completed_body_failure_does_not_wait_for_blocked_replay() {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async {
                    Err::<EntityBodyOutcome<u64>, _>(WorkerExecutorError::runtime(
                        "body replay failed",
                    ))
                },
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
        .expect_err("completed reconstruction must propagate body failure");

        assert!(error.to_string().contains("body replay failed"));
    }

    #[test]
    async fn completed_body_underconsumption_is_structural_divergence() {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async { Ok::<_, WorkerExecutorError>(EntityBodyOutcome::Completed(7)) },
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
            async {
                Err::<EntityBodyOutcome<u64>, _>(WorkerExecutorError::runtime(
                    "live continuation failed",
                ))
            },
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
                Ok::<_, WorkerExecutorError>(EntityBodyOutcome::<u64>::Cancelled)
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

    #[test]
    async fn cancelled_incomplete_replay_preserves_typed_owner_oom() {
        let expected = WorkerExecutorError::InvocationFailed {
            error: golem_common::model::oplog::AgentError::OutOfMemory,
            stderr: String::new(),
        };
        for cancelled in [false, true] {
            let cancellation = tokio_util::sync::CancellationToken::new();
            if cancelled {
                cancellation.cancel();
            }
            let failure = expected.clone();
            let result = coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingIncomplete,
                async move { Err::<EntityBodyOutcome<u64>, _>(failure) },
                async { Ok(EntityReconstructionResolution::<u64, ()>::Incomplete(())) },
                no_structural_stall(),
                || {},
                None::<()>,
                Some(&cancellation),
            )
            .await;

            assert_eq!(
                result.expect_err("typed owner OOM must not become a tool-local cancellation"),
                expected,
                "cancelled={cancelled}"
            );
        }
    }

    #[test]
    async fn abort_join_preserves_late_owner_oom() {
        let expected = WorkerExecutorError::InvocationFailed {
            error: golem_common::model::oplog::AgentError::OutOfMemory,
            stderr: String::new(),
        };
        let failure = expected.clone();
        let (aborted, wait_for_abort) = oneshot::channel();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingIncomplete,
            async move {
                wait_for_abort.await.unwrap();
                Err::<EntityBodyOutcome<u64>, _>(failure)
            },
            async { Ok(EntityReconstructionResolution::<u64, ()>::LiveAdmissionCancelled(())) },
            no_structural_stall(),
            move || {
                let _ = aborted.send(());
            },
            None::<()>,
            None,
        )
        .await;
        assert_eq!(
            result.expect_err("joined owner OOM must survive cancellation"),
            expected
        );
    }

    #[test]
    async fn cancellation_resolution_preserves_a_joined_body_failure() {
        for expected in [
            WorkerExecutorError::InvocationFailed {
                error: golem_common::model::oplog::AgentError::OutOfMemory,
                stderr: String::new(),
            },
            WorkerExecutorError::recovery_required_from(
                "storage unavailable",
                OplogIndex::from_u64(13),
            ),
            WorkerExecutorError::runtime("storage failed"),
        ] {
            for resolution in [
                EntityReconstructionResolution::Cancelled(7_u64),
                EntityReconstructionResolution::Incomplete(()),
                EntityReconstructionResolution::LiveAdmissionCancelled(()),
            ] {
                let cancellation = tokio_util::sync::CancellationToken::new();
                cancellation.cancel();
                let failure = expected.clone();
                let result = coordinate_entity_reconstruction(
                    &invocation(),
                    InvocationExecutionMode::ReplayingIncomplete,
                    async move {
                        // The resolver wins the first poll; joining the body must still retain
                        // its failure rather than publish the selected cancellation.
                        tokio::task::yield_now().await;
                        Err::<EntityBodyOutcome<u64>, _>(failure)
                    },
                    async move { Ok(resolution) },
                    no_structural_stall(),
                    || {},
                    None::<()>,
                    Some(&cancellation),
                )
                .await;
                assert_eq!(
                    result.expect_err("cancellation cannot hide a joined owner failure"),
                    expected
                );
            }
        }
    }
}
