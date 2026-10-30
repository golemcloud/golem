use super::*;
use test_r::test;

#[test]
#[timeout("3m")]
async fn k3_rate_configuration_is_part_of_backend_identity(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, rate_limit)
        .store()
        .await?;
    let old = agent_id!("RateLimitBackend", "same-policy", 1u64, 3_600_000u64);
    let changed_limit = agent_id!("RateLimitBackend", "same-policy", 2u64, 3_600_000u64);
    let changed_window = agent_id!("RateLimitBackend", "same-policy", 1u64, 7_200_000u64);
    assert_ne!(old, changed_limit);
    assert_ne!(old, changed_window);
    assert_ne!(changed_limit, changed_window);

    // Identical logical IDs must still execute independently in each configuration.
    for backend in [&old, &changed_limit, &changed_window] {
        for logical_id in ["first", "second"] {
            executor
                .invoke_and_await_agent(
                    &component,
                    backend,
                    "admit",
                    data_value!("principal", logical_id),
                )
                .await?;
        }
        let stats: RateLimitBackendStats = executor
            .invoke_and_await_agent(&component, backend, "stats", data_value!())
            .await?
            .into_typed()?;
        assert_eq!(stats.attempts, 2);
        assert_eq!(stats.recorded_decisions, 2);
        assert_eq!(
            stats.committed_charges,
            if backend == &changed_limit { 2 } else { 1 }
        );
    }
    // Returning to the old identity reconstructs the old policy, not the latest configuration.
    let old_worker = executor.start_agent(&component.id, old.clone()).await?;
    executor.simulated_crash(&old_worker).await?;
    executor
        .invoke_and_await_agent(&component, &old, "admit", data_value!("principal", "first"))
        .await?;
    let stats: RateLimitBackendStats = executor
        .invoke_and_await_agent(&component, &old, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(stats.attempts, 3);
    assert_eq!(stats.recorded_decisions, 2);
    assert_eq!(stats.committed_charges, 1);
    Ok(())
}

