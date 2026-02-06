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

//! MCP (Model Context Protocol) server for Golem CLI
//!
//! This module implements an MCP server that exposes Golem operations as tools
//! for AI assistants like Claude.

use anyhow::Context as AnyhowContext;
use golem_client::api::{
    ComponentClient, ComponentClientLive, EnvironmentClient, EnvironmentClientLive, WorkerClient,
    WorkerClientLive,
};
use golem_client::{Context as ClientContext, Security};
use golem_wasm::json::OptionallyValueAndTypeJson;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, ListResourceTemplatesResult, ListResourcesResult,
    PaginatedRequestParams, RawResource, ReadResourceRequestParams, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

const DEFAULT_API_URL: &str = "https://api.golem.cloud";
const MANIFEST_FILE_NAMES: [&str; 2] = ["golem.yaml", "golem.yml"];

/// MCP Server for Golem
///
/// Exposes Golem operations (list workers, invoke functions, etc.) as MCP tools
/// that can be called by AI assistants.
#[derive(Clone)]
pub struct GolemMcpServer {
    worker_client: Arc<WorkerClientLive>,
    component_client: Arc<ComponentClientLive>,
    environment_client: Arc<EnvironmentClientLive>,
    tool_router: ToolRouter<Self>,
    manifest_root: PathBuf,
}

// Input types for tools - these derive JsonSchema for automatic schema generation
#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct ListWorkersInput {
    /// Component ID (UUID format)
    pub component_id: String,
    /// Optional filter expression (e.g., 'status = Running')
    pub filter: Option<String>,
    /// Maximum number of workers to return
    pub max_count: Option<u64>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct GetWorkerInput {
    /// Component ID (UUID format)
    pub component_id: String,
    /// Name of the worker
    pub worker_name: String,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct InvokeWorkerInput {
    /// Component ID (UUID format)
    pub component_id: String,
    /// Name of the worker
    pub worker_name: String,
    /// Function name to invoke (e.g., 'golem:component/api.{add}')
    pub function: String,
    /// Function parameters as JSON values
    pub params: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct CreateWorkerInput {
    /// Component ID (UUID format)
    pub component_id: String,
    /// Name for the new worker
    pub worker_name: String,
    /// Environment variables for the worker
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct DeleteWorkerInput {
    /// Component ID (UUID format)
    pub component_id: String,
    /// Name of the worker to delete
    pub worker_name: String,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct GetDeploymentInput {
    /// Environment ID (UUID format)
    pub environment_id: String,
    /// Deployment ID (revision number)
    pub deployment_id: u64,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
pub struct ListComponentsInput {
    /// Environment ID (UUID format)
    pub environment_id: String,
}

impl GolemMcpServer {
    /// Create a new MCP server from environment variables
    ///
    /// Required env vars:
    /// - GOLEM_API_KEY: API token for authentication
    /// - GOLEM_API_URL: Base URL for Golem API (default: https://api.golem.cloud)
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = env::var("GOLEM_API_KEY")
            .context("GOLEM_API_KEY environment variable not set")?;

        let base_url = env::var("GOLEM_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
        let base_url = Url::parse(&base_url).context("GOLEM_API_URL is not a valid URL")?;
        let manifest_root = env::current_dir()
            .context("Failed to determine current directory")?
            .canonicalize()
            .context("Failed to canonicalize current directory")?;

        let http_client = reqwest::Client::builder()
            .build()
            .context("Failed to create HTTP client")?;

        let context = ClientContext {
            client: http_client,
            base_url: base_url.clone(),
            security_token: Security::Bearer(api_key),
        };

        Ok(Self {
            worker_client: Arc::new(WorkerClientLive {
                context: context.clone(),
            }),
            component_client: Arc::new(ComponentClientLive {
                context: context.clone(),
            }),
            environment_client: Arc::new(EnvironmentClientLive { context }),
            tool_router: Self::tool_router(),
            manifest_root,
        })
    }

    fn invalid_uri_error(uri: &str, message: &str) -> McpError {
        McpError::invalid_params(format!("Invalid resource URI '{}': {}", uri, message), None)
    }

    fn parse_uuid(value: &str, label: &str) -> Result<uuid::Uuid, McpError> {
        value.parse().map_err(|_| {
            McpError::invalid_params(format!("Invalid {} UUID: {}", label, value), None)
        })
    }

    fn resource_from_path(path: &Path, root: &Path) -> Option<Resource> {
        let uri = Url::from_file_path(path).ok()?;
        let name = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        let mut raw = RawResource::new(uri.to_string(), name);
        raw.description = Some("Golem manifest file".to_string());
        raw.mime_type = Some("text".to_string());
        Some(Resource::new(raw, None))
    }

    fn manifest_paths_in_dir(dir: &Path, paths: &mut HashSet<PathBuf>) {
        for filename in MANIFEST_FILE_NAMES {
            let candidate = dir.join(filename);
            if candidate.is_file() {
                paths.insert(candidate);
            }
        }
    }

    fn discover_manifest_paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut paths = HashSet::new();

        Self::manifest_paths_in_dir(&self.manifest_root, &mut paths);

        let mut current = self.manifest_root.clone();
        while let Some(parent) = current.parent() {
            Self::manifest_paths_in_dir(parent, &mut paths);
            current = parent.to_path_buf();
        }

        if let Ok(entries) = fs::read_dir(&self.manifest_root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    Self::manifest_paths_in_dir(&entry.path(), &mut paths);
                }
            }
        }

        let mut paths: Vec<PathBuf> = paths
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .collect();
        paths.sort();
        Ok(paths)
    }

    async fn read_manifest_resource(
        &self,
        uri: &str,
        path: &Path,
    ) -> Result<ReadResourceResult, McpError> {
        let text = fs::read_to_string(path)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(text, uri)],
        })
    }
}

