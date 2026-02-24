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
use crate::mcp::agent_mcp_prompt::AgentMcpPrompt;
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use dashmap::DashMap;
use golem_common::base_model::agent::{
    AgentId, AgentMethod, AgentTypeName, ComponentModelElementSchema, DataSchema, ElementSchema,
    NamedElementSchemas,
};
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::agent::NamedElementSchema;
use golem_wasm::analysis::analysed_type::u32;
use poem::http;
use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, handler::server::router::tool::ToolRouter,
    model::*, service::RequestContext, task_handler, task_manager::OperationProcessor,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

// Every client will get an instance of this
#[derive(Clone)]
pub struct GolemAgentMcpServer {
    processor: Arc<Mutex<OperationProcessor>>,
    tool_router: Arc<RwLock<Option<ToolRouter<GolemAgentMcpServer>>>>,
    tools: Arc<DashMap<String, Tool>>,
    domain: Arc<RwLock<Option<Domain>>>,
    agent_id: Option<AgentId>,
    mcp_definitions_lookup: Arc<dyn McpCapabilityLookup + Send + Sync + 'static>,
}

impl GolemAgentMcpServer {
    pub fn new(
        agent_id: Option<AgentId>,
        mcp_definitions_lookup: Arc<dyn McpCapabilityLookup + Send + Sync + 'static>,
    ) -> Self {
        Self {
            tool_router: Arc::new(RwLock::new(None)),
            tools: Arc::new(DashMap::new()),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
            domain: Arc::new(RwLock::new(None)),
            agent_id,
            mcp_definitions_lookup,
        }
    }

    async fn tool_router(&self, domain: &Domain) -> ToolRouter<GolemAgentMcpServer> {
        let tool_handlers =
            get_agent_tool_and_handlers(&self.agent_id, domain, &self.mcp_definitions_lookup).await;

        let mut router = ToolRouter::<Self>::new();

        for tool in tool_handlers {
            router = router.with_route(tool);
        }

        router
    }

    fn prompt_router(agent_id: Option<AgentId>) -> PromptRouter<GolemAgentMcpServer> {
        let prompt_handlers = get_agent_prompt_and_handlers(agent_id);

        let mut router = PromptRouter::<Self>::new();

        for agent_mcp_prompt in prompt_handlers {
            router = router.with_route(agent_mcp_prompt);
        }

        router
    }
}

pub fn get_agent_prompt_and_handlers(agent_id: Option<AgentId>) -> Vec<AgentMcpPrompt> {
    // similar to get_agent_tool_and_handlers, but for prompts
    // prompt name is `get_${method_name}_prompt`
    vec![]
}

pub async fn get_agent_tool_and_handlers(
    _agent_id: &Option<AgentId>,
    domain: &Domain,
    mcp_definition_lookup: &Arc<dyn McpCapabilityLookup + Send + Sync + 'static>,
) -> Vec<AgentMcpTool> {
    let compiled_mcp = match mcp_definition_lookup.get(domain).await {
        Ok(mcp) => mcp,
        Err(e) => {
            tracing::error!("Failed to get compiled MCP for domain {}: {}", domain.0, e);
            return vec![];
        }
    };

    let mut tools = vec![];

    for agent_type_name in compiled_mcp.agent_types() {
        match mcp_definition_lookup
            .resolve_agent_type(domain, &agent_type_name)
            .await
        {
            Ok(registered_agent_type) => {
                let agent_type = &registered_agent_type.agent_type;
                for method in &agent_type.methods {
                    let agent_method_mcp =
                        McpAgentCapability::from(&agent_type.type_name, method, &agent_type.constructor);

                    match agent_method_mcp {
                        McpAgentCapability::Tool(agent_mcp_tool) => {
                            tools.push(agent_mcp_tool);
                        }
                        McpAgentCapability::Resource(_) => {}
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

    tools
}

#[task_handler]
impl ServerHandler for GolemAgentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
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
                        ::serde_json::to_value(&"tool_meta_value").unwrap(),
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

    async fn read_resource(
        &self,
        ReadResourceRequestParams { meta: _, uri }: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        todo!("Resource support is not implemented yet. URI: {}", uri)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
            meta: None,
        })
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
                let tool_router = self.tool_router(&domain).await;
                for tool in tool_router.list_all() {
                    self.tools.insert(tool.name.to_string(), tool);
                }
                *self.domain.write().await = Some(domain);
                *self.tool_router.write().await = Some(tool_router);
            }
        }

        Ok(self.get_info())
    }
}
