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
use golem_common::schema::adapters::{
    UnstructuredOutput, decode_unstructured_output, multimodal_variant_cases,
};
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

/// Convert a single agent response value, typed by `ty` (resolved against
/// `graph`), into MCP `ResourceContents`.
///
/// Raw `Text` / `Binary` values map onto a text / blob resource, whether they
/// arrive bare or as the `inline` case of a canonical unstructured
/// `variant { inline, url }` wrapper. The wrapper's `url` case (a resource read
/// cannot redirect) is surfaced as a `text/uri-list` resource carrying the URL.
/// Every other (component-model) value renders as an `application/json` text
/// resource.
fn schema_value_to_resource_content(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    uri: &str,
) -> Result<ResourceContents, ErrorData> {
    // An unstructured text/binary output — either the canonical
    // `variant { inline, url }` wrapper or a bare `Text` / `Binary` rich scalar.
    // `inline` / raw values map onto a text / blob resource; the wrapper's `url`
    // case (a resource read cannot redirect) is surfaced as a `text/uri-list`
    // resource carrying the URL. The classifier also validates the value matches
    // the output kind; every other value renders as an `application/json` text
    // resource.
    if let Some(output) = decode_unstructured_output(graph, ty, value).map_err(internal_error)? {
        return match output {
            UnstructuredOutput::Url(url) => Ok(ResourceContents::TextResourceContents {
                uri: uri.to_string(),
                mime_type: Some("text/uri-list".to_string()),
                text: url.to_string(),
                meta: None,
            }),
            UnstructuredOutput::Inline(inline) => {
                value_to_resource_content(inline, uri).ok_or_else(|| {
                    internal_error("unstructured `inline` value must be a text or binary value")
                })
            }
        };
    }

    let json_value = to_json_value(graph, ty, value).map_err(|e| {
        internal_error(format!("Failed to serialize component model response: {e}"))
    })?;
    Ok(ResourceContents::TextResourceContents {
        uri: uri.to_string(),
        mime_type: Some("application/json".to_string()),
        text: json_value.to_string(),
        meta: None,
    })
}

/// Render a raw `Text` / `Binary` value into MCP `ResourceContents` (text or
/// blob). Returns `None` for any other value.
fn value_to_resource_content(value: &SchemaValue, uri: &str) -> Option<ResourceContents> {
    match value {
        // Note that languageCode cannot be encoded in the output to MCP clients
        // when they act as resources. `ResourceContents::text` takes (text, uri)
        // in that order.
        SchemaValue::Text(TextValuePayload { text, .. }) => {
            Some(ResourceContents::text(text.clone(), uri.to_string()))
        }
        SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }) => {
            Some(ResourceContents::BlobResourceContents {
                uri: uri.to_string(),
                mime_type: mime_type.clone(),
                blob: base64::engine::general_purpose::STANDARD.encode(bytes),
                meta: None,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::agent_mcp_resource::{AgentMcpResource, AgentMcpResourceKind};
    use crate::mcp::invoke::test_support::{InvocationHarness, phantom_id};
    use golem_common::base_model::agent::{AgentMode, AgentTypeName};
    use golem_common::model::AgentInvocationOutput;
    use golem_common::schema::adapters::unstructured::{
        unstructured_binary_schema_type, unstructured_inline_value, unstructured_text_schema_type,
        unstructured_url_value,
    };
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
        let mut list = SchemaType::list(SchemaType::variant(variant_cases));
        list.metadata_mut().role = Some(Role::Multimodal);
        OutputSchema::Single(Box::new(list))
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
    fn unstructured_text_to_text_resource() {
        let output = OutputSchema::Single(Box::new(unstructured_text_schema_type(
            TextRestrictions::default(),
        )));
        let response = unstructured_inline_value(SchemaValue::Text(TextValuePayload {
            text: "rainy day".to_string(),
            language: None,
        }));
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
    fn text_wrapper_schema_accepts_raw_text_value() {
        // DE: a wrapper-typed output schema must also accept a raw `Text` value.
        let output = OutputSchema::Single(Box::new(unstructured_text_schema_type(
            TextRestrictions::default(),
        )));
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
    fn text_wrapper_schema_with_raw_binary_value_is_rejected() {
        let output = OutputSchema::Single(Box::new(unstructured_text_schema_type(
            TextRestrictions::default(),
        )));
        let response = SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![1, 2, 3],
            mime_type: Some("image/png".to_string()),
        });
        let result =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI);
        assert!(result.is_err());
    }

    #[test]
    fn unstructured_binary_to_blob_resource() {
        let output = OutputSchema::Single(Box::new(unstructured_binary_schema_type(
            BinaryRestrictions::default(),
        )));
        let response = unstructured_inline_value(SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![1, 2, 3],
            mime_type: Some("image/png".to_string()),
        }));
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
    fn unstructured_text_url_to_uri_list_resource() {
        let output = OutputSchema::Single(Box::new(unstructured_text_schema_type(
            TextRestrictions::default(),
        )));
        let response = unstructured_url_value("https://example.com/doc.txt".to_string());
        let contents =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI)
                .unwrap();
        assert_eq!(contents.len(), 1);
        match &contents[0] {
            ResourceContents::TextResourceContents {
                text, mime_type, ..
            } => {
                assert_eq!(text, "https://example.com/doc.txt");
                assert_eq!(mime_type.as_deref(), Some("text/uri-list"));
            }
            _ => panic!("expected TextResourceContents"),
        }
    }

    #[test]
    fn unstructured_binary_url_to_uri_list_resource() {
        let output = OutputSchema::Single(Box::new(unstructured_binary_schema_type(
            BinaryRestrictions::default(),
        )));
        let response = unstructured_url_value("https://example.com/blob.bin".to_string());
        let contents =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI)
                .unwrap();
        assert_eq!(contents.len(), 1);
        match &contents[0] {
            ResourceContents::TextResourceContents {
                text, mime_type, ..
            } => {
                assert_eq!(text, "https://example.com/blob.bin");
                assert_eq!(mime_type.as_deref(), Some("text/uri-list"));
            }
            _ => panic!("expected TextResourceContents"),
        }
    }

    #[test]
    fn raw_text_value_to_text_resource() {
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
    fn raw_binary_value_to_blob_resource() {
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
    fn text_output_with_binary_value_is_rejected() {
        // Schema-driven output mapping: a text output paired with a binary value
        // is a type mismatch and must error rather than render a blob resource.
        let output = OutputSchema::Single(Box::new(SchemaType::text(TextRestrictions::default())));
        let response = SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![1, 2, 3],
            mime_type: None,
        });
        let result =
            map_agent_response_to_resource_contents(&graph(), &output, Some(response), TEST_URI);
        assert!(result.is_err());
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
            (
                "text",
                unstructured_text_schema_type(TextRestrictions::default()),
            ),
            (
                "img",
                unstructured_binary_schema_type(BinaryRestrictions::default()),
            ),
        ]);
        let response = SchemaValue::List {
            elements: vec![
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(unstructured_inline_value(SchemaValue::Text(
                        TextValuePayload {
                            text: "snow report".to_string(),
                            language: None,
                        },
                    )))),
                }),
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(unstructured_inline_value(SchemaValue::Binary(
                        BinaryValuePayload {
                            bytes: vec![1, 2, 3],
                            mime_type: Some("image/png".to_string()),
                        },
                    )))),
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
