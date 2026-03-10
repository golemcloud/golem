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
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::schema::mcp_schema::MULTIMODAL_PARTS_FIELD;
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::WorkerId;
use golem_common::base_model::agent::*;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content, JsonObject, ReadResourceResult, ResourceContents};
use serde_json::json;
use std::sync::Arc;

pub async fn invoke_tool(
    worker_service: &Arc<WorkerService>,
    args_map: JsonObject,
    mcp_tool: &AgentMcpTool,
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

    let agent_id = AgentId::new(
        mcp_tool.agent_type_name.clone(),
        golem_common::model::agent::DataValue::Tuple(golem_common::model::agent::ElementValues {
            elements: constructor_params
                .into_iter()
                .map(golem_common::model::agent::ElementValue::ComponentModel)
                .collect(),
        }),
        None,
    );

    let method_params_data_value =
        extract_method_parameters(&args_map, &mcp_tool.raw_method.input_schema).map_err(|e| {
            tracing::error!("Failed to extract method parameters: {}", e);
            ErrorData::invalid_params(format!("Failed to extract method parameters: {}", e), None)
        })?;

    let proto_method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue =
        method_params_data_value.into();

    let principal = Principal::anonymous();
    let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

    let worker_id = WorkerId {
        component_id: mcp_tool.component_id,
        worker_name: agent_id.to_string(),
    };

    let auth_ctx = golem_service_base::model::auth::AuthCtx::impersonated_user(mcp_tool.account_id);

    let agent_output = worker_service
        .invoke_agent(
            &worker_id,
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

    interpret_agent_response_as_tool_result(agent_result, &mcp_tool.raw_method.output_schema)
}

fn interpret_agent_response_as_tool_result(
    invoke_result: Option<UntypedDataValue>,
    expected_type: &DataSchema,
) -> Result<CallToolResult, ErrorData> {
    match invoke_result {
        Some(untyped_data_value) => {
            map_agent_response_to_contents(untyped_data_value, expected_type)
        }
        None => Ok(CallToolResult::success(vec![])),
    }
}

pub fn interpret_agent_response(
    invoke_result: Option<UntypedDataValue>,
    expected_type: &DataSchema,
) -> Result<serde_json::Value, ErrorData> {
    match invoke_result {
        Some(untyped_data_value) => {
            let typed_value = DataValue::try_from_untyped(
                untyped_data_value,
                expected_type.clone(),
            )
            .map_err(|error| {
                ErrorData::internal_error(format!("Agent response type mismatch: {error}"), None)
            })?;

            map_data_value_to_json(typed_value)
                .map(|json_value| {
                    json!({
                        "return-value": json_value,
                    })
                })
                .map_err(|e| {
                    tracing::error!("Failed to map successful agent response: {}", e);
                    ErrorData::internal_error(
                        format!("Failed to map successful agent response: {}", e),
                        None,
                    )
                })
        }
        None => Ok(json!({})),
    }
}

fn map_agent_response_to_contents(
    agent_response: UntypedDataValue,
    expected_type: &DataSchema,
) -> Result<CallToolResult, ErrorData> {
    let typed_value =
        DataValue::try_from_untyped(agent_response, expected_type.clone()).map_err(|error| {
            ErrorData::internal_error(format!("Agent response type mismatch: {error}"), None)
        })?;

    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(CallToolResult::success(vec![])),
            1 => element_value_to_content(elements.into_iter().next().unwrap())
                .map(|content| CallToolResult::success(vec![content])),
            _ => Err(ErrorData::internal_error(
                "Unexpected number of response tuple elements".to_string(),
                None,
            )),
        },
        DataValue::Multimodal(NamedElementValues { elements }) => {
            let mut parts = Vec::new();
            let mut contents = Vec::new();

            for named in elements {
                let value_json = element_value_to_json(&named.value)?;
                parts.push(json!({
                    "name": named.name,
                    "value": value_json,
                }));
                contents.push(element_value_to_content(named.value)?);
            }

            let structured = json!({ MULTIMODAL_PARTS_FIELD: parts });

            Ok(CallToolResult {
                content: contents,
                structured_content: Some(structured),
                is_error: Some(false),
                meta: None,
            })
        }
    }
}

fn element_value_to_json(element: &ElementValue) -> Result<serde_json::Value, ErrorData> {
    match element {
        ElementValue::ComponentModel(component_model_value) => {
            component_model_value.value.to_json_value().map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to serialize component model response: {e}"),
                    None,
                )
            })
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => match value {
            TextReference::Inline(TextSource { data, .. }) => Ok(json!({ "data": data })),
            TextReference::Url(url) => Ok(json!({ "data": url.value })),
        },
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                    Ok(json!({
                        "data": b64,
                        "mimeType": binary_type.mime_type,
                    }))
                }
                BinaryReference::Url(url) => Ok(json!({ "data": url.value })),
            }
        }
    }
}

