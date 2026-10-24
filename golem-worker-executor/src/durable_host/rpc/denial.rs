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

use super::{
    FutureInvokeResultEntry, FutureInvokeResultState, WasmRpcEntry, WasmRpcEntryPayload,
    embedded_invocation_span_builder, invocation_metadata, invocation_target_agent_id,
    restore_embedded_span,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::IdempotencyKeyPosition;
use crate::durable_host::concurrent::BegunCall;
use crate::durable_host::concurrent::CallReplayOutcome;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DropPolicy;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::NotCancellable;
use crate::durable_host::concurrent::ResolvedCall;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::preview2::golem::agent::host::AsyncInvocationWithMetadata;
use crate::preview2::golem::agent::host::CancellationToken;
use crate::preview2::golem::agent::host::InvocationMetadata;
use crate::preview2::golem::agent::host::InvocationResultWithMetadata;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::IdempotencyKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::oplog::HostRequestGolemRpcAsyncInvokeRejection;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::HostRequestGolemRpcScheduledInvocation;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::oplog::SpanFinished;
use golem_common::model::oplog::SpanOutcome;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcAsyncInvokeRejection;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvoke;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcInvokeAndAwaitResult;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcScheduleInvocation;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::schema::schema_value::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::fmt::Debug;
use wasmtime::component::Resource;

fn denied_rpc_request(
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
    method_name: String,
) -> HostRequestGolemRpcInvoke {
    HostRequestGolemRpcInvoke {
        remote_agent_id: remote_agent_id.agent_id(),
        idempotency_key: idempotency_key.clone(),
        method_name,
        input: SchemaValue::Tuple { elements: vec![] },
        logical_streaming_origin: None,
        remote_agent_type: None,
        remote_agent_parameters: None,
        scope_card: None,
    }
}

fn rpc_denied_details(error: RpcError) -> String {
    match error {
        RpcError::Denied(details) => details,
        _ => unreachable!("authorization only returns denied RPC errors"),
    }
}

/// The denial recorded in the claimed `Start` of an RPC call. A denied call never dispatches.
pub(super) async fn recorded_rpc_denial<Pair, P, Ctx>(
    ctx: &DurableWorkerCtx<Ctx>,
    handle: &DurableCallSession<Pair, P>,
) -> Result<Option<String>, WorkerExecutorError>
where
    Pair: HostPayloadPair,
    P: DropPolicy,
    Ctx: WorkerCtx,
{
    match ctx.state.oplog.read(handle.start_index()).await? {
        OplogEntry::Start { denial, .. } => Ok(denial),
        entry => Err(WorkerExecutorError::unexpected_oplog_entry(
            format!("Start at {}", handle.start_index()),
            format!("{entry:?}"),
        )),
    }
}

/// Replays an RPC call whose `Start` records a denial. Replay completes an incomplete denied
/// `Start` with the denied response, so the call is never repaired by dispatching it.
pub(super) async fn replay_rpc_denial<Pair, P, Ctx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: DurableCallSession<Pair, P>,
) -> anyhow::Result<Pair::Resp>
where
    Pair: HostPayloadPair,
    P: DropPolicy,
    Ctx: WorkerCtx,
{
    let started = handle.recorded_span_started(ctx).await?;
    let span = started
        .as_ref()
        .map(|started| restore_embedded_span(ctx, started))
        .transpose()?;
    let response = match handle.replay(ctx).await? {
        CallReplayOutcome::Replayed(response) => response,
        CallReplayOutcome::Incomplete(mut handle) => {
            let start_index = handle.start_index();
            return Err(handle.trap(WorkerExecutorError::unexpected_oplog_entry(
                format!(
                    "denied {} Start completed with its denied response",
                    Pair::FQFN
                ),
                format!("incomplete denied Start at {start_index}"),
            )));
        }
    };
    if let Some(span) = span {
        finish_span_in_memory(ctx, span.span_id())?;
    }
    Ok(response)
}

