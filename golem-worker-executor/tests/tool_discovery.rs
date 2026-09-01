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

use crate::Tracing;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolDeploymentState, ToolName,
    ToolProvisionConfig, ToolSource,
};
use golem_common::schema::SchemaGraph;
use golem_common::schema::tool::{
    CommandBody, CommandNode, CommandTree, Doc, Globals, Positionals, Tool,
};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::Arc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

type ToolSummary = (String, String, String, Vec<String>, u64, u64);

fn registered_tool(
    name: &str,
    component_id: ComponentId,
    component_revision: ComponentRevision,
    deployment_revision: DeploymentRevision,
) -> RegisteredTool {
    RegisteredTool {
        deployment_revision,
        definition: Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: vec![format!("{name}-alias")],
                    doc: Doc {
                        summary: format!("Summary for {name}"),
                        ..Default::default()
                    },
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: Some(CommandBody {
                        positionals: Positionals::default(),
                        options: Vec::new(),
                        flags: Vec::new(),
                        constraints: Vec::new(),
                        stdin: None,
                        stdout: None,
                        result: None,
                        errors: Vec::new(),
                        annotations: None,
                    }),
                }],
            },
            schema: SchemaGraph::empty(),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolSource::Component {
            component_id,
            component_revision,
            component_name: ComponentName("tool-component".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("test@golem"),
        metadata_version: "0.1.0".to_string(),
    }
}

fn binding(
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    tool: &RegisteredTool,
) -> CompiledToolBinding {
    CompiledToolBinding {
        deployment_revision: tool.deployment_revision,
        agent_type_name: agent_type.clone(),
        tool_name: tool_name.clone(),
        version: tool.definition.version.clone(),
        metadata_version: tool.metadata_version.clone(),
        account_id: tool.owner_account_id,
        account_email: tool.owner_account_email.clone(),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: golem_common::model::tool::ToolFilesystemAccess::Unset,
        source: tool.source.clone(),
    }
}

pub(crate) fn deployment_state(
    agent_type: &AgentTypeName,
    deployment_revision: u64,
    component_revision: ComponentRevision,
    tools: &[(&str, ComponentId, bool)],
) -> ToolDeploymentState {
    let deployment_revision = DeploymentRevision::try_from(deployment_revision).unwrap();
    let registered_tools = tools
        .iter()
        .map(|(name, component_id, _)| {
            let name = ToolName::try_from(*name).unwrap();
            let tool = registered_tool(
                name.as_str(),
                *component_id,
                component_revision,
                deployment_revision,
            );
            (name, tool)
        })
        .collect::<BTreeMap<_, _>>();
    let bindings = tools
        .iter()
        .filter(|(_, _, is_bound)| *is_bound)
        .map(|(name, _, _)| {
            let name = ToolName::try_from(*name).unwrap();
            let tool = registered_tools.get(&name).unwrap();
            (name.clone(), binding(agent_type, &name, tool))
        })
        .collect();

    ToolDeploymentState {
        deployment_revision,
        registered_tools,
        agent_tool_bindings: BTreeMap::from([(agent_type.clone(), bindings)]),
    }
}

fn set_agent_bindings(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_names: &[&str],
) {
    let bindings = tool_names
        .iter()
        .map(|name| {
            let name = ToolName::try_from(*name).unwrap();
            let tool = deployment.registered_tools.get(&name).unwrap();
            (name.clone(), binding(agent_type, &name, tool))
        })
        .collect();
    deployment
        .agent_tool_bindings
        .insert(agent_type.clone(), bindings);
}

