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

use rust_mcp_schema::schema_utils::CallToolError;
use rust_mcp_schema::{CallToolResult, TextContent, Tool, ToolInputSchema};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error};

/// Build MCP tool definitions by introspecting the golem-cli Clap commands.
/// Each top-level command group + subcommand becomes an MCP tool.
pub fn build_tool_definitions() -> Vec<Tool> {
    let mut tools = Vec::new();

    // Use Clap's CommandFactory to introspect the CLI structure
    use clap::CommandFactory;
    let cmd = crate::command::GolemCliCommand::command();

    for sub in cmd.get_subcommands() {
        let group_name = sub.get_name();

        // Skip non-actionable commands
        if matches!(group_name, "completion" | "repl" | "help" | "serve") {
            continue;
        }

        // If this command has subcommands, register each as a tool
        let nested: Vec<_> = sub.get_subcommands().collect();
        if nested.is_empty() {
            // Top-level command with no subcommands — register as single tool
            tools.push(command_to_tool(group_name, None, sub));
        } else {
            for nested_cmd in nested {
                let nested_name = nested_cmd.get_name();
                if nested_name == "help" {
                    continue;
                }

                // Check for further nesting (e.g. api definition list)
                let deep_nested: Vec<_> = nested_cmd.get_subcommands().collect();
                if deep_nested.is_empty() {
                    tools.push(command_to_tool(group_name, Some(nested_name), nested_cmd));
                } else {
                    for deep_cmd in deep_nested {
                        let deep_name = deep_cmd.get_name();
                        if deep_name == "help" {
                            continue;
                        }
                        let tool_name = format!("golem_{group_name}_{nested_name}_{deep_name}");
                        let description = deep_cmd
                            .get_about()
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                        tools.push(Tool {
                            name: tool_name,
                            description: Some(description),
                            input_schema: build_input_schema(deep_cmd),
                            annotations: None,
                            execution: None,
                            icons: vec![],
                            meta: None,
                            output_schema: None,
                            title: None,
                        });
                    }
                }
            }
        }
    }

    tools
}

fn command_to_tool(group: &str, subcommand: Option<&str>, cmd: &clap::Command) -> Tool {
    let tool_name = match subcommand {
        Some(sub) => format!("golem_{group}_{sub}"),
        None => format!("golem_{group}"),
    };

    let description = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();

    Tool {
        name: tool_name,
        description: Some(description),
        input_schema: build_input_schema(cmd),
        annotations: None,
        execution: None,
        icons: vec![],
        meta: None,
        output_schema: None,
        title: None,
    }
}

/// Build a `ToolInputSchema` from a Clap command's arguments.
fn build_input_schema(cmd: &clap::Command) -> ToolInputSchema {
    let mut properties: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut required = Vec::new();

    for arg in cmd.get_arguments() {
        let name = arg.get_id().as_str();
        if name == "help" || name == "version" {
            continue;
        }

        let mut prop = Map::new();

        // Determine type from value parser hint
        let type_str = match arg.get_action() {
            clap::ArgAction::SetTrue | clap::ArgAction::SetFalse => "boolean",
            clap::ArgAction::Count => "integer",
            _ => "string",
        };
        prop.insert("type".to_string(), Value::String(type_str.to_string()));

        // Add description from help text
        if let Some(help) = arg.get_help() {
            prop.insert("description".to_string(), Value::String(help.to_string()));
        }

        properties.insert(name.to_string(), prop);

        if arg.is_required_set() {
            required.push(name.to_string());
        }
    }

    ToolInputSchema::new(required, Some(properties), None)
}

/// Dispatch an MCP tool call by executing golem-cli as a subprocess.
/// This is the simplest and most robust approach — it reuses the exact same
/// CLI parsing, context initialization, and error handling.
pub async fn dispatch_tool_call(
    tool_name: &str,
    arguments: Option<Map<String, Value>>,
    working_dir: &Path,
) -> Result<CallToolResult, CallToolError> {
    // Parse tool name: golem_<group>_<subcommand>[_<sub2>] → ["group", "subcommand", ...]
    let parts: Vec<&str> = tool_name
        .strip_prefix("golem_")
        .unwrap_or(tool_name)
        .split('_')
        .collect();

    if parts.is_empty() {
        return Err(CallToolError::unknown_tool(tool_name));
    }

    // Build the CLI command line
    let mut cli_args: Vec<String> = parts.iter().map(|s| s.to_string()).collect();

    // Convert JSON arguments to CLI flags
    if let Some(args) = arguments {
        for (key, value) in args {
            match value {
                Value::Bool(true) => {
                    cli_args.push(format!("--{key}"));
                }
                Value::Bool(false) => {} // Skip false flags
                Value::Null => {}        // Skip nulls
                Value::Array(arr) => {
                    for item in arr {
                        cli_args.push(format!("--{key}"));
                        cli_args.push(value_to_string(&item));
                    }
                }
                other => {
                    cli_args.push(format!("--{key}"));
                    cli_args.push(value_to_string(&other));
                }
            }
        }
    }

    debug!("MCP tool dispatch: golem-cli {}", cli_args.join(" "));

    // Execute golem-cli as subprocess with JSON output format
    let executable = std::env::current_exe().unwrap_or_else(|_| "golem-cli".into());
    let output = Command::new(&executable)
        .args(["--format", "json"])
        .args(&cli_args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if output.status.success() {
                let text = if stdout.is_empty() { stderr } else { stdout };
                Ok(CallToolResult::text_content(vec![TextContent::from(text)]))
            } else {
                let error_text = if stderr.is_empty() {
                    stdout
                } else {
                    format!("{stderr}\n{stdout}")
                };
                error!("Tool {tool_name} failed: {error_text}");
                let mut result = CallToolResult::text_content(vec![TextContent::from(error_text)]);
                result.is_error = Some(true);
                Ok(result)
            }
        }
        Err(e) => Err(CallToolError::from_message(format!(
            "Failed to execute golem-cli: {e}"
        ))),
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}
