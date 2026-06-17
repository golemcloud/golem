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
use crate::mcp::invoke::build_constructor_parameters;
use crate::mcp::invoke::constructor_param_extraction::extract_constructor_input_values;
use crate::service::worker::WorkerService;
use base64::Engine;
use golem_common::base_model::AgentId;
use golem_common::base_model::agent::Principal;
use golem_common::model::agent::ParsedAgentId;
use golem_common::schema::adapters::{multimodal_variant_cases, resolve_ref};
use golem_common::schema::agent::OutputSchema;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render::json_value::to_json_value;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::{
    BinaryValuePayload, SchemaValue, TextValuePayload, VariantValuePayload,
};
use rmcp::ErrorData;
use rmcp::model::{JsonObject, ReadResourceResult, ResourceContents};
use std::sync::Arc;

pub async fn invoke_resource(
    worker_service: &Arc<WorkerService>,
    mcp_resource: &AgentMcpResource,
    uri: &str,
    extracted_params: Option<Vec<ConstructorParam>>,
) -> Result<ReadResourceResult, ErrorData> {
    let constructor_values = match extracted_params {
        None => Vec::new(),
        Some(params) => {
            let mut args_map = JsonObject::default();
            for param in &params {
                args_map.insert(
                    param.name.clone(),
                    serde_json::Value::String(param.value.clone()),
                );
            }
            extract_constructor_input_values(
                &args_map,
                &mcp_resource.schema_graph,
                &mcp_resource.constructor.input_schema,
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

    let parameters = build_constructor_parameters(
        &mcp_resource.schema_graph,
        &mcp_resource.constructor.input_schema,
        constructor_values,
    );

    let parsed_agent_id = ParsedAgentId::new_auto_phantom(
        mcp_resource.agent_type_name.clone(),
        parameters,
        None,
        mcp_resource.agent_mode,
    )
    .map_err(|e| {
        tracing::error!("Failed to parse agent id: {}", e);
        ErrorData::invalid_params(format!("Failed to parse agent id: {}", e), None)
    })?;

    // A resource method has no user-supplied input parameters.
    let method_parameters = SchemaValue::Record { fields: vec![] };

    let proto_method_parameters: golem_api_grpc::proto::golem::schema::SchemaValue =
        method_parameters.into();

    let principal = Principal::anonymous();
    let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

    let agent_id = AgentId {
        component_id: mcp_resource.component_id,
        agent_id: parsed_agent_id.to_string(),
    };

    let auth_ctx = golem_service_base::model::auth::AuthCtx::agent(
        mcp_resource.account_id,
        mcp_resource.account_email.clone(),
    );

    let agent_output = worker_service
        .invoke_agent(
            &agent_id,
            Some(mcp_resource.method.name.clone()),
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
        &mcp_resource.schema_graph,
        &mcp_resource.method.output_schema,
        agent_result,
        uri,
    )?;

    Ok(ReadResourceResult { contents })
}

fn map_agent_response_to_resource_contents(
    graph: &SchemaGraph,
    output: &OutputSchema,
    invoke_result: Option<SchemaValue>,
    uri: &str,
) -> Result<Vec<ResourceContents>, ErrorData> {
    let Some(value) = invoke_result else {
        return Ok(vec![]);
    };
    let Some(ty) = output.schema() else {
        // Unit output carries no value.
        return Ok(vec![]);
    };

    // Multimodal output: `list<variant<… Role::Multimodal>>`.
    if let Some(cases) = multimodal_variant_cases(graph, ty).map_err(internal_error)? {
        let elements = match value {
            SchemaValue::List { elements } => elements,
            _ => {
                return Err(ErrorData::internal_error(
                    "Expected a multimodal list response".to_string(),
                    None,
                ));
            }
        };

        return elements
            .into_iter()
            .map(|element| {
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
                schema_value_to_resource_content(graph, case_schema, &payload, uri)
            })
            .collect();
    }

    schema_value_to_resource_content(graph, ty, &value, uri).map(|c| vec![c])
}

fn internal_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

fn schema_value_to_resource_content(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    uri: &str,
) -> Result<ResourceContents, ErrorData> {
    match resolve_ref(graph, ty) {
        Ok(SchemaType::Text { .. }) => match value {
            // Note that languageCode cannot be encoded in the output to MCP
            // clients when they act as resources. `ResourceContents::text`
            // takes (text, uri) in that order.
            SchemaValue::Text(TextValuePayload { text, .. }) => {
                Ok(ResourceContents::text(text.clone(), uri.to_string()))
            }
            _ => Err(ErrorData::internal_error(
                "Expected a text value for a text output".to_string(),
                None,
            )),
        },
        Ok(SchemaType::Binary { .. }) => match value {
            SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }) => {
                Ok(ResourceContents::BlobResourceContents {
                    uri: uri.to_string(),
                    mime_type: mime_type.clone(),
                    blob: base64::engine::general_purpose::STANDARD.encode(bytes),
                    meta: None,
                })
            }
            _ => Err(ErrorData::internal_error(
                "Expected a binary value for a binary output".to_string(),
                None,
            )),
        },
        _ => {
            let json_value = to_json_value(graph, ty, value).map_err(|e| {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::agent_mcp_resource::{AgentMcpResource, AgentMcpResourceKind};
    use crate::mcp::invoke::test_support::{InvocationHarness, phantom_id};
    use golem_common::base_model::agent::{AgentMode, AgentTypeName};
    use golem_common::model::AgentInvocationOutput;
    use golem_common::schema::agent::{AgentConstructorSchema, AgentMethodSchema, OutputSchema};
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::metadata::Role;
    use golem_common::schema::schema_type::{
        BinaryRestrictions, SchemaType, TextRestrictions, VariantCaseType,
    };
    use golem_common::schema::{BinaryValuePayload, InputSchema, TextValuePayload};
    use rmcp::model::{Annotated, RawResource};
    use serde_json::json;
    use test_r::test;

    const TEST_URI: &str = "golem://Agent/resource";

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
    fn component_model_to_text_resource_json() {
        let response = SchemaValue::String("sunny".to_string());
        let contents = map_agent_response_to_resource_contents(
            &graph(),
            &str_output(),
            Some(response),
            TEST_URI,
        )
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
        let output = OutputSchema::Single(Box::new(SchemaType::text(TextRestrictions::default())));
        let response = SchemaValue::Text(TextValuePayload {
            text: "rainy day".to_string(),
            language: None,
        });
        let contents =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI)
                .unwrap();
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
        let output =
            OutputSchema::Single(Box::new(SchemaType::binary(BinaryRestrictions::default())));
        let response = SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![1, 2, 3],
            mime_type: Some("image/png".to_string()),
        });
        let contents =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI)
                .unwrap();
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
            map_agent_response_to_resource_contents(&graph(), &str_output(), None, TEST_URI)
                .unwrap();
        assert!(contents.is_empty());
    }

    #[test]
    fn multimodal_returns_multiple_contents() {
        let output = multimodal_output(vec![
            ("text", SchemaType::text(TextRestrictions::default())),
            ("img", SchemaType::binary(BinaryRestrictions::default())),
        ]);
        let response = SchemaValue::List {
            elements: vec![
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(SchemaValue::Text(TextValuePayload {
                        text: "snow report".to_string(),
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
        let contents =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI)
                .unwrap();
        assert_eq!(contents.len(), 2);
        assert!(
            matches!(&contents[0], ResourceContents::TextResourceContents { text, .. } if text == "snow report")
        );
        assert!(
            matches!(&contents[1], ResourceContents::BlobResourceContents { blob, .. } if blob == "AQID")
        );
    }

    #[test]
    async fn invoke_resource_auto_generates_phantom_for_ephemeral_agents() {
        let harness = InvocationHarness::new(AgentInvocationOutput {
            result: golem_common::model::AgentInvocationResult::AgentInitialization,
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            oplog_index: None,
            agent_fingerprint: None,
        });
        let resource = AgentMcpResource {
            kind: AgentMcpResourceKind::Static(Annotated::new(
                RawResource {
                    uri: TEST_URI.to_string(),
                    name: "mcp-agent-read".to_string(),
                    title: None,
                    description: None,
                    mime_type: None,
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            )),
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
                name: "read".to_string(),
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

        let result = invoke_resource(&harness.worker_service, &resource, TEST_URI, None).await;

        assert!(result.is_ok());
        let agent_id = harness.recorded_agent_id();
        assert_eq!(agent_id.component_id, harness.component_id);
        assert!(phantom_id(&agent_id).is_some());
    }
}
