// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::ParsedRequestBody;
use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::model::RichRequest;
use anyhow::anyhow;
use golem_common::model::agent::{BinarySource, BinaryType, UntypedElementValue};
use golem_service_base::custom_api::{PathSegmentType, QueryOrHeaderType, RequestBodySchema};
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;

pub fn parse_path_segment_value(
    value: String,
    r#type: &PathSegmentType,
) -> Result<UntypedElementValue, RequestHandlerError> {
    parse_path_segment_value_to_component_model(value, r#type)
        .map(UntypedElementValue::ComponentModel)
}

pub fn parse_path_segment_value_to_component_model(
    value: String,
    r#type: &PathSegmentType,
) -> Result<golem_wasm::Value, RequestHandlerError> {
    match r#type {
        PathSegmentType::Str => Ok(golem_wasm::Value::String(value)),

        PathSegmentType::Chr => {
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(golem_wasm::Value::Char(c)),
                _ => Err(RequestHandlerError::ValueParsingFailed {
                    value,
                    expected: "char",
                }),
            }
        }

        PathSegmentType::F64 => value
            .parse::<f64>()
            .map(golem_wasm::Value::F64)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "f64",
            }),

        PathSegmentType::F32 => value
            .parse::<f32>()
            .map(golem_wasm::Value::F32)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "f32",
            }),

        PathSegmentType::U64 => value
            .parse::<u64>()
            .map(golem_wasm::Value::U64)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "u64",
            }),

        PathSegmentType::S64 => value
            .parse::<i64>()
            .map(golem_wasm::Value::S64)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "i64",
            }),

        PathSegmentType::U32 => value
            .parse::<u32>()
            .map(golem_wasm::Value::U32)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "u32",
            }),

        PathSegmentType::S32 => value
            .parse::<i32>()
            .map(golem_wasm::Value::S32)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "i32",
            }),

        PathSegmentType::U16 => value
            .parse::<u16>()
            .map(golem_wasm::Value::U16)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "u16",
            }),

        PathSegmentType::S16 => value
            .parse::<i16>()
            .map(golem_wasm::Value::S16)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "i16",
            }),

        PathSegmentType::U8 => value.parse::<u8>().map(golem_wasm::Value::U8).map_err(|_| {
            RequestHandlerError::ValueParsingFailed {
                value,
                expected: "u8",
            }
        }),

        PathSegmentType::S8 => value.parse::<i8>().map(golem_wasm::Value::S8).map_err(|_| {
            RequestHandlerError::ValueParsingFailed {
                value,
                expected: "i8",
            }
        }),

        PathSegmentType::Bool => value
            .parse::<bool>()
            .map(golem_wasm::Value::Bool)
            .map_err(|_| RequestHandlerError::ValueParsingFailed {
                value,
                expected: "bool",
            }),

        PathSegmentType::Enum(inner) => {
            let case_index = inner
                .cases
                .iter()
                .position(|c| *c == value)
                .ok_or_else(|| RequestHandlerError::ValueParsingFailed {
                    value,
                    expected: "enum variant",
                })?;

            Ok(golem_wasm::Value::Enum(
                case_index
                    .try_into()
                    .expect("could not convert usize to u32"),
            ))
        }
    }
}

pub fn parse_query_or_header_value(
    values: &[String],
    r#type: &QueryOrHeaderType,
) -> Result<UntypedElementValue, RequestHandlerError> {
    match r#type {
        QueryOrHeaderType::Primitive(inner) => {
            if values.len() > 1 {
                return Err(RequestHandlerError::TooManyValues {
                    expected: "single value",
                });
            }

            let value = values
                .iter()
                .next()
                .ok_or_else(|| RequestHandlerError::MissingValue {
                    expected: "single value",
                })?;

            parse_path_segment_value(value.clone(), inner)
        }

        QueryOrHeaderType::Option { inner, .. } => match values.len() {
            0 => Ok(UntypedElementValue::ComponentModel(
                golem_wasm::Value::Option(None),
            )),

            1 => {
                let parsed = parse_path_segment_value_to_component_model(
                    values.iter().next().unwrap().clone(),
                    inner,
                )?;
                Ok(UntypedElementValue::ComponentModel(
                    golem_wasm::Value::Option(Some(Box::new(parsed))),
                ))
            }

            _ => Err(RequestHandlerError::TooManyValues {
                expected: "zero or one value",
            }),
        },

        QueryOrHeaderType::List { inner, .. } => {
            let mut parsed_values = Vec::with_capacity(values.len());

            for value in values {
                parsed_values.push(parse_path_segment_value_to_component_model(
                    value.clone(),
                    inner,
                )?);
            }

            Ok(UntypedElementValue::ComponentModel(
                golem_wasm::Value::List(parsed_values),
            ))
        }
    }
}

