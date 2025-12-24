use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{CallToolResult},
    tool, tool_handler, tool_router, ServerHandler,
    ErrorData as McpError,
};
use std::sync::Arc;
use crate::context::Context;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::command_handler::Handlers;
use golem_common::model::agent::RegisteredAgentType; // Corrected import

#[derive(Clone)]
pub struct GolemCliMcpService {
    ctx: Arc<Context>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl GolemCliMcpService {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }

    /// Ping the Golem CLI MCP server
    #[tool(description = "Ping the Golem CLI MCP server")]
    pub async fn ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::structured(serde_json::json!({ "status": "ok" })))}

    /// Lists all deployed agent types
    #[tool(description = "Lists all deployed agent types.")]
    pub async fn list_agent_types(&self) -> Result<CallToolResult, McpError> {
        let result = self.ctx.app_handler().cmd_list_agent_types().await;

        match result {
            Ok(agent_types) => {
                Ok(CallToolResult::structured(serde_json::to_value(agent_types).unwrap()))
            }
            Err(e) => Err(McpError::internal_error(format!("Failed to list agent types: {}", e), None)),
        }
    }
}

#[async_trait]
#[tool_handler]
impl ServerHandler for GolemCliMcpService {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListAgentTypesOutput(pub Vec<RegisteredAgentType>);



