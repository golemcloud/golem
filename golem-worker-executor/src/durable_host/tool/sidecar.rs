// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::attachment::AttachmentWriter;
use super::{
    AsContextMut, AttachmentController, ByteStreamFailure, DurabilityHost,
    DurableToolStdinProducer, EntityBodyOutcome, EntityCancellationEpochTask, EntityInvocationBody,
    GuestCallSettlementError, HostResponseEntityInvocation, InterruptKind, InvokeResult,
    SerializableEntityBodyExecution, SerializableToolInvocationResult,
    SerializableToolOperationTerminal, SerializableToolRpcError, Store, StreamReader,
    ToolBodyStdin, ToolOutputContract, ToolOutputWriterEntry, ToolSidecarBody,
    ToolSidecarInvocation, UnderlyingToolEntry, UnderlyingToolRevocationGuard, WitTool, WorkerCtx,
    WorkerExecutorError, decode_guest_tool_error, decode_typed_tool_value, durability,
    encode_tool_operation_terminal, encode_tool_terminal, encode_typed_tool_value,
    finish_invocation_and_get_fuel_consumption, first_output_limit_error,
    guest_body_accepts_admission_rejection, guest_trap_output_failure, input, operation,
    permanent_tool_guest_trap, prepare_guest_call, prepare_tool_parent_end,
    rollback_live_attachment_admission_rejection, streams, tool_guest_exports,
    tool_middleware_guest_exports, validate_declared_tool_error, validate_declared_tool_result,
};
use crate::durable_host::attribution::Attributed;
use crate::durable_host::stream_outcome::supervise_schema_outputs;
use crate::worker::invocation::{
    FailureCauses, SettlementDeadline, run_guest_call_settled, run_guest_call_settled_at,
    run_tool_guest_call,
};
use futures::FutureExt;
use wasmtime::component::Accessor;

async fn replay_disposal_termination<T>(
    replay: crate::durable_host::replay_state::ReplayState,
    operation: operation::OwnerToolOperation,
    body: impl Future<Output = wasmtime::Result<T>>,
) -> wasmtime::Result<T> {
    let result = tokio::select! {
        biased;
        error = replay.await_disposal_admission_refusal(operation.clone()) => {
            return Err(wasmtime::Error::from_anyhow(error.into()));
        }
        result = body => result,
    };
    if result.is_ok()
        && let Some(refusal) = replay
            .disposal_admission_refusal(&operation)
            .await
            .map_err(|error| wasmtime::Error::from_anyhow(error.into()))?
    {
        return Err(wasmtime::Error::from_anyhow(
            operation.reproduce_admission_refusal(refusal).await.into(),
        ));
    }
    result
}

/// The `ProcessExitStatus` result of a tool body that ended with `wasi:cli/exit`, when the tool
/// declares that result.
fn process_exit_status(
    result: &Result<wasmtime::Result<HostResponseEntityInvocation>, GuestCallSettlementError>,
    contract: &ToolOutputContract,
) -> Option<golem_common::schema::TypedSchemaValue> {
    use golem_common::schema::IntoTypedSchemaValue;

    let (Ok(Err(error)) | Err(GuestCallSettlementError::Trap(error))) = result else {
        return None;
    };
    let code = error.causes::<wasmtime_wasi::I32Exit>().next()?.0;
    let status = golem_common::schema::tool::ProcessExitStatus {
        exit_code: u8::try_from(code).unwrap_or(u8::MAX),
    }
    .into_typed_schema_value()
    .ok()?;
    let declared = contract.result.as_ref()?;
    golem_common::schema::validation::is_equivalent_cross_graph(
        status.graph(),
        &status.graph().root,
        declared,
        &declared.root,
    )
    .then_some(status)
}

