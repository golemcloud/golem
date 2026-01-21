// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! MCP (Model Context Protocol) Server implementation for Golem CLI.
//!
//! This module provides an MCP server that exposes CLI commands as tools
//! and golem.yaml manifests as resources, enabling AI agents to interact
//! with the Golem CLI programmatically.
//!
//! ## Features
//! - Exposes all CLI leaf commands as MCP tools
//! - Exposes golem.yaml manifests from current, ancestor, and child directories as resources
//! - Uses HTTP Streamable transport for MCP communication
//!
//! ## Usage
//! Start the server with: `golem-cli --serve --serve-port 1232`
//!
//! @ai_prompt Use this module to understand how Golem CLI is exposed via MCP protocol
//! @context_boundary Standalone MCP server module, depends on clap command structure

use crate::command::GolemCliCommand;
use crate::command_name;
use anyhow::{anyhow, Result};
use clap::{Command, CommandFactory};
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, InitializeResult, JsonObject,
    ListResourcesResult, ListToolsResult, PaginatedRequestParam, RawResource,
    ReadResourceRequestParam, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    Tool,
};
use rmcp::schemars::{self, JsonSchema};
use rmcp::serde::{Deserialize, Serialize};
use rmcp::serde_json::{self, json, Value};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command as TokioCommand;
use tracing::{debug, info};

/// Environment variable to detect MCP child process (prevents recursion)
const MCP_CHILD_ENV: &str = "GOLEM_MCP_CHILD";

/// Manifest filename to search for
const MANIFEST_FILENAME: &str = "golem.yaml";

/// Input schema for CLI tool invocations
///
/// @ai_prompt Pass CLI arguments in the `args` field as a list of strings
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolInput {
    /// CLI arguments to pass to the command
    pub args: Vec<String>,
    /// Optional stdin input to pipe to the command
    #[serde(default)]
    pub stdin: Option<String>,
    /// Optional timeout in milliseconds (default: 60000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    60000
}

/// Output from CLI tool execution
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Standard output from the command
    pub stdout: String,
    /// Standard error from the command
    pub stderr: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
}

/// MCP Server handler for Golem CLI
///
/// Implements the ServerHandler trait from rmcp to expose CLI commands
/// as tools and manifest files as resources.
///
/// ## Rejected Alternatives
/// - Using a static tool registry: Dynamic discovery via clap is more maintainable
/// - Embedding command logic: Subprocess execution is safer and more isolated
#[derive(Clone)]
pub struct GolemMcpServer {
    /// Map of tool names to their command paths
    tools: Arc<HashMap<String, Vec<String>>>,
    /// Cached list of tools for quick lookup
    tool_list: Arc<Vec<Tool>>,
    /// Current working directory for resource discovery
    working_dir: PathBuf,
    /// Path to the CLI executable
    exe_path: PathBuf,
}

impl GolemMcpServer {
    /// Create a new MCP server instance
    ///
    /// Discovers all CLI commands and prepares the tool registry.
    pub fn new() -> Result<Self> {
        let exe_path =
            env::current_exe().map_err(|e| anyhow!("Failed to get current exe: {}", e))?;
        let working_dir =
            env::current_dir().map_err(|e| anyhow!("Failed to get current dir: {}", e))?;

        let command = GolemCliCommand::command();
        let (tools, tool_list) = Self::discover_tools(&command)?;

        Ok(Self {
            tools: Arc::new(tools),
            tool_list: Arc::new(tool_list),
            working_dir,
            exe_path,
        })
    }

    /// Discover all leaf commands from the clap Command tree
    fn discover_tools(command: &Command) -> Result<(HashMap<String, Vec<String>>, Vec<Tool>)> {
        let mut tools = HashMap::new();
        let mut tool_list = Vec::new();
        let cmd_name = command_name();

        Self::walk_commands(command, vec![], &cmd_name, &mut tools, &mut tool_list);

        info!("Discovered {} CLI tools", tool_list.len());
        Ok((tools, tool_list))
    }

