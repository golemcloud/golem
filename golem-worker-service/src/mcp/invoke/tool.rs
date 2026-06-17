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

use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::invoke::agent_method_input::get_agent_method_input;
use crate::mcp::invoke::build_constructor_parameters;
use crate::mcp::invoke::constructor_param_extraction::extract_constructor_input_values;
use crate::mcp::schema::field_name_mapping;
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::AgentId;
use golem_common::base_model::agent::Principal;
use golem_common::model::agent::ParsedAgentId;
use golem_common::schema::adapters::{
    FALLBACK_OUTPUT_FIELD_NAME, multimodal_variant_cases, resolve_ref,
};
use golem_common::schema::agent::OutputSchema;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render::json_value::to_json_value;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::{
    BinaryValuePayload, SchemaValue, TextValuePayload, VariantValuePayload,
};
use rmcp::ErrorData;
use rmcp::model::{
    AnnotateAble, CallToolResult, Content, JsonObject, RawAudioContent, RawContent,
    RawEmbeddedResource, ResourceContents,
};
use serde_json::json;
use std::sync::Arc;

pub async fn invoke_tool(
    args_map: JsonObject,
    mcp_tool: &AgentMcpTool,
    worker_service: &Arc<WorkerService>,
) -> Result<CallToolResult, ErrorData> {
    // The advertised tool schema disambiguates constructor/method parameter
    // names that collide (see `combined_input_schema`). Recompute the same
    // mapping here and translate the advertised argument names back to the
    // original constructor/method field names each extractor expects.
    let field_names = field_name_mapping(&mcp_tool.constructor, &mcp_tool.method);
    let constructor_args = field_names.rewrite_constructor_args(&args_map);
    let method_args = field_names.rewrite_method_args(&args_map);

    let constructor_values = extract_constructor_input_values(
        &constructor_args,
        &mcp_tool.schema_graph,
        &mcp_tool.constructor.input_schema,
    )
    .map_err(|e| {
        tracing::error!("Failed to extract constructor parameters: {}", e);
        ErrorData::invalid_params(format!("Failed to extract constructor parameters: {}", e), None)
    })?;

    let parameters = build_constructor_parameters(
        &mcp_tool.schema_graph,
        &mcp_tool.constructor.input_schema,
        constructor_values,
    );

    let parsed_agent_id = ParsedAgentId::new_auto_phantom(
        mcp_tool.agent_type_name.clone(),
        parameters,
        None,
        mcp_tool.agent_mode,
    )
    .map_err(|e| {
        tracing::error!("Failed to parse agent id: {}", e);
        ErrorData::invalid_params(format!("Failed to parse agent id: {}", e), None)
    })?;

    let method_parameters = get_agent_method_input(
        &method_args,
        &mcp_tool.schema_graph,
        &mcp_tool.method.input_schema,
    )
    .map_err(|e| {
        tracing::error!("Failed to extract method parameters: {}", e);
        ErrorData::invalid_params(format!("Failed to extract method parameters: {}", e), None)
    })?;

    let proto_method_parameters: golem_api_grpc::proto::golem::schema::SchemaValue =
        method_parameters.into();

    let principal = Principal::anonymous();
    let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

    let agent_id = AgentId {
        component_id: mcp_tool.component_id,
        agent_id: parsed_agent_id.to_string(),
    };

    let auth_ctx = golem_service_base::model::auth::AuthCtx::agent(
        mcp_tool.account_id,
        mcp_tool.account_email.clone(),
    );

    let agent_output = worker_service
        .invoke_agent(
            &agent_id,
            Some(mcp_tool.method.name.clone()),
            Some(proto_method_parameters),
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            None,
            None,
            None,
            auth_ctx,
            proto_principal,
            None,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to invoke worker: {:?}", e);
            ErrorData::internal_error(format!("Failed to invoke worker: {:?}", e), None)
        })?;

    let agent_result = match agent_output.result {
        golem_common::model::AgentInvocationResult::AgentMethod { output } => Some(output),
        _ => None,
    };

    match agent_result {
        Some(schema_value) => map_agent_response_to_tool_result(
            &mcp_tool.schema_graph,
            &mcp_tool.method.output_schema,
            schema_value,
        ),
        None => Ok(CallToolResult::success(vec![])),
    }
}

