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
use golem_common::model::agent::{
    BinaryReference, ComponentModelElementValue, DataSchema, DataValue, ElementValue,
    ElementValues, TextReference, UnstructuredBinaryElementValue, UnstructuredTextElementValue,
    UntypedDataValue,
};
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;
use http::StatusCode;
use std::collections::HashMap;
use tracing::debug;

pub fn interpret_agent_response(
    invoke_result: Option<UntypedDataValue>,
    expected_type: &DataSchema,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    match invoke_result {
        Some(untyped_data_value) => {
            let mapped_response = map_successful_agent_response(untyped_data_value, expected_type)?;
            Ok(mapped_response)
        }
        None => Ok(RouteExecutionResult {
            status: StatusCode::NO_CONTENT,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        }),
    }
}

fn map_successful_agent_response(
    agent_response: UntypedDataValue,
    expected_type: &DataSchema,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let typed_value = DataValue::try_from_untyped(agent_response, expected_type.clone())
        .map_err(|error| RequestHandlerError::AgentResponseTypeMismatch { error })?;

    debug!("Received successful agent response: {typed_value:?}");

    match typed_value {
        DataValue::Tuple(ElementValues { elements }) => match elements.len() {
            0 => Ok(RouteExecutionResult {
                status: StatusCode::NO_CONTENT,
                headers: HashMap::new(),
                body: ResponseBody::NoBody,
            }),
            1 => map_single_element_agent_response(elements.into_iter().next().unwrap()),
            _ => Err(RequestHandlerError::invariant_violated(
                "Unexpected number of response tuple elements",
            )),
        },
        DataValue::Multimodal(_) => Err(RequestHandlerError::invariant_violated(
            "Unexpected multimodal response",
        )),
    }
}

fn map_single_element_agent_response(
    element: ElementValue,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    match element {
        ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
            map_component_model_agent_response(value)
        }

        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
            value: BinaryReference::Inline(binary),
            ..
        }) => Ok(RouteExecutionResult {
            status: StatusCode::OK,
            headers: HashMap::new(),
            body: ResponseBody::UnstructuredBinaryBody { body: binary },
        }),

        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
            value: BinaryReference::Url(_),
            ..
        }) => Err(RequestHandlerError::invariant_violated(
            "Unexpected unstructured binary URL response",
        )),

        ElementValue::UnstructuredText(UnstructuredTextElementValue {
            value: TextReference::Inline(text),
            ..
        }) => Ok(RouteExecutionResult {
            status: StatusCode::OK,
            headers: HashMap::new(),
            body: ResponseBody::UnstructuredTextBody { body: text },
        }),

        ElementValue::UnstructuredText(UnstructuredTextElementValue {
            value: TextReference::Url(_),
            ..
        }) => Err(RequestHandlerError::invariant_violated(
            "Unexpected unstructured text URL response",
        )),
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
    use golem_common::model::agent::{
        BinaryDescriptor, BinaryReference, BinaryReferenceValue, BinarySource, BinaryType,
        ElementSchema, NamedElementSchema, NamedElementSchemas, TextDescriptor, TextReference,
        TextReferenceValue, TextSource, TextType, UntypedDataValue, UntypedElementValue, Url,
    };
    use test_r::test;

    fn text_schema(restrictions: Option<Vec<TextType>>) -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "body".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions }),
            }],
        })
    }

    fn binary_schema(restrictions: Option<Vec<BinaryType>>) -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "body".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions }),
            }],
        })
    }

    #[test]
    fn inline_text_response_returns_200_with_unstructured_text_body() {
        let schema = text_schema(None);
        let invoke_result = Some(UntypedDataValue::Tuple(vec![
            UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: "hello".to_string(),
                    text_type: Some(TextType {
                        language_code: "en".to_string(),
                    }),
                }),
            }),
        ]));

        let result = interpret_agent_response(invoke_result, &schema).unwrap();

        assert_eq!(result.status, StatusCode::OK);
        let_assert!(ResponseBody::UnstructuredTextBody { body } = result.body);
        assert_eq!(body.data, "hello");
        let text_type = body.text_type.unwrap();
        assert_eq!(text_type.language_code, "en");
    }

    #[test]
    fn inline_text_response_without_language_returns_200() {
        let schema = text_schema(None);
        let invoke_result = Some(UntypedDataValue::Tuple(vec![
            UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: "hi".to_string(),
                    text_type: None,
                }),
            }),
        ]));

        let result = interpret_agent_response(invoke_result, &schema).unwrap();

        assert_eq!(result.status, StatusCode::OK);
        let_assert!(ResponseBody::UnstructuredTextBody { body } = result.body);
        assert_eq!(body.data, "hi");
        assert!(body.text_type.is_none());
    }

    #[test]
    fn url_text_response_is_invariant_violation() {
        let schema = text_schema(None);
        let invoke_result = Some(UntypedDataValue::Tuple(vec![
            UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Url(Url {
                    value: "https://example.com/text".into(),
                }),
            }),
        ]));

        let err = interpret_agent_response(invoke_result, &schema).unwrap_err();

        let_assert!(RequestHandlerError::InvariantViolated { msg } = err);
        assert!(msg.contains("text"));
    }

    #[test]
    fn inline_binary_response_returns_200_with_unstructured_binary_body() {
        let schema = binary_schema(None);
        let invoke_result = Some(UntypedDataValue::Tuple(vec![
            UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                value: BinaryReference::Inline(BinarySource {
                    data: vec![0x01, 0x02, 0x03],
                    binary_type: BinaryType {
                        mime_type: "application/octet-stream".into(),
                    },
                }),
            }),
        ]));

        let result = interpret_agent_response(invoke_result, &schema).unwrap();

        assert_eq!(result.status, StatusCode::OK);
        let_assert!(ResponseBody::UnstructuredBinaryBody { body } = result.body);
        assert_eq!(body.data, vec![0x01, 0x02, 0x03]);
        assert_eq!(body.binary_type.mime_type, "application/octet-stream");
    }

    #[test]
    fn url_binary_response_is_invariant_violation() {
        let schema = binary_schema(None);
        let invoke_result = Some(UntypedDataValue::Tuple(vec![
            UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                value: BinaryReference::Url(Url {
                    value: "https://example.com/blob".into(),
                }),
            }),
        ]));

        let err = interpret_agent_response(invoke_result, &schema).unwrap_err();

        let_assert!(RequestHandlerError::InvariantViolated { msg } = err);
        assert!(msg.contains("binary"));
    }

    #[test]
    fn no_response_returns_204() {
        let schema = text_schema(None);
        let result = interpret_agent_response(None, &schema).unwrap();
        assert_eq!(result.status, StatusCode::NO_CONTENT);
        let_assert!(ResponseBody::NoBody = result.body);
    }
}
