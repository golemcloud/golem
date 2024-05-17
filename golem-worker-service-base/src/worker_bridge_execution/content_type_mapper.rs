use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use poem::web::headers::ContentType;
use poem::web::WithContentType;
use poem::Body;
use std::fmt::{Display, Formatter};

pub trait HttpContentTypeResponseMapper {
    fn to_response_body(
        &self,
        content_type_headers: ContentTypeHeaders,
    ) -> Result<WithContentType<Body>, ContentTypeMapError>;
}

pub enum ContentTypeHeaders {
    Accept(Vec<ContentType>),
    Response(ContentType),
    Empty,
}

impl ContentTypeHeaders {
    pub fn from(
        response_content_type: Option<ContentType>,
        accepted_content_types: Vec<ContentType>,
    ) -> Self {
        if let Some(response_content_type) = response_content_type {
            ContentTypeHeaders::Response(response_content_type)
        } else if !accepted_content_types.is_empty() {
            ContentTypeHeaders::Accept(accepted_content_types)
        } else {
            ContentTypeHeaders::Empty
        }
    }

    pub fn from_accept(accept_content_headers: Vec<ContentType>) -> Self {
        ContentTypeHeaders::Accept(accept_content_headers)
    }

    pub fn from_response(response_content_type: ContentType) -> Self {
        ContentTypeHeaders::Response(response_content_type)
    }

    pub fn empty() -> Self {
        ContentTypeHeaders::Empty
    }
}

impl HttpContentTypeResponseMapper for TypeAnnotatedValue {
    fn to_response_body(
        &self,
        content_type_headers: ContentTypeHeaders,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match content_type_headers {
            ContentTypeHeaders::Response(content_type) => {
                internal::get_body_from_response_content_type(self, &content_type)
            }
            ContentTypeHeaders::Accept(accept_content_headers) => {
                internal::get_response_body_from_accept_content_headers(
                    self,
                    &accept_content_headers,
                )
            }
            ContentTypeHeaders::Empty => internal::get_response_body(self),
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum ContentTypeMapError {
    UnsupportedWorkerFunctionResult {
        expected: Vec<AnalysedType>,
        actual: AnalysedType,
    },

    IllegalMapping {
        input_type: AnalysedType,
        expected_content_types: Vec<ContentType>,
    },
    InternalError(String),
}

impl ContentTypeMapError {
    fn internal<A: AsRef<str>>(msg: A) -> ContentTypeMapError {
        ContentTypeMapError::InternalError(msg.as_ref().to_string())
    }

    fn illegal_mapping(
        input_type: &AnalysedType,
        expected_content_types: &Vec<ContentType>,
    ) -> ContentTypeMapError {
        ContentTypeMapError::IllegalMapping {
            input_type: input_type.clone(),
            expected_content_types: expected_content_types.clone(),
        }
    }

    fn expect_only_binary_stream(actual: &AnalysedType) -> ContentTypeMapError {
        ContentTypeMapError::UnsupportedWorkerFunctionResult {
            expected: vec![
                AnalysedType::List(Box::new(AnalysedType::U8)),
                AnalysedType::Str,
            ],
            actual: actual.clone(),
        }
    }
}

impl Display for ContentTypeMapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentTypeMapError::UnsupportedWorkerFunctionResult {
                expected,
                actual: obtained,
            } => {
                write!(
                    f,
                    "Failed to map to required content type. Expected: {:?}, Obtained: {:?}",
                    expected, obtained
                )
            }
            ContentTypeMapError::InternalError(message) => {
                write!(f, "{}", message)
            }
            ContentTypeMapError::IllegalMapping {
                input_type,
                expected_content_types,
            } => {
                write!(
                    f,
                    "Failed to map input type {:?} to any of the expected content types: {:?}",
                    input_type, expected_content_types
                )
            }
        }
    }
}

mod internal {
    use crate::worker_bridge_execution::content_type_mapper::ContentTypeMapError;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use poem::web::headers::ContentType;
    use poem::web::WithContentType;
    use poem::{Body, IntoResponse};
    use std::fmt::Display;


