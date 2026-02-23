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

use std::borrow::Cow;
use std::sync::Arc;
use poem::http;
use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, service::RequestContext, task_handler,
    task_manager::OperationProcessor, tool_handler, ErrorData as McpError, RoleServer,
    ServerHandler,
};
use rmcp::handler::server::router::prompt::PromptRouter;
use serde_json::{json};
use tokio::sync::{Mutex};
use golem_common::base_model::agent::{AgentId, AgentMethod, AgentTypeName, ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchemas};
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::agent::NamedElementSchema;
use golem_wasm::analysis::analysed_type::u32;
use crate::mcp::agent_mcp_capability::McpAgentCapability;
use crate::mcp::agent_mcp_prompt::AgentMcpPrompt;
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::mcp_schema::{McpToolGetSchema};
use crate::mcp::McpCapabilityLookup;

#[derive(Clone)]
pub struct GolemAgentMcpServer {
    pub tool_router: ToolRouter<GolemAgentMcpServer>,
    pub processor: Arc<Mutex<OperationProcessor>>,
    pub domain: Arc<Mutex<Option<Domain>>>,
    agent_id: Option<AgentId>,
}

impl GolemAgentMcpServer {
    pub fn new(agent_id: Option<AgentId>) -> Self {
        Self {
            tool_router: Self::tool_router(agent_id.clone()),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
            domain: Arc::new(Mutex::new(None)),
            agent_id,
        }
    }

    fn tool_router(agent_id: Option<AgentId>) -> ToolRouter<GolemAgentMcpServer> {
        let tool_handlers = get_agent_tool_and_handlers(agent_id);

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

pub fn get_agent_tool_and_handlers(agent_id: Option<AgentId>) -> Vec<AgentMcpTool> {

    match agent_id {
        Some(agent) => {
            // just dummy,
            let agent_method = get_agent_methods(&agent.agent_type);

            let mut tools = vec![];

            for method in agent_method.into_iter() {
                let agent_method_mcp = McpAgentCapability::from(method);

                match agent_method_mcp {
                    McpAgentCapability::Tool(agent_mcp_tool) => {
                        tools.push((agent_mcp_tool));
                    }
                    McpAgentCapability::Resource(_) => {}
                }
            }

            tools
        },
        None => {
            let agent_method = get_agent_methods(&AgentTypeName("dummy_agent".into()));

            let mut tools = vec![];

            for method in agent_method.into_iter() {
                let agent_method_mcp = McpAgentCapability::from(method);

                match agent_method_mcp {
                    McpAgentCapability::Tool(agent_mcp_tool) => {
                        tools.push((agent_mcp_tool));
                    }
                    McpAgentCapability::Resource(_) => {}
                }
            }

            tools
        }
    }

}


pub fn get_agent_methods(_agent_id: &AgentTypeName) -> Vec<AgentMethod> {
    vec![
        AgentMethod {
            name: "increment".into(),
            description: "increment the number".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(
               NamedElementSchemas {
                   elements: vec![
                       NamedElementSchema {
                           name: "number".into(),
                           schema: ElementSchema::ComponentModel(
                               ComponentModelElementSchema {
                                   element_type: u32(),
                               }
                           )
                       }
                   ]
               }
            ),
            output_schema: DataSchema::Tuple(
                NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "result".into(),
                            schema: ElementSchema::ComponentModel(
                                ComponentModelElementSchema {
                                    element_type: u32(),
                                }
                            )
                        }
                    ]
                }
            ),
            http_endpoint: vec![],
        }
    ]
}

#[tool_handler(meta = Meta(rmcp::object!({"tool_meta_key": "tool_meta_value"})))]
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

    async fn read_resource(
        &self,
        ReadResourceRequestParams { meta: _, uri }: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match uri.as_str() {
            "str:////Users/to/some/path/" => {
                let cwd = "/Users/to/some/path/";
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(cwd, uri)],
                })
            }
            "memo://insights" => {
                let memo = "Business Intelligence Memo\n\nAnalysis has revealed 5 key insights ...";
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(memo, uri)],
                })
            }
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({
                    "uri": uri
                })),
            )),
        }
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
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        // Extract http::request::Parts (injected by rmcp's StreamableHttpService)
        if let Some(parts) = context.extensions.get::<http::request::Parts>() {
            tracing::info!(
                version = ?parts.version,
                method = ?parts.method,
                uri = %parts.uri,
                headers = ?parts.headers,
                "initialize from http server"
            );
        }

        Ok(self.get_info())
    }
}