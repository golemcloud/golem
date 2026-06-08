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
use crate::mcp::invoke::constructor_param_extraction::validate_constructor_schema_for_mcp;
use crate::mcp::schema::{McpToolSchema, get_mcp_tool_schema};
use anyhow::Context;
use golem_common::base_model::account::AccountId;
use golem_common::base_model::agent::{AgentMode, AgentTypeName};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::schema::adapters::{
    is_multimodal_schema_type, resolve_ref, schema_agent_constructor_to_legacy,
    schema_agent_method_to_legacy,
};
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, FieldSource, OutputSchema,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use rmcp::model::{Annotated, RawResource, RawResourceTemplate, Tool};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone)]
pub enum McpAgentCapability {
    Tool(Box<AgentMcpTool>),
    Resource(Box<AgentMcpResource>),
}

impl McpAgentCapability {
    /// Build an MCP tool or resource capability for a single agent method.
    ///
    /// Performs export-time validation so we never advertise a capability that
    /// would always fail at invoke time: the constructor and method schemas are
    /// projected back to the legacy invoke carriers here (the same projection
    /// the invoke path performs per call), which resolves every `SchemaType::Ref`
    /// against `schema_graph` and rejects schema-only constructs. This also
    /// guarantees that the (infallible) JSON Schema rendering below operates on
    /// fully-resolvable refs, so the downstream `unwrap_or(false)` ref-classifier
    /// calls cannot silently swallow a dangling/recursive ref.
    #[allow(clippy::too_many_arguments)]
    pub fn from_agent_method(
        account_id: &AccountId,
        environment_id: &EnvironmentId,
        agent_type_name: &AgentTypeName,
        agent_mode: AgentMode,
        schema_graph: Arc<SchemaGraph>,
        method: &AgentMethodSchema,
        constructor: &AgentConstructorSchema,
        component_id: ComponentId,
    ) -> anyhow::Result<Self> {
        let legacy_constructor = schema_agent_constructor_to_legacy(&schema_graph, constructor)
            .with_context(|| {
                format!(
                    "constructor of agent type {} is not projectable to the MCP invoke model",
                    agent_type_name.0
                )
            })?;
        schema_agent_method_to_legacy(&schema_graph, method).with_context(|| {
            format!(
                "method {} of agent type {} is not projectable to the MCP invoke model",
                method.name, agent_type_name.0
            )
        })?;
        validate_constructor_schema_for_mcp(&legacy_constructor.input_schema).map_err(|e| {
            anyhow::anyhow!(
                "constructor of agent type {} cannot be supplied via MCP: {}",
                agent_type_name.0,
                e
            )
        })?;

        let has_user_input = method
            .input_schema
            .fields()
            .iter()
            .any(|f| matches!(f.source, FieldSource::UserSupplied));

        if has_user_input {
            tracing::debug!(
                "Method {} of agent type {} has input parameters, exposing as tool",
                method.name,
                agent_type_name.0
            );

            let McpToolSchema {
                input_schema,
                output_schema,
            } = get_mcp_tool_schema(&schema_graph, constructor, method);

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

            Ok(Self::Tool(Box::new(AgentMcpTool {
                environment_id: *environment_id,
                account_id: *account_id,
                schema_graph,
                constructor: constructor.clone(),
                method: method.clone(),
                tool,
                component_id,
                agent_type_name: agent_type_name.clone(),
                agent_mode,
            })))
        } else {
            tracing::debug!(
                "Method {} of agent type {} has no input parameters, exposing as resource",
                method.name,
                agent_type_name.0
            );

            let constructor_param_names = AgentMcpResource::constructor_param_names(constructor);
            let name = AgentMcpResource::resource_name(agent_type_name, method);

            let mime_type = output_resource_mime_type(&schema_graph, &method.output_schema);

            let kind = if constructor_param_names.is_empty() {
                let uri = AgentMcpResource::static_uri(agent_type_name, method);
                AgentMcpResourceKind::Static(Annotated::new(
                    RawResource {
                        uri,
                        name,
                        title: None,
                        description: Some(method.description.clone()),
                        mime_type,
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
                            mime_type,
                            icons: None,
                        },
                        None,
                    ),
                    constructor_param_names,
                }
            };

            Ok(Self::Resource(Box::new(AgentMcpResource {
                kind,
                environment_id: *environment_id,
                account_id: *account_id,
                schema_graph,
                constructor: constructor.clone(),
                method: method.clone(),
                component_id,
                agent_type_name: agent_type_name.clone(),
                agent_mode,
            })))
        }
    }
}

fn get_tool_name(agent_type_name: &AgentTypeName, method: &AgentMethodSchema) -> String {
    format!("{}-{}", agent_type_name.0, method.name)
}

/// MIME type advertised for a method exposed as an MCP resource.
///
/// - structured (component-model) single output → `application/json`
/// - unstructured text output → `text/plain`
/// - unstructured binary output → `None` (the actual MIME type is only known
///   at response time)
/// - multimodal / unit output → `None` (no single MIME type applies)
fn output_resource_mime_type(graph: &SchemaGraph, output: &OutputSchema) -> Option<String> {
    let OutputSchema::Single(ty) = output else {
        return None;
    };
    // Refs are pre-validated in `from_agent_method` (via the legacy projection),
    // so `is_multimodal_schema_type` / `resolve_ref` here cannot mask a real
    // dangling/recursive ref; the fallbacks only guard truly unreachable cases.
    if is_multimodal_schema_type(graph, ty).unwrap_or(false) {
        return None;
    }
    match resolve_ref(graph, ty) {
        Ok(SchemaType::Text { .. }) => Some("text/plain".to_string()),
        Ok(SchemaType::Binary { .. }) => None,
        Ok(_) => Some("application/json".to_string()),
        Err(_) => None,
    }
}
