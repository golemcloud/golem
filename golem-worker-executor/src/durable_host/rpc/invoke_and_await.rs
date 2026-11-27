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

//! `wasm-rpc.invoke-and-await`: invokes a remote method and awaits its result as one async
//! subtask. The call's `End` records the result, so replay returns it without contacting the
//! target. The guest can run several such calls at once; each completes on its own.
//!
//! Dropping the subtask records `Cancelled` and, once the call was dispatched, asks the target
//! to withdraw the invocation if it has not started yet. A running invocation keeps running.

use super::{
    CallResolution, CallerStreamsContext, CompletedReplay, DurableStreamingTaskParams, OpenRpcCall,
    PreparedRpcInvocation, RpcCall, RpcCallKind, RpcCallOpening, RpcTask, WasmRpcEntry,
    admit_rpc_result_secret_holds, caller_durable_rpc_streams, deliver_rpc_result, dispatch_target,
    invocation_metadata, invoke_and_await_response_to_wire, open_rpc_call, retry_after_backoff,
    rpc_input_root, rpc_output_root, rpc_span_finished, spawn_invoke_and_await_task,
    spawn_streaming_invoke_and_await_task, suspendable,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    Cancellable, CompletionDelivery, DurableCallSession, TeardownProbe,
};
use crate::durable_host::durable_session::{StreamSession, strip_streams};
use crate::durable_host::durable_stream::SessionError;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::{InvocationResultWithMetadata, RpcError};
use crate::services::rpc::{Rpc, RpcError as InternalRpcError};
use crate::workerctx::WorkerCtx;
use golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleOperation;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::caller::Principal;
use golem_common::model::card::ScopeCard;
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
use golem_common::model::oplog::payload::denied;
use golem_common::model::{AgentFingerprint, OwnedAgentId, OwnedInvocationId};
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::sync::Arc;
use wasmtime::component::{Accessor, HasSelf, Resource};

type InvokeAndAwaitCall = DurableCallSession<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>;

pub(super) async fn invoke_and_await<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: Resource<WasmRpcEntry>,
    method_name: String,
    input: core_wire::SchemaValueTree,
    scope_card: Option<Resource<PermissionCardHandleRep>>,
) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
    match open_rpc_call::<_, _, GolemRpcWasmRpcInvokeAndAwaitResult>(
        accessor,
        &this,
        RpcCallKind::Awaited,
        method_name,
        input,
        scope_card,
    )
    .await?
    {
        RpcCallOpening::Rejected(error) => Ok(Err(error)),
        RpcCallOpening::Denied {
            handle,
            span_id,
            denial,
        } => {
            let outcome = complete_denied(accessor, handle, span_id, denial).await?;
            deliver(accessor, outcome).await
        }
        RpcCallOpening::Open(OpenRpcCall { handle, call }) => {
            let outcome = resolve(accessor, handle, call, &this).await?;
            deliver(accessor, outcome).await
        }
    }
}

/// The recorded result of a call and what its delivery to the guest needs.
struct CallOutcome {
    response: HostResponseGolemRpcInvokeAndAwait,
    delivery: CompletionDelivery,
    /// The ID of the target's invocation, once the call reached a target.
    final_id: Option<OwnedInvocationId>,
    span_id: SpanId,
}

/// Records the live denial as the call's result.
async fn complete_denied<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: InvokeAndAwaitCall,
    span_id: SpanId,
    denial: String,
) -> anyhow::Result<CallOutcome> {
    let response = denied::rpc::invoke_and_await(&(), &denial);
    let finished =
        rpc_span_finished::<()>(&span_id, &Err(InternalRpcError::Denied { details: denial }));
    let (response, delivery) = handle
        .complete_access_deferred_with_span(accessor, accessor.getter(), response, finished)
        .await?;
    Ok(CallOutcome {
        response,
        delivery,
        final_id: None,
        span_id,
    })
}

