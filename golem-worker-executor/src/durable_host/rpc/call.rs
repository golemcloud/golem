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

//! The opening that `invoke-and-await` and `trigger` share. It validates the request, starts
//! the durable call under the target's authorization, and restores the call's span. A replay
//! or repair claims the recorded `Start` instead.

use super::denial::denied_rpc_request;
use super::{
    PreparedRpcInvocation, RpcCallTarget, WasmRpcEntry, claim_completed_preparation,
    current_streaming_origin, discard_owned_rpc_input, dispatch_target,
    embedded_invocation_span_builder, known_fresh_dispatch_allowed, preparation_request,
    prepare_ephemeral_identity, prepare_rpc_invocation, recorded_target_fingerprint,
    restore_embedded_span,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, CallReplayOutcome, Cancellable, CompletionDelivery,
    DeferredCallReplayOutcome, DurableCallSession, HostInternalCallOwner,
    reserve_invocation_id_before_await,
};
use crate::durable_host::permissions::resolve_invocation_scope_card;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::worker::target::ResolvedTarget;
use crate::workerctx::WorkerCtx;
use golem_common::model::RetryProperties;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::card::ScopeCard;
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestGolemRpcInvoke,
    HostRequestGolemRpcPrepareInvocation, OplogIndex,
};
use golem_common::model::{
    AgentFingerprint, IdempotencyKey, InvocationRequestKey, OwnedAgentId, OwnedInvocationId,
};
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::{Accessor, HasSelf, Resource};

/// The host function that makes an RPC call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RpcCallKind {
    /// `invoke-and-await`: a synchronous call that can use live streams.
    Awaited,
    /// `trigger`: an asynchronous call that returns the invocation's handle.
    Triggered,
    /// `schedule`: an asynchronous call for a given time that returns the invocation's handle.
    /// It may target the calling agent.
    Scheduled,
}

impl RpcCallKind {
    pub(super) fn verb(self) -> &'static str {
        match self {
            Self::Awaited => "invoke-and-await",
            Self::Triggered => "trigger",
            Self::Scheduled => "schedule",
        }
    }

    pub(super) fn host_function_name(self) -> &'static str {
        match self {
            Self::Awaited => "golem::rpc::wasm-rpc::invoke-and-await",
            Self::Triggered => "golem::rpc::wasm-rpc::trigger",
            Self::Scheduled => "golem::rpc::wasm-rpc::schedule",
        }
    }
}

/// How the opening of an RPC call ended.
///
/// Transient: the host function matches on it at once, so the size difference between the
/// variants never outlives the opening.
#[allow(clippy::large_enum_variant)]
pub(super) enum RpcCallOpening<Pair: HostPayloadPair> {
    /// The request is invalid. The call has no `Start`.
    Rejected(RpcError),
    /// The live permission check refused the call. The guest gets the denial as the call's
    /// result.
    Denied {
        handle: DurableCallSession<Pair, Cancellable>,
        span_id: SpanId,
        denial: String,
    },
    /// The call's `Start` is appended or claimed: its durable call session and what the call
    /// resolved when it opened. The call is live or replaying.
    Open {
        handle: DurableCallSession<Pair, Cancellable>,
        call: RpcCall,
    },
}

/// What an open RPC call resolved: the validated request, its span, and the target that its
/// `Start` index derives.
pub(super) struct RpcCall {
    prepared: PreparedRpcInvocation,
    span_id: SpanId,
    start_index: OplogIndex,
    idempotency_key: IdempotencyKey,
    /// The agent that the call dispatches to.
    remote_agent_id: OwnedAgentId,
    /// Whether the call appended its `Start` now, rather than claiming a recorded one.
    started_live: bool,
    /// The fingerprint of the target that the client knows: none for an ephemeral target.
    known_fingerprint: Option<AgentFingerprint>,
    /// The scope card that a live call lends. A replay or repair reads the recorded one.
    scope_card: Option<ScopeCard>,
}

