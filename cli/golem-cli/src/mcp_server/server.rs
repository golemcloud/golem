// MCP Server Core Implementation
// Implements ServerHandler trait for MCP protocol

use crate::context::Context;
use crate::mcp_server::tools;
use rmcp::{
    handler::server::ServerHandler,
    model::*,
    service::{RequestContext, RoleServer},
    ErrorData as McpError,
};
use rmcp_actix_web::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use actix_web::{App, HttpServer};
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
        // Phase 2 GREEN: Mock responses to demonstrate MCP protocol working
        // TODO Phase 3: Implement actual CLI command execution
        let tool_name = request.name.to_string();
        match tool_name.as_str() {
            "component_list" => {
                // Parse optional project parameter
                let _project = request.arguments
                    .as_ref()
                    .and_then(|args| args.get("project"))
                    .and_then(|v| v.as_str());

                // Mock component list output (JSON format as CLI would return)
                let output = serde_json::json!({
                    "components": [
                        {
                            "name": "example-component",
                            "version": 1,
                            "size": 12345,
                            "component_type": "ephemeral"
                        },
                        {
                            "name": "another-component",
                            "version": 3,
                            "size": 67890,
                            "component_type": "durable"
                        }
                    ]
                });

                Ok(CallToolResult::success(vec![
                    RawContent::text(
                        serde_json::to_string_pretty(&output)
                            .unwrap_or_else(|_| "Error formatting output".to_string())
                    ).optional_annotate(None)
                ]))
            }
            "worker_list" => {
                // Parse optional component parameter
                let component = request.arguments
                    .as_ref()
                    .and_then(|args| args.get("component"))
                    .and_then(|v| v.as_str());

                // Mock worker list output
                let output = if let Some(comp) = component {
                    serde_json::json!({
                        "workers": [
                            {
                                "worker_id": "worker-001",
                                "component_name": comp,
                                "status": "running"
                            },
                            {
                                "worker_id": "worker-002",
                                "component_name": comp,
                                "status": "idle"
                            }
                        ]
                    })
                } else {
                    serde_json::json!({
                        "workers": [
                            {
                                "worker_id": "worker-001",
                                "component_name": "example-component",
                                "status": "running"
                            },
                            {
                                "worker_id": "worker-002",
                                "component_name": "another-component",
                                "status": "idle"
                            }
                        ]
                    })
                };

                Ok(CallToolResult::success(vec![
                    RawContent::text(
                        serde_json::to_string_pretty(&output)
                            .unwrap_or_else(|_| "Error formatting output".to_string())
                    ).optional_annotate(None)
                ]))
            }
            _ => Err(McpError::invalid_params(
                format!("Unknown tool: {}", request.name),
                None
            )),
        }
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
            .service(http_service.clone().scope())
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

    let mcp_service = GolemMcpServer::new(context);
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    let server = HttpServer::new(move || {
        App::new()
            .service(http_service.clone().scope())
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