#[tool_router]
impl GolemMcpServer {
    /// List workers (agent instances) for a component
    #[tool(description = "List workers (agent instances) for a component. Workers are running instances of Golem components.")]
    async fn golem_list_workers(
        &self,
        params: Parameters<ListWorkersInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let component_id = Self::parse_uuid(&input.component_id, "component_id")?;

        match self
            .worker_client
            .get_workers_metadata(
                &component_id,
                input
                    .filter
                    .as_ref()
                    .map(|value| vec![value.clone()])
                    .as_deref(),
                None,
                input.max_count,
                Some(false),
            )
            .await
        {
            Ok(workers) => {
                let text = serde_json::to_string_pretty(&workers)
                    .unwrap_or_else(|_| "Failed to serialize workers".into());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to list workers: {}",
                e
            ))])),
        }
    }

    /// Get metadata about a specific worker
    #[tool(description = "Get metadata about a specific worker")]
    async fn golem_get_worker(
        &self,
        params: Parameters<GetWorkerInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let component_id = Self::parse_uuid(&input.component_id, "component_id")?;

        match self
            .worker_client
            .get_worker_metadata(&component_id, &input.worker_name)
            .await
        {
            Ok(worker) => {
                let text = serde_json::to_string_pretty(&worker)
                    .unwrap_or_else(|_| "Failed to serialize worker".into());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to get worker '{}': {}",
                input.worker_name, e
            ))])),
        }
    }

    /// Invoke a function on a Golem worker
    #[tool(description = "Invoke a function on a Golem worker. The worker will execute the function and return the result.")]
    async fn golem_invoke_worker(
        &self,
        params: Parameters<InvokeWorkerInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let component_id = Self::parse_uuid(&input.component_id, "component_id")?;
        let params_vec = input
            .params
            .unwrap_or_default()
            .into_iter()
            .map(|value| OptionallyValueAndTypeJson { typ: None, value })
            .collect();

        match self
            .worker_client
            .invoke_and_await_function(
                &component_id,
                &input.worker_name,
                None,
                &input.function,
                &golem_client::model::InvokeParameters { params: params_vec },
            )
            .await
        {
            Ok(result) => {
                let text = serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "Failed to serialize result".into());
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Function '{}' invoked successfully.\n\nResult:\n{}",
                    input.function, text
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to invoke function '{}': {}",
                input.function, e
            ))])),
        }
    }

    /// Create a new worker (agent instance) for a component
    #[tool(description = "Create a new worker (agent instance) for a component")]
    async fn golem_create_worker(
        &self,
        params: Parameters<CreateWorkerInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let component_id = Self::parse_uuid(&input.component_id, "component_id")?;

        match self
            .worker_client
            .launch_new_worker(
                &component_id,
                &golem_client::model::WorkerCreationRequest {
                    name: input.worker_name.clone(),
                    env: input.env.unwrap_or_default(),
                    config_vars: Default::default(),
                },
            )
            .await
        {
            Ok(worker) => {
                let text = serde_json::to_string_pretty(&worker)
                    .unwrap_or_else(|_| "Failed to serialize worker".into());
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Worker '{}' created successfully.\n\n{}",
                    input.worker_name, text
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to create worker '{}': {}",
                input.worker_name, e
            ))])),
        }
    }

    /// Delete a worker
    #[tool(description = "Delete a worker")]
    async fn golem_delete_worker(
        &self,
        params: Parameters<DeleteWorkerInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let component_id = Self::parse_uuid(&input.component_id, "component_id")?;

        match self
            .worker_client
            .delete_worker(&component_id, &input.worker_name)
            .await
        {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Worker '{}' deleted successfully.",
                input.worker_name
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to delete worker '{}': {}",
                input.worker_name, e
            ))])),
        }
    }

    /// List components for an environment
    #[tool(description = "List components for an environment. Components are WebAssembly modules that define the behavior of workers.")]
    async fn golem_list_components(
        &self,
        params: Parameters<ListComponentsInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let environment_id = Self::parse_uuid(&input.environment_id, "environment_id")?;
        match self
            .component_client
            .get_environment_components(&environment_id)
            .await
        {
            Ok(components) => {
                let text = serde_json::to_string_pretty(&components)
                    .unwrap_or_else(|_| "Failed to serialize components".into());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to list components: {}",
                e
            ))])),
        }
    }

    /// Get deployment summary for an environment
    #[tool(description = "Get deployment summary for an environment")]
    async fn golem_get_deployment(
        &self,
        params: Parameters<GetDeploymentInput>,
    ) -> Result<CallToolResult, McpError> {
        let input = params.0;
        let environment_id = Self::parse_uuid(&input.environment_id, "environment_id")?;
        match self
            .environment_client
            .get_deployment_summary(&environment_id, input.deployment_id)
            .await
        {
            Ok(deployment) => {
                let text = serde_json::to_string_pretty(&deployment)
                    .unwrap_or_else(|_| "Failed to serialize deployment summary".into());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to get deployment summary: {}",
                e
            ))])),
        }
    }
}

