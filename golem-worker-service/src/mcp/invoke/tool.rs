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
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::AgentId;
use golem_common::base_model::agent::*;
use golem_common::model::agent::ParsedAgentId;
use golem_wasm::json::ValueAndTypeJsonExtensions;
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
    let constructor_params =
        extract_constructor_input_values(&args_map, &mcp_tool.constructor.input_schema).map_err(
            |e| {
                tracing::error!("Failed to extract constructor parameters: {}", e);
                ErrorData::invalid_params(
                    format!("Failed to extract constructor parameters: {}", e),
                    None,
                )
            },
        )?;

    let parsed_agent_id = ParsedAgentId::new(
        mcp_tool.agent_type_name.clone(),
        DataValue::Tuple(ElementValues {
            elements: constructor_params
                .into_iter()
                .map(ElementValue::ComponentModel)
                .collect(),
        }),
        None,
    )
    .map_err(|e| {
        tracing::error!("Failed to parse agent id: {}", e);
        ErrorData::invalid_params(format!("Failed to parse agent id: {}", e), None)
    })?;

    let method_params_data_value =
        get_agent_method_input(&args_map, &mcp_tool.raw_method.input_schema).map_err(|e| {
            tracing::error!("Failed to extract method parameters: {}", e);
            ErrorData::invalid_params(format!("Failed to extract method parameters: {}", e), None)
        })?;

    let proto_method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue =
        method_params_data_value.into();

    let principal = Principal::anonymous();
    let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

    let agent_id = AgentId {
        component_id: mcp_tool.component_id,
        agent_id: parsed_agent_id.to_string(),
    };

    let auth_ctx = golem_service_base::model::auth::AuthCtx::impersonated_user(mcp_tool.account_id);

    let agent_output = worker_service
        .invoke_agent(
            &agent_id,
            mcp_tool.raw_method.name.clone(),
            proto_method_parameters,
            golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Await as i32,
            None,
            None,
            None,
            auth_ctx,
            proto_principal,
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
        Some(untyped_data_value) => map_agent_response_to_tool_result(
            untyped_data_value,
            &mcp_tool.raw_method.output_schema,
        ),
        None => Ok(CallToolResult::success(vec![])),
    }
}

pub fn map_agent_response_to_tool_result(
    agent_response: UntypedDataValue,
    expected_type: &DataSchema,
) -> Result<CallToolResult, ErrorData> {
    let typed_value =
        DataValue::try_from_untyped(agent_response, expected_type.clone()).map_err(|error| {
            ErrorData::internal_error(format!("Agent response type mismatch: {error}"), None)
        })?;

    // Note that, according to MCP specification, the output schema for a tool must be a JsonObject,
    // And as part of tool result, we simply ensure to respond according to the advertised output schema.
    // This is why even for multimodal response, we convert to structured format with "parts" array.
    // See `element_value_to_mcp_json` for more info.
    // We deal with actual content (text or binary) when it comes to "resource" results, where it doesn't
    // need to adhere to `mcp-schema`
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
        ElementValue::ComponentModel(component_model_value) => component_model_value
            .value
            .to_json_value()
            .map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to serialize component model response: {e}"),
                    None,
                )
            })
            .map(ToolResult::Default),

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
    use golem_common::base_model::agent::{
        BinaryDescriptor, ComponentModelElementSchema, ElementSchema, NamedElementSchema,
        NamedElementSchemas, TextDescriptor, TextType, UntypedNamedElementValue, Url,
    };
    use golem_wasm::Value;
    use golem_wasm::analysis::{AnalysedType, TypeStr};
    use serde_json::json;
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
        let response = UntypedDataValue::Tuple(vec![UntypedElementValue::ComponentModel(
            Value::String("hello".to_string()),
        )]);
        let result = map_agent_response_to_tool_result(response, &str_output_schema()).unwrap();
        assert_eq!(result.structured_content, Some(json!({"result": "hello"})));
        assert_eq!(result.is_error, Some(false));
    }

    #[test]
    fn tuple_empty_returns_success() {
        let schema = DataSchema::Tuple(NamedElementSchemas { elements: vec![] });
        let response = UntypedDataValue::Tuple(vec![]);
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
        let response = UntypedDataValue::Tuple(vec![UntypedElementValue::UnstructuredText(
            TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: "weather is sunny".to_string(),
                    text_type: Some(TextType {
                        language_code: "en".to_string(),
                    }),
                }),
            },
        )]);
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

        let response = UntypedDataValue::Tuple(vec![UntypedElementValue::UnstructuredBinary(
            BinaryReferenceValue {
                value: BinaryReference::Inline(BinarySource {
                    data: vec![1, 2, 3],
                    binary_type: BinaryType {
                        mime_type: "image/png".to_string(),
                    },
                }),
            },
        )]);
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
        let response = UntypedDataValue::Multimodal(vec![
            UntypedNamedElementValue {
                name: "desc".to_string(),
                value: UntypedElementValue::UnstructuredText(TextReferenceValue {
                    value: TextReference::Inline(TextSource {
                        data: "a photo".to_string(),
                        text_type: None,
                    }),
                }),
            },
            UntypedNamedElementValue {
                name: "photo".to_string(),
                value: UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data: vec![1, 2, 3],
                        binary_type: BinaryType {
                            mime_type: "image/png".to_string(),
                        },
                    }),
                }),
            },
        ]);
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
}