macro_rules! setup_k3_rate_limit_chain_with_checkpoint {
    ($last:expr, $deps:expr, $provider:expr, $caller:expr, $rate_limit:expr,
     $policy:expr, $limit:expr,
     $context:ident, $environment:ident, $executor:ident, $provider_component:ident,
     $caller_component:ident, $rate_limit_component:ident, $deployment:ident, $definition:ident) => {
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
        let $rate_limit_component = $executor
            .component_dep(&$context.default_environment_id, $rate_limit)
            .store()
            .await?;
        let checkpoint_component = $executor
            .component(
                &$context.default_environment_id,
                "golem_it_tool_streaming_rust_middleware_release",
            )
            .name("golem-it:k3-rate-limit-checkpoint")
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
        let rate_limit_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join(format!("{}.wasm", $rate_limit.wasm_name)),
            false,
            true,
        )
        .await?;
        let checkpoint_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
            false,
            true,
        )
        .await?;
        let rate_limit_definition = rate_limit_metadata
            .tool_middlewares
            .iter()
            .find(|definition| definition.name == "persistent-rate-limit")
            .expect("persistent rate-limit middleware metadata");
        let checkpoint_definition = checkpoint_metadata
            .tool_middlewares
            .iter()
            .find(|definition| definition.name == "k3-rate-limit-pre-leaf-checkpoint")
            .expect("K3 pre-leaf checkpoint middleware metadata");
        let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
        let tool_name = ToolName::try_from("middleware-probe").unwrap();
        let mut deployment = deployment_state(
            $context.account_id,
            $provider_component.id,
            $provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            agent_type.0.as_str(),
            provider_metadata.tools,
        );
        install_middleware_chain(
            &mut deployment,
            &agent_type,
            &tool_name,
            checkpoint_component.id,
            checkpoint_component.revision,
            "golem-it:k3-rate-limit-checkpoint",
            &checkpoint_metadata.tool_middlewares,
            vec![(
                checkpoint_definition.name.as_str(),
                empty_middleware_parameters(checkpoint_definition),
            )],
        );
        let checkpoint_occurrence = deployment.tool_middleware_chains
            [&ToolBindingOwner::AgentType {
                agent_type_name: agent_type.clone(),
            }][&tool_name]
            .occurrences[0]
            .clone();
        install_middleware_chain(
            &mut deployment,
            &agent_type,
            &tool_name,
            $rate_limit_component.id,
            $rate_limit_component.revision,
            "golem:rate-limit-middleware",
            &rate_limit_metadata.tool_middlewares,
            vec![(
                rate_limit_definition.name.as_str(),
                rate_limit_parameters(rate_limit_definition, $policy, $limit, 3_600_000),
            )],
        );
        deployment
            .tool_middleware_chains
            .get_mut(&ToolBindingOwner::AgentType {
                agent_type_name: agent_type,
            })
            .unwrap()
            .get_mut(&tool_name)
            .unwrap()
            .occurrences
            .push(checkpoint_occurrence);
        let mut $deployment = deployment.clone();
        let $definition = rate_limit_definition.clone();
        $environment.set_tool_deployment(
            $context.default_environment_id,
            $caller_component.id,
            $caller_component.revision,
            Some(deployment),
        );
    };
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn k3_rate_1_capacity_is_independent_between_principals(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_rate_limit_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        rate_limit,
        "k3-rate-1-principal",
        1,
        context,
        environment,
        executor,
        provider_component,
        caller_component,
        rate_limit_component,
        agent_type
    );
    let principal_a = oidc_principal("k3-principal-a");
    let principal_b = oidc_principal("k3-principal-b");
    let owner_a = agent_id!("ToolStreamingCaller", "k3-rate-1-owner-a");
    let owner_b = agent_id!("ToolStreamingCaller", "k3-rate-1-owner-b");
    let worker_a = executor
        .start_agent(&caller_component.id, owner_a.clone())
        .await?;
    let worker_b = executor
        .start_agent(&caller_component.id, owner_b.clone())
        .await?;

    let first_a: String = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &owner_a,
            principal_a.clone(),
            "middleware_probe_once",
            data_value!("owner-a-first"),
        )
        .await?
        .into_typed()?;
    assert_eq!(first_a, "leaf(owner-a-first)");

    let exhausted_a = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &owner_a,
            principal_a,
            "middleware_probe_once",
            data_value!("owner-a-second"),
        )
        .await;
    assert!(
        exhausted_a.is_err(),
        "call N+1 for one principal key must fail"
    );

    let first_b: String = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &owner_b,
            principal_b,
            "middleware_probe_once",
            data_value!("owner-b-first"),
        )
        .await?
        .into_typed()?;
    assert_eq!(first_b, "leaf(owner-b-first)");

    let oplog_a = executor.get_oplog(&worker_a, OplogIndex::INITIAL).await?;
    let oplog_b = executor.get_oplog(&worker_b, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts(&oplog_a, PublicAgentEntityKind::Tool, "middleware-probe").len()
            + entity_starts(&oplog_b, PublicAgentEntityKind::Tool, "middleware-probe").len(),
        2,
        "the exhausted principal must not consume the independent principal's capacity"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn k3_rate_2_distinct_owners_contend_on_one_principal_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const LIMIT: usize = 11;
    const OWNERS: usize = 32;
    setup_rate_limit_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        rate_limit,
        "k3-rate-2-contention",
        LIMIT as u64,
        context,
        environment,
        executor,
        provider_component,
        caller_component,
        rate_limit_component,
        agent_type
    );
    let mut owners = Vec::with_capacity(OWNERS);
    for ordinal in 0..OWNERS {
        let agent_id = agent_id!("ToolStreamingCaller", format!("k3-rate-2-owner-{ordinal}"));
        let worker_id = executor
            .start_agent(&caller_component.id, agent_id.clone())
            .await?;
        owners.push((agent_id, worker_id));
    }

    let barrier = Arc::new(tokio::sync::Barrier::new(OWNERS));
    let principal = oidc_principal("k3-shared-principal");
    let calls = owners.iter().enumerate().map(|(ordinal, (agent_id, _))| {
        let barrier = barrier.clone();
        let principal = principal.clone();
        let executor = &executor;
        let caller_component = &caller_component;
        let agent_id = agent_id.clone();
        async move {
            barrier.wait().await;
            executor
                .invoke_and_await_agent_as_principal(
                    caller_component,
                    &agent_id,
                    principal,
                    "middleware_probe_once",
                    data_value!(format!("k3-owner-{ordinal}")),
                )
                .await
        }
    });
    let results = futures::future::join_all(calls).await;
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        LIMIT
    );
    assert_eq!(
        results.iter().filter(|result| result.is_err()).count(),
        OWNERS - LIMIT
    );

    let mut leaf_effects = 0;
    for (_, worker_id) in &owners {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        leaf_effects +=
            entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe").len();
    }
    assert_eq!(leaf_effects, LIMIT, "only committed charges reach the leaf");

    let backend_id = agent_id!(
        "RateLimitBackend",
        "k3-rate-2-contention",
        LIMIT as u64,
        3_600_000u64
    );
    let stats: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(stats.attempts, OWNERS as u64);
    assert_eq!(stats.committed_charges, LIMIT as u64);
    assert_eq!(stats.recorded_decisions, OWNERS as u64);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn k3_rate_3_post_admission_pre_leaf_crash_replays_without_extra_charge_or_effect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_k3_rate_limit_chain_with_checkpoint!(
        last_unique_id,
        deps,
        provider,
        caller,
        rate_limit,
        "k3-rate-3-replay",
        2,
        context,
        environment,
        executor,
        provider_component,
        caller_component,
        rate_limit_component,
        deployment,
        definition
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let (checkpoint_port, checkpoint_server, mut arrivals) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "k3-rate-3-owner");
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
                    "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent_as_principal(
        &caller_component,
        &agent_id,
        oidc_principal("k3-replay-principal"),
        "middleware_probe_once",
        data_value!("lifecycle-effect(k3-pre-leaf)"),
    );
    tokio::pin!(invocation);

    let checkpoint = tokio::select! {
        checkpoint = next_promise_checkpoint(
            &mut arrivals,
            "k3-rate-limit-post-admission-pre-leaf"
        ) => checkpoint?,
        result = invocation.as_mut() => panic!("K3 call settled before post-admission checkpoint: {result:?}"),
    };
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;

    let backend_id = agent_id!("RateLimitBackend", "k3-rate-3-replay", 2u64, 3_600_000u64);
    let at_checkpoint: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(at_checkpoint.attempts, 1);
    assert_eq!(at_checkpoint.committed_charges, 1);
    assert_eq!(at_checkpoint.recorded_decisions, 1);
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let before_crash = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        entity_starts(
            &before_crash,
            PublicAgentEntityKind::Tool,
            "middleware-probe"
        )
        .is_empty(),
        "the leaf must not start before the post-admission checkpoint"
    );

    // Fresh calls select the new configuration, but the in-flight occurrence remains pinned.
    deployment.deployment_revision = DeploymentRevision::try_from(2u64).unwrap();
    for tool in deployment.registered_tools.values_mut() {
        tool.deployment_revision = deployment.deployment_revision;
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.deployment_revision = deployment.deployment_revision;
        }
    }
    for middleware in deployment.registered_tool_middlewares.values_mut() {
        middleware.deployment_revision = deployment.deployment_revision;
    }
    // The checkpoint is needed only for the old in-flight call.
    install_middleware_chain(
        &mut deployment,
        &AgentTypeName("ToolStreamingCaller".into()),
        &ToolName::try_from("middleware-probe").unwrap(),
        rate_limit_component.id,
        rate_limit_component.revision,
        "golem:rate-limit-middleware",
        std::slice::from_ref(&definition),
        vec![(
            definition.name.as_str(),
            rate_limit_parameters(&definition, "k3-rate-3-replay", 1, 7_200_000),
        )],
    );
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    executor.simulated_crash(&worker_id).await?;
    let after_crash: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(after_crash.attempts, 1);
    assert_eq!(after_crash.committed_charges, 1);
    assert_eq!(after_crash.recorded_decisions, 1);

    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: checkpoint.oplog_idx,
            },
            vec![],
        )
        .await?;
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "leaf(lifecycle-effect(k3-pre-leaf))");
    assert_eq!(
        effects.recv().await.as_deref(),
        Some("lifecycle-effect(k3-pre-leaf)")
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let after_replay: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(
        after_replay.attempts, 1,
        "replay is not a new admission attempt"
    );
    assert_eq!(after_replay.committed_charges, 1);
    assert_eq!(after_replay.recorded_decisions, 1);
    let after_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts(
            &after_oplog,
            PublicAgentEntityKind::Tool,
            "middleware-probe"
        )
        .len(),
        1,
        "recovery must dispatch the leaf exactly once"
    );

    let fresh: String = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            oidc_principal("k3-replay-principal"),
            "middleware_probe_once",
            data_value!("fresh-after-configuration-change"),
        )
        .await?
        .into_typed()?;
    assert_eq!(fresh, "leaf(fresh-after-configuration-change)");
    let changed_backend = agent_id!("RateLimitBackend", "k3-rate-3-replay", 1u64, 7_200_000u64);
    let changed_stats: RateLimitBackendStats = executor
        .invoke_and_await_agent(
            &rate_limit_component,
            &changed_backend,
            "stats",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(changed_stats.attempts, 1);
    assert_eq!(changed_stats.committed_charges, 1);
    let old_stats: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(old_stats.attempts, 1);
    assert_eq!(old_stats.committed_charges, 1);

    checkpoint_server.abort();
    effect_server.abort();
    Ok(())
}