/// Replays the recorded result of a completed call, or runs the call now.
async fn resolve<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut handle: InvokeAndAwaitCall,
    call: RpcCall,
    this: &Resource<WasmRpcEntry>,
) -> anyhow::Result<CallOutcome> {
    let (scope_card, streaming_origin) = call_lending(accessor, &mut handle, &call).await?;
    match call.replay_deferred(accessor, handle).await? {
        CallResolution::Recorded((response, delivery, completed)) => {
            replay_completed(
                accessor,
                call,
                completed,
                streaming_origin,
                response,
                delivery,
            )
            .await
        }
        CallResolution::Live(handle) => {
            invoke_live(accessor, handle, call, this, scope_card, streaming_origin).await
        }
    }
}

/// The scope card that the call lends and the logical origin of a streaming call. A live call
/// lends the card it was given; a replay or repair reuses what its `Start` recorded.
async fn call_lending<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut InvokeAndAwaitCall,
    call: &RpcCall,
) -> anyhow::Result<(Option<ScopeCard>, Option<StreamInvocationId>)> {
    if call.started_live && !call.prepared.is_streaming() {
        return Ok((call.scope_card.clone(), None));
    }
    let recorded = handle
        .recorded_request_access(accessor, accessor.getter())
        .await
        .map_err(|error| handle.trap(error))?;
    let scope_card = if call.started_live {
        call.scope_card.clone()
    } else {
        recorded.scope_card
    };
    Ok((scope_card, recorded.logical_streaming_origin))
}

/// The outcome of a completed call. Live, every call that was not denied bound and pumped its
/// input streams before its result was recorded, whether that result is a value or an error, so
/// the replay binds them again.
async fn replay_completed<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    call: RpcCall,
    completed: CompletedReplay,
    streaming_origin: Option<StreamInvocationId>,
    mut response: HostResponseGolemRpcInvokeAndAwait,
    delivery: CompletionDelivery,
) -> anyhow::Result<CallOutcome> {
    let denied = completed.denied;
    let fingerprint = completed.claim_preparation(accessor).await?;
    let final_id = fingerprint.map(|fingerprint| call.final_id(fingerprint));
    if call.prepared.is_streaming()
        && !denied
        && let (Some(final_id), Some(fingerprint)) = (&final_id, fingerprint)
    {
        let auth_ctx = with_import_access(accessor, |mut access| access.get().agent_auth_ctx());
        let origin = streaming_origin.ok_or_else(|| missing_streaming_origin(call.start_index))?;
        let streams = open_streams_at(
            accessor,
            &call.remote_agent_id,
            final_id,
            fingerprint,
            origin,
            auth_ctx,
        )
        .await?;
        materialize_input(accessor, &streams, &call.prepared).await?;
        if response.result.is_ok()
            && let Some(value) = streams
                .replay_remote_result()
                .await
                .map_err(SessionError::into_trap)?
        {
            response.result = Ok(value);
        }
    }
    Ok(CallOutcome {
        response,
        delivery,
        final_id,
        span_id: call.span_id,
    })
}

/// Runs the call now: prepares an ephemeral target, dispatches the invocation, awaits its result
/// and records it.
async fn invoke_live<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut handle: InvokeAndAwaitCall,
    call: RpcCall,
    this: &Resource<WasmRpcEntry>,
    scope_card: Option<ScopeCard>,
    streaming_origin: Option<StreamInvocationId>,
) -> anyhow::Result<CallOutcome> {
    let (result, final_id, streams) = match call
        .target_fingerprint(accessor, &mut handle, RpcCallKind::Awaited)
        .await?
    {
        Ok(fingerprint) => {
            let final_id = call.final_id(fingerprint);
            let (result, streams) = dispatch(
                accessor,
                &mut handle,
                &call,
                this,
                fingerprint,
                &final_id,
                scope_card,
                streaming_origin,
            )
            .await?;
            (result, Some(final_id), streams)
        }
        Err(error) => (Err(error), None, None),
    };
    let finished = rpc_span_finished(&call.span_id, &result);
    let (mut response, delivery) = handle
        .complete_access_deferred_with_span(
            accessor,
            accessor.getter(),
            HostResponseGolemRpcInvokeAndAwait {
                result: result.map(strip_streams).map_err(Into::into),
            },
            finished,
        )
        .await?;
    if let Some(streams) = &streams
        && response.result.is_ok()
        && let Some(value) = streams
            .replay_remote_result()
            .await
            .map_err(SessionError::into_trap)?
    {
        response.result = Ok(value);
    }
    Ok(CallOutcome {
        response,
        delivery,
        final_id,
        span_id: call.span_id,
    })
}

