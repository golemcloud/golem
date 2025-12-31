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
        let registered_agent_types = app_command_handler.cmd_list_agent_types().await.map_err(|e: anyhow::Error| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
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
        let components: Vec<McpComponentDto> = self.ctx.component_handler().cmd_list_components().await.map_err(|e: anyhow::Error| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?
            .into_iter()
            .map(|c: ComponentDto| c.into())
            .collect();
        let response = ListComponentsResponse { components };
        let content = serde_json::to_value(response).map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::json(content)?]))
    }
}
