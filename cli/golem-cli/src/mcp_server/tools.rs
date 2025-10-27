// MCP Tools - CLI commands exposed as MCP tools
// Exposes Golem CLI commands as MCP tools for AI agents

use rmcp::model::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Example tool parameter structure
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ComponentListParams {
    /// Optional project name filter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// Generate list of all available tools from Golem CLI commands
pub fn generate_tool_list() -> Vec<Tool> {
    vec![
        Tool::new(
            "component_list",
            "List all components in the current Golem project",
            Arc::new(serde_json::json!({
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "Optional project name filter"
                    }
                },
                "required": []
            }).as_object().unwrap().clone())
        ),
        Tool::new(
            "worker_list",
            "List all workers in the Golem project",
            Arc::new(serde_json::json!({
                "type": "object",
                "properties": {
                    "component": {
                        "type": "string",
                        "description": "Filter by component name"
                    }
                },
                "required": []
            }).as_object().unwrap().clone())
        ),
    ]
}

/// Check if a command should be exposed as a tool
/// Filters out security-sensitive commands
pub fn is_command_safe_to_expose(command: &str) -> bool {
    const UNSAFE_COMMANDS: &[&str] = &[
        "profile",  // Contains auth tokens
        "login",    // Authentication
        "token",    // Token management
    ];

    !UNSAFE_COMMANDS.iter().any(|unsafe_cmd| command.starts_with(unsafe_cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_tool_list() {
        let tools = generate_tool_list();
        assert!(tools.len() >= 2);
        assert!(tools.iter().any(|t| t.name == "component_list"));
        assert!(tools.iter().any(|t| t.name == "worker_list"));
    }

    #[test]
    fn test_security_filtering() {
        assert!(is_command_safe_to_expose("component"));
        assert!(is_command_safe_to_expose("worker"));
        assert!(!is_command_safe_to_expose("profile"));
        assert!(!is_command_safe_to_expose("login"));
    }
}
