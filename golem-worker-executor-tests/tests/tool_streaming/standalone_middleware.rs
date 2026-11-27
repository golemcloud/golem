// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::standalone_fixture::rebind_to_baseline;
use super::*;
use golem_api_grpc::proto::golem::worker::InvocationSessionResult;
use golem_common::model::AgentInvocation;
use golem_common::model::agent::AgentInvocationPayload;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
use golem_common::model::component::{ComponentDto, InstalledPlugin, PluginPriority};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::tool::ToolActivationSnapshot;
use golem_worker_executor::services::oplog::OplogOps;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);

struct MiddlewareServers {
    effect_port: u16,
    promise_port: u16,
    effects: tokio::sync::mpsc::UnboundedReceiver<String>,
    promises: tokio::sync::mpsc::UnboundedReceiver<PromiseCheckpointArrival>,
    effect_server: tokio::task::JoinHandle<()>,
    promise_server: tokio::task::JoinHandle<()>,
}

impl MiddlewareServers {
    async fn start() -> Self {
        let (effect_port, effects, effect_server) =
            middleware_acceptance::start_probe_effect_server().await;
        let (promise_port, promise_server, promises) = start_promise_checkpoint_server().await;
        Self {
            effect_port,
            promise_port,
            effects,
            promises,
            effect_server,
            promise_server,
        }
    }
}