    // Accept headers are provided, but the response type header is not set
    pub(crate) fn get_response_body_from_accept_content_headers(
        type_annotated_value: &TypeAnnotatedValue,
        accept_content_headers: &Vec<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::Record { .. } => {
                handle_record_with_accepted_headers(type_annotated_value, accept_content_headers)
            }
            TypeAnnotatedValue::List { .. } => {
                handle_list_with_accepted_headers(type_annotated_value, accept_content_headers)
            }
            TypeAnnotatedValue::Bool(bool) => {
                handle_primitive(bool, &AnalysedType::Bool, accept_content_headers)
            }
            TypeAnnotatedValue::S8(s8) => {
                handle_primitive(s8, &AnalysedType::S8, accept_content_headers)
            }
            TypeAnnotatedValue::U8(u8) => {
                handle_primitive(u8, &AnalysedType::U8, accept_content_headers)
            }
            TypeAnnotatedValue::S16(s16) => {
                handle_primitive(s16, &AnalysedType::S16, accept_content_headers)
            }
            TypeAnnotatedValue::U16(u16) => {
                handle_primitive(u16, &AnalysedType::U16, accept_content_headers)
            }
            TypeAnnotatedValue::S32(s32) => {
                handle_primitive(s32, &AnalysedType::S32, accept_content_headers)
            }
            TypeAnnotatedValue::U32(u32) => {
                handle_primitive(u32, &AnalysedType::U32, accept_content_headers)
            }
            TypeAnnotatedValue::S64(s64) => {
                handle_primitive(s64, &AnalysedType::S64, accept_content_headers)
            }
            TypeAnnotatedValue::U64(u64) => {
                handle_primitive(u64, &AnalysedType::U64, accept_content_headers)
            }
            TypeAnnotatedValue::F32(f32) => {
                handle_primitive(f32, &AnalysedType::F32, accept_content_headers)
            }
            TypeAnnotatedValue::F64(f64) => {
                handle_primitive(f64, &AnalysedType::F64, accept_content_headers)
            }
            TypeAnnotatedValue::Chr(char) => {
                handle_primitive(char, &AnalysedType::Chr, accept_content_headers)
            }
            TypeAnnotatedValue::Str(string) => handle_string(string, accept_content_headers),
            TypeAnnotatedValue::Tuple { .. } => {
                handle_complex(type_annotated_value, accept_content_headers)
            }
            TypeAnnotatedValue::Flags { .. } => {
                handle_complex(type_annotated_value, accept_content_headers)
            }
            TypeAnnotatedValue::Variant { .. } => {
                handle_record_with_accepted_headers(type_annotated_value, accept_content_headers)
            }
            TypeAnnotatedValue::Enum { value, .. } => handle_string(value, accept_content_headers),
            // Confirm this behaviour, given there is no specific content type
            TypeAnnotatedValue::Option { value, .. } => match value {
                Some(value) => {
                    get_response_body_from_accept_content_headers(value, accept_content_headers)
                }
                None => {
                    if accept_content_headers.contains(&ContentType::json()) {
                        get_json_null()
                    } else {
                        Err(ContentTypeMapError::illegal_mapping(
                            &AnalysedType::from(type_annotated_value),
                            accept_content_headers,
                        ))
                    }
                }
            },
            // Can be considered as a record
            TypeAnnotatedValue::Result { .. } => {
                handle_complex(type_annotated_value, accept_content_headers)
            }
            TypeAnnotatedValue::Handle { .. } => {
                handle_complex(type_annotated_value, accept_content_headers)
            }
        }
    }

    fn handle_complex(
        complex: &TypeAnnotatedValue,
        accepted_headers: &Vec<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        if accepted_headers.contains(&ContentType::json()) {
            get_json(complex)
        } else {
            Err(ContentTypeMapError::illegal_mapping(
                &AnalysedType::from(complex),
                accepted_headers,
            ))
        }
    }

    fn handle_string(
        string: &str,
        accepted_headers: &Vec<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        if accepted_headers.contains(&ContentType::json()) {
            Ok(Body::from_string(string.to_string())
                .with_content_type(ContentType::json().to_string()))
        } else {
            let non_json_content_type = pick_highest_priority_content_type(accepted_headers)?;
            Ok(Body::from_bytes(bytes::Bytes::from(string.to_string()))
                .with_content_type(non_json_content_type.to_string()))
        }
    }

    fn handle_record_with_accepted_headers(
        record: &TypeAnnotatedValue,
        accepted_headers: &Vec<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        // if record, we prioritise JSON
        if accepted_headers.contains(&ContentType::json()) {
            get_json(record)
        } else {
            // There is no way a Record can be properly serialised into any other formats to satisfy any other headers, therefore fail
            Err(ContentTypeMapError::illegal_mapping(
                &AnalysedType::from(record),
                accepted_headers,
            ))
        }
    }

    fn handle_list_with_accepted_headers(
        list: &TypeAnnotatedValue,
        accepted_headers: &Vec<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match list {
            TypeAnnotatedValue::List { values, typ } => {
                match typ {
                    // If the elements are u8, we consider it as a byte stream
                    AnalysedType::U8 => {
                        let byte_stream = get_byte_stream(values)?;
                        let body = Body::from_bytes(bytes::Bytes::from(byte_stream));
                        let content_type_header =
                            pick_highest_priority_content_type(accepted_headers)?;
                        Ok(body.with_content_type(content_type_header.to_string()))
                    }
                    _ => {
                        // If the accepted headers contain JSON, we prioritise that and set ther response type header to json
                        if accepted_headers.contains(&ContentType::json()) {
                            get_json(list)
                        } else {
                            // There is no way a List can be properly serialised into any other formats to satisfy any other headers, therefore fail
                            Err(ContentTypeMapError::illegal_mapping(
                                &AnalysedType::from(list),
                                accepted_headers,
                            ))
                        }
                    }
                }
            }
            _ => Err(ContentTypeMapError::illegal_mapping(
                &AnalysedType::from(list),
                accepted_headers,
            )),
        }
    }

    fn handle_primitive<A: Display + serde::Serialize>(
        input: &A,
        primitive_type: &AnalysedType,
        accepted_content_type: &Vec<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> where {
        if accepted_content_type.contains(&ContentType::text()) {
            Ok(get_text_body(input))
        } else if accepted_content_type.contains(&ContentType::json()) {
            let json = serde_json::to_value(input)
                .map_err(|_| ContentTypeMapError::internal("Failed to convert to json body"))?;

            let body = Body::from_json(json).map_err(|_| ContentTypeMapError::internal(format!("Failed to convert {} to json body", input)))?;

            Ok(body.with_content_type(ContentType::json().to_string()))

        } else {
            Err(ContentTypeMapError::illegal_mapping(
                primitive_type,
                accepted_content_type,
            ))
        }
    }

    fn pick_highest_priority_content_type(
        input_content_types: &[ContentType],
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

    // Neither Accept headers, nor the response type is set
    // In this case, we set the content header based on the type of TypeAnnotatedValue
    // If binary stream we set the content header to octet stream,
    // If text, we set it to content header to text/plain
    // For valid jsons (record, list), we set the content header to application/json
    pub(crate) fn get_response_body(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            // If record, we must jsonify it given the user hasn't provided any other content type
            TypeAnnotatedValue::Record { .. } => get_json(type_annotated_value),
            // Given no content type, we must jsonify it if it's not a byte stream
            TypeAnnotatedValue::List { values, typ } => {
                match typ {
                    // If the elements are u8, we consider it as a byte stream
                    AnalysedType::U8 => get_byte_stream_body(values),
                    _ => get_json(type_annotated_value),
                }
            }

            TypeAnnotatedValue::Bool(bool) => Ok(get_text_body(bool)),
            TypeAnnotatedValue::S8(s8) => Ok(get_text_body(s8.to_string())),
            TypeAnnotatedValue::U8(u8) => Ok(get_text_body(u8.to_string())),
            TypeAnnotatedValue::S16(s16) => Ok(get_text_body(s16.to_string())),
            TypeAnnotatedValue::U16(u16) => Ok(get_text_body(u16.to_string())),
            TypeAnnotatedValue::S32(s32) => Ok(get_text_body(s32.to_string())),
            TypeAnnotatedValue::U32(u32) => Ok(get_text_body(u32.to_string())),
            TypeAnnotatedValue::S64(s64) => Ok(get_text_body(s64.to_string())),
            TypeAnnotatedValue::U64(u64) => Ok(get_text_body(u64.to_string())),
            TypeAnnotatedValue::F32(f32) => Ok(get_text_body(f32.to_string())),
            TypeAnnotatedValue::F64(f64) => Ok(get_text_body(f64.to_string())),
            TypeAnnotatedValue::Chr(char) => Ok(get_text_body(char.to_string())),
            TypeAnnotatedValue::Str(string) => Ok(get_text_body(string.to_string())),
            TypeAnnotatedValue::Tuple { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Flags { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Variant { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Enum { value, .. } => Ok(get_text_body(value.to_string())),
            TypeAnnotatedValue::Option { value, .. } => match value {
                Some(value) => get_response_body(value),
                None => get_json_null(),
            },
            // Can be considered as a record
            TypeAnnotatedValue::Result { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Handle { .. } => get_json(type_annotated_value),
        }
    }

    pub(crate) fn get_body_from_response_content_type(
        type_annotated_value: &TypeAnnotatedValue,
        response_content_type: &ContentType,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        if response_content_type == &ContentType::json() {
            // If the request Content-Type is application/json,
            // then we can allow ANY component model type of value as the response.
            // However, only Rib return values which are not equivalent to a binary stream are automatically converted into JSON (jsonification).
            // We set the response content type to JSON
            get_json_or_binary_stream(type_annotated_value)
        } else {
            // If the request Content-Type is application/json,
            // but the Rib return value is equivalent to a binary stream,
            // then we assume the user has already rendered the data into JSON,
            // and we simply stream those bytes as the response, ensuring the content-type header is set appropriately.
            convert_only_if_binary_stream(type_annotated_value, response_content_type)
        }
    }

    fn convert_only_if_binary_stream(
        type_annotated_value: &TypeAnnotatedValue,
        non_json_content_type: &ContentType,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::List { typ, values } => match typ {
                AnalysedType::U8 => get_byte_stream(values).map(|bytes| {
                    Body::from_bytes(bytes::Bytes::from(bytes))
                        .with_content_type(non_json_content_type.to_string())
                }),
                // Fail if it is not a binary stream
                other => Err(ContentTypeMapError::expect_only_binary_stream(other)),
            },
            // Str is equivalent to binary stream, therefore proceeds with responding as a binary stream
            TypeAnnotatedValue::Str(str) => {
                Ok(Body::from_bytes(bytes::Bytes::from(str.to_string()))
                    .with_content_type(non_json_content_type.to_string()))
            }
            other => Err(ContentTypeMapError::expect_only_binary_stream(
                &AnalysedType::from(other),
            )),
        }
    }

    fn get_byte_stream(values: &Vec<TypeAnnotatedValue>) -> Result<Vec<u8>, ContentTypeMapError> {
        let bytes = values
            .into_iter()
            .map(|v| match v {
                TypeAnnotatedValue::U8(u8) => Ok(*u8),
                _ => Err(ContentTypeMapError::internal(
                    "The analysed type is a binary stream however unable to fetch vec<u8>",
                )),
            })
            .collect::<Result<Vec<_>, ContentTypeMapError>>()?;

        Ok(bytes)
    }

    fn get_byte_stream_body(
        values: &Vec<TypeAnnotatedValue>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let bytes = get_byte_stream(values)?;
        Ok(Body::from_bytes(bytes::Bytes::from(bytes))
            .with_content_type(ContentType::octet_stream().to_string()))
    }

    fn get_json(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let json = get_json_from_typed_value(type_annotated_value);
        Body::from_json(json)
            .map(|body| body.with_content_type(ContentType::json().to_string()))
            .map_err(|_| ContentTypeMapError::internal("Failed to convert to json body"))
    }

    // If the request Content-Type is application/json, then we can allow ANY component model type of value as the response.
    // However, only Rib return values which are not equivalent to a binary stream are automatically converted into JSON (jsonification).
    fn get_json_or_binary_stream(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::List { typ, values } => match typ {
                AnalysedType::U8 => get_byte_stream(values).map(|bytes| {
                    Body::from_bytes(bytes::Bytes::from(bytes))
                        .with_content_type(ContentType::json().to_string())
                }),
                _ => get_json(type_annotated_value),
            },
            TypeAnnotatedValue::Str(str) => Ok(Body::from_string(str.to_string())
                .with_content_type(ContentType::json().to_string())),
            _ => get_json(type_annotated_value),
        }
    }

    fn get_json_null() -> Result<WithContentType<Body>, ContentTypeMapError> {
        Body::from_json(serde_json::Value::Null)
            .map(|body| body.with_content_type(ContentType::json().to_string()))
            .map_err(|_| ContentTypeMapError::internal("Failed to convert to json body"))
    }

    fn get_text_body(value: impl Display) -> WithContentType<Body> {
        Body::from_string(value.to_string()).with_content_type(ContentType::text().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use poem::web::headers::ContentType;
    use poem::IntoResponse;

    // #[tokio::test]
    // async fn test_get_response_body() {
    //     let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
    //     let response_body = internal::get_response_body(&type_annotated_value).unwrap();
    //     let response = response_body.into_response();
    //     let bytes = response.into_body().into_bytes().await.unwrap();
    //     let result = String::from_utf8_lossy(&bytes).to_string();
    //     let content_type = response.content_type();
    //
    //     // Had it serialised as json, it would have been "\"Hello\""
    //     assert_eq!((result, content_type), ("Hello".to_string(), Some("text/plain")));
    //
    //     let type_annotated_value = TypeAnnotatedValue::U8(10);
    //     let response_body = internal::get_response_body(&type_annotated_value).unwrap();
    //     assert_eq!(response_body.into_response().content_type(),  Some("text/plain"));
    //
    //     let type_annotated_value = TypeAnnotatedValue::List {
    //         values: vec![TypeAnnotatedValue::U8(10)],
    //         typ: AnalysedType::U8,
    //     };
    //     let response_body = internal::get_response_body(&type_annotated_value).unwrap();
    //     assert_eq!(response_body.into_response().content_type(), Some("application/octet-stream"));
    //
    //     let type_annotated_value = TypeAnnotatedValue::List {
    //         values: vec![TypeAnnotatedValue::U8(10)],
    //         typ: AnalysedType::U16,
    //     };
    //     let response_body = internal::get_response_body(&type_annotated_value).unwrap();
    //     assert_eq!(response_body.into_response().content_type(), Some("application/json"));
    //
    //
    //     let type_annotated_value = TypeAnnotatedValue::Record {
    //         typ: vec![("name".to_string(), AnalysedType::Str)],
    //         value: vec![("name".to_string(), TypeAnnotatedValue::Str("Hello".to_string()))],
    //     };
    //     let response_body = internal::get_response_body(&type_annotated_value).unwrap();
    //     assert_eq!(response_body.into_response().content_type(), Some("application/json"));
    // }

    //#[tokio::test]
    //async fn test_get_response_body_from_content_type() {
    // let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
    // let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
    // let response = response_body.into_response();
    // let body = response.into_body().into_bytes().await.unwrap();
    // let result = String::from_utf8_lossy(&body).to_string();
    // // Had it serialised as Json, the result would have been: "\"Hello\""
    // assert_eq!(result, "Hello".to_string());
    //
    // let type_annotated_value = TypeAnnotatedValue::U8(10);
    // let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
    // let response = response_body.into_response();
    // let body = response.into_body().into_bytes().await.unwrap();
    // let result = String::from_utf8_lossy(&body).to_string();
    //
    // assert_eq!(result, "10".to_string())

    //
    // let type_annotated_value = TypeAnnotatedValue::List {
    //     values: vec![TypeAnnotatedValue::U8(10)],
    //     typ: AnalysedType::U8,
    // };
    // let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
    // assert_eq!(response_body.content_type(), ContentType::octet_stream().to_string());
    //
    // let type_annotated_value = TypeAnnotatedValue::List {
    //     values: vec![TypeAnnotatedValue::U8(10)],
    //     typ: AnalysedType::U16,
    // };
    // let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
    // assert_eq!(response_body.content_type(), ContentType::json().to_string());
    //
    // let type_annotated_value = TypeAnnotatedValue::Record {
    //     typ: vec![("name".to_string(), AnalysedType::Str)],
    //     value: vec![("name".to_string(), TypeAnnotatedValue::Str("Hello".to_string()))],
    // };
    // let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
    // assert_eq!(response_body.content_type(), ContentType::json().to_string());
    //}
}
