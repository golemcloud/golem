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

//! MCP Server implementation for the Golem CLI.
//!
//! This module provides an MCP server that exposes Golem CLI commands as MCP tools,
//! allowing AI agents (Claude Code, etc.) to interact with Golem via the Model Context Protocol.
//!
//! ## Usage
//!
//! ```bash
//! golem-cli server serve --port 8080
//! ```
//!
//! ## Available Tools
//!
//! - `golem_list_agents`: List all agent types
//! - `golem_invoke_agent`: Invoke a function on a Golem agent
//! - `golem_get_deployment`: Get deployment information
//! - `golem_health_check`: Check service health

use crate::context::Context;
use dashmap::DashMap;
use poem::endpoint::TowerCompatExt;
use poem::listener::{Listener, TcpListener};
use poem::middleware::Cors;
use poem::{EndpointExt, Route};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{
    handler::server::ServerHandler, model::*, service::RequestContext, task_handler,
    task_manager::OperationProcessor, ErrorData as McpError, RoleServer,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// The CLI MCP server.
///
/// Exposes Golem CLI commands as MCP tools for AI agents.
/// See issue #2679 for full implementation details.
#[derive(Clone)]
pub struct CliMcpServer {
    /// Task processor required by the #[task_handler] macro.
    processor: Arc<Mutex<OperationProcessor>>,
    /// Registered MCP tools.
    tools: DashMap<String, Tool>,
}

impl CliMcpServer {
    /// Create a new CLI MCP server.
    pub fn new() -> Self {
        let server = Self {
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
            tools: DashMap::new(),
        };
        server.register_tools();
        server
    }

    /// Register all available tools with the server.
    fn register_tools(&self) {
        use std::borrow::Cow;

        let tools = vec![
            Tool {
                name: Cow::Borrowed("golem_list_agents"),
                title: Some("List Golem Agent Types".to_string()),
                description: Some(Cow::Borrowed(
                    "List all available Golem agent types registered in the current environment. \
                     Returns agent type names and their descriptions.",
                )),
                input_schema: Arc::new(serde_json::Map::new().into()),
                output_schema: None,
                annotations: Some(ToolAnnotations {
                    title: Some("List Agent Types".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: None,
                    idempotent_hint: Some(true),
                    open_world_hint: Some(true),
                }),
                execution: None,
                icons: None,
                meta: None,
            },
            Tool {
                name: Cow::Borrowed("golem_invoke_agent"),
                title: Some("Invoke a Golem Agent".to_string()),
                description: Some(Cow::Borrowed(
                    "Invoke a function on a Golem agent. Requires agent type name, \
                     worker ID, and function name. This may modify agent state.",
                )),
                input_schema: Arc::new(
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "agent_type": {
                                "type": "string",
                                "description": "The agent type name to invoke"
                            },
                            "worker_id": {
                                "type": "string",
                                "description": "The worker ID to target"
                            },
                            "function": {
                                "type": "string",
                                "description": "The function name to call"
                            },
                            "arguments": {
                                "type": "object",
                                "description": "Function arguments as a JSON object"
                            }
                        },
                        "required": ["agent_type", "function"]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                output_schema: None,
                annotations: Some(ToolAnnotations {
                    title: Some("Invoke Agent".to_string()),
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(true),
                }),
                execution: None,
                icons: None,
                meta: None,
            },
            Tool {
                name: Cow::Borrowed("golem_get_deployment"),
                title: Some("Get Deployment Information".to_string()),
                description: Some(Cow::Borrowed(
                    "Get deployment information for a Golem worker or component. \
                     Returns version, status, and other deployment metadata.",
                )),
                input_schema: Arc::new(
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "worker_id": {
                                "type": "string",
                                "description": "The worker ID to query"
                            },
                            "component_id": {
                                "type": "string",
                                "description": "Optional component ID if worker ID is not available"
                            }
                        }
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                output_schema: None,
                annotations: Some(ToolAnnotations {
                    title: Some("Get Deployment Info".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: None,
                    idempotent_hint: Some(true),
                    open_world_hint: Some(true),
                }),
                execution: None,
                icons: None,
                meta: None,
            },
            Tool {
                name: Cow::Borrowed("golem_health_check"),
                title: Some("Check Golem Service Health".to_string()),
                description: Some(Cow::Borrowed(
                    "Check the health of the Golem service. Returns service status, \
                     version, and connectivity information.",
                )),
                input_schema: Arc::new(serde_json::Map::new().into()),
                output_schema: None,
                annotations: Some(ToolAnnotations {
                    title: Some("Health Check".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: None,
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                }),
                execution: None,
                icons: None,
                meta: None,
            },
        ];

        for tool in tools {
            self.tools.insert(tool.name.to_string(), tool);
        }
    }
}

impl Default for CliMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[task_handler]
impl ServerHandler for CliMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            // MCP 2025-11-25 requires the protocolVersion field to be set for client compatibility.
            // V_2025_06_18 is the latest rmcp version and is fully compatible with MCP 2025-11-25.
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Golem CLI MCP Server. Available tools: golem_list_agents, \
                 golem_invoke_agent, golem_get_deployment, golem_health_check."
                    .to_string(),
            ),
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).map(|entry| entry.clone())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools: Vec<Tool> = self
            .tools
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Tool invocation is a stub — full implementation requires Golem API client wiring.
        // See issue #2679 for the complete tool implementation.
        Err(McpError::method_not_found::<
            rmcp::model::CallToolRequestMethod,
        >())
    }
}

/// Start the MCP server on the specified port.
///
/// Wires up the rmcp `StreamableHttpService` with the `CliMcpServer` handler
/// and serves it over HTTP using Poem.
pub async fn run_mcp_server(_ctx: Arc<Context>, port: u16) -> anyhow::Result<()> {
    info!("Starting Golem CLI MCP Server on port {}", port);

    let server = CliMcpServer::new();

    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );

    let route = Route::new()
        .nest("/mcp", service.compat())
        .with(
            Cors::new()
                .allow_methods(vec!["GET", "POST", "DELETE", "OPTIONS"])
                .allow_headers(vec![
                    "Content-Type",
                    "Authorization",
                    "Mcp-Session-Id",
                    "Accept",
                    "Last-Event-ID",
                ])
                .expose_headers(vec!["Mcp-Session-Id"]),
        );

    let poem_listener = TcpListener::bind(format!("0.0.0.0:{}", port));
    let acceptor = poem_listener.into_acceptor().await?;

    info!(
        "Golem CLI MCP Server listening on http://0.0.0.0:{}",
        port
    );
    info!("MCP endpoint: http://0.0.0.0:{}/mcp", port);

    poem::Server::new_with_acceptor(acceptor)
        .run(route)
        .await?;

    Ok(())
}
