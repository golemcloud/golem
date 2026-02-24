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
use golem_common::base_model::agent::{AgentConstructor, AgentMethod, AgentTypeName};
use golem_common::base_model::component::ComponentId;
use rmcp::{ErrorData};
use rmcp::handler::server::router::tool::IntoToolRoute;
use rmcp::handler::server::tool::{CallToolHandler, ToolCallContext, ToolRoute};
use rmcp::model::{CallToolResult, Tool};

#[derive(Clone)]
pub struct AgentMcpTool {
    pub constructor: AgentConstructor,
    pub raw_method: AgentMethod,
    pub raw_tool: Tool,
    pub component_id: ComponentId,
    pub agent_type_name: AgentTypeName,
}

impl CallToolHandler<GolemAgentMcpServer, ()> for AgentMcpTool {
    fn call(
        self,
        context: ToolCallContext<'_, GolemAgentMcpServer>,
    ) -> BoxFuture<'_, Result<CallToolResult, ErrorData>> {
        async move {
            context.service.invoke(
                context.arguments.unwrap_or_default(),
                &self
            ).await
        }
        .boxed()
    }
}

impl IntoToolRoute<GolemAgentMcpServer, ()> for AgentMcpTool {
    fn into_tool_route(self) -> ToolRoute<GolemAgentMcpServer> {
        ToolRoute::new(self.raw_tool.clone(), self)
    }
}
