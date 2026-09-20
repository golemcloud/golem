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
use golem_common::model::mcp_import::{McpImport, McpImportSource};
use golem_common::model::oplog::payload::types::SerializableToolDiscoverySnapshot;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::tool::{
    CompiledToolBinding, HostToolId, RegisteredTool, SecretKeyScope, ToolBindingOwner,
    ToolDeploymentState, ToolName, ToolProvisionConfig, ToolSource,
};
use golem_common::model::tool_middleware::{
    CompiledToolMiddlewareChain, CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware,
    ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::model::{AgentStatus, IdempotencyKey};
use golem_common::schema::tool::{
    CommandBody, CommandNode, CommandTree, Doc, Globals, Positionals, Tool, ToolMiddleware,
    ToolMiddlewareScope,
};
use golem_common::schema::{FromSchema, SchemaGraph};
use golem_common::{agent_id, data_value};
use golem_mcp_import::tool::{Diagnostic, Limits, ProjectedTool};
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::mcp_import::McpImportObservation;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);

type ToolSummary = (String, String, String, Vec<String>, u64, u64);

#[derive(FromSchema)]
struct RecordedToolDiscoveryResponse {
    result: Result<SerializableToolDiscoverySnapshot, String>,
}

async fn assert_recorded_discovery_snapshot(
    executor: &golem_worker_executor_test_utils::TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    expected_revision: Option<u64>,
) -> anyhow::Result<SerializableToolDiscoverySnapshot> {
    executor.commit_oplog(worker_id).await?;
    let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
    let starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::tool::host::get_all_tools" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1, "expected one discovery Start: {oplog:?}");
    let response = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == starts[0] => {
                Some(RecordedToolDiscoveryResponse::from_value(
                    params
                        .response
                        .as_ref()
                        .expect("discovery End response")
                        .value(),
                ))
            }
            _ => None,
        })
        .expect("discovery Start must have a matching End")?;
    let snapshot = response.result.map_err(anyhow::Error::msg)?;
    assert_eq!(snapshot.deployment_revision, expected_revision);
    Ok(snapshot)
}