/// Dispatches the invocation from a background task and awaits its result, with the input
/// streams of a streaming call bound. Returns the result and the call's stream session.
#[allow(clippy::too_many_arguments)]
async fn dispatch<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut InvokeAndAwaitCall,
    call: &RpcCall,
    this: &Resource<WasmRpcEntry>,
    fingerprint: AgentFingerprint,
    final_id: &OwnedInvocationId,
    scope_card: Option<ScopeCard>,
    streaming_origin: Option<StreamInvocationId>,
) -> anyhow::Result<(Result<SchemaValue, InternalRpcError>, Option<StreamSession>)> {
    // A client reconstructed after a restart activates its recorded incarnation inside each
    // dispatch task, as before the restart, and creates nothing.
    let (deferred_activation, expected_callee_fingerprint) =
        with_import_access(accessor, |mut access| {
            let activation = &access.get().table().get(this)?.payload.target_activation;
            anyhow::Ok((
                activation.deferred_activation(),
                activation.target_fingerprint(),
            ))
        })
        .map_err(|error| handle.trap(error))?;
    let auth_ctx = handle.agent_auth_ctx().clone();
    let streaming = streaming_task_params(
        accessor,
        handle,
        call,
        fingerprint,
        final_id,
        streaming_origin,
        &auth_ctx,
    )
    .await?;

    let prepared = &call.prepared;
    let remote_target = dispatch_target(&call.remote_agent_id, prepared.ephemeral.as_ref());
    let retry_properties = prepared.retry_properties("invoke-and-await", &call.remote_agent_id);
    let spawn = |handle: &InvokeAndAwaitCall| {
        with_import_access(accessor, |mut access| {
            let ctx = access.get();
            match &streaming {
                Some(params) => spawn_streaming_invoke_and_await_task(
                    ctx,
                    remote_target.clone(),
                    call.idempotency_key.clone(),
                    prepared.method_name.clone(),
                    params.clone(),
                    prepared.env.clone(),
                    prepared.config.clone(),
                    deferred_activation.clone(),
                    &call.span_id,
                    scope_card.clone(),
                    auth_ctx.clone(),
                ),
                None => spawn_invoke_and_await_task(
                    ctx,
                    &remote_target,
                    call.idempotency_key.clone(),
                    prepared.method_name.clone(),
                    prepared.input_value.clone(),
                    prepared.env.clone(),
                    prepared.config.clone(),
                    deferred_activation.clone(),
                    expected_callee_fingerprint,
                    retry_properties.clone(),
                    &call.span_id,
                    handle.trap_context().retry_from,
                    handle.start_index(),
                    InvocationFreshnessDisposition::MayExist,
                    scope_card.clone(),
                    auth_ctx.clone(),
                ),
            }
        })
    };

    let streams = streaming.as_ref().map(|params| params.streams.clone());
    let mut withdrawal = PendingWithdrawal::arm(
        accessor,
        handle,
        final_id.clone(),
        auth_ctx.clone(),
        streams.clone(),
    );
    let result = await_task(
        accessor,
        handle,
        spawn,
        streaming.is_none().then(|| retry_properties.clone()),
    )
    .await;
    withdrawal.disarm();
    let result = admit_rpc_result_secret_holds(accessor, result?)
        .await
        .map_err(|error| handle.trap(error))?;
    Ok((result, streams))
}