#[test]
#[timeout("3m")]
async fn standalone_middleware_waits_survive_restarts_before_and_after_leaf(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let baseline_plugins = Arc::new(std::sync::RwLock::new(BaselineOwnerPlugins::default()));
    let mut processor_effects = ProcessorEffects::start().await?;
    let MiddlewareServers {
        effect_port,
        promise_port,
        mut effects,
        mut promises,
        effect_server,
        promise_server: _promises,
    } = MiddlewareServers::start().await;
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        wrap_component_service: Some(Arc::new({
            let baseline_plugins = baseline_plugins.clone();
            move |inner| {
                Arc::new(BaselineOwnerComponentService {
                    inner,
                    owner_loads: Default::default(),
                    http_port: 0,
                    plugins: baseline_plugins.clone(),
                    extra_env: BTreeMap::from([
                        (
                            "MIDDLEWARE_PROBE_EFFECT_PORT".into(),
                            effect_port.to_string(),
                        ),
                        (
                            "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".into(),
                            promise_port.to_string(),
                        ),
                    ]),
                })
            }
        })),
        configure: Some(Arc::new(|config| {
            config.oplog.plugin_max_commit_count = 1;
            config.oplog.plugin_max_elapsed_time = Duration::from_millis(100);
        })),
        ..Default::default()
    }
    .with_agent_sockets();
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .name("virtual-native-policy")
        .store()
        .await?;
    let middleware = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .store()
        .await?;
    let processor = executor
        .component(&context.default_environment_id, "oplog_processor_release")
        .store()
        .await?;
    let plugins: Vec<_> = ["a", "b", "c", "d", "x"]
        .into_iter()
        .map(|name| processor_effects.plugin(&context, &processor, name))
        .collect();
    let hosted_plugin = processor_effects.plugin(&context, &processor, "y");
    *baseline_plugins.write().unwrap() = BaselineOwnerPlugins {
        baseline: vec![plugins[4].clone()],
        agent: vec![hosted_plugin.clone()],
    };
    let metadata = extract_component_metadata(
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
    let agent_type = AgentTypeName("ToolStreamingCaller".into());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        &agent_type.0,
        metadata.tools,
    );
    let lifecycle = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-lifecycle")
        .unwrap();
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware.id,
        middleware.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            lifecycle.name.as_str(),
            empty_middleware_parameters(lifecycle),
        )],
    );
    let baseline = ToolBindingOwner::ComponentBaseline {
        component_id: component.id,
    };
    rebind_to_baseline(&mut deployment, component.id);
    deployment
        .registered_tools
        .get_mut(&tool_name)
        .unwrap()
        .registration
        .provision
        .plugins = plugins[..2].to_vec();
    deployment
        .registered_tools
        .get_mut(&tool_name)
        .unwrap()
        .registration
        .provision
        .env
        .insert(
            "MIDDLEWARE_PROBE_EFFECT_PORT".into(),
            effect_port.to_string(),
        );
    let mut repeated_a = plugins[0].clone();
    repeated_a
        .parameters
        .insert("callback-url".into(), processor_effects.url("duplicate-a"));
    deployment
        .tool_middleware_chains
        .get_mut(&baseline)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[0]
        .provision
        .plugins = vec![repeated_a, plugins[2].clone(), plugins[3].clone()];
    deployment
        .tool_bindings
        .get_mut(&baseline)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .parameters =
        NormalizedJsonValue::new(serde_json::json!({"marker": "accepted-before-redeploy"}));
    let type_id = deployment.registered_tools[&tool_name].type_id;
    let agent_owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    let mut hosted_bindings = deployment.tool_bindings[&baseline].clone();
    for binding in hosted_bindings.values_mut() {
        binding.owner = agent_owner.clone();
    }
    deployment
        .tool_bindings
        .insert(agent_owner.clone(), hosted_bindings);
    let mut hosted_chains = deployment.tool_middleware_chains[&baseline].clone();
    for chain in hosted_chains.values_mut() {
        chain.owner = agent_owner.clone();
    }
    deployment
        .tool_middleware_chains
        .insert(agent_owner, hosted_chains);
    environment.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment.clone()),
    );
    let hosted = executor
        .start_agent_with(
            &component.id,
            agent_key!("ToolStreamingCaller", "processor-hosted"),
            HashMap::from([(
                "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                effect_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&hosted).await?.fingerprint;
    let hosted_key = IdempotencyKey::fresh();
    let hosted_call = executor.standalone_tool(
        &component,
        type_id,
        "middleware-probe",
        "apply",
        &hosted_key,
        ToolValueInput {
            value: "lifecycle-effect(hosted)".into(),
        }
        .into_typed_schema_value()?,
    )?;
    let mut hosted_request = hosted_call.request;
    hosted_request.target = Some(crate::agent::id_target(&hosted));
    hosted_request.idempotency_key = Some(hosted_key.into());
    hosted_request.expected_callee_fingerprint = Some(fingerprint.0.into());
    hosted_request.external_tool.as_mut().unwrap().fresh_owner = false;
    let hosted_result = executor.invoke_agent_session(hosted_request).await?;
    assert_eq!(
        tool_result_value(hosted_result)?,
        SchemaValue::String("leaf(lifecycle-effect(hosted))".into())
    );
    assert_eq!(
        tokio::time::timeout(CHECKPOINT_TIMEOUT, effects.recv())
            .await?
            .expect("hosted leaf effect"),
        "lifecycle-effect(hosted)"
    );
    processor_effects.wait_for(&hosted, &["y"]).await?;
    let hosted_entries = executor.stored_oplog(&hosted.agent_id).await;
    let OplogEntry::Create { parameters, .. } = &hosted_entries[0] else {
        panic!("missing hosted Create");
    };
    assert_eq!(
        parameters.initial_active_plugins,
        [hosted_plugin.environment_plugin_grant_id]
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
    );
    assert!(
        processor_effects
            .observed
            .iter()
            .filter(|(_, body)| body["source_worker_id"] == hosted.agent_id.to_string())
            .all(|(label, _)| label == "y")
    );
    let key = IdempotencyKey::fresh();
    let input = ToolValueInput {
        value: "suspend(standalone-73)".into(),
    }
    .into_typed_schema_value()?;
    let call = executor.standalone_tool(
        &component,
        type_id,
        "middleware-probe",
        "apply",
        &key,
        input.clone(),
    )?;
    let owner = call.owned.clone();
    call.start(&executor).await?;
    let original_pin = accepted_activation(&executor, &owner).await?;
    assert_eq!(
        original_pin
            .processor_plugins()
            .cloned()
            .collect::<Vec<_>>(),
        plugins[..4]
    );
    for checkpoint_name in ["lifecycle-before-leaf", "lifecycle-after-leaf"] {
        let promise = wait_for_accepted_middleware_to_suspend(
            &executor,
            &owner,
            &mut promises,
            checkpoint_name,
        )
        .await?;
        drop(executor);
        executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        if checkpoint_name == "lifecycle-before-leaf" {
            // Before leaf, even after reconstruction → no external effect yet.
            assert!(
                effects.try_recv().is_err(),
                "leaf has not run at the before-leaf checkpoint"
            );
            redeploy_parameterized_middleware(
                &mut deployment,
                &baseline,
                &tool_name,
                &middleware_metadata,
            )?;
            let provision = &mut deployment
                .registered_tools
                .get_mut(&tool_name)
                .unwrap()
                .registration
                .provision;
            for plugin in &mut provision.plugins {
                plugin.parameters.insert(
                    "callback-url".into(),
                    processor_effects.url(&format!("new-{}", plugin.plugin_name)),
                );
            }
            deployment
                .tool_middleware_chains
                .get_mut(&baseline)
                .unwrap()
                .get_mut(&tool_name)
                .unwrap()
                .occurrences[0]
                .provision
                .plugins
                .clear();
            environment.set_tool_deployment(
                context.default_environment_id,
                component.id,
                component.revision,
                Some(deployment.clone()),
            );
        } else {
            // After leaf, even after reconstruction → exactly one observed effect.
            assert_eq!(
                effects.try_recv()?.as_str(),
                "lifecycle-effect(suspend(standalone-73))"
            );
        }
        executor
            .complete_promise(&context.default_environment_id, &promise, vec![])
            .await?;
    }
    let completed = call.result(&executor).await?;
    // Accepted middleware survives redeployment → original result/configuration, no repeated leaf.
    assert_eq!(
        tool_result_value(completed)?,
        SchemaValue::String("lifecycle(leaf(lifecycle-effect(suspend(standalone-73))))".into())
    );
    assert!(
        effects.try_recv().is_err(),
        "replay must not repeat the leaf effect"
    );
    let oplog = executor.get_oplog(&owner, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .count(),
        1
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count(),
        1
    );
    assert!(incomplete_starts_before(&oplog, OplogIndex::from_u64(u64::MAX)).is_empty());
    assert_settled_oplog(&oplog, &[], &[]);
    assert_eq!(accepted_activation(&executor, &owner).await?, original_pin);
    processor_effects
        .wait_for(&owner, &["a", "b", "c", "d"])
        .await?;
    assert!(
        processor_effects
            .observed
            .iter()
            .all(|(label, _)| label != "x" && label != "duplicate-a")
    );
    assert_standalone_plugin_mutation_refused(&executor, &owner).await?;
    assert_eq!(
        accepted_activation(&executor, &owner)
            .await?
            .binding
            .parameters
            .0,
        serde_json::json!({"marker": "accepted-before-redeploy"})
    );
    let new_key = IdempotencyKey::new("after-baseline-redeploy".into());
    let fresh_call = executor.standalone_tool(
        &component,
        type_id,
        "middleware-probe",
        "apply",
        &new_key,
        input,
    )?;
    let new_owner = fresh_call.owned.clone();
    let fresh = fresh_call.result(&executor).await?;
    // New key after redeployment → new middleware and parameters, no old lifecycle effect.
    assert_eq!(
        tool_result_value(fresh)?,
        SchemaValue::String("leaf(typed-prefix[first,second](suspend(standalone-73)))".into())
    );
    assert_eq!(
        accepted_activation(&executor, &new_owner)
            .await?
            .binding
            .parameters
            .0,
        serde_json::json!({"marker": "fresh-after-redeploy"})
    );
    assert!(
        effects.try_recv().is_err(),
        "the new configuration does not execute the lifecycle effect"
    );
    assert_eq!(
        accepted_activation(&executor, &new_owner)
            .await?
            .processor_plugins()
            .count(),
        2
    );
    let new_entries = executor.stored_oplog(&new_owner.agent_id).await;
    let OplogEntry::Create { parameters, .. } = &new_entries[0] else {
        panic!("missing new owner Create");
    };
    assert_eq!(
        parameters.initial_active_plugins,
        plugins[..2]
            .iter()
            .map(|plugin| plugin.environment_plugin_grant_id)
            .collect::<std::collections::HashSet<_>>()
    );
    processor_effects
        .wait_for(&new_owner, &["new-a", "new-b"])
        .await?;
    assert!(
        processor_effects
            .observed
            .iter()
            .filter(|(_, body)| body["source_worker_id"] == new_owner.agent_id.to_string())
            .all(|(label, _)| label == "new-a" || label == "new-b")
    );
    assert!(
        processor_effects
            .observed
            .iter()
            .filter(|(_, body)| body["source_worker_id"] == owner.agent_id.to_string())
            .all(|(label, _)| ["a", "b", "c", "d"].contains(&label.as_str()))
    );
    effect_server.abort();
    Ok(())
}

#[test]
#[timeout("3m")]
async fn standalone_concurrent_candidates_use_the_winning_create_for_body_and_processors(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let discovery = Arc::new(ReorderedToolActivationService::default());
    let StandaloneSetup { fixture, executor } = StandaloneFixture::start_with_environment(
        last_unique_id,
        deps,
        provider,
        caller,
        Some(discovery.clone()),
    )
    .await?;
    let mut callbacks = ProcessorEffects::start().await?;
    let processor = executor
        .component(
            &fixture.context.default_environment_id,
            "oplog_processor_release",
        )
        .store()
        .await?;
    let winner_plugin = callbacks.plugin(&fixture.context, &processor, "winner");
    let tool = ToolName::try_from("streaming").unwrap();
    let baseline = ToolBindingOwner::ComponentBaseline {
        component_id: fixture.tools.component.id,
    };
    let mut winner = fixture.tools.deployment.clone();
    winner
        .tool_bindings
        .get_mut(&baseline)
        .unwrap()
        .get_mut(&tool)
        .unwrap()
        .parameters = NormalizedJsonValue::new(serde_json::json!({"marker": "winner"}));
    winner
        .registered_tools
        .get_mut(&tool)
        .unwrap()
        .registration
        .provision
        .plugins = vec![winner_plugin.clone()];
    let loser_server = start_native_order_http_server(None).await;
    let _loser_server = tokio_util::task::AbortOnDropHandle::new(loser_server.server);
    let mut loser = winner.clone();
    loser
        .tool_bindings
        .get_mut(&baseline)
        .unwrap()
        .get_mut(&tool)
        .unwrap()
        .parameters = NormalizedJsonValue::new(serde_json::json!({"marker": "loser"}));
    let provision = &mut loser
        .registered_tools
        .get_mut(&tool)
        .unwrap()
        .registration
        .provision;
    provision.env.insert(
        "NATIVE_ORDER_HTTP_PORT".into(),
        loser_server.port.to_string(),
    );
    provision.plugins[0]
        .parameters
        .insert("callback-url".into(), callbacks.url("loser"));
    *discovery.first_deployment.write().unwrap() = Some(Arc::new(loser));
    discovery.inner.set_tool_deployment(
        fixture.context.default_environment_id,
        fixture.tools.component.id,
        fixture.tools.component.revision,
        Some(winner),
    );
    let call = fixture.call(&executor, "different-candidates-same-key", "native-order")?;
    let first = call.attach(&executor);
    tokio::pin!(first);
    tokio::select! {
        _ = discovery.wait_for_first_call_to_block() => {}
        _ = &mut first => panic!("the first candidate did not wait for the competing Create"),
    }
    let winning_result = call.result(&executor).await?;
    discovery.release_first_call();
    assert_eq!(first.await?.result().await?, winning_result);
    let activation = accepted_activation(&executor, &call.owned).await?;
    assert_eq!(
        activation.binding.parameters.0,
        serde_json::json!({"marker": "winner"})
    );
    assert_eq!(
        activation.processor_plugins().cloned().collect::<Vec<_>>(),
        vec![winner_plugin]
    );
    assert_eq!(fixture.effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        loser_server.effects.load(Ordering::SeqCst),
        0,
        "the losing candidate must not select the body environment"
    );
    callbacks.wait_for(&call.owned, &["winner"]).await?;
    assert!(
        callbacks
            .observed
            .iter()
            .all(|(label, _)| label == "winner")
    );
    assert_eq!(discovery.activation_calls(), 2);
    let entries = executor.stored_oplog(&call.owned.agent_id).await;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::Create { .. }))
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("3m")]
async fn standalone_tool_processor_admitted_batch_survives_unconfirmed_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    admitted_processor_batch_survives_unconfirmed_restart(
        last_unique_id,
        deps,
        provider,
        caller,
        "tool",
    )
    .await
}

