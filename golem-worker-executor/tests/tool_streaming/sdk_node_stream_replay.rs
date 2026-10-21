// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use test_r::test;

/// An ordinary SDK tool client and a Node PassThrough can finish live, then
/// stall while reconstructing the completed invocation. Build the standalone
/// fixture first; this test never starts a compiler or a JavaScript subprocess.
#[test]
#[ignore]
#[timeout("3m")]
async fn sdk_cancelled_tool_with_node_stream_reconstructs_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let probe_path = std::path::PathBuf::from(std::env::var("GOLEM_SDK_NODE_STREAM_REPLAY_WASM")?)
        .canonicalize()?;
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        configure: Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
            config.limits.max_tool_attachment_bytes = 65536;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let probe = executor
        .component(
            &context.default_environment_id,
            &probe_path.with_extension("").to_string_lossy(),
        )
        .store()
        .await?;
    let mut deployment = deployment_state(
        context.account_id,
        probe.id,
        probe.revision,
        "sdk-node-stream-replay",
        "ToolStreamingCaller",
        extract_component_metadata(&probe_path, false, true)
            .await?
            .tools,
    );
    let provider = executor
        .component_dep(&context.default_environment_id, streaming)
        .store()
        .await?;
    let provider_deployment = deployment_state(
        context.account_id,
        provider.id,
        provider.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", streaming.wasm_name)),
            false,
            true,
        )
        .await?
        .tools,
    );
    deployment
        .registered_tools
        .extend(provider_deployment.registered_tools);
    for (owner, bindings) in provider_deployment.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        bindings
            .get_mut(&ToolName::try_from("node-tool-probe").expect("valid probe tool name"))
            .expect("probe binding")
            .filesystem_access = ToolFilesystemAccess::Allowed;
    }
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent = agent_id!("ToolStreamingCaller", "node-stream-scheduling");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let (port, mut effects, server) = middleware_acceptance::start_probe_effect_server().await;
    let effect_url = format!("http://127.0.0.1:{port}");
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        invoke_cli_tool(
            &executor,
            &caller_component,
            "node-stream-scheduling",
            "node-tool-probe",
            "/workspace",
            vec!["streams-collect", &effect_url, "main-sdk-overflow"],
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SDK stream live execution timed out"))??;
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    assert_eq!(
        result.stdout,
        b"streams-collect[main-sdk-overflow]:overflow:4096\n",
    );
    println!("SDK cancellation and Node stream completed live");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), effects.recv()).await?,
        Some("streams-collect-main".to_string()),
    );
    assert!(
        effects.try_recv().is_err(),
        "unexpected additional live effect"
    );
    assert_collector_history_settled(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?);
    let recorded_before = executor.stored_oplog(&worker).await;
    server.abort();
    let _ = server.await;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed: String = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(&caller_component, &agent, "replay_probe", data_value!()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SDK stream reconstruction timed out"))??
    .into_typed()?;
    assert_eq!(replayed, "replayed");
    assert!(
        effects.try_recv().is_err(),
        "completed replay repeated an external effect"
    );
    let recorded_after = executor.stored_oplog(&worker).await;
    assert!(recorded_after.len() >= recorded_before.len());
    for (index, (before, after)) in recorded_before.iter().zip(&recorded_after).enumerate() {
        assert_eq!(
            before,
            after,
            "recorded prefix changed at entry {}",
            index + 1
        );
    }
    assert_collector_history_settled(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?);
    let next = invoke_cli_tool(
        &executor,
        &caller_component,
        "node-stream-scheduling",
        "node-tool-probe",
        "/workspace",
        vec!["streams-collect"],
    )
    .await?;
    assert_eq!(next.exit_code, 0, "after restart: {next:?}");
    assert!(next.stderr.is_empty(), "after restart: {next:?}");
    assert_eq!(next.stdout, b"streams-collect:7:700:14:1455\n");
    assert_collector_history_settled(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?);
    println!("SDK stream reconstruction completed with the effect server stopped");
    Ok(())
}