// Will consume the request body
pub async fn parse_request_body(
    request: &mut RichRequest,
    expected: &RequestBodySchema,
) -> Result<ParsedRequestBody, RequestHandlerError> {
    match expected {
        RequestBodySchema::Unused => Ok(ParsedRequestBody::Unused),

        RequestBodySchema::JsonBody { expected_type } => {
            let json_body: serde_json::Value = request
                .underlying
                .take_body()
                .into_json()
                .await
                .map_err(|err| RequestHandlerError::BodyIsNotValidJson {
                    error: err.to_string(),
                })?;
            let parsed_body = ValueAndType::parse_with_type(&json_body, expected_type)
                .map_err(|errors| RequestHandlerError::JsonBodyParsingFailed { errors })?;
            Ok(ParsedRequestBody::JsonBody(parsed_body.value))
        }

        RequestBodySchema::UnrestrictedBinary => parse_binary_body(request, None).await,

        RequestBodySchema::RestrictedBinary { allowed_mime_types } => {
            parse_binary_body(request, Some(allowed_mime_types)).await
        }
    }
}

async fn parse_binary_body(
    request: &mut RichRequest,
    allowed_mime_types: Option<&Vec<String>>,
) -> Result<ParsedRequestBody, RequestHandlerError> {
    let data = request
        .underlying
        .take_body()
        .into_vec()
        .await
        .map_err(|err| anyhow!("Failed reading raw body: {err}"))?;

    let header_name = http::header::CONTENT_TYPE.to_string();

    let mime_type = request
        .headers()
        .get(header_name.clone())
        .map(|value| value.to_str())
        .transpose()
        .map_err(|_| RequestHandlerError::HeaderIsNotAscii { header_name })?
        .map(|v| v.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    if let Some(allowed) = allowed_mime_types
        && !allowed.iter().any(|allowed| allowed == &mime_type)
    {
        return Err(RequestHandlerError::UnsupportedMimeType {
            mime_type,
            allowed_mime_types: allowed.clone(),
        });
    }

    Ok(ParsedRequestBody::UnstructuredBinary(Some(BinarySource {
        data,
        binary_type: BinaryType { mime_type },
    })))
}

#[cfg(test)]
mod path_segment_tests {
    use super::*;
    use assert2::assert;
    use golem_service_base::custom_api::PathSegmentType;
    use golem_wasm::analysis::TypeEnum;
    use test_r::test;

    #[test]
    fn parse_string_path_segment() {
        let result = parse_path_segment_value("hello".to_string(), &PathSegmentType::Str).unwrap();

        assert_eq!(
            result,
            UntypedElementValue::ComponentModel(golem_wasm::Value::String("hello".into()))
        );
    }

    #[test]
    fn parse_char_success() {
        let result = parse_path_segment_value("a".to_string(), &PathSegmentType::Chr).unwrap();

        assert_eq!(
            result,
            UntypedElementValue::ComponentModel(golem_wasm::Value::Char('a'))
        );
    }

    #[test]
    fn parse_char_failure_multiple_chars() {
        let err = parse_path_segment_value("ab".to_string(), &PathSegmentType::Chr).unwrap_err();

        assert!(let RequestHandlerError::ValueParsingFailed {
            expected: "char",
            ..
        } = err);
    }

    #[test]
    fn parse_numeric_success() {
        let cases = vec![
            (PathSegmentType::U8, "12", golem_wasm::Value::U8(12)),
            (PathSegmentType::S16, "-5", golem_wasm::Value::S16(-5)),
            (PathSegmentType::U32, "42", golem_wasm::Value::U32(42)),
            (PathSegmentType::S64, "-100", golem_wasm::Value::S64(-100)),
            (PathSegmentType::F32, "1.5", golem_wasm::Value::F32(1.5)),
            (PathSegmentType::F64, "2.25", golem_wasm::Value::F64(2.25)),
        ];

        for (ty, input, expected) in cases {
            let value =
                parse_path_segment_value_to_component_model(input.to_string(), &ty).unwrap();

            assert_eq!(value, expected);
        }
    }

    #[test]
    fn parse_numeric_failure() {
        let err = parse_path_segment_value_to_component_model(
            "not-a-number".to_string(),
            &PathSegmentType::U32,
        )
        .unwrap_err();

        assert!(let RequestHandlerError::ValueParsingFailed {
            expected: "u32",
            ..
        } = err);
    }

    #[test]
    fn parse_bool_success() {
        let value =
            parse_path_segment_value_to_component_model("true".to_string(), &PathSegmentType::Bool)
                .unwrap();

        assert_eq!(value, golem_wasm::Value::Bool(true));
    }

    #[test]
    fn parse_enum_success() {
        let enum_type = PathSegmentType::Enum(TypeEnum {
            owner: None,
            name: None,
            cases: vec!["red".into(), "green".into(), "blue".into()],
        });

        let value =
            parse_path_segment_value_to_component_model("green".to_string(), &enum_type).unwrap();

        assert_eq!(value, golem_wasm::Value::Enum(1));
    }

    #[test]
    fn parse_enum_failure() {
        let enum_type = PathSegmentType::Enum(TypeEnum {
            owner: None,
            name: None,
            cases: vec!["a".into(), "b".into()],
        });

        let err =
            parse_path_segment_value_to_component_model("c".to_string(), &enum_type).unwrap_err();

        assert!(let             RequestHandlerError::ValueParsingFailed {
            expected: "enum variant",
            ..
        } = err);
    }
}

#[cfg(test)]
mod query_or_header_tests {
    use super::*;
    use assert2::assert;
    use golem_service_base::custom_api::QueryOrHeaderType;
    use test_r::test;

    #[test]
    fn primitive_single_value_ok() {
        let values = vec!["42".to_string()];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Primitive(PathSegmentType::U32),
        )
        .unwrap();

        assert_eq!(
            result,
            UntypedElementValue::ComponentModel(golem_wasm::Value::U32(42))
        );
    }

    #[test]
    fn primitive_missing_value() {
        let values: Vec<String> = vec![];

        let err = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Primitive(PathSegmentType::U32),
        )
        .unwrap_err();

        assert!(let RequestHandlerError::MissingValue { .. } = err);
    }

    #[test]
    fn primitive_too_many_values() {
        let values = vec!["1".to_string(), "2".to_string()];

        let err = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Primitive(PathSegmentType::U32),
        )
        .unwrap_err();

        assert!(let RequestHandlerError::TooManyValues { .. } = err);
    }

    #[test]
    fn option_no_value() {
        let values: Vec<String> = vec![];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Option {
                owner: None,
                name: None,
                inner: Box::new(PathSegmentType::Bool),
            },
        )
        .unwrap();

        assert_eq!(
            result,
            UntypedElementValue::ComponentModel(golem_wasm::Value::Option(None))
        );
    }

    #[test]
    fn option_single_value() {
        let values = vec!["true".to_string()];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Option {
                owner: None,
                name: None,
                inner: Box::new(PathSegmentType::Bool),
            },
        )
        .unwrap();

        assert_eq!(
            result,
            UntypedElementValue::ComponentModel(golem_wasm::Value::Option(Some(Box::new(
                golem_wasm::Value::Bool(true)
            ))))
        );
    }

    #[test]
    fn option_too_many_values() {
        let values = vec!["true".to_string(), "false".to_string()];

        let err = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::Option {
                owner: None,
                name: None,
                inner: Box::new(PathSegmentType::Bool),
            },
        )
        .unwrap_err();

        assert!(let RequestHandlerError::TooManyValues { .. } = err);
    }

    #[test]
    fn list_values() {
        let values = vec!["1".to_string(), "2".to_string(), "3".to_string()];

        let result = parse_query_or_header_value(
            &values,
            &QueryOrHeaderType::List {
                owner: None,
                name: None,
                inner: Box::new(PathSegmentType::U8),
            },
        )
        .unwrap();

        assert_eq!(
            result,
            UntypedElementValue::ComponentModel(golem_wasm::Value::List(vec![
                golem_wasm::Value::U8(1),
                golem_wasm::Value::U8(2),
                golem_wasm::Value::U8(3),
            ]))
        );
    }
}

