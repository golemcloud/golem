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
    Accessor, AttachmentMemory, ByteStreamCloseCause, ByteStreamFailure, CallReplayOutcome,
    DurabilityHost, DurableCallSession, DurableFunctionType, DurableWorkerCtx, EntityCallMode,
    FutureInvokeResultEntry, FutureToolInvokeGet, FutureToolInvokeState, HasActiveAgents, HasSelf,
    HasWorker, Host, HostFutureInvokeResult, HostFutureInvokeResultWithStore, HostRequestNoInput,
    HostToolCommon, HostToolOutput, HostToolOutputWriter, HostToolOutputWriterWithStore,
    HostToolRpc, HostToolRpcWithStore, HostToolStdin, HostToolStdinClosed,
    HostToolStdinClosedWithStore, HostToolStdinWriter, HostToolStdinWriterWithStore,
    HostToolStreams, HostUnderlying, HostUnderlyingInvokeResult,
    HostUnderlyingInvokeResultWithStore, HostUnderlyingTool, HostUnderlyingToolWithStore,
    HostWithStore, InvocationResult, NotCancellable, Resource, StreamReader, StreamWriteError,
    ToolAttachmentCounterparty, ToolCallDispatch, ToolCallStdin, ToolCallTarget, ToolCommonHost,
    ToolName, ToolOutputEntry, ToolOutputWriterEntry, ToolRpcEntry, ToolRpcError,
    ToolStdinClosedEntry, ToolStdinEntry, ToolStdinStreamConsumer, ToolStdinStreamPumpTask,
    ToolStdinWriterEntry, TypedSchemaValue, UnderlyingError, UnderlyingInvokeResultEntry,
    UnderlyingToolEntry, WitRegisteredTool, WorkerCtx, admit_tool_response_secret_holds,
    attachment_pair, await_tool_invoke_plans, boundary, cleanup_failed_tool_dispatch, close_stdin,
    create_output_attachment, create_underlying_stdin, dispatch_tool_attempt, dispatch_tool_call,
    durability, get_tool_invoke_results, input, invoke_tool_terminal, mpsc, project_tool_response,
    project_tool_unit, read_tool_attempt, reject_output, reserve_invocation_id_before_await,
    resolve_tool_command, spawn_tool_execution, tool_admission_activity,
    tool_rpc_for_current_owner, tool_rpc_resource,
};

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolStdinWriterWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn write(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinWriterEntry>,
        bytes: Vec<u8>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let (writer, controller, ordinal) = accessor.with(|mut access| {
            let producer = &mut access.get().table().get_mut(&self_)?.producer;
            Ok::<_, anyhow::Error>((
                producer.writer(),
                producer.controller(),
                producer.next_operation_ordinal(),
            ))
        })?;
        durability::write::<golem_common::model::oplog::host_functions::GolemToolStdinWrite, _, _>(
            accessor, writer, controller, ordinal, bytes,
        )
        .await
    }

    async fn finish(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinWriterEntry>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let (writer, controller, ordinal) = accessor.with(|mut access| {
            let producer = &mut access.get().table().get_mut(&self_)?.producer;
            Ok::<_, anyhow::Error>((
                producer.writer(),
                producer.controller(),
                producer.next_operation_ordinal(),
            ))
        })?;
        durability::finish::<golem_common::model::oplog::host_functions::GolemToolStdinFinish, _, _>(
            accessor, writer, controller, ordinal,
        )
        .await
    }

    async fn fail(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinWriterEntry>,
        reason: ByteStreamFailure,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let (writer, controller, ordinal) = accessor.with(|mut access| {
            let producer = &mut access.get().table().get_mut(&self_)?.producer;
            Ok::<_, anyhow::Error>((
                producer.writer(),
                producer.controller(),
                producer.next_operation_ordinal(),
            ))
        })?;
        durability::fail::<golem_common::model::oplog::host_functions::GolemToolStdinFail, _, _>(
            accessor, writer, controller, ordinal, reason,
        )
        .await
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolStdinClosedWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn wait(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolStdinClosedEntry>,
    ) -> anyhow::Result<ByteStreamCloseCause> {
        let (observer, operation_ordinal) = accessor.with(|mut access| {
            let entry = access.get().table().get_mut(&self_)?;
            let operation_ordinal = entry.next_operation_ordinal;
            entry.next_operation_ordinal += 1;
            Ok::<_, anyhow::Error>((entry.observer.clone(), operation_ordinal))
        })?;
        durability::wait(accessor, observer, operation_ordinal).await
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolOutputWriterWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn write(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolOutputWriterEntry>,
        bytes: Vec<u8>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let (writer, controller, ordinal) = accessor.with(|mut access| {
            let entry = access.get().table().get_mut(&self_)?;
            Ok::<_, anyhow::Error>((
                entry.producer.writer(),
                entry.controller(),
                entry.producer.next_operation_ordinal(),
            ))
        })?;
        durability::write::<golem_common::model::oplog::host_functions::GolemToolOutputWrite, _, _>(
            accessor, writer, controller, ordinal, bytes,
        )
        .await
    }

    async fn finish(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolOutputWriterEntry>,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let (writer, controller, ordinal) = accessor.with(|mut access| {
            let entry = access.get().table().get_mut(&self_)?;
            Ok::<_, anyhow::Error>((
                entry.producer.writer(),
                entry.controller(),
                entry.producer.next_operation_ordinal(),
            ))
        })?;
        durability::finish::<golem_common::model::oplog::host_functions::GolemToolOutputFinish, _, _>(
            accessor, writer, controller, ordinal,
        )
        .await
    }

    async fn fail(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolOutputWriterEntry>,
        reason: ByteStreamFailure,
    ) -> anyhow::Result<Result<(), StreamWriteError>> {
        let (writer, controller, ordinal) = accessor.with(|mut access| {
            let entry = access.get().table().get_mut(&self_)?;
            Ok::<_, anyhow::Error>((
                entry.producer.writer(),
                entry.controller(),
                entry.producer.next_operation_ordinal(),
            ))
        })?;
        durability::fail::<golem_common::model::oplog::host_functions::GolemToolOutputFail, _, _>(
            accessor, writer, controller, ordinal, reason,
        )
        .await
    }
}

pub(super) fn allocate_stdin<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    capacity: usize,
    identity: Option<golem_common::model::oplog::attachment::ToolAttachmentId>,
) -> anyhow::Result<(
    Resource<ToolStdinWriterEntry>,
    Resource<ToolStdinEntry>,
    Resource<ToolStdinClosedEntry>,
)> {
    let memory = AttachmentMemory::for_store(
        ctx.public_state.worker().active_agents(),
        ctx.state.is_live(),
    );
    let (producer, consumer, observer) = attachment_pair(capacity, memory);
    if let Some(identity) = identity {
        producer.writer().set_identity(identity);
        ctx.state
            .replay_state
            .register_tool_attachment(identity, &consumer.controller())?;
    }
    let writer = ctx.table().push(ToolStdinWriterEntry { producer })?;
    let source = match ctx.table().push(ToolStdinEntry { consumer }) {
        Ok(source) => source,
        Err(error) => {
            ctx.table().delete(writer)?;
            return Err(error.into());
        }
    };
    let closed = match ctx.table().push(ToolStdinClosedEntry {
        observer: observer.into_guest_watcher(),
        next_operation_ordinal: 0,
    }) {
        Ok(closed) => closed,
        Err(error) => {
            ctx.table().delete(source)?;
            ctx.table().delete(writer)?;
            return Err(error.into());
        }
    };
    Ok((writer, source, closed))
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn create_stdin_from_stream(
        accessor: &Accessor<U, Self>,
        source: StreamReader<Result<Vec<u8>, ByteStreamFailure>>,
    ) -> anyhow::Result<Resource<ToolStdinEntry>> {
        accessor.with(|mut access| {
            let ctx = access.get();
            let memory = AttachmentMemory::for_store(
                ctx.public_state.worker().active_agents(),
                ctx.state.is_live(),
            );
            let (producer, consumer, observer) =
                attachment_pair(ctx.state.config.limits.max_tool_attachment_bytes, memory);
            let stdin = ctx.table().push(ToolStdinEntry { consumer })?;
            let (items, received) = mpsc::unbounded_channel();
            if let Err(error) =
                source.pipe(&mut access, ToolStdinStreamConsumer::new(items, observer))
            {
                access.get().table().delete(stdin)?;
                return Err(error.into());
            }
            access.spawn(ToolStdinStreamPumpTask::<Ctx> {
                producer,
                items: received,
                source_cancel: None,
                _ctx: std::marker::PhantomData,
            });
            Ok(stdin)
        })
    }

    async fn create_output(
        accessor: &Accessor<U, Self>,
    ) -> anyhow::Result<(
        Resource<ToolOutputEntry>,
        StreamReader<Result<Vec<u8>, ByteStreamFailure>>,
    )> {
        use crate::durable_host::concurrent::DeferredCallReplayOutcome;
        use golem_common::model::oplog::HostResponseToolAttachmentCreated;
        use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};
        use golem_common::model::oplog::host_functions::GolemToolCreateOutput;
        use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentCreated;

        if accessor.with(|mut access| access.get().state.durability_is_suppressed()) {
            let (target, consumer) = create_output_attachment(accessor, false)?;
            return accessor.with(|mut access| {
                match StreamReader::new(&mut access, consumer.into_stream_producer()) {
                    Ok(reader) => Ok((target, reader)),
                    Err(error) => {
                        access.get().table().delete(target)?;
                        Err(error.into())
                    }
                }
            });
        }
        let mut call = DurableCallSession::<GolemToolCreateOutput, NotCancellable>::start_access(
            accessor,
            accessor.getter(),
            HostRequestNoInput {},
            DurableFunctionType::WriteLocal,
        )
        .await?;
        let identity = ToolAttachmentId {
            creation_start: call.start_index(),
            slot: ToolAttachmentSlot::Output,
        };
        let (created, delivery) = 'creation: {
            if !call.is_live() {
                match call
                    .replay_access_deferred(accessor, accessor.getter())
                    .await?
                {
                    DeferredCallReplayOutcome::Replayed(response, delivery) => {
                        anyhow::ensure!(
                            response.created.attachment == identity,
                            "tool output identity changed during replay"
                        );
                        break 'creation (response.created, delivery);
                    }
                    DeferredCallReplayOutcome::Incomplete(live) => call = live,
                }
            }
            let capacity_bytes = accessor.with(|mut access| {
                access.get().state.config.limits.max_tool_attachment_bytes as u64
            });
            let (response, delivery) = call
                .complete_access_deferred(
                    accessor,
                    accessor.getter(),
                    HostResponseToolAttachmentCreated {
                        created: ToolAttachmentCreated {
                            attachment: identity,
                            capacity_bytes,
                        },
                    },
                )
                .await?;
            (response.created, delivery)
        };
        let output = accessor.with(|mut access| -> anyhow::Result<_> {
            let capacity = usize::try_from(created.capacity_bytes)?;
            let ctx = access.get();
            let memory = AttachmentMemory::for_store(
                ctx.public_state.worker().active_agents(),
                ctx.state.is_live(),
            );
            let (producer, consumer, _) = attachment_pair(capacity, memory);
            producer.writer().set_identity(created.attachment);
            ctx.state
                .replay_state
                .register_tool_attachment(created.attachment, &consumer.controller())?;
            let target = ctx.table().push(ToolOutputEntry {
                producer: Some(producer),
                completion_only: false,
            })?;
            let input = input::AttachmentInputProducer::new(consumer, accessor.getter(), ctx)?;
            match StreamReader::new(&mut access, input) {
                Ok(reader) => Ok((target, reader)),
                Err(error) => {
                    access.get().table().delete(target)?;
                    Err(error.into())
                }
            }
        });
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                delivery.suppress();
                return Err(error);
            }
        };
        if delivery.is_replay_discarded() {
            std::future::pending::<()>().await;
        }
        delivery.deliver_at_accessor_terminal(accessor).await?;
        Ok(output)
    }

    async fn get_invoke_results(
        accessor: &Accessor<U, Self>,
        futures: Vec<Resource<FutureInvokeResultEntry>>,
    ) -> anyhow::Result<Vec<Result<InvocationResult, ToolRpcError>>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host", "get-invoke-results");
        });
        get_tool_invoke_results(accessor, &futures).await
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostUnderlyingToolWithStore<U> for ToolCommonHost<Ctx> {
    async fn invoke(
        accessor: &Accessor<U, Self>,
        self_: Resource<UnderlyingToolEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
    ) -> anyhow::Result<(
        Resource<UnderlyingInvokeResultEntry>,
        Option<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
        Option<StreamReader<Result<Vec<u8>, ByteStreamFailure>>>,
    )> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let accessor = accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(accessor.getter());
        let accessor = &accessor;
        let underlying = accessor
            .with(|mut access| Ok::<_, anyhow::Error>(access.get().table().get(&self_)?.clone()))?;
        let definition = underlying.next_definition()?;
        let mut attempt = read_tool_attempt(
            accessor,
            ToolCallTarget::Underlying(underlying.clone()),
            input,
        )?;
        let mut command_path = command_path;
        let mut response_edge = None;
        if let Ok(input) = &attempt.input {
            let mut streams =
                crate::durable_host::schema_value_stream::ExecutorProjectionStreams::new(accessor);
            match boundary::prepare_underlying_tool_call(
                &underlying.position,
                command_path.clone(),
                input.clone(),
                &mut streams,
            ) {
                Ok(prepared) => {
                    command_path = prepared.command_path;
                    attempt.input = Ok(prepared.input);
                    response_edge = Some(prepared.response_edge);
                }
                Err(error) => attempt.rejection = Some(error),
            }
        }
        let stdout_requested = attempt
            .input
            .as_ref()
            .ok()
            .and_then(|input| resolve_tool_command(definition, &command_path, input).ok())
            .and_then(|command| command.stdout_required)
            .is_some();
        let stderr_requested = attempt
            .input
            .as_ref()
            .ok()
            .and_then(|input| resolve_tool_command(definition, &command_path, input).ok())
            .and_then(|command| command.stderr_required)
            .is_some();
        let stdin = stdin
            .map(|source| create_underlying_stdin(accessor, source))
            .transpose()?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let (stdout, stdout_consumer) = if stdout_requested {
            let (stdout, consumer) =
                create_output_attachment(accessor, false).map_err(|error| {
                    cleanup_failed_tool_dispatch(accessor, stdin_rep, None, None, error)
                })?;
            (Some(stdout), Some(consumer))
        } else {
            (None, None)
        };
        let stdout_rep = stdout.as_ref().map(Resource::rep);
        let (stderr, stderr_consumer) = if stderr_requested {
            let (stderr, consumer) =
                create_output_attachment(accessor, false).map_err(|error| {
                    cleanup_failed_tool_dispatch(accessor, stdin_rep, stdout_rep, None, error)
                })?;
            (Some(stderr), Some(consumer))
        } else {
            (None, None)
        };
        let stderr_rep = stderr.as_ref().map(Resource::rep);
        let admission_activity = tool_admission_activity(accessor);
        let dispatch = dispatch_tool_attempt(
            accessor,
            attempt,
            command_path,
            stdin.map(ToolCallStdin::Attachment),
            stdout_requested,
            stderr_requested,
            EntityCallMode::Asynchronous,
            None,
            None,
            ToolAttachmentCounterparty::Guest,
        )
        .await;
        let state = match dispatch {
            Err(error) => {
                return Err(cleanup_failed_tool_dispatch(
                    accessor, stdin_rep, stdout_rep, stderr_rep, error,
                ));
            }
            Ok(ToolCallDispatch::Rejected { response, stdin }) => {
                close_stdin(accessor, stdin)?;
                reject_output(accessor, stdout)?;
                reject_output(accessor, stderr)?;
                FutureToolInvokeState::Ready(response)
            }
            Ok(ToolCallDispatch::Accepted(accepted)) => FutureToolInvokeState::Active(
                spawn_tool_execution(accessor, *accepted, stdout, stderr).await?,
            ),
        };
        drop(admission_activity);
        let result = accessor.with(|mut access| {
            access.get().table().push(UnderlyingInvokeResultEntry {
                state,
                response_edge,
            })
        })?;
        let stdout = stdout_consumer
            .map(|consumer| {
                accessor.with(|mut access| {
                    StreamReader::new(&mut access, consumer.into_stream_producer())
                })
            })
            .transpose()?;
        let stderr = stderr_consumer
            .map(|consumer| {
                accessor.with(|mut access| {
                    StreamReader::new(&mut access, consumer.into_stream_producer())
                })
            })
            .transpose()?;
        Ok((result, stdout, stderr))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostUnderlyingInvokeResultWithStore<U>
    for ToolCommonHost<Ctx>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<UnderlyingInvokeResultEntry>,
    ) -> anyhow::Result<Result<Option<TypedSchemaValue>, UnderlyingError>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        let accessor = accessor.with_getter::<HasSelf<DurableWorkerCtx<Ctx>>>(accessor.getter());
        let (plan, response_edge) = accessor.with(|mut access| {
            let entry = access.get().table().get(&self_)?;
            let plan = match &entry.state {
                FutureToolInvokeState::Ready(response) => {
                    FutureToolInvokeGet::Ready(response.clone())
                }
                FutureToolInvokeState::Active(execution) => execution.get_plan(),
            };
            Ok::<_, anyhow::Error>((plan, entry.response_edge.clone()))
        })?;
        let mut responses = await_tool_invoke_plans(&accessor, vec![plan]).await?;
        let response = responses
            .pop()
            .expect("a scalar underlying result await must return one response");
        let response = if let Some(edge) = response_edge {
            let mut streams =
                crate::durable_host::schema_value_stream::ExecutorProjectionStreams::new(&accessor);
            boundary::project_underlying_tool_response(response, &edge, &mut streams)
        } else {
            response
        };
        let response = admit_tool_response_secret_holds(&accessor, response).await?;
        let response = project_tool_response(&accessor, response);
        match response {
            Ok(result) => Ok(Ok(result.result)),
            Err(ToolRpcError::RemoteToolError(error)) => Ok(Err(UnderlyingError::ToolError(error))),
            Err(ToolRpcError::Cancelled) => Ok(Err(UnderlyingError::Cancelled)),
            Err(ToolRpcError::ResourceExhausted(message)) => {
                Ok(Err(UnderlyingError::ResourceExhausted(message)))
            }
            Err(ToolRpcError::ProtocolError(message)) => {
                Ok(Err(UnderlyingError::ProtocolError(message)))
            }
            Err(ToolRpcError::Denied(message)) => Ok(Err(UnderlyingError::Denied(message))),
            Err(ToolRpcError::RemoteInternalError(message)) => {
                Ok(Err(UnderlyingError::InternalError(message)))
            }
            Err(ToolRpcError::NotFound(message)) => {
                Ok(Err(UnderlyingError::InternalError(message)))
            }
        }
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolRpcWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn invoke(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<Resource<ToolStdinEntry>>,
    ) -> anyhow::Result<Result<(), ToolRpcError>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let _admission_activity = tool_admission_activity(accessor);
        let dispatch = dispatch_tool_call(
            accessor,
            ToolCallTarget::Ambient(rpc),
            command_path,
            input,
            stdin,
            false,
            false,
            EntityCallMode::FireAndForget,
        )
        .await;
        match dispatch {
            Err(error) => Err(cleanup_failed_tool_dispatch(
                accessor, stdin_rep, None, None, error,
            )),
            Ok(dispatch) => match dispatch {
                ToolCallDispatch::Rejected { response, stdin } => {
                    close_stdin(accessor, stdin)?;
                    Ok(accessor.with(|mut access| {
                        project_tool_unit((*response).map(|_| ()), access.get())
                    }))
                }
                ToolCallDispatch::Accepted(accepted) => {
                    spawn_tool_execution(accessor, *accepted, None, None).await?;
                    Ok(Ok(()))
                }
            },
        }
    }

    async fn invoke_and_await(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<Resource<ToolStdinEntry>>,
        stdout: Option<Resource<ToolOutputEntry>>,
        stderr: Option<Resource<ToolOutputEntry>>,
    ) -> anyhow::Result<Result<InvocationResult, ToolRpcError>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke-and-await");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let response =
            invoke_tool_terminal(accessor, rpc, command_path, input, stdin, stdout, stderr).await?;
        Ok(project_tool_response(accessor, response))
    }

    async fn async_invoke_and_await(
        accessor: &Accessor<U, Self>,
        self_: Resource<ToolRpcEntry>,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<Resource<ToolStdinEntry>>,
        stdout: Option<Resource<ToolOutputEntry>>,
        stderr: Option<Resource<ToolOutputEntry>>,
    ) -> anyhow::Result<Resource<FutureInvokeResultEntry>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "async-invoke-and-await");
        });
        let rpc = tool_rpc_resource(accessor, &self_)?;
        let stdout_requested = stdout.is_some();
        let stderr_requested = stderr.is_some();
        let stdin_rep = stdin.as_ref().map(Resource::rep);
        let stdout_rep = stdout.as_ref().map(Resource::rep);
        let stderr_rep = stderr.as_ref().map(Resource::rep);
        let _admission_activity = tool_admission_activity(accessor);
        let dispatch = dispatch_tool_call(
            accessor,
            ToolCallTarget::Ambient(rpc),
            command_path,
            input,
            stdin,
            stdout_requested,
            stderr_requested,
            EntityCallMode::Asynchronous,
        )
        .await;
        match dispatch {
            Err(error) => Err(cleanup_failed_tool_dispatch(
                accessor, stdin_rep, stdout_rep, stderr_rep, error,
            )),
            Ok(dispatch) => match dispatch {
                ToolCallDispatch::Rejected { response, stdin } => {
                    close_stdin(accessor, stdin)?;
                    reject_output(accessor, stdout)?;
                    reject_output(accessor, stderr)?;
                    let response = admit_tool_response_secret_holds(accessor, *response).await?;
                    accessor.with(|mut access| {
                        Ok(access.get().table().push(FutureInvokeResultEntry {
                            state: FutureToolInvokeState::Ready(Box::new(response)),
                        })?)
                    })
                }
                ToolCallDispatch::Accepted(accepted) => {
                    let execution =
                        spawn_tool_execution(accessor, *accepted, stdout, stderr).await?;
                    accessor.with(|mut access| {
                        Ok(access.get().table().push(FutureInvokeResultEntry {
                            state: FutureToolInvokeState::Active(execution),
                        })?)
                    })
                }
            },
        }
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        self_: Resource<FutureInvokeResultEntry>,
    ) -> anyhow::Result<Result<InvocationResult, ToolRpcError>> {
        reserve_invocation_id_before_await(accessor, accessor.getter())?;
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::future-invoke-result", "get");
        });
        let mut responses = get_tool_invoke_results(accessor, &[self_]).await?;
        Ok(responses
            .pop()
            .expect("a scalar tool result await must return one response"))
    }
}

