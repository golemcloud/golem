//! HTTP/SSE MCP server implementation using rust-mcp-sdk.
//!
//! This module provides the Streamable HTTP transport for the MCP server,
//! complementing the stdio transport in `mcp_server.rs`.
//!
//! Enabled by the `mcp` feature flag.

use async_trait::async_trait;
use rust_mcp_sdk::{
    error::SdkResult,
    event_store::InMemoryEventStore,
    macros,
    mcp_server::{hyper_server, HyperServerOptions, ServerHandler},
    schema::*,
    McpServer,
};
use std::process::Command;
use std::sync::Arc;

// ── Tool Definitions ────────────────────────────────────────────────────

#[macros::mcp_tool(
    name = "app_deploy",
    description = "Build and deploy a Golem application from a golem.yaml manifest"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AppDeployTool {
    /// Optional application manifest path (defaults to ./golem.yaml)
    #[serde(default)]
    pub manifest_path: Option<String>,
}

#[macros::mcp_tool(
    name = "app_build",
    description = "Build WASM components defined in a Golem application manifest"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AppBuildTool {
    /// Optional step filter: only run specific build steps
    #[serde(default)]
    pub step: Option<String>,
}

#[macros::mcp_tool(
    name = "component_list",
    description = "List all deployed Golem components with their metadata"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ComponentListTool {
    /// Optional name filter for components
    #[serde(default)]
    pub filter: Option<String>,
}

#[macros::mcp_tool(
    name = "worker_new",
    description = "Create a new Golem worker instance from a deployed component"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct WorkerNewTool {
    /// Component name or URN to create worker from
    pub component: String,
    /// Worker name
    pub worker_name: String,
}

#[macros::mcp_tool(
    name = "worker_invoke",
    description = "Invoke a function on a running Golem worker"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct WorkerInvokeTool {
    /// Component name or URN
    pub component: String,
    /// Worker name
    pub worker: String,
    /// Function name to invoke (e.g. 'golem:it/api.{add}')
    pub function: String,
    /// JSON arguments to pass to the function
    #[serde(default)]
    pub args: Option<String>,
}

#[macros::mcp_tool(
    name = "worker_list",
    description = "List all workers for a given Golem component"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct WorkerListTool {
    /// Component name or URN
    pub component: String,
}

// ── CLI Execution Helper ────────────────────────────────────────────────

fn run_golem_cli(args: &[&str]) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| "golem-cli".into());
    let mut cmd_args = vec!["--format", "json"];
    cmd_args.extend_from_slice(args);

    match Command::new(&exe).args(&cmd_args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                if stdout.is_empty() { stderr } else { stdout }
            } else {
                format!("Error (exit {}): {}{}", output.status, stderr, stdout)
            }
        }
        Err(e) => format!("Failed to execute golem-cli: {e}"),
    }
}

// ── Server Handler ──────────────────────────────────────────────────────

#[derive(Default)]
pub struct GolemMcpHandler;

