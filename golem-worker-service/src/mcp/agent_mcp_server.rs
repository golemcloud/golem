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

use crate::mcp::agent_mcp_capability::McpAgentCapability;
use crate::mcp::agent_mcp_prompt::{AgentMcpPrompt, PromptRegistry};
use crate::mcp::agent_mcp_resource::{AgentMcpResource, McpResourceUri, ResourceRegistry};
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::serving::{McpServing, ServeKind, ServeRefusal, Serving};
use crate::mcp::{McpCapabilityLookup, invoke};
use crate::service::worker::WorkerService;
use golem_common::base_model::domain_registration::Domain;
use golem_common::schema::AgentTypeKind;
use golem_service_base::mcp::CompiledMcp;
use poem::http;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::router::tool::ToolRouter,
    model::*, service::RequestContext,
};
use std::sync::Arc;

/// Serves the MCP requests of one deployment domain. It keeps no state between requests: every
/// request is served with the deployment that the [`McpServing`] in its extensions names.
#[derive(Clone)]
pub struct GolemAgentMcpServer {
    mcp_definitions_lookup: Arc<dyn McpCapabilityLookup>,
    worker_service: Arc<WorkerService>,
}

/// The tools, resources and prompts of one deployment.
struct ServedCapabilities {
    router: ToolRouter<GolemAgentMcpServer>,
    tools: Vec<Tool>,
    resources: ResourceRegistry,
    prompts: PromptRegistry,
}

impl ServedCapabilities {
    fn of(compiled: &CompiledMcp) -> Result<Self, ErrorData> {
        let capabilities = agent_capabilities_from_deployment(compiled);
        let router = capabilities
            .tools
            .into_iter()
            .fold(ToolRouter::new(), |router, tool| router.with_route(tool));
        let tools = router
            .list_all()
            .into_iter()
            .map(Ok)
            .chain(
                compiled
                    .tools
                    .iter()
                    .map(invoke::native_tool::tool_metadata),
            )
            .collect::<Result<Vec<_>, _>>()?;
        let resources = capabilities.resources.into_iter().fold(
            ResourceRegistry::default(),
            |mut registry, resource| {
                registry.insert(resource);
                registry
            },
        );
        let prompts = capabilities.prompts.into_iter().fold(
            PromptRegistry::default(),
            |mut registry, prompt| {
                registry.insert(prompt);
                registry
            },
        );
        Ok(Self {
            router,
            tools,
            resources,
            prompts,
        })
    }
}

impl GolemAgentMcpServer {
    pub fn new(
        mcp_definitions_lookup: Arc<dyn McpCapabilityLookup>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            mcp_definitions_lookup,
            worker_service,
        }
    }

    pub async fn invoke_tool(
        &self,
        args_map: JsonObject,
        mcp_tool: &AgentMcpTool,
    ) -> Result<CallToolResult, ErrorData> {
        invoke::tool::invoke_tool(args_map, mcp_tool, &self.worker_service).await
    }

    async fn run_tool(
        &self,
        request: CallToolRequestParams,
        compiled: Arc<CompiledMcp>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Some(export) = compiled
            .tools
            .iter()
            .find(|tool| tool.mcp_name == request.name)
        {
            let (input, stdin) = export
                .parse_arguments(request.arguments.unwrap_or_default())
                .map_err(|error| ErrorData::invalid_params(error, None))?;
            let result = invoke::native_tool::invoke(
                &self.worker_service,
                &compiled,
                export,
                input,
                stdin,
                context.ct.clone(),
            )
            .await;
            if result.is_err() {
                self.mcp_definitions_lookup
                    .invalidate(&compiled.domain)
                    .await;
                if let Ok(current) = self.mcp_definitions_lookup.get(&compiled.domain).await
                    && !same_deployment(&compiled, &current)
                {
                    let _ = context.peer.notify_tool_list_changed().await;
                    return Err(changed_tool_set());
                }
            }
            return result.map(Into::into);
        }
        let capabilities = ServedCapabilities::of(&compiled)?;
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        capabilities.router.call(tcc).await
    }

    async fn run_resource(
        &self,
        uri: String,
        compiled: &CompiledMcp,
    ) -> Result<ReadResourceResponse, McpError> {
        let resource_registry = ServedCapabilities::of(compiled)?.resources;

        if let Some(resource) = resource_registry.get_static(&uri) {
            return invoke::resource::invoke_resource(&self.worker_service, resource, &uri, None)
                .await
                .map(Into::into);
        }

        let parsed_resource_uri = McpResourceUri::parse(&uri)
            .map_err(|e| McpError::invalid_params(format!("Invalid resource URI: {e}"), None))?;

        if let Some((resource, params)) =
            resource_registry.extract_mcp_resource_with_input(&parsed_resource_uri)
        {
            return invoke::resource::invoke_resource(
                &self.worker_service,
                resource,
                &uri,
                Some(params),
            )
            .await
            .map(Into::into);
        }

        Err(McpError::invalid_params(
            format!("Resource not found for URI: {}", uri),
            None,
        ))
    }
}