pub(super) fn denied_rpc_failure<T: Debug>(
    result: Result<T, SerializableRpcError>,
) -> Result<InternalRpcError, WorkerExecutorError> {
    match result {
        Err(error) => Ok(InternalRpcError::from(error)),
        Ok(value) => Err(WorkerExecutorError::unexpected_oplog_entry(
            "denied RPC response",
            format!("{value:?}"),
        )),
    }
}

pub(super) async fn replay_async_rpc_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: DurableCallSession<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let request = handle.recorded_request(ctx).await?;
    let metadata = invocation_metadata(
        &OwnedAgentId::new(ctx.owned_agent_id.environment_id, &request.remote_agent_id),
        &request.idempotency_key,
    );
    let response = replay_rpc_denial(ctx, handle).await?;
    bake_closed_async_error(ctx, metadata, denied_rpc_failure(response.result)?)
}

pub(super) async fn persist_invoke_and_await_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
    let begun = DurableCallSession::<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>::begin(
        ctx,
        DurableFunctionType::WriteRemote,
    )
    .await?;
    let key_position = ctx.idempotency_key_position();
    persist_invoke_and_await_denial_from_begun(
        ctx,
        begun,
        key_position,
        remote_agent_id,
        method_name,
        error,
    )
    .await
}

pub(super) async fn persist_invoke_and_await_denial_from_begun<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    begun: BegunCall<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable, Ctx>,
    key_position: IdempotencyKeyPosition,
    remote_agent_id: OwnedAgentId,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
    let details = rpc_denied_details(error);
    let (_, response, _) = begun
        .start_live_denied_at_index(ctx, details, move |start_index| {
            let idempotency_key = key_position.derive(start_index);
            Ok((
                denied_rpc_request(&remote_agent_id, &idempotency_key, method_name),
                None,
            ))
        })
        .await?;
    Ok(Err(denied_rpc_failure(response.result)?.into()))
}

pub(super) async fn persist_invoke_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<Result<InvocationMetadata, RpcError>> {
    let begun = DurableCallSession::<GolemRpcWasmRpcInvoke, NotCancellable>::begin(
        ctx,
        DurableFunctionType::WriteRemote,
    )
    .await?;
    let key_position = ctx.idempotency_key_position();
    persist_invoke_denial_from_begun(
        ctx,
        begun,
        key_position,
        remote_agent_id,
        method_name,
        error,
    )
    .await
}

pub(super) async fn persist_invoke_denial_from_begun<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    begun: BegunCall<GolemRpcWasmRpcInvoke, NotCancellable, Ctx>,
    key_position: IdempotencyKeyPosition,
    remote_agent_id: OwnedAgentId,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<Result<InvocationMetadata, RpcError>> {
    let details = rpc_denied_details(error);
    let (_, response, _) = begun
        .start_live_denied_at_index(ctx, details, move |start_index| {
            let idempotency_key = key_position.derive(start_index);
            Ok((
                denied_rpc_request(&remote_agent_id, &idempotency_key, method_name),
                None,
            ))
        })
        .await?;
    Ok(Err(denied_rpc_failure(response.result)?.into()))
}

pub(super) async fn persist_async_invoke_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    logical_remote_agent_id: OwnedAgentId,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let begun = DurableCallSession::<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>::begin(
        ctx,
        DurableFunctionType::WriteRemote,
    )
    .await?;
    let key_position = ctx.idempotency_key_position();
    persist_async_invoke_denial_from_begun(
        ctx,
        begun,
        key_position,
        resource,
        logical_remote_agent_id,
        method_name,
        error,
    )
    .await
}