/// Completes an exited tool body as a native process exit does: the output writers the body still
/// holds finish, and the exit code becomes the returned status. The trapped Store only runs host
/// work here.
async fn finish_process_exit<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    status: golem_common::schema::TypedSchemaValue,
    writers: [Option<(AttachmentWriter, AttachmentController)>; 2],
    output_contract: &ToolOutputContract,
) -> wasmtime::Result<HostResponseEntityInvocation> {
    use golem_common::model::oplog::host_functions::GolemToolOutputFinish;
    use wasmtime::component::HasSelf;
    type Writers<Ctx> = HasSelf<crate::durable_host::DurableWorkerCtx<Ctx>>;

    let attribution = store.data().durable_ctx().attribution.clone();
    let invocation_key = store.data().durable_ctx().invocation_key();
    store
        .as_context_mut()
        .run_concurrent(async move |accessor| {
            let accessor = accessor.with_getter::<Writers<Ctx>>(|ctx| ctx.durable_ctx_mut());
            for (writer, controller) in writers.into_iter().flatten() {
                // A writer the guest dropped keeps the terminal its disposal selected. One it
                // still holds finishes; finish reports an earlier terminal as closed.
                if !controller.producer_active() {
                    continue;
                }
                let ordinal = writer.next_operation_ordinal();
                let _ = attribution
                    .scoped(
                        Some(invocation_key),
                        durability::finish::<GolemToolOutputFinish, _, _>(
                            &accessor, writer, controller, ordinal,
                        ),
                    )
                    .await
                    .map_err(wasmtime::Error::from_anyhow)?;
            }
            Ok::<_, wasmtime::Error>(())
        })
        .await??;
    let ctx = store.data_mut().durable_ctx_mut();
    let result = encode_typed_tool_value(&status, ctx).map_err(wasmtime::Error::msg)?;
    ctx.attribution
        .clone()
        .scoped(
            Some(invocation_key),
            materialize_returned_tool_response(
                ctx,
                Ok(golem_common::schema::tool::wit::wire::InvocationResult {
                    result: Some(result),
                    stdout: None,
                    stderr: None,
                }),
                output_contract,
                false,
            ),
        )
        .await
}

impl<Ctx: WorkerCtx> EntityInvocationBody<Ctx, HostResponseEntityInvocation> for ToolSidecarBody {
    fn invoke<'a>(
        self,
        instance: &'a wasmtime::component::Instance,
        store: &'a mut Store<Ctx>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        EntityBodyOutcome<HostResponseEntityInvocation>,
                        WorkerExecutorError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let result = invoke_tool_sidecar(
                instance,
                store,
                self.invocation,
                self.operation,
                self.cancellation,
            )
            .await;
            store.data().durable_ctx().entity_body_finalization.begin();
            if result.is_err() {
                durability::abort_producer_disposals(store.data_mut().durable_ctx_mut()).await;
            }
            result
        })
    }
}

impl Drop for EntityCancellationEpochTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for UnderlyingToolRevocationGuard {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

async fn materialize_returned_tool_response<Ctx: WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    result: Result<
        golem_common::schema::tool::wit::wire::InvocationResult,
        golem_common::schema::tool::wit::wire::ToolError,
    >,
    output_contract: &ToolOutputContract,
    allow_undeclared_custom_error: bool,
) -> wasmtime::Result<HostResponseEntityInvocation> {
    let scope = ctx
        .tool_frontends
        .as_ref()
        .expect("tool Store has a frontend scope")
        .clone();
    scope.begin_return();
    let _retirement = scope.retirement();
    ctx.settle_schema_producers(Ok(()));
    let response = match result {
        Ok(result) => {
            if result.stdout.is_some() || result.stderr.is_some() {
                return Err(wasmtime::Error::msg(
                    "tool guest returned attachment streams instead of using the supplied output writers",
                ));
            }
            let value = result
                .result
                .map(|value| decode_typed_tool_value(value, ctx))
                .transpose()
                .map_err(|_| {
                    wasmtime::Error::msg("tool guest returned an invalid result payload")
                })?;
            validate_declared_tool_result(value, output_contract).map(|result| {
                SerializableToolInvocationResult {
                    result: result.map(Box::new),
                }
            })
        }
        Err(error) => Err(validate_declared_tool_error(
            decode_guest_tool_error(error, ctx),
            output_contract,
            allow_undeclared_custom_error,
        )),
    };
    super::host::abandon_tool_writers(ctx).map_err(wasmtime::Error::from_anyhow)?;
    let interrupt = ctx.create_interrupt_signal();
    let closing = scope.finish_returned_frontends();
    tokio::select! {
        biased;
        kind = interrupt => Err(wasmtime::Error::from_anyhow(kind.into())),
        result = async {
            let finalization = async {
                durability::finish_producer_disposals(ctx).await.map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                let response = response.map(|result| super::SerializableToolStructuredResult {
                    result: result
                        .result
                        .as_deref()
                        .map(super::SerializableToolResultValue::from_typed)
                        .transpose()
                        .expect("a validated non-stream tool result is serializable"),
                });
                let response = encode_tool_operation_terminal(response)
                    .await.map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                scope.finish_recording().await;
                Ok(response)
            };
            tokio::pin!(finalization);
            tokio::select! {
                result = &mut finalization => result,
                _ = closing => finalization.await,
            }
        } => result,
    }
}

