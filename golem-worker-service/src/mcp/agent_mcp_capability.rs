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

use std::borrow::Cow;
use std::sync::Arc;
use rmcp::model::Tool;
use golem_common::base_model::agent::{AgentMethod, DataSchema};
use crate::mcp::agent_mcp_resource::AgentMcpResource;
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::mcp_schema::{McpToolSchema, McpToolGetSchema};

#[derive(Clone)]
pub enum McpAgentCapability {
    Tool(AgentMcpTool),
    Resource(AgentMcpResource),
}

impl McpAgentCapability {
    pub fn from(method: AgentMethod) -> Self {
        match &method.input_schema {
            DataSchema::Tuple(schemas) => {
                if schemas.elements.len() > 0 {
                    let McpToolSchema {input_schema, output_schema} = method.get_schema();
                    
                    let tool = Tool {
                        name: Cow::from(method.name.clone()),
                        title: None,
                        description: Some("An increment method that takes a number and increment it".into()),
                        input_schema: Arc::new(input_schema),
                        output_schema: output_schema.map(Arc::new),
                        annotations: None,
                        execution: None,
                        icons: None,
                        meta: None,
                    };
                    
                    Self::Tool(AgentMcpTool { raw_method: method, raw_tool: tool })
                    
                } else {
                    Self::Resource(AgentMcpResource { resource: method })
                }
            }
            DataSchema::Multimodal(_) => {
                todo!("Multimodal schema handling not implemented yet")
            }
        }
    }
}
