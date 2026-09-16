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
use golem_common::base_model::account::{AccountEmail, AccountId};
use golem_common::base_model::agent::{AgentMode, AgentTypeName};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, FieldSource, OutputSchema,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::multimodal::is_multimodal_schema_type;
use golem_common::schema::unstructured::{UnstructuredPayloadKind, unstructured_or_raw_kind};
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
    /// would always fail at invoke time: streaming methods are rejected, and the
    /// constructor schema is checked to be supplyable via MCP (no multimodal /
    /// unstructured constructor parameters), resolving `SchemaType::Ref`s
    /// against `schema_graph`.
    #[allow(clippy::too_many_arguments)]
    pub fn from_agent_method(
        account_id: &AccountId,
        account_email: &AccountEmail,
        environment_id: &EnvironmentId,
        agent_type_name: &AgentTypeName,
        agent_mode: AgentMode,
        schema_graph: Arc<SchemaGraph>,
        method: &AgentMethodSchema,
        constructor: &AgentConstructorSchema,
        component_id: ComponentId,
    ) -> anyhow::Result<Self> {
        if method.uses_streams(&schema_graph) {
            anyhow::bail!(
                "streaming method {} of agent type {} cannot be exported through MCP",
                method.name,
                agent_type_name.0
            );
        }

        validate_constructor_schema_for_mcp(&schema_graph, &constructor.input_schema).map_err(
            |e| {
                anyhow::anyhow!(
                    "constructor of agent type {} cannot be supplied via MCP: {}",
                    agent_type_name.0,
                    e
                )
            },
        )?;

        let has_user_input = method
            .input_schema
            .fields()
            .iter()
            .any(|f| matches!(f.source, FieldSource::UserSupplied));

        if has_user_input || method.read_only.is_none() {
            tracing::debug!(
                "Exposing method {} of agent type {} as a tool",
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
                account_email: account_email.clone(),
                constructor: constructor.clone(),
                method: method.clone(),
                tool,
                component_id,
                agent_type_name: agent_type_name.clone(),
                agent_mode,
            })))
        } else {
            tracing::debug!(
                "Method {} of agent type {} is read-only and has no input parameters, exposing as resource",
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
                account_email: account_email.clone(),
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
    // so `is_multimodal_schema_type` / `unstructured_or_raw_kind` here cannot
    // mask a real dangling/recursive ref; the fallbacks only guard truly
    // unreachable cases.
    if is_multimodal_schema_type(graph, ty).unwrap_or(false) {
        return None;
    }
    // Canonical unstructured wrappers advertise the same MIME type as the raw
    // text/binary rich scalars they wrap.
    match unstructured_or_raw_kind(graph, ty) {
        Ok(Some(UnstructuredPayloadKind::Text)) => Some("text/plain".to_string()),
        Ok(Some(UnstructuredPayloadKind::Binary)) => None,
        Ok(None) => Some("application/json".to_string()),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::agent::{CachePolicy, ReadOnlyConfig};
    use golem_common::model::Empty;
    use golem_common::schema::agent::{AutoInjectedKind, InputSchema, NamedField};
    use golem_common::schema::schema_type::SchemaType;
    use test_r::test;

    fn constructor() -> AgentConstructorSchema {
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        }
    }

    fn method(input: Vec<NamedField>, output: OutputSchema, read_only: bool) -> AgentMethodSchema {
        AgentMethodSchema {
            name: "method".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(input),
            output_schema: output,
            http_endpoint: vec![],
            read_only: read_only.then_some(ReadOnlyConfig {
                cache_policy: CachePolicy::NoCache(Empty {}),
                uses_principal: false,
            }),
        }
    }

    fn capability(
        graph: SchemaGraph,
        method: &AgentMethodSchema,
    ) -> anyhow::Result<McpAgentCapability> {
        capability_with_constructor(graph, method, &constructor())
    }

    fn capability_with_constructor(
        graph: SchemaGraph,
        method: &AgentMethodSchema,
        constructor: &AgentConstructorSchema,
    ) -> anyhow::Result<McpAgentCapability> {
        McpAgentCapability::from_agent_method(
            &AccountId::new(),
            &AccountEmail::new("mcp@golem"),
            &EnvironmentId::new(),
            &AgentTypeName("TestAgent".to_string()),
            AgentMode::Durable,
            Arc::new(graph),
            method,
            constructor,
            ComponentId::new(),
        )
    }

    #[test]
    fn parameterless_read_only_method_is_a_resource() {
        let capability = capability(
            SchemaGraph::empty(),
            &method(vec![], OutputSchema::Unit, true),
        )
        .unwrap();

        assert!(matches!(capability, McpAgentCapability::Resource(_)));
    }

    #[test]
    fn parameterless_non_read_only_method_is_a_tool() {
        let capability = capability(
            SchemaGraph::empty(),
            &method(vec![], OutputSchema::Unit, false),
        )
        .unwrap();

        assert!(matches!(capability, McpAgentCapability::Tool(_)));
    }

    #[test]
    fn read_only_method_with_user_input_is_a_tool() {
        let capability = capability(
            SchemaGraph::empty(),
            &method(
                vec![NamedField::user_supplied("value", SchemaType::string())],
                OutputSchema::Unit,
                true,
            ),
        )
        .unwrap();

        assert!(matches!(capability, McpAgentCapability::Tool(_)));
    }

    #[test]
    fn read_only_method_with_only_auto_injected_input_is_a_resource() {
        let capability = capability(
            SchemaGraph::empty(),
            &method(
                vec![NamedField::auto_injected(
                    "principal",
                    AutoInjectedKind::Principal,
                    SchemaType::string(),
                )],
                OutputSchema::Unit,
                true,
            ),
        )
        .unwrap();

        assert!(matches!(capability, McpAgentCapability::Resource(_)));
    }

    #[test]
    fn auto_injected_constructor_input_keeps_resource_static() {
        let constructor = AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::auto_injected(
                "principal",
                AutoInjectedKind::Principal,
                SchemaType::string(),
            )]),
        };
        let capability = capability_with_constructor(
            SchemaGraph::empty(),
            &method(vec![], OutputSchema::Unit, true),
            &constructor,
        )
        .unwrap();

        assert!(matches!(
            capability,
            McpAgentCapability::Resource(resource)
                if matches!(resource.kind, AgentMcpResourceKind::Static(_))
        ));
    }

    #[test]
    fn method_with_input_stream_is_not_exported() {
        let error = capability(
            SchemaGraph::empty(),
            &method(
                vec![NamedField::user_supplied(
                    "values",
                    SchemaType::stream(Some(SchemaType::string())),
                )],
                OutputSchema::Unit,
                false,
            ),
        )
        .err()
        .expect("streaming method should be rejected");

        assert!(error.to_string().contains("cannot be exported through MCP"));
    }

    #[test]
    fn method_with_output_stream_is_not_exported() {
        let error = capability(
            SchemaGraph::empty(),
            &method(
                vec![],
                OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::string())))),
                true,
            ),
        )
        .err()
        .expect("streaming method should be rejected");

        assert!(error.to_string().contains("cannot be exported through MCP"));
    }
}
