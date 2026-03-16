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

use crate::mcp::agent_mcp_resource::{AgentMcpResource, ConstructorParam};
use crate::mcp::invoke::constructor_param_extraction::extract_constructor_input_values;
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::AgentId;
use golem_common::base_model::agent::*;
use golem_common::model::agent::ParsedAgentId;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::ErrorData;
use rmcp::model::{JsonObject, ReadResourceResult, ResourceContents};
use std::sync::Arc;

pub async fn invoke_resource(
    worker_service: &Arc<WorkerService>,
    mcp_resource: &AgentMcpResource,
    uri: &str,
    extracted_params: Option<Vec<ConstructorParam>>,
) -> Result<ReadResourceResult, ErrorData> {
    let constructor_params = match extracted_params {
        None => {
            vec![]
        }
        Some(params) => {
            let mut args_map = JsonObject::default();
            for param in &params {
                args_map.insert(
                    param.name.clone(),
                    serde_json::Value::String(param.value.clone()),
                );
            }
            extract_constructor_input_values(&args_map, &mcp_resource.constructor.input_schema)
                .map_err(|e| {
                    tracing::error!("Failed to extract constructor parameters from URI: {}", e);
                    ErrorData::invalid_params(
                        format!("Failed to extract constructor parameters from URI: {}", e),
                        None,
                    )
                })?
        }
    };

    let parsed_agent_id = ParsedAgentId::new(
        mcp_resource.agent_type_name.clone(),
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

    let method_params_data_value = UntypedDataValue::Tuple(vec![]);

    let proto_method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue =
        method_params_data_value.into();

    let principal = Principal::anonymous();
    let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

    let agent_id = AgentId {
        component_id: mcp_resource.component_id,
        agent_id: parsed_agent_id.to_string(),
    };

    let auth_ctx =
        golem_service_base::model::auth::AuthCtx::impersonated_user(mcp_resource.account_id);

    let agent_output = worker_service
        .invoke_agent(
            &agent_id,
            Some(mcp_resource.raw_method.name.clone()),
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
            tracing::error!("Failed to invoke worker for resource: {:?}", e);
            ErrorData::internal_error(
                format!("Failed to invoke worker for resource: {:?}", e),
                None,
            )
        })?;

    let agent_result = match agent_output.result {
        golem_common::model::AgentInvocationResult::AgentMethod { output } => Some(output),
        _ => None,
    };

    let contents = map_agent_response_to_resource_contents(
        agent_result,
        &mcp_resource.raw_method.output_schema,
        uri,
    )?;

    Ok(ReadResourceResult { contents })
}

fn map_agent_response_to_resource_contents(
    invoke_result: Option<UntypedDataValue>,
    expected_type: &DataSchema,
    uri: &str,
) -> Result<Vec<ResourceContents>, ErrorData> {
    match invoke_result {
        Some(untyped_data_value) => {
            let typed_value = DataValue::try_from_untyped(
                untyped_data_value,
                expected_type.clone(),
            )
            .map_err(|error| {
                ErrorData::internal_error(format!("Agent response type mismatch: {error}"), None)
            })?;

            data_value_to_resource_contents(typed_value, uri)
        }
        None => Ok(vec![]),
    }
}

fn data_value_to_resource_contents(
    typed_value: DataValue,
    uri: &str,
) -> Result<Vec<ResourceContents>, ErrorData> {
    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(vec![]),
            1 => {
                let element = elements.into_iter().next().unwrap();
                convert_to_resource_content(element, uri).map(|c| vec![c])
            }
            _ => Err(ErrorData::internal_error(
                "Unexpected number of response tuple elements".to_string(),
                None,
            )),
        },
        DataValue::Multimodal(NamedElementValues { elements }) => elements
            .into_iter()
            .map(|named| convert_to_resource_content(named.value, uri))
            .collect(),
    }
}