/// What the call's synchronous step validated and captured, before its first await.
struct RpcCallRequest<SpanBuilder> {
    prepared: PreparedRpcInvocation,
    target: golem_common::model::card::PermissionTarget,
    span_started: SpanBuilder,
    call_target: RpcCallTarget,
    scope_card: Option<ScopeCard>,
    streaming_origin: Option<StreamInvocationId>,
}

pub(super) async fn open_rpc_call<U, Ctx, Pair>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: &Resource<WasmRpcEntry>,
    kind: RpcCallKind,
    method_name: String,
    input: core_wire::SchemaValueTree,
    scope_card: Option<Resource<PermissionCardHandleRep>>,
) -> anyhow::Result<RpcCallOpening<Pair>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Req = HostRequestGolemRpcInvoke>,
{
    reserve_invocation_id_before_await(accessor, accessor.getter())?;
    let request = accessor.with(|mut access| {
        rpc_call_request(access.get(), this, kind, method_name, input, scope_card)
    })?;
    let request = match request {
        Ok(request) => request,
        Err(error) => return Ok(RpcCallOpening::Rejected(error)),
    };
    let RpcCallRequest {
        prepared,
        target,
        span_started,
        call_target,
        scope_card,
        streaming_origin,
    } = request;

    let (permission_check, decision) = AccessPermissionCheck::new(Ok(vec![target]));
    // A tool body that returned before this call finished parks the call's replay; its recorded
    // preparation child is then claimed as an abandoned subtree.
    let parks_incomplete_replay = prepared.is_ephemeral()
        && accessor.with(|mut access| access.get().parks_incomplete_tool_guest_replay());
    let request_prepared = prepared.clone();
    let request_target = call_target.clone();
    let request_scope_card = scope_card.clone();
    let mut handle =
        DurableCallSession::<Pair, Cancellable>::start_access_with_agent_authority_and_indexed_request(
            accessor,
            accessor.getter(),
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                permission_check: Some(permission_check),
                host_internal_calls: prepared.is_ephemeral(),
                abandon_incomplete_host_internal_calls: parks_incomplete_replay,
                ..Default::default()
            },
            move |start_index, denied| {
                let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
                let started = span_started(&idempotency_key);
                let request = if denied {
                    denied_rpc_request(
                        &remote_agent_id,
                        &idempotency_key,
                        request_prepared.method_name.clone(),
                    )
                } else {
                    request_prepared.invoke_request(
                        &remote_agent_id,
                        &idempotency_key,
                        request_scope_card.as_ref(),
                        streaming_origin,
                    )
                };
                Ok((request, Some(started)))
            },
        )
        .await?;
    let decision = decision.await.ok().flatten();
    let started_live = handle.is_live();
    let span_id = restore_recorded_span(accessor, &mut handle, kind).await?;

    Ok(match decision {
        Some(Err(denial)) if started_live => RpcCallOpening::Denied {
            handle,
            span_id,
            denial,
        },
        _ => {
            let (idempotency_key, remote_agent_id) = call_target
                .at(handle.start_index())
                .map_err(|error| handle.trap(error))?;
            let known_fingerprint =
                recorded_target_fingerprint(accessor, this).map_err(|error| handle.trap(error))?;
            let start_index = handle.start_index();
            RpcCallOpening::Open {
                handle,
                call: RpcCall {
                    prepared,
                    span_id,
                    start_index,
                    idempotency_key,
                    remote_agent_id,
                    started_live,
                    known_fingerprint,
                    scope_card,
                },
            }
        }
    })
}

/// Validates the request and captures what the call needs after its first await. An invalid
/// request is the guest's error; the call then has no `Start`.
fn rpc_call_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: &Resource<WasmRpcEntry>,
    kind: RpcCallKind,
    method_name: String,
    input: core_wire::SchemaValueTree,
    scope_card: Option<Resource<PermissionCardHandleRep>>,
) -> anyhow::Result<
    Result<
        RpcCallRequest<
            impl Fn(&IdempotencyKey) -> golem_common::model::oplog::SpanStarted
            + Send
            + Sync
            + 'static
            + use<Ctx>,
        >,
        RpcError,
    >,