/// Map an agent method's [`SchemaValue`] response into an MCP tool result,
/// typed by the method's [`OutputSchema`] (resolved against `graph`).
///
/// According to the MCP specification, a tool's advertised output schema must be
/// a JSON object, so structured (component-model) single outputs are wrapped
/// under the synthetic [`FALLBACK_OUTPUT_FIELD_NAME`] key (kept in sync with the
/// schema exporter) and returned as `structured_content`. Unstructured
/// (text/binary) and multimodal outputs have no advertised output schema (see
/// `mcp_tool_schema`); they are returned as the MCP `content` array with
/// `structured_content: None`.
pub fn map_agent_response_to_tool_result(
    graph: &SchemaGraph,
    output: &OutputSchema,
    agent_response: SchemaValue,
) -> Result<CallToolResult, ErrorData> {
    let Some(ty) = output.schema() else {
        // Unit output carries no value.
        return Ok(CallToolResult::success(vec![]));
    };

    // Multimodal output: `list<variant<… Role::Multimodal>>`.
    if let Some(cases) = multimodal_variant_cases(graph, ty).map_err(internal_error)? {
        let elements = match agent_response {
            SchemaValue::List { elements } => elements,
            _ => {
                return Err(ErrorData::internal_error(
                    "Expected a multimodal list response".to_string(),
                    None,
                ));
            }
        };

        let mut contents: Vec<Content> = vec![];
        for element in elements {
            let SchemaValue::Variant(VariantValuePayload { case, payload }) = element else {
                return Err(ErrorData::internal_error(
                    "Expected a multimodal variant element".to_string(),
                    None,
                ));
            };
            let case_schema = cases
                .get(case as usize)
                .and_then(|c| c.payload.as_ref())
                .ok_or_else(|| {
                    ErrorData::internal_error(
                        format!("Multimodal case index {case} out of range"),
                        None,
                    )
                })?;
            let payload = payload.ok_or_else(|| {
                ErrorData::internal_error("Multimodal variant has no payload".to_string(), None)
            })?;

            match schema_value_to_tool_result(graph, case_schema, &payload)? {
                ToolResult::Default(json_value) => {
                    contents.push(Content::text(json_value.to_string()));
                }
                ToolResult::Content(content) => contents.push(content),
            }
        }

        return Ok(CallToolResult {
            content: contents,
            structured_content: None,
            is_error: Some(false),
            meta: None,
        });
    }

    match schema_value_to_tool_result(graph, ty, &agent_response)? {
        ToolResult::Default(json_value) => {
            // Wrap in an object keyed by the synthetic output name to match the
            // advertised outputSchema (which must be type: object per MCP spec).
            Ok(CallToolResult::structured(
                json!({ FALLBACK_OUTPUT_FIELD_NAME: json_value }),
            ))
        }
        ToolResult::Content(content) => Ok(CallToolResult {
            content: vec![content],
            structured_content: None,
            is_error: Some(false),
            meta: None,
        }),
    }
}

// https://modelcontextprotocol.io/specification/2025-11-25/server/tools#tool-result
// Unstructured types are part of content-array, but note that it's better of sending `none` outputSchema
// when the types are unstructured.

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ToolResult {
    Default(serde_json::Value),
    Content(Content),
}

fn internal_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

/// Convert a single agent response value, typed by `ty` (resolved against
/// `graph`), into the JSON format expected by MCP clients. Component-model
/// values render through the shared schema-layer JSON codec; `Text` / `Binary`
/// values map onto MCP content blocks. Any changes here must be carefully
/// tested against real MCP clients.
fn schema_value_to_tool_result(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<ToolResult, ErrorData> {
    match resolve_ref(graph, ty) {
        Ok(SchemaType::Text { .. }) => match value {
            SchemaValue::Text(TextValuePayload { text, .. }) => Ok(ToolResult::Content(
                RawContent::text(text.clone()).no_annotation(),
            )),
            _ => Err(ErrorData::internal_error(
                "Expected a text value for a text output".to_string(),
                None,
            )),
        },
        Ok(SchemaType::Binary { .. }) => match value {
            SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }) => {
                Ok(binary_to_tool_content(bytes, mime_type.as_deref().unwrap_or("")))
            }
            _ => Err(ErrorData::internal_error(
                "Expected a binary value for a binary output".to_string(),
                None,
            )),
        },
        _ => to_json_value(graph, ty, value)
            .map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to serialize component model response: {e}"),
                    None,
                )
            })
            .map(ToolResult::Default),
    }
}

