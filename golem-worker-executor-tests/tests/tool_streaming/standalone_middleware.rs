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

use super::*;
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

#[test]
#[timeout("3m")]
async fn standalone_middleware_waits_survive_restarts_before_and_after_leaf(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::{
        AgentInvocationMode, ExternalToolInvocation, InvocationStart,
    };
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let (effect_port, mut effects, effect_server) =
        middleware_acceptance::start_probe_effect_server().await;
    let (promise_port, _promises, mut promises) = start_promise_checkpoint_server().await;
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        wrap_component_service: Some(Arc::new(move |inner| {
            Arc::new(BaselineOwnerComponentService {
                inner,
                owner_loads: Default::default(),
                http_port: 0,
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
    let old_owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type,
    };
    let baseline = ToolBindingOwner::ComponentBaseline {
        component_id: component.id,
    };
    let mut bindings = deployment.tool_bindings.remove(&old_owner).unwrap();
    for binding in bindings.values_mut() {
        binding.owner = baseline.clone();
    }
    bindings.get_mut(&tool_name).unwrap().parameters =
        NormalizedJsonValue::new(serde_json::json!({"marker": "accepted-before-redeploy"}));
    deployment.tool_bindings.insert(baseline.clone(), bindings);
    let mut chains = deployment
        .tool_middleware_chains
        .remove(&old_owner)
        .unwrap();
    for chain in chains.values_mut() {
        chain.owner = baseline.clone();
    }
    deployment
        .tool_middleware_chains
        .insert(baseline.clone(), chains);
    let type_id = deployment.registered_tools[&tool_name].type_id;
    environment.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment.clone()),
    );
    let key = IdempotencyKey::fresh();
    let invocation_id = golem_common::model::StandaloneToolInvocation::unique_part(
        context.default_environment_id,
        &tool_name,
        executor.auth_ctx().idempotency_scope(),
        "apply",
        Some(&key),
    );
    let owner = OwnedAgentId::new(
        context.default_environment_id,
        &golem_common::model::AgentId::standalone_tool_invocation(
            context.default_environment_id,
            type_id,
            invocation_id,
        ),
    );
    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "value".into(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::String("suspend(standalone-73)".into())],
        },
    );
    let mut start = InvocationStart {
        target: Some(crate::agent::id_target(&owner)),
        component_id: Some(component.id.into()),
        idempotency_key: Some(IdempotencyKey::new(invocation_id.to_string()).into()),
        auth_ctx: Some(executor.auth_ctx().into()),
        caller: Some(callers::account(context.account_id).into()),
        environment_id: Some(context.default_environment_id.into()),
        component_owner_account_id: Some(context.account_id.into()),
        mode: AgentInvocationMode::Schedule as i32,
        external_tool: Some(ExternalToolInvocation {
            tool_name: tool_name.to_string(),
            command_path: vec!["apply".into()],
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            fresh_owner: true,
            ..Default::default()
        }),
        request_key: Some(
            golem_common::model::InvocationRequestKey::external(
                executor.auth_ctx().idempotency_scope(),
                Some(&key),
            )
            .into(),
        ),
        ..Default::default()
    };
    let accepted_activation =
        async |executor: &TestWorkerExecutor,
               owner: &OwnedAgentId|
               -> anyhow::Result<golem_common::model::tool::ToolActivationSnapshot> {
            let oplog = executor
                .active_agent(owner)
                .await
                .expect("active owner")
                .execution()
                .oplog();
            for entry in executor.stored_oplog(&owner.agent_id).await {
                if let golem_common::model::oplog::OplogEntry::PendingAgentInvocation {
                    operation,
                    payload,
                    ..
                } = entry
                {
                    let golem_common::model::agent::AgentInvocationPayload::ExternalTool {
                        activation,
                        ..
                    } = oplog
                        .download_request(operation.as_ref().clone(), payload)
                        .await
                        .map_err(anyhow::Error::msg)?
                    else {
                        anyhow::bail!("expected external-tool acceptance");
                    };
                    return Ok(*activation);
                }
            }
            anyhow::bail!("missing accepted invocation");
        };
    executor.invoke_agent_session(start.clone()).await?;
    for checkpoint_name in ["lifecycle-before-leaf", "lifecycle-after-leaf"] {
        let checkpoint = next_promise_checkpoint(&mut promises, checkpoint_name).await?;
        executor
            .wait_for_status(&owner, AgentStatus::Suspended, Duration::from_secs(30))
            .await?;
        let fingerprint = executor.get_worker_metadata(&owner).await?.fingerprint;
        assert_eq!(
            accepted_activation(&executor, &owner)
                .await?
                .binding
                .parameters
                .0,
            serde_json::json!({"marker": "accepted-before-redeploy"})
        );
        drop(executor);
        executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        if checkpoint_name == "lifecycle-before-leaf" {
            assert!(effects.try_recv().is_err());
            deployment
                .tool_bindings
                .get_mut(&baseline)
                .unwrap()
                .get_mut(&tool_name)
                .unwrap()
                .parameters =
                NormalizedJsonValue::new(serde_json::json!({"marker": "fresh-after-redeploy"}));
            let parameterized = middleware_metadata
                .tool_middlewares
                .iter()
                .find(|definition| definition.name == "streaming-parameterized")
                .unwrap();
            let occurrence = &mut deployment
                .tool_middleware_chains
                .get_mut(&baseline)
                .unwrap()
                .get_mut(&tool_name)
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
            environment.set_tool_deployment(
                context.default_environment_id,
                component.id,
                component.revision,
                Some(deployment.clone()),
            );
        } else {
            assert_eq!(
                effects.try_recv()?.as_str(),
                "lifecycle-effect(suspend(standalone-73))"
            );
        }
        executor
            .complete_promise(
                &context.default_environment_id,
                &PromiseId {
                    agent_id: owner.agent_id,
                    fingerprint,
                    oplog_idx: checkpoint.oplog_idx,
                },
                vec![],
            )
            .await?;
    }
    start.mode = AgentInvocationMode::Await as i32;
    let completed = executor.invoke_agent_session(start.clone()).await?;
    let value = |result: golem_api_grpc::proto::golem::worker::InvocationSessionResult| -> anyhow::Result<SchemaValue> {
        let Some(golem_api_grpc::proto::golem::worker::invocation_session_result::Result::ToolResult(result)) = result.result else {
            anyhow::bail!("expected a tool result");
        };
        let result: golem_common::model::oplog::PublicExternalToolResult = result.try_into().map_err(anyhow::Error::msg)?;
        let golem_common::model::oplog::PublicExternalToolResult::Success(result) = result else {
            anyhow::bail!("expected a successful tool result: {result:?}");
        };
        Ok(result.result.expect("tool value").value().clone())
    };
    assert_eq!(
        value(completed)?,
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
    assert_eq!(
        accepted_activation(&executor, &owner)
            .await?
            .binding
            .parameters
            .0,
        serde_json::json!({"marker": "accepted-before-redeploy"})
    );
    let new_key = IdempotencyKey::new("after-baseline-redeploy".into());
    let new_id = golem_common::model::StandaloneToolInvocation::unique_part(
        context.default_environment_id,
        &tool_name,
        executor.auth_ctx().idempotency_scope(),
        "apply",
        Some(&new_key),
    );
    let new_owner = OwnedAgentId::new(
        context.default_environment_id,
        &golem_common::model::AgentId::standalone_tool_invocation(
            context.default_environment_id,
            type_id,
            new_id,
        ),
    );
    start.target = Some(crate::agent::id_target(&new_owner));
    start.idempotency_key = Some(IdempotencyKey::new(new_id.to_string()).into());
    start.request_key = Some(
        golem_common::model::InvocationRequestKey::external(
            executor.auth_ctx().idempotency_scope(),
            Some(&new_key),
        )
        .into(),
    );
    let fresh = executor.invoke_agent_session(start).await?;
    assert_eq!(
        value(fresh)?,
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
