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
    PaginatedRequestParams, RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult,
    ResourceContents, ResourceTemplate, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use url::Url;

const DEFAULT_API_URL: &str = "https://api.golem.cloud";
const DEFAULT_OPLOG_COUNT: u64 = 100;

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
        })
    }

    fn invalid_uri_error(uri: &str, message: &str) -> McpError {
        McpError::invalid_params(format!("Invalid resource URI '{}': {}", uri, message), None)
    }

    fn resource_template(
        uri_template: &str,
        name: &str,
        description: Option<&str>,
    ) -> ResourceTemplate {
        let raw = RawResourceTemplate {
            uri_template: uri_template.to_string(),
            name: name.to_string(),
            title: None,
            description: description.map(str::to_string),
            mime_type: Some("text".to_string()),
            icons: None,
        };
        ResourceTemplate::new(raw, None)
    }

    fn parse_uuid(value: &str, label: &str) -> Result<uuid::Uuid, McpError> {
        value.parse().map_err(|_| {
            McpError::invalid_params(format!("Invalid {} UUID: {}", label, value), None)
        })
    }

    async fn read_components_resource(
        &self,
        uri: &str,
        environment_id: &str,
    ) -> Result<ReadResourceResult, McpError> {
        let environment_id = Self::parse_uuid(environment_id, "environment_id")?;
        let components = self
            .component_client
            .get_environment_components(&environment_id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&components)
            .unwrap_or_else(|_| "Failed to serialize components".into());
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(text, uri)],
        })
    }

    async fn read_workers_resource(
        &self,
        uri: &str,
        component_id: &str,
        query: &[(String, String)],
    ) -> Result<ReadResourceResult, McpError> {
        let component_id = Self::parse_uuid(component_id, "component_id")?;
        let filter = query
            .iter()
            .find(|(key, _)| key == "filter")
            .map(|(_, value)| value.clone());
        let max_count = query
            .iter()
            .find(|(key, _)| key == "max_count")
            .and_then(|(_, value)| value.parse::<u64>().ok());

        let workers = self
            .worker_client
            .get_workers_metadata(
                &component_id,
                filter.as_ref().map(|value| vec![value.clone()]).as_deref(),
                None,
                max_count,
                Some(false),
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&workers)
            .unwrap_or_else(|_| "Failed to serialize workers".into());
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(text, uri)],
        })
    }

    async fn read_worker_oplog_resource(
        &self,
        uri: &str,
        component_id: &str,
        worker_name: &str,
        query: &[(String, String)],
    ) -> Result<ReadResourceResult, McpError> {
        let component_id = Self::parse_uuid(component_id, "component_id")?;
        let from = query
            .iter()
            .find(|(key, _)| key == "from")
            .and_then(|(_, value)| value.parse::<u64>().ok());
        let count = query
            .iter()
            .find(|(key, _)| key == "count")
            .and_then(|(_, value)| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_OPLOG_COUNT);
        let query_filter = query
            .iter()
            .find(|(key, _)| key == "query")
            .map(|(_, value)| value.clone());

        let response = self
            .worker_client
            .get_oplog(
                &component_id,
                worker_name,
                from,
                count,
                None,
                query_filter.as_deref(),
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&response)
            .unwrap_or_else(|_| "Failed to serialize worker logs".into());
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(text, uri)],
        })
    }

    async fn read_deployments_resource(
        &self,
        uri: &str,
        environment_id: &str,
        query: &[(String, String)],
    ) -> Result<ReadResourceResult, McpError> {
        let environment_id = Self::parse_uuid(environment_id, "environment_id")?;
        let version = query
            .iter()
            .find(|(key, _)| key == "version")
            .map(|(_, value)| value.clone());

        let deployments = self
            .environment_client
            .list_deployments(&environment_id, version.as_deref())
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&deployments)
            .unwrap_or_else(|_| "Failed to serialize deployments".into());
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(text, uri)],
        })
    }

    async fn read_deployment_summary_resource(
        &self,
        uri: &str,
        environment_id: &str,
        deployment_id: &str,
    ) -> Result<ReadResourceResult, McpError> {
        let environment_id = Self::parse_uuid(environment_id, "environment_id")?;
        let deployment_id = deployment_id.parse::<u64>().map_err(|_| {
            McpError::invalid_params(
                format!("Invalid deployment_id: {}", deployment_id),
                None,
            )
        })?;

        let deployment = self
            .environment_client
            .get_deployment_summary(&environment_id, deployment_id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&deployment)
            .unwrap_or_else(|_| "Failed to serialize deployment summary".into());
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(text, uri)],
        })
    }

    async fn read_environment_resource(
        &self,
        uri: &str,
        environment_id: &str,
    ) -> Result<ReadResourceResult, McpError> {
        let environment_id = Self::parse_uuid(environment_id, "environment_id")?;
        let environment = self
            .environment_client
            .get_environment(&environment_id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&environment)
            .unwrap_or_else(|_| "Failed to serialize environment".into());
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
        let resources = Vec::new();

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
        let resource_templates = vec![
            Self::resource_template(
                "golem://components/{environment_id}",
                "component_list",
                Some("List components for an environment"),
            ),
            Self::resource_template(
                "golem://workers/{component_id}",
                "worker_list",
                Some("List workers for a component"),
            ),
            Self::resource_template(
                "golem://workers/{component_id}/{worker_name}/oplog{?from,count,query}",
                "worker_logs",
                Some("Worker oplog entries"),
            ),
            Self::resource_template(
                "golem://deployments/{environment_id}",
                "deployment_list",
                Some("List deployments for an environment"),
            ),
            Self::resource_template(
                "golem://deployments/{environment_id}/current",
                "deployment_current",
                Some("Get current deployment status for an environment"),
            ),
            Self::resource_template(
                "golem://deployments/{environment_id}/{deployment_id}",
                "deployment_summary",
                Some("Get deployment summary for an environment"),
            ),
        ];

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

        if url.scheme() != "golem" {
            return Err(Self::invalid_uri_error(
                &uri,
                "unsupported scheme (expected golem://)",
            ));
        }

        let host = url.host_str().ok_or_else(|| {
            Self::invalid_uri_error(&uri, "missing host (expected golem://<resource>)")
        })?;
        let segments: Vec<&str> = url
            .path_segments()
            .map(|segments| segments.filter(|segment| !segment.is_empty()).collect())
            .unwrap_or_default();
        let query_pairs: Vec<(String, String)> =
            url.query_pairs().map(|(k, v)| (k.into(), v.into())).collect();

        match host {
            "components" => match segments.as_slice() {
                [environment_id] => self.read_components_resource(&uri, environment_id).await,
                _ => Err(Self::invalid_uri_error(
                    &uri,
                    "expected golem://components/{environment_id}",
                )),
            },
            "workers" => match segments.as_slice() {
                [component_id] => self
                    .read_workers_resource(&uri, component_id, &query_pairs)
                    .await,
                [component_id, worker_name, "oplog"] => {
                    self.read_worker_oplog_resource(&uri, component_id, worker_name, &query_pairs)
                        .await
                }
                _ => Err(Self::invalid_uri_error(
                    &uri,
                    "expected golem://workers/{component_id} or golem://workers/{component_id}/{worker_name}/oplog",
                )),
            },
            "deployments" => match segments.as_slice() {
                [environment_id] => self
                    .read_deployments_resource(&uri, environment_id, &query_pairs)
                    .await,
                [environment_id, "current"] => {
                    self.read_environment_resource(&uri, environment_id).await
                }
                [environment_id, deployment_id] => {
                    self.read_deployment_summary_resource(&uri, environment_id, deployment_id)
                        .await
                }
                _ => Err(Self::invalid_uri_error(
                    &uri,
                    "expected golem://deployments/{environment_id} or golem://deployments/{environment_id}/current or golem://deployments/{environment_id}/{deployment_id}",
                )),
            },
            _ => Err(McpError::resource_not_found(
                format!("Unknown resource '{}'.", host),
                None,
            )),
        }
    }
}

/// Start the MCP server using stdio transport
pub async fn run_mcp_server() -> anyhow::Result<()> {
    eprintln!("Starting Golem MCP Server...");

    let server = GolemMcpServer::from_env()?;

    eprintln!("MCP Server initialized. Waiting for connections...");

    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .context("Failed to start MCP server")?;

    service.waiting().await?;

    Ok(())
}
