use crate::command::mcp_server::{RunArgs, McpServerSubcommand};
use crate::mcp_server_service::Tools;
use crate::context::Context;
use anyhow::anyhow;
use async_trait::async_trait;
use rmcp::transport;
use std::sync::Arc;

#[async_trait]
pub trait McpServerCommandHandler {
    async fn handle(&self, subcommand: McpServerSubcommand) -> anyhow::Result<()>;
}

pub struct McpServerCommandHandlerImpl {
    pub ctx: Arc<Context>,
}

impl McpServerCommandHandlerImpl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl McpServerCommandHandler for McpServerCommandHandlerImpl {
    async fn handle(&self, subcommand: McpServerSubcommand) -> anyhow::Result<()> {
        match subcommand {
            McpServerSubcommand::Run { args } => self.handle_run(args).await,
        }
    }
}

impl McpServerCommandHandlerImpl {
    async fn handle_run(&self, args: RunArgs) -> anyhow::Result<()> {
        let tools = Tools::new(self.ctx.clone());

        println!(
            "Starting Golem CLI MCP server with transport {}...",
            args.transport
        );

        match args.transport.as_str() {
            "stdio" => {
                let transport = transport::stdio();
                crate::mcp_server_service::serve_router(tools, transport).await?;
            }
            "sse" => {
                // Not supported yet, returning an error.
                return Err(anyhow!("SSE transport is not yet supported."));
            }
            _ => {
                return Err(anyhow!("Unsupported transport: {}", args.transport));
            }
        }

        Ok(())
    }
}