fn assert_collector_history_settled(history: &[PublicOplogEntryWithIndex]) {
    let last_finished = history
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("the owner has completed an invocation")
        .oplog_index;
    let mut open_invocation = None;
    for entry in history {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(_) => {
                assert!(open_invocation.replace(entry.oplog_index).is_none());
            }
            PublicOplogEntry::AgentInvocationFinished(_) => {
                assert!(open_invocation.take().is_some());
            }
            PublicOplogEntry::Start(start) => {
                let terminals = history
                    .iter()
                    .filter(|terminal| match &terminal.entry {
                        PublicOplogEntry::End(end) => end.start_index == entry.oplog_index,
                        PublicOplogEntry::Cancelled(cancelled) => {
                            cancelled.start_index == entry.oplog_index
                        }
                        _ => false,
                    })
                    .count();
                if terminals == 0 {
                    assert!(
                        collector_call_abandoned_by_entity(history, entry),
                        "unsettled Start {} ({}) has no owning entity cancellation; parent={:?}",
                        entry.oplog_index,
                        start.function_name,
                        start.parent_start_index,
                    );
                } else {
                    assert_eq!(
                        terminals,
                        1,
                        "duplicated terminal for Start {} ({}): {:?}",
                        entry.oplog_index,
                        start.function_name,
                        history
                            .iter()
                            .filter_map(|terminal| match &terminal.entry {
                                PublicOplogEntry::End(end)
                                    if end.start_index == entry.oplog_index =>
                                    Some((terminal.oplog_index, "End")),
                                PublicOplogEntry::Cancelled(cancelled)
                                    if cancelled.start_index == entry.oplog_index =>
                                    Some((terminal.oplog_index, "Cancelled")),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    );
                }
                assert!(entry.oplog_index < last_finished);
            }
            PublicOplogEntry::End(_) | PublicOplogEntry::Cancelled(_) => {
                assert!(entry.oplog_index < last_finished);
            }
            _ => {}
        }
    }
    assert!(open_invocation.is_none());
}

fn collector_call_abandoned_by_entity(
    history: &[PublicOplogEntryWithIndex],
    call: &PublicOplogEntryWithIndex,
) -> bool {
    let PublicOplogEntry::Start(start) = &call.entry else {
        return false;
    };
    if !matches!(
        start.function_name.as_str(),
        "golem::tool::streams::tool-output-writer::write" | "clocks::monotonic-clock::wait-for"
    ) || history.iter().any(|entry| match &entry.entry {
        PublicOplogEntry::End(end) => end.start_index == call.oplog_index,
        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == call.oplog_index,
        _ => false,
    }) {
        return false;
    }
    let Some(parent) = start
        .parent_start_index
        .filter(|parent| *parent < call.oplog_index)
    else {
        return false;
    };
    if !history.iter().any(|entry| entry.oplog_index == parent &&
        matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke"))
    {
        return false;
    }
    // A cancelled body owns its abandoned subtree, including a write or timer
    // whose future was destroyed with the body. A normal End cannot own it.
    let mut terminals = history.iter().filter(|entry| match &entry.entry {
        PublicOplogEntry::End(end) => end.start_index == parent,
        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == parent,
        _ => false,
    });
    matches!(terminals.next(), Some(entry) if entry.oplog_index > call.oplog_index &&
        matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == parent))
        && terminals.next().is_none()
}

#[test]
fn collector_abandonment_requires_the_owning_entity_cancellation() {
    use golem_common::model::oplog::PublicDurableFunctionType;
    use golem_common::model::oplog::public_oplog_entry::{CancelledParams, EndParams, StartParams};
    use golem_common::model::{Empty, Timestamp};

    let entry = |index, entry| PublicOplogEntryWithIndex {
        oplog_index: OplogIndex::from_u64(index),
        attribution: PublicOplogEntryAttribution::agent(),
        entry,
    };
    let start = |index, parent: Option<u64>, function: &str| {
        entry(
            index,
            PublicOplogEntry::Start(StartParams {
                timestamp: Timestamp::now_utc(),
                parent_start_index: parent.map(OplogIndex::from_u64),
                function_name: function.to_string(),
                invocation_id: None,
                observational_owner: None,
                request: None,
                durable_function_type: PublicDurableFunctionType::ReadLocal(Empty {}),
                span_started: None,
                denial: None,
            }),
        )
    };
    let cancel = |index, start| {
        entry(
            index,
            PublicOplogEntry::Cancelled(CancelledParams {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(start),
                partial: None,
                span_finished: None,
            }),
        )
    };
    let end = |index, start| {
        entry(
            index,
            PublicOplogEntry::End(EndParams {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(start),
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            }),
        )
    };
    for function in [
        "golem::tool::streams::tool-output-writer::write",
        "clocks::monotonic-clock::wait-for",
    ] {
        let parent = start(1, None, "golem::entity::invoke");
        let call = start(2, Some(1), function);
        let history = vec![parent.clone(), call.clone(), cancel(3, 1)];
        assert!(collector_call_abandoned_by_entity(&history, &call));
        assert!(!collector_call_abandoned_by_entity(&history[..2], &call));
        assert!(!collector_call_abandoned_by_entity(&history[1..], &call));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), end(3, 1)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), cancel(3, 99)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), cancel(3, 1), cancel(4, 1)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), cancel(3, 1), end(4, 2)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[start(1, None, "unrelated"), call.clone(), cancel(3, 1)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &history,
            &start(2, None, function)
        ));
        assert!(!collector_call_abandoned_by_entity(
            &history,
            &start(4, Some(1), function)
        ));
        assert!(!collector_call_abandoned_by_entity(
            &history,
            &start(2, Some(1), "unrelated")
        ));
    }
}
