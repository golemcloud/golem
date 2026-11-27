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
    PreparedRpcInvocation, RpcCallTarget, WasmRpcEntry, current_streaming_origin,
    discard_owned_rpc_input, embedded_invocation_span_builder, prepare_rpc_invocation,
    recorded_target_fingerprint, restore_embedded_span,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, Cancellable, DurableCallSession,
    reserve_invocation_id_before_await,
};
use crate::durable_host::permissions::resolve_invocation_scope_card;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::RpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::card::ScopeCard;
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::{DurableFunctionType, HostPayloadPair, HostRequestGolemRpcInvoke};
use golem_common::model::{AgentFingerprint, IdempotencyKey, OwnedAgentId};
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
    /// The call is open, live or replaying.
    Open(OpenRpcCall<Pair>),
}

/// An RPC call whose `Start` is appended or claimed.
pub(super) struct OpenRpcCall<Pair: HostPayloadPair> {
    pub handle: DurableCallSession<Pair, Cancellable>,
    pub prepared: PreparedRpcInvocation,
    pub span_id: SpanId,
    pub idempotency_key: IdempotencyKey,
    /// The agent that the call dispatches to.
    pub remote_agent_id: OwnedAgentId,
    /// Whether the call appended its `Start` now, rather than claiming a recorded one.
    pub started_live: bool,
    /// The fingerprint of the target that the client knows: none for an ephemeral target.
    pub known_fingerprint: Option<AgentFingerprint>,
    /// The scope card that a live call lends. A replay or repair reads the recorded one.
    pub scope_card: Option<ScopeCard>,
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
    // The live decision holds its authorization permit until the call completes.
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
            RpcCallOpening::Open(OpenRpcCall {
                handle,
                prepared,
                span_id,
                idempotency_key,
                remote_agent_id,
                started_live,
                known_fingerprint,
                scope_card,
            })
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
