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

use crate::command::GolemCliCommand;
use crate::mcp::resources;
use crate::mcp::tools;
use crate::model::cli_command_metadata::CliCommandMetadata;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::json;
use std::process::Command;
use tracing::debug;

/// The MCP server handler for Golem CLI.
/// Exposes CLI commands as tools and manifest files as resources.
#[derive(Clone)]
pub struct GolemMcpHandler {
    metadata: CliCommandMetadata,
    tools: Vec<Tool>,
}

impl GolemMcpHandler {
    pub fn new() -> Self {
        let metadata = GolemCliCommand::collect_metadata();
        let tools = tools::cli_metadata_to_tools(&metadata);
        debug!("Registered {} MCP tools", tools.len());
        Self { metadata, tools }
    }
}

impl ServerHandler for GolemMcpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "golem-cli".into(),
                title: Some("Golem CLI MCP Server".into()),
                version: crate::version().into(),
                description: Some("MCP Server exposing Golem CLI commands as tools and manifest files as resources".into()),
                icons: None,
                website_url: Some("https://golem.cloud".into()),
            },
            instructions: Some(
                "This MCP server exposes Golem CLI commands as tools. \
                 Each tool corresponds to a CLI command. Tool names use dot-separated paths \
                 (e.g. 'component.new', 'agent.invoke'). \
                 Manifest files (golem.yaml) in the project are available as resources."
                    .to_string(),
            ),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tools.clone(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.as_ref();
        let params = match &request.arguments {
            Some(args) => args.clone(),
            None => serde_json::Map::new(),
        };

        debug!("Calling tool: {} with params: {:?}", tool_name, params);

        // Build CLI arguments from the tool call
        let mut cli_args = tools::tool_call_to_cli_args(tool_name, &params, &self.metadata);

        // Default to JSON format for machine-readable output unless explicitly set
        if !cli_args.iter().any(|a| a == "--format" || a == "-F") {
            cli_args.insert(0, "--format".to_string());
            cli_args.insert(1, "json".to_string());
        }

        // Execute the CLI command as a subprocess
        let binary_path = std::env::current_exe().map_err(|e| {
            McpError::internal_error(
                format!("Failed to determine CLI binary path: {}", e),
                None,
            )
        })?;

        debug!(
            "Executing: {} {}",
            binary_path.display(),
            cli_args.join(" ")
        );

        let output = Command::new(&binary_path)
            .args(&cli_args)
            .output()
            .map_err(|e| {
                McpError::internal_error(format!("Failed to execute CLI command: {}", e), None)
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let is_error = !output.status.success();
        let mut content = Vec::new();

        if !stdout.is_empty() {
            content.push(Content::text(stdout));
        }
        if !stderr.is_empty() {
            content.push(Content::text(format!("[stderr] {}", stderr)));
        }
        if content.is_empty() {
            content.push(Content::text(if is_error {
                format!(
                    "Command failed with exit code: {}",
                    output.status.code().unwrap_or(-1)
                )
            } else {
                "Command completed successfully (no output)".to_string()
            }));
        }

        Ok(CallToolResult {
            content,
            structured_content: None,
            is_error: Some(is_error),
            meta: None,
        })
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let manifest_resources = resources::discover_manifest_resources();
        Ok(ListResourcesResult {
            resources: manifest_resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();

        match resources::read_manifest_resource(uri) {
            Ok(contents) => Ok(ReadResourceResult { contents }),
            Err(e) => Err(McpError::resource_not_found(
                e,
                Some(json!({ "uri": uri })),
            )),
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
            meta: None,
        })
    }
}
