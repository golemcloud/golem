use rmcp::model::{InitializeResult, ServerCapabilities, Implementation, ProtocolVersion, ErrorData, ErrorCode, CallToolResult, Content};
use rmcp::service::{RequestContext, NotificationContext};
use rmcp::service::{RoleServer, ServiceRole};
use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use golem_client::model::ComponentDto;
use rmcp_macros::{tool_router, tool};
use crate::command_handler::app::AppCommandHandler;
use crate::command_handler::Handlers;
use crate::context::Context;
use std::future::Future;

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
}

impl rmcp::service::Service<RoleServer> for McpServerImpl {
    fn handle_request(
        &self,
        _request: <RoleServer as rmcp::service::ServiceRole>::PeerReq,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<<RoleServer as ServiceRole>::Resp, ErrorData>> + Send + '_ {
        async {
            Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                "Request not handled".to_string(),
                None,
            ))
        }
    }

    fn handle_notification(
        &self,
        _notification: <RoleServer as rmcp::service::ServiceRole>::PeerNot,
        _context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<(), ErrorData>> + Send + '_ {
        async {
            Ok(())
        }
    }

    fn get_info(&self) -> <RoleServer as rmcp::service::ServiceRole>::Info {
        InitializeResult {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().build(),
            server_info: Implementation {
                name: "Golem CLI MCP Server".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: None,
        }
    }
}

#[tool_router]
impl McpServerImpl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
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
