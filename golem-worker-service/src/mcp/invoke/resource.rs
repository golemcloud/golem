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
use crate::mcp::invoke::response_mapping::element_value_to_mcp_json;
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::WorkerId;
use golem_common::base_model::agent::*;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::ErrorData;
use rmcp::model::{JsonObject, ReadResourceResult, ResourceContents};
use serde_json::json;
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

fn interpret_agent_response(
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

            map_data_value_to_resource(typed_value)
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

// Map a DataValue returned by an agent method to the JSON format expected by MCP responses.
// This is used for both tool and resource invocation responses, as well as for agent method responses when the agent is invoked directly (e.g. from a workflow).
fn map_data_value_to_resource(typed_value: DataValue) -> Result<serde_json::Value, ErrorData> {
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
                    let value_json = element_value_to_mcp_json(&named.value)?;
                    Ok(json!({
                        "name": named.name,
                        "value": value_json,
                    }))
                })
                .collect::<Result<Vec<_>, ErrorData>>()?;

            Ok(json!({ "parts" : parts }))
        }
    }
}
