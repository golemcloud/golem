// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::entity::EnvOverlay;
use golem_common::model::oplog::PublicEntityInvocationContext;
use golem_service_base::error::worker_executor::WorkerExecutorError;
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

#[derive(Debug, Clone, PartialEq, Eq, FromSchema)]
struct EnvOverlayObservation {
    term: Option<String>,
    columns: Option<String>,
    lines: Option<String>,
    golem_agent_id: Option<String>,
}

fn overlay(entries: &[(&str, &str)]) -> EnvOverlay {
    entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn start_probe(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    key: &IdempotencyKey,
    principal: &Principal,
    env_overlay: EnvOverlay,
) -> tokio::task::JoinHandle<Result<AgentInvocationResult, WorkerExecutorError>> {
    let executor = executor.clone();
    let worker_id = worker_id.clone();
    let key = key.clone();
    let principal = principal.clone();
    tokio::spawn(async move {
        executor
            .invoke_external_tool_with_env_overlay(
                executor.context.default_environment_id,
                &worker_id,
                fingerprint,
                key,
                ToolName::try_from("env-overlay-probe").unwrap(),
                vec!["observe".to_string()],
                TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::record(vec![])),
                    SchemaValue::Record { fields: vec![] },
                ),
                InvocationContextStack::fresh(),
                principal,
                None,
                env_overlay,
            )
            .await
            .map(|output| output.result)
    })
}

fn observation(result: AgentInvocationResult) -> anyhow::Result<EnvOverlayObservation> {
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = result else {
        anyhow::bail!("expected a successful env-overlay-probe result, got {result:?}");
    };
    let value = result
        .result
        .map(|value| value.into_parts().1)
        .ok_or_else(|| anyhow::anyhow!("env-overlay-probe returned no value"))?;
    EnvOverlayObservation::from_value(&value).map_err(|error| anyhow::anyhow!("{error:?}"))
}

async fn release_checkpoint(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    checkpoint: &PromiseCheckpointArrival,
) -> anyhow::Result<()> {
    executor
        .complete_promise(
            &executor.promise_id(worker_id, checkpoint.oplog_idx).await?,
            vec![],
        )
        .await?;
    Ok(())
}

