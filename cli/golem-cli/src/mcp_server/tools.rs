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

pub struct GolemTools {
    ctx: Arc<Context>,
}

impl GolemTools {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn list_tools(&self) -> Vec<Tool> {
        vec![
            ComponentListTool::tool(),
            ComponentInfoTool::tool(),
            AgentListTool::tool(),
            AgentInfoTool::tool(),
            AppListTool::tool(),
            AppInfoTool::tool(),
        ]
    }

    pub async fn execute_golem_command(
        &self,
        request: &CallToolRequest,
    ) -> Result<CallToolResult, CallToolError> {
        let args: GolemCommandTool = serde_json::from_value(request.arguments.clone())
            .map_err(|e| CallToolError::invalid_params(format!("Invalid arguments: {}", e)))?;

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

    pub async fn handle_call_tool_request(
        &self,
        request: &CallToolRequest,
        _runtime: Arc<dyn rmcp::McpServer>,
    ) -> Result<CallToolResult, CallToolError> {
        match request.name.as_str() {
            "list_components" => self.list_components(request).await,
            "get_component_info" => self.get_component_info(request).await,
            "list_agents" => self.list_agents(request).await,
            "get_agent_info" => self.get_agent_info(request).await,
            "list_apps" => self.list_apps(request).await,
            "get_app_info" => self.get_app_info(request).await,
            _ => Err(CallToolError::unknown_tool(request.name.clone())),
        }
    }

    async fn list_components(&self, _request: &CallToolRequest) -> Result<CallToolResult, CallToolError> {
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

    async fn get_component_info(&self, request: &CallToolRequest) -> Result<CallToolResult, CallToolError> {
        let args: GetComponentInfoArgs = serde_json::from_value(request.arguments.clone())
            .map_err(|e| CallToolError::invalid_params(format!("Invalid arguments: {}", e)))?;

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

    async fn list_agents(&self, _request: &CallToolRequest) -> Result<CallToolResult, CallToolError> {
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

    async fn get_agent_info(&self, request: &CallToolRequest) -> Result<CallToolResult, CallToolError> {
        let args: GetAgentInfoArgs = serde_json::from_value(request.arguments.clone())
            .map_err(|e| CallToolError::invalid_params(format!("Invalid arguments: {}", e)))?;

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

    async fn list_apps(&self, _request: &CallToolRequest) -> Result<CallToolResult, CallToolError> {
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

    async fn get_app_info(&self, request: &CallToolRequest) -> Result<CallToolResult, CallToolError> {
        let args: GetAppInfoArgs = serde_json::from_value(request.arguments.clone())
            .map_err(|e| CallToolError::invalid_params(format!("Invalid arguments: {}", e)))?;

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

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetComponentInfoArgs {
    /// The name of the component
    pub component_name: String,
    /// Optional component version
    pub version: Option<u64>,
}

#[rmcp::tool]
impl GetComponentInfoArgs {
    pub fn tool() -> Tool {
        Tool {
            name: "get_component_info".to_string(),
            description: "Get detailed information about a Golem component".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "string",
                        "description": "The name of the component"
                    },
                    "version": {
                        "type": "integer",
                        "description": "Optional component version"
                    }
                },
                "required": ["component_name"]
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetAgentInfoArgs {
    /// The name of the agent
    pub agent_name: String,
}

#[rmcp::tool]
impl GetAgentInfoArgs {
    pub fn tool() -> Tool {
        Tool {
            name: "get_agent_info".to_string(),
            description: "Get detailed information about a Golem agent".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "The name of the agent"
                    }
                },
                "required": ["agent_name"]
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetAppInfoArgs {
    /// The name of the app
    pub app_name: String,
}

#[rmcp::tool]
impl GetAppInfoArgs {
    pub fn tool() -> Tool {
        Tool {
            name: "get_app_info".to_string(),
            description: "Get detailed information about a Golem app".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "app_name": {
                        "type": "string",
                        "description": "The name of the app"
                    }
                },
                "required": ["app_name"]
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListComponentsTool {}

#[rmcp::tool]
impl ListComponentsTool {
    pub fn tool() -> Tool {
        Tool {
            name: "list_components".to_string(),
            description: "List all Golem components".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListAgentsTool {}

#[rmcp::tool]
impl ListAgentsTool {
    pub fn tool() -> Tool {
        Tool {
            name: "list_agents".to_string(),
            description: "List all Golem agents".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListAppsTool {}

#[rmcp::tool]
impl ListAppsTool {
    pub fn tool() -> Tool {
        Tool {
            name: "list_apps".to_string(),
            description: "List all Golem apps".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
}
