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
use anyhow::{Context, Result};
use rmcp::{
    schemars::JsonSchema,
    CallToolError, CallToolRequest, CallToolResult, ListToolsResult, RpcError, Tool,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{debug, error, info};

// Tool definitions using #[mcp_tool] macro
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "execute_golem_command",
    title = "Execute Golem Command",
    description = "Execute a Golem CLI command with specified arguments"
)]
pub struct ExecuteGolemCommandTool {
    /// The Golem CLI command to execute
    pub command: String,
    /// Arguments for the command
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "list_components",
    title = "List Components",
    description = "List all Golem components"
)]
pub struct ListComponentsTool {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "get_component_info",
    title = "Get Component Info",
    description = "Get detailed information about a Golem component"
)]
pub struct GetComponentInfoTool {
    /// The name of the component
    pub component_name: String,
    /// Optional component version
    pub version: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "list_agents",
    title = "List Agents",
    description = "List all Golem agents"
)]
pub struct ListAgentsTool {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "get_agent_info",
    title = "Get Agent Info",
    description = "Get detailed information about a Golem agent"
)]
pub struct GetAgentInfoTool {
    /// The name of the agent
    pub agent_name: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "list_apps",
    title = "List Apps",
    description = "List all Golem apps"
)]
pub struct ListAppsTool {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "get_app_info",
    title = "Get App Info",
    description = "Get detailed information about a Golem app"
)]
pub struct GetAppInfoTool {
    /// The name of the app
    pub app_name: String,
}

// Generate tool box enum for automatic tool dispatch
rmcp::tool_box!(GolemTools, [
    ExecuteGolemCommandTool,
    ListComponentsTool,
    GetComponentInfoTool,
    ListAgentsTool,
    GetAgentInfoTool,
    ListAppsTool,
    GetAppInfoTool
]);

pub struct GolemToolHandler {
    ctx: Arc<Context>,
}

impl GolemToolHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn list_tools(&self) -> Vec<Tool> {
        GolemTools::tools()
    }

    pub async fn handle_call_tool_request(
        &self,
        request: &CallToolRequest,
    ) -> Result<CallToolResult, CallToolError> {
        match GolemTools::try_from(request.clone()) {
            Ok(tool) => match tool {
                GolemTools::ExecuteGolemCommandTool(args) => {
                    self.execute_golem_command(&args).await
                }
                GolemTools::ListComponentsTool(_args) => {
                    self.list_components().await
                }
                GolemTools::GetComponentInfoTool(args) => {
                    self.get_component_info(&args).await
                }
                GolemTools::ListAgentsTool(_args) => {
                    self.list_agents().await
                }
                GolemTools::GetAgentInfoTool(args) => {
                    self.get_agent_info(&args).await
                }
                GolemTools::ListAppsTool(_args) => {
                    self.list_apps().await
                }
                GolemTools::GetAppInfoTool(args) => {
                    self.get_app_info(&args).await
                }
            },
            Err(_) => Err(CallToolError::unknown_tool(request.name.clone())),
        }
    }

    async fn execute_golem_command(
        &self,
        args: &ExecuteGolemCommandTool,
    ) -> Result<CallToolResult, CallToolError> {
        info!("Executing Golem command: {} with args: {:?}", args.command, args.args);

        // Build the command
        let mut cmd = Command::new("golem-cli");
        cmd.arg(&args.command);
        cmd.args(&args.args);

        // Execute the command
        let output = cmd
            .output()
            .await
            .context("Failed to execute golem-cli command")
            .map_err(|e| CallToolError::internal_error(format!("Command execution failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let result_text = if output.status.success() {
            format!("Command executed successfully:\n{}", stdout)
        } else {
            format!("Command failed with exit code {}:\nStdout:\n{}\nStderr:\n{}", 
                output.status.code().unwrap_or(-1), stdout, stderr)
        };

        Ok(CallToolResult::text_content(vec![result_text.into()]))
    }

    async fn list_components(&self) -> Result<CallToolResult, CallToolError> {
        debug!("Listing components");

        let mut cmd = Command::new("golem-cli");
        cmd.args(["component", "list"]);

        let output = cmd
            .output()
            .await
            .context("Failed to list components")
            .map_err(|e| CallToolError::internal_error(format!("Failed to list components: {}", e)))?;

        let result = String::from_utf8_lossy(&output.stdout);

        Ok(CallToolResult::text_content(vec![result.into()]))
    }

    async fn get_component_info(
        &self,
        args: &GetComponentInfoTool,
    ) -> Result<CallToolResult, CallToolError> {
        debug!("Getting component info for: {}", args.component_name);

        let mut cmd = Command::new("golem-cli");
        cmd.args(["component", "info", &args.component_name]);

        if let Some(version) = args.version {
            cmd.arg("--version");
            cmd.arg(version.to_string());
        }

        let output = cmd
            .output()
            .await
            .context("Failed to get component info")
            .map_err(|e| CallToolError::internal_error(format!("Failed to get component info: {}", e)))?;

        let result = String::from_utf8_lossy(&output.stdout);

        Ok(CallToolResult::text_content(vec![result.into()]))
    }

    async fn list_agents(&self) -> Result<CallToolResult, CallToolError> {
        debug!("Listing agents");

        let mut cmd = Command::new("golem-cli");
        cmd.args(["agent", "list"]);

        let output = cmd
            .output()
            .await
            .context("Failed to list agents")
            .map_err(|e| CallToolError::internal_error(format!("Failed to list agents: {}", e)))?;

        let result = String::from_utf8_lossy(&output.stdout);

        Ok(CallToolResult::text_content(vec![result.into()]))
    }

    async fn get_agent_info(
        &self,
        args: &GetAgentInfoTool,
    ) -> Result<CallToolResult, CallToolError> {
        debug!("Getting agent info for: {}", args.agent_name);

        let mut cmd = Command::new("golem-cli");
        cmd.args(["agent", "info", &args.agent_name]);

        let output = cmd
            .output()
            .await
            .context("Failed to get agent info")
            .map_err(|e| CallToolError::internal_error(format!("Failed to get agent info: {}", e)))?;

        let result = String::from_utf8_lossy(&output.stdout);

        Ok(CallToolResult::text_content(vec![result.into()]))
    }

    async fn list_apps(&self) -> Result<CallToolResult, CallToolError> {
        debug!("Listing apps");

        let mut cmd = Command::new("golem-cli");
        cmd.args(["app", "list"]);

        let output = cmd
            .output()
            .await
            .context("Failed to list apps")
            .map_err(|e| CallToolError::internal_error(format!("Failed to list apps: {}", e)))?;

        let result = String::from_utf8_lossy(&output.stdout);

        Ok(CallToolResult::text_content(vec![result.into()]))
    }

    async fn get_app_info(
        &self,
        args: &GetAppInfoTool,
    ) -> Result<CallToolResult, CallToolError> {
        debug!("Getting app info for: {}", args.app_name);

        let mut cmd = Command::new("golem-cli");
        cmd.args(["app", "info", &args.app_name]);

        let output = cmd
            .output()
            .await
            .context("Failed to get app info")
            .map_err(|e| CallToolError::internal_error(format!("Failed to get app info: {}", e)))?;

        let result = String::from_utf8_lossy(&output.stdout);

        Ok(CallToolResult::text_content(vec![result.into()]))
    }
}