pub(super) async fn persist_async_validation_rejection<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    connection_span_id: &SpanId,
    logical_remote_agent_id: &OwnedAgentId,
    ephemeral_logical_agent_id: Option<&ParsedAgentId>,
    method_name: &str,
    error: InternalRpcError,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let begun = DurableCallSession::<GolemRpcWasmRpcAsyncInvokeRejection, NotCancellable>::begin(
        ctx,
        DurableFunctionType::WriteLocal,
    )
    .await?;
    let key_position = ctx.idempotency_key_position();
    let serializable_error: SerializableRpcError = error.clone().into();
    let expected = HostResponseGolemRpcInvokeAndAwait {
        result: Err(serializable_error.clone()),
    };
    let mut handle = match begun.resolve(ctx).await? {
        ResolvedCall::Live(begun) => {
            let span_started =
                embedded_invocation_span_builder(ctx, connection_span_id, method_name)?;
            let key_position = key_position.clone();
            let logical_remote_agent_id = logical_remote_agent_id.clone();
            let ephemeral_logical_agent_id = ephemeral_logical_agent_id.cloned();
            let method_name = method_name.to_string();
            begun
                .start_live_with_index_and_span(ctx, move |start_index| {
                    let idempotency_key = key_position.derive(start_index);
                    let remote_agent_id = invocation_target_agent_id(
                        &logical_remote_agent_id,
                        ephemeral_logical_agent_id.as_ref(),
                        &idempotency_key,
                    )
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                    let started = span_started(&idempotency_key);
                    Ok((
                        HostRequestGolemRpcAsyncInvokeRejection {
                            remote_agent_id: remote_agent_id.agent_id(),
                            idempotency_key,
                            method_name,
                            error: serializable_error,
                        },
                        started,
                    ))
                })
                .await?
        }
        ResolvedCall::Replay(handle) => handle,
    };
    let idempotency_key = key_position.derive(handle.start_index());
    let remote_agent_id = invocation_target_agent_id(
        logical_remote_agent_id,
        ephemeral_logical_agent_id,
        &idempotency_key,
    )?;
    let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
    let started = handle.recorded_span_started(ctx).await?.ok_or_else(|| {
        WorkerExecutorError::unexpected_oplog_entry(
            "async RPC validation rejection Start carrying span_started",
            format!(
                "rejection Start at {} without span metadata",
                handle.start_index()
            ),
        )
    })?;
    let span = restore_embedded_span(ctx, &started)?;
    let span_finished = SpanFinished {
        span_id: span.span_id().clone(),
        finished_at: golem_common::model::Timestamp::now_utc(),
        outcome: SpanOutcome::Failed,
    };
    if handle.is_live() {
        handle
            .complete_with_span(ctx, expected, span_finished)
            .await?;
    } else {
        match handle.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) if response == expected => {}
            CallReplayOutcome::Replayed(response) => {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!("async RPC validation rejection {expected:?}"),
                    format!("{response:?}"),
                )
                .into());
            }
            CallReplayOutcome::Incomplete(live) => {
                handle = live;
                handle
                    .complete_with_span(ctx, expected, span_finished)
                    .await?;
            }
        }
    }
    finish_span_in_memory(ctx, span.span_id())?;
    bake_closed_async_error(ctx, metadata, error)
}

fn bake_closed_async_error<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    metadata: InvocationMetadata,
    error: InternalRpcError,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let future = ctx.table().push(FutureInvokeResultEntry {
        payload: Box::new(FutureInvokeResultState::Baked {
            result: Ok(Err(error)),
        }),
        child_pollables: Vec::new(),
        drop_pending: false,
    })?;
    Ok(AsyncInvocationWithMetadata { future, metadata })
}

pub(super) async fn persist_async_invoke_denial_from_begun<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    begun: BegunCall<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable, Ctx>,
    key_position: IdempotencyKeyPosition,
    resource: &Resource<WasmRpcEntry>,
    logical_remote_agent_id: OwnedAgentId,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let details = rpc_denied_details(error);
    let (ephemeral_logical_agent_id, connection_span_id) = {
        let payload = ctx
            .table()
            .get(resource)?
            .payload
            .downcast_ref::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
        (
            payload.ephemeral_logical_agent_id.clone(),
            payload.span_id.clone(),
        )
    };
    let span_started = embedded_invocation_span_builder(ctx, &connection_span_id, &method_name)?;
    let (start_index, response, started) = {
        let key_position = key_position.clone();
        let logical_remote_agent_id = logical_remote_agent_id.clone();
        let ephemeral_logical_agent_id = ephemeral_logical_agent_id.clone();
        begun
            .start_live_denied_at_index(ctx, details, move |start_index| {
                let idempotency_key = key_position.derive(start_index);
                let remote_agent_id = invocation_target_agent_id(
                    &logical_remote_agent_id,
                    ephemeral_logical_agent_id.as_ref(),
                    &idempotency_key,
                )
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                let started = span_started(&idempotency_key);
                Ok((
                    denied_rpc_request(&remote_agent_id, &idempotency_key, method_name),
                    Some(started),
                ))
            })
            .await?
    };
    let idempotency_key = key_position.derive(start_index);
    let remote_agent_id = invocation_target_agent_id(
        &logical_remote_agent_id,
        ephemeral_logical_agent_id.as_ref(),
        &idempotency_key,
    )?;
    let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
    let started = started.expect("the denied asynchronous RPC records its span");
    let span = restore_embedded_span(ctx, &started)?;
    finish_span_in_memory(ctx, span.span_id())?;
    bake_closed_async_error(ctx, metadata, denied_rpc_failure(response.result)?)
}

