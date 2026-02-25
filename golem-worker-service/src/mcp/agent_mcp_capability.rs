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

use crate::mcp::agent_mcp_resource::AgentMcpResource;
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::mcp_schema::{GetMcpSchema, GetMcpToolSchema, McpToolSchema};
use golem_common::base_model::account::AccountId;
use golem_common::base_model::agent::{AgentMethod, AgentTypeName, DataSchema};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::AgentConstructor;
use rmcp::model::Tool;
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone)]
pub enum McpAgentCapability {
    Tool(Box<AgentMcpTool>),
    #[allow(unused)]
    Resource(AgentMcpResource),
}

impl McpAgentCapability {
    pub fn from(
        account_id: &AccountId,
        environment_id: &EnvironmentId,
        agent_type_name: &AgentTypeName,
        method: &AgentMethod,
        constructor: &AgentConstructor,
        component_id: ComponentId,
    ) -> Self {
        match &method.input_schema {
            DataSchema::Tuple(schemas) => {
                if !schemas.elements.is_empty() {
                    let constructor_schema = constructor.input_schema.get_mcp_schema();
                    let mut tool_schema = method.get_mcp_tool_schema();
                    tool_schema.merge_input_schema(constructor_schema);

                    let McpToolSchema {
                        input_schema,
                        output_schema,
                    } = method.get_mcp_tool_schema();

                    let tool = Tool {
                        name: Cow::from(get_tool_name(agent_type_name, method)),
                        title: None,
                        description: Some(method.description.clone().into()),
                        input_schema: Arc::new(input_schema),
                        output_schema: output_schema.map(Arc::new),
                        annotations: None,
                        execution: None,
                        icons: None,
                        meta: None,
                    };

                    Self::Tool(Box::new(AgentMcpTool {
                        environment_id: *environment_id,
                        account_id: *account_id,
                        constructor: constructor.clone(),
                        raw_method: method.clone(),
                        tool,
                        component_id,
                        agent_type_name: agent_type_name.clone(),
                    }))
                } else {
                    Self::Resource(AgentMcpResource {
                        resource: method.clone(),
                    })
                }
            }
            DataSchema::Multimodal(_) => {
                todo!("Multimodal schema handling not implemented yet")
            }
        }
    }
}

fn get_tool_name(agent_type_name: &AgentTypeName, method: &AgentMethod) -> String {
    format!("{}-{}", agent_type_name.0, method.name)
}
