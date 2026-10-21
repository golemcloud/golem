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
    Accessor, AccessorTask, Arc, AttachmentController, ByteStreamFailure, DurabilityHost,
    DurableInputEndpoint, DurableStreamHandle, DurableWorkerCtx, EntityCallMode,
    EntityInvocationKeyContext, FromSchema, Future, GuestCallSettlementError, HasNativeToolCatalog,
    HasSelf, HasWorker, HostResponseEntityInvocation, IntoTypedSchemaValue, LiveStreamEndpoint,
    NativeToolInvocation, NativeToolKey, NativeToolTask, Resource, RetainedEntityStore,
    RetainedNativeContext, SchemaGraph, SchemaType, SchemaValueStream,
    SerializableEntityBodyExecution, SerializableToolOperationTerminal, SerializableToolRpcError,
    SessionError, SessionStreamRole, StreamReader, StreamSession, ToolAttachmentCounterparty,
    ToolBodyStdin, ToolCallDispatch, ToolCallStdin, ToolCallTarget, ToolInvocationAttempt,
    ToolInvocationInput, ToolInvocationOutput, ToolOutputWriterEntry, ToolSidecarInvocation, Weak,
    WorkerCtx, WorkerCtxExecutable, WorkerExecutorError, accounted_byte_output_stream_pair,
    admit_tool_response_secret_holds, anyhow, cleanup_failed_tool_dispatch, close_stdin,
    create_native_underlying_stdin, create_output_attachment, dispatch_tool_attempt, durability,
    encode_tool_operation_terminal, encode_tool_terminal, execute_accepted_tool_call,
    first_output_limit_error, mcp, native_output_handles, operation, register_tool_admission,
    reject_output, tool_rpc_for_current_owner, validate_native_tool_output,
};