fn projected_tool(name: &str) -> ProjectedTool {
    ProjectedTool::new(
        &serde_json::json!({
            "name": name,
            "description": format!("Summary for {name}"),
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        name,
        Limits::default(),
    )
    .unwrap()
}

fn mcp_source(environment_id: EnvironmentId, revision: u64, import_index: u32) -> McpImportSource {
    McpImportSource {
        environment_id,
        deployment_revision: revision.try_into().unwrap(),
        import_index,
        upstream_tool_name: String::new(),
    }
}

fn mcp_observation(source: McpImportSource, names: &[&str]) -> McpImportObservation {
    McpImportObservation {
        source,
        protocol_version: golem_mcp_import::transport::PROTOCOL_VERSION.to_string(),
        tools: names.iter().map(|name| projected_tool(name)).collect(),
        diagnostics: Vec::new(),
    }
}

fn add_mcp_imports(deployment: &mut ToolDeploymentState, count: usize) {
    deployment.mcp_imports = (0..count)
        .map(|index| McpImport {
            url: format!("https://mcp-{index}.example.com"),
            auth: None,
            security_scheme: None,
            prefix: None,
            include: None,
            exclude: None,
            version: None,
        })
        .collect();
}

fn tool_names(tools: &[ToolSummary]) -> Vec<&str> {
    tools.iter().map(|tool| tool.0.as_str()).collect()
}

fn registered_tool(
    name: &str,
    component_id: ComponentId,
    component_revision: ComponentRevision,
    deployment_revision: DeploymentRevision,
) -> RegisteredTool {
    RegisteredTool {
        deployment_revision,
        release_id: None,
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
        component_bindings: Default::default(),
        source: ToolSource::Component {
            component_id,
            component_revision,
            component_name: ComponentName("tool-component".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("test@golem"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
    }
}

fn binding(
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    tool: &RegisteredTool,
) -> CompiledToolBinding {
    CompiledToolBinding {
        deployment_revision: tool.deployment_revision,
        release_id: tool.release_id,
        owner: ToolBindingOwner::AgentType {
            agent_type_name: agent_type.clone(),
        },
        tool_name: tool_name.clone(),
        version: tool.definition.version.clone(),
        metadata_version: tool.metadata_version.clone(),
        metadata_digest: tool.metadata_digest,
        account_id: tool.owner_account_id,
        account_email: tool.owner_account_email.clone(),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable: Default::default(),
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

    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    ToolDeploymentState {
        deployment_revision,
        registered_tools,
        tool_bindings: BTreeMap::from([(owner, bindings)]),
        mcp_imports: Vec::new(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
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
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    deployment.tool_bindings.insert(owner, bindings);
}

fn add_missing_component_middleware(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    component_revision: ComponentRevision,
) -> ComponentId {
    let component_id = ComponentId::new();
    let middleware_name = ToolMiddlewareName::try_from("test-middleware").unwrap();
    let registered = RegisteredToolMiddleware {
        deployment_revision: deployment.deployment_revision,
        release_id: None,
        definition: ToolMiddleware {
            name: middleware_name.to_string(),
            version: "1.0.0".to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            scope: ToolMiddlewareScope::Universal,
            parameter_schema: SchemaGraph::empty(),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolMiddlewareSource::Component {
            component_id,
            component_revision,
            component_name: ComponentName("test-middleware-component".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("middleware@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
    };
    let effective_definition = deployment.registered_tools[tool_name].definition.clone();
    let occurrence = CompiledToolMiddlewareOccurrence {
        middleware: registered.clone(),
        parameters: golem_common::schema::TypedSchemaValue::new(
            registered.definition.parameter_schema.clone(),
            golem_common::schema::SchemaValue::Record { fields: vec![] },
        ),
        provision: ToolProvisionConfig::default(),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: golem_common::model::tool::ToolFilesystemAccess::Unset,
        expected_definition: None,
        presented_definition: None,
        next_effective_definition: effective_definition.clone(),
        compatibility: None,
    };
    deployment
        .registered_tool_middlewares
        .insert(middleware_name, registered);
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    deployment.tool_middleware_chains.insert(
        owner.clone(),
        BTreeMap::from([(
            tool_name.clone(),
            CompiledToolMiddlewareChain {
                deployment_revision: deployment.deployment_revision,
                owner,
                tool_name: tool_name.clone(),
                effective_definition,
                occurrences: vec![occurrence],
            },
        )]),
    );
    component_id
}

fn rename_tool_with_middleware(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    presented_name: &str,
) {
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    let mut effective_definition = deployment.registered_tools[tool_name].definition.clone();
    effective_definition.commands.nodes[0].name = presented_name.to_string();
    deployment.tool_middleware_chains.insert(
        owner.clone(),
        BTreeMap::from([(
            tool_name.clone(),
            CompiledToolMiddlewareChain {
                deployment_revision: deployment.deployment_revision,
                owner,
                tool_name: tool_name.clone(),
                effective_definition,
                occurrences: Vec::new(),
            },
        )]),
    );
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
            10,
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

    let calls_before_invalid_name = service.tool_deployment_calls();
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
    assert_eq!(service.tool_deployment_calls(), calls_before_invalid_name);

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
    assert_eq!(service.tool_deployment_calls(), 11);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn tool_invocation_uses_caller_owned_tagged_snapshot_dispatch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let tool_name = ToolName::try_from("remote-search").unwrap();
    let publisher_account_id = AccountId::new();
    let publisher_account_email = AccountEmail::new("publisher@example.com");
    let tool_component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let tool_component = executor
        .update_component(&tool_component.id, &host_api_tests.wasm_name)
        .await?;
    let tool_component_id = tool_component.id;
    let tool_component_revision = tool_component.revision;
    assert_ne!(publisher_account_id, context.account_id);
    assert_ne!(tool_component_id, component.id);
    assert_ne!(tool_component_revision, component.revision);

    let mut component_deployment = deployment_state(
        &agent_type,
        1,
        tool_component_revision,
        &[(tool_name.as_str(), tool_component_id, true)],
    );
    let registered = component_deployment
        .registered_tools
        .get_mut(&tool_name)
        .unwrap();
    registered.owner_account_id = publisher_account_id;
    registered.owner_account_email = publisher_account_email.clone();
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    let binding = component_deployment
        .tool_bindings
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    binding.account_id = publisher_account_id;
    binding.account_email = publisher_account_email.clone();
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(component_deployment),
    );

    let component_caller = agent_id!("GolemHostApi", "component-snapshot-dispatch");
    executor
        .start_agent(&component.id, component_caller.clone())
        .await?;
    let component_result = executor
        .invoke_and_await_agent(
            &component,
            &component_caller,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name.as_str(), Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        component_result.as_ref().is_err_and(|error| {
            error.contains("InvalidToolName")
                && error.contains(tool_name.as_str())
                && !error.contains("Denied")
        }),
        "cross-account component source must pass caller-owned authorization and reach dispatch: {component_result:?}"
    );

    let updated_component = executor
        .update_component(&component.id, &host_api_tests.wasm_name)
        .await?;
    let mut host_deployment = deployment_state(
        &agent_type,
        2,
        updated_component.revision,
        &[(tool_name.as_str(), ComponentId::new(), true)],
    );
    let host_source = ToolSource::Host {
        host_tool_id: HostToolId::try_from("native-search".to_string()).unwrap(),
        implementation_version: "2026.08.28".to_string(),
    };
    let registered = host_deployment
        .registered_tools
        .get_mut(&tool_name)
        .unwrap();
    registered.owner_account_id = publisher_account_id;
    registered.owner_account_email = publisher_account_email.clone();
    registered.source = host_source.clone();
    let binding = host_deployment
        .tool_bindings
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    binding.account_id = publisher_account_id;
    binding.account_email = publisher_account_email;
    binding.source = host_source;
    service.set_tool_deployment(
        context.default_environment_id,
        updated_component.id,
        updated_component.revision,
        Some(host_deployment),
    );

    let host_caller = agent_id!("GolemHostApi", "host-snapshot-dispatch");
    executor
        .start_agent(&updated_component.id, host_caller.clone())
        .await?;
    let host_result = executor
        .invoke_and_await_agent(
            &updated_component,
            &host_caller,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name.as_str(), Vec::<String>::new(), String::new()),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        host_result.contains("native-search")
            && host_result.contains("2026.08.28")
            && host_result.contains("is not installed")
            && !host_result.contains("Denied")
            && !host_result.contains("panicked"),
        "host source must pass shared admission and reach tagged dispatch: {host_result:?}"
    );
    assert_eq!(service.tool_deployment_calls(), 2);
    assert_eq!(
        service.tool_deployment_lookups(),
        vec![
            (
                context.default_environment_id,
                component.id,
                component.revision,
            ),
            (
                context.default_environment_id,
                updated_component.id,
                updated_component.revision,
            ),
        ]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn durable_tool_invocation_fails_when_middleware_component_is_missing(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let tool_name = ToolName::try_from("middleware-search").unwrap();
    let mut deployment = deployment_state(
        &agent_type,
        1,
        component.revision,
        &[(tool_name.as_str(), component.id, true)],
    );
    let missing_component = add_missing_component_middleware(
        &mut deployment,
        &agent_type,
        &tool_name,
        component.revision,
    );
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("GolemHostApi", "middleware-fail-closed");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "tool_rpc_invoke_and_await_result",
            data_value!(tool_name.as_str(), Vec::<String>::new(), String::new()),
        )
        .await;

    let error = result.expect_err("missing middleware component must fail the invocation");
    assert!(
        error.to_string().contains(&format!(
            "No such component found: {missing_component}/{}",
            component.revision
        )),
        "dispatch must resolve the pinned middleware rather than bypass it: {error}"
    );
    assert_eq!(
        service.tool_deployment_lookups(),
        vec![(
            context.default_environment_id,
            component.id,
            component.revision
        )]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn tool_discovery_replay_rehydrates_exact_revision_without_live_lookup(
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
    assert_eq!(service.tool_deployment_calls(), 1);
    executor.check_oplog_is_queryable(&worker_id).await?;
    assert_eq!(
        assert_recorded_discovery_snapshot(&executor, &worker_id, Some(1))
            .await?
            .dynamic_tools,
        Vec::new()
    );

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
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;

    let replayed = executor
        .invoke_and_await_agent(&component, &agent_id, "get_recorded_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(replayed, vec![summary("alpha", alpha_component)]);
    assert_eq!(
        service.tool_deployment_calls(),
        1,
        "replay must not consult the current environment state"
    );
    assert_eq!(
        service.tool_deployment_revision_lookups(),
        vec![(context.default_environment_id, 1_u64.try_into().unwrap())]
    );

    let live_after_replay = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(live_after_replay, vec![summary("beta", beta_component)]);
    assert_eq!(service.tool_deployment_calls(), 2);

    drop(executor);
    service
        .remove_tool_deployment_revision(context.default_environment_id, 1_u64.try_into().unwrap());
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let missing = executor
        .invoke_and_await_agent(&component, &agent_id, "get_recorded_tools", data_value!())
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("does not exist"), "{missing}");
    assert_eq!(
        service.tool_deployment_calls(),
        2,
        "missing history must not fall back to a current deployment"
    );
    assert_eq!(service.tool_deployment_revision_lookups().len(), 2);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn middleware_renamed_tool_keeps_binding_lookup_name_live_and_on_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let tool_name = ToolName::try_from("beta").unwrap();
    let tool_component = ComponentId::new();
    let mut deployment = deployment_state(
        &agent_type,
        1,
        component.revision,
        &[(tool_name.as_str(), tool_component, true)],
    );
    rename_tool_with_middleware(&mut deployment, &agent_type, &tool_name, "adapter-a");
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("GolemHostApi", "middleware-renamed-discovery");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "record_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(recorded, vec![summary("beta", tool_component)]);
    let live_lookup = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_tool",
            data_value!("beta".to_string()),
        )
        .await?
        .into_typed::<Option<ToolSummary>>()?;
    assert_eq!(live_lookup, Some(summary("beta", tool_component)));

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed = executor
        .invoke_and_await_agent(&component, &agent_id, "get_recorded_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(replayed, vec![summary("beta", tool_component)]);
    assert_eq!(
        service.tool_deployment_revision_lookups(),
        vec![
            (context.default_environment_id, 1_u64.try_into().unwrap()),
            (context.default_environment_id, 1_u64.try_into().unwrap()),
        ],
        "both get_all_tools and get_tool must rehydrate their recorded deployment during replay"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn tool_discovery_replay_distinguishes_absent_and_empty_deployments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());

    let absent_agent_id = agent_id!("GolemHostApi", "tool-discovery-absent-replay");
    let absent_worker_id = executor
        .start_agent(&component.id, absent_agent_id.clone())
        .await?;
    let absent = executor
        .invoke_and_await_agent(
            &component,
            &absent_agent_id,
            "record_all_tools",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(absent, Vec::new());
    assert_eq!(
        assert_recorded_discovery_snapshot(&executor, &absent_worker_id, None)
            .await?
            .dynamic_tools,
        Vec::new()
    );

    let empty_revision = DeploymentRevision::try_from(10_u64).unwrap();
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &agent_type,
            empty_revision.get(),
            component.revision,
            &[],
        )),
    );
    let empty_agent_id = agent_id!("GolemHostApi", "tool-discovery-empty-replay");
    let empty_worker_id = executor
        .start_agent(&component.id, empty_agent_id.clone())
        .await?;
    let empty = executor
        .invoke_and_await_agent(
            &component,
            &empty_agent_id,
            "record_all_tools",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(empty, Vec::new());
    assert_eq!(
        assert_recorded_discovery_snapshot(
            &executor,
            &empty_worker_id,
            Some(empty_revision.get()),
        )
        .await?
        .dynamic_tools,
        Vec::new()
    );

    drop(executor);
    let populated_component = ComponentId::new();
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            &agent_type,
            11,
            component.revision,
            &[("populated", populated_component, true)],
        )),
    );
    let executor = start_with_overrides(deps, &context, overrides).await?;

    for agent_id in [&absent_agent_id, &empty_agent_id] {
        let replayed = executor
            .invoke_and_await_agent(&component, agent_id, "get_recorded_tools", data_value!())
            .await?
            .into_typed::<Vec<ToolSummary>>()?;
        assert_eq!(replayed, Vec::new());
    }
    assert_eq!(
        service.tool_deployment_revision_lookups(),
        vec![(context.default_environment_id, empty_revision)],
        "absence must replay as None while an empty deployment must rehydrate its exact revision"
    );
    assert_eq!(
        service.tool_deployment_calls(),
        2,
        "neither replay may select the later populated deployment"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn dynamic_tool_discovery_records_ordered_observations_and_replays_without_resolution(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let mut deployment = deployment_state(
        &agent_type,
        20,
        component.revision,
        &[
            ("native-bound", ComponentId::new(), true),
            ("native-unbound", ComponentId::new(), false),
        ],
    );
    add_mcp_imports(&mut deployment, 3);
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let sources = (0..3)
        .map(|index| mcp_source(context.default_environment_id, 20, index))
        .collect::<Vec<_>>();
    service.set_mcp_observation(
        sources[0].clone(),
        Ok(mcp_observation(
            sources[0].clone(),
            &["first", "duplicate", "native-bound", "native-unbound"],
        )),
    );
    let mut empty = mcp_observation(sources[1].clone(), &[]);
    empty.diagnostics.push(Diagnostic {
        upstream_name: "excluded-upstream".to_string(),
        reason: "excluded by test policy".to_string(),
    });
    service.set_mcp_observation(sources[1].clone(), Ok(empty));
    service.set_mcp_observation(
        sources[2].clone(),
        Ok(mcp_observation(
            sources[2].clone(),
            &["duplicate", "later-only"],
        )),
    );

    let agent_id = agent_id!("GolemHostApi", "dynamic-discovery-replay");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let recorded = executor
        .invoke_and_await_agent(&component, &agent_id, "record_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(
        tool_names(&recorded),
        vec!["native-bound", "first", "duplicate", "later-only"]
    );
    let requests = service.mcp_observation_requests();
    assert_eq!(
        requests
            .iter()
            .map(|(source, _)| source)
            .collect::<Vec<_>>(),
        sources.iter().collect::<Vec<_>>()
    );
    assert!(
        requests
            .iter()
            .all(|(_, auth)| matches!(auth, AuthCtx::Agent(_)))
    );

    let snapshot = assert_recorded_discovery_snapshot(&executor, &worker_id, Some(20)).await?;
    assert_eq!(
        snapshot
            .dynamic_tools
            .iter()
            .map(|observation| observation.import_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(snapshot.dynamic_tools[1].tools.0, Vec::new());
    assert_eq!(
        snapshot.dynamic_tools[1].exclusions,
        vec![(
            "excluded-upstream".to_string(),
            "excluded by test policy".to_string()
        )]
    );
    assert_eq!(
        snapshot
            .dynamic_tools
            .iter()
            .flat_map(|observation| observation.tools.0.iter())
            .filter_map(|tool| tool.definition.name())
            .collect::<Vec<_>>(),
        vec![
            "first",
            "duplicate",
            "native-bound",
            "native-unbound",
            "duplicate",
            "later-only"
        ]
    );

    drop(executor);
    service.clear_mcp_observations();
    let mut newer = deployment_state(&agent_type, 21, component.revision, &[]);
    add_mcp_imports(&mut newer, 1);
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(newer),
    );
    let newer_source = mcp_source(context.default_environment_id, 21, 0);
    service.set_mcp_observation(
        newer_source.clone(),
        Ok(mcp_observation(newer_source, &["fresh"])),
    );
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed = executor
        .invoke_and_await_agent(&component, &agent_id, "get_recorded_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(tool_names(&replayed), tool_names(&recorded));
    assert_eq!(
        service.mcp_observation_requests().len(),
        3,
        "replay resolved MCP"
    );

    let fresh = executor
        .invoke_and_await_agent(&component, &agent_id, "get_all_tools", data_value!())
        .await?
        .into_typed::<Vec<ToolSummary>>()?;
    assert_eq!(tool_names(&fresh), vec!["fresh"]);
    assert_eq!(service.mcp_observation_requests().len(), 4);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn dynamic_tool_lookup_is_cold_ordered_and_fail_closed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let mut deployment = deployment_state(&agent_type, 30, component.revision, &[]);
    add_mcp_imports(&mut deployment, 2);
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let first = mcp_source(context.default_environment_id, 30, 0);
    let second = mcp_source(context.default_environment_id, 30, 1);
    service.set_mcp_observation(
        first.clone(),
        Ok(mcp_observation(first.clone(), &["winner"])),
    );
    service.set_mcp_observation(
        second.clone(),
        Ok(mcp_observation(second.clone(), &["winner", "later"])),
    );
    let agent_id = agent_id!("GolemHostApi", "dynamic-lookup-cold");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let winner = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_tool",
            data_value!("winner".to_string()),
        )
        .await?
        .into_typed::<Option<ToolSummary>>()?;
    assert_eq!(winner.as_ref().map(|tool| tool.0.as_str()), Some("winner"));
    assert_eq!(
        service
            .mcp_observation_requests()
            .iter()
            .map(|(source, _)| source.import_index)
            .collect::<Vec<_>>(),
        vec![0]
    );

    service.set_mcp_observation(
        first,
        Err(RegistryServiceError::NotFound(
            "first import unavailable".to_string(),
        )),
    );
    let requests_before = service.mcp_observation_requests().len();
    let error = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_tool",
            data_value!("later".to_string()),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("first import unavailable"),
        "{error}"
    );
    assert!(
        service.mcp_observation_requests()[requests_before..]
            .iter()
            .all(|(source, _)| source.import_index == 0),
        "a failed earlier import must not promote a later definition"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn dynamic_discovery_quota_exhaustion_suspends_without_completing_the_observation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(service.clone()),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let mut deployment = deployment_state(&agent_type, 40, component.revision, &[]);
    add_mcp_imports(&mut deployment, 1);
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let source = mcp_source(context.default_environment_id, 40, 0);
    service.set_mcp_observation(
        source,
        Err(RegistryServiceError::LimitExceeded(
            "monthly HTTP budget".into(),
        )),
    );
    let agent_id = agent_id!("GolemHostApi", "dynamic-discovery-quota");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(&component, &agent_id, "record_all_tools", data_value!())
                .await
        })
    };
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(30))
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let discovery_starts = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(start)
                    if start.function_name == "golem::tool::host::get_all_tools"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        discovery_starts.len(),
        1,
        "quota retry duplicated discovery Start"
    );
    assert!(
        !oplog.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::End(end) if end.start_index == discovery_starts[0].oplog_index
        )),
        "quota suspension must leave the discovery observation incomplete"
    );
    invocation.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn dynamic_tool_invocation_replays_committed_response_without_upstream_or_credentials(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use serde_json::{Value, json};
    use std::sync::Mutex;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let effects = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    let crash_after_effect = Arc::new(tokio::sync::Notify::new());
    let handler = axum::routing::post({
        let requests = requests.clone();
        let effects = effects.clone();
        let crash_after_effect = crash_after_effect.clone();
        move |headers: http::HeaderMap, axum::Json(body): axum::Json<Value>| {
            let requests = requests.clone();
            let effects = effects.clone();
            let crash_after_effect = crash_after_effect.clone();
            async move {
                use axum::response::IntoResponse;
                let key = headers
                    .get("idempotency-key")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                assert_eq!(body["method"], "tools/call");
                assert_eq!(body["params"]["name"], "dynamic-call");
                let count = {
                    let mut requests = requests.lock().unwrap();
                    requests.push((key.clone(), body.clone()));
                    requests.len()
                };
                if count == 5 {
                    return http::StatusCode::UNAUTHORIZED.into_response();
                }
                let effect = {
                    let mut effects = effects.lock().unwrap();
                    let next = effects.len() + 1;
                    *effects.entry(key).or_insert(next)
                };
                if count == 2 {
                    crash_after_effect.notify_one();
                    std::future::pending::<()>().await;
                }
                if count == 1 {
                    // The effect happened, but the HTTP body fails before its
                    // complete response reaches the executor.
                    return http::Response::builder()
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from_stream(futures::stream::iter([
                            Ok(bytes::Bytes::from_static(b"{\"jsonrpc\":\"2.0\",")),
                            Err(std::io::Error::other("lost MCP response")),
                        ])))
                        .unwrap();
                }
                axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":{
                    "content":[{"type":"text","text":format!("effect {effect}")}],
                    "structuredContent":{"count":effect}
                }}))
                .into_response()
            }
        }
    });
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/mcp", handler))
            .await
            .unwrap();
    }));
    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let mut deployment = deployment_state(&agent_type, 41, component.revision, &[]);
    add_mcp_imports(&mut deployment, 1);
    deployment.mcp_imports[0].url = format!("http://127.0.0.1:{port}/mcp");
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let source = mcp_source(context.default_environment_id, 41, 0);
    service.set_mcp_observation(
        source.clone(),
        Ok(mcp_observation(source.clone(), &["dynamic-call"])),
    );
    let mut credential_source = source.clone();
    credential_source.upstream_tool_name = "dynamic-call".into();
    let credential_generation = uuid::Uuid::new_v4();
    service.set_mcp_credential(
        credential_source.clone(),
        golem_service_base::clients::registry::McpRuntimeCredential {
            credential: None,
            oauth_grant_generation: Some(credential_generation),
        },
    );
    let agent_id = agent_id!("GolemHostApi", "dynamic-invocation-admission");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let invocation_key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let invocation_key = invocation_key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &invocation_key,
                    "tool_rpc_invoke_and_await_result",
                    data_value!("dynamic-call", Vec::<String>::new(), String::new()),
                )
                .await
        })
    };
    crash_after_effect.notified().await;
    invocation.abort();
    let _ = invocation.await;
    drop(executor);
    executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &invocation_key,
            "tool_rpc_invoke_and_await_result",
            data_value!("dynamic-call", Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(result, Ok(()));
    let first_call_key = requests.lock().unwrap()[0].0.clone();
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert_eq!(
        effects.lock().unwrap().len(),
        1,
        "response loss duplicated the logical effect"
    );
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|(key, _)| key == &first_call_key),
        "inline retries and executor reconstruction must preserve the MCP idempotency key"
    );
    let duplicate = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &invocation_key,
            "tool_rpc_invoke_and_await_result",
            data_value!("dynamic-call", Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(duplicate, Ok(()));
    assert_eq!(requests.lock().unwrap().len(), 3);

    let next_key = IdempotencyKey::fresh();
    let next = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &next_key,
            "tool_rpc_invoke_and_await_result",
            data_value!("dynamic-call", Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(next, Ok(()));
    assert_eq!(requests.lock().unwrap().len(), 4);
    assert_ne!(requests.lock().unwrap()[3].0, first_call_key);
    assert_eq!(effects.lock().unwrap().len(), 2);

    let unauthorized_key = IdempotencyKey::fresh();
    let unauthorized = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &unauthorized_key,
            "tool_rpc_invoke_and_await_result",
            data_value!("dynamic-call", Vec::<String>::new(), String::new()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        unauthorized
            .as_ref()
            .is_err_and(|error| error.contains("RemoteInternalError") && error.contains("401")),
        "HTTP 401 must reach the guest as a categorized tool failure: {unauthorized:?}"
    );
    assert_eq!(requests.lock().unwrap().len(), 5, "HTTP 401 was retried");
    assert_eq!(
        service.mcp_unauthorized_reports(),
        vec![(credential_source.clone(), Some(credential_generation))],
        "HTTP 401 must report exactly one source and credential generation"
    );
    assert_eq!(
        service
            .mcp_observation_requests()
            .iter()
            .map(|(request, auth)| (request, matches!(auth, AuthCtx::Agent(_))))
            .collect::<Vec<_>>(),
        vec![(&source, true), (&source, true), (&source, true)]
    );
    assert_eq!(service.mcp_credential_requests().len(), 5);
    assert_eq!(service.mcp_credential_requests()[0].0, credential_source);
    assert!(matches!(
        &service.mcp_credential_requests()[0].1,
        AuthCtx::Agent(_)
    ));

    #[derive(FromSchema)]
    struct RecordedResponse {
        result:
            Result<Vec<u8>, golem_common::model::oplog::payload::types::SerializableToolRpcError>,
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let call = oplog.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call")).expect("nested MCP Start");
    let response = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == call.oplog_index => {
                Some(RecordedResponse::from_value(end.response.as_ref().unwrap().value()).unwrap())
            }
            _ => None,
        })
        .expect("nested MCP End");
    let raw: Value = serde_json::from_slice(&response.result.unwrap())?;
    assert_eq!(raw["Ok"]["structuredContent"]["count"], 1);
    let live_lookups = service.tool_deployment_calls();
    let exact_lookups = service.tool_deployment_revision_lookups().len();
    drop(server);
    service.clear_mcp_observations();
    service.clear_mcp_credentials();
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let recovered_agent_id = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(recovered_agent_id, Ok(agent_id.to_string()));
    assert_eq!(effects.lock().unwrap().len(), 2);
    assert_eq!(
        requests.lock().unwrap().len(),
        5,
        "completed replay repeated MCP effect"
    );
    assert_eq!(
        service.mcp_credential_requests().len(),
        5,
        "replay acquired credentials"
    );
    assert_eq!(
        service.mcp_observation_requests().len(),
        3,
        "replay resolved metadata"
    );
    assert_eq!(
        service.mcp_unauthorized_reports(),
        vec![(credential_source, Some(credential_generation))],
        "replay repeated authorization feedback"
    );
    assert_eq!(
        service.tool_deployment_calls(),
        live_lookups,
        "replay selected a live deployment"
    );
    assert_eq!(
        service.tool_deployment_revision_lookups().len(),
        exact_lookups,
        "bridge replay required registry access"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn dynamic_tool_invalid_params_forces_refresh_to_distinguish_removal_from_exclusion(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use axum::response::IntoResponse;
    use serde_json::{Value, json};
    use tokio::sync::{mpsc, oneshot};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (received_tx, mut received_rx) = mpsc::unbounded_channel();
    let releases = Arc::new(std::sync::Mutex::new(Vec::<oneshot::Receiver<()>>::new()));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handler = axum::routing::post({
        let releases = releases.clone();
        let calls = calls.clone();
        move |axum::Json(body): axum::Json<Value>| {
            let release = releases.lock().unwrap().remove(0);
            let received_tx = received_tx.clone();
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                assert_eq!(body["method"], "tools/call");
                received_tx.send(()).unwrap();
                release.await.unwrap();
                axum::Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "error": {"code": -32602, "message": "invalid parameters"}
                }))
                .into_response()
            }
        }
    });
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/mcp", handler))
            .await
            .unwrap();
    }));

    let context = TestContext::new(last_unique_id);
    let service = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(service.clone()),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_type = AgentTypeName("GolemHostApi".to_string());
    let mut deployment = deployment_state(&agent_type, 42, component.revision, &[]);
    add_mcp_imports(&mut deployment, 1);
    deployment.mcp_imports[0].url = format!("http://127.0.0.1:{port}/mcp");
    service.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment),
    );
    let source = mcp_source(context.default_environment_id, 42, 0);
    service.set_mcp_observation(
        source.clone(),
        Ok(mcp_observation(source.clone(), &["dynamic-call"])),
    );
    let mut credential_source = source.clone();
    credential_source.upstream_tool_name = "dynamic-call".into();
    service.set_mcp_credential(
        credential_source,
        golem_service_base::clients::registry::McpRuntimeCredential {
            credential: None,
            oauth_grant_generation: None,
        },
    );
    let agent_id = agent_id!("GolemHostApi", "dynamic-invalid-params-presence");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    for excluded_but_present in [false, true] {
        let (release_tx, release_rx) = oneshot::channel();
        releases.lock().unwrap().push(release_rx);
        let invocation_key = IdempotencyKey::fresh();
        let invocation = {
            let executor = executor.clone();
            let component = component.clone();
            let agent_id = agent_id.clone();
            let invocation_key = invocation_key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent_id,
                        &invocation_key,
                        "tool_rpc_invoke_and_await_result",
                        data_value!("dynamic-call", Vec::<String>::new(), String::new()),
                    )
                    .await
            })
        };
        received_rx.recv().await.expect("tools/call received");
        let mut refreshed = mcp_observation(source.clone(), &[]);
        if excluded_but_present {
            refreshed.diagnostics.push(Diagnostic {
                upstream_name: "dynamic-call".into(),
                reason: "excluded by policy".into(),
            });
        }
        service.set_mcp_observation(
            source.clone(),
            if excluded_but_present {
                Ok(refreshed.clone())
            } else {
                Err(RegistryServiceError::LimitExceeded(
                    "presence HTTP budget".into(),
                ))
            },
        );
        release_tx.send(()).unwrap();
        let result = if excluded_but_present {
            invocation.await??
        } else {
            executor
                .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(30))
                .await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let remote = oplog.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call")).expect("remote Start");
            let remote_end = oplog.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == remote.oplog_index)).expect("committed remote End");
            let presence = oplog.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::presence")).expect("presence Start");
            assert!(remote_end.oplog_index < presence.oplog_index);
            assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == presence.oplog_index)), "quota must leave presence incomplete");
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            invocation.abort();
            let _ = invocation.await;
            drop(executor);
            service.set_mcp_observation(source.clone(), Ok(refreshed));
            executor = start_with_overrides(deps, &context, overrides.clone()).await?;
            executor.resume(&worker_id, false).await?;
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &invocation_key,
                    "tool_rpc_invoke_and_await_result",
                    data_value!("dynamic-call", Vec::<String>::new(), String::new()),
                )
                .await?
        };
        let result = result.into_typed::<Result<(), String>>()?.unwrap_err();
        let expected = if excluded_but_present {
            "InvalidInput"
        } else {
            "InvalidToolName"
        };
        assert!(result.contains(expected), "expected {expected}: {result}");

        service.set_mcp_observation(
            source.clone(),
            Ok(mcp_observation(source.clone(), &["dynamic-call"])),
        );
    }

    assert_eq!(
        service.mcp_observation_refreshes(),
        vec![false, true, true, false, true]
    );
    let requests = service.mcp_observation_requests();
    assert_eq!(requests.len(), 5);
    assert!(requests[1].0.upstream_tool_name.is_empty());
    assert!(requests[2].0.upstream_tool_name.is_empty());
    assert!(requests[4].0.upstream_tool_name.is_empty());

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for (name, expected) in [
        ("golem::tool::mcp::call", 2),
        ("golem::tool::mcp::presence", 2),
    ] {
        let starts = oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == name)).collect::<Vec<_>>();
        assert_eq!(starts.len(), expected);
        for start in starts {
            assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start.oplog_index)).count(), 1);
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    drop(server);
    service.clear_mcp_observations();
    service.clear_mcp_credentials();
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let recovered = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(recovered, Ok(agent_id.to_string()));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        service.mcp_observation_refreshes(),
        vec![false, true, true, false, true],
        "completed presence replay must not refresh"
    );
    assert_eq!(
        service.mcp_credential_requests().len(),
        2,
        "completed rejection replay acquired credentials"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn dynamic_tool_crash_obeys_caller_atomic_and_non_idempotent_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use serde_json::{Value, json};
    use std::sync::Mutex;
    for (idempotent, atomic) in [(true, true), (false, false)] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let effects = Arc::new(Mutex::new(std::collections::BTreeSet::new()));
        let crash_after_effect = Arc::new(tokio::sync::Notify::new());
        let handler = axum::routing::post({
            let requests = requests.clone();
            let effects = effects.clone();
            let crash_after_effect = crash_after_effect.clone();
            move |headers: http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                let requests = requests.clone();
                let effects = effects.clone();
                let crash_after_effect = crash_after_effect.clone();
                async move {
                    assert_eq!(body["method"], "tools/call");
                    let key = headers["idempotency-key"].to_str().unwrap().to_owned();
                    let first = {
                        let mut requests = requests.lock().unwrap();
                        requests.push(key.clone());
                        requests.len() == 1
                    };
                    effects.lock().unwrap().insert(key);
                    if first {
                        crash_after_effect.notify_one();
                        std::future::pending::<()>().await;
                    }
                    axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":[]}}))
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().route("/mcp", handler))
                .await
                .unwrap();
        }));
        let context = TestContext::new(last_unique_id);
        let service = Arc::new(TestEnvironmentStateService::default());
        let overrides = TestExecutorOverrides {
            environment_state_service: Some(service.clone()),
            ..Default::default()
        };
        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let mut deployment = deployment_state(
            &AgentTypeName("GolemHostApi".into()),
            41,
            component.revision,
            &[],
        );
        add_mcp_imports(&mut deployment, 1);
        deployment.mcp_imports[0].url = format!("http://127.0.0.1:{port}/mcp");
        service.set_tool_deployment(
            context.default_environment_id,
            component.id,
            component.revision,
            Some(deployment),
        );
        let source = mcp_source(context.default_environment_id, 41, 0);
        service.set_mcp_observation(
            source.clone(),
            Ok(mcp_observation(source.clone(), &["dynamic-call"])),
        );
        let mut credential_source = source;
        credential_source.upstream_tool_name = "dynamic-call".into();
        service.set_mcp_credential(
            credential_source,
            golem_service_base::clients::registry::McpRuntimeCredential {
                credential: None,
                oauth_grant_generation: None,
            },
        );
        let agent_id = agent_id!("GolemHostApi", format!("mcp-policy-{idempotent}-{atomic}"));
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        let invocation_key = IdempotencyKey::fresh();
        let invocation = {
            let executor = executor.clone();
            let component = component.clone();
            let agent_id = agent_id.clone();
            let key = invocation_key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent_id,
                        &key,
                        "tool_rpc_invoke_with_policy",
                        data_value!("dynamic-call", idempotent, atomic),
                    )
                    .await
            })
        };
        crash_after_effect.notified().await;
        let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let original = before.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call")).expect("committed incomplete MCP Start").oplog_index;
        assert!(!before.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == original)));
        invocation.abort();
        let _ = invocation.await;
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        let result = executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent_id,
                &invocation_key,
                "tool_rpc_invoke_with_policy",
                data_value!("dynamic-call", idempotent, atomic),
            )
            .await;
        if atomic {
            assert_eq!(result?.into_typed::<Result<(), String>>()?, Ok(()));
            {
                let requests = requests.lock().unwrap();
                assert_eq!(requests.len(), 2);
                assert_eq!(
                    requests[0], requests[1],
                    "atomic rollback changed the child key"
                );
            }
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            assert!(oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call" && entry.oplog_index != original)), "atomic retry must run under a new physical Start");
        } else {
            assert!(
                result.is_err(),
                "non-idempotent incomplete effect must fail recovery: {result:?}"
            );
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "non-idempotent effect was resent"
            );
        }
        assert_eq!(
            effects.lock().unwrap().len(),
            1,
            "recovery duplicated the upstream effect"
        );
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn dynamic_tool_http_limits_apply_before_dispatch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedLimits(Arc<AtomicResourceEntry>);

    #[async_trait::async_trait]
    impl ResourceLimits for FixedLimits {
        async fn initialize_account(
            &self,
            _account_id: AccountId,
        ) -> Result<
            Arc<AtomicResourceEntry>,
            golem_service_base::error::worker_executor::WorkerExecutorError,
        > {
            Ok(self.0.clone())
        }
    }

    for (per_invocation, monthly) in [(0, 1), (1, 1), (1, 0)] {
        let requests = Arc::new(AtomicUsize::new(0));
        let handler = axum::routing::post({
            let requests = requests.clone();
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let requests = requests.clone();
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    axum::Json(
                        serde_json::json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":[]}}),
                    )
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().route("/mcp", handler))
                .await
                .unwrap();
        }));
        let context = TestContext::new(last_unique_id);
        let service = Arc::new(TestEnvironmentStateService::default());
        let entry = Arc::new(AtomicResourceEntry::new_with_all_limits(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            per_invocation,
            u64::MAX,
            monthly,
            u64::MAX,
            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ));
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                environment_state_service: Some(service.clone()),
                resource_limits: Some(Arc::new(FixedLimits(entry.clone()))),
                ..Default::default()
            },
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let mut deployment = deployment_state(
            &AgentTypeName("GolemHostApi".into()),
            43,
            component.revision,
            &[],
        );
        add_mcp_imports(&mut deployment, 1);
        deployment.mcp_imports[0].url = format!("http://127.0.0.1:{port}/mcp");
        service.set_tool_deployment(
            context.default_environment_id,
            component.id,
            component.revision,
            Some(deployment),
        );
        let source = mcp_source(context.default_environment_id, 43, 0);
        service.set_mcp_observation(
            source.clone(),
            Ok(mcp_observation(source.clone(), &["dynamic-call"])),
        );
        let mut credential_source = source;
        credential_source.upstream_tool_name = "dynamic-call".into();
        service.set_mcp_credential(
            credential_source,
            golem_service_base::clients::registry::McpRuntimeCredential {
                credential: None,
                oauth_grant_generation: None,
            },
        );
        let agent_id = agent_id!(
            "GolemHostApi",
            format!("mcp-limits-{per_invocation}-{monthly}")
        );
        let worker_id = executor
            .start_agent(&component.id, agent_id.clone())
            .await?;
        let invocation = {
            let executor = executor.clone();
            let component = component.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "tool_rpc_invoke_with_policy",
                        data_value!("dynamic-call", true, false),
                    )
                    .await
            })
        };
        if monthly == 0 {
            executor
                .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(30))
                .await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let starts = oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::tool::mcp::call")).collect::<Vec<_>>();
            assert_eq!(starts.len(), 1);
            assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == starts[0].oplog_index)), "quota suspension must leave the MCP call incomplete");
            assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == starts[0].oplog_index)), "quota suspension must not invent cancellation");
            invocation.abort();
            let _ = invocation.await;
        } else {
            let result = invocation.await?;
            if per_invocation == 0 {
                assert!(
                    result.is_err(),
                    "HTTP call limit must trap, not become a tool error: {result:?}"
                );
                let error = format!("{result:?}");
                assert!(
                    error.contains("HTTP call limit") || error.contains("ExceededHttpCallLimit"),
                    "{error}"
                );
            } else {
                assert_eq!(result?.into_typed::<Result<(), String>>()?, Ok(()));
            }
        }
        let accepted = u64::from(per_invocation > 0 && monthly > 0);
        assert_eq!(requests.load(Ordering::SeqCst), accepted as usize);
        assert_eq!(entry.remaining_http_calls(), monthly - accepted);
    }
    Ok(())
}