fn element_value_to_content(element: ElementValue) -> Result<Content, ErrorData> {
    match element {
        ElementValue::ComponentModel(component_model_value) => {
            let json_value = component_model_value.value.to_json_value().map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to serialize component model response: {e}"),
                    None,
                )
            })?;
            Ok(Content::text(json_value.to_string()))
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => match value {
            TextReference::Inline(TextSource { data, .. }) => Ok(Content::text(data)),
            TextReference::Url(url) => Ok(Content::text(url.value)),
        },
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    Ok(Content::image(b64, binary_type.mime_type))
                }
                BinaryReference::Url(url) => Ok(Content::text(url.value)),
            }
        }
    }
}

fn map_data_value_to_json(typed_value: DataValue) -> Result<serde_json::Value, ErrorData> {
    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(json!({})),
            1 => {
                let element = elements.into_iter().next().unwrap();
                match element {
                    ElementValue::ComponentModel(v) => v.value.to_json_value().map_err(|e| {
                        ErrorData::internal_error(
                            format!("Failed to serialize component model response: {e}"),
                            None,
                        )
                    }),
                    ElementValue::UnstructuredText(UnstructuredTextElementValue {
                        value, ..
                    }) => match value {
                        TextReference::Inline(TextSource { data, .. }) => {
                            Ok(serde_json::Value::String(data))
                        }
                        TextReference::Url(url) => Ok(serde_json::Value::String(url.value)),
                    },
                    ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                        value,
                        ..
                    }) => match value {
                        BinaryReference::Inline(BinarySource { data, binary_type }) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                            Ok(json!({
                                "data": b64,
                                "mimeType": binary_type.mime_type,
                            }))
                        }
                        BinaryReference::Url(url) => Ok(serde_json::Value::String(url.value)),
                    },
                }
            }
            _ => Err(ErrorData::internal_error(
                "Unexpected number of response tuple elements".to_string(),
                None,
            )),
        },
        DataValue::Multimodal(NamedElementValues { elements }) => {
            let parts: Vec<serde_json::Value> = elements
                .iter()
                .map(|named| {
                    let value_json = element_value_to_json(&named.value)?;
                    Ok(json!({
                        "name": named.name,
                        "value": value_json,
                    }))
                })
                .collect::<Result<Vec<_>, ErrorData>>()?;

            Ok(json!({ MULTIMODAL_PARTS_FIELD: parts }))
        }
    }
}

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

    let agent_id = AgentId::new(
        mcp_resource.agent_type_name.clone(),
        DataValue::Tuple(ElementValues {
            elements: constructor_params
                .into_iter()
                .map(ElementValue::ComponentModel)
                .collect(),
        }),
        None,
    );

    let method_params_data_value = UntypedDataValue::Tuple(vec![]);

    let proto_method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue =
        method_params_data_value.into();

    let principal = Principal::anonymous();
    let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

    let worker_id = WorkerId {
        component_id: mcp_resource.component_id,
        worker_name: agent_id.to_string(),
    };

    let auth_ctx =
        golem_service_base::model::auth::AuthCtx::impersonated_user(mcp_resource.account_id);

    let agent_output = worker_service
        .invoke_agent(
            &worker_id,
            mcp_resource.raw_method.name.clone(),
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

    let json_value =
        interpret_agent_response(agent_result, &mcp_resource.raw_method.output_schema)?;

    Ok(ReadResourceResult {
        contents: vec![ResourceContents::text(json_value.to_string(), uri)],
    })
}

