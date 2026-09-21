// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);

macro_rules! setup_probe_chain {
    ($last:expr, $deps:expr, $provider:expr, $caller:expr, $names:expr,
     $context:ident, $environment:ident, $executor:ident, $provider_component:ident,
     $caller_component:ident, $middleware_metadata:ident, $agent_type:ident, $deployment:ident) => {
        let $context = TestContext::new($last);
        let $environment = Arc::new(TestEnvironmentStateService::default());
        let $executor = start_with_overrides(
            $deps,
            &$context,
            TestExecutorOverrides {
                environment_state_service: Some($environment.clone()),
                ..Default::default()
            },
        )
        .await?;
        let $provider_component = $executor
            .component_dep(&$context.default_environment_id, $provider)
            .store()
            .await?;
        let $caller_component = $executor
            .component_dep(&$context.default_environment_id, $caller)
            .store()
            .await?;
        let middleware_component = $executor
            .component(
                &$context.default_environment_id,
                "golem_it_tool_streaming_rust_middleware_release",
            )
            .name("golem-it:tool-streaming-middleware-step-acceptance")
            .store()
            .await?;
        let provider_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join(format!("{}.wasm", $provider.wasm_name)),
            false,
            true,
        )
        .await?;
        let $middleware_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
            false,
            true,
        )
        .await?;
        let $agent_type = AgentTypeName("ToolStreamingCaller".to_string());
        let tool_name = ToolName::try_from("middleware-probe").unwrap();
        let mut $deployment = deployment_state(
            $context.account_id,
            $provider_component.id,
            $provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            $agent_type.0.as_str(),
            provider_metadata.tools,
        );
        let occurrences = $names
            .iter()
            .map(|name| {
                let definition = $middleware_metadata
                    .tool_middlewares
                    .iter()
                    .find(|definition| definition.name == *name)
                    .unwrap();
                (
                    definition.name.as_str(),
                    empty_middleware_parameters(definition),
                )
            })
            .collect();
        install_middleware_chain(
            &mut $deployment,
            &$agent_type,
            &tool_name,
            middleware_component.id,
            middleware_component.revision,
            "golem-it:tool-streaming-rust-middleware",
            &$middleware_metadata.tool_middlewares,
            occurrences,
        );
    };
}

async fn start_probe_effect_server() -> (
    u16,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::task::JoinHandle<()>,
) {
    async fn effect(
        Path(value): Path<String>,
        State(effects): State<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> axum::http::StatusCode {
        effects.send(value).expect("record middleware probe effect");
        axum::http::StatusCode::NO_CONTENT
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (effects, received) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/{value}", post(effect))
                .with_state(effects),
        )
        .await
        .expect("serve middleware probe effects");
    });
    (port, received, task)
}

fn entity_starts<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    kind: PublicAgentEntityKind,
    name: &str,
) -> Vec<&'a PublicOplogEntryWithIndex> {
    oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            ) && matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == kind
                        && entity.invocation.entity.name == name
            )
        })
        .collect()
}