fn bake_closed_async_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    metadata: InvocationMetadata,
    details: String,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let future = ctx.table().push(FutureInvokeResultEntry {
        payload: Box::new(FutureInvokeResultState::Baked {
            result: Ok(Err(InternalRpcError::Denied { details })),
        }),
        child_pollables: Vec::new(),
        drop_pending: false,
    })?;
    Ok(AsyncInvocationWithMetadata { future, metadata })
}

pub(super) async fn bake_recorded_async_invoke_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    logical_remote_agent_id: &OwnedAgentId,
    error: RpcError,
    idempotency_key: IdempotencyKey,
) -> anyhow::Result<AsyncInvocationWithMetadata> {
    let details = rpc_denied_details(error);
    let ephemeral_logical_agent_id = {
        let payload = ctx
            .table()
            .get(resource)?
            .payload
            .downcast_ref::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
        payload.ephemeral_logical_agent_id.clone()
    };
    let remote_agent_id = invocation_target_agent_id(
        logical_remote_agent_id,
        ephemeral_logical_agent_id.as_ref(),
        &idempotency_key,
    )?;
    let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
    bake_closed_async_denial(ctx, metadata, details)
}

pub(super) async fn persist_schedule_denial<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<Result<Resource<CancellationToken>, RpcError>> {
    let begun = DurableCallSession::<GolemRpcWasmRpcScheduleInvocation, NotCancellable>::begin(
        ctx,
        DurableFunctionType::WriteRemote,
    )
    .await?;
    let key_position = ctx.idempotency_key_position();
    match begun.resolve(ctx).await? {
        ResolvedCall::Live(begun) => {
            persist_schedule_denial_from_begun(
                ctx,
                begun,
                key_position,
                remote_agent_id,
                datetime,
                method_name,
                error,
            )
            .await
        }
        ResolvedCall::Replay(handle) => {
            let response = replay_rpc_denial(ctx, handle).await?;
            Ok(Err(denied_rpc_failure(response.result)?.into()))
        }
    }
}

pub(super) async fn persist_schedule_denial_from_begun<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    begun: BegunCall<GolemRpcWasmRpcScheduleInvocation, NotCancellable, Ctx>,
    key_position: IdempotencyKeyPosition,
    remote_agent_id: OwnedAgentId,
    datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
    method_name: String,
    error: RpcError,
) -> anyhow::Result<Result<Resource<CancellationToken>, RpcError>> {
    let details = rpc_denied_details(error);
    let datetime: golem_common::model::oplog::types::SerializableDateTime = datetime.into();
    let (_, response, _) = begun
        .start_live_denied_at_index(ctx, details, move |start_index| {
            Ok((
                HostRequestGolemRpcScheduledInvocation {
                    remote_agent_id: remote_agent_id.agent_id(),
                    idempotency_key: key_position.derive(start_index),
                    method_name,
                    input: SchemaValue::Tuple { elements: vec![] },
                    datetime,
                    remote_agent_type: None,
                    remote_agent_parameters: None,
                },
                None,
            ))
        })
        .await?;
    Ok(Err(denied_rpc_failure(response.result)?.into()))
}
