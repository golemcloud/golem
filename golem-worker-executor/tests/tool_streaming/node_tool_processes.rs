// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use std::time::Instant;
use test_r::test;

/// Exercise a guest-owned stdin pump, output drains and result wait through
/// ordinary tool imports. Build the fixture before running this test.
#[test]
#[ignore]
#[timeout("3m")]
async fn node_guest_tool_process_bridge(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let probe_path =
        std::path::PathBuf::from(std::env::var("GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM")?)
            .canonicalize()?;
    let cases = std::env::var("GOLEM_NODE_TOOL_STREAM_PROBE_CASES")?;
    let execution = std::env::var("GOLEM_NODE_TOOL_STREAM_PROBE_EXECUTION")
        .unwrap_or_else(|_| "all".to_string());
    anyhow::ensure!(matches!(
        execution.as_str(),
        "all"
            | "main-sync"
            | "main-async"
            | "main-both"
            | "main-overlap"
            | "main-cancel"
            | "main-native-cancel"
            | "fresh-sync"
            | "fresh-async"
            | "fresh-native-cancel"
            | "fresh-sdk-overflow"
            | "fresh-stream-control"
    ));
    let replay_probe = std::env::var_os("GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE").is_some();
    anyhow::ensure!(cases.split(',').all(|mode| matches!(
        mode,
        "streams-collect" | "streams-echo-collect" | "streams-pressure-collect"
    )));
    anyhow::ensure!(
        !matches!(
            execution.as_str(),
            "fresh-stream-control"
                | "main-overlap"
                | "main-cancel"
                | "main-native-cancel"
                | "fresh-native-cancel"
                | "fresh-sdk-overflow"
        ) || cases == "streams-collect"
    );
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
        "node-tool-processes-probe",
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
        // The parent probe needs the owner filesystem, not its streaming child.
        bindings
            .get_mut(&ToolName::try_from("node-tool-probe").expect("valid probe tool name"))
            .unwrap()
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
    let mut effect_probe = if replay_probe {
        Some(middleware_acceptance::start_probe_effect_server().await)
    } else {
        None
    };
    let effect_url = effect_probe
        .as_ref()
        .map(|(port, _, _)| format!("http://127.0.0.1:{port}"));
    for mode in cases.split(',') {
        println!("node-tool-stream starting mode={mode} execution={execution}");
        let started = Instant::now();
        let args = vec![
            mode,
            effect_url.as_deref().unwrap_or(""),
            execution.as_str(),
        ];
        let label = if execution == "all" {
            mode.to_string()
        } else {
            format!("{mode}[{execution}]")
        };
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            invoke_cli_tool(
                &executor,
                &caller_component,
                "node-stream-scheduling",
                "node-tool-probe",
                "/workspace",
                args,
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("guest bridge timed out: {mode}"))??;
        assert_eq!(result.exit_code, 0, "{mode}: {result:?}");
        assert!(result.stderr.is_empty(), "{mode}: {result:?}");
        if mode == "streams-pressure-collect" || execution == "fresh-sdk-overflow" {
            assert_eq!(
                String::from_utf8(result.stdout)?,
                format!("{label}:overflow:4096\n")
            );
            println!(
                "node-tool-stream completed mode={mode} elapsed_us={}",
                started.elapsed().as_micros()
            );
            continue;
        }
        let (bytes, checksum, stderr_bytes, stderr_checksum) = if mode == "streams-echo-collect" {
            (8, 661, 0, 0)
        } else {
            (7, 700, 14, 1455)
        };
        assert_eq!(
            String::from_utf8(result.stdout)?,
            format!("{label}:{bytes}:{checksum}:{stderr_bytes}:{stderr_checksum}\n")
        );
        println!(
            "node-tool-stream completed mode={mode} elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    if let Some((_, effects, server)) = effect_probe.as_mut() {
        for mode in cases.split(',') {
            for runtime in if mode.contains("echo") || execution.starts_with("main-") {
                ["main"].as_slice()
            } else {
                ["main", "job"].as_slice()
            } {
                let effect = tokio::time::timeout(Duration::from_secs(5), effects.recv()).await?;
                assert_eq!(effect, Some(format!("{mode}-{runtime}")));
            }
        }
        assert!(
            effects.try_recv().is_err(),
            "unexpected additional live effect"
        );
        let before = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        assert_collector_history_settled(&before);
        let recorded_before = executor.stored_oplog(&worker).await;
        server.abort();
        let _ = server.await;
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        let replayed: String = tokio::time::timeout(
            Duration::from_secs(30),
            executor.invoke_and_await_agent(
                &caller_component,
                &agent,
                "replay_probe",
                data_value!(),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("collector reconstruction timed out"))??
        .into_typed()?;
        assert_eq!(replayed, "replayed");
        assert!(
            effects.try_recv().is_err(),
            "completed replay repeated an external effect"
        );
        let after = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        // Public span attributes come from maps and have no display order.
        // Compare the authoritative records, not that presentation order.
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
        assert_collector_history_settled(&after);
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
        assert_eq!(next.stdout, b"streams-collect:7:700:14:1455\n");
        assert!(next.stderr.is_empty());
        assert_collector_history_settled(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?);
        println!("node-tool-stream reconstruction completed with the effect server stopped");
    }
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