    /// Recursively walk the command tree to find leaf commands
    fn walk_commands(
        cmd: &Command,
        path: Vec<String>,
        prefix: &str,
        tools: &mut HashMap<String, Vec<String>>,
        tool_list: &mut Vec<Tool>,
    ) {
        let subcommands: Vec<_> = cmd.get_subcommands().collect();

        if subcommands.is_empty() && !path.is_empty() {
            // This is a leaf command
            let tool_name = format!("{}.{}", prefix, path.join("."));
            let description = cmd
                .get_about()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("Execute {} command", path.join(" ")));

            let schema = schemars::schema_for!(ToolInput);
            let input_schema: Arc<JsonObject> = Arc::new(
                serde_json::from_value(
                    serde_json::to_value(&schema).unwrap_or_else(|_| json!({"type": "object"})),
                )
                .unwrap_or_default(),
            );

            let tool = Tool::new(tool_name.clone(), description, input_schema);

            tools.insert(tool_name.clone(), path.clone());
            tool_list.push(tool);
            debug!("Registered tool: {} -> {:?}", tool_name, path);
        } else {
            // Recurse into subcommands
            for subcmd in subcommands {
                if subcmd.is_hide_set() {
                    continue; // Skip hidden commands
                }
                let name = subcmd.get_name().to_string();
                let mut new_path = path.clone();
                new_path.push(name);
                Self::walk_commands(subcmd, new_path, prefix, tools, tool_list);
            }
        }
    }

    /// Execute a CLI command as a subprocess
    async fn execute_command(
        &self,
        command_path: &[String],
        input: ToolInput,
    ) -> Result<ToolOutput> {
        let mut cmd = TokioCommand::new(&self.exe_path);

        // Add command path segments
        for segment in command_path {
            cmd.arg(segment);
        }

        // Add user-provided arguments
        for arg in &input.args {
            cmd.arg(arg);
        }

        // Set environment to prevent recursion and disable colors
        cmd.env(MCP_CHILD_ENV, "1");
        cmd.env("NO_COLOR", "1");
        cmd.env("CLICOLOR", "0");

        // Set working directory
        cmd.current_dir(&self.working_dir);

        // Configure I/O
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if input.stdin.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }

        debug!("Executing command: {:?}", cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("Failed to spawn command: {}", e))?;

        // Handle stdin if provided
        if let Some(stdin_data) = &input.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                stdin
                    .write_all(stdin_data.as_bytes())
                    .await
                    .map_err(|e| anyhow!("Failed to write stdin: {}", e))?;
            }
        }

        // Wait for completion with timeout
        let timeout = tokio::time::Duration::from_millis(input.timeout_ms);
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("Command timed out after {}ms", input.timeout_ms))?
            .map_err(|e| anyhow!("Command execution failed: {}", e))?;

        Ok(ToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Discover golem.yaml manifests in current, ancestor, and child directories
    fn discover_manifests(&self) -> Vec<Resource> {
        let mut resources = Vec::new();

        // Check current directory
        self.check_manifest(&self.working_dir, &mut resources);

        // Check ancestor directories
        let mut current = self.working_dir.clone();
        while let Some(parent) = current.parent() {
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
            self.check_manifest(&current, &mut resources);
        }

        // Check direct child directories (one level deep)
        if let Ok(entries) = std::fs::read_dir(&self.working_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    self.check_manifest(&path, &mut resources);
                }
            }
        }

        info!("Discovered {} manifest resources", resources.len());
        resources
    }

    /// Check if a directory contains a golem.yaml manifest
    fn check_manifest(&self, dir: &Path, resources: &mut Vec<Resource>) {
        let manifest_path = dir.join(MANIFEST_FILENAME);
        if manifest_path.exists() && manifest_path.is_file() {
            let uri = format!("golem-manifest://{}", manifest_path.display());
            let name = self.relative_path_name(&manifest_path);

            let raw = RawResource {
                uri,
                name,
                title: None,
                description: Some(format!("Golem manifest at {}", manifest_path.display())),
                mime_type: Some("text/yaml".to_string()),
                size: None,
                icons: None,
                meta: None,
            };
            resources.push(Resource {
                raw,
                annotations: None,
            });
            debug!("Found manifest: {}", manifest_path.display());
        }
    }

    /// Create a relative path name for display
    fn relative_path_name(&self, path: &Path) -> String {
        path.strip_prefix(&self.working_dir)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.display().to_string())
    }

    /// Read the contents of a manifest file
    fn read_manifest(&self, uri: &str) -> Result<String> {
        let path = uri
            .strip_prefix("golem-manifest://")
            .ok_or_else(|| anyhow!("Invalid manifest URI: {}", uri))?;

        std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read manifest at {}: {}", path, e))
    }
}

