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
        anyhow::ensure!(cases.split(',').all(|mode| matches!(
            mode,
            "streams-collect"
                | "streams-echo-collect"
                | "streams-pressure-collect"
                | "streams-finite-stdin"
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
        ));
        println!("node-tool-stream starting mode={mode}");
        let started = Instant::now();
        let mut args = vec![mode];
        if let Some(url) = &effect_url {
            args.push(url);
        }
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
        .map_err(|_| anyhow::anyhow!("stream scheduling probe timed out: {mode}"))??;
        assert_eq!(result.exit_code, 0, "{mode}: {result:?}");
        assert!(result.stderr.is_empty(), "{mode}: {result:?}");
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
                    // Cancelling an entity tears down its pending write. That
                    // write stays incomplete; the entity's Cancelled terminal
                    // owns the abandoned subtree during reconstruction.
                    assert_eq!(
                        start.function_name,
                        "golem::tool::streams::tool-output-writer::write"
                    );
                    let parent = start
                        .parent_start_index
                        .expect("an abandoned write belongs to a body");
                    assert!(history.iter().any(|entry| entry.oplog_index == parent &&
                        matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke")));
                    assert!(history.iter().any(|terminal| terminal.oplog_index > entry.oplog_index &&
                        matches!(&terminal.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == parent)),
                        "an incomplete write requires its enclosing entity's cancellation");
                } else {
                    assert_eq!(
                        terminals, 1,
                        "duplicated terminal for Start {}",
                        entry.oplog_index
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
