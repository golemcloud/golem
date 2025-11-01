// Dynamic tool generation from Clap metadata
// Leverages existing CLI command structure to reduce maintenance burden

use crate::command::GolemCliCommand;
use crate::mcp_server::security::is_sensitive_command;
use clap::{Command, CommandFactory};
use rmcp::model::Tool;
use serde_json::json;
use std::sync::Arc;

/// Generate MCP tools dynamically from Clap command metadata
pub fn generate_tools() -> Vec<Tool> {
    let cli = GolemCliCommand::command();
    let commands = extract_all_commands(&cli, vec![]);

    commands
        .into_iter()
        .filter(|cmd| !is_sensitive_command(&cmd.full_path))
        .map(|cmd| create_tool_from_command(cmd))
        .collect()
}

/// Represents a command extracted from Clap hierarchy
#[derive(Debug, Clone)]
struct CommandInfo {
    full_path: String,
    tool_name: String,
    description: String,
}

/// Recursively extract all leaf commands from Clap command tree
fn extract_all_commands(cmd: &Command, parent_path: Vec<String>) -> Vec<CommandInfo> {
    let mut commands = Vec::new();
    let current_name = cmd.get_name().to_string();

    // Build current path
    let mut current_path = parent_path.clone();
    if !current_name.is_empty() && current_name != "golem-cli" {
        current_path.push(current_name.clone());
    }

    // Get subcommands
    let subcommands: Vec<_> = cmd.get_subcommands().collect();

    if subcommands.is_empty() && !current_path.is_empty() {
        // Leaf command - create tool
        let full_path = current_path.join(" ");
        let tool_name = current_path.join("_");
        let description = cmd.get_about()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Execute {} command", full_path));

        commands.push(CommandInfo {
            full_path,
            tool_name,
            description,
        });
    } else {
        // Has subcommands - recurse
        for subcmd in subcommands {
            commands.extend(extract_all_commands(subcmd, current_path.clone()));
        }
    }

    commands
}

/// Create MCP Tool from CommandInfo
fn create_tool_from_command(cmd: CommandInfo) -> Tool {
    // Basic schema - tools accept arguments as array of strings
    let schema = json!({
        "type": "object",
        "properties": {
            "args": {
                "type": "array",
                "items": {"type": "string"},
                "description": "Command arguments"
            }
        }
    });

    Tool::new(
        cmd.tool_name,
        cmd.description,
        Arc::new(schema.as_object().unwrap().clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_tools_returns_non_empty_list() {
        let tools = generate_tools();
        assert!(!tools.is_empty(), "Should generate at least one tool");
    }

    #[test]
    fn test_generate_tools_meets_minimum_requirement() {
        let tools = generate_tools();
        assert!(
            tools.len() >= 90,
            "Should generate at least 90 tools (bounty requirement), got {}",
            tools.len()
        );
    }

    #[test]
    fn test_tool_names_use_underscore_separator() {
        let tools = generate_tools();

        // Check a sample of expected tool names
        let tool_names: Vec<_> = tools.iter().map(|t| &t.name).collect();

        // Should have agent subcommands with underscore
        assert!(
            tool_names.iter().any(|n| n.contains("agent_")),
            "Should have agent_* tools"
        );

        // Tool names should not contain spaces
        for tool in &tools {
            assert!(
                !tool.name.contains(' '),
                "Tool name '{}' should not contain spaces",
                tool.name
            );
        }
    }

    #[test]
    fn test_tools_have_descriptions() {
        let tools = generate_tools();

        for tool in tools {
            assert!(
                !tool.description.is_empty(),
                "Tool '{}' should have a description",
                tool.name
            );
        }
    }

    #[test]
    fn test_sensitive_commands_filtered() {
        let tools = generate_tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();

        // Profile commands should be filtered
        assert!(
            !tool_names.iter().any(|n| n.starts_with("profile")),
            "Profile commands should be filtered as sensitive"
        );

        // Token/grant commands should be filtered
        assert!(
            !tool_names.iter().any(|n| n.contains("token") || n.contains("grant")),
            "Token and grant commands should be filtered"
        );
    }

    #[test]
    fn test_expected_commands_present() {
        let tools = generate_tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();

        // Verify key commands are present
        let expected_commands = [
            "component_list",
            "component_add",
            "agent_list",
            "agent_invoke",
            "app_new",
            "api_definition_list",
        ];

        for expected in expected_commands {
            assert!(
                tool_names.contains(&expected),
                "Expected tool '{}' not found in generated tools",
                expected
            );
        }
    }

    #[test]
    fn test_tool_schemas_valid() {
        let tools = generate_tools();

        for tool in tools {
            // Schema should have properties
            assert!(
                tool.input_schema.contains_key("properties"),
                "Tool '{}' schema should have properties",
                tool.name
            );

            // Schema should have type
            assert_eq!(
                tool.input_schema.get("type"),
                Some(&serde_json::Value::String("object".to_string())),
                "Tool '{}' schema should have type 'object'",
                tool.name
            );
        }
    }

    #[test]
    fn test_extract_commands_handles_nested_structure() {
        // Create minimal Clap command for testing
        let test_cmd = Command::new("test")
            .subcommand(
                Command::new("group")
                    .subcommand(Command::new("action").about("Test action"))
            );

        let commands = extract_all_commands(&test_cmd, vec![]);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].full_path, "group action");
        assert_eq!(commands[0].tool_name, "group_action");
    }

    #[test]
    fn test_command_info_from_clap_metadata() {
        let test_cmd = Command::new("test_command")
            .about("Test command description");

        let commands = extract_all_commands(&test_cmd, vec![]);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].description, "Test command description");
    }

    #[test]
    fn test_no_duplicate_tool_names() {
        let tools = generate_tools();
        let mut seen_names = std::collections::HashSet::new();

        for tool in &tools {
            assert!(
                seen_names.insert(&tool.name),
                "Duplicate tool name found: '{}'",
                tool.name
            );
        }
    }
}
