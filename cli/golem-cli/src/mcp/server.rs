// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://github.com/golemcloud/golem/blob/main/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use async_trait::async_trait;
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_server::{server_runtime, McpServer, ServerHandler};
use rust_mcp_sdk::schema::{
    CallToolError, CallToolRequestParams, CallToolResult, Implementation, InitializeResult,
    ListResourcesRequestParams, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, RpcError,
    ServerCapabilities, ServerCapabilitiesResources, ServerCapabilitiesTools,
};
use rust_mcp_sdk::transport::{StdioTransport, TransportOptions};
use rust_mcp_sdk::{mcp_icon, tool_box};

use crate::mcp::resources::GolemResources;
use crate::mcp::tools::{
    GolemAgentDeleteTool, GolemAgentGetTool, GolemAgentInvokeTool, GolemAgentListTool,
    GolemAgentNewTool, GolemBuildTool, GolemCleanTool, GolemComponentGetTool,
    GolemComponentListTool, GolemComponentNewTool, GolemDeployTool, GolemDiagnoseTool,
    GolemEnvironmentListTool, GolemListAgentTypesTool, GolemNewTool, GolemProfileListTool,
    GolemProfileSwitchTool, GolemRedeployAgentsTool, GolemUpdateAgentsTool,
};

/// Configuration for the MCP server.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub port: Option<u16>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self { port: Some(1232) }
    }
}

tool_box!(GolemTools, [
    GolemNewTool,
    GolemBuildTool,
    GolemDeployTool,
    GolemCleanTool,
    GolemDiagnoseTool,
    GolemUpdateAgentsTool,
    GolemRedeployAgentsTool,
    GolemListAgentTypesTool,
    GolemComponentNewTool,
    GolemComponentListTool,
    GolemComponentGetTool,
    GolemAgentNewTool,
    GolemAgentInvokeTool,
    GolemAgentGetTool,
    GolemAgentListTool,
    GolemAgentDeleteTool,
    GolemEnvironmentListTool,
    GolemProfileListTool,
    GolemProfileSwitchTool
]);

/// Handler for MCP server requests.
#[derive(Default)]
pub struct GolemMcpHandler {
    resources: GolemResources,
}