/// The deployment to serve the request with, from the request's [`McpServing`]. A refusal is
/// turned into the client's error here, the one place for every handler method.
async fn serve(
    context: &RequestContext<RoleServer>,
    kind: ServeKind,
) -> Result<Serving, ErrorData> {
    let serving = context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Arc<dyn McpServing>>())
        .cloned()
        .ok_or_else(|| {
            ErrorData::invalid_params("MCP request has no authenticated deployment", None)
        })?;
    match serving.serve(kind, &context.ct).await {
        Ok(serving) => Ok(serving),
        Err(ServeRefusal::ToolSetChanged) => {
            let _ = context.peer.notify_tool_list_changed().await;
            Err(changed_tool_set())
        }
        Err(ServeRefusal::Gone) => Err(ErrorData::invalid_request("MCP session ended", None)),
        Err(ServeRefusal::Unavailable) => Err(ErrorData::internal_error(
            "MCP session could not be read",
            None,
        )),
    }
}

/// Runs `work` until it completes. The serving's `while_running` work runs next to it; it never
/// completes.
async fn while_serving<T>(
    serving: Serving,
    work: impl Future<Output = Result<T, ErrorData>>,
) -> Result<T, ErrorData> {
    tokio::select! {
        result = work => result,
        never = serving.while_running => match never {},
    }
}

fn same_deployment(left: &CompiledMcp, right: &CompiledMcp) -> bool {
    left.domain == right.domain
        && left.environment_id == right.environment_id
        && left.deployment_revision == right.deployment_revision
}

fn changed_tool_set() -> ErrorData {
    ErrorData::invalid_params("tool set changed; list tools again", None)
}

pub struct AgentCapabilities {
    pub tools: Vec<AgentMcpTool>,
    pub resources: Vec<AgentMcpResource>,
    pub prompts: Vec<AgentMcpPrompt>,
}

pub async fn get_agent_capabilities(
    domain: &Domain,
    mcp_definition_lookup: &Arc<dyn McpCapabilityLookup>,
) -> AgentCapabilities {
    let compiled_mcp = match mcp_definition_lookup.get(domain).await {
        Ok(mcp) => mcp,
        Err(e) => {
            tracing::error!("Failed to get compiled MCP for domain {}: {}", domain.0, e);
            return AgentCapabilities {
                tools: vec![],
                resources: vec![],
                prompts: vec![],
            };
        }
    };

    agent_capabilities_from_deployment(&compiled_mcp)
}

