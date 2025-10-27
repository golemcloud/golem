// MCP Server Core Implementation
// Implements ServerHandler trait for MCP protocol

use crate::context::Context;
use rmcp::prelude::*;
use rmcp_actix_web::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use actix_web::{App, HttpServer, web};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

/// Main Golem MCP Server structure
pub struct GolemMcpServer {
    context: Arc<Context>,
    client_id: Arc<Mutex<Option<String>>>,
}

impl GolemMcpServer {
    pub fn new(context: Arc<Context>) -> Self {
        Self {
            context,
            client_id: Arc::new(Mutex::new(None)),
        }
    }
}

/// Implement MCP ServerHandler trait
#[tool_handler]
impl ServerHandler for GolemMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation {
                name: "golem-cli".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(
                "Golem CLI MCP Server. Exposes CLI commands as tools and manifest files as resources."
                .to_string()
            ),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        Ok(self.get_info())
    }
}

/// Start MCP server on specified port
pub async fn serve(
    context: Arc<Context>,
    port: u16,
) -> anyhow::Result<()> {
    if port == 0 || port > 65535 {
        anyhow::bail!("Invalid port number: {}", port);
    }

    let mcp_service = Arc::new(GolemMcpServer::new(context));
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    eprintln!("🚀 Golem CLI MCP Server starting on http://localhost:{}/mcp", port);

    HttpServer::new(move || {
        App::new()
            .service(web::scope("/mcp").service(http_service.clone().scope()))
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await?;

    Ok(())
}

pub async fn serve_with_shutdown(
    context: Arc<Context>,
    port: u16,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    if port == 0 || port > 65535 {
        anyhow::bail!("Invalid port number: {}", port);
    }

    let mcp_service = Arc::new(GolemMcpServer::new(context));
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    let server = HttpServer::new(move || {
        App::new()
            .service(web::scope("/mcp").service(http_service.clone().scope()))
    })
    .bind(("127.0.0.1", port))?
    .run();

    let handle = server.handle();
    let server_task = tokio::spawn(server);

    let _ = shutdown_rx.await;
    handle.stop(true).await;
    server_task.await??;

    Ok(())
}
