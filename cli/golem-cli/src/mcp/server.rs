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

use crate::mcp::resources::{discover_manifests, read_resource};
use crate::mcp::tools::{build_tool_definitions, dispatch_tool_call};
use async_trait::async_trait;
use rust_mcp_schema::schema_utils::CallToolError;
use rust_mcp_schema::{
    CallToolRequestParams, CallToolResult, Implementation, InitializeResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResult, RpcError, ServerCapabilities, ServerCapabilitiesResources,
    ServerCapabilitiesTools,
};
use rust_mcp_sdk::mcp_server::{hyper_server, HyperServerOptions, ServerHandler};
use rust_mcp_sdk::McpServer;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// The MCP server handler for golem-cli.
/// Exposes CLI commands as MCP tools and project manifests as MCP resources.
pub struct GolemMcpHandler {
    working_dir: PathBuf,
}

impl GolemMcpHandler {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }
}

#[async_trait]
impl ServerHandler for GolemMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListToolsResult, RpcError> {
        let tools = build_tool_definitions();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<CallToolResult, CallToolError> {
        let name = params.name.clone();
        let arguments = params.arguments.clone();
        dispatch_tool_call(&name, arguments, &self.working_dir).await
    }

    async fn handle_list_resources_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListResourcesResult, RpcError> {
        let resources = discover_manifests(&self.working_dir);
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ReadResourceResult, RpcError> {
        read_resource(&params.uri)
    }
}

/// Start the MCP server on the given port.
pub async fn start_mcp_server(port: u16) -> anyhow::Result<()> {
    let working_dir = std::env::current_dir()?;
    let handler = GolemMcpHandler::new(working_dir);

    let server_info = InitializeResult {
        server_info: Implementation {
            name: "golem-cli-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Golem CLI MCP Server".into()),
            description: Some(
                "Exposes golem-cli commands as MCP tools and project manifests as resources."
                    .into(),
            ),
            icons: vec![],
            website_url: Some("https://github.com/golemcloud/golem-cli".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools {
                list_changed: Some(false),
            }),
            resources: Some(ServerCapabilitiesResources {
                list_changed: Some(false),
                subscribe: Some(false),
            }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(
            "Golem CLI MCP Server — exposes golem-cli commands as MCP tools and project manifests as resources."
                .into(),
        ),
        meta: None,
    };

    info!("Starting Golem CLI MCP server on http://127.0.0.1:{port}");

    use rust_mcp_sdk::mcp_server::ToMcpServerHandler;
    let mcp_handler = handler.to_mcp_server_handler();

    let server = hyper_server::create_server(
        server_info,
        mcp_handler,
        HyperServerOptions {
            host: "127.0.0.1".to_string(),
            port,
            enable_json_response: Some(true),
            ..Default::default()
        },
    );

    server.start().await.map_err(|e| anyhow::anyhow!("{e}"))
}
