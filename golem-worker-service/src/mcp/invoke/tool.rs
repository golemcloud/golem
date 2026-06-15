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
use crate::mcp::invoke::constructor_param_extraction::extract_constructor_input_values;
use crate::mcp::schema::field_name_mapping;
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::AgentId;
use golem_common::base_model::agent::*;
use golem_common::model::agent::LegacyParsedAgentId;
use golem_common::schema::SchemaValue;
use golem_common::schema::adapters::{
    schema_agent_constructor_to_legacy, schema_agent_method_to_legacy,
    schema_output_value_to_legacy_data_value,
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
    // The MCP capability stores schema-layer constructor/method bodies. The
    // invoke/runtime extraction code still operates on the legacy
    // `DataSchema` / `DataValue` carriers, so convert at this boundary,
    // resolving any `SchemaType::Ref` against the agent's schema graph.
    let legacy_constructor = schema_agent_constructor_to_legacy(
        &mcp_tool.schema_graph,
        &mcp_tool.constructor,
    )
    .map_err(|e| {
        tracing::error!("Failed to convert constructor schema: {}", e);
        ErrorData::internal_error(format!("Failed to convert constructor schema: {}", e), None)
    })?;
    let legacy_method = schema_agent_method_to_legacy(&mcp_tool.schema_graph, &mcp_tool.method)
        .map_err(|e| {
            tracing::error!("Failed to convert method schema: {}", e);
            ErrorData::internal_error(format!("Failed to convert method schema: {}", e), None)
        })?;

    // The advertised tool schema disambiguates constructor/method parameter
    // names that collide (see `combined_input_schema`). Recompute the same
    // mapping here and translate the advertised argument names back to the
    // original constructor/method field names each extractor expects.
    let field_names = field_name_mapping(&mcp_tool.constructor, &mcp_tool.method);
    let constructor_args = field_names.rewrite_constructor_args(&args_map);
    let method_args = field_names.rewrite_method_args(&args_map);

    let constructor_params =
        extract_constructor_input_values(&constructor_args, &legacy_constructor.input_schema)
            .map_err(|e| {
                tracing::error!("Failed to extract constructor parameters: {}", e);
                ErrorData::invalid_params(
                    format!("Failed to extract constructor parameters: {}", e),
                    None,
                )
            })?;

    let parsed_agent_id = LegacyParsedAgentId::new_auto_phantom(
        mcp_tool.agent_type_name.clone(),
        DataValue::Tuple(ElementValues {
            elements: constructor_params
                .into_iter()
                .map(ElementValue::ComponentModel)
                .collect(),
        }),
        None,
        mcp_tool.agent_mode,
    )
    .map_err(|e| {
        tracing::error!("Failed to parse agent id: {}", e);
        ErrorData::invalid_params(format!("Failed to parse agent id: {}", e), None)
    })?;

    let method_parameters = get_agent_method_input(&method_args, &legacy_method.input_schema)
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
            Some(legacy_method.name.clone()),
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
        Some(schema_value) => {
            map_agent_response_to_tool_result(schema_value, &legacy_method.output_schema)
        }
        None => Ok(CallToolResult::success(vec![])),
    }
}