fn assert_one_terminal_before_finished(
    oplog: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    finished: OplogIndex,
) {
    let terminals = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)
                || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminals.len(),
        1,
        "entity {start} must settle exactly once before Finished"
    );
    assert!(terminals[0].oplog_index < finished);
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn universal_middleware_preserves_native_modes_effects_and_completed_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let mut streaming = native_streaming_tool_metadata();
    streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-native-middleware-acceptance")
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
        .expect("streaming universal middleware fixture");
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("native-streaming").unwrap();
    let mut deployment = native_deployment_state(
        context.account_id,
        agent_type.0.as_str(),
        streaming,
        native_test_tool_metadata(),
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

    let agent_id = agent_id!("ToolStreamingCaller", "native-middleware-acceptance");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let helper_effects_before = executor.native_test_helper_effect_count();
    let evidence: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "native_modes_stream_cancel_overlap",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        &evidence[..6],
        [
            "sync",
            "fire-and-forget",
            "async",
            "stream",
            "cancel",
            "overlap"
        ]
    );
    assert_eq!(evidence[6], "5");
    let helper_effects = executor.native_test_helper_effect_count();
    assert_eq!(helper_effects, helper_effects_before + 1);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let native_leaves = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            ) && matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == PublicAgentEntityKind::Tool
                        && entity.invocation.entity.name == "native-streaming"
                        && entity.ancestors.last().is_some_and(|ancestor|
                            ancestor.entity.kind == PublicAgentEntityKind::ToolMiddleware
                                && ancestor.entity.name == "streaming-universal-pass-through")
            )
        })
        .count();
    assert!(
        native_leaves >= 6,
        "every native call mode must enter through middleware"
    );

    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    let count: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "native_effect_count",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(count, "5");
    assert_eq!(executor.native_test_helper_effect_count(), helper_effects);

    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let replayed_native_leaves = replayed_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            ) && matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == PublicAgentEntityKind::Tool
                        && entity.invocation.entity.name == "native-streaming"
                        && entity.ancestors.last().is_some_and(|ancestor|
                            ancestor.entity.kind == PublicAgentEntityKind::ToolMiddleware
                                && ancestor.entity.name == "streaming-universal-pass-through")
            )
        })
        .count();
    assert_eq!(replayed_native_leaves, native_leaves + 1);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn middleware_race_cancels_loser_drops_observer_and_settles_all_siblings(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-race-settlement"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let (promise_port, promise_server, mut promise_arrivals) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-race-settlement");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
                (
                    "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                    promise_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("asymmetric-17"),
    );
    tokio::pin!(invocation);
    let cancelled = tokio::select! {
        result = invocation.as_mut() => panic!("race settled before loser admission: {result:?}"),
        arrival = next_crash_checkpoint(&mut arrivals, "middleware-race-cancelled") => arrival?,
    };
    let cancel_ready =
        next_promise_checkpoint(&mut promise_arrivals, "middleware-race-cancel-ready").await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: cancel_ready.oplog_idx,
            },
            vec![],
        )
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(&owner)
                .await
                .is_some_and(|active| {
                    active.tool_operations.operations.iter().any(|operation| {
                        matches!(
                            operation.winner,
                            ToolOperationWinnerMetadata::SelectingCancelled
                                | ToolOperationWinnerMetadata::Cancelled
                        )
                    })
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for loser cancellation selection"))?;
    drop(cancelled);
    let detached = tokio::select! {
        result = invocation.as_mut() => panic!("parent settled before detached child: {result:?}"),
        arrival = next_crash_checkpoint(&mut arrivals, "middleware-race-detached") => arrival?,
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );
    detached
        .release
        .send(())
        .expect("release detached observer child");
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "race-winner[leaf(race-sibling(asymmetric-17))]");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let leaf_starts = oplog
        .iter()
        .filter_map(|entry| match (&entry.entry, &entry.attribution) {
            (PublicOplogEntry::Start(parameters), PublicOplogEntryAttribution::Entity(entity))
                if parameters.function_name == "golem::entity::invoke"
                    && entity.invocation.entity.kind == PublicAgentEntityKind::Tool
                    && entity.invocation.entity.name == "middleware-probe" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        leaf_starts.len(),
        3,
        "cancelled, detached, and successful siblings are admitted"
    );
    assert!(leaf_starts.iter().all(|start| oplog.iter().any(|entry|
        matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == *start)
            || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == *start)
    )), "every admitted sibling must have settled before the parent result");
    assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if leaf_starts.contains(&cancelled.start_index))).count(), 1);
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("race invocation finished")
        .oplog_index;
    for start in leaf_starts {
        assert_one_terminal_before_finished(&oplog, start, finished);
    }
    let middleware_starts = entity_starts(
        &oplog,
        PublicAgentEntityKind::ToolMiddleware,
        "streaming-race-settlement",
    );
    assert_eq!(middleware_starts.len(), 1);
    assert_one_terminal_before_finished(&oplog, middleware_starts[0].oplog_index, finished);
    checkpoint_server.abort();
    promise_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn filesystem_capable_incapable_capable_chain_serializes_fanout_lanes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &[
            "streaming-filesystem-outer",
            "streaming-incapable-fanout",
            "streaming-filesystem-inner"
        ],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let chain = deployment
        .tool_middleware_chains
        .get_mut(&ToolBindingOwner::AgentType {
            agent_type_name: agent_type.clone(),
        })
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    for occurrence in &mut chain.occurrences {
        if occurrence.middleware.definition.name != "streaming-incapable-fanout" {
            occurrence.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-filesystem-lanes");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let result: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_once",
            data_value!("seed-29"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        result,
        "fanout[leaf(fs-inner(fanout-left(fs-outer(seed-29))))|leaf(fs-inner(fanout-right(fs-outer(seed-29))))]"
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/middleware-lanes.log")
            .await?,
        b"OLR".as_slice()
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn incapable_middleware_preserves_capable_streaming_staging_and_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-universal-pass-through"],
        context,
        environment_state,
        executor,
        _provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let middleware = deployment
        .registered_tool_middlewares
        .values()
        .next()
        .unwrap();
    let ToolMiddlewareSource::Component {
        component_id,
        component_revision,
        ..
    } = middleware.source.clone();
    let definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .unwrap();
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &ToolName::try_from("capable-streaming").unwrap(),
        component_id,
        component_revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            definition.name.as_str(),
            empty_middleware_parameters(definition),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-capable-stream-staging");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let input = b"capable-inner-through-incapable-outer-37".to_vec();
    let result: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/decorated-capable.bin", input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&result, &input, 1, input.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/decorated-capable.bin")
            .await?,
        input
    );
    executor.simulated_crash(&worker_id).await?;
    let next = b"fresh-after-reconstruction-19".to_vec();
    let result: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/decorated-capable-next.bin", next.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&result, &next, 1, next.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/decorated-capable.bin")
            .await?,
        input
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .unwrap()
        .oplog_index;
    for start in entity_starts(&oplog, PublicAgentEntityKind::Tool, "capable-streaming")
        .into_iter()
        .chain(entity_starts(
            &oplog,
            PublicAgentEntityKind::ToolMiddleware,
            "streaming-universal-pass-through",
        ))
    {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn partial_fanout_restart_replays_completed_child_repairs_pending_and_keeps_pinned_plan(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-partial-fanout"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let original_deployment = deployment.clone();
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-partial-fanout-restart");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("restart-43"),
    );
    tokio::pin!(invocation);
    let first_effect = tokio::select! {
        effect = effects.recv() => effect.expect("first child effect"),
        result = invocation.as_mut() => panic!("partial fanout settled before its pending child: {result:?}"),
    };
    let second_effect = tokio::select! {
        effect = effects.recv() => effect.expect("second child effect"),
        result = invocation.as_mut() => panic!("partial fanout settled before its pending child: {result:?}"),
    };
    assert!(
        [&first_effect, &second_effect]
            .iter()
            .any(|effect| effect.starts_with("partial-completed("))
    );
    assert!(
        [&first_effect, &second_effect]
            .iter()
            .any(|effect| effect.starts_with("partial-pending("))
    );
    let _original_pending =
        next_crash_checkpoint(&mut arrivals, "middleware-partial-pending").await?;
    let completed_start = wait_for_completed_entity_terminal(&executor, &worker_id).await?;
    let before_crash = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        entity_starts(
            &before_crash,
            PublicAgentEntityKind::Tool,
            "middleware-probe"
        )
        .iter()
        .any(|entry| entry.oplog_index == completed_start)
    );

    let short = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-short-circuit")
        .unwrap();
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut changed = original_deployment;
    let (middleware_component_id, middleware_component_revision) = match &changed
        .tool_middleware_chains[&ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    }][&tool_name]
        .occurrences[0]
        .middleware
        .source
    {
        ToolMiddlewareSource::Component {
            component_id,
            component_revision,
            ..
        } => (*component_id, *component_revision),
    };
    install_middleware_chain(
        &mut changed,
        &agent_type,
        &tool_name,
        middleware_component_id,
        middleware_component_revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(short.name.as_str(), empty_middleware_parameters(short))],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(changed),
    );
    executor.simulated_crash(&worker_id).await?;
    let replay_pending = tokio::select! {
        checkpoint = next_crash_checkpoint(&mut arrivals, "middleware-partial-pending") => checkpoint?,
        result = invocation.as_mut() => panic!("partial fanout settled before its repaired child was released: {result:?}"),
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), effects.recv())
            .await
            .is_err(),
        "neither independently completed effect may repeat during replay"
    );
    replay_pending
        .release
        .send(())
        .expect("release repaired pending child");
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(
        result, "partial[leaf(partial-completed(restart-43))|leaf(partial-pending(restart-43))]",
        "the in-flight invocation must retain its recorded middleware plan"
    );
    assert_eq!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let middleware_starts = entity_starts(
        &oplog,
        PublicAgentEntityKind::ToolMiddleware,
        "streaming-partial-fanout",
    );
    let leaf_starts = entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe");
    assert_eq!(middleware_starts.len(), 1);
    assert_eq!(leaf_starts.len(), 2);
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("partial fanout invocation finished")
        .oplog_index;
    for start in middleware_starts.into_iter().chain(leaf_starts) {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }

    let fresh: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_once",
            data_value!("control"),
        )
        .await?
        .into_typed()?;
    assert_eq!(fresh, "short(control)");
    assert_eq!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    checkpoint_server.abort();
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn early_return_parent_end_survives_restart_while_detached_child_is_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-early-return"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (promise_port, promise_server, mut arrivals) = start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-early-return-restart");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "PROVIDER_PROMISE_CHECKPOINT_PORT".to_string(),
                promise_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("restart-parent"),
    );
    tokio::pin!(invocation);
    let child = tokio::select! {
        result = invocation.as_mut() => panic!("parent settled before detached child: {result:?}"),
        child = next_promise_checkpoint(&mut arrivals, "middleware-early-child") => child?,
    };
    let parent_start = wait_for_completed_entity_terminal(&executor, &worker_id).await?;
    let before_crash = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        entity_starts(
            &before_crash,
            PublicAgentEntityKind::ToolMiddleware,
            "streaming-early-return"
        )
        .iter()
        .any(|entry| entry.oplog_index == parent_start)
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );

    executor.simulated_crash(&worker_id).await?;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: child.oplog_idx,
            },
            vec![],
        )
        .await?;
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "early-return(restart-parent)");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let middleware_starts = entity_starts(
        &oplog,
        PublicAgentEntityKind::ToolMiddleware,
        "streaming-early-return",
    );
    let leaf_starts = entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe");
    assert_eq!(middleware_starts.len(), 1);
    assert_eq!(leaf_starts.len(), 1);
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("early-return invocation finished")
        .oplog_index;
    for start in middleware_starts.into_iter().chain(leaf_starts) {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }
    promise_server.abort();
    Ok(())
}
