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
    InitializeResult, Implementation, ServerCapabilities, ServerCapabilitiesTools,
    ServiceExt, transport::StdioTransport, TransportOptions,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

mod tools;
mod resources;

use tools::GolemToolHandler;
use resources::GolemResources;

pub struct GolemMcpServer {
    ctx: Arc<Context>,
}

impl GolemMcpServer {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting Golem MCP Server");
        
        let server_details = InitializeResult {
            server_info: Implementation {
                name: "Golem CLI MCP Server".to_string(),
                version: "0.1.0".to_string(),
                title: Some("Golem CLI MCP Server".to_string()),
            },
            capabilities: ServerCapabilities {
                tools: Some(ServerCapabilitiesTools {
                    list_changed: None,
                }),
                ..Default::default()
            },
            meta: None,
            instructions: Some("Golem CLI MCP Server provides access to Golem CLI commands and manifest files".to_string()),
            protocol_version: rmcp::LATEST_PROTOCOL_VERSION.to_string(),
        };
        
        let transport = StdioTransport::new(TransportOptions::default())?;
        
        let handler = GolemMcpHandler::new(self.ctx.clone());
        
        let server = rmcp::server_runtime::create_server(server_details, transport, handler);
        
        server.start().await
    }
}

pub struct GolemMcpHandler {
    ctx: Arc<Context>,
    tools: GolemToolHandler,
    resources: GolemResources,
}

impl GolemMcpHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self {
            ctx,
            tools: GolemToolHandler::new(ctx.clone()),
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
        
        let tools = self.tools.list_tools();
        
        Ok(rmcp::ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        request: rmcp::CallToolRequest,
        _runtime: Arc<dyn rmcp::McpServer>,
    ) -> Result<rmcp::CallToolResult, rmcp::CallToolError> {
        debug!("Handling call tool request: {}", request.name);
        
        self.tools.handle_call_tool_request(&request).await
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