#[test]
#[timeout("3m")]
async fn standalone_middleware_processor_admitted_batch_survives_unconfirmed_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    admitted_processor_batch_survives_unconfirmed_restart(
        last_unique_id,
        deps,
        provider,
        caller,
        "middleware",
    )
    .await
}

async fn admitted_processor_batch_survives_unconfirmed_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    held_recipient: &str,
) -> anyhow::Result<()> {
    let StandaloneSetup { fixture, executor } =
        StandaloneFixture::start(last_unique_id, deps, provider, caller).await?;
    let mut effects = ProcessorEffects::start().await?;
    let processor = executor
        .component(
            &fixture.context.default_environment_id,
            "oplog_processor_release",
        )
        .store()
        .await?;
    let middleware = executor
        .component(
            &fixture.context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let definition = metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .expect("universal middleware fixture");
    let tool = ToolName::try_from("streaming").unwrap();
    let agent = ToolBindingOwner::AgentType {
        agent_type_name: AgentTypeName("ToolStreamingCaller".into()),
    };
    let baseline = ToolBindingOwner::ComponentBaseline {
        component_id: fixture.tools.component.id,
    };
    let mut deployment = fixture.tools.deployment.clone();
    install_middleware_chain(
        &mut deployment,
        &AgentTypeName("ToolStreamingCaller".into()),
        &tool,
        middleware.id,
        middleware.revision,
        "golem-it:tool-streaming-rust-middleware",
        &metadata.tool_middlewares,
        vec![(
            definition.name.as_str(),
            empty_middleware_parameters(definition),
        )],
    );
    let mut chains = deployment.tool_middleware_chains.remove(&agent).unwrap();
    chains.get_mut(&tool).unwrap().owner = baseline.clone();
    deployment
        .tool_middleware_chains
        .insert(baseline.clone(), chains);
    let tool_plugin = effects.plugin(&fixture.context, &processor, "tool");
    let middleware_plugin = effects.plugin(&fixture.context, &processor, "middleware");
    deployment
        .registered_tools
        .get_mut(&tool)
        .unwrap()
        .registration
        .provision
        .plugins = vec![tool_plugin.clone()];
    deployment
        .tool_middleware_chains
        .get_mut(&baseline)
        .unwrap()
        .get_mut(&tool)
        .unwrap()
        .occurrences[0]
        .provision
        .plugins = vec![middleware_plugin.clone()];
    fixture.environment.set_tool_deployment(
        fixture.context.default_environment_id,
        fixture.tools.component.id,
        fixture.tools.component.revision,
        Some(deployment.clone()),
    );
    let plugin = if held_recipient == "tool" {
        &tool_plugin
    } else {
        &middleware_plugin
    };

    // Keep the source in a real HTTP call until both processor owners exist. This lets the
    // next batch's admission receipt be gated without predicting a random processor identity.
    fixture.effect_gate.hold_next.store(true, Ordering::SeqCst);
    let call = fixture.call(&executor, "processor-unconfirmed-restart", "native-order")?;
    let session = call.attach(&executor).await?;
    tokio::time::timeout(CHECKPOINT_TIMEOUT, fixture.effect_gate.arrived.notified()).await?;
    let pin = accepted_activation(&executor, &call.owned).await?;
    assert_eq!(
        pin.processor_plugins().cloned().collect::<Vec<_>>(),
        vec![tool_plugin.clone(), middleware_plugin.clone()],
        "the standalone Create pins both independent recipients"
    );
    effects
        .wait_for_any_batch(&call.owned, &["tool", "middleware"])
        .await?;
    let target = tokio::time::timeout(CHECKPOINT_TIMEOUT, async {
        loop {
            let entries = executor.stored_oplog(&call.owned.agent_id).await;
            if let Some(OplogEntry::OplogProcessorCheckpoint {
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                delivery,
                ..
            }) = entries.iter().rev().find(|entry| {
                matches!(entry,
                    OplogEntry::OplogProcessorCheckpoint { plugin_grant_id, .. }
                        if *plugin_grant_id == plugin.environment_plugin_grant_id
                )
            }) && confirmed_up_to == sending_up_to
                && delivery.is_none()
            {
                break OwnedAgentId::new(call.owned.environment_id, target_agent_id);
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let mut admission = executor
        .gate_after_next_append(&target.agent_id, |entry| {
            matches!(entry, OplogEntry::PendingAgentInvocation { operation, .. }
            if **operation == golem_common::model::InvocationRequestOperation::ProcessOplogEntries)
        })
        .await;
    fixture.effect_gate.release.notify_one();
    tokio::time::timeout(CHECKPOINT_TIMEOUT, admission.entered()).await?;

    // The existing append gate commits the target's Pending before withholding its receipt.
    // The real source forwarder is still awaiting enqueue, so it cannot confirm this batch.
    // Compare the stored codec form: oplog timestamps persist at millisecond precision.
    let encoded =
        golem_common::serialization::serialize(&admission.entry()).map_err(anyhow::Error::msg)?;
    let accepted_entry: OplogEntry =
        golem_common::serialization::deserialize(&encoded).map_err(anyhow::Error::msg)?;
    let OplogEntry::PendingAgentInvocation {
        invocation_id,
        operation,
        payload,
        ..
    } = accepted_entry.clone()
    else {
        panic!("processor acceptance")
    };
    let accepted = executor.stored_oplog(&target.agent_id).await;
    assert!(
        accepted.contains(&accepted_entry),
        "target acceptance must be durable before the crash"
    );
    let accepted_payload = executor
        .active_agent(&target)
        .await
        .expect("processor owner")
        .execution()
        .oplog()
        .download_request(*operation, payload)
        .await
        .map_err(anyhow::Error::msg)?;
    let AgentInvocationPayload::ProcessOplogEntries {
        first_entry_index,
        entries,
        ..
    } = &accepted_payload
    else {
        panic!("processor payload")
    };
    let batch_start = *first_entry_index;
    let batch_end = batch_start.range_end(entries.len() as u64);
    let source_before = executor.stored_oplog(&call.owned.agent_id).await;
    let pending_checkpoint = source_before
        .iter()
        .rev()
        .find(|entry| {
            matches!(entry,
                OplogEntry::OplogProcessorCheckpoint { plugin_grant_id, .. }
                    if *plugin_grant_id == plugin.environment_plugin_grant_id
            )
        })
        .expect("source pre-send checkpoint")
        .clone();
    assert!(
        matches!(&pending_checkpoint,
            OplogEntry::OplogProcessorCheckpoint {
                target_agent_id, confirmed_up_to, sending_up_to, last_batch_start, delivery: Some(_), ..
            } if *target_agent_id == target.agent_id && *confirmed_up_to < *sending_up_to
                && *sending_up_to == batch_end && *last_batch_start == batch_start
        ),
        "the source must retain the exact unconfirmed target and delivery snapshot: {pending_checkpoint:?}"
    );
    assert!(
        !accepted.iter().any(|entry| matches!(entry,
            OplogEntry::AgentInvocationFinished { invocation_id: key, .. } if *key == invocation_id
        )),
        "the admitted batch has not executed yet"
    );

    drop(session);
    drop(executor);
    admission.abort();
    drop(admission);
    fixture.environment.set_tool_deployment(
        fixture.context.default_environment_id,
        fixture.tools.component.id,
        fixture.tools.component.revision,
        None,
    );
    let executor = start_with_overrides(deps, &fixture.context, fixture.overrides.clone()).await?;
    call.result(&executor).await?;
    assert_eq!(accepted_activation(&executor, &call.owned).await?, pin);
    effects
        .wait_for(&call.owned, &["tool", "middleware"])
        .await?;
    tokio::time::timeout(CHECKPOINT_TIMEOUT, async {
        loop {
            if executor
                .stored_oplog(&call.owned.agent_id)
                .await
                .iter()
                .any(|entry| {
                    matches!(entry,
                        OplogEntry::OplogProcessorCheckpoint {
                            plugin_grant_id, target_agent_id, confirmed_up_to, sending_up_to,
                            last_batch_start, delivery: None, ..
                        } if *plugin_grant_id == plugin.environment_plugin_grant_id
                            && *target_agent_id == target.agent_id && *confirmed_up_to == batch_end
                            && *sending_up_to == batch_end && *last_batch_start == batch_start
                    )
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;

    // Recovery can confirm only after the real target accepts the resend. Its ordinary
    // request-equality check compares the entire payload, not just the idempotency key.
    // Also make an explicit same-target/key/full-payload resend through the production
    // Worker admission API used by local processor delivery, rather than inferring it from
    // equal results. The callback ranges and execution records below count its effects.
    let repeated = executor
        .active_agent(&target)
        .await
        .expect("recovered processor")
        .primary()
        .invoke_and_await(AgentInvocation::from_parts(
            invocation_id,
            accepted_payload.clone(),
            InvocationContextStack::fresh(),
        ))
        .await?;
    assert!(matches!(
        repeated.result,
        AgentInvocationResult::ProcessOplogEntries { error: None }
    ));
    let recovered = executor.stored_oplog(&target.agent_id).await;
    assert_eq!(recovered.iter().filter(|entry| matches!(entry,
        OplogEntry::PendingAgentInvocation { invocation_id: key, .. } if *key == invocation_id
    )).count(), 1, "the resend must reuse the target's admitted key");
    assert!(
        recovered.contains(&accepted_entry),
        "the original target/key/payload must remain unchanged"
    );
    for is_finished in [false, true] {
        assert_eq!(
            recovered
                .iter()
                .filter(|entry| match entry {
                    OplogEntry::AgentInvocationStarted {
                        invocation_id: key, ..
                    } if !is_finished => *key == invocation_id,
                    OplogEntry::AgentInvocationFinished {
                        invocation_id: key, ..
                    } if is_finished => *key == invocation_id,
                    _ => false,
                })
                .count(),
            1,
            "one real processor execution and completion per admitted key"
        );
    }
    let OplogEntry::PendingAgentInvocation {
        operation, payload, ..
    } = accepted_entry
    else {
        unreachable!()
    };
    let recovered_payload = executor
        .active_agent(&target)
        .await
        .expect("recovered processor")
        .execution()
        .oplog()
        .download_request(*operation, payload)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(recovered_payload, accepted_payload);
    assert_processor_ranges_once(&effects, &call.owned);
    assert_eq!(
        effects
            .observed
            .iter()
            .filter(|(label, body)| {
                label == held_recipient
                    && body["source_worker_id"] == call.owned.agent_id.to_string()
                    && body["first_entry_index"] == batch_start.as_u64()
                    && body["entry_count"] == entries.len() as u64
            })
            .count(),
        1,
        "the real processor posts exactly one callback for the resent range"
    );

    // A fresh owner still makes progress through both independent pinned recipients.
    fixture.environment.set_tool_deployment(
        fixture.context.default_environment_id,
        fixture.tools.component.id,
        fixture.tools.component.revision,
        Some(deployment),
    );
    let fresh = fixture.call(
        &executor,
        "processor-after-unconfirmed-restart",
        "native-order",
    )?;
    fresh.result(&executor).await?;
    effects
        .wait_for(&fresh.owned, &["tool", "middleware"])
        .await?;
    assert_processor_ranges_once(&effects, &call.owned);
    assert_processor_ranges_once(&effects, &fresh.owned);
    assert_settled_oplog(
        &executor.get_oplog(&call.owned, OplogIndex::INITIAL).await?,
        &[],
        &[],
    );
    assert_eq!(fixture.effects.load(Ordering::SeqCst), 2);
    Ok(())
}

fn assert_processor_ranges_once(effects: &ProcessorEffects, source: &OwnedAgentId) {
    for label in ["tool", "middleware"] {
        let mut ranges: Vec<_> = effects
            .observed
            .iter()
            .filter(|(seen, body)| {
                seen == label && body["source_worker_id"] == source.agent_id.to_string()
            })
            .map(|(_, body)| {
                let start = body["first_entry_index"].as_u64().unwrap();
                let count = body["entry_count"].as_u64().unwrap();
                assert!(count > 0);
                (start, start + count - 1)
            })
            .collect();
        assert!(
            !ranges.is_empty(),
            "{label} must execute the real processor"
        );
        ranges.sort_unstable();
        for pair in ranges.windows(2) {
            assert!(
                pair[0].1 < pair[1].0,
                "{label} duplicated a processor effect range: {ranges:?}"
            );
        }
    }
}

async fn assert_standalone_plugin_mutation_refused(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        ActivatePluginRequest, DeactivatePluginRequest, activate_plugin_response,
        deactivate_plugin_response,
    };
    let before = executor.get_worker_metadata(owner).await?;
    let activated = executor
        .client
        .clone()
        .activate_plugin(ActivatePluginRequest {
            agent_id: Some(owner.agent_id.into()),
            environment_id: Some(owner.environment_id.into()),
            plugin_priority: 0,
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            expected_fingerprint: Some(before.fingerprint.0.into()),
        })
        .await?
        .into_inner();
    let Some(activate_plugin_response::Result::Failure(error)) = activated.result else {
        anyhow::bail!("standalone activate unexpectedly succeeded");
    };
    assert!(matches!(
        WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?,
        WorkerExecutorError::NotAnAgent { .. }
    ));
    let deactivated = executor
        .client
        .clone()
        .deactivate_plugin(DeactivatePluginRequest {
            agent_id: Some(owner.agent_id.into()),
            environment_id: Some(owner.environment_id.into()),
            plugin_priority: 0,
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            expected_fingerprint: Some(before.fingerprint.0.into()),
        })
        .await?
        .into_inner();
    let Some(deactivate_plugin_response::Result::Failure(error)) = deactivated.result else {
        anyhow::bail!("standalone deactivate unexpectedly succeeded");
    };
    assert!(matches!(
        WorkerExecutorError::try_from(error).map_err(anyhow::Error::msg)?,
        WorkerExecutorError::NotAnAgent { .. }
    ));
    assert_eq!(
        executor.get_worker_metadata(owner).await?.active_plugins,
        before.active_plugins
    );
    assert!(
        executor
            .stored_oplog(&owner.agent_id)
            .await
            .iter()
            .all(|entry| !matches!(
                entry,
                OplogEntry::ActivatePlugin { .. } | OplogEntry::DeactivatePlugin { .. }
            ))
    );
    Ok(())
}

pub(super) async fn accepted_activation(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
) -> anyhow::Result<ToolActivationSnapshot> {
    let oplog = executor
        .active_agent(owner)
        .await
        .expect("active owner")
        .execution()
        .oplog();
    let entries = executor.stored_oplog(&owner.agent_id).await;
    let OplogEntry::Create { parameters, .. } = &entries[0] else {
        anyhow::bail!("missing Create");
    };
    let pin = &parameters
        .standalone_tool
        .as_ref()
        .expect("standalone owner")
        .activation;
    let mut accepted = false;
    for entry in entries.iter().cloned() {
        if let OplogEntry::PendingAgentInvocation {
            operation, payload, ..
        } = entry
        {
            let AgentInvocationPayload::ExternalTool { activation, .. } = oplog
                .download_request(operation.as_ref().clone(), payload)
                .await
                .map_err(anyhow::Error::msg)?
            else {
                anyhow::bail!("expected external-tool acceptance");
            };
            assert_eq!(
                activation.as_ref(),
                pin,
                "every Pending uses the winning Create pin"
            );
            accepted = true;
        }
    }
    anyhow::ensure!(accepted, "missing accepted invocation");
    Ok(pin.clone())
}

struct ProcessorEffects {
    address: std::net::SocketAddr,
    callbacks: tokio::sync::mpsc::UnboundedReceiver<(String, serde_json::Value)>,
    observed: Vec<(String, serde_json::Value)>,
    _server: tokio_util::task::AbortOnDropHandle<()>,
}

impl ProcessorEffects {
    async fn start() -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (send, callbacks) = tokio::sync::mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/{label}", post(
                move |Path(label): Path<String>, axum::Json(body): axum::Json<serde_json::Value>| {
                    let send = send.clone();
                    async move { send.send((label, body)).unwrap(); }
                }
            ))).await.unwrap();
        });
        Ok(Self {
            address,
            callbacks,
            observed: Vec::new(),
            _server: tokio_util::task::AbortOnDropHandle::new(server),
        })
    }

    fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.address)
    }

    fn plugin(
        &self,
        context: &TestContext,
        processor: &ComponentDto,
        name: &str,
    ) -> InstalledPlugin {
        InstalledPlugin {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
            priority: PluginPriority(0),
            parameters: BTreeMap::from([("callback-url".into(), self.url(name))]),
            plugin_registration_id: PluginRegistrationId::new(),
            plugin_name: name.into(),
            plugin_version: "1".into(),
            oplog_processor_component_id: Some(processor.id),
            oplog_processor_component_revision: Some(processor.revision),
            installed_by: context.account_id,
        }
    }

    async fn wait_for(&mut self, owner: &OwnedAgentId, labels: &[&str]) -> anyhow::Result<()> {
        let source = owner.agent_id.to_string();
        tokio::time::timeout(CHECKPOINT_TIMEOUT, async {
            while !labels.iter().all(|label| {
                self.observed.iter().any(|(seen, body)| {
                    seen == label
                        && body["source_worker_id"] == source
                        && body["invocations"].as_array().is_some_and(|invocations| {
                            invocations
                                .iter()
                                .any(|invocation| invocation["fn_name"] == "external-tool")
                        })
                })
            }) {
                self.observed
                    .push(self.callbacks.recv().await.expect("processor callbacks"));
            }
        })
        .await?;
        while let Ok(callback) = self.callbacks.try_recv() {
            self.observed.push(callback);
        }
        Ok(())
    }

    async fn wait_for_any_batch(
        &mut self,
        owner: &OwnedAgentId,
        labels: &[&str],
    ) -> anyhow::Result<()> {
        tokio::time::timeout(CHECKPOINT_TIMEOUT, async {
            while !labels.iter().all(|label| {
                self.observed.iter().any(|(seen, body)| {
                    seen == label && body["source_worker_id"] == owner.agent_id.to_string()
                })
            }) {
                self.observed
                    .push(self.callbacks.recv().await.expect("processor callbacks"));
            }
        })
        .await?;
        Ok(())
    }
}

fn redeploy_parameterized_middleware(
    deployment: &mut ToolDeploymentState,
    baseline: &ToolBindingOwner,
    tool_name: &ToolName,
    metadata: &ExtractedComponentMetadata,
) -> anyhow::Result<()> {
    deployment
        .tool_bindings
        .get_mut(baseline)
        .unwrap()
        .get_mut(tool_name)
        .unwrap()
        .parameters =
        NormalizedJsonValue::new(serde_json::json!({"marker": "fresh-after-redeploy"}));
    let parameterized = metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-parameterized")
        .unwrap();
    let occurrence = &mut deployment
        .tool_middleware_chains
        .get_mut(baseline)
        .unwrap()
        .get_mut(tool_name)
        .unwrap()
        .occurrences[0];
    occurrence.middleware = deployment.registered_tool_middlewares
        [&ToolMiddlewareName::try_from("streaming-parameterized").unwrap()]
        .clone();
    occurrence.parameters = parameterized_middleware_parameters(parameterized);
    deployment.deployment_revision = deployment.deployment_revision.next()?;
    let revision = deployment.deployment_revision;
    for registered in deployment.registered_tools.values_mut() {
        registered.registration.deployment_revision = revision;
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.deployment_revision = revision;
        }
    }
    for middleware in deployment.registered_tool_middlewares.values_mut() {
        middleware.deployment_revision = revision;
    }
    for chains in deployment.tool_middleware_chains.values_mut() {
        for chain in chains.values_mut() {
            chain.deployment_revision = revision;
            for occurrence in &mut chain.occurrences {
                occurrence.middleware.deployment_revision = revision;
            }
        }
    }
    Ok(())
}