pub(super) async fn materialize_guest_tool_response<Ctx: WorkerCtx>(
    accessor: &Accessor<Ctx>,
    result: Result<
        golem_common::schema::tool::wit::wire::InvocationResult,
        golem_common::schema::tool::wit::wire::ToolError,
    >,
    output_contract: &ToolOutputContract,
    allow_undeclared_custom_error: bool,
    result_streams: &crate::durable_host::durable_session::StreamSession,
) -> wasmtime::Result<HostResponseEntityInvocation> {
    let response = accessor
        .with(
            |mut access| -> Result<super::ToolInvokeResponse, WorkerExecutorError> {
                let ctx = access.data_mut().durable_ctx_mut();
                Ok(match result {
                    Ok(result) => {
                        if result.stdout.is_some() || result.stderr.is_some() {
                            return Err(WorkerExecutorError::runtime(
                                "tool guest returned attachment streams instead of using the supplied output writers",
                            ));
                        }
                        let value = result
                            .result
                            .map(|value| decode_typed_tool_value(value, ctx))
                            .transpose()
                            .map_err(|_| {
                                WorkerExecutorError::runtime(
                                    "tool guest returned an invalid result payload",
                                )
                            })?;
                        validate_declared_tool_result(value, output_contract).map(|result| {
                            SerializableToolInvocationResult {
                                result: result.map(Box::new),
                            }
                        })
                    }
                    Err(error) => Err(validate_declared_tool_error(
                        decode_guest_tool_error(error, ctx),
                        output_contract,
                        allow_undeclared_custom_error,
                    )),
                })
            },
        )
        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
    let (completed, completion) = super::oneshot::channel();
    accessor.with(|mut access| {
        let ctx = access.data_mut().durable_ctx_mut();
        let activity = ctx.tail_work_tracker().activity();
        let revision = ctx.component_metadata().revision;
        let output_tasks = ctx.schema_output_spawner()?;
        let captured = ctx.attribution.capture();
        let task = streams::MaterializeResponse {
            streams: result_streams.clone().with_runtime_source(
                ctx.runtime_suspension
                    .as_ref()
                    .map(|runtime| runtime.source()),
            ),
            revision,
            response,
            completed,
            activity,
            output_tasks,
        };
        access.spawn(async move |access| captured.run(task.run(access)).await)
    })?;
    completion
        .await
        .map_err(|_| wasmtime::Error::msg("tool result materialization task stopped"))?
        .map_err(|error| wasmtime::Error::msg(error.to_string()))
}

/// The tool that a middleware layer wraps: the last layer of its plan.
fn wrapped_tool(
    position: &crate::durable_host::entity::ResolvedEntityInvocationPosition,
) -> Option<golem_common::model::tool::CompiledToolBinding> {
    let plan = position.plan();
    let leaf = plan
        .layer(u32::try_from(plan.len().checked_sub(1)?).ok()?)
        .ok()?;
    match leaf.activation().policy() {
        golem_common::model::entity::EntityActivationPolicy::Tool { binding, .. } => {
            Some(binding.as_ref().clone())
        }
        golem_common::model::entity::EntityActivationPolicy::ToolMiddleware { .. } => None,
    }
}

