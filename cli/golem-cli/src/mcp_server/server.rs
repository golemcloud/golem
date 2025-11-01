// MCP Server Core Implementation
// Implements ServerHandler trait for MCP protocol

use crate::context::Context;
use crate::mcp_server::{tools, resources, executor};
use rmcp::{
    handler::server::ServerHandler,
    model::*,
    service::{RequestContext, RoleServer},
    ErrorData as McpError,
};
use rmcp_actix_web::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use actix_web::{web, App, HttpServer};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

/// Main Golem MCP Server structure
#[derive(Clone)]
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
                title: Some("Golem CLI MCP Server".to_string()),
                website_url: Some("https://golem.cloud".to_string()),
                icons: Some(vec![]),
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

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = tools::generate_tool_list();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Execute CLI command via subprocess
        // Note: Streaming output is available via executor::execute_cli_command_streaming()
        // for long-running operations like builds, deployments, etc.
        // Current implementation uses simple execution for all commands.
        // Future enhancement: Detect long-running commands and use streaming automatically.

        let tool_name = request.name.to_string();

        // Execute the command
        match executor::execute_cli_command(&tool_name, &request.arguments).await {
            Ok(output) => {
                Ok(CallToolResult::success(vec![
                    RawContent::text(output).optional_annotate(None)
                ]))
            }
            Err(e) => Err(McpError::internal_error(
                format!("Command execution failed: {}", e),
                None
            )),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // Discover manifests from current working directory
        let current_dir = std::env::current_dir()
            .map_err(|e| McpError::internal_error(
                format!("Cannot determine current directory: {}", e),
                None
            ))?;

        let resources = resources::discover_manifests(&current_dir)
            .await
            .map_err(|e| McpError::internal_error(
                format!("Failed to discover manifests: {}", e),
                None
            ))?;

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.to_string();

        let contents = resources::read_manifest(&uri)
            .await
            .map_err(|e| McpError::invalid_params(
                format!("Failed to read resource: {}", e),
                None
            ))?;

        Ok(ReadResourceResult {
            contents: vec![
                ResourceContents::TextResourceContents {
                    uri: uri.clone(),
                    mime_type: Some("application/x-yaml".to_string()),
                    text: contents,
                    meta: None,
                }
            ],
        })
    }
}

/// Start MCP server on specified port
pub async fn serve(
    context: Arc<Context>,
    port: u16,
) -> anyhow::Result<()> {
    if port == 0 {
        anyhow::bail!("Invalid port number: {}", port);
    }

    let mcp_service = GolemMcpServer::new(context);
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    eprintln!("🚀 Golem CLI MCP Server starting on http://localhost:{}", port);

    HttpServer::new(move || {
        App::new()
            .service(
                web::scope("/mcp").service(http_service.clone().scope())
            )
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
    if port == 0 {
        anyhow::bail!("Invalid port number: {}", port);
    }

    let mcp_service = GolemMcpServer::new(context);
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    let server = HttpServer::new(move || {
        App::new()
            .service(
                web::scope("/mcp").service(http_service.clone().scope())
            )
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