> {
    let scope_card = match scope_card {
        Some(handle) => match resolve_invocation_scope_card(ctx, &handle) {
            Ok(card) => Some(card),
            Err(details) => {
                discard_owned_rpc_input(input, ctx);
                return Ok(Err(RpcError::Denied(details)));
            }
        },
        None => None,
    };
    let prepared = match prepare_rpc_invocation(ctx, this, kind, method_name, input)? {
        Ok(prepared) => prepared,
        Err(error) => return Ok(Err(error)),
    };
    let target = ctx.outbound_agent_method_target(
        this,
        &prepared.logical_remote_agent_id,
        &prepared.method_name,
    )?;
    ctx.record_outbound_rpc_call()?;
    let span_started =
        embedded_invocation_span_builder(ctx, &prepared.connection_span_id, &prepared.method_name)?;
    let call_target = RpcCallTarget::new(
        ctx.idempotency_key_position(None),
        &prepared.logical_remote_agent_id,
        prepared.ephemeral.as_ref(),
        &prepared.method_name,
    );
    let streaming_origin = if prepared.is_streaming() {
        let parent_key = ctx.state.get_current_idempotency_key().ok_or_else(|| {
            anyhow::anyhow!("durable streaming RPC requires a caller invocation key")
        })?;
        Some(current_streaming_origin(ctx, parent_key))
    } else {
        None
    };
    Ok(Ok(RpcCallRequest {
        prepared,
        target,
        span_started,
        call_target,
        scope_card,
        streaming_origin,
    }))
}

/// Restores the span that the call's `Start` recorded, and closes it if the guest drops the
/// call.
async fn restore_recorded_span<U, Ctx, Pair>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<Pair, Cancellable>,
    kind: RpcCallKind,
) -> anyhow::Result<SpanId>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
{
    let started = handle
        .recorded_span_started_access(accessor, accessor.getter())
        .await
        .map_err(|error| handle.trap(error))?
        .ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                handle.start_index(),
                format!("RPC {} Start carrying span_started", kind.verb()),
                format!(
                    "RPC {} Start at {} without span metadata",
                    kind.verb(),
                    handle.start_index()
                ),
            )
        })
        .map_err(|error| handle.trap(error))?;
    let span = with_import_access(accessor, |mut access| {
        restore_embedded_span(access.get(), &started)
    })
    .map_err(|error| handle.trap(error))?;
    let span_id = span.span_id().clone();
    handle.close_span_on_cancellation(span_id.clone());
    Ok(span_id)
}

/// How a durable call resolved once its `Start` is appended or claimed.
///
/// Transient: callers match on it at once, as on [`CallReplayOutcome`].
#[allow(clippy::large_enum_variant)]
pub(super) enum CallResolution<Pair: HostPayloadPair, Recorded> {
    /// The call completed in the recorded run: its recorded result.
    Recorded(Recorded),
    /// The call runs now: a first execution, or the repair of an incomplete `Start`.
    Live(DurableCallSession<Pair, Cancellable>),
}

/// Replays the recorded result of a completed call. A call that started now, or whose `Start`
/// has no `End`, continues live.
pub(super) async fn replay_call<U, Ctx, Pair>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: DurableCallSession<Pair, Cancellable>,
) -> anyhow::Result<CallResolution<Pair, Pair::Resp>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
{
    if handle.is_live() {
        return Ok(CallResolution::Live(handle));
    }
    Ok(
        match handle.replay_access(accessor, accessor.getter()).await? {
            CallReplayOutcome::Replayed(response) => CallResolution::Recorded(response),
            CallReplayOutcome::Incomplete(live) => CallResolution::Live(live),
        },
    )
}

/// [`replay_call`] for a call whose result reaches the guest at a recorded delivery boundary.
pub(super) async fn replay_call_deferred<U, Ctx, Pair>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: DurableCallSession<Pair, Cancellable>,
) -> anyhow::Result<CallResolution<Pair, (Pair::Resp, CompletionDelivery)>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
{
    if handle.is_live() {
        return Ok(CallResolution::Live(handle));
    }
    Ok(
        match handle
            .replay_access_deferred(accessor, accessor.getter())
            .await?
        {
            DeferredCallReplayOutcome::Replayed(response, delivery) => {
                CallResolution::Recorded((response, delivery))
            }
            DeferredCallReplayOutcome::Incomplete(live) => CallResolution::Live(live),
        },
    )
}