impl ServerHandler for GolemMcpServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult {
            protocol_version: Default::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation {
                name: command_name(),
                title: Some("Golem CLI MCP Server".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Golem CLI MCP Server - Use tools to execute CLI commands, \
                 use resources to access golem.yaml manifests"
                    .to_string(),
            ),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: (*self.tool_list).clone(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.to_string();
        let command_path = self.tools.get(&tool_name).ok_or_else(|| {
            McpError::invalid_params(format!("Unknown tool: {}", tool_name), None)
        })?;

        let input: ToolInput = match request.arguments {
            Some(args) => serde_json::from_value(Value::Object(
                args.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ))
            .map_err(|e| McpError::invalid_params(format!("Invalid arguments: {}", e), None))?,
            None => ToolInput {
                args: vec![],
                stdin: None,
                timeout_ms: default_timeout(),
            },
        };

        match self.execute_command(command_path, input).await {
            Ok(output) => {
                let is_error = output.exit_code != 0;
                let content = if is_error {
                    format!(
                        "Exit code: {}\n\nStdout:\n{}\n\nStderr:\n{}",
                        output.exit_code, output.stdout, output.stderr
                    )
                } else {
                    output.stdout
                };

                if is_error {
                    Ok(CallToolResult::error(vec![Content::text(content)]))
                } else {
                    Ok(CallToolResult::success(vec![Content::text(content)]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = self.discover_manifests();
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let contents = self
            .read_manifest(&request.uri)
            .map_err(|e| McpError::resource_not_found(format!("{}", e), None))?;

        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(contents, request.uri)],
        })
    }
}

/// Start the MCP server on the specified port
///
/// This function blocks until the server is shut down (Ctrl+C).
///
/// # Arguments
/// * `port` - Port to listen on
///
/// # Example
/// ```no_run
/// # use golem_cli::mcp::run_mcp_server;
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// run_mcp_server(1232).await.unwrap();
/// # });
/// ```
pub async fn run_mcp_server(port: u16) -> Result<()> {
    // Check if we're running as a child process (prevent recursion)
    if env::var(MCP_CHILD_ENV).is_ok() {
        return Err(anyhow!(
            "Cannot start MCP server from within an MCP tool execution"
        ));
    }

    let server = GolemMcpServer::new()?;
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();

    info!("{} running MCP Server at port {}", command_name(), port);

    // Create HTTP transport with session manager
    let session_manager: Arc<LocalSessionManager> = Arc::new(LocalSessionManager::default());
    let config = StreamableHttpServerConfig::default();
    let service = StreamableHttpService::new(move || Ok(server.clone()), session_manager, config);

    // Build the axum router
    let app = axum::Router::new().route("/mcp", axum::routing::any_service(service));

    info!("Starting MCP HTTP server on {}", addr);

    // Create listener
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow!("Failed to bind to {}: {}", addr, e))?;

    // Run with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| anyhow!("Server error: {}", e))?;

    info!("MCP server shut down gracefully");
    Ok(())
}

/// Wait for shutdown signal (Ctrl+C or SIGTERM)
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}
