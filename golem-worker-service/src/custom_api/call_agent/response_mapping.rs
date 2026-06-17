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

use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::{ResponseBody, RouteExecutionResult};
use golem_common::model::agent::{BinarySource, BinaryType, TextSource, TextType};
use golem_common::schema::adapters::{
    is_multimodal_schema_type, resolve_ref, schema_type_to_analysed_type, schema_value_to_value,
};
use golem_common::schema::{BinaryValuePayload, SchemaType, SchemaValue, TextValuePayload};
use golem_service_base::custom_api::CompiledOutputSchema;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;
use http::StatusCode;
use std::collections::HashMap;

pub fn interpret_agent_response(
    invoke_result: Option<SchemaValue>,
    expected: &CompiledOutputSchema,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    match invoke_result {
        Some(schema_value) => map_successful_agent_response(schema_value, expected),
        None => Ok(no_content()),
    }
}

fn no_content() -> RouteExecutionResult {
    RouteExecutionResult {
        status: StatusCode::NO_CONTENT,
        headers: HashMap::new(),
        body: ResponseBody::NoBody,
    }
}

fn map_successful_agent_response(
    agent_response: SchemaValue,
    expected: &CompiledOutputSchema,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let graph = &expected.graph;

    // A unit output produces no body.
    let Some(output_type) = expected.output_schema.schema() else {
        return Ok(no_content());
    };

    if is_multimodal_schema_type(graph, output_type).map_err(map_schema_error)? {
        return Err(RequestHandlerError::invariant_violated(
            "Unexpected multimodal response",
        ));
    }

    let resolved = resolve_ref(graph, output_type).map_err(map_schema_error)?;

    match resolved {
        SchemaType::Text { .. } => match agent_response {
            SchemaValue::Text(TextValuePayload { text, language }) => Ok(RouteExecutionResult {
                status: StatusCode::OK,
                headers: HashMap::new(),
                body: ResponseBody::UnstructuredTextBody {
                    body: TextSource {
                        data: text,
                        text_type: language.map(|language_code| TextType { language_code }),
                    },
                },
            }),
            _ => Err(RequestHandlerError::invariant_violated(
                "Expected text response value for text output schema",
            )),
        },

        SchemaType::Binary { .. } => match agent_response {
            SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }) => {
                Ok(RouteExecutionResult {
                    status: StatusCode::OK,
                    headers: HashMap::new(),
                    body: ResponseBody::UnstructuredBinaryBody {
                        body: BinarySource {
                            data: bytes,
                            binary_type: BinaryType {
                                mime_type: mime_type
                                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                            },
                        },
                    },
                })
            }
            _ => Err(RequestHandlerError::invariant_violated(
                "Expected binary response value for binary output schema",
            )),
        },

        _ => {
            let value = schema_value_to_value(graph, output_type, &agent_response)
                .map_err(map_schema_error)?;
            let typ = schema_type_to_analysed_type(graph, output_type).map_err(map_schema_error)?;
            map_component_model_agent_response(ValueAndType { value, typ })
        }
    }
}

fn map_schema_error(
    error: golem_common::schema::adapters::SchemaAdapterError,
) -> RequestHandlerError {
    RequestHandlerError::AgentResponseTypeMismatch {
        error: error.to_string(),
    }
}

fn map_component_model_agent_response(
    value_and_type: ValueAndType,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    use golem_wasm::Value;

    match value_and_type.value {
        Value::Option(None) => Ok(RouteExecutionResult {
            status: StatusCode::NOT_FOUND,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        }),

        Value::Option(Some(inner)) => {
            let inner_type = unwrap_option_type(value_and_type.typ)?;
            Ok(RouteExecutionResult {
                status: StatusCode::OK,
                headers: HashMap::new(),
                body: json_response_body(*inner, inner_type),
            })
        }

        Value::Result(Ok(None)) => Ok(RouteExecutionResult {
            status: StatusCode::NO_CONTENT,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        }),

        Value::Result(Ok(Some(inner))) => {
            let inner_type = unwrap_result_ok_type(value_and_type.typ)?;
            Ok(RouteExecutionResult {
                status: StatusCode::OK,
                headers: HashMap::new(),
                body: json_response_body(*inner, inner_type),
            })
        }

        Value::Result(Err(None)) => Ok(RouteExecutionResult {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        }),

        Value::Result(Err(Some(inner))) => {
            let inner_type = unwrap_result_err_type(value_and_type.typ)?;
            Ok(RouteExecutionResult {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                headers: HashMap::new(),
                body: json_response_body(*inner, inner_type),
            })
        }

        other => Ok(RouteExecutionResult {
            status: StatusCode::OK,
            headers: HashMap::new(),
            body: json_response_body(other, value_and_type.typ),
        }),
    }
}