pub fn map_agent_response_to_tool_result(
    agent_response: SchemaValue,
    expected_type: &DataSchema,
) -> Result<CallToolResult, ErrorData> {
    let typed_value = schema_output_value_to_legacy_data_value(agent_response, expected_type)
        .map_err(|error| {
            ErrorData::internal_error(format!("Agent response type mismatch: {error}"), None)
        })?;

    // According to the MCP specification, a tool's advertised output schema must
    // be a JSON object, so structured (component-model) single outputs are
    // wrapped under the synthetic output key and returned as `structured_content`.
    // Unstructured (text/binary) and multimodal outputs have no advertised output
    // schema (see `mcp_tool_schema`); they are returned as the MCP `content` array
    // with `structured_content: None`. See `convert_elem_value_to_mcp_tool_response`.
    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(CallToolResult::success(vec![])),
            1 => {
                let element_name = match expected_type {
                    DataSchema::Tuple(NamedElementSchemas { elements: schemas }) => {
                        schemas.first().map(|s| s.name.clone())
                    }
                    _ => None,
                };

                let element = elements.into_iter().next().unwrap();
                let too_result = convert_elem_value_to_mcp_tool_response(&element)?;

                match too_result {
                    ToolResult::Default(value) => {
                        let json_value = value;
                        // Wrap in an object keyed by the schema element name to match the
                        // advertised outputSchema (which must be type: object per MCP spec).
                        let structured = match element_name {
                            Some(name) => json!({ name: json_value }),
                            None => json_value,
                        };

                        // Both contents and structured fields are populated here (apparently)
                        Ok(CallToolResult::structured(structured))
                    }
                    ToolResult::Content(content) => {
                        // For content results, we put the content in the "content" field of the tool result,
                        // and still provide the structured JSON for the rest of the schema (if any).
                        Ok(CallToolResult {
                            content: vec![content],
                            structured_content: None,
                            is_error: Some(false),
                            meta: None,
                        })
                    }
                }
            }
            _ => Err(ErrorData::internal_error(
                "Unexpected number of response tuple elements".to_string(),
                None,
            )),
        },

        // multimodal
        DataValue::Multimodal(NamedElementValues { elements }) => {
            let mut contents: Vec<Content> = vec![];

            for named in elements {
                let tool_result = convert_elem_value_to_mcp_tool_response(&named.value)?;

                match tool_result {
                    ToolResult::Default(json_value) => {
                        contents.push(Content::text(json_value.to_string()));
                    }

                    // Mostly multimodal is a collection of binary or unstructured text data
                    ToolResult::Content(content) => {
                        contents.push(content);
                    }
                }
            }

            Ok(CallToolResult {
                content: contents,
                structured_content: None,
                is_error: Some(false),
                meta: None,
            })
        }
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

