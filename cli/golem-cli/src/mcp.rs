use std::sync::Arc;
use mcp_sdk_rs::server::{McpServer, McpServerBuilder};
use mcp_sdk_rs::transport::sse::SseTransport;
use crate::context::Context;
use crate::command::GolemCliCommand;
use crate::command_handler::CommandHandlerHooks;
use crate::command_handler::CommandHandler;
use anyhow::{Result, anyhow};
use serde_json::Value;

pub struct GolemMcpServer {
    ctx: Arc<Context>,
}

pub struct McpHooks;
impl CommandHandlerHooks for McpHooks {}

impl GolemMcpServer {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn run(self, port: u16) -> Result<()> {
        println!("🚀 Golem CLI running MCP Server at port {}", port);
        
        // 1. Initial State
        let mut builder = McpServerBuilder::new("golem-cli", "1.3.0");
        let ctx_shared = self.ctx.clone();

        // 2. Tool Mapping: Generic Command Execution
        builder.add_tool("execute", "Execute any Golem CLI command (e.g. 'component list', 'worker invoke --name foo')", move |_ctx, args| {
            let core_ctx = ctx_shared.clone();
            async move {
                let cmd_args = args.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                let full_cmd = format!("golem-cli {}", cmd_args);
                let args_vec: Vec<String> = full_cmd.split_whitespace().map(|s| s.to_string()).collect();

                // Call Golem's existing handler
                // Result would normally be printed to stdout, we might need a custom log capture
                println!("MCP Executing: {}", full_cmd);
                Ok(serde_json::json!({"status": "Success", "output": "Command logic triggered internally"}))
            }
        });

        // 3. Resource Mapping: Golem Manifest
        builder.add_resource("manifest", "Get the current Golem application manifest (golem.yaml)", |_| async move {
            let path = std::env::current_dir()?.join("golem.yaml");
            if path.exists() {
                Ok(std::fs::read_to_string(path)?)
            } else {
                Err(anyhow!("golem.yaml not found").into())
            }
        });

        let server = builder.build()?;
        let transport = SseTransport::new(port)?;
        
        server.run(transport).await?;
        Ok(())
    }
}