fn agent_capabilities_from_deployment(compiled_mcp: &CompiledMcp) -> AgentCapabilities {
    let domain = &compiled_mcp.domain;
    let mut tools = vec![];
    let mut resources = vec![];
    let mut prompts = vec![];

    let account_id = compiled_mcp.account_id;
    let account_email = compiled_mcp.account_email.clone();
    let environment_id = compiled_mcp.environment_id;

    tracing::info!(
        "Found {} registered agent types for domain {}: {:?}",
        compiled_mcp.registered_agent_types.len(),
        domain.0,
        compiled_mcp
            .registered_agent_types
            .iter()
            .map(|rat| rat.agent_type.type_name.0.clone())
            .collect::<Vec<_>>()
    );

    for registered_agent_type in &compiled_mcp.registered_agent_types {
        if registered_agent_type.agent_type.kind == AgentTypeKind::HttpRouter {
            continue;
        }

        tracing::debug!(
            "Processing agent type {} for domain {}: implemented by component {}, methods: {:?}",
            registered_agent_type.agent_type.type_name.0,
            domain.0,
            registered_agent_type.implemented_by.component_id.0,
            registered_agent_type
                .agent_type
                .methods
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>()
        );

        let agent_type = &registered_agent_type.agent_type;
        let component_id = registered_agent_type.implemented_by.component_id;
        // One `SchemaGraph` per agent type, shared by its constructor and
        // methods; `SchemaType::Ref` bodies resolve against it both while
        // rendering MCP schemas and while converting to the legacy invoke
        // types.
        let schema_graph = Arc::new(agent_type.schema.clone());

        if let Some(prompt_hint) = &agent_type.constructor.prompt_hint {
            prompts.push(AgentMcpPrompt::from_constructor_hint(
                &agent_type.type_name,
                &agent_type.description,
                prompt_hint,
            ));
        }

        for method in &agent_type.methods {
            // Validate the method for MCP export before advertising anything.
            // If the capability cannot be exported, invoking it would always
            // fail, so skip both the tool/resource and its prompt.
            let agent_method_mcp = McpAgentCapability::from_agent_method(
                &account_id,
                &account_email,
                &environment_id,
                &agent_type.type_name,
                agent_type.mode,
                schema_graph.clone(),
                method,
                &agent_type.constructor,
                component_id,
            );

            match agent_method_mcp {
                Ok(McpAgentCapability::Tool(agent_mcp_tool)) => {
                    tools.push(*agent_mcp_tool);
                }
                Ok(McpAgentCapability::Resource(agent_mcp_resource)) => {
                    resources.push(*agent_mcp_resource);
                }
                Err(e) => {
                    tracing::warn!(
                        "Skipping method {} of agent type {} for domain {}: {:#}",
                        method.name,
                        agent_type.type_name.0,
                        domain.0,
                        e
                    );
                    continue;
                }
            }

            if let Some(prompt_hint) = &method.prompt_hint {
                prompts.push(AgentMcpPrompt::from_method_hint(
                    &agent_type.type_name,
                    &schema_graph,
                    method,
                    &agent_type.constructor,
                    prompt_hint,
                ));
            }
        }
    }

    tracing::info!(
        "Found {} tools, {} resources, and {} prompts for domain {}",
        tools.len(),
        resources.len(),
        prompts.len(),
        domain.0
    );

    AgentCapabilities {
        tools,
        resources,
        prompts,
    }
}

