// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use test_r::{test, timeout};

fn set_effect_fixture_secret(
    environment: &TestEnvironmentStateService,
    environment_id: golem_common::model::environment::EnvironmentId,
) {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_service_base::model::agent_secret::AgentSecret;

    environment.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id,
        path: CanonicalAgentSecretPath(vec!["secret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String("matrix-secret-value".to_string())),
    });
}

#[derive(Debug, FromSchema)]
struct ChunkMEffectRuntimeObservation {
    success_stdout: Vec<u8>,
    success_bytes_read: u64,
    success_outcome: String,
    error_stdout: Vec<u8>,
    error_terminal: String,
}

fn assert_runtime_observation(observation: &ChunkMEffectRuntimeObservation) {
    assert_eq!(
        observation.success_stdout,
        [0, 127, 128, 255, 1, 2, 3, 250, 251]
    );
    assert_eq!(observation.success_bytes_read, 5);
    assert_eq!(observation.success_outcome, "success");
    assert_eq!(observation.error_stdout, [0, 127, 128, 255, 9, 8, 7]);
    assert_eq!(observation.error_terminal, "declared:expected");
}

fn entity_starts_for<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    entity_name: &str,
) -> impl Iterator<Item = &'a PublicOplogEntryWithIndex> {
    oplog.iter().filter(move |entry| {
        matches!(
            &entry.entry,
            PublicOplogEntry::Start(parameters)
                if parameters.function_name == "golem::entity::invoke"
        ) && matches!(
            &entry.attribution,
            PublicOplogEntryAttribution::Entity(entity)
                if entity.invocation.entity.name == entity_name
        )
    })
}

fn assert_every_start_settled(oplog: &[PublicOplogEntryWithIndex]) {
    for start in oplog.iter().filter_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(parameters)
            if parameters.function_name == "golem::entity::invoke" =>
        {
            Some(entry.oplog_index)
        }
        _ => None,
    }) {
        assert!(
            oplog.iter().any(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(parameters) if parameters.start_index == start)
                    || matches!(&entry.entry, PublicOplogEntry::Cancelled(parameters) if parameters.start_index == start)
            }),
            "entity Start {start} has no End or Cancelled terminal"
        );
    }
}

async fn assert_runtime_cleaned_up(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(owner)
                .await
                .is_none_or(|active| {
                    active.tool_operations.operations.is_empty()
                        && active.lane.holder.is_none()
                        && active.lane.active_invocation_count == 0
                        && active.slots.iter().all(|slot| slot.invocations.is_empty())
                })
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Effect Chunk M tool runtime did not clean up"))?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn effect_stream_runtime_middleware_replay_and_cleanup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_effect_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_effect_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    set_effect_fixture_secret(&environment_state, context.default_environment_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let agent_type = AgentTypeName("EffectToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("chunk-m-effect-streaming").unwrap();
    let bare_deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-effect-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(bare_deployment),
    );

    let agent_id = agent_id!("EffectToolStreamingCaller", "chunk-m-effect-stream-runtime");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let bare: ChunkMEffectRuntimeObservation = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "chunk_m_runtime_observation",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_runtime_observation(&bare);
    assert_runtime_cleaned_up(&executor, &owner).await?;

    let bare_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts_for(&bare_oplog, "chunk-m-effect-streaming").count(),
        2,
        "success and declared-error leaves must each execute once"
    );
    assert_every_start_settled(&bare_oplog);

    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:chunk-m-effect-stream-middleware")
        .store()
        .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let universal = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .expect("universal stream middleware fixture");
    let mut middleware_deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-effect-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut middleware_deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            universal.name.as_str(),
            empty_middleware_parameters(universal),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(middleware_deployment),
    );
    executor.simulated_crash(&worker_id).await?;

    let through_universal: ChunkMEffectRuntimeObservation = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "chunk_m_runtime_observation",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_runtime_observation(&through_universal);
    assert_runtime_cleaned_up(&executor, &owner).await?;

    let final_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let effect_leaves =
        entity_starts_for(&final_oplog, "chunk-m-effect-streaming").collect::<Vec<_>>();
    assert_eq!(
        effect_leaves.len(),
        4,
        "replay must not repeat the two bare effects; middleware success/error add two leaves"
    );
    assert!(effect_leaves[2..].iter().all(|entry| {
        matches!(
            &entry.attribution,
            PublicOplogEntryAttribution::Entity(entity)
                if entity.ancestors.last().is_some_and(|ancestor|
                    ancestor.entity.kind == PublicAgentEntityKind::ToolMiddleware
                        && ancestor.entity.name == "streaming-universal-pass-through")
        )
    }));
    assert_every_start_settled(&final_oplog);

    executor.simulated_crash(&worker_id).await?;
    let replay_probe: ChunkMEffectRuntimeObservation = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "chunk_m_runtime_observation",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_runtime_observation(&replay_probe);
    assert_runtime_cleaned_up(&executor, &owner).await?;
    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts_for(&replayed_oplog, "chunk-m-effect-streaming").count(),
        6,
        "completed replay must add only the two newly requested provider effects"
    );
    assert_every_start_settled(&replayed_oplog);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn effect_stream_explicit_cancellation_through_universal_middleware_settles_and_cleans_up(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_effect_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_effect_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    set_effect_fixture_secret(&environment_state, context.default_environment_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:chunk-m-effect-cancellation-middleware")
        .store()
        .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let universal = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .expect("universal stream middleware fixture");
    let agent_type = AgentTypeName("EffectToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("chunk-m-effect-streaming").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-effect-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            universal.name.as_str(),
            empty_middleware_parameters(universal),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("EffectToolStreamingCaller", "chunk-m-effect-cancellation");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let cancellation: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "chunk_m_cancellation_observation",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(cancellation, "invocation-cancelled");
    assert_runtime_cleaned_up(&executor, &owner).await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let effect_leaves = entity_starts_for(&oplog, "chunk-m-effect-streaming").collect::<Vec<_>>();
    assert_eq!(effect_leaves.len(), 1, "cancellation must admit one leaf");
    assert!(matches!(
        &effect_leaves[0].attribution,
        PublicOplogEntryAttribution::Entity(entity)
            if entity.ancestors.last().is_some_and(|ancestor|
                ancestor.entity.kind == PublicAgentEntityKind::ToolMiddleware
                    && ancestor.entity.name == "streaming-universal-pass-through")
    ));
    assert_every_start_settled(&oplog);
    Ok(())
}