#[tool_handler]
impl ServerHandler for GolemMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            instructions: Some(
                "Golem MCP Server - interact with Golem Cloud for durable computing. \
                 Use these tools to manage workers (agents), \
                 invoke functions, and check deployment status."
                    .to_string(),
            ),
            ..Default::default()
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let manifest_paths = self
            .discover_manifest_paths()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let resources = manifest_paths
            .iter()
            .filter_map(|path| Self::resource_from_path(path, &self.manifest_root))
            .collect();

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let resource_templates = Vec::new();

        Ok(ListResourceTemplatesResult {
            resource_templates,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri;
        let url = Url::parse(&uri).map_err(|err| Self::invalid_uri_error(&uri, &err.to_string()))?;

        if url.scheme() != "file" {
            return Err(Self::invalid_uri_error(
                &uri,
                "unsupported scheme (expected file://)",
            ));
        }

        let path = url
            .to_file_path()
            .map_err(|_| Self::invalid_uri_error(&uri, "invalid file path"))?;
        let path = path
            .canonicalize()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let manifest_paths = self
            .discover_manifest_paths()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        if !manifest_paths.contains(&path) {
            return Err(McpError::resource_not_found(
                "Manifest resource not found".to_string(),
                None,
            ));
        }

        self.read_manifest_resource(&uri, &path).await
    }
}

/// Start the MCP server using stdio transport
pub async fn run_mcp_server(port: u16) -> anyhow::Result<()> {
    let server = GolemMcpServer::from_env()?;
    let service: StreamableHttpService<GolemMcpServer> = StreamableHttpService::new(
        move || Ok(server.clone()),
        Default::default(),
        StreamableHttpServerConfig::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind MCP server port")?;

    println!("golem-cli running MCP Server at port {}", port);

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("MCP server terminated unexpectedly")?;

    Ok(())
}
