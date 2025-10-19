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

use golem_wasm::analysis::AnalysedType;
use golem_wasm::ValueAndType;
use mime::Mime;
use poem::web::headers::ContentType;
use poem::web::WithContentType;
use poem::Body;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub trait HttpContentTypeResponseMapper {
    fn to_http_resp_with_content_type(
        &self,
        content_type_headers: ContentTypeHeaders,
    ) -> Result<WithContentType<Body>, ContentTypeMapError>;
}

#[derive(Debug, Clone)]
pub enum ContentTypeHeaders {
    FromClientAccept(AcceptHeaders),
    FromUserDefinedResponseMapping(ContentType),
    Empty,
}

#[derive(Clone, Debug)]
pub struct AcceptHeaders(Vec<String>);

pub trait ContentTypeHeaderExt {
    fn has_application_json(&self) -> bool;
    fn response_content_type(&self) -> Result<ContentType, ContentTypeMapError>;
}

impl ContentTypeHeaderExt for ContentType {
    fn has_application_json(&self) -> bool {
        self == &ContentType::json()
    }
    fn response_content_type(&self) -> Result<ContentType, ContentTypeMapError> {
        Ok(self.clone())
    }
}
impl ContentTypeHeaderExt for AcceptHeaders {
    fn has_application_json(&self) -> bool {
        self.0.iter().any(|v| {
            if let Ok(mime) = Mime::from_str(v) {
                matches!(
                    (mime.type_(), mime.subtype()),
                    (mime::APPLICATION, mime::JSON)
                        | (mime::APPLICATION, mime::STAR)
                        | (mime::STAR, mime::STAR)
                        | (mime::STAR, mime::JSON)
                )
            } else {
                false
            }
        })
    }

    fn response_content_type(&self) -> Result<ContentType, ContentTypeMapError> {
        internal::pick_highest_priority_content_type(self)
    }
}

impl AcceptHeaders {
    fn from_str<A: AsRef<str>>(input: A) -> AcceptHeaders {
        let headers = input
            .as_ref()
            .split(',')
            .map(|v| v.trim().to_string())
            .collect::<Vec<String>>();

        AcceptHeaders(headers)
    }
}

impl Display for AcceptHeaders {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0.join(", "))
    }
}

impl AcceptHeaders {
    fn contains(&self, content_type: &ContentType) -> bool {
        self.0
            .iter()
            .any(|accept_header| accept_header.contains(content_type.to_string().as_str()))
    }
}

impl ContentTypeHeaders {
    pub fn from(
        response_content_type: Option<ContentType>,
        accepted_content_types: Option<String>,
    ) -> Self {
        if let Some(response_content_type) = response_content_type {
            ContentTypeHeaders::FromUserDefinedResponseMapping(response_content_type)
        } else if let Some(accept_header_string) = accepted_content_types {
            ContentTypeHeaders::FromClientAccept(AcceptHeaders::from_str(accept_header_string))
        } else {
            ContentTypeHeaders::Empty
        }
    }
}

impl HttpContentTypeResponseMapper for ValueAndType {
    fn to_http_resp_with_content_type(
        &self,
        content_type_headers: ContentTypeHeaders,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match content_type_headers {
            ContentTypeHeaders::FromUserDefinedResponseMapping(content_type) => {
                internal::get_response_body_based_on_content_type(self, &content_type)
            }
            ContentTypeHeaders::FromClientAccept(accept_content_headers) => {
                internal::get_response_body_based_on_content_type(self, &accept_content_headers)
            }
            ContentTypeHeaders::Empty => internal::get_response_body(self),
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum ContentTypeMapError {
    IllegalMapping {
        input_type: AnalysedType,
        expected_content_types: String,
    },
    InternalError(String),
}

impl ContentTypeMapError {
    fn internal<A: AsRef<str>>(msg: A) -> ContentTypeMapError {
        ContentTypeMapError::InternalError(msg.as_ref().to_string())
    }

    fn illegal_mapping<A: Display>(
        input_type: &AnalysedType,
        expected_content_types: &A,
    ) -> ContentTypeMapError {
        ContentTypeMapError::IllegalMapping {
            input_type: input_type.clone(),
            expected_content_types: expected_content_types.to_string(),
        }
    }
}

impl Display for ContentTypeMapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentTypeMapError::InternalError(message) => {
                write!(f, "{message}")
            }
            ContentTypeMapError::IllegalMapping {
                input_type,
                expected_content_types,
            } => {
                write!(
                    f,
                    "Failed to map input type {input_type:?} to any of the expected content types: {expected_content_types:?}"
                )
            }
        }
    }
}