impl<Ctx: WorkerCtx> HostToolStdinWriter for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdinWriterEntry>) -> anyhow::Result<()> {
        let writer = self.table().delete(rep)?;
        durability::dispose_producer(self, writer.producer, ByteStreamFailure::Abandoned, false)
    }
}

impl<Ctx: WorkerCtx> HostToolStdin for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdinEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostToolStdinClosed for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolStdinClosedEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostToolOutput for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolOutputEntry>) -> anyhow::Result<()> {
        let mut stdout = self.table().delete(rep)?;
        durability::dispose_producer(
            self,
            stdout
                .producer
                .take()
                .expect("output target already consumed"),
            ByteStreamFailure::Abandoned,
            true,
        )
    }
}

impl<Ctx: WorkerCtx> HostToolOutputWriter for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<ToolOutputWriterEntry>) -> anyhow::Result<()> {
        let writer = self.table().delete(rep)?;
        durability::dispose_producer(self, writer.producer, ByteStreamFailure::Abandoned, false)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_tools(&mut self) -> anyhow::Result<Vec<WitRegisteredTool>> {
        self.get_all_tools_model()
            .await?
            .iter()
            .map(|tool| WitRegisteredTool::try_from(tool.as_ref()))
            .collect()
    }

    async fn get_tool(&mut self, name: String) -> anyhow::Result<Option<WitRegisteredTool>> {
        self.get_tool_model(name)
            .await?
            .as_ref()
            .map(|tool| WitRegisteredTool::try_from(tool.as_ref()))
            .transpose()
    }