/// An env overlay given to an external tool start reaches every middleware layer and the leaf
/// tool of that invocation only. The owner's primary instance never sees it, concurrent
/// invocations on other owners keep their own overlay, the overlay survives an executor restart in
/// the middle of the chain, and it is part of the invocation's idempotent identity.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn env_overlay_is_inherited_by_nested_calls_of_its_invocation_only(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
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
        .name("golem-it:tool-streaming-rust-middleware")
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
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let layer = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "env-overlay-layer")
        .unwrap();
    let agent_type = AgentTypeName("EnvOverlayCaller".to_string());
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &ToolName::try_from("env-overlay-probe").unwrap(),
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (layer.name.as_str(), empty_middleware_parameters(layer)),
            (layer.name.as_str(), empty_middleware_parameters(layer)),
        ],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let (promise_port, promise_server, mut checkpoints) = start_promise_checkpoint_server().await;
    let owner_env = HashMap::from([
        (
            "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
            promise_port.to_string(),
        ),
        ("TERM".to_string(), "owner-term".to_string()),
    ]);
    let agent_id = agent_id!("EnvOverlayCaller", "env-overlay");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            owner_env.clone(),
            Vec::new(),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let other_worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id!("EnvOverlayCaller", "env-overlay-other"),
            owner_env,
            Vec::new(),
        )
        .await?;
    let other_fingerprint = executor
        .get_worker_metadata(&other_worker_id)
        .await?
        .fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let observe_primary = || async {
        executor
            .invoke_and_await_agent(
                &caller_component,
                &agent_id,
                "observe_env_overlay",
                data_value!(),
            )
            .await?
            .into_typed::<EnvOverlayObservation>()
    };

    let primary = observe_primary().await?;
    assert_eq!(primary.columns, None);
    assert_eq!(primary.lines, None);

    // Hold overlaid invocations of two owners inside their first middleware layer at once. One
    // owner runs one invocation at a time, so concurrent overlays live on different owners.
    let first_key = IdempotencyKey::fresh();
    let first = start_probe(
        &executor,
        &worker_id,
        fingerprint,
        &first_key,
        &principal,
        overlay(&[
            ("COLUMNS", "80"),
            ("TERM", "xterm-256color"),
            ("GOLEM_AGENT_ID", "spoofed"),
        ]),
    );
    let first_outer = next_promise_checkpoint(&mut checkpoints, "env-overlay-layer-80").await?;
    let second_key = IdempotencyKey::fresh();
    let second = start_probe(
        &executor,
        &other_worker_id,
        other_fingerprint,
        &second_key,
        &principal,
        overlay(&[("COLUMNS", "132"), ("LINES", "50")]),
    );
    let second_outer = next_promise_checkpoint(&mut checkpoints, "env-overlay-layer-132").await?;

    release_checkpoint(&executor, &worker_id, &first_outer).await?;
    let first_inner = next_promise_checkpoint(&mut checkpoints, "env-overlay-layer-80").await?;
    release_checkpoint(&executor, &worker_id, &first_inner).await?;
    let first_observed = observation(first.await??)?;
    assert_eq!(
        first_observed,
        EnvOverlayObservation {
            term: Some("xterm-256color".to_string()),
            columns: Some("80".to_string()),
            lines: None,
            golem_agent_id: primary.golem_agent_id.clone(),
        },
        "the overlay replaces owner variables but never Golem variables"
    );

    assert_eq!(
        observe_primary().await?,
        primary,
        "the primary instance must not keep the overlay of an earlier invocation"
    );

    release_checkpoint(&executor, &other_worker_id, &second_outer).await?;
    let second_inner = next_promise_checkpoint(&mut checkpoints, "env-overlay-layer-132").await?;
    release_checkpoint(&executor, &other_worker_id, &second_inner).await?;
    assert_eq!(
        observation(second.await??)?,
        EnvOverlayObservation {
            term: primary.term.clone(),
            columns: Some("132".to_string()),
            lines: Some("50".to_string()),
            golem_agent_id: Some("EnvOverlayCaller(\"env-overlay-other\")".to_string()),
        },
        "each invocation sees only its own overlay"
    );

    executor.commit_oplog(&worker_id).await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let leaf_chains = oplog
        .iter()
        .filter_map(|entry| match &entry.attribution {
            PublicOplogEntryAttribution::Entity(PublicEntityInvocationContext {
                invocation,
                ancestors,
            }) if invocation.entity.kind == PublicAgentEntityKind::Tool && ancestors.len() == 2 => {
                Some(
                    std::iter::once(invocation)
                        .chain(ancestors.iter())
                        .map(|invocation| invocation.env_overlay.clone())
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let recorded_first = Some(BTreeMap::from([
        ("COLUMNS".to_string(), "80".to_string()),
        ("GOLEM_AGENT_ID".to_string(), "spoofed".to_string()),
        ("TERM".to_string(), "xterm-256color".to_string()),
    ]));
    assert_eq!(
        leaf_chains,
        BTreeSet::from([vec![None; 3], vec![recorded_first; 3]]),
        "every entity of an overlaid chain records the overlay, and primary chains record none"
    );

    // Lose the executor while an overlaid invocation waits inside its first layer.
    let restart_key = IdempotencyKey::fresh();
    let restart_overlay = overlay(&[("COLUMNS", "99")]);
    let interrupted = start_probe(
        &executor,
        &worker_id,
        fingerprint,
        &restart_key,
        &principal,
        restart_overlay.clone(),
    );
    let restart_outer = next_promise_checkpoint(&mut checkpoints, "env-overlay-layer-99").await?;
    wait_for_promise_checkpoint_to_await(&executor, &worker_id, &restart_outer).await?;
    interrupted.abort();
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides).await?;
    let resumed = start_probe(
        &executor,
        &worker_id,
        fingerprint,
        &restart_key,
        &principal,
        restart_overlay.clone(),
    );
    release_checkpoint(&executor, &worker_id, &restart_outer).await?;
    let restart_inner = next_promise_checkpoint(&mut checkpoints, "env-overlay-layer-99")
        .await
        .map_err(|error| {
            anyhow::anyhow!("the inner layer must see the recovered overlay: {error}")
        })?;
    release_checkpoint(&executor, &worker_id, &restart_inner).await?;
    let expected_restart = EnvOverlayObservation {
        term: primary.term.clone(),
        columns: Some("99".to_string()),
        lines: None,
        golem_agent_id: primary.golem_agent_id.clone(),
    };
    assert_eq!(observation(resumed.await??)?, expected_restart);

    assert_eq!(
        observation(
            start_probe(
                &executor,
                &worker_id,
                fingerprint,
                &restart_key,
                &principal,
                restart_overlay,
            )
            .await??
        )?,
        expected_restart,
        "a completed invocation returns its recorded result"
    );
    let conflict = start_probe(
        &executor,
        &worker_id,
        fingerprint,
        &restart_key,
        &principal,
        overlay(&[("COLUMNS", "100")]),
    )
    .await?
    .expect_err("the same key with another overlay is a different request");
    assert!(
        conflict.to_string().contains("IdempotencyConflict"),
        "unexpected error: {conflict}"
    );

    promise_server.abort();
    Ok(())
}