async fn run_tool_command<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    call: impl AsyncFnOnce(
        &Accessor<Ctx>,
    ) -> wasmtime::Result<
        Result<
            golem_common::schema::tool::wit::wire::InvocationResult,
            golem_common::schema::tool::wit::wire::ToolError,
        >,
    >,
    deadline: SettlementDeadline,
    output_contract: &ToolOutputContract,
    allow_undeclared_custom_error: bool,
    result_streams: &crate::durable_host::durable_session::StreamSession,
) -> Result<wasmtime::Result<HostResponseEntityInvocation>, GuestCallSettlementError> {
    match deadline {
        SettlementDeadline::RootReturned => match run_tool_guest_call(store, call).await {
            Ok(Ok(result)) => {
                let drain_started = std::sync::Arc::new(tokio::sync::Notify::new());
                let _deadline = store
                    .data()
                    .durable_ctx()
                    .arm_tail_work_deadline(drain_started.clone());
                drain_started.notify_one();
                let ctx = store.data_mut().durable_ctx_mut();
                Ok(ctx
                    .attribution
                    .clone()
                    .scoped(
                        Some(ctx.invocation_key()),
                        materialize_returned_tool_response(
                            ctx,
                            result,
                            output_contract,
                            allow_undeclared_custom_error,
                        ),
                    )
                    .await)
            }
            Ok(Err(error)) => Ok(Err(error)),
            Err(error) => Err(error),
        },
        SettlementDeadline::GuestTasksExited | SettlementDeadline::AfterResult => {
            run_guest_call_settled_at(
                store,
                async move |accessor| {
                    let result = call(accessor).await?;
                    materialize_guest_tool_response(
                        accessor,
                        result,
                        output_contract,
                        allow_undeclared_custom_error,
                        result_streams,
                    )
                    .await
                },
                deadline,
            )
            .await
        }
    }
}