fn convert_to_resource_content(
    element: ElementValue,
    uri: &str,
) -> Result<ResourceContents, ErrorData> {
    match element {
        ElementValue::ComponentModel(v) => {
            let json_value = v.value.to_json_value().map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to serialize component model response: {e}"),
                    None,
                )
            })?;
            Ok(ResourceContents::TextResourceContents {
                uri: uri.to_string(),
                mime_type: Some("application/json".to_string()),
                text: json_value.to_string(),
                meta: None,
            })
        }

        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
            match value {
                TextReference::Inline(TextSource { data, .. }) => {
                    // Note that languageCode cannot be encoded in the output to MCP clients when they act as resources
                    // ResourceContents::text(text, uri) — first param is text content, second is URI
                    Ok(ResourceContents::text(data.to_string(), uri.to_string()))
                }
                TextReference::Url(url) => {
                    // This cannot be possible according to MCP spec
                    // A resource content must respond with either an actual text or blob
                    // https://modelcontextprotocol.info/docs/concepts/resources/#reading-resources
                    Err(ErrorData::internal_error(
                        format!(
                            "Received URL text reference, which cannot be part of resource output: {}",
                            url.value
                        ),
                        None,
                    ))
                }
            }
        }

        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

                    Ok(ResourceContents::BlobResourceContents {
                        uri: uri.to_string(),
                        mime_type: Some(binary_type.mime_type),
                        blob: b64,
                        meta: None,
                    })
                }
                BinaryReference::Url(_) => Err(ErrorData::internal_error(
                    "Received URL binary reference, which cannot be part of resource output"
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
        NamedElementSchemas, TextDescriptor, UntypedNamedElementValue, Url as AgentUrl,
    };
    use golem_wasm::Value;
    use golem_wasm::analysis::analysed_type::str;
    use serde_json::json;
    use test_r::test;

    const TEST_URI: &str = "golem://Agent/resource";

    fn str_output_schema() -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "result".to_string(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: str(),
                }),
            }],
        })
    }

    #[test]
    fn component_model_to_text_resource_json() {
        let response = UntypedDataValue::Tuple(vec![UntypedElementValue::ComponentModel(
            Value::String("sunny".to_string()),
        )]);
        let contents =
            map_agent_response_to_resource_contents(Some(response), &str_output_schema(), TEST_URI)
                .unwrap();
        assert_eq!(contents.len(), 1);
        match &contents[0] {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => {
                assert_eq!(uri, TEST_URI);
                assert_eq!(mime_type.as_deref(), Some("application/json"));
                let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(parsed, json!("sunny"));
            }
            _ => panic!("expected TextResourceContents"),
        }
    }

    #[test]
    fn text_element_to_text_resource() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "report".to_string(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            }],
        });
        let response = UntypedDataValue::Tuple(vec![UntypedElementValue::UnstructuredText(
            TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: "rainy day".to_string(),
                    text_type: None,
                }),
            },
        )]);
        let contents =
            map_agent_response_to_resource_contents(Some(response), &schema, TEST_URI).unwrap();
        assert_eq!(contents.len(), 1);
        match &contents[0] {
            ResourceContents::TextResourceContents { text, .. } => {
                assert_eq!(text, "rainy day");
            }
            _ => panic!("expected TextResourceContents"),
        }
    }

    #[test]
    fn binary_element_to_blob_resource() {
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
        let contents =
            map_agent_response_to_resource_contents(Some(response), &schema, TEST_URI).unwrap();
        assert_eq!(contents.len(), 1);
        match &contents[0] {
            ResourceContents::BlobResourceContents {
                blob, mime_type, ..
            } => {
                assert_eq!(blob, "AQID");
                assert_eq!(mime_type.as_deref(), Some("image/png"));
            }
            _ => panic!("expected BlobResourceContents"),
        }
    }

    #[test]
    fn none_response_returns_empty() {
        let contents =
            map_agent_response_to_resource_contents(None, &str_output_schema(), TEST_URI).unwrap();
        assert!(contents.is_empty());
    }

    #[test]
    fn multimodal_returns_multiple_contents() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "text".to_string(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                },
                NamedElementSchema {
                    name: "img".to_string(),
                    schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                        restrictions: None,
                    }),
                },
            ],
        });
        let response = UntypedDataValue::Multimodal(vec![
            UntypedNamedElementValue {
                name: "text".to_string(),
                value: UntypedElementValue::UnstructuredText(TextReferenceValue {
                    value: TextReference::Inline(TextSource {
                        data: "snow report".to_string(),
                        text_type: None,
                    }),
                }),
            },
            UntypedNamedElementValue {
                name: "img".to_string(),
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
        let contents =
            map_agent_response_to_resource_contents(Some(response), &schema, TEST_URI).unwrap();
        assert_eq!(contents.len(), 2);
        assert!(
            matches!(&contents[0], ResourceContents::TextResourceContents { text, .. } if text == "snow report")
        );
        assert!(
            matches!(&contents[1], ResourceContents::BlobResourceContents { blob, .. } if blob == "AQID")
        );
    }

    #[test]
    fn error_on_text_url_reference() {
        let elem = ElementValue::UnstructuredText(UnstructuredTextElementValue {
            value: TextReference::Url(AgentUrl {
                value: "https://example.com".to_string(),
            }),
            descriptor: TextDescriptor { restrictions: None },
        });
        let err = convert_to_resource_content(elem, TEST_URI).unwrap_err();
        assert!(err.message.contains("URL"), "got: {}", err.message);
    }

    #[test]
    fn error_on_binary_url_reference() {
        let elem = ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
            value: BinaryReference::Url(AgentUrl {
                value: "https://example.com/img.png".to_string(),
            }),
            descriptor: BinaryDescriptor { restrictions: None },
        });
        let err = convert_to_resource_content(elem, TEST_URI).unwrap_err();
        assert!(err.message.contains("URL"), "got: {}", err.message);
    }
}