#[async_trait]
impl ServerHandler for GolemMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _request: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: vec![
                AppDeployTool::tool(),
                AppBuildTool::tool(),
                ComponentListTool::tool(),
                WorkerNewTool::tool(),
                WorkerInvokeTool::tool(),
                WorkerListTool::tool(),
            ],
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<CallToolResult, CallToolError> {
        let result = match params.name.as_str() {
            "app_deploy" => {
                let tool: AppDeployTool = serde_json::from_value(
                    serde_json::to_value(&params.arguments).unwrap_or_default(),
                )
                .unwrap_or(AppDeployTool { manifest_path: None });
                let mut args = vec!["app", "deploy"];
                let mp;
                if let Some(ref p) = tool.manifest_path {
                    mp = p.clone();
                    args.extend_from_slice(&["--manifest", &mp]);
                }
                run_golem_cli(&args)
            }
            "app_build" => {
                let tool: AppBuildTool = serde_json::from_value(
                    serde_json::to_value(&params.arguments).unwrap_or_default(),
                )
                .unwrap_or(AppBuildTool { step: None });
                let mut args = vec!["app", "build"];
                let s;
                if let Some(ref step) = tool.step {
                    s = step.clone();
                    args.extend_from_slice(&["--step", &s]);
                }
                run_golem_cli(&args)
            }
            "component_list" => {
                let tool: ComponentListTool = serde_json::from_value(
                    serde_json::to_value(&params.arguments).unwrap_or_default(),
                )
                .unwrap_or(ComponentListTool { filter: None });
                let mut args = vec!["component", "list"];
                let f;
                if let Some(ref filter) = tool.filter {
                    f = filter.clone();
                    args.push(&f);
                }
                run_golem_cli(&args)
            }
            "worker_new" => {
                let tool: WorkerNewTool = serde_json::from_value(
                    serde_json::to_value(&params.arguments).unwrap_or_default(),
                )
                .map_err(|e| CallToolError::invalid_params(e.to_string()))?;
                run_golem_cli(&["worker", "new", &tool.component, &tool.worker_name])
            }
            "worker_invoke" => {
                let tool: WorkerInvokeTool = serde_json::from_value(
                    serde_json::to_value(&params.arguments).unwrap_or_default(),
                )
                .map_err(|e| CallToolError::invalid_params(e.to_string()))?;
                let mut args = vec![
                    "worker",
                    "invoke",
                    &tool.component,
                    &tool.worker,
                    &tool.function,
                ];
                let a;
                if let Some(ref arg_str) = tool.args {
                    a = arg_str.clone();
                    args.push(&a);
                }
                run_golem_cli(&args)
            }
            "worker_list" => {
                let tool: WorkerListTool = serde_json::from_value(
                    serde_json::to_value(&params.arguments).unwrap_or_default(),
                )
                .map_err(|e| CallToolError::invalid_params(e.to_string()))?;
                run_golem_cli(&["worker", "list", &tool.component])
            }
            _ => return Err(CallToolError::unknown_tool(params.name)),
        };

        Ok(CallToolResult::text_content(vec![result.into()]))
    }

    async fn handle_list_resources_request(
        &self,
        _request: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ListResourcesResult, RpcError> {
        let mut resources = Vec::new();
        // Discover golem.yaml manifests
        if let Ok(entries) = std::fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().map_or(false, |n| n == "golem.yaml") {
                    resources.push(Resource {
                        uri: format!("file://{}", path.display()),
                        name: "golem.yaml".into(),
                        description: Some("Golem application manifest".into()),
                        mime_type: Some("application/yaml".into()),
                        annotations: None,
                        size: None,
                    });
                }
            }
        }
        Ok(ListResourcesResult {
            resources,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> Result<ReadResourceResult, RpcError> {
        let path = params.uri.replace("file://", "");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(&params.uri, content, None)],
                meta: None,
            }),
            Err(e) => Err(RpcError::internal_error(
                format!("Failed to read {}: {e}", params.uri),
                None,
            )),
        }
    }
}

/// Start the MCP server over HTTP/SSE with Streamable HTTP transport.
pub async fn run_http_server(host: &str, port: u16) -> SdkResult<()> {
    eprintln!("Starting Golem MCP Server at http://{host}:{port}");

    let server_info = InitializeResult {
        server_info: Implementation {
            name: "golem-cli-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Golem CLI MCP Server".into()),
            description: Some(
                "MCP server exposing Golem CLI commands as tools for AI agents".into(),
            ),
            icons: vec![],
            website_url: Some("https://golem.cloud".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            resources: Some(ServerCapabilitiesResources {
                subscribe: None,
                list_changed: None,
            }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(
            "Use this server to interact with Golem Cloud via MCP tools. \
             Deploy applications, manage components, create and invoke workers."
                .into(),
        ),
        meta: None,
    };

    let handler = GolemMcpHandler::default().to_mcp_server_handler();

    let server = hyper_server::create_server(
        server_info,
        handler,
        HyperServerOptions {
            host: host.to_string(),
            port,
            sse_support: true, // backward compatibility
            event_store: Some(Arc::new(InMemoryEventStore::default())),
            ..Default::default()
        },
    );

    server.start().await?;
    Ok(())
}