fn extract_method_parameters(
    args_map: &JsonObject,
    schema: &DataSchema,
) -> Result<UntypedDataValue, String> {
    match schema {
        DataSchema::Tuple(named_schemas) => {
            let elements = extract_element_values(args_map, &named_schemas.elements)?;
            Ok(UntypedDataValue::Tuple(elements))
        }
        DataSchema::Multimodal(named_schemas) => {
            let parts_array = args_map
                .get(MULTIMODAL_PARTS_FIELD)
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    format!(
                        "Multimodal input requires a '{}' array field",
                        MULTIMODAL_PARTS_FIELD
                    )
                })?;

            let schema_map: std::collections::HashMap<&str, &ElementSchema> = named_schemas
                .elements
                .iter()
                .map(|s| (s.name.as_str(), &s.schema))
                .collect();

            let mut named_elements = Vec::new();
            for (i, part) in parts_array.iter().enumerate() {
                let obj = part.as_object().ok_or_else(|| {
                    format!("parts[{}] must be an object with 'name' and 'value'", i)
                })?;

                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("parts[{}] is missing 'name' string field", i))?;

                let elem_schema = schema_map.get(name).ok_or_else(|| {
                    format!(
                        "parts[{}]: unknown element name '{}'. Expected one of: {}",
                        i,
                        name,
                        schema_map.keys().copied().collect::<Vec<_>>().join(", ")
                    )
                })?;

                let value_json = obj
                    .get("value")
                    .ok_or_else(|| format!("parts[{}] is missing 'value' field", i))?;

                let element = extract_multimodal_element_value(name, value_json, elem_schema, i)?;

                named_elements.push(UntypedNamedElementValue {
                    name: name.to_string(),
                    value: element,
                });
            }
            Ok(UntypedDataValue::Multimodal(named_elements))
        }
    }
}

fn extract_element_values(
    args_map: &JsonObject,
    schemas: &[NamedElementSchema],
) -> Result<Vec<UntypedElementValue>, String> {
    let mut params = Vec::new();
    for schema_element in schemas {
        let element =
            extract_single_element_value(args_map, &schema_element.name, &schema_element.schema)?;
        params.push(element);
    }
    Ok(params)
}

fn extract_multimodal_element_value(
    name: &str,
    value_json: &serde_json::Value,
    elem_schema: &ElementSchema,
    index: usize,
) -> Result<UntypedElementValue, String> {
    match elem_schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let value_and_type =
                golem_wasm::ValueAndType::parse_with_type(value_json, element_type).map_err(
                    |errs| {
                        format!(
                            "parts[{}] '{}': failed to parse value: {}",
                            index,
                            name,
                            errs.join(", ")
                        )
                    },
                )?;
            Ok(UntypedElementValue::ComponentModel(value_and_type.value))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let obj = value_json.as_object().ok_or_else(|| {
                format!(
                    "parts[{}] '{}': value must be an object with 'data' and optional 'languageCode'",
                    index, name
                )
            })?;

            let data = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("parts[{}] '{}': missing 'data' string field", index, name))?
                .to_string();

            let language_code = obj.get("languageCode").and_then(|v| v.as_str());

            if let Some(code) = language_code {
                if let Some(allowed) = &descriptor.restrictions {
                    if !allowed.is_empty() && !allowed.iter().any(|t| t.language_code == code) {
                        let expected: Vec<&str> =
                            allowed.iter().map(|t| t.language_code.as_str()).collect();
                        return Err(format!(
                            "parts[{}] '{}': language code '{}' is not allowed. Expected one of: {}",
                            index,
                            name,
                            code,
                            expected.join(", ")
                        ));
                    }
                }
            }

            let text_type = language_code.map(|code| TextType {
                language_code: code.to_string(),
            });

            Ok(UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource { data, text_type }),
            }))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let obj = value_json.as_object().ok_or_else(|| {
                format!(
                    "parts[{}] '{}': value must be an object with 'data' and 'mimeType'",
                    index, name
                )
            })?;

            let b64 = obj.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
                format!("parts[{}] '{}': missing 'data' string field", index, name)
            })?;

            let mime_type = obj
                .get("mimeType")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!(
                        "parts[{}] '{}': missing 'mimeType' string field",
                        index, name
                    )
                })?;

            if let Some(allowed) = &descriptor.restrictions {
                if !allowed.is_empty() && !allowed.iter().any(|t| t.mime_type == mime_type) {
                    let expected: Vec<&str> =
                        allowed.iter().map(|t| t.mime_type.as_str()).collect();
                    return Err(format!(
                        "parts[{}] '{}': MIME type '{}' is not allowed. Expected one of: {}",
                        index,
                        name,
                        mime_type,
                        expected.join(", ")
                    ));
                }
            }

            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    format!(
                        "parts[{}] '{}': failed to decode base64: {}",
                        index, name, e
                    )
                })?;

            Ok(UntypedElementValue::UnstructuredBinary(
                BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data,
                        binary_type: BinaryType {
                            mime_type: mime_type.to_string(),
                        },
                    }),
                },
            ))
        }
    }
}