pub(super) async fn invoke_native_tool<Ctx: WorkerCtx>(
    worker: Arc<crate::worker::Worker<Ctx>>,
    owner_component_metadata: Arc<golem_service_base::model::component::Component>,
    scope: golem_common::model::entity::EntityInvocationScope,
    registration: &crate::worker::entity_slot::EntitySlotRegistration,
    invocation: ToolSidecarInvocation,
    operation: operation::OwnerToolOperation,
    cancellation: Option<tokio_util::sync::CancellationToken>,
    runner_abort: tokio_util::sync::CancellationToken,
) -> (
    Result<HostResponseEntityInvocation, WorkerExecutorError>,
    Option<Box<dyn RetainedEntityStore>>,
) {
    let source = scope.activation().source();
    let output_contract = invocation.output_contract.clone();
    let golem_common::model::entity::EntityActivationSource::Host {
        host_tool_id,
        implementation_version,
    } = source
    else {
        return (
            Err(WorkerExecutorError::runtime(
                "native runner requires a host activation",
            )),
            None,
        );
    };
    let key = NativeToolKey {
        host_tool_id: host_tool_id.clone(),
        implementation_version: implementation_version.clone(),
    };
    let golem_common::model::entity::EntityActivationPolicy::Tool {
        binding,
        mcp_import,
        ..
    } = scope.activation().policy()
    else {
        return (
            Err(WorkerExecutorError::runtime(
                "native tool activation does not contain a tool binding",
            )),
            None,
        );
    };
    let handler = if mcp_import.is_some() {
        None
    } else {
        let Some(registration) = worker.native_tool_catalog().get(&key) else {
            return (
                Err(WorkerExecutorError::runtime(format!(
                    "native tool implementation '{:?}@{}' is not installed",
                    key.host_tool_id, key.implementation_version
                ))),
                None,
            );
        };
        if let Err(error) = registration.validate_dispatch(binding) {
            return (Err(WorkerExecutorError::runtime(error)), None);
        }
        Some(registration.handler)
    };
    let executable = WorkerCtxExecutable::Native {
        host_tool_id: host_tool_id.clone(),
        implementation_version: implementation_version.clone(),
    };
    let mut ctx = match await_native_entity_body(
        &runner_abort,
        worker.create_entity_context(
            golem_common::model::entity::OwnerRuntime::Entity(scope.activation().entity()),
            scope.mode(),
            scope.activation().filesystem(),
            executable,
            scope.activation().clone(),
            owner_component_metadata,
            scope.authority_wallet().to_vec(),
            Some(scope.invocation_id().start_index()),
        ),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(error) => return (Err(error), None),
    };
    if let Err(error) = registration
        .attach_linear_memory(ctx.durable_ctx().linear_memory_tracker())
        .and_then(|_| ctx.set_entity_invocation_scope(Some(scope.clone())))
        .and_then(|_| {
            ctx.durable_ctx_mut()
                .set_entity_tool_operation(operation.clone())
        })
    {
        return (Err(error), None);
    }
    if let Some(cancellation) = cancellation.clone()
        && let Err(error) = ctx
            .durable_ctx_mut()
            .set_entity_cancellation(cancellation.clone())
    {
        return (Err(error), None);
    }
    ctx.durable_ctx_mut()
        .set_invocation_principal(Some(invocation.principal.clone()));
    let tasks = match crate::worker::tasks::NativeReconstructionTasks::new(&worker.tasks) {
        Ok(tasks) => tasks,
        Err(error) => return (Err(error), None),
    };
    let spawner = tasks.spawner();
    let monitor = tasks.monitor();
    let mut retained = RetainedNativeContext::new(
        ctx,
        registration.clone(),
        worker.owner_execution(),
        crate::worker::owner_lane::OwnerInvocationId::Entity(scope.invocation_id().clone()),
        tasks,
    );
    let stdout_controller = invocation
        .stdout
        .as_ref()
        .map(ToolOutputWriterEntry::controller);
    let stdout_observer = stdout_controller
        .as_ref()
        .map(AttachmentController::observer);
    let stderr_observer = invocation
        .stderr
        .as_ref()
        .map(ToolOutputWriterEntry::controller)
        .as_ref()
        .map(AttachmentController::observer);
    let (stdout, stderr) = native_output_handles(
        invocation.stdout.as_ref(),
        invocation.stderr.as_ref(),
        &spawner,
    );
    let stdin = match invocation
        .stdin
        .map(|stdin| match stdin {
            ToolBodyStdin::Attachment(stdin) => retained
                .context_mut()
                .shared
                .with_ctx(|ctx| stdin.into_native(ctx, spawner.clone())),
            ToolBodyStdin::Durable(_) => Err(anyhow::anyhow!(
                "native tool body was given a durable stdin without its attachment"
            )),
        })
        .transpose()
    {
        Ok(stdin) => stdin,
        Err(error) => return (Err(error.into()), None),
    };
    let native_invocation = NativeToolInvocation {
        command_path: invocation.command_path,
        input: invocation.input,
        principal: invocation.principal,
        cancellation,
        stdin,
        stdout,
        stderr,
    };
    let completed = await_native_entity_body(&runner_abort, async move {
        let result = tokio::select! {
            biased;
            error = monitor.wait_for_failure() => Err(error),
            error = monitor.wait_for_body_failure() => Err(error),
            result = async {
                match handler {
            Some(handler) => {
                handler
                    .invoke(retained.context_mut(), native_invocation)
                    .await
            }
            None => mcp::invoke(retained.context_mut(), native_invocation).await,
                }
            } => result,
        };
        if result.is_err() {
            durability::abort_producer_disposals(&mut retained.context_mut().shared).await;
        }
        let parent_end = if result.is_ok() {
            retained.prepare_parent_end().await
        } else {
            Ok(())
        };
        let result = select_native_body_result(
            result,
            parent_end,
            operation.has_pending_live_admission_rejection().await,
        );
        let result = match result {
            Ok(result) => {
                if let Some(error) =
                    first_output_limit_error(stdout_observer.as_ref(), stderr_observer.as_ref())
                {
                    encode_tool_operation_terminal(Err(error)).await
                } else {
                    encode_tool_operation_terminal(validate_native_tool_output(
                        result,
                        &output_contract,
                    ))
                    .await
                }
            }
            Err(error) => Err(error),
        };
        if result.is_err() {
            durability::abort_producer_disposals(&mut retained.context_mut().shared).await;
        }
        let retained = Box::new(retained) as Box<dyn RetainedEntityStore>;
        Ok((result, Some(retained)))
    })
    .await;
    let completed = completed.unwrap_or_else(|error| (Err(error), None));
    if completed
        .0
        .as_ref()
        .is_err_and(crate::durable_host::is_tool_attachment_live_admission_rejection)
    {
        for output in [invocation.stdout.as_ref(), invocation.stderr.as_ref()]
            .into_iter()
            .flatten()
        {
            output
                .controller()
                .reserve_owner_terminal(ByteStreamFailure::ResourceExhausted);
        }
    }
    completed
}

