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

use crate::mcp::agent_mcp_resource::{AgentMcpResource, AgentMcpResourceKind};
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::schema::{McpToolSchema, get_mcp_schema, get_mcp_tool_schema};
use golem_common::base_model::account::AccountId;
use golem_common::base_model::agent::{AgentMethod, AgentTypeName, DataSchema};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::AgentConstructor;
use rmcp::model::{Annotated, RawResource, RawResourceTemplate, Tool};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone)]
pub enum McpAgentCapability {
    Tool(Box<AgentMcpTool>),
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
                    tracing::debug!(
                        "Method {} of agent type {} has input parameters, exposing as tool",
                        method.name,
                        agent_type_name.0
                    );

                    let constructor_schema = get_mcp_schema(&constructor.input_schema);

                    let McpToolSchema {
                        mut input_schema,
                        output_schema,
                    } = get_mcp_tool_schema(method);

                    input_schema.prepend_schema(constructor_schema);

                    let tool = Tool {
                        name: Cow::from(get_tool_name(agent_type_name, method)),
                        title: None,
                        description: Some(method.description.clone().into()),
                        input_schema: Arc::new(rmcp::model::JsonObject::from(input_schema)),
                        output_schema: output_schema
                            .map(|internal| Arc::new(rmcp::model::JsonObject::from(internal))),
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
                    tracing::debug!(
                        "Method {} of agent type {} has no input parameters, exposing as resource",
                        method.name,
                        agent_type_name.0
                    );

                    let constructor_param_names =
                        AgentMcpResource::constructor_param_names(constructor);
                    let name = AgentMcpResource::resource_name(agent_type_name, method);

                    let kind = if constructor_param_names.is_empty() {
                        let uri = AgentMcpResource::static_uri(agent_type_name, method);
                        AgentMcpResourceKind::Static(Annotated::new(
                            RawResource {
                                uri,
                                name,
                                title: None,
                                description: Some(method.description.clone()),
                                mime_type: Some("application/json".to_string()),
                                size: None,
                                icons: None,
                                meta: None,
                            },
                            None,
                        ))
                    } else {
                        let uri_template = AgentMcpResource::template_uri(
                            agent_type_name,
                            method,
                            &constructor_param_names,
                        );
                        AgentMcpResourceKind::Template {
                            template: Annotated::new(
                                RawResourceTemplate {
                                    uri_template,
                                    name,
                                    title: None,
                                    description: Some(method.description.clone()),
                                    mime_type: Some("application/json".to_string()),
                                    icons: None,
                                },
                                None,
                            ),
                            constructor_param_names,
                        }
                    };

                    Self::Resource(AgentMcpResource {
                        kind,
                        environment_id: *environment_id,
                        account_id: *account_id,
                        constructor: constructor.clone(),
                        raw_method: method.clone(),
                        component_id,
                        agent_type_name: agent_type_name.clone(),
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