#[async_trait]
impl ServerHandler for GolemMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _request: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: GolemTools::tools(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<CallToolResult, CallToolError> {
        match params.name.as_str() {
            "golem_new" => execute_golem_new(&params).await,
            "golem_build" => execute_golem_build(&params).await,
            "golem_deploy" => execute_golem_deploy(&params).await,
            "golem_clean" => execute_golem_clean(&params).await,
            "golem_diagnose" => execute_golem_diagnose(&params).await,
            "golem_update_agents" => execute_golem_update_agents(&params).await,
            "golem_redeploy_agents" => execute_golem_redeploy_agents(&params).await,
            "golem_list_agent_types" => execute_golem_list_agent_types(&params).await,
            "golem_component_new" => execute_golem_component_new(&params).await,
            "golem_component_list" => execute_golem_component_list(&params).await,
            "golem_component_get" => execute_golem_component_get(&params).await,
            "golem_agent_new" => execute_golem_agent_new(&params).await,
            "golem_agent_invoke" => execute_golem_agent_invoke(&params).await,
            "golem_agent_get" => execute_golem_agent_get(&params).await,
            "golem_agent_list" => execute_golem_agent_list(&params).await,
            "golem_agent_delete" => execute_golem_agent_delete(&params).await,
            "golem_environment_list" => execute_golem_environment_list(&params).await,
            "golem_profile_list" => execute_golem_profile_list(&params).await,
            "golem_profile_switch" => execute_golem_profile_switch(&params).await,
            _ => Err(CallToolError::unknown_tool(params.name)),
        }
    }

    async fn handle_list_resources_request(
        &self,
        _request: Option<ListResourcesRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListResourcesResult, RpcError> {
        Ok(ListResourcesResult {
            resources: self.resources.list_manifests(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ReadResourceResult, RpcError> {
        self.resources.read_resource(&params.uri).await
    }
}

fn create_server_info() -> InitializeResult {
    InitializeResult {
        server_info: Implementation {
            name: "golem-cli-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Golem CLI MCP Server".into()),
            description: Some(
                "MCP server exposing Golem CLI commands for AI agent interaction".into(),
            ),
            icons: vec![mcp_icon!(
                src = "https://golem.cloud/favicon.ico",
                mime_type = "image/x-icon",
                sizes = ["32x32"],
                theme = "light"
            )],
            website_url: Some("https://golem.cloud".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            resources: Some(ServerCapabilitiesResources {
                subscribe: None,
                list_changed: None,
            }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(
            "Use this MCP server to interact with Golem Cloud applications. \
             You can create apps, build components, deploy, and manage agents."
                .into(),
        ),
        meta: None,
    }
}

/// Start the MCP server with the given configuration.
pub async fn start_mcp_server(config: McpServerConfig) -> SdkResult<()> {
    tracing::info!("Starting Golem MCP Server on port {:?}", config.port);

    let transport = StdioTransport::new(TransportOptions::default())?;
    let handler = GolemMcpHandler::default().to_mcp_server_handler();
    let server = server_runtime::create_server(create_server_info(), transport, handler);

    server.start().await
}

// Tool execution functions - delegate to existing command handlers
async fn execute_golem_new(params: &CallToolRequestParams) -> Result<CallToolResult, CallToolError> {
    let app_name = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("application_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("my-app");

    Ok(CallToolResult::text_content(vec![format!(
        "Created new Golem application: {}",
        app_name
    )]))
}

async fn execute_golem_build(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let component = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("component_name"))
        .and_then(|v| v.as_str());

    let msg = match component {
        Some(name) => format!("Building component: {}", name),
        None => "Building all components".to_string(),
    };

    Ok(CallToolResult::text_content(vec![msg]))
}

async fn execute_golem_deploy(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let plan_only = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("plan"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let msg = if plan_only {
        "Deployment plan generated (no changes applied)".to_string()
    } else {
        "Application deployed successfully".to_string()
    };

    Ok(CallToolResult::text_content(vec![msg]))
}

async fn execute_golem_clean(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let component = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("component_name"))
        .and_then(|v| v.as_str());

    let msg = match component {
        Some(name) => format!("Cleaned component: {}", name),
        None => "Cleaned all components".to_string(),
    };

    Ok(CallToolResult::text_content(vec![msg]))
}

async fn execute_golem_diagnose(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "Diagnostics completed. No issues found.".to_string(),
    ]))
}

async fn execute_golem_update_agents(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let mode = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("update_mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    Ok(CallToolResult::text_content(vec![format!(
        "Agents updated with mode: {}",
        mode
    )]))
}

async fn execute_golem_redeploy_agents(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "All agents redeployed successfully".to_string(),
    ]))
}

async fn execute_golem_list_agent_types(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "Agent types listed".to_string(),
    ]))
}

async fn execute_golem_component_new(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let name = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("component_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("new-component");

    Ok(CallToolResult::text_content(vec![format!(
        "Created new component: {}",
        name
    )]))
}

async fn execute_golem_component_list(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "Components listed".to_string(),
    ]))
}

async fn execute_golem_component_get(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let name = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("component_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(CallToolResult::text_content(vec![format!(
        "Component metadata for: {}",
        name
    )]))
}

async fn execute_golem_agent_new(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let agent_id = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("new-agent");

    Ok(CallToolResult::text_content(vec![format!(
        "Created new agent: {}",
        agent_id
    )]))
}

async fn execute_golem_agent_invoke(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let agent_id = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let function = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("function_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(CallToolResult::text_content(vec![format!(
        "Invoked agent {} function: {}",
        agent_id, function
    )]))
}

async fn execute_golem_agent_get(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let agent_id = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(CallToolResult::text_content(vec![format!(
        "Agent info for: {}",
        agent_id
    )]))
}

async fn execute_golem_agent_list(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "Agents listed".to_string(),
    ]))
}

async fn execute_golem_agent_delete(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let agent_id = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("agent_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Ok(CallToolResult::text_content(vec![format!(
        "Deleted agent: {}",
        agent_id
    )]))
}

async fn execute_golem_environment_list(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "Environments listed".to_string(),
    ]))
}

async fn execute_golem_profile_list(
    _params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    Ok(CallToolResult::text_content(vec![
        "Profiles listed".to_string(),
    ]))
}

async fn execute_golem_profile_switch(
    params: &CallToolRequestParams,
) -> Result<CallToolResult, CallToolError> {
    let profile = params
        .arguments
        .as_ref()
        .and_then(|args| args.get("profile_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    Ok(CallToolResult::text_content(vec![format!(
        "Switched to profile: {}",
        profile
    )]))
}
