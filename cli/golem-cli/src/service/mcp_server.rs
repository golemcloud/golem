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

use crate::context::Context;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{InitializeResult, ServerCapabilities, Implementation, ProtocolVersion, CallToolRequestParam, CallToolResult, ErrorData, Content, PaginatedRequestParam, ListToolsResult};
use rmcp::service::{RequestContext, RoleServer};
use std::sync::Arc;
use anyhow::Result;

#[derive(Clone)]
pub struct McpServerImpl {
    pub ctx: Arc<Context>,
}

impl McpServerImpl {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }
}

impl ServerHandler for McpServerImpl {
    fn get_info(&self) -> InitializeResult {
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

    fn call_tool(
        &self,
        tool_call_request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        async move {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Tool call received: {:?}",
                tool_call_request
            ))]))
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        async move {
            Ok(ListToolsResult {
                tools: vec![],
                next_cursor: None,
                meta: Default::default(),
            })
        }
    }
}