fn summary(name: &str, component_id: ComponentId) -> ToolSummary {
    let (high_bits, low_bits) = component_id.0.as_u64_pair();
    (
        name.to_string(),
        "1.0.0".to_string(),
        format!("Summary for {name}"),
        vec![format!("{name}-alias")],
        high_bits,
        low_bits,
    )
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn tool_discovery_host_filters_and_uses_caller_deployment_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let other_agent_type = AgentTypeName("ToolDiscoveryOther".to_string());
    let alpha_component = ComponentId::new();
    let beta_component = ComponentId::new();
    let unbound_component = ComponentId::new();
    let mut initial_deployment = deployment_state(
        &agent_type,
        1,
        ComponentRevision::try_from(1_u64).unwrap(),
        &[
            ("beta", beta_component, true),
            ("unbound", unbound_component, false),
            ("alpha", alpha_component, true),
        ],
    );
    set_agent_bindings(&mut initial_deployment, &other_agent_type, &["beta"]);
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(initial_deployment),
    );
    let other_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .unique()
        .store()
        .await?;
    assert_eq!(other_component.revision, component.revision);
    let other_component_tool = ComponentId::new();
    service.set_tool_deployment(
        context.default_environment_id,
        other_component.id,
        other_component.revision,
        Some(deployment_state(
            &agent_type,
            1,
            ComponentRevision::try_from(1_u64).unwrap(),
            &[("other-component", other_component_tool, true)],
        )),
    );
    let agent_id = agent_id!("GolemHostApi", "tool-discovery-live");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let other_component_agent_id = agent_id!("GolemHostApi", "tool-discovery-other-component");
    executor
        .start_agent(&other_component.id, other_component_agent_id.clone())
        .await?;

    let all = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(
        all,
        vec![
            summary("alpha", alpha_component),
            summary("beta", beta_component)
        ]
    );
    let other_component_tools = executor
        .invoke_and_await_agent(
            &other_component,
            &other_component_agent_id,
            "get_all_tools",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(
        other_component_tools,
        vec![summary("other-component", other_component_tool)]
    );

    let other_agent_id = agent_id!("ToolDiscoveryOther", "tool-discovery-other");
    executor
        .start_agent(&component.id, other_agent_id.clone())
        .await?;
    let other_tools = executor
        .invoke_and_await_agent(&component, &other_agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(other_tools, vec![summary("beta", beta_component)]);

    let other_environment_id = EnvironmentId::new();
    let other_environment_component = executor
        .component_dep(&other_environment_id, host_api_tests)
        .unique()
        .store()
        .await?;
    let other_environment_tool_component = ComponentId::new();
    service.set_tool_deployment(
        other_environment_id,
        other_environment_component.id,
        other_environment_component.revision,
        Some(deployment_state(
            &agent_type,
            1,
            ComponentRevision::try_from(1_u64).unwrap(),
            &[("other-environment", other_environment_tool_component, true)],
        )),
    );
    let other_environment_agent_id = agent_id!("GolemHostApi", "tool-discovery-other-environment");
    executor
        .start_agent(
            &other_environment_component.id,
            other_environment_agent_id.clone(),
        )
        .await?;
    let other_environment_tools = executor
        .invoke_and_await_agent(
            &other_environment_component,
            &other_environment_agent_id,
            "get_all_tools",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(
        other_environment_tools,
        vec![summary(
            "other-environment",
            other_environment_tool_component
        )]
    );

    let alpha = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_tool",
            data_value!("alpha".to_string()),
        )
        .await?
        .into_typed::<Option<ToolSummary>>()?;
    assert_eq!(alpha, Some(summary("alpha", alpha_component)));

    for name in ["missing", "unbound"] {
        let tool = executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "get_tool",
                data_value!(name.to_string()),
            )
            .await?
            .into_typed::<Option<ToolSummary>>()?;
        assert_eq!(tool, None);
    }

    let calls_before_invalid_name = service.accessible_tool_calls();
    let invalid = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_tool",
            data_value!("Not-A-Tool".to_string()),
        )
        .await?
        .into_typed::<Option<ToolSummary>>()?;
    assert_eq!(invalid, None);
    assert_eq!(service.accessible_tool_calls(), calls_before_invalid_name);

    let gamma_component = ComponentId::new();
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &agent_type,
            2,
            ComponentRevision::try_from(1_u64).unwrap(),
            &[("gamma", gamma_component, true)],
        )),
    );
    let after_update = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(after_update, vec![summary("gamma", gamma_component)]);

    let updated_component = executor
        .update_component(&component.id, &host_api_tests.wasm_name)
        .await?;
    let epsilon_component = ComponentId::new();
    service.set_tool_deployment(
        context.default_environment_id,
        updated_component.id,
        updated_component.revision,
        Some(deployment_state(
            &agent_type,
            3,
            ComponentRevision::try_from(1_u64).unwrap(),
            &[("epsilon", epsilon_component, true)],
        )),
    );
    let updated_agent_id = agent_id!("GolemHostApi", "tool-discovery-updated-component");
    executor
        .start_agent(&updated_component.id, updated_agent_id.clone())
        .await?;
    let updated_tools = executor
        .invoke_and_await_agent(
            &updated_component,
            &updated_agent_id,
            "get_all_tools",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(updated_tools, vec![summary("epsilon", epsilon_component)]);

    let original_revision_tools = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(
        original_revision_tools,
        vec![summary("gamma", gamma_component)]
    );

    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        None,
    );
    let without_deployment = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(without_deployment, Vec::new());
    assert_eq!(service.accessible_tools_calls(), 8);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn tool_discovery_replay_uses_persisted_result_without_environment_lookup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let alpha_component = ComponentId::new();
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &agent_type,
            1,
            ComponentRevision::try_from(1_u64).unwrap(),
            &[("alpha", alpha_component, true)],
        )),
    );
    let agent_id = agent_id!("GolemHostApi", "tool-discovery-replay");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "record_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(recorded, vec![summary("alpha", alpha_component)]);
    assert_eq!(service.accessible_tools_calls(), 1);
    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    let beta_component = ComponentId::new();
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &agent_type,
            2,
            ComponentRevision::try_from(1_u64).unwrap(),
            &[("beta", beta_component, true)],
        )),
    );
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let replayed = executor
        .invoke_and_await_agent(&component, &agent_id, "get_recorded_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(replayed, vec![summary("alpha", alpha_component)]);
    assert_eq!(
        service.accessible_tools_calls(),
        1,
        "replay must not consult the current environment state"
    );

    let live_after_replay = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(live_after_replay, vec![summary("beta", beta_component)]);
    assert_eq!(service.accessible_tools_calls(), 2);

    Ok(())
}