#[allow(deprecated)]
impl ServerHandler for GolemAgentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_03_26)
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_03_26))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let serving = serve(&context, ServeKind::List).await?;
        let tools = ServedCapabilities::of(&serving.deployment)?.tools;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let serving = serve(&context, ServeKind::Invoke).await?;
        let compiled = serving.deployment.clone();
        while_serving(serving, self.run_tool(request, compiled, context)).await
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let serving = serve(&context, ServeKind::List).await?;
        let resource_list = ServedCapabilities::of(&serving.deployment)?
            .resources
            .list_static_resources();

        tracing::info!("Listing {} static resources", resource_list.len());

        Ok(ListResourcesResult::with_all_items(resource_list))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let serving = serve(&context, ServeKind::List).await?;
        let resource_templates = ServedCapabilities::of(&serving.deployment)?
            .resources
            .list_resource_templates();

        tracing::info!("Listing {} resource templates", resource_templates.len());

        Ok(ListResourceTemplatesResult::with_all_items(
            resource_templates,
        ))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let serving = serve(&context, ServeKind::List).await?;
        let prompt_list = ServedCapabilities::of(&serving.deployment)?
            .prompts
            .list_prompts();

        tracing::info!("Listing {} prompts", prompt_list.len());

        Ok(ListPromptsResult::with_all_items(prompt_list))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let serving = serve(&context, ServeKind::Invoke).await?;
        let prompts = ServedCapabilities::of(&serving.deployment)?.prompts;
        let prompt = async {
            prompts
                .get_by_name(&request.name)
                .map(|p| p.get_prompt_result().into())
                .ok_or_else(|| {
                    McpError::invalid_params(format!("Prompt not found: {}", request.name), None)
                })
        };
        while_serving(serving, prompt).await
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParams { uri, .. }: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let serving = serve(&context, ServeKind::Invoke).await?;
        let compiled = serving.deployment.clone();
        while_serving(serving, self.run_resource(uri, &compiled)).await
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        serve(&context, ServeKind::Initialize).await?;
        Ok(self.get_info())
    }
}

#[cfg(test)]
mod tests {
    use super::agent_capabilities_from_deployment;
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, RegisteredAgentTypeImplementer, Snapshotting,
    };
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::schema::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
        OutputSchema, RegisteredAgentTypeSchema, SchemaGraph,
    };
    use golem_service_base::mcp::CompiledMcp;
    use test_r::test;

    fn agent_type(kind: AgentTypeKind, name: &str) -> RegisteredAgentTypeSchema {
        RegisteredAgentTypeSchema {
            agent_type: AgentTypeSchema {
                kind,
                type_name: AgentTypeName(name.to_string()),
                description: String::new(),
                source_language: String::new(),
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: Some("constructor prompt".to_string()),
                    input_schema: InputSchema::Parameters(vec![]),
                },
                methods: vec![AgentMethodSchema {
                    name: "call".to_string(),
                    description: String::new(),
                    prompt_hint: Some("method prompt".to_string()),
                    input_schema: InputSchema::Parameters(vec![]),
                    output_schema: OutputSchema::Unit,
                    http_endpoint: vec![],
                    read_only: None,
                }],
                dependencies: vec![],
                mode: AgentMode::Durable,
                http_mount: None,
                snapshotting: Snapshotting::Disabled(Empty {}),
                config: vec![],
            },
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: "component".to_string(),
                account_id: AccountId::new(),
                account_email: AccountEmail::new("owner@example.com"),
            },
        }
    }

    fn deployment(agent_type: RegisteredAgentTypeSchema) -> CompiledMcp {
        CompiledMcp {
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            environment_id: EnvironmentId::new(),
            application_name: ApplicationName("app".to_string()),
            environment_name: EnvironmentName("dev".to_string()),
            deployment_revision: DeploymentRevision::INITIAL,
            domain: Domain("mcp.example.com".to_string()),
            security_scheme_name: None,
            security_scheme: None,
            registered_agent_types: vec![agent_type],
            tools: vec![],
        }
    }

    #[test]
    fn tooling_corpus_controls_mcp_agent_discovery_by_kind() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();

        for id in ["tooling-router-discovery", "tooling-name-not-kind"] {
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap();
            let kind = match case["input"]["kind"].as_str().unwrap() {
                "regular" => AgentTypeKind::Regular,
                "http-router" => AgentTypeKind::HttpRouter,
                other => panic!("{id}: unsupported agent kind {other}"),
            };
            let name = case["input"]["name"].as_str().unwrap_or("Router");
            let capabilities =
                agent_capabilities_from_deployment(&deployment(agent_type(kind, name)));
            let included = !capabilities.tools.is_empty()
                || !capabilities.resources.is_empty()
                || !capabilities.prompts.is_empty();

            assert_eq!(
                included,
                case["expect"]["included"].as_bool().unwrap(),
                "{id}"
            );
        }
    }
}
