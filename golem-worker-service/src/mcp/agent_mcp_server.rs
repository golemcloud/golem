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
use poem::http;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::router::tool::ToolRouter,
    model::*, service::RequestContext, task_handler, task_manager::OperationProcessor,
};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
pub struct GolemAgentMcpServer {
    processor: Arc<Mutex<OperationProcessor>>,
    tool_router: Arc<RwLock<Option<ToolRouter<GolemAgentMcpServer>>>>,
    tools: Arc<DashMap<String, Tool>>,
    resources: Arc<RwLock<ResourceRegistry>>,
    prompts: Arc<RwLock<PromptRegistry>>,
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
            resources: Arc::new(RwLock::new(ResourceRegistry::default())),
            prompts: Arc::new(RwLock::new(PromptRegistry::default())),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
            domain: Arc::new(RwLock::new(None)),
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
        domain: &Domain,
    ) -> (
        ToolRouter<GolemAgentMcpServer>,
        Vec<AgentMcpResource>,
        Vec<AgentMcpPrompt>,
    ) {
        let capabilities = get_agent_capabilities(domain, &self.mcp_definitions_lookup).await;

        let mut router = ToolRouter::<GolemAgentMcpServer>::new();

        for tool in capabilities.tools {
            router = router.with_route(tool);
        }

        (router, capabilities.resources, capabilities.prompts)
    }
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

    let mut tools = vec![];
    let mut resources = vec![];
    let mut prompts = vec![];

    let account_id = compiled_mcp.account_id;
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

        if let Some(prompt_hint) = &agent_type.constructor.prompt_hint {
            prompts.push(AgentMcpPrompt::from_constructor_hint(
                &agent_type.type_name,
                &agent_type.description,
                prompt_hint,
            ));
        }

        for method in &agent_type.methods {
            if let Some(prompt_hint) = &method.prompt_hint {
                prompts.push(AgentMcpPrompt::from_method_hint(
                    &agent_type.type_name,
                    method,
                    &agent_type.constructor,
                    prompt_hint,
                ));
            }

            let agent_method_mcp = McpAgentCapability::from_agent_method(
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
                    resources.push(*agent_mcp_resource);
                }
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
        let registry = self.resources.read().await;
        let resource_list = registry.list_static_resources();

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
        let registry = self.resources.read().await;
        let resource_templates = registry.list_resource_templates();

        tracing::info!("Listing {} resource templates", resource_templates.len());

        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates,
            meta: None,
        })
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let registry = self.prompts.read().await;
        let prompt_list = registry.list_prompts();

        tracing::info!("Listing {} prompts", prompt_list.len());

        Ok(ListPromptsResult {
            prompts: prompt_list,
            next_cursor: None,
            meta: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let registry = self.prompts.read().await;

        registry
            .get_by_name(&request.name)
            .map(|p| p.get_prompt_result())
            .ok_or_else(|| {
                McpError::invalid_params(format!("Prompt not found: {}", request.name), None)
            })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParams { meta: _, uri }: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let resource_registry = self.resources.read().await;

        if let Some(resource) = resource_registry.get_static(&uri) {
            return invoke::resource::invoke_resource(&self.worker_service, resource, &uri, None)
                .await;
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
            .await;
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
                let (router, agent_resources, agent_prompts) =
                    self.build_capabilities(&domain).await;

                for tool in router.list_all() {
                    self.tools.insert(tool.name.to_string(), tool);
                }

                let mut resources = self.resources.write().await;
                for resource in agent_resources {
                    resources.insert(resource);
                }

                let mut prompts = self.prompts.write().await;
                for prompt in agent_prompts {
                    prompts.insert(prompt);
                }

                *self.domain.write().await = Some(domain);
                *self.tool_router.write().await = Some(router);
            }
        }

        Ok(self.get_info())
    }
}
