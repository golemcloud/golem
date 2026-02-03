use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use crate::command::mcp_server::{McpServerStartArgs, McpServerSubcommand};
use crate::context::Context;
use crate::log::{logln, set_log_output, Output};
use crate::service::mcp_server::McpServerImpl;


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
        let service = McpServerImpl::new(self.ctx.clone());

        match args.transport.as_str() {
            "stdio" => {
                // MUST set before any tool runs - stdout is exclusively for JSON-RPC
                set_log_output(Output::None);
                eprintln!("Starting MCP server in stdio mode");
                self.run_stdio(service).await
            }
            "http" | _ => {
                // Default mode: HTTP/SSE (Streamable HTTP)
                let addr = format!("{}:{}", args.host, args.port);
                logln(format!("Starting MCP server in HTTP/SSE mode on {}", addr));
                self.run_http(service, addr).await
            }
        }
    }

    async fn run_stdio(&self, service: McpServerImpl) -> Result<()> {
        use rmcp::service::ServiceExt;
        use rmcp::transport::io;
        
        let stdio_transport = io::stdio();
        let running_service = service.serve(stdio_transport).await?;
        
        // Wait for the service to finish
        running_service.waiting().await?;
        
        Ok(())
    }

    async fn run_http(&self, service: McpServerImpl, addr: String) -> Result<()> {
        use rmcp::transport::streamable_http_server::{session::local::LocalSessionManager, StreamableHttpService};
        use axum::Router;
        use axum::routing::get;

        let mcp_service = StreamableHttpService::new(
            move || Ok(service.clone()),
            LocalSessionManager::default().into(),
            Default::default(),
        );

        let app = Router::new()
            .nest_service("/mcp", mcp_service)
            .route("/", get(|| async { "Hello from Golem CLI MCP Server!" }));

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