#[cfg(test)]
mod request_body_tests {
    use super::*;
    use assert2::{assert, let_assert};
    use golem_service_base::custom_api::RequestBodySchema;
    use golem_wasm::analysis::{NameTypePair, analysed_type};
    use http::Method;
    use poem::{Body, Request};
    use serde_json::json;
    use test_r::test;

    fn raw_request_with_content_type(
        bytes: &'static [u8],
        content_type: &'static str,
    ) -> RichRequest {
        let req = Request::builder()
            .method(Method::POST)
            .header(http::header::CONTENT_TYPE, content_type)
            .body(Body::from(bytes));
        RichRequest::new(req)
    }

    fn json_request(value: serde_json::Value) -> RichRequest {
        let req = Request::builder()
            .method(Method::POST)
            .body(Body::from_json(value).unwrap());
        RichRequest::new(req)
    }

    fn raw_request(bytes: &'static [u8]) -> RichRequest {
        let req = Request::builder()
            .method(Method::POST)
            .body(Body::from(bytes));
        RichRequest::new(req)
    }

    #[test]
    async fn unused_body_schema_does_not_consume_body() {
        let mut request = json_request(json!({ "x": 1 }));

        let result = parse_request_body(&mut request, &RequestBodySchema::Unused)
            .await
            .unwrap();

        assert!(let ParsedRequestBody::Unused = result);
    }