/// What the replay of a completed call claims after the call's `End`: the recorded preparation
/// of an ephemeral target. A call refused at its `Start` prepared nothing.
pub(super) struct CompletedReplay {
    denied: bool,
    preparation: Option<(HostInternalCallOwner, HostRequestGolemRpcPrepareInvocation)>,
    known_fingerprint: Option<AgentFingerprint>,
}

impl CompletedReplay {
    /// Whether the live permission check refused the call at its `Start`.
    pub(super) fn is_denied(&self) -> bool {
        self.denied
    }

    /// Claims the recorded preparation and returns the fingerprint of the target that the call
    /// reached, if it reached one. A completed call repeats no preparation.
    pub(super) async fn claim_preparation<U: Send + 'static, Ctx: WorkerCtx>(
        self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> anyhow::Result<Option<AgentFingerprint>> {
        Ok(match self.preparation {
            Some((owner, request)) => claim_completed_preparation(accessor, owner, request)
                .await?
                .result
                .ok(),
            None => self.known_fingerprint,
        })
    }
}

impl RpcCall {
    pub(super) fn prepared(&self) -> &PreparedRpcInvocation {
        &self.prepared
    }

    pub(super) fn span_id(&self) -> &SpanId {
        &self.span_id
    }

    pub(super) fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// The agent that the call dispatches to.
    pub(super) fn remote_agent_id(&self) -> &OwnedAgentId {
        &self.remote_agent_id
    }

    /// The target that the executor dispatches the call to.
    pub(super) fn dispatch_target(&self) -> ResolvedTarget {
        dispatch_target(&self.remote_agent_id, self.prepared.ephemeral.as_ref())
    }

    /// How the call retries a transient failure of its dispatch.
    pub(super) fn retry_properties(&self, kind: RpcCallKind) -> RetryProperties {
        self.prepared
            .retry_properties(kind.verb(), &self.remote_agent_id)
    }

    /// The scope card that the call lends and the logical origin of a streaming call. A live
    /// call lends the card it was given; a replay or repair reuses what its `Start` recorded.
    pub(super) async fn lending<U, Ctx, Pair>(
        &self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        handle: &mut DurableCallSession<Pair, Cancellable>,
    ) -> anyhow::Result<(Option<ScopeCard>, Option<StreamInvocationId>)>
    where
        U: Send + 'static,
        Ctx: WorkerCtx,
        Pair: HostPayloadPair<Req = HostRequestGolemRpcInvoke>,
    {
        if self.started_live && !self.prepared.is_streaming() {
            return Ok((self.scope_card.clone(), None));
        }
        let recorded = handle
            .recorded_request_access(accessor, accessor.getter())
            .await
            .map_err(|error| handle.trap(error))?;
        let scope_card = if self.started_live {
            self.scope_card.clone()
        } else {
            recorded.scope_card
        };
        Ok((scope_card, recorded.logical_streaming_origin))
    }

    /// The error for a streaming call whose `Start` recorded no logical origin.
    pub(super) fn missing_streaming_origin(&self) -> WorkerExecutorError {
        let start_index = self.start_index;
        WorkerExecutorError::unexpected_oplog_entry(
            start_index,
            "streaming RPC Start with logical origin",
            format!("streaming RPC Start at {start_index} without logical origin"),
        )
    }

    /// The ID of the target's invocation, once the target's fingerprint is known.
    pub(super) fn final_id(&self, fingerprint: AgentFingerprint) -> OwnedInvocationId {
        InvocationRequestKey::Internal(self.idempotency_key.clone()).finalize(
            self.remote_agent_id.agent_id(),
            fingerprint,
            &self.prepared.method_name,
        )
    }