/// Opens the stream session of a call whose `Start` recorded a streaming origin and binds its
/// input streams, for the task that dispatches it. A streaming method without a recorded origin
/// traps.
async fn streaming_task_params<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut InvokeAndAwaitCall,
    call: &RpcCall,
    fingerprint: AgentFingerprint,
    final_id: &OwnedInvocationId,
    streaming_origin: Option<StreamInvocationId>,
    auth_ctx: &AuthCtx,
) -> anyhow::Result<Option<DurableStreamingTaskParams>> {
    let origin = match streaming_origin {
        Some(origin) => origin,
        None if call.prepared.is_streaming() => {
            return Err(handle.trap(missing_streaming_origin(call.start_index)));
        }
        None => return Ok(None),
    };
    let streams = open_streams_at(
        accessor,
        &call.remote_agent_id,
        final_id,
        fingerprint,
        origin.clone(),
        auth_ctx.clone(),
    )
    .await
    .map_err(|error| handle.trap(error))?;
    let input = materialize_input(accessor, &streams, &call.prepared)
        .await
        .map_err(|error| handle.trap(error))?;
    let attempt_id = streams
        .caller_attempt_id()
        .await
        .map_err(|error| handle.trap(error.into_trap()))?;
    Ok(Some(DurableStreamingTaskParams {
        streams,
        input_mappings: input.proto_mappings(),
        input: input.value,
        expected_callee_fingerprint: fingerprint,
        attempt_id: attempt_id.0,
        origin_invocation: origin_invocation_identity(origin),
        output_graph: Arc::new(call.prepared.remote_agent_type.schema.clone()),
        output_root: rpc_output_root(&call.prepared),
    }))
}

/// Awaits the call's RPC task. The wait lets the agent suspend while the call is long, and an
/// interrupt of the agent traps the call. A hard failure of the task, after its in-task retries,
/// also traps: the `Start` stays incomplete for repair. With `retry_properties`, a transient
/// failure whose backoff can be waited durably sends the call again with its key.
async fn await_task<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut InvokeAndAwaitCall,
    spawn: impl Fn(&InvokeAndAwaitCall) -> RpcTask,
    retry_properties: Option<golem_common::model::RetryProperties>,
) -> anyhow::Result<Result<SchemaValue, InternalRpcError>> {
    loop {
        let mut task = spawn(handle);
        let result = match suspendable(accessor, &mut task).await {
            Ok(result) => result.map_err(|error| handle.trap(error))?,
            Err(interrupt) => {
                drop(task);
                return Err(handle.trap(interrupt));
            }
        };
        let retry = match &retry_properties {
            Some(properties) => {
                retry_after_backoff(accessor, handle, properties.clone(), &result).await?
            }
            None => false,
        };
        if !retry {
            return Ok(result);
        }
    }
}

fn missing_streaming_origin(start_index: OplogIndex) -> WorkerExecutorError {
    WorkerExecutorError::unexpected_oplog_entry(
        start_index,
        "streaming RPC Start with logical origin",
        format!("streaming RPC Start at {start_index} without logical origin"),
    )
}

/// Finishes the call's span and returns the recorded result to the guest.
async fn deliver<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    outcome: CallOutcome,
) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
    let CallOutcome {
        response,
        delivery,
        final_id,
        span_id,
    } = outcome;
    deliver_rpc_result(accessor, delivery, Some(&span_id), |ctx| {
        Ok(
            match invoke_and_await_response_to_wire(response.result, ctx)? {
                Ok(result) => {
                    let final_id = final_id.ok_or_else(|| {
                        anyhow::anyhow!("a successful RPC result has no final invocation identity")
                    })?;
                    Ok(InvocationResultWithMetadata {
                        metadata: invocation_metadata(&final_id)?,
                        result,
                    })
                }
                Err(error) => Err(error),
            },
        )
    })
    .await
}

