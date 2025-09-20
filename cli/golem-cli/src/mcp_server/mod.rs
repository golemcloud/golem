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

use crate::context::Context;
use anyhow::Result;
use rmcp::{
    schemars::JsonSchema,
    ServiceExt,
    transport::{TokioChildProcess, ConfigureCommandExt},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{debug, info};

mod tools;
mod resources;

use tools::GolemTools;
use resources::GolemResources;

pub struct GolemMcpServer {
    ctx: Arc<Context>,
}

impl GolemMcpServer {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn start(&self, port: u16) -> Result<()> {
        info!("Starting Golem MCP Server on port {}", port);
        
        // Create the server handler
        let handler = GolemMcpHandler::new(self.ctx.clone());
        
        // For now, we'll use stdio transport as it's simpler to implement
        // In the future, we can add HTTP transport support
        let transport = (tokio::io::stdin(), tokio::io::stdout());
        
        let server = handler.serve(transport).await?;
        
        info!("Golem MCP Server running");
        
        // Wait for the server to finish
        let quit_reason = server.waiting().await?;
        info!("MCP Server stopped: {:?}", quit_reason);
        
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GolemCommandTool {
    /// The Golem CLI command to execute
    pub command: String,
    /// Arguments for the command
    #[serde(default)]
    pub args: Vec<String>,
}

#[rmcp::tool]
impl GolemCommandTool {
    pub fn tool() -> rmcp::Tool {
        rmcp::Tool {
            name: "execute_golem_command".to_string(),
            description: "Execute a Golem CLI command".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The Golem CLI command to execute (e.g., 'component', 'app', 'agent')"
                    },
                    "args": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        },
                        "description": "Arguments for the command"
                    }
                },
                "required": ["command"]
            }),
        }
    }
}

pub struct GolemMcpHandler {
    ctx: Arc<Context>,
    tools: GolemTools,
    resources: GolemResources,
}

impl GolemMcpHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self {
            ctx,
            tools: GolemTools::new(ctx.clone()),
            resources: GolemResources::new(ctx.clone()),
        }
    }
}

#[rmcp::server_handler]
impl rmcp::ServerHandler for GolemMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _request: rmcp::ListToolsRequest,
        _runtime: Arc<dyn rmcp::McpServer>,
    ) -> Result<rmcp::ListToolsResult, rmcp::RpcError> {
        debug!("Handling list tools request");
        
        let mut tools = vec![
            GolemCommandTool::tool(),
        ];
        
        // Add all tools from the tools module
        tools.extend(self.tools.list_tools());
        
        Ok(rmcp::ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        request: rmcp::CallToolRequest,
        runtime: Arc<dyn rmcp::McpServer>,
    ) -> Result<rmcp::CallToolResult, rmcp::CallToolError> {
        debug!("Handling call tool request: {}", request.name);
        
        match request.name.as_str() {
            "execute_golem_command" => {
                self.tools.execute_golem_command(&request).await
            }
            _ => {
                // Try to handle with tools module
                self.tools.handle_call_tool_request(&request, runtime).await
            }
        }
    }

    async fn handle_list_resources_request(
        &self,
        _request: rmcp::ListResourcesRequest,
        _runtime: Arc<dyn rmcp::McpServer>,
    ) -> Result<rmcp::ListResourcesResult, rmcp::RpcError> {
        debug!("Handling list resources request");
        
        let resources = self.resources.list_resources().await;
        
        Ok(rmcp::ListResourcesResult {
            resources,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        request: rmcp::ReadResourceRequest,
        _runtime: Arc<dyn rmcp::McpServer>,
    ) -> Result<rmcp::ReadResourceResult, rmcp::RpcError> {
        debug!("Handling read resource request: {}", request.uri);
        
        self.resources.read_resource(&request.uri).await
    }
}