pub(super) async fn invoke_tool_sidecar<Ctx: WorkerCtx>(
    instance: &wasmtime::component::Instance,
    store: &mut Store<Ctx>,
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
) -> Result<EntityBodyOutcome<HostResponseEntityInvocation>, WorkerExecutorError> {
    let ToolSidecarInvocation {
        tool_name,
        position,
        command_path,
        input,
        stdin,
        stdout,
        stderr,
        caller,
        output_contract,
        result_streams,
    } = invocation;
    let mut store = store.as_context_mut();
    store
        .data_mut()
        .durable_ctx_mut()
        .set_entity_tool_operation(operation.clone())?;
    let cancellation_epoch = if let Some(cancellation) = cancellation {
        store
            .data_mut()
            .durable_ctx_mut()
            .set_entity_cancellation(cancellation.clone())?;
        let engine = store.engine().clone();
        Some(EntityCancellationEpochTask(tokio::spawn(async move {
            cancellation.cancelled().await;
            engine.increment_epoch();
        })))
    } else {
        None
    };
    let input = encode_typed_tool_value(&input, store.data_mut().durable_ctx_mut())
        .map_err(WorkerExecutorError::runtime)?;
    let stdout_controller = stdout.as_ref().map(ToolOutputWriterEntry::controller);
    let stderr_controller = stderr.as_ref().map(ToolOutputWriterEntry::controller);
    let output_writers = [stdout.as_ref(), stderr.as_ref()]
        .map(|entry| entry.map(|entry| (entry.producer.writer(), entry.controller())));
    let stdout_observer = stdout_controller
        .as_ref()
        .map(AttachmentController::observer);
    let stderr_observer = stderr_controller
        .as_ref()
        .map(AttachmentController::observer);
    let stdout = stdout
        .map(|stdout| store.data_mut().durable_ctx_mut().table().push(stdout))
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let stderr = stderr
        .map(|stderr| store.data_mut().durable_ctx_mut().table().push(stderr))
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let display_name = match position.layer() {
        golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
            activation, ..
        } => {
            format!(
                "golem:tool/tool-middleware-guest.invoke-tool-middleware({})",
                activation.entity().name()
            )
        }
        golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => {
            format!("golem:tool/guest.invoke({tool_name})")
        }
    };
    prepare_guest_call(&mut store, &display_name).await;
    let stdin = stdin
        .map(|stdin| -> anyhow::Result<_> {
            match stdin {
                ToolBodyStdin::Attachment(stdin)
                    if stdin.controller().observer().identity().is_some() =>
                {
                    let input = input::AttachmentInputProducer::new(
                        stdin.consumer,
                        |ctx: &mut Ctx| ctx.durable_ctx_mut(),
                        store.data().durable_ctx(),
                    )?;
                    let input =
                        Attributed::new(store.data().durable_ctx().attribution.clone(), input);
                    Ok(StreamReader::new(store.as_context_mut(), input)?)
                }
                ToolBodyStdin::Attachment(stdin) => Ok(StreamReader::new(
                    store.as_context_mut(),
                    stdin.into_stream_producer(),
                )?),
                ToolBodyStdin::Durable(endpoint) => {
                    let runtime_teardown =
                        store.data().durable_ctx().stream_runtime_teardown_probe();
                    let input = Attributed::new(
                        store.data().durable_ctx().attribution.clone(),
                        DurableToolStdinProducer::new(*endpoint, runtime_teardown),
                    );
                    Ok(StreamReader::new(store.as_context_mut(), input)?)
                }
            }
        })
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_caller(Some(caller));
    if let golem_common::model::entity::EntityInvocationPlanLayer::Middleware { .. } =
        position.layer()
    {
        store
            .data_mut()
            .durable_ctx_mut()
            .set_middleware_wrapped_tool(wrapped_tool(&position));
    }
    let parent = store.data().durable_ctx().owner_invocation_id()?;
    let instance_pre = instance.instance_pre(&store);
    let replay = store.data().durable_ctx().state.replay_state.clone();
    let replay_operation = operation.clone();
    let output_watch = tokio_util::sync::CancellationToken::new();
    let output_watch_guard = output_watch.clone().drop_guard();
    let output_monitor = store.data().durable_ctx().schema_output_drain_monitor()?;
    let output_runtime = store.data().durable_ctx().runtime_suspension.clone();
    let deadline = if output_contract
        .result
        .as_ref()
        .is_some_and(|graph| graph.contains_stream_type())
        || output_contract
            .errors
            .iter()
            .any(|error| error.payload.contains_stream_type())
        || matches!(
            position.layer(),
            golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
                presented_definition: None,
                ..
            }
        ) {
        SettlementDeadline::GuestTasksExited
    } else {
        SettlementDeadline::RootReturned
    };
    if matches!(deadline, SettlementDeadline::GuestTasksExited) {
        store.data_mut().durable_ctx_mut().tool_frontends = None;
    }
    let attribution = store.data().durable_ctx().attribution.clone();
    let invocation_key = store.data().durable_ctx().invocation_key();
    let tool_body_writers = match position.layer() {
        golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => Some(output_writers),
        _ => None,
    };
    let mut result = match position.layer() {
        golem_common::model::entity::EntityInvocationPlanLayer::Tool { .. } => {
            let indices =
                tool_guest_exports::GuestIndices::new(&instance_pre).map_err(|error| {
                    WorkerExecutorError::invalid_request(format!(
                        "tool guest export not available: {error}"
                    ))
                })?;
            let guest = indices.load(&mut store, instance).map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "failed to load tool guest export: {error}"
                ))
            })?;
            run_tool_command(
                &mut store,
                async move |accessor| {
                    accessor.with(|mut access| {
                        supervise_schema_outputs(
                            &mut access,
                            output_monitor,
                            output_runtime,
                            output_watch,
                        )
                    })?;
                    replay_disposal_termination(
                        replay,
                        replay_operation,
                        attribution.armed(
                            invocation_key,
                            guest.call_invoke(
                                accessor,
                                tool_name,
                                command_path,
                                input,
                                stdin,
                                stdout,
                                stderr,
                            ),
                        ),
                    )
                    .await
                },
                deadline,
                &output_contract,
                false,
                &result_streams,
            )
            .await
        }
        golem_common::model::entity::EntityInvocationPlanLayer::Middleware {
            activation,
            parameters,
            presented_definition,
            next_effective_definition,
            ..
        } => {
            let middleware_name = activation.entity().name().to_string();
            let metadata = WitTool::try_from(next_effective_definition).map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to encode middleware tool metadata: {error}"
                ))
            })?;
            let parameters =
                encode_typed_tool_value(parameters, store.data_mut().durable_ctx_mut())
                    .map_err(WorkerExecutorError::runtime)?;
            let underlying = UnderlyingToolEntry::new(position.clone());
            let revoke = underlying.clone();
            let wrapped = store
                .data_mut()
                .durable_ctx_mut()
                .table()
                .push(underlying)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            let _revoke_on_exit = UnderlyingToolRevocationGuard(revoke.clone());
            let indices = tool_middleware_guest_exports::GuestIndices::new(&instance_pre).map_err(
                |error| {
                    WorkerExecutorError::invalid_request(format!(
                        "tool middleware guest export not available: {error}"
                    ))
                },
            )?;
            let guest = indices.load(&mut store, instance).map_err(|error| {
                WorkerExecutorError::invalid_request(format!(
                    "failed to load tool middleware guest export: {error}"
                ))
            })?;
            run_tool_command(
                &mut store,
                async move |accessor| {
                    accessor.with(|mut access| {
                        supervise_schema_outputs(
                            &mut access,
                            output_monitor,
                            output_runtime,
                            output_watch,
                        )
                    })?;
                    let result = replay_disposal_termination(
                        replay,
                        replay_operation,
                        attribution.armed(
                            invocation_key,
                            guest.call_invoke_tool_middleware(
                                accessor,
                                middleware_name,
                                tool_name,
                                metadata,
                                parameters,
                                command_path,
                                input,
                                stdin,
                                stdout,
                                stderr,
                                wrapped,
                            ),
                        ),
                    )
                    .await;
                    revoke.revoke();
                    result
                },
                deadline,
                &output_contract,
                presented_definition.is_none(),
                &result_streams,
            )
            .await
        }
    };
    drop(output_watch_guard);
    let guest_returned = store
        .data()
        .durable_ctx()
        .entity_body_finalization
        .guest_returned();
    store.data().durable_ctx().entity_body_finalization.begin();
    let finish = finish_invocation_and_get_fuel_consumption(&mut store, &display_name).await;
    if let Some(error) = store
        .data()
        .durable_ctx()
        .schema_output_drain_monitor()?
        .wait_for_failure()
        .now_or_never()
    {
        return Err(error);
    }
    let locally_cancelled = match &result {
        Ok(Err(error))
        | Err(GuestCallSettlementError::Interrupted(error))
        | Err(GuestCallSettlementError::Trap(error)) => {
            crate::worker::invocation::interrupt_kind_in(error)
                == Some(InterruptKind::EntityCancellation)
        }
        _ => false,
    };
    if locally_cancelled && !operation.has_pending_live_admission_rejection().await {
        finish?;
        operation.claim_local_cancellation_interruption();
        drop(cancellation_epoch);
        return Ok(EntityBodyOutcome::Cancelled);
    }
    if matches!(deadline, SettlementDeadline::RootReturned)
        && let Some(writers) = tool_body_writers
        && let Some(status) = process_exit_status(&result, &output_contract)
        && store
            .data()
            .durable_ctx()
            .owner_execution
            .tool_operations()
            .selected_owner_failure()
            .is_none()
        && !operation.has_pending_live_admission_rejection().await
    {
        result = match finish_process_exit(&mut store, status, writers, &output_contract).await {
            Ok(response) => Ok(Ok(response)),
            Err(error) => Err(GuestCallSettlementError::Trap(error)),
        };
    }
    let permanent_trap = match &result {
        Ok(Err(error)) | Err(GuestCallSettlementError::Trap(error)) => {
            permanent_tool_guest_trap(error)
        }
        _ => None,
    };
    if let Some(details) = permanent_trap
        && store
            .data()
            .durable_ctx()
            .owner_execution
            .tool_operations()
            .selected_owner_failure()
            .is_none()
        && !operation.has_pending_live_admission_rejection().await
    {
        finish?;
        store
            .data_mut()
            .durable_ctx_mut()
            .settle_schema_producers(Err(details.clone()));
        // A successful encoded body terminal requests Store destruction in the entity runner.
        // Its original host context, not the poisoned guest scheduler, drains admitted work.
        let response = encode_tool_terminal(
            SerializableToolOperationTerminal {
                body_execution: SerializableEntityBodyExecution::Trapped,
                admission_refusal: None,
                result: Err(SerializableToolRpcError::RemoteInternalError(
                    details.clone(),
                )),
            },
            "failed to encode trapped tool terminal",
        )
        .await?;
        store
            .data()
            .durable_ctx()
            .owner_execution
            .tool_operations()
            .cancel_children(&parent);
        for output in [stdout_controller.as_ref(), stderr_controller.as_ref()]
            .into_iter()
            .flatten()
        {
            output.reserve_owner_terminal(ByteStreamFailure::Failed(details.clone()));
        }
        drop(cancellation_epoch);
        return Ok(EntityBodyOutcome::Completed(response));
    }
    if matches!(deadline, SettlementDeadline::GuestTasksExited)
        && matches!(&result, Ok(Ok(_)))
        && finish.is_ok()
        && !operation.has_pending_live_admission_rejection().await
    {
        store
            .data_mut()
            .durable_ctx_mut()
            .settle_schema_producers(Ok(()));
        match run_guest_call_settled(&mut store, async move |accessor| {
            crate::durable_host::stream_outcome::drain_schema_outputs(accessor).await
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error @ WorkerExecutorError::Interrupted { .. })) => {
                result = Err(GuestCallSettlementError::Interrupted(wasmtime::Error::new(
                    error,
                )));
            }
            Ok(Err(error)) => return Err(error),
            Err(error) => result = Err(error),
        }
    }
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_caller(None);
    if !matches!(&result, Ok(Ok(_))) {
        durability::abort_producer_disposals(store.data_mut().durable_ctx_mut()).await;
    }
    let mut parent_end = if guest_returned {
        let ctx = store.data_mut().durable_ctx_mut();
        ctx.attribution
            .clone()
            .scoped(
                Some(ctx.invocation_key()),
                durability::finish_producer_disposals(ctx),
            )
            .await?;
        super::prepare_tool_parent_end_owned(&ctx.owner_execution, parent).await
    } else {
        let errors = prepare_tool_parent_end(&mut store, parent).await;
        durability::native_finalization_failures(store.data().durable_ctx(), errors).await
    };
    if matches!(&result, Ok(Ok(_))) && finish.is_ok() && parent_end.is_ok() {
        let attribution = store.data().durable_ctx().attribution.clone();
        let invocation_key = store.data().durable_ctx().invocation_key();
        parent_end = if guest_returned {
            attribution
                .scoped(
                    Some(invocation_key),
                    durability::finish_producer_disposals(store.data_mut().durable_ctx_mut()),
                )
                .await
        } else {
            store
                .as_context_mut()
                .run_concurrent(async move |accessor| {
                    attribution
                        .scoped(
                            Some(invocation_key),
                            durability::finish_producer_disposals(
                                &mut crate::durable_host::concurrent::AccessorContext::new(
                                    accessor,
                                    |ctx| ctx.durable_ctx_mut(),
                                ),
                            ),
                        )
                        .await
                })
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        };
    }
    drop(cancellation_epoch);
    if let Err(
        error @ (WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost,
        }
        | WorkerExecutorError::OplogFenced { .. }),
    ) = &parent_end
    {
        return Err(error.clone());
    }
    if operation.has_pending_live_admission_rejection().await {
        for error in [finish.as_ref().err(), parent_end.as_ref().err()]
            .into_iter()
            .flatten()
        {
            if !crate::durable_host::is_tool_attachment_live_admission_rejection(error) {
                return Err(error.clone());
            }
        }
        if guest_body_accepts_admission_rejection(&result) {
            return Err(crate::durable_host::tool_attachment_live_admission_rejected_error());
        }
        rollback_live_attachment_admission_rejection(&operation).await?;
    }
    finish?;
    if matches!(&result, Ok(Ok(_))) || locally_cancelled {
        parent_end?;
    }

    match result {
        Ok(Ok(result)) => {
            if let Some(error) =
                first_output_limit_error(stdout_observer.as_ref(), stderr_observer.as_ref())
            {
                return encode_tool_operation_terminal(Err(error))
                    .await
                    .map(EntityBodyOutcome::Completed);
            }
            Ok(EntityBodyOutcome::Completed(result))
        }
        Ok(Err(error))
        | Err(GuestCallSettlementError::Interrupted(error))
        | Err(GuestCallSettlementError::Trap(error)) => {
            let error = store
                .data()
                .durable_ctx()
                .linear_memory_tracker()
                .annotate_gc_heap_error(error);
            let error: anyhow::Error = error.into();
            if durability::is_entity_cancellation(&error) {
                operation.claim_local_cancellation_interruption();
                return Ok(EntityBodyOutcome::Cancelled);
            }
            let trap = InvokeResult::from_error::<Ctx>(
                0,
                &error,
                store.data().get_current_retry_point().await,
                store.data().agent_mode(),
            )
            .as_trap_type::<Ctx>()
            .expect("a failed tool guest call must classify as a trap");
            let output_failure = guest_trap_output_failure(&operation, trap.clone()).await;
            if let Some(stdout) = stdout_controller {
                let _ = stdout.host_fail(output_failure.clone());
            }
            if let Some(stderr) = stderr_controller {
                let _ = stderr.host_fail(output_failure);
            }
            Err(trap
                .as_golem_error("")
                .unwrap_or_else(|| WorkerExecutorError::runtime("tool sidecar was interrupted")))
        }
        Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
    }
}
