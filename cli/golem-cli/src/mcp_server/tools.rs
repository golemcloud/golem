// Tool generation interface - delegates to dynamic tool_generator
// This module provides backward compatibility with existing server code

use crate::mcp_server::tool_generator;
use rmcp::model::Tool;

/// Generate list of all CLI commands as MCP tools
/// Dynamically generated from Clap metadata to reduce maintenance burden
pub fn generate_tool_list() -> Vec<Tool> {
    tool_generator::generate_tools()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_list_generation() {
        let tools = generate_tool_list();
        assert!(tools.len() >= 90, "Should generate at least 90 tools");
    }
}
