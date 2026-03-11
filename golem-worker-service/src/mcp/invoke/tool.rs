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
use rmcp::model::{CallToolResult, JsonObject};
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
                let element = elements.into_iter().next().unwrap();
                let json_value = convert_elem_value_to_mcp_tool_response(&element)?;
                Ok(CallToolResult::structured(json_value))
            }
            _ => Err(ErrorData::internal_error(
                "Unexpected number of response tuple elements".to_string(),
                None,
            )),
        },
        DataValue::Multimodal(NamedElementValues { elements }) => {
            let mut parts = Vec::new();

            for named in elements {
                let value_json = convert_elem_value_to_mcp_tool_response(&named.value)?;
                parts.push(json!({
                    "name": named.name,
                    "value": value_json,
                }));
            }

            let structured = json!({ "parts" : parts });

            Ok(CallToolResult {
                content: vec![],
                structured_content: Some(structured),
                is_error: Some(false),
                meta: None,
            })
        }
    }
}

// Mapping from ElementValue to the JSON format expected by MCP clients
// (based on the schema they learned from initialization)
// This is used only for tools, and not for resources.
// Any changes in this mapping should be carefully tested with actual MCP clients
fn convert_elem_value_to_mcp_tool_response(
    element: &ElementValue,
) -> Result<serde_json::Value, ErrorData> {
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
            TextReference::Inline(TextSource { data, text_type }) => {
                let mut obj = serde_json::Map::new();
                obj.insert("data".to_string(), json!(data));
                if let Some(tt) = text_type {
                    obj.insert("languageCode".to_string(), json!(tt.language_code));
                }
                Ok(serde_json::Value::Object(obj))
            }
            TextReference::Url(_) => Err(ErrorData::internal_error(
                "A text reference URL can only be part of tool input and not output".to_string(),
                None,
            )),
        },
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    // https://modelcontextprotocol.info/docs/concepts/resources/#binary-resources
                    // Binary resources contain raw binary data encoded in base64.
                    // Note that when unstructured-binary response is part of a `tool`, we simply `normalize` to the way
                    // component-model behaves, but the schema information has explicit descriptions around the base64 encoding.
                    // This is the best approximation we can make to expose a method as a mcp tool that returns binary data.
                    let b64 = base64::engine::general_purpose::STANDARD.encode(data);

                    // Also for tools, we don't use `ResourceContents` and it's impossible. So for tools, we will
                    // strictly stick on `JsonObject` output schema (and that's the only way to do it)
                    Ok(json!({
                        "data": b64,
                        "mimeType": binary_type.mime_type,
                    }))
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
