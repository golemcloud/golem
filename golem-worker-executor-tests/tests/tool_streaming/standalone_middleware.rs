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
use golem_common::model::agent::AgentInvocationPayload;
use golem_common::model::agent::extraction::ExtractedComponentMetadata;
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
    effects: tokio::sync::mpsc::UnboundedReceiver<String>,
    promises: PromiseCheckpointServer,
    effect_server: tokio::task::JoinHandle<()>,
}

impl MiddlewareServers {
    async fn start() -> anyhow::Result<Self> {
        let (effect_port, effects, effect_server) =
            middleware_acceptance::start_probe_effect_server().await;
        let promises = PromiseCheckpointServer::start().await?;
        Ok(Self {
            effect_port,
            effects,
            promises,
            effect_server,
        })
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
    let MiddlewareServers {
        effect_port,
        mut effects,
        mut promises,
        effect_server,
    } = MiddlewareServers::start().await?;
    let extra_env: BTreeMap<String, String> = promises
        .agent_env("MIDDLEWARE")
        .into_iter()
        .chain([(
            "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
            effect_port.to_string(),
        )])
        .collect();
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        wrap_component_service: Some(Arc::new(move |inner| {
            Arc::new(BaselineOwnerComponentService {
                inner,
                owner_loads: Default::default(),
                http_port: 0,
                extra_env: extra_env.clone(),
            })
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
        .tool_bindings
        .get_mut(&baseline)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .parameters =
        NormalizedJsonValue::new(serde_json::json!({"marker": "accepted-before-redeploy"}));
    let type_id = deployment.registered_tools[&tool_name].type_id;
    environment.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment.clone()),
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
    OplogShape::of(&oplog).assert_settled();
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
    effect_server.abort();
    Ok(())
}

async fn accepted_activation(
    executor: &TestWorkerExecutor,
    owner: &OwnedAgentId,
) -> anyhow::Result<ToolActivationSnapshot> {
    let oplog = executor
        .active_agent(owner)
        .await
        .expect("active owner")
        .execution()
        .oplog();
    for entry in executor.stored_oplog(&owner.agent_id).await {
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
            return Ok(*activation);
        }
    }
    anyhow::bail!("missing accepted invocation")
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
    promises: &mut PromiseCheckpointServer,
    checkpoint_name: &str,
) -> anyhow::Result<PromiseId> {
    let checkpoint = promises.reach(checkpoint_name).await?;
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
        oplog_idx: checkpoint.oplog_index,
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
