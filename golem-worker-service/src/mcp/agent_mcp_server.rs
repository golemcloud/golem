// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::mcp::McpCapabilityLookup;
use crate::mcp::agent_mcp_capability::McpAgentCapability;
use crate::mcp::agent_mcp_resource::{AgentMcpResource, AgentMcpResourceKind, ResourceUri};
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::invoke::{agent_invoke, resource_invoke};
use crate::service::worker::WorkerService;
use dashmap::DashMap;
use golem_common::base_model::domain_registration::Domain;
use poem::http;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::router::tool::ToolRouter,
    model::*, service::RequestContext, task_handler, task_manager::OperationProcessor,
};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

// Every client will get an instance of this
#[derive(Clone)]
pub struct GolemAgentMcpServer {
    processor: Arc<Mutex<OperationProcessor>>,
    tool_router: Arc<RwLock<Option<ToolRouter<GolemAgentMcpServer>>>>,
    tools: Arc<DashMap<String, Tool>>,
    static_resources: Arc<DashMap<ResourceUri, AgentMcpResource>>,
    template_resources: Arc<RwLock<Vec<AgentMcpResource>>>,
    domain: Arc<RwLock<Option<Domain>>>,
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
            static_resources: Arc::new(DashMap::new()),
            template_resources: Arc::new(RwLock::new(Vec::new())),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
            domain: Arc::new(RwLock::new(None)),
            mcp_definitions_lookup,
            worker_service,
        }
    }

    pub async fn invoke(
        &self,
        args_map: JsonObject,
        mcp_tool: &AgentMcpTool,
    ) -> Result<CallToolResult, ErrorData> {
        agent_invoke(&self.worker_service, args_map, mcp_tool).await
    }

    async fn build_capabilities(
        &self,
        domain: &Domain,
    ) -> (ToolRouter<GolemAgentMcpServer>, Vec<AgentMcpResource>) {
        let capabilities = get_agent_capabilities(domain, &self.mcp_definitions_lookup).await;

        let mut router = ToolRouter::<GolemAgentMcpServer>::new();

        for tool in capabilities.tools {
            router = router.with_route(tool);
        }

        (router, capabilities.resources)
    }
}

