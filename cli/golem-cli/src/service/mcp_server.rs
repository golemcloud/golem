use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{ErrorData, ErrorCode, CallToolResult, Content};

use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use golem_client::model::ComponentDto;
use rmcp_macros::{tool_router, tool, tool_handler};
use crate::command_handler::app::AppCommandHandler;
use crate::command_handler::Handlers;
use crate::context::Context;
use std::collections::HashMap;


#[derive(JsonSchema, Deserialize, Serialize)]
pub struct ListAgentTypesRequest {}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct ListAgentTypesResponse {
    pub agent_types: Vec<String>,
}

#[derive(JsonSchema, Deserialize, Serialize, Clone, Debug)]
pub struct McpComponentDto {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub size: u64,
}

impl From<ComponentDto> for McpComponentDto {
    fn from(dto: ComponentDto) -> Self {
        McpComponentDto {
            id: dto.id.to_string(),
            name: dto.component_name.0,
            revision: dto.revision.into(),
            size: dto.component_size,
        }
    }
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct ListComponentsRequest {}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct ListComponentsResponse {
    pub components: Vec<McpComponentDto>,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct ListWorkersRequest {
    /// Optional component name to filter workers by component
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_name: Option<String>,
    /// Optional maximum number of workers to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_count: Option<u64>,
}

#[derive(JsonSchema, Deserialize, Serialize, Clone, Debug)]
pub struct McpWorkerDto {
    pub worker_id: String,
    pub component_name: String,
    pub component_id: String,
    pub status: String,
    pub created_at: String,
    pub last_error: Option<String>,
    pub retry_count: u32,
    pub pending_invocation_count: u64,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct ListWorkersResponse {
    pub workers: Vec<McpWorkerDto>,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct GetWorkerRequest {
    /// Worker name/ID in format: component-name/worker-name or full path
    pub worker_name: String,
}

#[derive(JsonSchema, Deserialize, Serialize, Clone, Debug)]
pub struct GetWorkerResponse {
    pub worker_id: String,
    pub component_name: String,
    pub component_id: String,
    pub status: String,
    pub created_at: String,
    pub last_error: Option<String>,
    pub retry_count: u32,
    pub pending_invocation_count: u64,
    pub component_version: u64,
    pub environment_id: String,
    pub env: HashMap<String, String>,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct GetComponentRequest {
    /// Component name
    pub component_name: String,
    /// Optional component revision (defaults to latest)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

#[derive(JsonSchema, Deserialize, Serialize, Clone, Debug)]
pub struct GetComponentResponse {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub size: u64,
    pub created_at: Option<String>,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct CreateWorkerRequest {
    /// Worker name/ID
    pub worker_name: String,
    /// Optional environment variables as key-value pairs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

#[derive(JsonSchema, Deserialize, Serialize, Clone, Debug)]
pub struct CreateWorkerResponse {
    pub worker_id: String,
    pub component_name: String,
    pub status: String,
}

#[derive(JsonSchema, Deserialize, Serialize)]
pub struct InvokeWorkerRequest {
    /// Worker name/ID
    pub worker_name: String,
    /// Function name to invoke
    pub function_name: String,
    /// Function arguments as JSON string or object
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

#[derive(JsonSchema, Deserialize, Serialize, Clone, Debug)]
pub struct InvokeWorkerResponse {
    pub result: serde_json::Value,
    pub idempotency_key: String,
}

#[derive(Clone)]
pub struct McpServerImpl {
    pub ctx: Arc<Context>,
    tool_router: ToolRouter<Self>,
}

#[tool_handler]
impl ServerHandler for McpServerImpl {}

#[tool_router]
impl McpServerImpl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "list_agent_types",
        description = "List all available agent types"
    )]
    async fn list_agent_types(
        &self,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let app_command_handler = AppCommandHandler::new(self.ctx.clone());
        // Return empty list if there's any error (e.g., no environment/authentication configured)
        let registered_agent_types = app_command_handler.cmd_list_agent_types().await.unwrap_or_else(|_| Vec::new());
        let agent_types = registered_agent_types.into_iter().map(|rat| rat.agent_type.type_name).collect();
        let response = ListAgentTypesResponse {
            agent_types,
        };
        let content = serde_json::to_value(response).map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::json(content)?]))
    }

    #[tool(
        name = "list_components",
        description = "List all available components"
    )]
    async fn list_components(
        &self,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // Return empty list if there's any error (e.g., no authentication/configuration)
        let components: Vec<McpComponentDto> = self.ctx.component_handler().cmd_list_components().await.unwrap_or_else(|_| Vec::new())
            .into_iter()
            .map(|c: ComponentDto| c.into())
            .collect();
        let response = ListComponentsResponse { components };
        let content = serde_json::to_value(response).map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::json(content)?]))
    }

    #[tool(
        name = "list_workers",
        description = "List all workers across all components"
    )]
    async fn list_workers(
        &self,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // Get all components
        let components = self.ctx.component_handler()
            .cmd_list_components()
            .await
            .unwrap_or_else(|_| Vec::new());

        let mut all_workers = Vec::new();
        
        for component in components {
            let (workers, _) = self.ctx.worker_handler()
                .list_component_workers(
                    &component.component_name,
                    &component.id,
                    None, // filters
                    None, // scan_cursor
                    None, // max_count
                    false, // precise
                )
                .await
                .unwrap_or_else(|_| (Vec::new(), None));
            
            for worker in workers {
                all_workers.push(McpWorkerDto {
                    worker_id: worker.worker_id.component_id.to_string(),
                    component_name: worker.component_name.to_string(),
                    component_id: component.id.to_string(),
                    status: format!("{:?}", worker.status),
                    created_at: worker.created_at.to_string(),
                    last_error: worker.last_error,
                    retry_count: worker.retry_count,
                    pending_invocation_count: worker.pending_invocation_count,
                });
            }
        }
        
        let response = ListWorkersResponse { workers: all_workers };
        let content = serde_json::to_value(response).map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::json(content)?]))
    }

    // TODO: Tools with parameters need to be implemented differently - rmcp tool macro 
    // doesn't support struct parameters. Need to find the correct pattern.
    // For now, commenting out parameterized tools.
    
    /*
    #[tool(
        name = "get_worker",
        description = "Get detailed information about a specific worker. Worker name format: component-name/worker-name"
    )]
    async fn get_worker(
        &self,
        request: GetWorkerRequest,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // Parse worker name - format: component-name/worker-name
        let worker_name = WorkerName::from(request.worker_name.as_str());
        
        // Match worker name to get component and full worker info
        let worker_name_match = self.ctx.worker_handler()
            .match_worker_name(worker_name)
            .await
            .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Failed to find worker: {}", e), None))?;
        
        // Get all components to find the matching one
        let components = self.ctx.component_handler()
            .cmd_list_components()
            .await
            .unwrap_or_else(|_| Vec::new());
        
        let component = components.into_iter()
            .find(|c| c.component_name.to_string() == worker_name_match.component_name.to_string())
            .ok_or_else(|| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Component not found: {}", worker_name_match.component_name), None))?;
        
        // Get worker metadata
        let clients = self.ctx.golem_clients().await
            .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Failed to get clients: {}", e), None))?;
        
        let metadata = WorkerClient::get_worker_metadata(
            &clients.worker,
            &component.id.0,
            &worker_name_match.worker_name.0,
        )
        .await
        .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Failed to get worker metadata: {}", e), None))?;
        
        let worker_meta = WorkerMetadata::from(worker_name_match.component_name, metadata);
        
        let response = GetWorkerResponse {
            worker_id: worker_meta.worker_id.component_id.to_string(),
            component_name: worker_meta.component_name.to_string(),
            component_id: component.id.to_string(),
            status: format!("{:?}", worker_meta.status),
            created_at: worker_meta.created_at.to_string(),
            last_error: worker_meta.last_error,
            retry_count: worker_meta.retry_count,
            pending_invocation_count: worker_meta.pending_invocation_count,
            component_version: worker_meta.component_version.into(),
            environment_id: worker_meta.environment_id.0.to_string(),
            env: worker_meta.env,
        };
        
        let content = serde_json::to_value(response).map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::json(content)?]))
    }
    */
    
    /*
    #[tool(
        name = "get_component",
        description = "Get detailed information about a specific component"
    )]
    async fn get_component(
        &self,
        request: GetComponentRequest,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        // Get all components and find the matching one
        let components = self.ctx.component_handler()
            .cmd_list_components()
            .await
            .unwrap_or_else(|_| Vec::new());
        
        let component = components.into_iter()
            .find(|c| c.component_name.to_string() == request.component_name || 
                      c.component_name.to_string().ends_with(&request.component_name))
            .ok_or_else(|| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Component not found: {}", request.component_name), None))?;
        
        // Get specific revision if requested
        let component_dto = if let Some(rev) = request.revision {
            let clients = self.ctx.golem_clients().await
                .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Failed to get clients: {}", e), None))?;
            
            ComponentClient::get_component_revision(
                &clients.component,
                &component.id.0,
                rev.into(),
            )
            .await
            .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("Failed to get component revision: {}", e), None))?
        } else {
            component
        };
        
        let response = GetComponentResponse {
            id: component_dto.id.to_string(),
            name: component_dto.component_name.to_string(),
            revision: component_dto.revision.into(),
            size: component_dto.component_size,
            created_at: component_dto.created_at.map(|d| d.to_string()),
        };
        
        let content = serde_json::to_value(response).map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::json(content)?]))
    }
    */
}