fn unwrap_option_type(typ: AnalysedType) -> Result<AnalysedType, RequestHandlerError> {
    use golem_wasm::analysis;

    if let AnalysedType::Option(analysis::TypeOption { inner, .. }) = typ {
        Ok(*inner)
    } else {
        Err(RequestHandlerError::invariant_violated(
            "analysed type did not match value",
        ))
    }
}

fn unwrap_result_ok_type(typ: AnalysedType) -> Result<AnalysedType, RequestHandlerError> {
    use golem_wasm::analysis;

    if let AnalysedType::Result(analysis::TypeResult {
        ok: Some(inner), ..
    }) = typ
    {
        Ok(*inner)
    } else {
        Err(RequestHandlerError::invariant_violated(
            "analysed type did not match value",
        ))
    }
}

fn unwrap_result_err_type(typ: AnalysedType) -> Result<AnalysedType, RequestHandlerError> {
    use golem_wasm::analysis;

    if let AnalysedType::Result(analysis::TypeResult {
        err: Some(inner), ..
    }) = typ
    {
        Ok(*inner)
    } else {
        Err(RequestHandlerError::invariant_violated(
            "analysed type did not match value",
        ))
    }
}

fn json_response_body(value: golem_wasm::Value, typ: AnalysedType) -> ResponseBody {
    ResponseBody::ComponentModelJsonBody {
        body: ValueAndType::new(value, typ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::let_assert;
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::schema_type::{BinaryRestrictions, TextRestrictions};
    use golem_common::schema::{BinaryValuePayload, OutputSchema, TextValuePayload};
    use test_r::test;

    fn text_output() -> CompiledOutputSchema {
        let ty = SchemaType::text(TextRestrictions::default());
        CompiledOutputSchema {
            graph: SchemaGraph::anonymous(ty.clone()),
            output_schema: OutputSchema::Single(Box::new(ty)),
        }
    }

    fn binary_output() -> CompiledOutputSchema {
        let ty = SchemaType::binary(BinaryRestrictions::default());
        CompiledOutputSchema {
            graph: SchemaGraph::anonymous(ty.clone()),
            output_schema: OutputSchema::Single(Box::new(ty)),
        }
    }

    fn unit_output() -> CompiledOutputSchema {
        CompiledOutputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
            output_schema: OutputSchema::Unit,
        }
    }

    #[test]
    fn inline_text_response_returns_200_with_unstructured_text_body() {
        let schema = text_output();
        let invoke_result = Some(SchemaValue::Text(TextValuePayload {
            text: "hello".to_string(),
            language: Some("en".to_string()),
        }));

        let result = interpret_agent_response(invoke_result, &schema).unwrap();

        assert_eq!(result.status, StatusCode::OK);
        let_assert!(ResponseBody::UnstructuredTextBody { body } = result.body);
        assert_eq!(body.data, "hello");
        let text_type = body.text_type.unwrap();
        assert_eq!(text_type.language_code, "en");
    }

    #[test]
    fn inline_text_response_without_language_returns_200() {
        let schema = text_output();
        let invoke_result = Some(SchemaValue::Text(TextValuePayload {
            text: "hi".to_string(),
            language: None,
        }));

        let result = interpret_agent_response(invoke_result, &schema).unwrap();

        assert_eq!(result.status, StatusCode::OK);
        let_assert!(ResponseBody::UnstructuredTextBody { body } = result.body);
        assert_eq!(body.data, "hi");
        assert!(body.text_type.is_none());
    }

    #[test]
    fn inline_binary_response_returns_200_with_unstructured_binary_body() {
        let schema = binary_output();
        let invoke_result = Some(SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![0x01, 0x02, 0x03],
            mime_type: Some("application/octet-stream".into()),
        }));

        let result = interpret_agent_response(invoke_result, &schema).unwrap();

        assert_eq!(result.status, StatusCode::OK);
        let_assert!(ResponseBody::UnstructuredBinaryBody { body } = result.body);
        assert_eq!(body.data, vec![0x01, 0x02, 0x03]);
        assert_eq!(body.binary_type.mime_type, "application/octet-stream");
    }

    #[test]
    fn no_response_returns_204() {
        let result = interpret_agent_response(None, &unit_output()).unwrap();
        assert_eq!(result.status, StatusCode::NO_CONTENT);
        let_assert!(ResponseBody::NoBody = result.body);
    }

    #[test]
    fn unit_output_with_value_returns_204() {
        let invoke_result = Some(SchemaValue::Record { fields: vec![] });
        let result = interpret_agent_response(invoke_result, &unit_output()).unwrap();
        assert_eq!(result.status, StatusCode::NO_CONTENT);
        let_assert!(ResponseBody::NoBody = result.body);
    }
}
