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
use crate::mcp::invoke::response_mapping::{element_value_to_mcp_json};
use crate::service::worker::WorkerService;
use golem_common::base_model::WorkerId;
use golem_common::base_model::agent::*;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content, JsonObject};
use serde_json::json;
use std::sync::Arc;
use base64::Engine;
use golem_wasm::json::ValueAndTypeJsonExtensions;

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

    let agent_id = AgentId::new(
        mcp_tool.agent_type_name.clone(),
        DataValue::Tuple(ElementValues {
            elements: constructor_params
                .into_iter()
                .map(golem_common::model::agent::ElementValue::ComponentModel)
                .collect(),
        }),
        None,
    );

    let method_params_data_value =
        get_agent_method_input(&args_map, &mcp_tool.raw_method.input_schema).map_err(|e| {
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
                let value_json = element_value_to_mcp_json(&named.value)?;
                parts.push(json!({
                    "name": named.name,
                    "value": value_json,
                }));
                contents.push(
                    element_value_to_content(named.value)?,
                );
            }

            let structured = json!({ "parts" : parts });

            Ok(CallToolResult {
                content: contents,
                structured_content: Some(structured),
                is_error: Some(false),
                meta: None,
            })
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