fn extract_single_element_value(
    args_map: &JsonObject,
    name: &str,
    elem_schema: &ElementSchema,
) -> Result<UntypedElementValue, String> {
    let json_value = args_map.get(name);
    match elem_schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let json_value = match json_value {
                Some(value) => value.clone(),
                None => {
                    if matches!(element_type, AnalysedType::Option(_)) {
                        serde_json::Value::Null
                    } else {
                        return Err(format!("Missing parameter: {}", name));
                    }
                }
            };

            let value_and_type =
                golem_wasm::ValueAndType::parse_with_type(&json_value, element_type).map_err(
                    |errs| format!("Failed to parse parameter '{}': {}", name, errs.join(", ")),
                )?;

            Ok(UntypedElementValue::ComponentModel(value_and_type.value))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let obj = match json_value {
                Some(serde_json::Value::Object(o)) => o,
                Some(_) => {
                    return Err(format!(
                        "Parameter '{}' must be an object with 'data' and optional 'languageCode'",
                        name
                    ));
                }
                None => return Err(format!("Missing parameter: {}", name)),
            };

            let data = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("Parameter '{}' is missing 'data' string field", name))?
                .to_string();

            let language_code = obj.get("languageCode").and_then(|v| v.as_str());

            if let Some(code) = language_code {
                if let Some(allowed) = &descriptor.restrictions {
                    if !allowed.is_empty() && !allowed.iter().any(|t| t.language_code == code) {
                        let expected: Vec<&str> =
                            allowed.iter().map(|t| t.language_code.as_str()).collect();
                        return Err(format!(
                            "Parameter '{}': language code '{}' is not allowed. Expected one of: {}",
                            name,
                            code,
                            expected.join(", ")
                        ));
                    }
                }
            }

            let text_type = language_code.map(|code| TextType {
                language_code: code.to_string(),
            });

            Ok(UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource { data, text_type }),
            }))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let obj = match json_value {
                Some(serde_json::Value::Object(o)) => o,
                Some(_) => {
                    return Err(format!(
                        "Parameter '{}' must be an object with 'data' and 'mimeType'",
                        name
                    ));
                }
                None => return Err(format!("Missing parameter: {}", name)),
            };

            let b64 = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("Parameter '{}' is missing 'data' string field", name))?;

            let mime_type = obj
                .get("mimeType")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!("Parameter '{}' is missing 'mimeType' string field", name)
                })?;

            if let Some(allowed) = &descriptor.restrictions {
                if !allowed.is_empty() && !allowed.iter().any(|t| t.mime_type == mime_type) {
                    let expected: Vec<&str> =
                        allowed.iter().map(|t| t.mime_type.as_str()).collect();
                    return Err(format!(
                        "Parameter '{}': MIME type '{}' is not allowed. Expected one of: {}",
                        name,
                        mime_type,
                        expected.join(", ")
                    ));
                }
            }

            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("Failed to decode base64 parameter '{}': {}", name, e))?;

            Ok(UntypedElementValue::UnstructuredBinary(
                BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data,
                        binary_type: BinaryType {
                            mime_type: mime_type.to_string(),
                        },
                    }),
                },
            ))
        }
    }
}

fn extract_constructor_input_values(
    args_map: &JsonObject,
    schema: &DataSchema,
) -> Result<Vec<ComponentModelElementValue>, String> {
    match schema {
        DataSchema::Tuple(named_schemas) => {
            let mut params = Vec::new();

            for NamedElementSchema {
                name,
                schema: elem_schema,
            } in &named_schemas.elements
            {
                match elem_schema {
                    ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
                        let json_value = match args_map.get(name) {
                            Some(value) => value.clone(),
                            None => {
                                if matches!(element_type, AnalysedType::Option(_)) {
                                    serde_json::Value::Null
                                } else {
                                    return Err(format!("Missing parameter: {}", name));
                                }
                            }
                        };

                        let value_and_type =
                            golem_wasm::ValueAndType::parse_with_type(&json_value, element_type)
                                .map_err(|errs| {
                                    format!(
                                        "Failed to parse parameter '{}': {}",
                                        name,
                                        errs.join(", ")
                                    )
                                })?;

                        params.push(ComponentModelElementValue {
                            value: value_and_type,
                        });
                    }
                    ElementSchema::UnstructuredText(_) => {
                        return Err(format!(
                            "MCP cannot support unstructured-text constructor parameters like '{}'",
                            name
                        ));
                    }

                    ElementSchema::UnstructuredBinary(_) => {
                        return Err(format!(
                            "MCP cannot support unstructured-binary constructor parameters like '{}'",
                            name
                        ));
                    }
                }
            }

            Ok(params)
        }
        DataSchema::Multimodal(_) => {
            Err("MCP does not support multimodal constructor schemas".to_string())
        }
    }
}
