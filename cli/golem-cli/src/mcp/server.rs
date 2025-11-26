use crate::mcp::context::McpContext;
use crate::mcp::tools::{CommandInfo, Mcptool};
use crate::mcp::{executor, resources};
use actix_web::{web, App, HttpServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::{
    handler::server::ServerHandler,
    model::*,
    service::{RequestContext, RoleServer},
    ErrorData as McpError,
};
use rmcp_actix_web::transport::StreamableHttpService;
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

/// Main Golem MCP Server structure
#[derive(Clone)]
pub struct GolemMcpServer {
    #[allow(dead_code)]
    context: Arc<McpContext>,
    _working_dir: PathBuf,
    scanner: Arc<resources::ResourceScanner>,
    executor: Arc<executor::CliExecutor>,
    cached_tools: Arc<Mutex<Option<Vec<Tool>>>>,
    cached_tool_names: Arc<Mutex<Option<HashSet<String>>>>,

    _client_id: Arc<Mutex<Option<String>>>,
}

impl GolemMcpServer {
    pub fn new(context: Arc<McpContext>, working_dir: PathBuf) -> anyhow::Result<Self> {
        let scanner = Arc::new(resources::ResourceScanner::new(working_dir.clone())?);
        let executor = Arc::new(executor::CliExecutor::new(working_dir.clone()));

        Ok(Self {
            context,
            _working_dir: working_dir,
            scanner,
            executor,
            cached_tools: Arc::new(Mutex::new(None)),
            cached_tool_names: Arc::new(Mutex::new(None)),
            _client_id: Arc::new(Mutex::new(None)),
        })
    }

    /// Populate tool cache (call at startup or on-demand)
    async fn refresh_tool_cache(&self) {
        let mcp_tools = CommandInfo::extract_all_tools(&CommandInfo);
        let converted: Vec<Tool> = mcp_tools.into_iter().map(convert_to_rmcp_tool).collect();
        let names: HashSet<String> = converted.iter().map(|t| t.name.to_string()).collect();

        let mut tools_lock = self.cached_tools.lock().await;
        *tools_lock = Some(converted);

        let mut names_lock = self.cached_tool_names.lock().await;
        *names_lock = Some(names);
    }

    /// Get cached tools, refresh if empty
    async fn tools_cached(&self) -> Vec<Tool> {
        {
            let tools_lock = self.cached_tools.lock().await;
            if let Some(ref tools) = *tools_lock {
                return tools.clone();
            }
        }
        // cache miss - refresh
        self.refresh_tool_cache().await;
        let tools_lock = self.cached_tools.lock().await;
        tools_lock.as_ref().cloned().unwrap_or_default()
    }

    /// Fast existence check using the cached name set
    async fn name_cached(&self, name: &str) -> bool {
        {
            let names_lock = self.cached_tool_names.lock().await;
            if let Some(ref names) = *names_lock {
                return names.contains(name);
            }
        }
        // If name set missing, refresh and check
        self.refresh_tool_cache().await;
        let names_lock = self.cached_tool_names.lock().await;
        names_lock
            .as_ref()
            .map(|s| s.contains(name))
            .unwrap_or(false)
    }
}

/// Convert internal Mcptool to rmcp::protocol::Tool
fn convert_to_rmcp_tool(mcp_tool: Mcptool) -> Tool {
    let input_obj = serde_json::json!({
        "type": mcp_tool.input_schema.type_,
        "properties": mcp_tool.input_schema.properties.unwrap_or_default(),
        "required": mcp_tool.input_schema.required.unwrap_or_default(),
    })
    .as_object()
    .unwrap()
    .clone();

    Tool {
        name: mcp_tool.tool_name.into(),
        description: mcp_tool.summary.map(|s| s.into()),
        title: None,
        input_schema: Arc::new(input_obj),
        output_schema: None,
        annotations: None,
        icons: None,
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
                icons: None,
            },
            instructions: Some(
                "Golem CLI MCP Server. Exposes CLI commands as tools and manifest files as resources."
                    .to_string(),
            ),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.refresh_tool_cache().await;
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self.tools_cached().await;
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
        let tool_name = request.name.clone();

        // Validate existence quickly via cached names
        if !self.name_cached(&tool_name).await {
            return Err(McpError::invalid_params(
                format!("Tool '{}' not found", tool_name),
                None,
            ));
        }

        let raw_args = request.arguments.clone();

        let result = self
            .executor
            .execute_cli_command_streaming(&tool_name, &raw_args)
            .await;

        match result {
            Ok(output) => Ok(CallToolResult {
                content: vec![RawContent::text(output).optional_annotate(None)],
                structured_content: None,
                is_error: Some(false),
                meta: None,
            }),
            Err(e) => {
                // Error — return proper MCP error
                Err(McpError::internal_error(
                    format!("Command execution failed: {}", e),
                    None,
                ))
            }
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = self.scanner.discover_manifests().await.map_err(|e| {
            McpError::internal_error(format!("Failed to find manifests: {}", e), None)
        })?;

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

        let contents = self.scanner.read_resources(&uri).await.map_err(|e| {
            McpError::internal_error(format!("Failed to read resource: {}", e), None)
        })?;

        Ok(ReadResourceResult { contents })
    }
}

/// Start MCP server on specified port
pub async fn serve(
    context: Arc<McpContext>,
    working_dir: PathBuf,
    port: u16,
) -> anyhow::Result<()> {
    if port == 0 {
        anyhow::bail!("Invalid port number: {}", port);
    }

    let mcp_service = GolemMcpServer::new(context, working_dir)?;
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    HttpServer::new(move || {
        App::new().service(web::scope("/mcp").service(http_service.clone().scope()))
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await?;

    Ok(())
}

pub async fn serve_with_shutdown(
    context: Arc<McpContext>,
    working_dir: PathBuf,
    port: u16,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    if port == 0 {
        anyhow::bail!("Invalid port number: {}", port);
    }

    let mcp_service = GolemMcpServer::new(context, working_dir)?;
    let http_service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(mcp_service.clone())))
        .session_manager(Arc::new(LocalSessionManager::default()))
        .stateful_mode(true)
        .sse_keep_alive(Duration::from_secs(30))
        .build();

    let server = HttpServer::new(move || {
        App::new().service(web::scope("/mcp").service(http_service.clone().scope()))
    })
    .bind(("127.0.0.1", port))?
    .run();

    let server_handle = server.handle();

    // Spawn shutdown listener
    tokio::spawn(async move {
        shutdown_signal.await;
        server_handle.stop(true).await;
    });

    server.await?;
    Ok(())
}
