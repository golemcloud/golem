use super::*;
use test_r::test;

async fn exercise_python_sidecar(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    python: &PrecompiledComponent,
    reconstruct: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider = executor
        .component_dep(&context.default_environment_id, python)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", python.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider.id,
        provider.revision,
        "golem:python",
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    if !reconstruct {
        exercise_python_session(&executor, &context, &caller_component, PythonSession::Echo)
            .await?;

        let large = invoke_cli_tool(
            &executor, &caller_component, "python-large", "python", "/",
            vec!["-c", "import sys; sys.stdout.buffer.write(b'A' * 131089); sys.stderr.buffer.write(b'B' * 65553); sys.exit(9)"],
        ).await?;
        assert_eq!(large.exit_code, 9);
        assert_eq!(large.stdout, vec![b'A'; 131089]);
        assert_eq!(large.stderr, vec![b'B'; 65553]);

        exercise_python_session(
            &executor,
            &context,
            &caller_component,
            PythonSession::CloseInput,
        )
        .await?;
        // A fresh call on the same owner must still acquire the filesystem lane.
        let next = invoke_cli_tool(
            &executor,
            &caller_component,
            "python-cancel",
            "python",
            "/",
            vec!["-c", "print('after-cancel')"],
        )
        .await?;
        assert_eq!(next.exit_code, 0);
        assert_eq!(next.stdout, b"after-cancel\n");
        assert!(next.stderr.is_empty());
        exercise_python_session(
            &executor,
            &context,
            &caller_component,
            PythonSession::Interrupt,
        )
        .await?;
        return Ok(());
    }

    let worker_id = executor
        .start_agent(
            &caller_component.id,
            agent_id!("ToolStreamingCaller", "python-files"),
        )
        .await?;
    let setup = invoke_cli_tool(
        &executor, &caller_component, "python-files", "python", "/",
        vec!["-c", "import os; os.mkdir('python-probe'); open('python-probe/value', 'wb').write(b'owner-data'); interpreter_only = 42"],
    ).await?;
    assert_eq!(setup.exit_code, 0, "{setup:?}");
    assert!(setup.stdout.is_empty());
    assert!(setup.stderr.is_empty());
    let source = "import os; print(os.getcwd()); print(open('value', 'rb').read().decode()); print('interpreter_only' in globals())";
    let before = invoke_cli_tool(
        &executor,
        &caller_component,
        "python-files",
        "python",
        "/python-probe",
        vec!["-c", source],
    )
    .await?;
    assert_eq!(before.exit_code, 0, "{before:?}");
    assert_eq!(before.stdout, b"/python-probe\nowner-data\nFalse\n");
    assert!(before.stderr.is_empty());
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let after = invoke_cli_tool(
        &executor,
        &caller_component,
        "python-files",
        "python",
        "/python-probe",
        vec!["-c", source],
    )
    .await?;
    assert_eq!(after.exit_code, 0, "{after:?}");
    assert_eq!(after.stdout, before.stdout);
    assert!(after.stderr.is_empty());
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                if method.method_name.replace('-', "_") == "builtin_cli"))
        })
        .map(|entry| entry.oplog_index)
        .collect();
    assert_eq!(
        invocation_starts.len(),
        3,
        "completed replay must not record another owner invocation"
    );
    for (index, start) in invocation_starts.iter().enumerate() {
        let next = invocation_starts.get(index + 1);
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    entry.oplog_index > *start
                        && next.is_none_or(|next| entry.oplog_index < *next)
                        && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
                })
                .count(),
            1,
            "each owner invocation must finish once"
        );
    }
    let entity_starts: Vec<_> = oplog.iter().filter(|entry| {
        matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke")
    }).collect();
    assert_eq!(entity_starts.len(), 3);
    for start in entity_starts {
        assert_eq!(oplog.iter().filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start.oplog_index)
        }).count(), 1, "each Python entity must settle once");
    }
    Ok(())
}

enum PythonSession {
    Echo,
    CloseInput,
    Interrupt,
}

