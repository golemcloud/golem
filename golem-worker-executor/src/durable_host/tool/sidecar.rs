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

use super::{
    Accessor, AsContextMut, AttachmentController, ByteStreamFailure, DurableToolStdinProducer,
    EntityBodyOutcome, EntityCancellationEpochTask, EntityInvocationBody, GuestCallSettlementError,
    HostResponseEntityInvocation, InterruptKind, InvokeResult, SerializableEntityBodyExecution,
    SerializableToolInvocationResult, SerializableToolOperationTerminal, SerializableToolRpcError,
    Store, StreamReader, ToolBodyStdin, ToolInvokeResponse, ToolOutputContract,
    ToolOutputWriterEntry, ToolSidecarBody, ToolSidecarInvocation, UnderlyingToolEntry,
    UnderlyingToolRevocationGuard, WitTool, WorkerCtx, WorkerExecutorError,
    decode_guest_tool_error, decode_typed_tool_value, durability, encode_tool_operation_terminal,
    encode_tool_terminal, encode_typed_tool_value, finish_invocation_and_get_fuel_consumption,
    first_output_limit_error, guest_body_accepts_admission_rejection, guest_trap_output_failure,
    input, oneshot, operation, permanent_tool_guest_trap, prepare_guest_call,
    prepare_tool_parent_end, rollback_live_attachment_admission_rejection, run_guest_call_settled,
    streams, tool_guest_exports, tool_middleware_guest_exports, validate_declared_tool_error,
    validate_declared_tool_result,
};

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
            |mut access| -> Result<ToolInvokeResponse, WorkerExecutorError> {
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
    let (completed, completion) = oneshot::channel();
    accessor.with(|mut access| {
        let ctx = access.data_mut().durable_ctx_mut();
        let activity = ctx.tail_work_tracker().activity();
        let revision = ctx.component_metadata().revision;
        let task = streams::MaterializeResponse {
            streams: result_streams.clone(),
            revision,
            response,
            completed,
            activity,
        };
        access.spawn(async move |access| task.run(access).await)
    })?;
    completion
        .await
        .map_err(|_| wasmtime::Error::msg("tool result materialization task stopped"))?
        .map_err(|error| wasmtime::Error::msg(error.to_string()))
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
        principal,
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
                    Ok(StreamReader::new(store.as_context_mut(), input)?)
                }
                ToolBodyStdin::Attachment(stdin) => Ok(StreamReader::new(
                    store.as_context_mut(),
                    stdin.into_stream_producer(),
                )?),
                ToolBodyStdin::Durable(endpoint) => {
                    let runtime_teardown =
                        store.data().durable_ctx().stream_runtime_teardown_probe();
                    Ok(StreamReader::new(
                        store.as_context_mut(),
                        DurableToolStdinProducer::new(*endpoint, runtime_teardown),
                    )?)
                }
            }
        })
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(Some(principal.clone()));
    let principal = principal.into();
    let parent = store.data().durable_ctx().owner_invocation_id()?;
    let instance_pre = instance.instance_pre(&store);
    let replay = store.data().durable_ctx().state.replay_state.clone();
    let replay_operation = operation.clone();
    let result = match position.layer() {
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
            run_guest_call_settled(&mut store, async move |accessor| {
                let result = replay_disposal_termination(
                    replay,
                    replay_operation,
                    guest.call_invoke(
                        accessor,
                        tool_name,
                        command_path,
                        input,
                        stdin,
                        stdout,
                        stderr,
                        principal,
                    ),
                )
                .await?;
                materialize_guest_tool_response(
                    accessor,
                    result,
                    &output_contract,
                    false,
                    &result_streams,
                )
                .await
            })
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
            run_guest_call_settled(&mut store, async move |accessor| {
                let result = replay_disposal_termination(
                    replay,
                    replay_operation,
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
                        principal,
                        wrapped,
                    ),
                )
                .await;
                revoke.revoke();
                materialize_guest_tool_response(
                    accessor,
                    result?,
                    &output_contract,
                    presented_definition.is_none(),
                    &result_streams,
                )
                .await
            })
            .await
        }
    };
    store.data().durable_ctx().entity_body_finalization.begin();
    let finish = finish_invocation_and_get_fuel_consumption(&mut store, &display_name).await;
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
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(None);
    if !matches!(&result, Ok(Ok(_))) {
        durability::abort_producer_disposals(store.data_mut().durable_ctx_mut()).await;
    }
    let errors = prepare_tool_parent_end(&mut store, parent).await;
    let mut parent_end =
        durability::native_finalization_failures(store.data().durable_ctx(), errors).await;
    if matches!(&result, Ok(Ok(_))) && finish.is_ok() && parent_end.is_ok() {
        parent_end = store
            .as_context_mut()
            .run_concurrent(async move |accessor| {
                durability::finish_producer_disposals(
                    &mut crate::durable_host::concurrent::AccessorContext::new(accessor, |ctx| {
                        ctx.durable_ctx_mut()
                    }),
                )
                .await
            })
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
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
        rollback_live_attachment_admission_rejection(&operation).await;
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
