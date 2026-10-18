// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use std::time::Instant;
use test_r::test;

/// Compare the existing async observer with the experimentally sync-lowered
/// terminal import. Select cases explicitly; build the fixture before running.
#[test]
#[ignore]
#[timeout("3m")]
async fn node_native_tool_stream_scheduling_probe(
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
    let replay_probe = std::env::var_os("GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE").is_some();
    let crash_stage = std::env::var("GOLEM_NODE_TOOL_STREAM_CRASH_STAGE").ok();
    if let Some(stage) = &crash_stage {
        anyhow::ensure!(cases == "streams-deadline-collect");
        anyhow::ensure!(matches!(
            stage.as_str(),
            "before-race" | "race-started" | "race-selected"
        ));
    }
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
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
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
        anyhow::ensure!(cases.split(',').all(|mode| matches!(
            mode,
            "streams-collect"
                | "streams-echo-collect"
                | "streams-pressure-collect"
                | "streams-finite-stdin"
                | "streams-deadline-collect"
        )));
        Some(middleware_acceptance::start_probe_effect_server().await)
    } else {
        None
    };
    let effect_url = effect_probe
        .as_ref()
        .map(|(port, _, _)| format!("http://127.0.0.1:{port}"));
    for mode in cases.split(',') {
        anyhow::ensure!(matches!(
            mode,
            "streams-sync"
                | "streams-async"
                | "streams-echo-sync"
                | "streams-echo-async"
                | "streams-pressure-sync"
                | "streams-pressure-async"
                | "streams-cancel-async"
                | "streams-collect"
                | "streams-echo-collect"
                | "streams-pressure-collect"
                | "streams-finite-stdin"
                | "streams-deadline-collect"
        ));
        println!("node-tool-stream starting mode={mode}");
        let started = Instant::now();
        let mut args = vec![mode];
        if let Some(url) = &effect_url {
            args.push(url);
        }
        let result = if let Some(stage) = &crash_stage {
            let stage = match stage.as_str() {
                "before-race" => ReplayAdmissionStage::BeforeLiveStart,
                "race-started" => ReplayAdmissionStage::AfterDeferredStart,
                "race-selected" => ReplayAdmissionStage::AfterHostInternalTerminal,
                _ => unreachable!(),
            };
            let mut gate =
                executor.gate_next_replay_access_admission(&worker, "collection-race", stage);
            let key = IdempotencyKey::fresh();
            let params = data_value!("node-tool-probe", "/workspace", args.clone());
            executor
                .invoke_agent_with_key(
                    &caller_component,
                    &agent,
                    &key,
                    "builtin_cli",
                    params.clone(),
                )
                .await?;
            tokio::time::timeout(Duration::from_secs(30), gate.entered()).await?;
            executor.commit_oplog(&worker).await?;
            let before = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
            let collection = before
                .iter()
                .find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(start)
                        if start.function_name.ends_with("collection-wait") =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                })
                .expect("the collection boundary precedes the race");
            assert!(!before.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == collection)));
            let race = before.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start)
                    if start.function_name.ends_with("collection-race") =>
                {
                    assert_eq!(start.parent_start_index, Some(collection));
                    Some(entry.oplog_index)
                }
                _ => None,
            });
            assert_eq!(
                race.is_some(),
                stage != ReplayAdmissionStage::BeforeLiveStart
            );
            let selected = race.is_some_and(|race| {
                before.iter().any(|entry|
                matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == race))
            });
            assert_eq!(
                selected,
                stage == ReplayAdmissionStage::AfterHostInternalTerminal
            );
            let recorded_before = executor.stored_oplog(&worker).await;
            drop(executor);
            drop(gate);
            executor = start_with_overrides(deps, &context, overrides.clone()).await?;
            let result: CliToolEvidence = tokio::time::timeout(
                Duration::from_secs(30),
                executor.invoke_and_await_agent_with_key(
                    &caller_component,
                    &agent,
                    &key,
                    "builtin_cli",
                    params,
                ),
            )
            .await??
            .into_typed()?;
            let after = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
            assert_eq!(after.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == collection)).count(), 1);
            if let Some(race) = race {
                assert_eq!(after.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == race)).count(), 1);
            }
            let recorded_after = executor.stored_oplog(&worker).await;
            assert!(recorded_after.len() >= recorded_before.len());
            for (before, after) in recorded_before.iter().zip(&recorded_after) {
                assert_eq!(before, after, "reconstruction changed the recorded prefix");
            }
            assert_collector_history_settled(&after);
            result
        } else {
            tokio::time::timeout(
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
            .map_err(|_| anyhow::anyhow!("stream scheduling probe timed out: {mode}"))??
        };
        assert_eq!(result.exit_code, 0, "{mode}: {result:?}");
        assert!(result.stderr.is_empty(), "{mode}: {result:?}");
        if mode == "streams-deadline-collect" {
            assert_eq!(
                result.stdout,
                b"streams-deadline-collect:cancelled:followup-ok\n"
            );
            println!(
                "node-tool-stream completed mode={mode} elapsed_us={}",
                started.elapsed().as_micros()
            );
            continue;
        }
        if mode == "streams-finite-stdin" {
            assert_eq!(result.stdout, b"streams-finite-stdin:262161:5\n");
            println!(
                "node-tool-stream completed mode={mode} elapsed_us={}",
                started.elapsed().as_micros()
            );
            continue;
        }
        if mode == "streams-pressure-collect" {
            assert_eq!(
                String::from_utf8(result.stdout)?,
                "streams-pressure-collect:overflow:4096\n"
            );
            println!(
                "node-tool-stream completed mode={mode} elapsed_us={}",
                started.elapsed().as_micros()
            );
            continue;
        }
        let (bytes, checksum, stderr_bytes, stderr_checksum) = if mode.contains("cancel") {
            (7, 700, 0, 0)
        } else if mode.contains("echo") {
            (8, 661, 0, 0)
        } else if mode.contains("pressure") {
            let sum: u64 = (0..512_u32)
                .map(|index| u64::from(index % 251) * 65536)
                .sum();
            (512 * 65536, sum, 0, 0)
        } else {
            (7, 700, 14, 1455)
        };
        let suffix = if mode.contains("cancel") {
            ":cancelled"
        } else {
            ""
        };
        assert_eq!(
            String::from_utf8(result.stdout)?,
            format!("{mode}:{bytes}:{checksum}:{stderr_bytes}:{stderr_checksum}{suffix}\n")
        );
        println!(
            "node-tool-stream completed mode={mode} elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    if let Some((_, effects, server)) = effect_probe.as_mut() {
        for mode in cases.split(',') {
            for runtime in if mode.contains("echo") {
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

/// Opt-in feasibility probe, built before the test. There are no linker overrides or
/// test-only runtime hooks: the JS tool and write-file use ordinary tool admission.
#[test]
#[ignore]
#[timeout("10m")]
async fn node_sync_native_import_waits_for_filesystem_tool(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let probe_path =
        std::path::PathBuf::from(std::env::var("GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM")?)
            .canonicalize()?;
    anyhow::ensure!(probe_path.extension().is_some_and(|ext| ext == "wasm"));
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        configure: Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
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
    let provider = executor
        .component(
            &context.default_environment_id,
            &probe_path.with_extension("").to_string_lossy(),
        )
        .name("node-tool-processes-probe")
        .store()
        .await?;
    let filesystem_component = executor
        .component_dep(&context.default_environment_id, filesystem)
        .store()
        .await?;
    let metadata = extract_component_metadata(&probe_path, false, true).await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider.id,
        provider.revision,
        "node-tool-processes-probe",
        "ToolStreamingCaller",
        metadata.tools,
    );
    let filesystem_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", filesystem.wasm_name)),
        false,
        true,
    )
    .await?;
    let filesystem_deployment = deployment_state(
        context.account_id,
        filesystem_component.id,
        filesystem_component.revision,
        "golem:filesystem-tools",
        "ToolStreamingCaller",
        filesystem_metadata.tools,
    );
    deployment
        .registered_tools
        .extend(filesystem_deployment.registered_tools);
    for (owner, bindings) in filesystem_deployment.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let name = "node-sync-filesystem";
    let worker = executor
        .start_agent(&caller_component.id, agent_id!("ToolStreamingCaller", name))
        .await?;

    // Compile/cache fixtures and initialize the owner outside the timed matrix.
    // Alternate order between repetitions; each invocation still gets a fresh sidecar.
    let modes = ["baseline", "nested", "job-baseline", "job-nested"];
    for repetition in 0..=5 {
        for index in 0..modes.len() {
            let mode = modes[(index + repetition) % modes.len()];
            let content = format!("owner-data-{repetition}-{mode}");
            let started = Instant::now();
            let result = invoke_cli_tool(
                &executor,
                &caller_component,
                name,
                "node-tool-probe",
                "/workspace",
                vec![mode, &content],
            )
            .await?;
            let elapsed = started.elapsed();
            assert_eq!(result.exit_code, 0, "{mode}: {result:?}");
            assert_eq!(result.stdout, format!("{mode}:{content}\n").as_bytes());
            assert!(result.stderr.is_empty(), "{mode}: {result:?}");
            if repetition > 0 {
                println!(
                    "node-tool-executor mode={mode} repetition={repetition} elapsed_us={}",
                    elapsed.as_micros(),
                );
            }
        }
    }

    // Last matrix write is repetition 5, baseline. A read-only fixture mode
    // verifies owner visibility after an actual executor teardown/reconstruction.
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let result = invoke_cli_tool(
        &executor,
        &caller_component,
        name,
        "node-tool-probe",
        "/workspace",
        vec!["verify", "owner-data-5-baseline"],
    )
    .await?;
    assert_eq!(result.exit_code, 0, "after restart: {result:?}");
    assert_eq!(result.stdout, b"verify:owner-data-5-baseline\n");
    assert!(result.stderr.is_empty());

    let history = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let entities: Vec<_> = history
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start)
                if start.function_name == "golem::entity::invoke")
        })
        .collect();
    assert_eq!(
        entities.len(),
        25 + 12,
        "25 outer tools and 12 nested writes"
    );
    for entity in entities {
        assert_eq!(
            history
                .iter()
                .filter(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::End(end)
                    if end.start_index == entity.oplog_index)
                })
                .count(),
            1,
            "each entity must settle exactly once: {}",
            entity.oplog_index,
        );
    }
    Ok(())
}