pub(super) fn guest_body_accepts_admission_rejection<T>(
    result: &Result<Result<T, wasmtime::Error>, GuestCallSettlementError>,
) -> bool {
    match result {
        Ok(Ok(_)) => true,
        Ok(Err(error))
        | Err(GuestCallSettlementError::Interrupted(error))
        | Err(GuestCallSettlementError::Trap(error)) => error.chain().any(|cause| {
            cause
                .downcast_ref::<WorkerExecutorError>()
                .is_some_and(crate::durable_host::is_tool_attachment_live_admission_rejection)
        }),
        Err(GuestCallSettlementError::Infrastructure(error)) => {
            crate::durable_host::is_tool_attachment_live_admission_rejection(error)
        }
    }
}

pub(super) fn select_native_body_result<T>(
    body: Result<T, WorkerExecutorError>,
    parent_end: Result<(), WorkerExecutorError>,
    live_admission_rejected: bool,
) -> Result<T, WorkerExecutorError> {
    let result = body.and_then(|value| parent_end.map(|()| value));
    if live_admission_rejected && result.is_ok() {
        return Err(crate::durable_host::tool_attachment_live_admission_rejected_error());
    }
    result
}

pub(super) async fn await_native_entity_body<T>(
    runner_abort: &tokio_util::sync::CancellationToken,
    body: impl Future<Output = Result<T, WorkerExecutorError>>,
) -> Result<T, WorkerExecutorError> {
    tokio::select! {
        biased;
        _ = runner_abort.cancelled() => Err(WorkerExecutorError::runtime("native entity body was aborted")),
        result = body => result,
    }
}

pub(super) async fn guest_trap_output_failure(
    operation: &operation::OwnerToolOperation,
    trap: crate::model::TrapType,
    replaying_completed: bool,
    local_cancellation_selected: bool,
) -> ByteStreamFailure {
    if replaying_completed {
        ByteStreamFailure::Failed("tool sidecar trapped".to_string())
    } else if local_cancellation_selected || !operation.select_trap(trap).await {
        ByteStreamFailure::Cancelled
    } else {
        ByteStreamFailure::Failed("tool sidecar trapped".to_string())
    }
}

pub(super) fn resource_exhausted_terminal(
    body_execution: SerializableEntityBodyExecution,
) -> SerializableToolOperationTerminal {
    SerializableToolOperationTerminal {
        body_execution,
        admission_refusal: None,
        result: Err(SerializableToolRpcError::ResourceExhausted(
            "tool stdin exceeded the attachment byte limit".to_string(),
        )),
    }
}

pub(super) async fn resource_exhausted_without_body()
-> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        resource_exhausted_terminal(SerializableEntityBodyExecution::Skipped),
        "failed to encode resource-exhausted tool terminal",
    )
    .await
}

pub(super) async fn resource_exhausted_after_body()
-> Result<HostResponseEntityInvocation, WorkerExecutorError> {
    encode_tool_terminal(
        resource_exhausted_terminal(SerializableEntityBodyExecution::Executed),
        "failed to encode resource-exhausted tool terminal",
    )
    .await
}

pub(super) fn native_output_runtime_teardown_probe(
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    owner_operations: Weak<operation::OwnerToolOperations>,
) -> Arc<dyn Fn() -> bool + Send + Sync + 'static> {
    Arc::new(move || {
        runtime_teardown()
            || owner_operations
                .upgrade()
                .is_some_and(|owner| owner.selected_owner_failure().is_some())
    })
}

