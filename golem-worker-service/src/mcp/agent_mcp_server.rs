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
use crate::mcp::{McpCapabilityLookup, invoke};
use crate::service::worker::WorkerService;
use dashmap::DashMap;
use golem_common::base_model::domain_registration::Domain;
use golem_service_base::mcp::CompiledMcp;
use poem::http;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::router::tool::ToolRouter,
    model::*, service::RequestContext,
};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct GolemAgentMcpServer {
    tool_router: Arc<RwLock<Option<ToolRouter<GolemAgentMcpServer>>>>,
    tools: Arc<DashMap<String, Tool>>,
    resources: Arc<RwLock<ResourceRegistry>>,
    prompts: Arc<RwLock<PromptRegistry>>,
    deployment: Arc<RwLock<Option<Arc<CompiledMcp>>>>,
    mcp_definitions_lookup: Arc<dyn McpCapabilityLookup>,
    worker_service: Arc<WorkerService>,
}

impl GolemAgentMcpServer {
    pub fn new(
        mcp_definitions_lookup: Arc<dyn McpCapabilityLookup>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            tool_router: Arc::new(RwLock::new(None)),
            tools: Arc::new(DashMap::new()),
            resources: Arc::new(RwLock::new(ResourceRegistry::default())),
            prompts: Arc::new(RwLock::new(PromptRegistry::default())),
            deployment: Arc::new(RwLock::new(None)),
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

    async fn build_capabilities(
        &self,
        compiled: &CompiledMcp,
    ) -> (
        ToolRouter<GolemAgentMcpServer>,
        Vec<AgentMcpResource>,
        Vec<AgentMcpPrompt>,
    ) {
        let capabilities = agent_capabilities_from_deployment(compiled);

        let mut router = ToolRouter::<GolemAgentMcpServer>::new();

        for tool in capabilities.tools {
            router = router.with_route(tool);
        }

        (router, capabilities.resources, capabilities.prompts)
    }

    async fn refresh_tools(&self, compiled: Arc<CompiledMcp>) -> Result<Vec<Tool>, ErrorData> {
        let mut pinned = self.deployment.write().await;
        if let Some(previous) = pinned.as_ref()
            && (previous.domain != compiled.domain
                || previous.environment_id != compiled.environment_id)
        {
            return Err(ErrorData::invalid_params(
                "MCP session belongs to another deployment domain",
                None,
            ));
        }
        let (router, resources, prompts) = self.build_capabilities(&compiled).await;
        let mut tools = router.list_all();
        for export in &compiled.tools {
            tools.push(invoke::native_tool::tool_metadata(export)?);
        }
        self.tools.clear();
        for tool in &tools {
            self.tools.insert(tool.name.to_string(), tool.clone());
        }
        let mut resource_registry = ResourceRegistry::default();
        for resource in resources {
            resource_registry.insert(resource);
        }
        let mut prompt_registry = PromptRegistry::default();
        for prompt in prompts {
            prompt_registry.insert(prompt);
        }
        *self.resources.write().await = resource_registry;
        *self.prompts.write().await = prompt_registry;
        *self.tool_router.write().await = Some(router);
        *pinned = Some(compiled);
        Ok(tools)
    }

    async fn require_pinned_deployment(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Arc<CompiledMcp>, ErrorData> {
        let authenticated = authenticated_deployment(context)?;
        let compiled = self
            .deployment
            .read()
            .await
            .clone()
            .ok_or_else(changed_tool_set)?;
        if !same_deployment(&compiled, &authenticated) {
            let _ = context.peer.notify_tool_list_changed().await;
            return Err(changed_tool_set());
        }
        Ok(compiled)
    }
}

fn authenticated_deployment(
    context: &RequestContext<RoleServer>,
) -> Result<Arc<CompiledMcp>, ErrorData> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Arc<CompiledMcp>>())
        .cloned()
        .ok_or_else(|| {
            ErrorData::invalid_params("MCP request has no authenticated deployment", None)
        })
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

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).map(|ref_multi| ref_multi.clone())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self
            .refresh_tools(authenticated_deployment(&context)?)
            .await?;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let compiled = self.require_pinned_deployment(&context).await?;
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
        let tool_router = self.tool_router.read().await;
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        if let Some(tool_router) = tool_router.as_ref() {
            tool_router.call(tcc).await
        } else {
            Err(McpError::invalid_params(
                "tool router not initialized",
                None,
            ))
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        self.refresh_tools(authenticated_deployment(&context)?)
            .await?;
        let registry = self.resources.read().await;
        let resource_list = registry.list_static_resources();

        tracing::info!("Listing {} static resources", resource_list.len());

        Ok(ListResourcesResult::with_all_items(resource_list))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        self.refresh_tools(authenticated_deployment(&context)?)
            .await?;
        let registry = self.resources.read().await;
        let resource_templates = registry.list_resource_templates();

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
        self.refresh_tools(authenticated_deployment(&context)?)
            .await?;
        let registry = self.prompts.read().await;
        let prompt_list = registry.list_prompts();

        tracing::info!("Listing {} prompts", prompt_list.len());

        Ok(ListPromptsResult::with_all_items(prompt_list))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        self.require_pinned_deployment(&context).await?;
        let registry = self.prompts.read().await;

        registry
            .get_by_name(&request.name)
            .map(|p| p.get_prompt_result().into())
            .ok_or_else(|| {
                McpError::invalid_params(format!("Prompt not found: {}", request.name), None)
            })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParams { uri, .. }: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        self.require_pinned_deployment(&context).await?;
        let resource_registry = self.resources.read().await;

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

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.refresh_tools(authenticated_deployment(&context)?)
            .await?;
        Ok(self.get_info())
    }
}
