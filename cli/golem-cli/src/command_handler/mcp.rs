// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::command::McpSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::environment::EnvironmentResolveMode;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ServerHandler, model::*, tool, tool_router, tool_handler, ServiceExt};
use std::sync::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub struct McpCommandHandler {
    ctx: Arc<Context>,
}

impl McpCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: McpSubcommand) -> anyhow::Result<()> {
        match subcommand {
            McpSubcommand::Serve { port: _ } => self.cmd_serve().await,
        }
    }

    async fn cmd_serve(&self) -> anyhow::Result<()> {
        let server = GolemMcpServer::new(self.ctx.clone());
        
        // Start server over stdio
        server.serve((tokio::io::stdin(), tokio::io::stdout())).await?.waiting().await?;
        
        Ok(())
    }
}

#[derive(Clone)]
pub struct GolemMcpServer {
    ctx: Arc<Context>,
    tool_router: ToolRouter<Self>,
}

impl GolemMcpServer {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ListWorkersRequest {
    /// Name of the component
    pub component_name: String,
}

#[tool_router]
impl GolemMcpServer {
    #[tool(name = "golem_list_workers", description = "Lists workers for a given component")]
    pub async fn list_workers(&self, params: Parameters<ListWorkersRequest>) -> Result<CallToolResult, rmcp::ErrorData> {
        let component_name_str = &params.0.component_name;
        let component_name = golem_common::model::component::ComponentName(component_name_str.clone());
        
        let component_handler = self.ctx.component_handler();
        let worker_handler = self.ctx.worker_handler();
        let env_handler = self.ctx.environment_handler();

        let env = env_handler.resolve_environment(EnvironmentResolveMode::Any).await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Env error: {}", e), None))?;

        let component_opt = component_handler.resolve_component(&env, &component_name, None).await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Lookup error: {}", e), None))?;

        let component = component_opt
            .ok_or_else(|| rmcp::ErrorData::invalid_params(format!("Component not found: {}", component_name_str), None))?;

        let workers = worker_handler.list_component_workers(&component_name, &component.id, None, None, None, false).await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Failed to list workers: {}", e), None))?;

        Ok(CallToolResult {
            content: vec![Content::text(format!("Found {} workers for {}.", workers.0.len(), component_name_str))],
            is_error: None,
            meta: None,
            structured_content: None,
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GolemMcpServer {}