    async fn create_stdin(
        &mut self,
    ) -> anyhow::Result<(
        Resource<ToolStdinWriterEntry>,
        Resource<ToolStdinEntry>,
        Resource<ToolStdinClosedEntry>,
    )> {
        use golem_common::model::oplog::HostResponseToolAttachmentCreated;
        use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};
        use golem_common::model::oplog::host_functions::GolemToolCreateStdin;
        use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentCreated;

        if self.state.durability_is_suppressed() {
            return allocate_stdin(
                self,
                self.state.config.limits.max_tool_attachment_bytes,
                None,
            );
        }
        let mut call = DurableCallSession::<GolemToolCreateStdin, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::WriteLocal,
        )
        .await?;
        let identity = ToolAttachmentId {
            creation_start: call.start_index(),
            slot: ToolAttachmentSlot::Stdin,
        };
        let created = 'creation: {
            if !call.is_live() {
                match call.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        anyhow::ensure!(
                            response.created.attachment == identity,
                            "tool stdin identity changed during replay"
                        );
                        break 'creation response.created;
                    }
                    CallReplayOutcome::Incomplete(live) => call = live,
                }
            }
            let capacity_bytes = self.state.config.limits.max_tool_attachment_bytes as u64;
            call.complete(
                self,
                HostResponseToolAttachmentCreated {
                    created: ToolAttachmentCreated {
                        attachment: identity,
                        capacity_bytes,
                    },
                },
            )
            .await?
            .created
        };
        allocate_stdin(
            self,
            usize::try_from(created.capacity_bytes)?,
            Some(created.attachment),
        )
    }
}