// Mapping from ElementValue to the JSON format expected by MCP clients
// (based on the schema they learned from initialization)
// This is used only for tools, and not for resources.
// Any changes in this mapping should be carefully tested with actual MCP clients
fn convert_elem_value_to_mcp_tool_response(
    element: &ElementValue,
) -> Result<ToolResult, ErrorData> {
    match element {
        ElementValue::ComponentModel(component_model_value) => {
            crate::mcp::invoke::component_model_value_to_json(&component_model_value.value)
                .map_err(|e| {
                    ErrorData::internal_error(
                        format!("Failed to serialize component model response: {e}"),
                        None,
                    )
                })
                .map(ToolResult::Default)
        }

        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => match value {
            TextReference::Inline(TextSource { data, .. }) => Ok(ToolResult::Content(
                RawContent::text(data.clone()).no_annotation(),
            )),
            TextReference::Url(_) => Err(ErrorData::internal_error(
                "A text reference URL can only be part of tool input and not output".to_string(),
                None,
            )),
        },

        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    let mime_type = binary_type.mime_type.as_str();

                    match mime_type {
                        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(data);

                            Ok(ToolResult::Content(
                                RawContent::image(b64, mime_type.to_string()).no_annotation(),
                            ))
                        }

                        "audio/mpeg" | "audio/wav" | "audio/ogg" => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(data);

                            Ok(ToolResult::Content(
                                RawContent::Audio(RawAudioContent {
                                    data: b64,
                                    mime_type: mime_type.to_string(),
                                })
                                .no_annotation(),
                            ))
                        }

                        "text/plain" | "text/csv" | "application/pdf" => {
                            let data_str = String::from_utf8_lossy(data).to_string();
                            Ok(ToolResult::Content(
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
                            ))
                        }

                        _ => Ok(ToolResult::Content(
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
                        )),
                    }
                }
                BinaryReference::Url(_) => Err(ErrorData::internal_error(
                    "A binary reference URL can only be part of tool input and not output"
                        .to_string(),
                    None,
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::agent_mcp_tool::AgentMcpTool;
    use crate::mcp::invoke::test_support::{InvocationHarness, phantom_id};
    use golem_common::base_model::agent::{
        AgentMode, AgentTypeName, BinaryDescriptor, ComponentModelElementSchema, DataSchema,
        ElementSchema, NamedElementSchema, NamedElementSchemas, TextDescriptor, Url,
    };
    use golem_common::model::AgentInvocationOutput;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, NamedField, OutputSchema,
    };
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::schema_type::SchemaType;
    use golem_common::schema::{
        BinaryValuePayload, InputSchema, TextValuePayload, VariantValuePayload,
    };
    use golem_wasm::analysis::{AnalysedType, TypeStr};
    use rmcp::model::Tool;
    use serde_json::json;
    use std::borrow::Cow;
    use std::sync::Arc;
    use test_r::test;

    fn str_output_schema() -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "result".to_string(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: AnalysedType::Str(TypeStr),
                }),
            }],
        })
    }

    #[test]
    fn tuple_single_component_model_to_structured_json() {
        let response = SchemaValue::String("hello".to_string());
        let result = map_agent_response_to_tool_result(response, &str_output_schema()).unwrap();
        assert_eq!(result.structured_content, Some(json!({"result": "hello"})));
        assert_eq!(result.is_error, Some(false));
    }

    #[test]
    fn tuple_empty_returns_success() {
        let schema = DataSchema::Tuple(NamedElementSchemas { elements: vec![] });
        let response = SchemaValue::Tuple { elements: vec![] };
        let result = map_agent_response_to_tool_result(response, &schema).unwrap();
        assert!(result.content.is_empty());
        assert_eq!(result.is_error, Some(false));
    }

    #[test]
    fn tuple_text_element_to_data_object() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "report".to_string(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            }],
        });
        let response = SchemaValue::Text(TextValuePayload {
            text: "weather is sunny".to_string(),
            language: Some("en".to_string()),
        });
        let result = map_agent_response_to_tool_result(response, &schema).unwrap();

        let raw_content = &result.content[0].raw;

        assert_eq!(raw_content, &RawContent::text("weather is sunny"));
    }

    #[test]
    fn tuple_binary_element_to_base64_object() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "image".to_string(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
            }],
        });

        let response = SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![1, 2, 3],
            mime_type: Some("image/png".to_string()),
        });
        let result = map_agent_response_to_tool_result(response, &schema).unwrap();

        let raw_content = &result.content[0].raw;

        assert_eq!(
            raw_content,
            &RawContent::image("AQID", "image/png".to_string())
        );
    }

    #[test]
    fn multimodal_response_to_parts_array() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "desc".to_string(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                },
                NamedElementSchema {
                    name: "photo".to_string(),
                    schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                        restrictions: None,
                    }),
                },
            ],
        });
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
        let result = map_agent_response_to_tool_result(response, &schema).unwrap();
        let contents = &result.content;

        assert_eq!(contents.len(), 2);

        assert_eq!(&contents[0].raw, &RawContent::text("a photo"));
        assert_eq!(
            &contents[1].raw,
            &RawContent::image("AQID", "image/png".to_string())
        );
    }

    #[test]
    fn error_on_text_url_reference() {
        let elem = ElementValue::UnstructuredText(UnstructuredTextElementValue {
            value: TextReference::Url(Url {
                value: "https://example.com".to_string(),
            }),
            descriptor: TextDescriptor { restrictions: None },
        });
        let err = convert_elem_value_to_mcp_tool_response(&elem).unwrap_err();
        assert!(err.message.contains("URL"), "got: {}", err.message);
    }

    #[test]
    fn error_on_binary_url_reference() {
        let elem = ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
            value: BinaryReference::Url(Url {
                value: "https://example.com/img.png".to_string(),
            }),
            descriptor: BinaryDescriptor { restrictions: None },
        });
        let err = convert_elem_value_to_mcp_tool_response(&elem).unwrap_err();
        assert!(err.message.contains("URL"), "got: {}", err.message);
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
