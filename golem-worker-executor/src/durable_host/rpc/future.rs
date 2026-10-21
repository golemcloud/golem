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
    DurableStreamingTaskParams, FutureInvokeCallHandle, FutureInvokeGetResult,
    FutureInvokeResultEntry, FutureInvokeResultState, FutureInvokeTaskHandle,
    FutureInvokeTaskResult, RpcTargetActivation, RpcTask, SubscribeAny,
    invoke_and_await_response_to_wire, rpc_span_finished, schema_value_to_wire_output,
    spawn_invoke_and_await_task, spawn_streaming_invoke_and_await_task,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::DeferredCallReplayOutcome;
use crate::durable_host::concurrent::authorize_live_permissions_at_serialized_access;
use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::concurrent::try_agent_auth_ctx_at_serialized_access;
use crate::durable_host::durability::ClassifiedHostError;
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::durable_session::strip_streams;
use crate::durable_host::durable_stream::SessionError;
use crate::durable_host::secrets::secret_hold_targets_for_value;
use crate::preview2::golem::agent::host::FutureInvokeResult;
use crate::preview2::golem::agent::host::HostFutureInvokeResult;
use crate::preview2::golem::agent::host::HostFutureInvokeResultWithStore;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::AgentId;
use golem_common::model::IdempotencyKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostRequestGolemRpcInvoke;
use golem_common::model::oplog::HostResponseGolemRpcInvokeAndAwait;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::any::Any;
use wasmtime::component::Accessor;
use wasmtime::component::HasSelf;
use wasmtime::component::Resource;
use wasmtime::component::ResourceTableError;

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for FutureInvokeResultEntry {
    async fn ready(&mut self) {
        self.payload.ready().await
    }
}

impl wasmtime_wasi::DynamicPollable for FutureInvokeResultEntry {
    fn override_index(&self) -> Option<u32> {
        None
    }
}

impl Drop for RpcTask {
    fn drop(&mut self) {
        if let Some(revoker) = &self.revoker {
            revoker.revoke();
        }
    }
}

impl std::future::Future for RpcTask {
    type Output = FutureInvokeTaskResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.task).poll(cx)
    }
}

async fn admit_rpc_result_secret_holds<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    result: Result<SchemaValue, InternalRpcError>,
) -> Result<Result<SchemaValue, InternalRpcError>, WorkerExecutorError>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let targets = match &result {
        Ok(value) => {
            accessor.with(|mut access| secret_hold_targets_for_value(access.get(), value))?
        }
        Err(_) => return Ok(result),
    };
    if targets.is_empty() {
        return Ok(result);
    }
    match authorize_live_permissions_at_serialized_access(accessor, accessor.getter(), &targets)
        .await?
    {
        Ok(_) => Ok(result),
        Err(_) => Ok(Err(InternalRpcError::Denied {
            details: "permission denied".to_string(),
        })),
    }
}

/// Projects a background RPC task result (as produced for the [`FutureInvokeResultState::Baked`]
/// path) into the wire result shape returned by `future-invoke-result.get`. A hard task failure
/// (`Err`) is surfaced as a `get` trap.
fn future_invoke_get_result_to_wire<Ctx: WorkerCtx>(
    result: FutureInvokeGetResult,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        Ok(Ok(value)) => Ok(Ok(schema_value_to_wire_output(&value, ctx)?)),
        Ok(Err(rpc_error)) => Ok(Err(rpc_error)),
        Err(err) => Err(err),
    }
}

/// Projects a completed `invoke-and-await` durable response (the payload of the call's `End`) into
/// the wire result shape returned by `future-invoke-result.get`.
fn future_invoke_task_result_to_get_result(
    result: &FutureInvokeTaskResult,
) -> FutureInvokeGetResult {
    match result {
        Ok(Ok(value)) => Ok(Ok(value.clone())),
        Ok(Err(error)) => Ok(Err(error.clone().into())),
        Err(err) => Err(anyhow::anyhow!(err.to_string())),
    }
}