mod internal {
    use crate::gateway_execution::http_content_type_mapper::{
        AcceptHeaders, ContentTypeHeaderExt, ContentTypeMapError,
    };
    use golem_wasm::analysis::{AnalysedType, TypeEnum, TypeList, TypeOption, TypeRecord};
    use golem_wasm::json::ValueAndTypeJsonExtensions;
    use golem_wasm::{Value, ValueAndType};
    use poem::web::headers::ContentType;
    use poem::web::WithContentType;
    use poem::{Body, IntoResponse};
    use std::fmt::Display;

    pub(crate) fn get_response_body_based_on_content_type<A: ContentTypeHeaderExt + Display>(
        value_and_type: &ValueAndType,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match (&value_and_type.typ, &value_and_type.value) {
            (AnalysedType::Record(_record), Value::Record(_values)) => {
                handle_record(value_and_type, content_header)
            }
            (AnalysedType::Variant(_variant), Value::Variant { .. }) => {
                handle_record(value_and_type, content_header)
            }
            (AnalysedType::List(TypeList { inner, .. }), Value::List(values)) => {
                handle_list(value_and_type, values, inner, content_header)
            }
            (AnalysedType::Bool(_), Value::Bool(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::S8(_), Value::S8(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::U8(_), Value::U8(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::S16(_), Value::S16(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::U16(_), Value::U16(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::S32(_), Value::S32(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::U32(_), Value::U32(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::S64(_), Value::S64(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::U64(_), Value::U64(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::F32(_), Value::F32(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::F64(_), Value::F64(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::Chr(_), Value::Char(value)) => {
                handle_primitive(value, &value_and_type.typ, content_header)
            }
            (AnalysedType::Str(_), Value::String(string)) => handle_string(string, content_header),
            (AnalysedType::Tuple(_), Value::Tuple(_)) => {
                handle_complex(value_and_type, content_header)
            }
            (AnalysedType::Flags(_), Value::Flags(_)) => {
                handle_complex(value_and_type, content_header)
            }
            // Can be considered as a record
            (AnalysedType::Result(_), Value::Result(_)) => {
                handle_complex(value_and_type, content_header)
            }
            (AnalysedType::Handle(_), Value::Handle { .. }) => {
                handle_complex(value_and_type, content_header)
            }
            (AnalysedType::Enum(TypeEnum { cases, .. }), Value::Enum(name_idx)) => {
                let name = cases
                    .get(*name_idx as usize)
                    .ok_or(ContentTypeMapError::internal("Invalid enum index"))?;
                handle_string(name, content_header)
            }
            (AnalysedType::Option(TypeOption { inner, .. }), Value::Option(value)) => match value {
                Some(value) => {
                    let value_and_type = ValueAndType::new((**value).clone(), (**inner).clone());
                    get_response_body_based_on_content_type(&value_and_type, content_header)
                }
                None => {
                    if content_header.has_application_json() {
                        get_json_null()
                    } else {
                        let typ = value_and_type.typ.clone();
                        Err(ContentTypeMapError::illegal_mapping(&typ, content_header))
                    }
                }
            },
            _ => Err(ContentTypeMapError::InternalError(
                "Value and type mismatch".to_string(),
            )),
        }
    }

    pub(crate) fn get_response_body(
        value_and_type: &ValueAndType,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match (&value_and_type.typ, &value_and_type.value) {
            (AnalysedType::Record(TypeRecord { .. }), Value::Record { .. }) => {
                get_json(value_and_type)
            }
            (AnalysedType::List(TypeList { inner, .. }), Value::List(values)) => match &**inner {
                AnalysedType::U8(_) => get_byte_stream_body(values),
                _ => get_json(value_and_type),
            },
            (AnalysedType::Str(_), Value::String(string)) => {
                Ok(Body::from_string(string.to_string())
                    .with_content_type(ContentType::json().to_string()))
            }
            (AnalysedType::Enum(TypeEnum { cases, .. }), Value::Enum(case_idx)) => {
                let case_name = cases
                    .get(*case_idx as usize)
                    .ok_or(ContentTypeMapError::internal("Invalid enum index"))?;
                Ok(Body::from_string(case_name.to_string())
                    .with_content_type(ContentType::json().to_string()))
            }
            (AnalysedType::Bool(_), Value::Bool(bool)) => get_json_of(bool),
            (AnalysedType::S8(_), Value::S8(s8)) => get_json_of(s8),
            (AnalysedType::U8(_), Value::U8(u8)) => get_json_of(u8),
            (AnalysedType::S16(_), Value::S16(s16)) => get_json_of(s16),
            (AnalysedType::U16(_), Value::U16(u16)) => get_json_of(u16),
            (AnalysedType::S32(_), Value::S32(s32)) => get_json_of(s32),
            (AnalysedType::U32(_), Value::U32(u32)) => get_json_of(u32),
            (AnalysedType::S64(_), Value::S64(s64)) => get_json_of(s64),
            (AnalysedType::U64(_), Value::U64(u64)) => get_json_of(u64),
            (AnalysedType::F32(_), Value::F32(f32)) => get_json_of(f32),
            (AnalysedType::F64(_), Value::F64(f64)) => get_json_of(f64),
            (AnalysedType::Chr(_), Value::Char(char)) => get_json_of(char),
            (AnalysedType::Tuple(_), Value::Tuple(_)) => get_json(value_and_type),
            (AnalysedType::Flags(_), Value::Flags(_)) => get_json(value_and_type),
            (AnalysedType::Variant(_), Value::Variant { .. }) => get_json(value_and_type),
            (AnalysedType::Result(_), Value::Result { .. }) => get_json(value_and_type),
            (AnalysedType::Handle(_), Value::Handle { .. }) => get_json(value_and_type),
            (AnalysedType::Option(TypeOption { inner, .. }), Value::Option(value)) => match value {
                Some(value) => {
                    let value = ValueAndType::new((**value).clone(), (**inner).clone());
                    get_response_body(&value)
                }
                None => get_json_null(),
            },
            _ => Err(ContentTypeMapError::internal("Value and type mismatch")),
        }
    }

    pub(crate) fn pick_highest_priority_content_type(
        input_content_types: &AcceptHeaders,
    ) -> Result<ContentType, ContentTypeMapError> {
        let content_headers_in_priority: Vec<ContentType> = vec![
            ContentType::json(),
            ContentType::text(),
            ContentType::text_utf8(),
            ContentType::html(),
            ContentType::xml(),
            ContentType::form_url_encoded(),
            ContentType::jpeg(),
            ContentType::png(),
            ContentType::octet_stream(),
        ];

        let mut prioritised_content_type = None;
        for content_type in &content_headers_in_priority {
            if input_content_types.contains(content_type) {
                prioritised_content_type = Some(content_type.clone());
                break;
            }
        }

        if let Some(prioritised) = prioritised_content_type {
            Ok(prioritised)
        } else {
            Err(ContentTypeMapError::internal(
                "Failed to pick a content type to set in response headers",
            ))
        }
    }

    fn get_byte_stream(values: &[Value]) -> Result<Vec<u8>, ContentTypeMapError> {
        let bytes = values
            .iter()
            .map(|v| match v {
                Value::U8(u8) => Ok(*u8),
                _ => Err(ContentTypeMapError::internal(
                    "The analysed type is a binary stream however unable to fetch vec<u8>",
                )),
            })
            .collect::<Result<Vec<_>, ContentTypeMapError>>()?;

        Ok(bytes)
    }

    fn get_byte_stream_body(
        values: &[Value],
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let bytes = get_byte_stream(values)?;
        Ok(Body::from_bytes(bytes::Bytes::from(bytes))
            .with_content_type(ContentType::octet_stream().to_string()))
    }

    fn get_json(
        value_and_type: &ValueAndType,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let json = value_and_type.to_json_value().map_err(|err| {
            ContentTypeMapError::internal(format!("Failed to encode value as JSON: {err}"))
        })?;
        Body::from_json(json)
            .map(|body| body.with_content_type(ContentType::json().to_string()))
            .map_err(|_| ContentTypeMapError::internal("Failed to convert to json body"))
    }

    fn get_json_of<A: serde::Serialize + Display>(
        a: A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let json = serde_json::to_value(&a).map_err(|_| {
            ContentTypeMapError::internal(format!("Failed to serialise {a} to json"))
        })?;
        let body = Body::from_json(json)
            .map_err(|_| ContentTypeMapError::internal("Failed to create body from JSON"))?;

        Ok(body.with_content_type(ContentType::json().to_string()))
    }

    fn get_json_null() -> Result<WithContentType<Body>, ContentTypeMapError> {
        Body::from_json(serde_json::Value::Null)
            .map(|body| body.with_content_type(ContentType::json().to_string()))
            .map_err(|_| ContentTypeMapError::internal("Failed to convert to json body"))
    }

    fn handle_complex<A: ContentTypeHeaderExt + Display>(
        complex: &ValueAndType,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        if content_header.has_application_json() {
            get_json(complex)
        } else {
            let typ = complex.typ.clone();
            Err(ContentTypeMapError::illegal_mapping(&typ, content_header))
        }
    }

    fn handle_list<A: ContentTypeHeaderExt + Display>(
        original: &ValueAndType,
        inner_values: &[Value],
        elem_type: &AnalysedType,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match elem_type {
            AnalysedType::U8(_) => {
                let byte_stream = get_byte_stream(inner_values)?;
                let body = Body::from_bytes(bytes::Bytes::from(byte_stream));
                let content_type_header = content_header.response_content_type()?;
                Ok(body.with_content_type(content_type_header.to_string()))
            }
            _ => {
                if content_header.has_application_json() {
                    get_json(original)
                } else {
                    Err(ContentTypeMapError::illegal_mapping(
                        elem_type,
                        content_header,
                    ))
                }
            }
        }
    }

    fn handle_primitive<A: Display + serde::Serialize, B: ContentTypeHeaderExt + Display>(
        input: &A,
        primitive_type: &AnalysedType,
        content_header: &B,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> where {
        if content_header.has_application_json() {
            let json = serde_json::to_value(input)
                .map_err(|_| ContentTypeMapError::internal("Failed to convert to json body"))?;

            let body = Body::from_json(json).map_err(|_| {
                ContentTypeMapError::internal(format!("Failed to convert {input} to json body"))
            })?;

            Ok(body.with_content_type(ContentType::json().to_string()))
        } else {
            Err(ContentTypeMapError::illegal_mapping(
                primitive_type,
                content_header,
            ))
        }
    }

    fn handle_record<A: ContentTypeHeaderExt + Display>(
        value_and_type: &ValueAndType,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        // if record, we prioritise JSON
        if content_header.has_application_json() {
            get_json(value_and_type)
        } else {
            let typ = value_and_type.typ.clone();
            // There is no way a Record can be properly serialised into any other formats to satisfy any other headers, therefore fail
            Err(ContentTypeMapError::illegal_mapping(&typ, content_header))
        }
    }

    fn handle_string<A: ContentTypeHeaderExt + Display>(
        string: &str,
        content_type: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let response_content_type = content_type.response_content_type()?;

        let body = if content_type.has_application_json() {
            bytes::Bytes::from(format!("\"{string}\""))
        } else {
            bytes::Bytes::from(string.to_string())
        };

        Ok(Body::from_bytes(body).with_content_type(response_content_type.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm::analysis::analysed_type::{field, list, record, str};
    use golem_wasm::{IntoValue, Value};
    use poem::web::headers::ContentType;
    use poem::IntoResponse;

    fn sample_record() -> ValueAndType {
        ValueAndType::new(
            Value::Record(vec!["Hello".into_value()]),
            record(vec![field("name", str())]),
        )
    }

    fn create_list(vec: Vec<Value>, analysed_type: AnalysedType) -> ValueAndType {
        ValueAndType::new(Value::List(vec), list(analysed_type))
    }

    fn create_record(values: Vec<(&str, ValueAndType)>) -> ValueAndType {
        ValueAndType::new(
            Value::Record(values.iter().map(|(_, v)| v.value.clone()).collect()),
            record(
                values
                    .iter()
                    .map(|(name, value)| field(name, value.typ.clone()))
                    .collect(),
            ),
        )
    }

    #[cfg(test)]
    mod no_content_type_header {
        use test_r::test;

        use super::*;
        use golem_wasm::analysis::analysed_type::{u16, u8};
        use golem_wasm::IntoValueAndType;

        fn get_content_type_and_body(input: &ValueAndType) -> (Option<String>, Body) {
            let response_body = internal::get_response_body(input).unwrap();
            let response = response_body.into_response();
            let (parts, body) = response.into_parts();
            let content_type = parts
                .headers
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_string());
            (content_type, body)
        }

        #[test]
        async fn test_string_type() {
            let type_annotated_value = "Hello".into_value_and_type();
            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            // Had it serialized as json, it would have been "\"Hello\""
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_singleton_u8_type() {
            let type_annotated_value = 10u8.into_value_and_type();
            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("10".to_string(), Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_list_u8_type() {
            let type_annotated_value = create_list(vec![10u8.into_value()], u8());
            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let result = body.into_bytes().await.unwrap();

            assert_eq!(
                (result, content_type),
                (
                    bytes::Bytes::from(vec![10]),
                    Some("application/octet-stream".to_string())
                )
            );
        }

        #[test]
        async fn test_list_non_u8_type() {
            let type_annotated_value = create_list(vec![10u16.into_value()], u16());

            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: serde_json::Value =
                serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(10),
            )]);
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_record_type() {
            let type_annotated_value = create_record(vec![("name", "Hello".into_value_and_type())]);

            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: serde_json::Value =
                serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::json!({"name": "Hello"});
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }
    }

    #[cfg(test)]
    mod with_response_type_header {
        use test_r::test;

        use super::*;
        use golem_wasm::analysis::analysed_type::{u16, u8};
        use golem_wasm::IntoValueAndType;

        fn get_content_type_and_body(
            input: &ValueAndType,
            header: &ContentType,
        ) -> (Option<String>, Body) {
            let response_body =
                internal::get_response_body_based_on_content_type(input, header).unwrap();
            let response = response_body.into_response();
            let (parts, body) = response.into_parts();
            let content_type = parts
                .headers
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_string());
            (content_type, body)
        }

        #[test]
        async fn test_string_type_as_text() {
            let type_annotated_value = "Hello".into_value_and_type();
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::text());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            // Had it serialized as json, it would have been "\"Hello\""
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("text/plain".to_string()))
            );
        }

        #[test]
        async fn test_string_type_as_json() {
            let type_annotated_value = "\"Hello\"".into_value_and_type();
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();

            // It doesn't matter if the string is already jsonified, it will be jsonified again
            assert_eq!(
                (result, content_type),
                (
                    "\"\"Hello\"\"".to_string(),
                    Some("application/json".to_string())
                )
            );
        }

        #[test]
        async fn test_singleton_u8_type() {
            let type_annotated_value = 10u8.into_value_and_type();
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("10".to_string(), Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_list_u8_type() {
            let type_annotated_value = create_list(vec![10u8.into_value()], u8());

            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = &body.into_bytes().await.unwrap();
            let data_as_str = String::from_utf8_lossy(result).to_string();
            let result_json: Result<serde_json::Value, _> =
                serde_json::from_str(data_as_str.as_str());

            assert_eq!(
                (result, content_type),
                (
                    &bytes::Bytes::from(vec![10]),
                    Some("application/json".to_string())
                )
            );
            assert!(result_json.is_err()); // That we haven't jsonified this case explicitly
        }

        #[test]
        async fn test_list_non_u8_type() {
            let type_annotated_value = create_list(vec![10u16.into_value()], u16());

            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: serde_json::Value =
                serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(10),
            )]);
            // That we jsonify any list other than u8, and can be retrieveed as a valid JSON
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_record_type() {
            // Record
            let type_annotated_value = sample_record();

            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: serde_json::Value =
                serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::json!({"name": "Hello"});
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }
    }

    #[cfg(test)]
    mod with_accept_headers {
        use test_r::test;

        use super::*;
        use golem_wasm::analysis::analysed_type::{u16, u8};
        use golem_wasm::IntoValueAndType;

        fn get_content_type_and_body(
            input: &ValueAndType,
            headers: &AcceptHeaders,
        ) -> (Option<String>, Body) {
            let response_body =
                internal::get_response_body_based_on_content_type(input, headers).unwrap();
            let response = response_body.into_response();
            let (parts, body) = response.into_parts();
            let content_type = parts
                .headers
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_string());
            (content_type, body)
        }

        #[test]
        async fn test_string_type_with_json() {
            let type_annotated_value = "Hello".into_value_and_type();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                (
                    "\"Hello\"".to_string(),
                    Some("application/json".to_string())
                )
            );
        }

        #[test]
        async fn test_string_type_without_json() {
            let type_annotated_value = "Hello".into_value_and_type();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                (
                    "\"Hello\"".to_string(),
                    Some("application/json".to_string())
                )
            );
        }

        #[test]
        async fn test_string_type_with_html() {
            let type_annotated_value = "Hello".into_value_and_type();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("text/html".to_string()))
            );
        }

        #[test]
        async fn test_singleton_u8_type_text() {
            let type_annotated_value = 10u8.into_value_and_type();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("application/json"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("10".to_string(), Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_singleton_u8_type_json() {
            let type_annotated_value = 10u8.into_value_and_type();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("application/json"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("10".to_string(), Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_singleton_u8_failed_content_mapping() {
            let type_annotated_value = 10u8.into_value_and_type();
            let result = internal::get_response_body_based_on_content_type(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html"),
            );

            assert!(matches!(
                result,
                Err(ContentTypeMapError::IllegalMapping { .. })
            ));
        }

        #[test]
        async fn test_list_u8_type_with_json() {
            let type_annotated_value = create_list(vec![10u8.into_value()], u8());

            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.50"),
            );
            let result = &body.into_bytes().await.unwrap();
            let data_as_str = String::from_utf8_lossy(result).to_string();
            let result_json: Result<serde_json::Value, _> =
                serde_json::from_str(data_as_str.as_str());

            assert_eq!(
                (result, content_type),
                (
                    &bytes::Bytes::from(vec![10]),
                    Some("application/json".to_string())
                )
            );
            assert!(result_json.is_err()); // That we haven't jsonified this case explicitly
        }

        #[test]
        async fn test_list_non_u8_type_with_json() {
            let type_annotated_value = create_list(vec![10u16.into_value()], u16());

            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: serde_json::Value =
                serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(10),
            )]);
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_list_non_u8_type_with_html_fail() {
            let type_annotated_value = create_list(vec![10u16.into_value()], u16());

            let result = internal::get_response_body_based_on_content_type(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html"),
            );

            assert!(matches!(
                result,
                Err(ContentTypeMapError::IllegalMapping { .. })
            ));
        }

        #[test]
        async fn test_record_type_json() {
            let type_annotated_value = sample_record();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: serde_json::Value =
                serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::json!({"name": "Hello"});
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[test]
        async fn test_record_type_html() {
            let type_annotated_value = sample_record();

            let result = internal::get_response_body_based_on_content_type(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html"),
            );

            assert!(matches!(
                result,
                Err(ContentTypeMapError::IllegalMapping { .. })
            ));
        }
    }
}