pub struct AgentCapabilities {
    pub tools: Vec<AgentMcpTool>,
    pub resources: Vec<AgentMcpResource>,
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
            };
        }
    };

    let mut tools = vec![];
    let mut resources = vec![];

    let account_id = compiled_mcp.account_id;
    let environment_id = compiled_mcp.environment_id;

    let agent_types = compiled_mcp.agent_types();

    tracing::info!(
        "Found {} agent types for domain {}: {:?}",
        agent_types.len(),
        domain.0,
        agent_types
            .iter()
            .map(|at| at.0.clone())
            .collect::<Vec<_>>()
    );

    for agent_type_name in &agent_types {
        match mcp_definition_lookup
            .resolve_agent_type(domain, agent_type_name)
            .await
        {
            Ok(registered_agent_type) => {
                tracing::debug!(
                    "Resolved agent type {} for domain {}: implemented by component {}, methods: {:?}",
                    agent_type_name.0,
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
                for method in &agent_type.methods {
                    let agent_method_mcp = McpAgentCapability::from(
                        &account_id,
                        &environment_id,
                        &agent_type.type_name,
                        method,
                        &agent_type.constructor,
                        component_id,
                    );

                    match agent_method_mcp {
                        McpAgentCapability::Tool(agent_mcp_tool) => {
                            tools.push(*agent_mcp_tool);
                        }
                        McpAgentCapability::Resource(agent_mcp_resource) => {
                            resources.push(agent_mcp_resource);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(
                    "Failed to resolve agent type {} for domain {}: {}",
                    agent_type_name.0,
                    domain.0,
                    e
                );
            }
        }
    }

    tracing::info!(
        "Found {} tools and {} resources for domain {}",
        tools.len(),
        resources.len(),
        domain.0
    );

    AgentCapabilities { tools, resources }
}

#[allow(deprecated)]
#[task_handler]
impl ServerHandler for GolemAgentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            // This is not the latest,
            // ProtocolVersion::V_2025_06_18 is the latest, however RMCP
            // is not widely tested with this version as per comments
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("This server provides  tools related to agent in golem and prompts. Tools: increment, decrement, get_value, say_hello, echo, sum. Prompts: example_prompt (takes a message), counter_analysis (analyzes counter state with a goal).".to_string()),
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).map(|ref_multi| ref_multi.clone())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tool_router = self.tool_router.read().await;

        if let Some(tool_router) = tool_router.as_ref() {
            tracing::info!("Listing tools: {:?}", tool_router.list_all());

            Ok(ListToolsResult {
                tools: tool_router.list_all(),
                meta: Some(Meta(object(::serde_json::Value::Object({
                    let mut object = ::serde_json::Map::new();
                    let _ = object.insert(
                        ("tool_meta_key").into(),
                        ::serde_json::to_value("tool_meta_value").unwrap(),
                    );
                    object
                })))),
                next_cursor: None,
            })
        } else {
            Err(McpError::invalid_params(
                "tool router not initialized",
                None,
            ))
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
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
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resource_list: Vec<Resource> = self
            .static_resources
            .iter()
            .filter_map(|entry| match &entry.value().kind {
                AgentMcpResourceKind::Static(resource) => Some(resource.clone()),
                AgentMcpResourceKind::Template { .. } => None,
            })
            .collect();

        tracing::info!("Listing {} static resources", resource_list.len());

        Ok(ListResourcesResult {
            resources: resource_list,
            next_cursor: None,
            meta: None,
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let templates = self.template_resources.read().await;

        let resource_templates: Vec<ResourceTemplate> = templates
            .iter()
            .filter_map(|r| match &r.kind {
                AgentMcpResourceKind::Template { template, .. } => Some(template.clone()),
                AgentMcpResourceKind::Static(_) => None,
            })
            .collect();

        tracing::info!("Listing {} resource templates", resource_templates.len());

        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParams { meta: _, uri }: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        if let Some(entry) = self.static_resources.get(&uri) {
            return resource_invoke(&self.worker_service, entry.value(), &uri, None).await;
        }

        let templates = self.template_resources.read().await;
        for resource in templates.iter() {
            if let AgentMcpResourceKind::Template {
                template,
                constructor_param_names: _,
            } = &resource.kind
            {
                if let Ok(params) =
                    AgentMcpResource::extract_params_from_uri(&template.uri_template, &uri)
                {
                    return resource_invoke(&self.worker_service, resource, &uri, Some(params))
                        .await;
                }
            }
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
        if let Some(parts) = context.extensions.get::<http::request::Parts>() {
            tracing::info!(
                version = ?parts.version,
                method = ?parts.method,
                uri = %parts.uri,
                headers = ?parts.headers,
                "initialize from http server"
            );

            if let Some(session_header) = parts.headers.get("mcp-session-id") {
                tracing::info!(
                    "Session ID from header: {}",
                    session_header.to_str().unwrap_or("invalid session id")
                );
            } else {
                tracing::info!("No session ID found in headers");
            }

            if let Some(host) = parts.headers.get("host") {
                let domain = Domain(host.to_str().unwrap().to_string());
                let (tool_router, agent_resources) = self.build_capabilities(&domain).await;
                for tool in tool_router.list_all() {
                    self.tools.insert(tool.name.to_string(), tool);
                }
                let mut template_resources = self.template_resources.write().await;
                for resource in agent_resources {
                    match &resource.kind {
                        AgentMcpResourceKind::Static(res) => {
                            self.static_resources.insert(res.uri.clone(), resource);
                        }
                        AgentMcpResourceKind::Template { .. } => {
                            template_resources.push(resource);
                        }
                    }
                }
                *self.domain.write().await = Some(domain);
                *self.tool_router.write().await = Some(tool_router);
            }
        }

        Ok(self.get_info())
    }
}
