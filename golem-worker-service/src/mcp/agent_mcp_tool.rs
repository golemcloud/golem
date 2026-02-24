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

use crate::mcp::GolemAgentMcpServer;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::base_model::agent::AgentMethod;
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::IntoToolRoute;
use rmcp::handler::server::tool::{CallToolHandler, ToolCallContext, ToolRoute};
use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::json;

#[derive(Clone)]
pub struct AgentMcpTool {
    pub raw_method: AgentMethod,
    pub raw_tool: Tool,
}

impl CallToolHandler<GolemAgentMcpServer, ()> for AgentMcpTool {
    fn call(
        self,
        context: ToolCallContext<'_, GolemAgentMcpServer>,
    ) -> BoxFuture<'_, Result<CallToolResult, ErrorData>> {
        let _arguments: Option<JsonObject> = context.arguments;

        async move {
            Ok(CallToolResult::structured(
                json!({"result": "example output"}),
            ))
        }
        .boxed()
    }
}

impl IntoToolRoute<GolemAgentMcpServer, ()> for AgentMcpTool {
    fn into_tool_route(self) -> ToolRoute<GolemAgentMcpServer> {
        ToolRoute::new(self.raw_tool.clone(), self)
    }
}
