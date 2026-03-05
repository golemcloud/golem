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
use crate::service::worker::WorkerService;
use golem_common::base_model::WorkerId;
use base64::Engine;
use golem_common::base_model::agent::*;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, JsonObject, ReadResourceResult, ResourceContents};
use serde_json::json;
use std::sync::Arc;

pub async fn invoke_tool(
    worker_service: &Arc<WorkerService>,
    args_map: JsonObject,
    mcp_tool: &AgentMcpTool,
) -> Result<CallToolResult, ErrorData> {
    let constructor_params = extract_parameters_by_schema(
        &args_map,
        &mcp_tool.constructor.input_schema,
        |value_and_type| ComponentModelElementValue {
            value: value_and_type,
        },
    )
    .map_err(|e| {
        tracing::error!("Failed to extract constructor parameters: {}", e);
        ErrorData::invalid_params(
            format!("Failed to extract constructor parameters: {}", e),
            None,
        )
    })?;

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

    let method_params =
        extract_method_parameters(&args_map, &mcp_tool.raw_method.input_schema)
            .map_err(|e| {
                tracing::error!("Failed to extract method parameters: {}", e);
                ErrorData::invalid_params(
                    format!("Failed to extract method parameters: {}", e),
                    None,
                )
            })?;

    let method_params_data_value = UntypedDataValue::Tuple(method_params);

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

    interpret_agent_response(agent_result, &mcp_tool.raw_method.output_schema)
        .map(CallToolResult::structured)
}

pub fn interpret_agent_response(
    invoke_result: Option<UntypedDataValue>,
    expected_type: &DataSchema,
) -> Result<serde_json::Value, ErrorData> {
    match invoke_result {
        Some(untyped_data_value) => {
            map_successful_agent_response(untyped_data_value, expected_type)
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

fn map_successful_agent_response(
    agent_response: UntypedDataValue,
    expected_type: &DataSchema,
) -> Result<serde_json::Value, ErrorData> {
    let typed_value =
        DataValue::try_from_untyped(agent_response, expected_type.clone()).map_err(|error| {
            ErrorData::internal_error(format!("Agent response type mismatch: {error}"), None)
        })?;

    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(json!({})),
            1 => map_single_element_agent_response(elements.into_iter().next().unwrap()).map_err(
                |e| {
                    tracing::error!("Failed to map single element agent response: {}", e);
                    ErrorData::internal_error(
                        format!("Failed to map single element agent response: {}", e),
                        None,
                    )
                },
            ),
            _ => Err(ErrorData::internal_error(
                "Unexpected number of response tuple elements".to_string(),
                None,
            )),
        },
        DataValue::Multimodal(_) => Err(ErrorData::internal_error(
            "multi modal response not yet supported".to_string(),
            None,
        )),
    }
}

fn map_single_element_agent_response(element: ElementValue) -> Result<serde_json::Value, String> {
    match element {
        ElementValue::ComponentModel(component_model_value) => {
            component_model_value.value.to_json_value()
        }

        ElementValue::UnstructuredBinary(_) => Err(
            "Received unstructured binary response, which is not supported in this context"
                .to_string(),
        ),

        ElementValue::UnstructuredText(_) => Err(
            "Received unstructured text response, which is not supported in this context"
                .to_string(),
        ),
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
            extract_parameters_by_schema(
                &args_map,
                &mcp_resource.constructor.input_schema,
                |value_and_type| ComponentModelElementValue {
                    value: value_and_type,
                },
            )
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
) -> Result<Vec<UntypedElementValue>, String> {
    match schema {
        DataSchema::Tuple(named_schemas) => {
            let mut params = Vec::new();

            for NamedElementSchema {
                name,
                schema: elem_schema,
            } in &named_schemas.elements
            {
                let json_value = args_map.get(name);
                let element = match elem_schema {
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
                            golem_wasm::ValueAndType::parse_with_type(&json_value, element_type)
                                .map_err(|errs| {
                                    format!(
                                        "Failed to parse parameter '{}': {}",
                                        name,
                                        errs.join(", ")
                                    )
                                })?;

                        UntypedElementValue::ComponentModel(value_and_type.value)
                    }
                    ElementSchema::UnstructuredText(_) => {
                        let text = match json_value {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                            None => return Err(format!("Missing parameter: {}", name)),
                        };

                        UntypedElementValue::UnstructuredText(TextReferenceValue {
                            value: TextReference::Inline(TextSource {
                                data: text,
                                text_type: None,
                            }),
                        })
                    }
                    ElementSchema::UnstructuredBinary(descriptor) => {
                        let obj = match json_value {
                            Some(serde_json::Value::Object(o)) => o,
                            Some(_) => {
                                return Err(format!(
                                    "Parameter '{}' must be an object with 'data' and 'mimeType'",
                                    name
                                ))
                            }
                            None => return Err(format!("Missing parameter: {}", name)),
                        };

                        let b64 = obj
                            .get("data")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                format!("Parameter '{}' is missing 'data' string field", name)
                            })?;

                        let mime_type = obj
                            .get("mimeType")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                format!("Parameter '{}' is missing 'mimeType' string field", name)
                            })?;

                        if let Some(allowed) = &descriptor.restrictions {
                            if !allowed.is_empty()
                                && !allowed.iter().any(|t| t.mime_type == mime_type)
                            {
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

                        let data =
                            base64::engine::general_purpose::STANDARD.decode(b64).map_err(
                                |e| format!("Failed to decode base64 parameter '{}': {}", name, e),
                            )?;

                        UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                            value: BinaryReference::Inline(BinarySource {
                                data,
                                binary_type: BinaryType {
                                    mime_type: mime_type.to_string(),
                                },
                            }),
                        })
                    }
                };

                params.push(element);
            }

            Ok(params)
        }
        DataSchema::Multimodal(_) => Err("Multimodal schema is not yet supported".to_string()),
    }
}

fn extract_parameters_by_schema<F, A>(
    args_map: &JsonObject,
    schema: &DataSchema,
    f: F,
) -> Result<Vec<A>, String>
where
    F: Fn(ValueAndType) -> A,
{
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

                        params.push(f(value_and_type));
                    }
                    _ => {
                        return Err(format!(
                            "Unsupported element schema type for parameter '{}'",
                            name
                        ));
                    }
                }
            }

            Ok(params)
        }
        DataSchema::Multimodal(_) => Err("Multimodal schema is not yet supported".to_string()),
    }
}