impl<Ctx: WorkerCtx> AccessorTask<Ctx, HasSelf<DurableWorkerCtx<Ctx>>> for NativeToolTask {
    async fn run(
        self,
        accessor: &Accessor<Ctx, HasSelf<DurableWorkerCtx<Ctx>>>,
    ) -> wasmtime::Result<()> {
        let Self {
            activation,
            tool_name,
            command_path,
            input,
            stdin: has_stdin,
            stdout: has_stdout,
            stderr: has_stderr,
            principal,
            result,
        } = self;
        let (worker, key) = accessor.with(|mut access| {
            let ctx = access.get();
            (
                ctx.public_state.worker(),
                ctx.state.get_current_idempotency_key(),
            )
        });
        let key = key.ok_or_else(|| wasmtime::Error::msg("native tool has no invocation key"))?;
        let session = worker
            .native_tool_session(&key)
            .await
            .map_err(|error| wasmtime::Error::from_anyhow(error.into()))?;
        let input = if session.is_some() {
            ToolInvocationInput::from_value(input.value())
                .map_err(|error| wasmtime::Error::msg(error.to_string()))?
        } else {
            ToolInvocationInput {
                arguments: input,
                stdin: None,
            }
        };
        let output_handle = |role| {
            session.as_ref().and_then(|(prepared, streams)| {
                prepared
                    .stream_mappings
                    .iter()
                    .find(|mapping| mapping.role == role)
                    .and_then(|mapping| streams.handle(mapping.transport_stream_id))
            })
        };
        let stdout_handle = output_handle(SessionStreamRole::ToolStdout);
        let stderr_handle = output_handle(SessionStreamRole::ToolStderr);
        if input.stdin.is_some() != has_stdin
            || stdout_handle.is_some() != has_stdout
            || stderr_handle.is_some() != has_stderr
        {
            return Err(wasmtime::Error::msg(
                "native tool byte streams disagree with its accepted session",
            ));
        }
        // The durable stdin goes to the body as it is. A component body reads it itself, so
        // its reads are the body's recorded observations. A native body gets an attachment fed
        // from it once the call is accepted and its activation is known.
        let stdin = input
            .stdin
            .map(|stdin| {
                stdin
                    .take_host_endpoint::<DurableInputEndpoint>()
                    .map(|endpoint| ToolCallStdin::Durable(Box::new(endpoint)))
                    .map_err(wasmtime::Error::msg)
            })
            .transpose()?;
        let create_output = |handle: Option<DurableStreamHandle>| -> wasmtime::Result<_> {
            let Some((_, streams)) = &session else {
                return Ok((None, None));
            };
            let Some(handle) = handle else {
                return Ok((None, None));
            };
            let (output, consumer) =
                create_output_attachment(accessor, false).map_err(wasmtime::Error::from_anyhow)?;
            let endpoint = accessor.with(|mut access| -> wasmtime::Result<_> {
                let capacity = access.get().live_stream_event_capacity();
                let runtime_teardown = native_output_runtime_teardown_probe(
                    access.get().stream_runtime_teardown_probe(),
                    Arc::downgrade(&access.get().owner_execution.tool_operations()),
                );
                let interrupt = access.get().create_interrupt_signal();
                let runtime_source = access
                    .get()
                    .runtime_suspension
                    .as_ref()
                    .map(|runtime| runtime.source());
                let (sink, stream) = accounted_byte_output_stream_pair(
                    capacity,
                    runtime_teardown,
                    runtime_source,
                    interrupt,
                )
                .map_err(wasmtime::Error::msg)?;
                let reader = StreamReader::new(&mut access, consumer.into_raw_stream_producer())?;
                reader.pipe(&mut access, sink)?;
                stream
                    .take_host_endpoint::<LiveStreamEndpoint>()
                    .map_err(wasmtime::Error::msg)
            })?;
            Ok((Some(output), Some((streams.clone(), handle, endpoint))))
        };
        let (stdout, stdout_drain) = create_output(stdout_handle)?;
        let (stderr, stderr_drain) = create_output(stderr_handle)?;
        let (target, parent, attempt_ordinal, key_context) = accessor
            .with(|mut access| {
                let ctx = access.get();
                let rpc = tool_rpc_for_current_owner(ctx, tool_name)?;
                let parent = ctx.owner_invocation_id()?;
                let next_ordinal = ctx
                    .state
                    .tool_invocation_attempt_ordinals
                    .entry(parent.clone())
                    .or_default();
                let attempt_ordinal = *next_ordinal;
                *next_ordinal = next_ordinal
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("tool invocation attempt ordinal overflow"))?;
                let target = ToolCallTarget::Ambient(rpc);
                let key_context = EntityInvocationKeyContext::capture(ctx, attempt_ordinal)?;
                Ok::<_, anyhow::Error>((target, parent, attempt_ordinal, key_context))
            })
            .map_err(wasmtime::Error::from_anyhow)?;
        let stdout_rep = stdout.as_ref().map(Resource::rep);
        let stderr_rep = stderr.as_ref().map(Resource::rep);
        let execute = async {
            let dispatch = dispatch_tool_attempt(
                accessor,
                ToolInvocationAttempt {
                    target,
                    input: Ok(input.arguments),
                    rejection: None,
                    key_context,
                    parent,
                    attempt_ordinal,
                },
                command_path,
                stdin,
                has_stdout,
                has_stderr,
                EntityCallMode::Synchronous,
                Some(activation),
                Some(principal),
                ToolAttachmentCounterparty::SessionJournal,
            )
            .await
            .map_err(|error| {
                cleanup_failed_tool_dispatch(accessor, None, stdout_rep, stderr_rep, error)
            })?;
            let response = match dispatch {
                ToolCallDispatch::Rejected { response, stdin } => {
                    close_stdin(accessor, stdin)?;
                    reject_output(accessor, stdout)?;
                    reject_output(accessor, stderr)?;
                    *response
                }
                ToolCallDispatch::Accepted(mut accepted) => {
                    // A native body reads an attachment, not a durable input: the host pumps
                    // the durable stdin into one for it. A component body reads the durable
                    // stdin itself.
                    let native_body = matches!(
                        accepted.durability.scope().activation().source(),
                        golem_common::model::entity::EntityActivationSource::Host { .. }
                    );
                    accepted.stdin = match accepted.stdin.take() {
                        Some(ToolCallStdin::Durable(endpoint)) if native_body => {
                            let attachment = create_native_underlying_stdin(accessor, *endpoint)
                                .map_err(|error| {
                                    cleanup_failed_tool_dispatch(
                                        accessor, None, stdout_rep, stderr_rep, error,
                                    )
                                })?;
                            Some(ToolCallStdin::Attachment(attachment))
                        }
                        stdin => stdin,
                    };
                    register_tool_admission(accessor, &mut accepted)?;
                    execute_accepted_tool_call(accessor, *accepted, stdout, stderr, None, None)
                        .await?
                }
            };
            Ok::<_, anyhow::Error>(response)
        };
        let drain = |drain: Option<(StreamSession, DurableStreamHandle, LiveStreamEndpoint)>| async move {
            if let Some((streams, handle, endpoint)) = drain {
                streams
                    .drain_registered_output(
                        handle,
                        endpoint,
                        Arc::new(SchemaGraph::empty()),
                        SchemaType::u8(),
                    )
                    .await
                    .map_err(SessionError::into_trap)?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (response, (), ()) =
            tokio::try_join!(execute, drain(stdout_drain), drain(stderr_drain))
                .map_err(wasmtime::Error::from_anyhow)?;
        let response = admit_tool_response_secret_holds(accessor, response)
            .await
            .map_err(wasmtime::Error::from_anyhow)?;
        if let Some((prepared, streams)) = &session {
            let value = ToolInvocationOutput {
                outcome: response.clone(),
                stdout: prepared
                    .stream_mappings
                    .iter()
                    .find(|mapping| mapping.role == SessionStreamRole::ToolStdout)
                    .map(|mapping| {
                        SchemaValueStream::from_host_endpoint(
                            crate::durable_host::durable_session::RegisteredOutputStream {
                                transport_stream_id: mapping.transport_stream_id,
                                role: mapping.role,
                            },
                        )
                    }),
                stderr: prepared
                    .stream_mappings
                    .iter()
                    .find(|mapping| mapping.role == SessionStreamRole::ToolStderr)
                    .map(|mapping| {
                        SchemaValueStream::from_host_endpoint(
                            crate::durable_host::durable_session::RegisteredOutputStream {
                                transport_stream_id: mapping.transport_stream_id,
                                role: mapping.role,
                            },
                        )
                    }),
            }
            .into_typed_schema_value()
            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
            streams
                .materialize_result(
                    value.value().clone(),
                    value.graph(),
                    value.root_type(),
                    prepared.attempt.invocation.target_component_revision,
                )
                .await
                .map_err(|error| wasmtime::Error::from_anyhow(error.into_trap()))?;
        }
        result
            .send(response)
            .map_err(|_| wasmtime::Error::msg("native tool result receiver dropped"))?;
        Ok(())
    }
}
