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
    CallerStreamsContext, DurableStreamingTaskParams, OpenRpcCall, PreparedRpcInvocation,
    RpcCallKind, RpcCallOpening, WasmRpcEntry, admit_rpc_result_secret_holds,
    caller_durable_rpc_streams, claim_completed_preparation, deliver_rpc_result, dispatch_target,
    invocation_metadata, invoke_and_await_response_to_wire, known_fresh_dispatch_allowed,
    open_rpc_call, preparation_request, prepare_ephemeral_identity, retry_after_backoff,
    rpc_input_root, rpc_output_root, rpc_span_finished, spawn_invoke_and_await_task,
    spawn_streaming_invoke_and_await_task, suspendable,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    Cancellable, CompletionDelivery, DeferredCallReplayOutcome, DurableCallSession, TeardownProbe,
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
use golem_common::model::durable_stream::StreamInvocationId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
use golem_common::model::oplog::payload::denied;
use golem_common::model::{
    AgentFingerprint, InvocationRequestKey, OwnedAgentId, OwnedInvocationId,
};
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
    let call = match open_rpc_call::<_, _, GolemRpcWasmRpcInvokeAndAwaitResult>(
        accessor,
        &this,
        RpcCallKind::Awaited,
        method_name,
        input,
        scope_card,
    )
    .await?
    {
        RpcCallOpening::Rejected(error) => return Ok(Err(error)),
        RpcCallOpening::Denied {
            handle,
            span_id,
            denial,
        } => {
            let response = denied::rpc::invoke_and_await(&(), &denial);
            let finished = rpc_span_finished::<()>(
                &span_id,
                &Err(InternalRpcError::Denied { details: denial }),
            );
            let (response, delivery) = handle
                .complete_access_deferred_with_span(accessor, accessor.getter(), response, finished)
                .await?;
            return deliver(accessor, response, None, delivery, &span_id).await;
        }
        RpcCallOpening::Open(call) => call,
    };
    let OpenRpcCall {
        mut handle,
        prepared,
        span_id,
        idempotency_key,
        remote_agent_id,
        started_live,
        known_fingerprint,
        scope_card,
        ..
    } = call;
    let request_key = InvocationRequestKey::Internal(idempotency_key.clone());

    // A replay or repair reuses the scope card and streaming origin its `Start` recorded.
    let (scope_card, streaming_origin) = if started_live {
        let origin = if prepared.is_streaming() {
            let recorded = handle
                .recorded_request_access(accessor, accessor.getter())
                .await;
            recorded
                .map_err(|error| handle.trap(error))?
                .logical_streaming_origin
        } else {
            None
        };
        (scope_card, origin)
    } else {
        let recorded = handle
            .recorded_request_access(accessor, accessor.getter())
            .await
            .map_err(|error| handle.trap(error))?;
        (recorded.scope_card, recorded.logical_streaming_origin)
    };

    let start_index = handle.start_index();
    if !started_live {
        // A completed call repeats no preparation; its replay claims the recorded one.
        let denied = handle
            .recorded_denial_access(accessor, accessor.getter())
            .await
            .map_err(|error| handle.trap(error))?;
        let recorded_preparation = if prepared.is_ephemeral() && !denied {
            Some((
                handle.host_internal_owner(),
                preparation_request(
                    handle.start_index(),
                    &prepared,
                    &remote_agent_id,
                    &idempotency_key,
                ),
            ))
        } else {
            None
        };
        match handle
            .replay_access_deferred(accessor, accessor.getter())
            .await?
        {
            DeferredCallReplayOutcome::Replayed(mut response, delivery) => {
                let fingerprint = match recorded_preparation {
                    Some((owner, request)) => claim_completed_preparation(accessor, owner, request)
                        .await?
                        .result
                        .ok(),
                    None => known_fingerprint,
                };
                let final_id = fingerprint.map(|fingerprint| {
                    request_key.finalize(
                        remote_agent_id.agent_id(),
                        fingerprint,
                        &prepared.method_name,
                    )
                });
                // Live, every call that was not denied bound and pumped its input streams before
                // its result was recorded, whether that result is a value or an error.
                if prepared.is_streaming()
                    && !denied
                    && let (Some(final_id), Some(fingerprint)) = (&final_id, fingerprint)
                {
                    let auth_ctx =
                        with_import_access(accessor, |mut access| access.get().agent_auth_ctx());
                    let origin = streaming_origin.ok_or_else(|| {
                        WorkerExecutorError::unexpected_oplog_entry(
                            start_index,
                            "streaming RPC Start with logical origin",
                            format!("streaming RPC Start at {start_index} without logical origin"),
                        )
                    })?;
                    let streams = open_streams_at(
                        accessor,
                        &remote_agent_id,
                        final_id,
                        fingerprint,
                        origin,
                        auth_ctx,
                    )
                    .await?;
                    materialize_input(accessor, &streams, &prepared).await?;
                    if response.result.is_ok()
                        && let Some(value) = streams
                            .replay_remote_result()
                            .await
                            .map_err(SessionError::into_trap)?
                    {
                        response.result = Ok(value);
                    }
                }
                return deliver(accessor, response, final_id.as_ref(), delivery, &span_id).await;
            }
            DeferredCallReplayOutcome::Incomplete(live) => handle = live,
        }
    }

    let freshness = if known_fresh_dispatch_allowed(started_live, prepared.is_ephemeral()) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    };
    let fingerprint = match known_fingerprint {
        Some(fingerprint) => Ok(fingerprint),
        None => {
            prepare_ephemeral_identity(
                accessor,
                &mut handle,
                &prepared,
                &remote_agent_id,
                &idempotency_key,
                &span_id,
                freshness,
                "invoke-and-await",
            )
            .await?
        }
    };
    let fingerprint = match fingerprint {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let finished = rpc_span_finished::<()>(&span_id, &Err(error.clone()));
            let (response, delivery) = handle
                .complete_access_deferred_with_span(
                    accessor,
                    accessor.getter(),
                    HostResponseGolemRpcInvokeAndAwait {
                        result: Err(error.into()),
                    },
                    finished,
                )
                .await?;
            return deliver(accessor, response, None, delivery, &span_id).await;
        }
    };
    let final_id = request_key.finalize(
        remote_agent_id.agent_id(),
        fingerprint,
        &prepared.method_name,
    );
    // A client reconstructed after a restart activates its recorded incarnation inside each
    // dispatch task, as before the restart, and creates nothing.
    let (deferred_activation, expected_callee_fingerprint) =
        with_import_access(accessor, |mut access| {
            let activation = &access.get().table().get(&this)?.payload.target_activation;
            anyhow::Ok((
                activation.deferred_activation(),
                activation.target_fingerprint(),
            ))
        })
        .map_err(|error| handle.trap(error))?;

    let auth_ctx = handle.agent_auth_ctx().clone();
    let streaming = match streaming_origin {
        Some(origin) => {
            let streams = open_streams_at(
                accessor,
                &remote_agent_id,
                &final_id,
                fingerprint,
                origin.clone(),
                auth_ctx.clone(),
            )
            .await
            .map_err(|error| handle.trap(error))?;
            let input = materialize_input(accessor, &streams, &prepared)
                .await
                .map_err(|error| handle.trap(error))?;
            let attempt_id = streams
                .caller_attempt_id()
                .await
                .map_err(|error| handle.trap(error.into_trap()))?;
            Some(DurableStreamingTaskParams {
                streams,
                input_mappings: input.proto_mappings(),
                input: input.value,
                expected_callee_fingerprint: fingerprint,
                attempt_id: attempt_id.0,
                origin_invocation: origin_invocation_identity(origin),
                output_graph: Arc::new(prepared.remote_agent_type.schema.clone()),
                output_root: rpc_output_root(&prepared),
            })
        }
        None if prepared.is_streaming() => {
            return Err(handle.trap(WorkerExecutorError::unexpected_oplog_entry(
                handle.start_index(),
                "streaming RPC Start with logical origin",
                format!(
                    "streaming RPC Start at {} without logical origin",
                    handle.start_index()
                ),
            )));
        }
        None => None,
    };

    let remote_target = dispatch_target(&remote_agent_id, prepared.ephemeral.as_ref());
    let retry_properties = prepared.retry_properties("invoke-and-await", &remote_agent_id);
    let spawn = |handle: &InvokeAndAwaitCall| {
        with_import_access(accessor, |mut access| {
            let ctx = access.get();
            match &streaming {
                Some(params) => spawn_streaming_invoke_and_await_task(
                    ctx,
                    remote_target.clone(),
                    idempotency_key.clone(),
                    prepared.method_name.clone(),
                    params.clone(),
                    prepared.env.clone(),
                    prepared.config.clone(),
                    deferred_activation.clone(),
                    &span_id,
                    scope_card.clone(),
                    auth_ctx.clone(),
                ),
                None => spawn_invoke_and_await_task(
                    ctx,
                    &remote_target,
                    idempotency_key.clone(),
                    prepared.method_name.clone(),
                    prepared.input_value.clone(),
                    prepared.env.clone(),
                    prepared.config.clone(),
                    deferred_activation.clone(),
                    expected_callee_fingerprint,
                    retry_properties.clone(),
                    &span_id,
                    handle.trap_context().retry_from,
                    handle.start_index(),
                    InvocationFreshnessDisposition::MayExist,
                    scope_card.clone(),
                    auth_ctx.clone(),
                ),
            }
        })
    };

    let mut withdrawal = PendingWithdrawal::arm(
        accessor,
        &handle,
        final_id.clone(),
        auth_ctx.clone(),
        streaming.as_ref().map(|params| params.streams.clone()),
    );
    let mut task = spawn(&handle);
    let result = loop {
        let task_result = suspendable(accessor, &mut task).await;
        let rpc_result = match task_result {
            Ok(Ok(rpc_result)) => rpc_result,
            Ok(Err(error)) => {
                // The background RPC failed hard after its in-task retries: a trap, not a
                // recorded result. The `Start` stays incomplete for repair.
                withdrawal.disarm();
                return Err(handle.trap(error));
            }
            Err(interrupt) => {
                withdrawal.disarm();
                drop(task);
                return Err(handle.trap(interrupt));
            }
        };
        if streaming.is_none() {
            // A failed backoff wait traps the call; repair dispatches it again with its key.
            let retry =
                retry_after_backoff(accessor, &mut handle, retry_properties.clone(), &rpc_result)
                    .await;
            if retry.is_err() {
                withdrawal.disarm();
            }
            if retry? {
                task = spawn(&handle);
                continue;
            }
        }
        break rpc_result;
    };
    withdrawal.disarm();

    let result = admit_rpc_result_secret_holds(accessor, result)
        .await
        .map_err(|error| handle.trap(error))?;
    let finished = rpc_span_finished(&span_id, &result);
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
    if let Some(params) = &streaming
        && response.result.is_ok()
        && let Some(value) = params
            .streams
            .replay_remote_result()
            .await
            .map_err(SessionError::into_trap)?
    {
        response.result = Ok(value);
    }
    deliver(accessor, response, Some(&final_id), delivery, &span_id).await
}

/// Finishes the call's span and returns the recorded result to the guest.
async fn deliver<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: HostResponseGolemRpcInvokeAndAwait,
    final_id: Option<&OwnedInvocationId>,
    delivery: CompletionDelivery,
    span_id: &SpanId,
) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
    deliver_rpc_result(accessor, delivery, Some(span_id), |ctx| {
        Ok(
            match invoke_and_await_response_to_wire(response.result, ctx)? {
                Ok(result) => {
                    let final_id = final_id.ok_or_else(|| {
                        anyhow::anyhow!("a successful RPC result has no final invocation identity")
                    })?;
                    Ok(InvocationResultWithMetadata {
                        metadata: invocation_metadata(final_id)?,
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
