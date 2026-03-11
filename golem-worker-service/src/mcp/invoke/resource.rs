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
use golem_common::base_model::WorkerId;
use golem_common::base_model::agent::*;
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
                    Ok(ResourceContents::text(uri.to_string(), data.to_string()))
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
