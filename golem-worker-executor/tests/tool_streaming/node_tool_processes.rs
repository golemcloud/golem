// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use std::time::Instant;
use test_r::test;

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