    /// Replays a completed call and returns its recorded response, with what its replay claims
    /// after the response. A call that started now, or whose `Start` has no `End`, continues
    /// live.
    pub(super) async fn replay<U, Ctx, Pair>(
        &self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        handle: DurableCallSession<Pair, Cancellable>,
    ) -> anyhow::Result<CallResolution<Pair, (Pair::Resp, CompletedReplay)>>
    where
        U: Send + 'static,
        Ctx: WorkerCtx,
        Pair: HostPayloadPair,
    {
        if self.started_live {
            return Ok(CallResolution::Live(handle));
        }
        let (completed, handle) = self.completed_replay(accessor, handle).await?;
        Ok(match replay_call(accessor, handle).await? {
            CallResolution::Recorded(response) => CallResolution::Recorded((response, completed)),
            CallResolution::Live(handle) => CallResolution::Live(handle),
        })
    }

    /// [`Self::replay`] for a call whose result reaches the guest at a recorded delivery
    /// boundary.
    pub(super) async fn replay_deferred<U, Ctx, Pair>(
        &self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        handle: DurableCallSession<Pair, Cancellable>,
    ) -> anyhow::Result<CallResolution<Pair, (Pair::Resp, CompletionDelivery, CompletedReplay)>>
    where
        U: Send + 'static,
        Ctx: WorkerCtx,
        Pair: HostPayloadPair,
    {
        if self.started_live {
            return Ok(CallResolution::Live(handle));
        }
        let (completed, handle) = self.completed_replay(accessor, handle).await?;
        Ok(match replay_call_deferred(accessor, handle).await? {
            CallResolution::Recorded((response, delivery)) => {
                CallResolution::Recorded((response, delivery, completed))
            }
            CallResolution::Live(handle) => CallResolution::Live(handle),
        })
    }

    /// Reads what the replay of this call claims once it replays the call's `End`.
    async fn completed_replay<U, Ctx, Pair>(
        &self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        mut handle: DurableCallSession<Pair, Cancellable>,
    ) -> anyhow::Result<(CompletedReplay, DurableCallSession<Pair, Cancellable>)>
    where
        U: Send + 'static,
        Ctx: WorkerCtx,
        Pair: HostPayloadPair,
    {
        let denied = handle
            .recorded_denial_access(accessor, accessor.getter())
            .await
            .map_err(|error| handle.trap(error))?;
        let preparation = (self.prepared.is_ephemeral() && !denied).then(|| {
            (
                handle.host_internal_owner(),
                preparation_request(
                    self.start_index,
                    &self.prepared,
                    &self.remote_agent_id,
                    &self.idempotency_key,
                ),
            )
        });
        let completed = CompletedReplay {
            denied,
            preparation,
            known_fingerprint: self.known_fingerprint,
        };
        Ok((completed, handle))
    }

    /// The fingerprint of the target that a live call dispatches to: the known one, or the one
    /// of the ephemeral instance that the call prepares now. Only the first live dispatch to an
    /// ephemeral target may create it fresh.
    pub(super) async fn target_fingerprint<U, Ctx, Pair>(
        &self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        handle: &mut DurableCallSession<Pair, Cancellable>,
        kind: RpcCallKind,
    ) -> anyhow::Result<Result<AgentFingerprint, InternalRpcError>>
    where
        U: Send + 'static,
        Ctx: WorkerCtx,
        Pair: HostPayloadPair,
    {
        match self.known_fingerprint {
            Some(fingerprint) => Ok(Ok(fingerprint)),
            None => {
                let freshness = if known_fresh_dispatch_allowed(
                    self.started_live,
                    self.prepared.is_ephemeral(),
                ) {
                    InvocationFreshnessDisposition::KnownFresh
                } else {
                    InvocationFreshnessDisposition::MayExist
                };
                prepare_ephemeral_identity(
                    accessor,
                    handle,
                    &self.prepared,
                    &self.remote_agent_id,
                    &self.idempotency_key,
                    &self.span_id,
                    freshness,
                    kind.verb(),
                )
                .await
            }
        }
    }
}