fn binary_to_tool_content(data: &[u8], mime_type: &str) -> ToolResult {
    match mime_type {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            ToolResult::Content(RawContent::image(b64, mime_type.to_string()).no_annotation())
        }

        "audio/mpeg" | "audio/wav" | "audio/ogg" => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);
            ToolResult::Content(
                RawContent::Audio(RawAudioContent {
                    data: b64,
                    mime_type: mime_type.to_string(),
                })
                .no_annotation(),
            )
        }

        "text/plain" | "text/csv" | "application/pdf" => {
            let data_str = String::from_utf8_lossy(data).to_string();
            ToolResult::Content(
                RawContent::Resource(RawEmbeddedResource {
                    meta: None,
                    resource: ResourceContents::TextResourceContents {
                        uri: "data:".to_string(),
                        mime_type: Some(mime_type.to_string()),
                        text: data_str,
                        meta: None,
                    },
                })
                .no_annotation(),
            )
        }

        _ => ToolResult::Content(
            RawContent::Resource(RawEmbeddedResource {
                meta: None,
                resource: ResourceContents::BlobResourceContents {
                    uri: "data:".to_string(),
                    mime_type: Some(mime_type.to_string()),
                    blob: base64::engine::general_purpose::STANDARD.encode(data),
                    meta: None,
                },
            })
            .no_annotation(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::agent_mcp_tool::AgentMcpTool;
    use crate::mcp::invoke::test_support::{InvocationHarness, phantom_id};
    use golem_common::base_model::agent::{AgentMode, AgentTypeName};
    use golem_common::model::AgentInvocationOutput;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, NamedField, OutputSchema,
    };
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::metadata::Role;
    use golem_common::schema::schema_type::{
        BinaryRestrictions, SchemaType, TextRestrictions, VariantCaseType,
    };
    use golem_common::schema::{BinaryValuePayload, InputSchema, TextValuePayload};
    use rmcp::model::Tool;
    use serde_json::json;
    use std::borrow::Cow;
    use std::sync::Arc;
    use test_r::test;

    fn graph() -> SchemaGraph {
        SchemaGraph::empty()
    }

    fn str_output() -> OutputSchema {
        OutputSchema::Single(Box::new(SchemaType::string()))
    }

    fn multimodal_output(cases: Vec<(&str, SchemaType)>) -> OutputSchema {
        let variant_cases = cases
            .into_iter()
            .map(|(name, ty)| VariantCaseType {
                name: name.to_string(),
                payload: Some(ty),
                metadata: Default::default(),
            })
            .collect();
        let mut variant = SchemaType::variant(variant_cases);
        variant.metadata_mut().role = Some(Role::Multimodal);
        OutputSchema::Single(Box::new(SchemaType::list(variant)))
    }

    #[test]
    fn tuple_single_component_model_to_structured_json() {
        let response = SchemaValue::String("hello".to_string());
        let result = map_agent_response_to_tool_result(&graph(), &str_output(), response).unwrap();
        assert_eq!(result.structured_content, Some(json!({"value": "hello"})));
        assert_eq!(result.is_error, Some(false));
    }

    #[test]
    fn tuple_empty_returns_success() {
        let response = SchemaValue::Tuple { elements: vec![] };
        let result =
            map_agent_response_to_tool_result(&graph(), &OutputSchema::Unit, response).unwrap();
        assert!(result.content.is_empty());
        assert_eq!(result.is_error, Some(false));
    }

    #[test]
    fn tuple_text_element_to_data_object() {
        let output = OutputSchema::Single(Box::new(SchemaType::text(TextRestrictions::default())));
        let response = SchemaValue::Text(TextValuePayload {
            text: "weather is sunny".to_string(),
            language: Some("en".to_string()),
        });
        let result = map_agent_response_to_tool_result(&graph(), &output, response).unwrap();

        let raw_content = &result.content[0].raw;
        assert_eq!(raw_content, &RawContent::text("weather is sunny"));
    }

    #[test]
    fn tuple_binary_element_to_base64_object() {
        let output =
            OutputSchema::Single(Box::new(SchemaType::binary(BinaryRestrictions::default())));
        let response = SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![1, 2, 3],
            mime_type: Some("image/png".to_string()),
        });
        let result = map_agent_response_to_tool_result(&graph(), &output, response).unwrap();

        let raw_content = &result.content[0].raw;
        assert_eq!(
            raw_content,
            &RawContent::image("AQID", "image/png".to_string())
        );
    }

    #[test]
    fn multimodal_response_to_parts_array() {
        let output = multimodal_output(vec![
            ("desc", SchemaType::text(TextRestrictions::default())),
            ("photo", SchemaType::binary(BinaryRestrictions::default())),
        ]);
        let response = SchemaValue::List {
            elements: vec![
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(SchemaValue::Text(TextValuePayload {
                        text: "a photo".to_string(),
                        language: None,
                    }))),
                }),
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(SchemaValue::Binary(BinaryValuePayload {
                        bytes: vec![1, 2, 3],
                        mime_type: Some("image/png".to_string()),
                    }))),
                }),
            ],
        };
        let result = map_agent_response_to_tool_result(&graph(), &output, response).unwrap();
        let contents = &result.content;

        assert_eq!(contents.len(), 2);
        assert_eq!(&contents[0].raw, &RawContent::text("a photo"));
        assert_eq!(
            &contents[1].raw,
            &RawContent::image("AQID", "image/png".to_string())
        );
    }

    #[test]
    async fn invoke_tool_auto_generates_phantom_for_ephemeral_agents() {
        let harness = InvocationHarness::new(AgentInvocationOutput {
            result: golem_common::model::AgentInvocationResult::AgentInitialization,
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            oplog_index: None,
            agent_fingerprint: None,
        });
        let tool = AgentMcpTool {
            tool: Tool {
                name: Cow::Borrowed("mcp-agent-run"),
                title: None,
                description: None,
                input_schema: Arc::new(JsonObject::default()),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            },
            environment_id: harness.environment_id,
            account_id: harness.account_id,
            schema_graph: Arc::new(SchemaGraph::empty()),
            account_email: golem_common::model::account::AccountEmail::new("mcp@golem"),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            method: AgentMethodSchema {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            },
            component_id: harness.component_id,
            agent_type_name: AgentTypeName("mcp-agent".to_string()),
            agent_mode: AgentMode::Ephemeral,
        };

        let result = invoke_tool(JsonObject::default(), &tool, &harness.worker_service).await;

        assert!(result.is_ok());
        let agent_id = harness.recorded_agent_id();
        assert_eq!(agent_id.component_id, harness.component_id);
        assert!(phantom_id(&agent_id).is_some());
    }

    #[test]
    async fn invoke_tool_routes_disambiguated_args_to_each_side() {
        // Constructor and method both declare a user-supplied `id`, so the
        // advertised tool schema disambiguates them to `constructor_id` /
        // `method_id`. The invoke path must translate those advertised names
        // back and route each value to the correct side (different types make
        // a swap observable: constructor = string, method = u32).
        let harness = InvocationHarness::new(AgentInvocationOutput {
            result: golem_common::model::AgentInvocationResult::AgentMethod {
                output: SchemaValue::Tuple { elements: vec![] },
            },
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            oplog_index: None,
            agent_fingerprint: None,
        });
        let tool = AgentMcpTool {
            tool: Tool {
                name: Cow::Borrowed("mcp-agent-run"),
                title: None,
                description: None,
                input_schema: Arc::new(JsonObject::default()),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            },
            environment_id: harness.environment_id,
            account_id: harness.account_id,
            account_email: harness.account_email.clone(),
            schema_graph: Arc::new(SchemaGraph::empty()),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "id",
                    SchemaType::string(),
                )]),
            },
            method: AgentMethodSchema {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "id",
                    SchemaType::u32(),
                )]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            },
            component_id: harness.component_id,
            agent_type_name: AgentTypeName("mcp-agent".to_string()),
            agent_mode: AgentMode::Ephemeral,
        };

        let args = json!({ "constructor_id": "abc", "method_id": 7 })
            .as_object()
            .unwrap()
            .clone();

        let result = invoke_tool(args, &tool, &harness.worker_service).await;
        assert!(result.is_ok(), "invoke failed: {result:?}");

        // The method received the u32 value, not the constructor's string.
        let method_params = harness.recorded_method_params();
        assert_eq!(
            method_params,
            SchemaValue::Record {
                fields: vec![SchemaValue::U32(7)]
            }
        );

        // The constructor received the string value: it is encoded into the
        // generated agent id.
        let agent_id = harness.recorded_agent_id();
        assert!(
            agent_id.agent_id.contains("abc"),
            "constructor value missing from agent id: {}",
            agent_id.agent_id
        );
    }
}
