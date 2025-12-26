use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use crate::command::mcp_server::{McpServerStartArgs, McpServerSubcommand};
use crate::context::Context;
use crate::log::logln;
use crate::service::mcp_server::McpServerImpl;
use tokio::io::{stdin, stdout};
use rmcp::ServiceExt;

#[async_trait]
pub trait McpServerCommandHandler {
    async fn handle(&self, subcommand: McpServerSubcommand) -> anyhow::Result<()>;
}

pub struct McpServerCommandHandlerDefault {
    pub ctx: Arc<Context>,
}

impl McpServerCommandHandlerDefault {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl McpServerCommandHandler for McpServerCommandHandlerDefault {
    async fn handle(&self, subcommand: McpServerSubcommand) -> anyhow::Result<()> {
        match subcommand {
            McpServerSubcommand::Start(args) => {
                self.run(args).await?;
            }
        }
        Ok(())
    }
}

impl McpServerCommandHandlerDefault {
    async fn run(&self, args: McpServerStartArgs) -> Result<()> {
        let addr = format!("{}:{}", args.host, args.port);
        logln(format!("Starting MCP server on {}", addr));

        let service = McpServerImpl::new(self.ctx.clone());
        service.serve((stdin(), stdout())).await?;

        Ok(())
    }
}