    #[test]
    async fn valid_json_body_is_parsed() {
        let mut request = json_request(json!({
            "x": 1
        }));

        let schema = RequestBodySchema::JsonBody {
            expected_type: analysed_type::record(vec![NameTypePair {
                name: String::from("x"),
                typ: analysed_type::s32(),
            }]),
        };

        let result = parse_request_body(&mut request, &schema).await.unwrap();

        let_assert!(ParsedRequestBody::JsonBody(golem_wasm::Value::Record(_)) = result);
    }

    #[test]
    async fn invalid_json_body_returns_error() {
        let mut request = raw_request(b"this is not json");

        let schema = RequestBodySchema::JsonBody {
            expected_type: analysed_type::u8(),
        };

        let err = parse_request_body(&mut request, &schema).await.unwrap_err();

        assert!(let RequestHandlerError::BodyIsNotValidJson { .. } = err);
    }

    #[test]
    async fn json_body_schema_mismatch_returns_error() {
        // JSON is valid, but shape does not match expected type
        let mut request = json_request(json!("not a record"));

        let schema = RequestBodySchema::JsonBody {
            expected_type: analysed_type::record(vec![NameTypePair {
                name: String::from("x"),
                typ: analysed_type::s32(),
            }]),
        };

        let err = parse_request_body(&mut request, &schema).await.unwrap_err();

        assert!(let RequestHandlerError::JsonBodyParsingFailed { .. } = err);
    }

    #[test]
    async fn restricted_binary_body_accepts_allowed_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/octet-stream");

        let schema = RequestBodySchema::RestrictedBinary {
            allowed_mime_types: vec!["application/octet-stream".to_string()],
        };

        let result = parse_request_body(&mut request, &schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(BinarySource { data, binary_type })) =
                result
        );

        assert!(data == b"binary-data");
        assert!(binary_type.mime_type == "application/octet-stream");
    }

    #[test]
    async fn restricted_binary_body_rejects_disallowed_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/json");

        let schema = RequestBodySchema::RestrictedBinary {
            allowed_mime_types: vec!["application/octet-stream".to_string()],
        };

        let err = parse_request_body(&mut request, &schema).await.unwrap_err();

        {
            let_assert!(
                RequestHandlerError::UnsupportedMimeType {
                    mime_type,
                    allowed_mime_types,
                } = err
            );

            assert!(mime_type == "application/json");
            assert!(allowed_mime_types == vec!["application/octet-stream"]);
        }
    }

    #[test]
    async fn restricted_binary_body_without_content_type_uses_default_and_is_checked() {
        let mut request = raw_request(b"binary-data");

        let schema = RequestBodySchema::RestrictedBinary {
            allowed_mime_types: vec!["application/octet-stream".to_string()],
        };

        let result = parse_request_body(&mut request, &schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(BinarySource { binary_type, .. })) = result
        );

        assert!(binary_type.mime_type == "application/octet-stream");
    }

    #[test]
    async fn unrestricted_binary_body_accepts_any_mime_type() {
        let mut request = raw_request_with_content_type(b"binary-data", "application/weird");

        let schema = RequestBodySchema::UnrestrictedBinary;

        let result = parse_request_body(&mut request, &schema).await.unwrap();

        let_assert!(
            ParsedRequestBody::UnstructuredBinary(Some(BinarySource { binary_type, .. })) = result
        );

        assert!(binary_type.mime_type == "application/weird");
    }
}