async fn wait_for_accepted_middleware_to_suspend(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
    promises: &mut tokio::sync::mpsc::UnboundedReceiver<PromiseCheckpointArrival>,
    checkpoint_name: &str,
) -> anyhow::Result<PromiseId> {
    let checkpoint = next_promise_checkpoint(promises, checkpoint_name).await?;
    executor
        .wait_for_status(owner, AgentStatus::Suspended, CHECKPOINT_TIMEOUT)
        .await?;
    let fingerprint = executor.get_worker_metadata(owner).await?.fingerprint;
    // Before each restart → acceptance pins the original baseline parameters.
    assert_eq!(
        accepted_activation(executor, owner)
            .await?
            .binding
            .parameters
            .0,
        serde_json::json!({"marker": "accepted-before-redeploy"})
    );
    Ok(PromiseId {
        agent_id: owner.agent_id,
        fingerprint,
        oplog_idx: checkpoint.oplog_idx,
    })
}

fn tool_result_value(result: InvocationSessionResult) -> anyhow::Result<SchemaValue> {
    let Some(invocation_session_result::Result::ToolResult(result)) = result.result else {
        anyhow::bail!("expected a tool result");
    };
    let result: PublicExternalToolResult = result.try_into().map_err(anyhow::Error::msg)?;
    let PublicExternalToolResult::Success(result) = result else {
        anyhow::bail!("expected a successful tool result: {result:?}");
    };
    Ok(result.result.expect("tool value").value().clone())
}