/// Opens the caller's stream session of a completed streaming call, whose `Start` recorded the
/// logical streaming origin.
async fn open_streams_at<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    remote_agent_id: &OwnedAgentId,
    final_id: &OwnedInvocationId,
    fingerprint: AgentFingerprint,
    origin: StreamInvocationId,
    auth_ctx: AuthCtx,
) -> anyhow::Result<StreamSession> {
    let context = with_import_access(accessor, |mut access| {
        CallerStreamsContext::capture(access.get())
    })?;
    let (streams, _) = caller_durable_rpc_streams(
        context,
        remote_agent_id,
        fingerprint,
        final_id.id(),
        origin,
        auth_ctx,
    )
    .await?;
    Ok(streams)
}

fn origin_invocation_identity(
    origin: StreamInvocationId,
) -> golem_api_grpc::proto::golem::worker::StreamInvocationIdentity {
    golem_api_grpc::proto::golem::worker::StreamInvocationIdentity {
        callee_environment_id: Some(origin.callee_environment_id.into()),
        callee: Some(origin.callee.into()),
        callee_fingerprint: Some(origin.callee_fingerprint.0.into()),
        idempotency_key: Some(origin.idempotency_key.into()),
    }
}

async fn materialize_input<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    streams: &StreamSession,
    prepared: &PreparedRpcInvocation,
) -> anyhow::Result<crate::durable_host::durable_session::SessionValue> {
    let caller_revision = with_import_access(accessor, |mut access| {
        access.get().owner_component_metadata().revision
    });
    streams
        .materialize_agent_input(
            &prepared.input_value,
            &prepared.remote_agent_type.schema,
            &rpc_input_root(prepared),
            caller_revision,
        )
        .await
        .map_err(SessionError::into_trap)
}

/// Asks the target to withdraw a dispatched invocation when the guest drops the awaiting
/// subtask before its result arrived. The request is best effort and unrecorded: the dropped
/// call's durable answer is its `Cancelled` entry, and a target that already started the
/// invocation keeps running it.
struct PendingWithdrawal {
    armed: Option<Withdrawal>,
}

struct Withdrawal {
    rpc: Arc<dyn Rpc>,
    invocation_id: OwnedInvocationId,
    caller: Principal,
    auth_ctx: AuthCtx,
    streams: Option<StreamSession>,
    teardown: TeardownProbe,
}

impl PendingWithdrawal {
    fn arm<U: Send + 'static, Ctx: WorkerCtx>(
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        handle: &InvokeAndAwaitCall,
        invocation_id: OwnedInvocationId,
        auth_ctx: AuthCtx,
        streams: Option<StreamSession>,
    ) -> Self {
        let (rpc, caller) = with_import_access(accessor, |mut access| {
            let ctx = access.get();
            (ctx.rpc(), ctx.outgoing_caller().immediate)
        });
        Self {
            armed: Some(Withdrawal {
                rpc,
                invocation_id,
                caller,
                auth_ctx,
                streams,
                teardown: handle.teardown_probe(),
            }),
        }
    }

    fn disarm(&mut self) {
        self.armed = None;
    }
}

impl Drop for PendingWithdrawal {
    fn drop(&mut self) {
        let Some(withdrawal) = self.armed.take() else {
            return;
        };
        // An executor or Store shutting down does not drop the call: its `Start` stays
        // incomplete and recovery repairs it.
        if withdrawal.teardown.is_tearing_down() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            if let Some(streams) = &withdrawal.streams
                && let Err(error) = streams.cancel_unbound_rpc_inputs(&[]).await
            {
                tracing::info!(%error, "Best-effort cancellation of unbound RPC inputs failed");
            }
            if let Err(error) = withdrawal
                .rpc
                .invocation_handle(
                    &withdrawal.invocation_id,
                    &withdrawal.caller,
                    InvocationHandleOperation::Cancel,
                    &withdrawal.auth_ctx,
                )
                .await
            {
                tracing::info!(%error, "Best-effort withdrawal of a dropped RPC call failed");
            }
        });
    }
}