async fn exercise_python_session(
    executor: &TestWorkerExecutor,
    context: &TestContext,
    caller: &golem_common::model::component::ComponentDto,
    mode: PythonSession,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::{
        ExternalToolInvocation, InputStreamEnd, InputStreamItem, InvocationRequest,
        InvocationStart, StreamCancel, StreamCancelReason, StreamCancelRole, ToolByteStreamRole,
        input_stream_item, invocation_request, invocation_response, invocation_session_completion,
        invocation_session_result,
    };
    use golem_common::model::oplog::PublicExternalToolResult;

    let cancel_input = matches!(mode, PythonSession::CloseInput);
    let name = match mode {
        PythonSession::Echo => "python-stream",
        PythonSession::CloseInput => "python-cancel",
        PythonSession::Interrupt => "python-interrupt",
    };
    let worker_id = executor
        .start_agent(&caller.id, agent_id!("ToolStreamingCaller", name))
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let source = "import sys; sys.stdout.buffer.write(b'ready:'); sys.stdout.flush(); data = sys.stdin.buffer.read(); sys.stdout.buffer.write(data); sys.stderr.buffer.write(b'error:\\x00\\xff'); sys.exit(23)";
    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "args".into(),
                body: SchemaType::list(SchemaType::string()),
                metadata: Default::default(),
            },
            golem_common::schema::NamedFieldType {
                name: "cwd".into(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("-c".into()),
                        SchemaValue::String(source.into()),
                    ],
                },
                SchemaValue::String("/".into()),
            ],
        },
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(worker_id.clone().into()),
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                auth_ctx: Some(executor.auth_ctx().into()),
                principal: Some(
                    Principal::GolemUser(GolemUserPrincipal {
                        account_id: context.account_id,
                    })
                    .into(),
                ),
                environment_id: Some(context.default_environment_id.into()),
                component_owner_account_id: Some(context.account_id.into()),
                expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                external_tool: Some(ExternalToolInvocation {
                    tool_name: "python".into(),
                    command_path: vec![],
                    input: Some(input.try_into().map_err(anyhow::Error::msg)?),
                    stdin: true,
                    stdout: true,
                    stderr: true,
                    fresh_owner: false,
                    expected_deployment_revision: None,
                }),
                ..Default::default()
            })),
        })
        .await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let accepted = responses
        .message()
        .await?
        .expect("Python session acceptance");
    let Some(invocation_response::Response::Accepted(accepted)) = accepted.response else {
        anyhow::bail!("Python session not accepted: {accepted:?}");
    };
    let mapping = |role: ToolByteStreamRole| {
        accepted
            .stream_mappings
            .iter()
            .find(|mapping| mapping.tool_byte_stream_role == Some(role as i32))
            .unwrap()
    };
    let stdin = mapping(ToolByteStreamRole::Stdin).clone();
    let stdout_id = mapping(ToolByteStreamRole::Stdout).transport_stream_id;
    let stderr_id = mapping(ToolByteStreamRole::Stderr).transport_stream_id;
    let mut output = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while output.len() < b"ready:".len() {
            let response = responses
                .message()
                .await?
                .expect("Python stdout before stdin EOF");
            let Some(invocation_response::Response::OutputItem(item)) = response.response else {
                anyhow::bail!("expected handshake output: {response:?}");
            };
            assert_eq!(item.transport_stream_id, stdout_id);
            output.extend(item.packed_u8);
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(output, b"ready:");
    if matches!(mode, PythonSession::Interrupt) {
        executor.interrupt(&worker_id).await?;
        let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
        if let Some(active) = executor.active_entity_metadata(&owner).await {
            assert!(active.tool_operations.operations.is_empty());
            assert!(active.lane.holder.is_none());
            assert_eq!(active.lane.active_invocation_count, 0);
        }
        drop(responses);
        drop(sender);
        executor.delete_worker(&worker_id).await?;
        return Ok(());
    }
    let bytes: Vec<u8> = (0..65553).map(|i| (i % 256) as u8).collect();
    let send = async {
        if cancel_input {
            sender
                .send(InvocationRequest {
                    request: Some(invocation_request::Request::StreamCancel(StreamCancel {
                        transport_stream_id: stdin.transport_stream_id,
                        producer_sequence: 0,
                        role: StreamCancelRole::InputProducer as i32,
                        reason: StreamCancelReason::Cancelled as i32,
                        durable_stream_id: stdin.handle.as_ref().unwrap().stream_id,
                        epoch: accepted.epoch,
                        ..Default::default()
                    })),
                })
                .await?;
        } else {
            for (index, chunk) in bytes.chunks(16384).enumerate() {
                sender
                    .send(InvocationRequest {
                        request: Some(invocation_request::Request::InputItem(InputStreamItem {
                            transport_stream_id: stdin.transport_stream_id,
                            sequence: (index * 16384) as u64,
                            payload: Some(input_stream_item::Payload::PackedU8(chunk.to_vec())),
                            durable_stream_id: stdin.handle.as_ref().unwrap().stream_id,
                            epoch: accepted.epoch,
                        })),
                    })
                    .await?;
            }
            sender
                .send(InvocationRequest {
                    request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                        transport_stream_id: stdin.transport_stream_id,
                        sequence: bytes.len() as u64,
                        durable_stream_id: stdin.handle.as_ref().unwrap().stream_id,
                        epoch: accepted.epoch,
                    })),
                })
                .await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let receive = async {
        let mut errors = Vec::new();
        let mut result_seen = false;
        let mut finished = false;
        let mut ends = BTreeSet::new();
        while let Some(response) = responses.message().await? {
            match response.response {
                Some(invocation_response::Response::OutputItem(item)) => {
                    if item.transport_stream_id == stdout_id {
                        output.extend(item.packed_u8);
                    } else {
                        assert_eq!(item.transport_stream_id, stderr_id);
                        errors.extend(item.packed_u8);
                    }
                }
                Some(invocation_response::Response::OutputEnd(end)) => {
                    assert!(ends.insert(end.transport_stream_id));
                }
                Some(invocation_response::Response::Result(result)) => {
                    let Some(invocation_session_result::Result::ToolResult(result)) = result.result
                    else {
                        anyhow::bail!("not a Python tool result");
                    };
                    let result: PublicExternalToolResult =
                        result.try_into().map_err(anyhow::Error::msg)?;
                    let PublicExternalToolResult::Success(result) = result else {
                        anyhow::bail!("Python failed: {result:?}");
                    };
                    let status = result.result.unwrap();
                    assert_eq!(
                        status.root_type(),
                        &SchemaType::ref_to("golem.tool.ProcessExitStatus".into())
                    );
                    // A cancelled stdin is Ctrl+C for the script: the read raises
                    // KeyboardInterrupt, and Python exits with 130.
                    assert_eq!(
                        status.value(),
                        &SchemaValue::Record {
                            fields: vec![SchemaValue::U8(if cancel_input { 130 } else { 23 })]
                        }
                    );
                    assert!(!result_seen);
                    result_seen = true;
                }
                Some(invocation_response::Response::InputAck(_)) => {}
                Some(invocation_response::Response::Finished(completion)) => {
                    assert!(
                        matches!(
                            completion.outcome,
                            Some(invocation_session_completion::Outcome::Success(_))
                        ),
                        "{completion:?}"
                    );
                    finished = true;
                }
                Some(invocation_response::Response::StreamCancel(_)) if cancel_input => {}
                other => anyhow::bail!("unexpected Python session response: {other:?}"),
            }
        }
        assert!(result_seen && finished);
        assert_eq!(ends, BTreeSet::from([stdout_id, stderr_id]));
        if cancel_input {
            assert_eq!(output, b"ready:");
            let errors = String::from_utf8_lossy(&errors);
            assert!(
                errors.contains("KeyboardInterrupt") && !errors.contains("error:"),
                "{errors}"
            );
        } else {
            assert_eq!(output, [b"ready:".as_slice(), bytes.as_slice()].concat());
            assert_eq!(errors, b"error:\x00\xff");
        }
        Ok::<_, anyhow::Error>(())
    };
    tokio::try_join!(send, receive)?;
    Ok(())
}

#[test]
#[timeout("5m")]
async fn builtin_python_streams_handshake_input_close_and_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("python")] python: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_python_sidecar(last_unique_id, deps, caller, python, false).await
}

#[test]
#[timeout("5m")]
async fn builtin_python_owner_filesystem_cwd_and_completed_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("python")] python: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_python_sidecar(last_unique_id, deps, caller, python, true).await
}