/// Terminal path for a live in-flight `future-invoke-result.get` whose shared cancel token was
/// triggered by a concurrent `future-invoke-result::cancel`. The caller has already dropped the
/// background task (aborting the in-flight RPC). This best-effort cancels the remote invocation,
/// records the call's `Cancelled` terminal with a partial "Invocation cancelled" response (so a
/// replayed `get` deterministically delivers the same result via `replay_access_deferred`),
/// finishes the invocation span, and marks the resource consumed.
async fn cancel_in_flight_get<T: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut handle: FutureInvokeCallHandle,
    request: &HostRequestGolemRpcInvoke,
    span_id: &SpanId,
    this_rep: u32,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    handle.close_span_on_cancellation(span_id.clone());
    let worker_proxy = accessor.with(|mut access| access.get().worker_proxy());
    match try_agent_auth_ctx_at_serialized_access(accessor, accessor.getter()).await {
        Ok(Some(auth_ctx)) => {
            if let Err(err) = worker_proxy
                .cancel_invocation(
                    &request.remote_agent_id,
                    request.idempotency_key.clone(),
                    &auth_ctx,
                )
                .await
            {
                tracing::info!(err=%err, "Best-effort cancel_invocation failed");
            }
        }
        Ok(None) => {
            tracing::debug!(
                "Skipping best-effort cancel_invocation while card authority recovery is pending"
            );
        }
        Err(err) => {
            tracing::info!(err=%err, "Skipping best-effort cancel_invocation after wallet synchronization failed");
        }
    }

    let partial_result: Result<SchemaValue, SerializableRpcError> =
        Err(SerializableRpcError::ProtocolError {
            details: "Invocation cancelled".to_string(),
        });
    handle
        .cancel_access(
            accessor,
            accessor.getter(),
            Some(HostResponseGolemRpcInvokeAndAwait {
                result: partial_result.clone(),
            }),
        )
        .await
        .map_err(anyhow::Error::from)?;
    accessor.with(|mut access| finish_span_in_memory(access.get(), span_id))?;
    accessor.with(|mut access| {
        let ctx = access.get();
        let entry = ctx
            .table()
            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
        let state = entry
            .payload
            .as_any_mut()
            .downcast_mut::<FutureInvokeResultState>()
            .unwrap();
        *state = FutureInvokeResultState::Consumed;
        Ok::<_, anyhow::Error>(())
    })?;
    accessor.with(|mut access| invoke_and_await_response_to_wire(partial_result, access.get()))
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
        let this_rep = this.rep();

        // Decide what `get` must do while holding the state lock, then run the async terminal
        // outside it. The durable call handle (and, on the live path, the background task) are taken
        // out of the state here so the terminal runs while nothing in the resource table still owns
        // them: a live handle left behind and later dropped would spuriously enqueue a `Cancelled`.
        #[allow(clippy::large_enum_variant)]
        enum GetPlan {
            /// The result is already known — a baked deterministic failure or a prior cancellation.
            Ready {
                result: anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>>,
            },
            /// The single durable call is still open; drive it to its `End`. `request`,
            /// `remote_agent_id`, and `env` are carried so a replay that finds an incomplete `Start`
            /// (crash-after-`Start`) can re-dispatch the read-only RPC and complete it.
            /// `cancel_token` is shared with the state left in the table: a concurrent
            /// `future-invoke-result::cancel` (which finds `handle: None` there) triggers it, and
            /// the live await below reacts by cancelling this call.
            Active {
                handle: FutureInvokeCallHandle,
                task: Option<FutureInvokeTaskHandle>,
                streaming: Option<DurableStreamingTaskParams>,
                request: HostRequestGolemRpcInvoke,
                remote_agent_id: OwnedAgentId,
                env: Vec<(String, String)>,
                config: Vec<AgentConfigEntryDto>,
                deferred_activation: Option<RpcTargetActivation>,
                span_id: SpanId,
                cancel_token: tokio_util::sync::CancellationToken,
            },
        }

        let plan = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "get");
            let entry = ctx
                .table()
                .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            Ok::<_, anyhow::Error>(match state {
                FutureInvokeResultState::Consumed => {
                    // The result was already consumed and its span already closed.
                    GetPlan::Ready {
                        result: Err(anyhow::Error::new(ClassifiedHostError {
                            kind: HostFailureKind::Permanent,
                            message: "future-invoke-result already consumed".to_string(),
                        })),
                    }
                }
                FutureInvokeResultState::Baked { result } => {
                    let result = future_invoke_task_result_to_get_result(result);
                    *state = FutureInvokeResultState::Consumed;
                    GetPlan::Ready {
                        result: future_invoke_get_result_to_wire(result, ctx),
                    }
                }
                FutureInvokeResultState::Cancelled => {
                    let rpc_error = InternalRpcError::ProtocolError {
                        details: "Invocation cancelled".to_string(),
                    };
                    *state = FutureInvokeResultState::Consumed;
                    GetPlan::Ready {
                        result: Ok(Err(rpc_error.into())),
                    }
                }
                FutureInvokeResultState::Active {
                    handle,
                    task,
                    streaming,
                    request,
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id,
                    cancel_token,
                } => {
                    let handle = handle
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("future-invoke-result already consumed"))?;
                    GetPlan::Active {
                        handle,
                        task: task.take(),
                        streaming: streaming.clone(),
                        request: request.clone(),
                        remote_agent_id: remote_agent_id.clone(),
                        env: env.clone(),
                        config: config.clone(),
                        deferred_activation: deferred_activation.clone(),
                        span_id: span_id.clone(),
                        cancel_token: cancel_token.clone(),
                    }
                }
            })
        })?;

        match plan {
            GetPlan::Ready { result, .. } => result,
            GetPlan::Active {
                mut handle,
                task,
                streaming,
                request,
                remote_agent_id,
                env,
                config,
                deferred_activation,
                span_id,
                cancel_token,
            } => {
                handle.close_span_on_cancellation(span_id.clone());
                // The response crosses more guest-facing work after the durable `End` (span
                // finish, resource-state transition, wire conversion), so the terminal is
                // recorded through the deferred-delivery API: the returned token stays armed
                // until this method actually returns the result, and a torn future in between
                // records the `CompletionDiscarded` marker. The span closure is embedded in the
                // same terminal as the response, so replay restores both from one durable fact.
                let (response, delivery) = if handle.is_live() {
                    let task =
                        task.expect("a live future-invoke-result must own its background task");
                    let (rpc_wait, interrupt_signal) = accessor.with(|mut access| {
                        let ctx = access.get();
                        let wait = accessor.runtime_activity().and_then(|activity| {
                            ctx.runtime_suspension.as_ref().and_then(|runtime| {
                                runtime.rpc_wait(
                                    activity,
                                    ctx.state.config.suspend.rpc_suspend_after,
                                    ctx.state.config.suspend.rpc_resume_after,
                                )
                            })
                        });
                        (wait, ctx.create_interrupt_signal())
                    });
                    let task_result = {
                        let mut guard = task.lock().await;
                        tokio::select! {
                            biased;
                            _ = cancel_token.cancelled() => None,
                            result = async {
                                let ready = &mut *guard;
                                match rpc_wait {
                                    Some(wait) => wait.wait_result(ready, interrupt_signal).await,
                                    None => tokio::select! {
                                        biased;
                                        kind = interrupt_signal => Err(kind),
                                        result = ready => Ok(result),
                                    },
                                }
                            } => match result {
                                Ok(result) => Some(result),
                                Err(interrupt_kind) => {
                                    drop(guard);
                                    drop(task);
                                    return Err(handle.trap(interrupt_kind));
                                }
                            },
                        }
                    };
                    let task_result = match task_result {
                        Some(result) => result,
                        None => {
                            // A concurrent `future-invoke-result::cancel` fired while this `get`
                            // owned the handle. Drop the background task (aborting the in-flight
                            // RPC), then cancel the durable call: best-effort remote cancellation
                            // plus a `Cancelled` terminal carrying the partial "Invocation
                            // cancelled" response, so replay delivers the same result here.
                            drop(task);
                            return cancel_in_flight_get(
                                accessor, handle, &request, &span_id, this_rep,
                            )
                            .await;
                        }
                    };
                    match task_result {
                        Ok(rpc_result) => {
                            let rpc_result =
                                admit_rpc_result_secret_holds(accessor, rpc_result).await?;
                            let finish_span = rpc_span_finished(&span_id, &rpc_result);
                            handle
                                .complete_access_deferred_with_span(
                                    accessor,
                                    accessor.getter(),
                                    HostResponseGolemRpcInvokeAndAwait {
                                        result: rpc_result.map(strip_streams).map_err(Into::into),
                                    },
                                    finish_span,
                                )
                                .await
                                .map_err(anyhow::Error::from)?
                        }
                        Err(err) => {
                            // The background RPC failed hard after its in-task retries. This is a
                            // trap, not a durable result: abandon the call, leaving its `Start`
                            // incomplete for durable-scope recovery, instead of recording an `End`.
                            return Err(handle.trap(err));
                        }
                    }
                } else {
                    match handle
                        .replay_access_deferred(accessor, accessor.getter())
                        .await
                        .map_err(anyhow::Error::from)?
                    {
                        DeferredCallReplayOutcome::Replayed(response, delivery) => {
                            (response, delivery)
                        }
                        DeferredCallReplayOutcome::Incomplete(mut live) => {
                            // Crash-after-`Start` recovery: the eager `Start` is committed but its
                            // terminal was never written. A read-only `WriteRemote` call is safe to
                            // re-execute, so re-dispatch the RPC now and complete the existing `Start`
                            // (mirrors the synchronous path's `Incomplete` -> re-run).
                            let retry_point = live.begin_index();
                            let auth_ctx = live.take_agent_auth_ctx();
                            let mut task = accessor.with(|mut access| {
                                let ctx = access.get();
                                match streaming.clone() {
                                    Some(params) => spawn_streaming_invoke_and_await_task(
                                        ctx,
                                        remote_agent_id.clone(),
                                        request.idempotency_key.clone(),
                                        request.method_name.clone(),
                                        params,
                                        env.clone(),
                                        config.clone(),
                                        deferred_activation.clone(),
                                        &span_id,
                                        request.scope_card.clone(),
                                        auth_ctx,
                                    ),
                                    None => spawn_invoke_and_await_task(
                                        ctx,
                                        &remote_agent_id,
                                        request.idempotency_key.clone(),
                                        request.method_name.clone(),
                                        request.input.clone(),
                                        env.clone(),
                                        config.clone(),
                                        deferred_activation.clone(),
                                        &span_id,
                                        retry_point,
                                        InvocationFreshnessDisposition::MayExist,
                                        request.scope_card.clone(),
                                        auth_ctx,
                                    ),
                                }
                            });
                            let (rpc_wait, interrupt_signal) = accessor.with(|mut access| {
                                let ctx = access.get();
                                let wait = accessor.runtime_activity().and_then(|activity| {
                                    ctx.runtime_suspension.as_ref().and_then(|runtime| {
                                        runtime.rpc_wait(
                                            activity,
                                            ctx.state.config.suspend.rpc_suspend_after,
                                            ctx.state.config.suspend.rpc_resume_after,
                                        )
                                    })
                                });
                                (wait, ctx.create_interrupt_signal())
                            });
                            let task_result = {
                                tokio::select! {
                                biased;
                                _ = cancel_token.cancelled() => None,
                                result = async {
                                    match rpc_wait {
                                        Some(wait) => wait.wait_result(&mut task, interrupt_signal).await,
                                        None => tokio::select! {
                                            biased;
                                            kind = interrupt_signal => Err(kind),
                                            result = &mut task => Ok(result),
                                        },
                                    }
                                } => match result {
                                    Ok(result) => Some(result),
                                    Err(interrupt_kind) => {
                                        drop(task);
                                        return Err(live.trap(interrupt_kind));
                                    }
                                },
                                }
                            };
                            match task_result {
                                None => {
                                    // Cancelled while re-executing the recovered call: same
                                    // handling as the live-path cancellation race above.
                                    drop(task);
                                    return cancel_in_flight_get(
                                        accessor, live, &request, &span_id, this_rep,
                                    )
                                    .await;
                                }
                                Some(Ok(rpc_result)) => {
                                    let rpc_result =
                                        admit_rpc_result_secret_holds(accessor, rpc_result).await?;
                                    let finish_span = rpc_span_finished(&span_id, &rpc_result);
                                    live.complete_access_deferred_with_span(
                                        accessor,
                                        accessor.getter(),
                                        HostResponseGolemRpcInvokeAndAwait {
                                            result: rpc_result
                                                .map(strip_streams)
                                                .map_err(Into::into),
                                        },
                                        finish_span,
                                    )
                                    .await
                                    .map_err(anyhow::Error::from)?
                                }
                                Some(Err(err)) => {
                                    return Err(live.trap(err));
                                }
                            }
                        }
                    }
                };

                let mut response = response;
                if let Some(streaming) = &streaming
                    && response.result.is_ok()
                    && let Some(value) = streaming
                        .streams
                        .replay_remote_result()
                        .await
                        .map_err(SessionError::into_trap)?
                {
                    response.result = Ok(value);
                }

                if delivery.is_replay_discarded() {
                    // The recorded run persisted the terminal span closure but the guest
                    // dropped this future before `get` returned. Mirror the recorded post-`End`
                    // continuation deterministically — update resident context and mark the
                    // resource consumed — then park: never return the response, so the
                    // deterministic guest drops this future at the same point it did live (its
                    // resource `drop` sees no open handle and writes nothing durable).
                    accessor.with(|mut access| finish_span_in_memory(access.get(), &span_id))?;
                    accessor.with(|mut access| {
                        let ctx = access.get();
                        let entry = ctx
                            .table()
                            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                        let state = entry
                            .payload
                            .as_any_mut()
                            .downcast_mut::<FutureInvokeResultState>()
                            .unwrap();
                        *state = FutureInvokeResultState::Consumed;
                        Ok::<_, anyhow::Error>(())
                    })?;
                    std::future::pending::<()>().await;
                    unreachable!("std::future::pending never completes")
                }

                if delivery.is_live_armed() {
                    // Live: the durable span closure is already recorded by the owned terminal
                    // task; everything left before the return is synchronous (in-memory span
                    // finish, resource transition, wire conversion), so no tear window remains
                    // between here and consuming the token.
                    let finalize = accessor.with(|mut access| {
                        let ctx = access.get();
                        finish_span_in_memory(ctx, &span_id).map_err(anyhow::Error::from)?;
                        let entry = ctx
                            .table()
                            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                        let state = entry
                            .payload
                            .as_any_mut()
                            .downcast_mut::<FutureInvokeResultState>()
                            .unwrap();
                        *state = FutureInvokeResultState::Consumed;
                        invoke_and_await_response_to_wire(response.result, ctx)
                    });
                    match finalize {
                        Ok(wire) => {
                            // The wire result returned below still crosses Wasmtime's lowering
                            // and terminal-consumption boundary: hand the token to the terminal
                            // observer instead of consuming it here.
                            delivery.deliver_at_accessor_terminal(accessor).await?;
                            Ok(wire)
                        }
                        Err(err) => {
                            // The error is observed by the caller (the worker traps): the
                            // completion was not silently discarded, so no marker.
                            delivery.suppress();
                            Err(err)
                        }
                    }
                } else {
                    // Replay of a delivered completion, or a live unpersisted (snapshotting)
                    // call: only the resident invocation context remains to be updated here.
                    accessor.with(|mut access| finish_span_in_memory(access.get(), &span_id))?;
                    accessor.with(|mut access| {
                        let ctx = access.get();
                        let entry = ctx
                            .table()
                            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                        let state = entry
                            .payload
                            .as_any_mut()
                            .downcast_mut::<FutureInvokeResultState>()
                            .unwrap();
                        *state = FutureInvokeResultState::Consumed;
                        Ok::<_, anyhow::Error>(())
                    })?;
                    let wire = accessor.with(|mut access| {
                        invoke_and_await_response_to_wire(response.result, access.get())
                    });
                    delivery.deliver_at_accessor_terminal(accessor).await?;
                    wire
                }
            }
        }
    }

    async fn drop(
        accessor: &Accessor<U, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<()> {
        let future_rep = this.rep();

        #[allow(clippy::large_enum_variant)]
        enum DropPlan {
            /// An open durable call must be cancelled and its span finished.
            Cancel {
                handle: FutureInvokeCallHandle,
                span_id: SpanId,
            },
            /// A dropped get owns the durable cancellation; release its resident span here.
            FinishInMemory { span_id: SpanId },
            /// Nothing to finish — already consumed or cancelled.
            Nothing,
        }

        let plan = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "drop");
            let entry = ctx
                .table()
                .get_mut(&Resource::<FutureInvokeResult>::new_borrow(future_rep))?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            Ok::<_, anyhow::Error>(match state {
                FutureInvokeResultState::Active {
                    handle,
                    task,
                    span_id,
                    ..
                } => match handle.take() {
                    Some(handle) => {
                        drop(task.take());
                        DropPlan::Cancel {
                            handle,
                            span_id: span_id.clone(),
                        }
                    }
                    None => DropPlan::FinishInMemory {
                        span_id: span_id.clone(),
                    },
                },
                FutureInvokeResultState::Baked { .. }
                | FutureInvokeResultState::Cancelled
                | FutureInvokeResultState::Consumed => DropPlan::Nothing,
            })
        })?;

        match plan {
            DropPlan::Cancel {
                mut handle,
                span_id,
            } => {
                handle.close_span_on_cancellation(span_id.clone());
                handle
                    .cancel_access(accessor, accessor.getter(), None)
                    .await
                    .map_err(anyhow::Error::from)?;
                accessor.with(|mut access| finish_span_in_memory(access.get(), &span_id))?;
            }
            DropPlan::FinishInMemory { span_id } => {
                accessor.with(|mut access| finish_span_in_memory(access.get(), &span_id))?;
            }
            DropPlan::Nothing => {}
        }

        accessor.with(|mut access| {
            let ctx = access.get();
            match ctx.table().delete(this) {
                Ok(entry) => {
                    for child_rep in &entry.child_pollables {
                        ctx.state.rpc_pollable_to_parent.remove(child_rep);
                    }
                }
                Err(ResourceTableError::HasChildren) => {
                    let parent: Resource<FutureInvokeResult> = Resource::new_borrow(future_rep);
                    ctx.table().get_mut(&parent)?.drop_pending = true;
                }
                Err(err) => return Err(err.into()),
            }

            Ok(())
        })
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::future-invoke-result", "cancel");

        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::future-invoke-result::cancel")
            .map_err(wasmtime::Error::from)?;

        // Decide what to cancel while holding the table borrow, taking the durable call handle out
        // of the state (a live handle must never drop while still owned here). The background task,
        // if any, is dropped together with the old `Active` state, aborting the in-flight RPC.
        #[allow(clippy::large_enum_variant)]
        enum CancelPlan {
            /// The single durable call is still open; record its `Cancelled` and, for a live call,
            /// best-effort cancel the remote invocation.
            Cancel {
                handle: FutureInvokeCallHandle,
                remote_agent_id: AgentId,
                idempotency_key: IdempotencyKey,
                span_id: SpanId,
            },
            /// Nothing to cancel: a baked failure, or already cancelled / consumed.
            Nothing,
        }

        let plan = {
            let entry = self.table().get_mut(&this)?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            match state {
                FutureInvokeResultState::Active {
                    handle,
                    request,
                    span_id,
                    cancel_token,
                    ..
                } => match handle.take() {
                    Some(handle) => {
                        let remote_agent_id = request.remote_agent_id.clone();
                        let idempotency_key = request.idempotency_key.clone();
                        let span_id = span_id.clone();
                        *state = FutureInvokeResultState::Cancelled;
                        CancelPlan::Cancel {
                            handle,
                            remote_agent_id,
                            idempotency_key,
                            span_id,
                        }
                    }
                    None => {
                        // An in-flight `get` owns the handle. Signal it through the shared
                        // token: the racing `get` aborts the RPC, records the `Cancelled`
                        // terminal (with the partial "Invocation cancelled" response), and
                        // finishes the span, so nothing durable is written here.
                        cancel_token.cancel();
                        CancelPlan::Nothing
                    }
                },
                FutureInvokeResultState::Baked { .. }
                | FutureInvokeResultState::Cancelled
                | FutureInvokeResultState::Consumed => CancelPlan::Nothing,
            }
        };

        if let CancelPlan::Cancel {
            mut handle,
            remote_agent_id,
            idempotency_key,
            span_id,
        } = plan
        {
            handle.close_span_on_cancellation(span_id.clone());
            // Best-effort remote cancellation, only meaningful for a live call — on replay the
            // recorded `Cancelled` is re-applied without re-issuing the side effect.
            if handle.is_live() {
                match self.try_agent_auth_ctx_at_boundary().await {
                    Ok(Some(auth_ctx)) => {
                        if let Err(err) = self
                            .worker_proxy()
                            .cancel_invocation(&remote_agent_id, idempotency_key, &auth_ctx)
                            .await
                        {
                            tracing::info!(err=%err, "Best-effort cancel_invocation failed");
                        }
                    }
                    Ok(None) => {
                        tracing::debug!(
                            "Skipping best-effort cancel_invocation while card authority recovery is pending"
                        );
                    }
                    Err(err) => {
                        tracing::info!(err=%err, "Skipping best-effort cancel_invocation after wallet synchronization failed");
                    }
                }
            }

            handle
                .cancel(self, None)
                .await
                .map_err(anyhow::Error::from)?;
            finish_span_in_memory(self, &span_id)?;
        }

        Ok(())
    }
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        // The p3 `future-invoke-result` resource exposes no `subscribe`, so this pollable-readiness
        // hook is never driven: `get` is the sole consumer and awaits the background task directly.
        // Kept only to satisfy the `SubscribeAny` bound of the resource-table payload.
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
