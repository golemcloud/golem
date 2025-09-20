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
use std::ffi::OsString;
use crate::command::{GolemCliCommand, GolemCliGlobalFlags};
use crate::command_handler::{CommandHandler, CommandHandlerHooks};
use crate::hooks::NoHooks;
use tracing::{debug, error, info};

// Dynamic tool discovery from Clap metadata
// We'll keep execute_golem_command as a special tool for generic command execution
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

// Dynamic tool for specific Golem commands
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[mcp_tool(
    name = "execute_golem_cli_command",
    title = "Execute Golem CLI Command",
    description = "Execute a specific Golem CLI command with structured arguments"
)]
pub struct ExecuteGolemCliCommandTool {
    /// The command path (e.g., "component list", "agent info")
    pub command_path: String,
    /// Arguments for the command
    #[serde(default)]
    pub arguments: Vec<String>,
}

// Generate tool box enum for automatic tool dispatch
rmcp::tool_box!(GolemTools, [
    ExecuteGolemCommandTool,
    ExecuteGolemCliCommandTool
]);

pub struct GolemToolHandler {
    ctx: Arc<Context>,
}

impl GolemToolHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    /// Helper method to execute Golem commands using the real CommandHandler
    async fn execute_command(&self, command_parts: Vec<&str>) -> Result<String, CallToolError> {
        let mut cli_args: Vec<OsString> = vec!["golem-cli".into()];
        for part in command_parts {
            cli_args.push(part.into());
        }

        let global_flags = GolemCliGlobalFlags::default();
        let hooks = Arc::new(NoHooks {});
        let handler = CommandHandler::new(global_flags, None, hooks)
            .await
            .context("Failed to create CommandHandler")
            .map_err(|e| CallToolError::internal_error(format!("CommandHandler creation failed: {}", e)))?;

        match GolemCliCommand::try_parse_from_lenient(cli_args, true) {
            crate::command::GolemCliCommandParseResult::FullMatch(command) => {
                match handler.handle_command(command).await {
                    Ok(()) => Ok("Command executed successfully".to_string()),
                    Err(e) => Ok(format!("Command failed: {}", e)),
                }
            }
            crate::command::GolemCliCommandParseResult::Error(error) => {
                Ok(format!("Command parsing failed: {}", error))
            }
            crate::command::GolemCliCommandParseResult::ErrorWithPartialMatch { error, .. } => {
                Ok(format!("Command parsing failed: {}", error))
            }
            crate::command::GolemCliCommandParseResult::NoMatch => {
                Ok("No matching command found".to_string())
            }
        }
    }

    /// Get available commands from Clap metadata
    fn get_available_commands(&self) -> Vec<String> {
        let command = GolemCliCommand::command();
        let mut commands = Vec::new();
        
        // Get top-level subcommands
        for subcommand in command.get_subcommands() {
            let subcommand_name = subcommand.get_name();
            commands.push(subcommand_name.to_string());
            
            // Get nested subcommands
            for nested_subcommand in subcommand.get_subcommands() {
                let nested_name = nested_subcommand.get_name();
                commands.push(format!("{} {}", subcommand_name, nested_name));
            }
        }
        
        commands
    }

    pub fn list_tools(&self) -> Vec<Tool> {
        let mut tools = GolemTools::tools();
        
        // Add dynamic tools for each available command
        let available_commands = self.get_available_commands();
        for command in available_commands {
            let tool = Tool {
                name: format!("golem_{}", command.replace(" ", "_")),
                description: Some(format!("Execute the Golem CLI command: {}", command)),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "arguments": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Arguments for the command"
                        }
                    },
                    "required": []
                })),
            };
            tools.push(tool);
        }
        
        tools
    }

    pub async fn handle_call_tool_request(
        &self,
        request: &CallToolRequest,
    ) -> Result<CallToolResult, CallToolError> {
        // First try to handle with the tool_box generated tools
        match GolemTools::try_from(request.clone()) {
            Ok(tool) => match tool {
                GolemTools::ExecuteGolemCommandTool(args) => {
                    self.execute_golem_command(&args).await
                }
                GolemTools::ExecuteGolemCliCommandTool(args) => {
                    self.execute_golem_cli_command(&args).await
                }
            },
            Err(_) => {
                // If not a tool_box tool, try to handle as a dynamic tool
                self.handle_dynamic_tool(request).await
            }
        }
    }

    async fn handle_dynamic_tool(
        &self,
        request: &CallToolRequest,
    ) -> Result<CallToolResult, CallToolError> {
        let tool_name = &request.name;
        
        // Check if this is a dynamic tool (starts with "golem_")
        if let Some(command_path) = tool_name.strip_prefix("golem_") {
            // Convert underscores back to spaces
            let command_path = command_path.replace("_", " ");
            
            // Parse arguments from the request
            let arguments = if let Some(args) = &request.arguments {
                if let Some(args_array) = args.get("arguments").and_then(|v| v.as_array()) {
                    args_array
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            
            // Create a tool request and execute it
            let tool_request = ExecuteGolemCliCommandTool {
                command_path,
                arguments,
            };
            
            self.execute_golem_cli_command(&tool_request).await
        } else {
            Err(CallToolError::unknown_tool(tool_name.clone()))
        }
    }

    async fn execute_golem_command(
        &self,
        args: &ExecuteGolemCommandTool,
    ) -> Result<CallToolResult, CallToolError> {
        info!("Executing Golem command: {} with args: {:?}", args.command, args.args);

        // Build command arguments for CommandHandler
        let mut cli_args: Vec<OsString> = vec![
            "golem-cli".into(),
            args.command.clone().into(),
        ];
        
        // Add additional arguments
        for arg in &args.args {
            cli_args.push(arg.into());
        }

        // Create global flags with default values
        let global_flags = GolemCliGlobalFlags::default();
        
        // Create CommandHandler with the same context
        let hooks = Arc::new(NoHooks {});
        let handler = CommandHandler::new(global_flags, None, hooks)
            .await
            .context("Failed to create CommandHandler")
            .map_err(|e| CallToolError::internal_error(format!("CommandHandler creation failed: {}", e)))?;

        // Parse and execute the command
        let result = match GolemCliCommand::try_parse_from_lenient(cli_args, true) {
            crate::command::GolemCliCommandParseResult::FullMatch(command) => {
                match handler.handle_command(command).await {
                    Ok(()) => {
                        Ok(CallToolResult::text_content(vec!["Command executed successfully".into()]))
                    }
                    Err(e) => {
                        Ok(CallToolResult::text_content(vec![format!("Command failed: {}", e).into()]))
                    }
                }
            }
            crate::command::GolemCliCommandParseResult::Error(error) => {
                Ok(CallToolResult::text_content(vec![format!("Command parsing failed: {}", error).into()]))
            }
            crate::command::GolemCliCommandParseResult::ErrorWithPartialMatch { error, .. } => {
                Ok(CallToolResult::text_content(vec![format!("Command parsing failed: {}", error).into()]))
            }
            crate::command::GolemCliCommandParseResult::NoMatch => {
                Ok(CallToolResult::text_content(vec!["No matching command found".into()]))
            }
        };

        result
    }

    async fn execute_golem_cli_command(
        &self,
        args: &ExecuteGolemCliCommandTool,
    ) -> Result<CallToolResult, CallToolError> {
        debug!("Executing Golem CLI command: {} with args: {:?}", args.command_path, args.arguments);

        // Parse command path into parts
        let command_parts: Vec<&str> = args.command_path.split_whitespace().collect();
        if command_parts.is_empty() {
            return Ok(CallToolResult::text_content(vec!["Command path cannot be empty".into()]));
        }

        // Build full command parts including arguments
        let mut full_command_parts = command_parts;
        for arg in &args.arguments {
            full_command_parts.push(arg);
        }

        // Execute the command
        let result = self.execute_command(full_command_parts).await?;
        Ok(CallToolResult::text_content(vec![result.into()]))
    }
}