impl<Ctx: WorkerCtx> HostUnderlyingTool for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<UnderlyingToolEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostUnderlying for DurableWorkerCtx<Ctx> {}
impl<Ctx: WorkerCtx> HostToolStreams for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostToolCommon for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> HostUnderlyingInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, self_: Resource<UnderlyingInvokeResultEntry>) -> anyhow::Result<()> {
        if let FutureToolInvokeState::Active(execution) = &self.table().get(&self_)?.state {
            execution.cancel();
        }
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<UnderlyingInvokeResultEntry>) -> anyhow::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostToolRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, tool_name: String) -> anyhow::Result<Resource<ToolRpcEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "new");
        let tool_name = ToolName::try_from(tool_name).map_err(anyhow::Error::msg)?;
        let rpc = tool_rpc_for_current_owner(self, tool_name)?;
        Ok(self.table().push(rpc)?)
    }

    async fn create(
        &mut self,
        tool_name: String,
    ) -> anyhow::Result<Result<Resource<ToolRpcEntry>, ToolRpcError>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "create");
        let tool_name = match ToolName::try_from(tool_name) {
            Ok(name) => name,
            Err(error) => return Ok(Err(ToolRpcError::ProtocolError(error.to_string()))),
        };
        let rpc = match tool_rpc_for_current_owner(self, tool_name) {
            Ok(rpc) => rpc,
            Err(error) => return Ok(Err(ToolRpcError::ProtocolError(error.to_string()))),
        };
        Ok(Ok(self.table().push(rpc)?))
    }

    async fn drop(&mut self, rep: Resource<ToolRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::tool-rpc", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, self_: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "cancel");
        if let FutureToolInvokeState::Active(execution) = &self.table().get(&self_)?.state {
            execution.cancel();
        }
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}
