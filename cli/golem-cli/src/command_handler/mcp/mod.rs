use std::sync::Arc;
use mcp_sdk::server::{McpServer, McpServerConfig};
use mcp_sdk::types::{Tool, ToolCallResponse};
use crate::command_handler::CommandHandler;
use crate::context::Context;

pub struct GolemMcpHandler {
    context: Arc<Context>,
    handler: Arc<dyn CommandHandler + Send + Sync>,
}

impl GolemMcpHandler {
    pub fn new(context: Arc<Context>, handler: Arc<dyn CommandHandler + Send + Sync>) -> Self {
        Self { context, handler }
    }

    pub async fn run(&self, port: u16) -> Result<(), Box<dyn std::error::Error>> {
        let config = McpServerConfig::default().with_port(port);
        let mut server = McpServer::new(config);

        // Define golem_deploy tool
        server.register_tool(Tool::new(
            "golem_deploy",
            "Deploy a Golem application",
            r#"{"type": "object", "properties": {"app_name": {"type": "string"}}}"#,
        ), |args| async move {
            // Implementation logic for golem_deploy
            Ok(ToolCallResponse::new("Application deployed successfully"))
        });

        // Define golem_worker_invoke tool
        server.register_tool(Tool::new(
            "golem_worker_invoke",
            "Invoke a function on a Golem worker",
            r#"{"type": "object", "properties": {"worker_id": {"type": "string"}, "function": {"type": "string"}}}"#,
        ), |args| async move {
            // Implementation logic for golem_worker_invoke
            Ok(ToolCallResponse::new("Worker invoked successfully"))
        });

        println!("Golem MCP Server running on port {}", port);
        server.start().await?;
        Ok(())
    }
}
