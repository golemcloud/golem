use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use poem::web::headers::ContentType;
use poem::web::WithContentType;
use poem::Body;
use std::fmt::{Display, Formatter};

pub trait HttpContentTypeResponseMapper {
    fn to_response_body(
        &self,
        content_type_opt: &Option<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError>;
}

impl HttpContentTypeResponseMapper for TypeAnnotatedValue {
    fn to_response_body(
        &self,
        response_content_type: &Option<ContentType>,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match response_content_type {
            Some(content_type) => internal::get_response_body_from_content_type(self, content_type),
            None => internal::get_response_body(self),
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum ContentTypeMapError {
    UnsupportedWorkerFunctionResult {
        expected: Vec<AnalysedType>,
        actual: AnalysedType,
    },

    InternalError(String),
}

impl ContentTypeMapError {
    fn internal(msg: &str) -> ContentTypeMapError {
        ContentTypeMapError::InternalError(msg.to_string())
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

    // Convert a type annotated value to Body, given no specific content type
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
            // Variant is considered as a record of 1 element with type name and details
            // Confirm this behaviour
            TypeAnnotatedValue::Variant { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Enum { value, .. } => Ok(get_text_body(value.to_string())),
            // Confirm this behaviour, given there is no specific content type
            TypeAnnotatedValue::Option { value, .. } => match value {
                Some(value) => get_response_body(value),
                None => get_json_null(),
            },
            // Can be considered as a record
            TypeAnnotatedValue::Result { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Handle { .. } => get_json(type_annotated_value),
        }
    }

    pub(crate) fn get_response_body_from_content_type(
        type_annotated_value: &TypeAnnotatedValue,
        content_type: &ContentType,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        if content_type == &ContentType::json() {
            get_json_or_binary_stream(type_annotated_value)
        } else {
            convert_only_if_binary_stream(type_annotated_value, content_type)
        }
    }

    fn convert_only_if_binary_stream(
        type_annotated_value: &TypeAnnotatedValue,
        non_json_content_type: &ContentType,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::List { typ, values } => match typ {
                // It is already a binary stream and we keep it as is
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
                // It is already a binary stream and we keep it as is
                AnalysedType::U8 => get_byte_stream_body(values),
                // Convert to Json otherwise
                _ => get_json(type_annotated_value),
            },
            // A string is considered as a binary stream
            TypeAnnotatedValue::Str(str) => Ok(get_text_body(str)),
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
//
// #[cfg(test)]
// mod tests {
//     use super::*;
//     use golem_wasm_rpc::json::get_json_from_typed_value;
//     use golem_wasm_rpc::TypeAnnotatedValue;
//     use poem::IntoResponse;
//     use poem::web::headers::ContentType;
//
//     #[test]
//     fn test_get_response_body() {
//         let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
//         let response_body = internal::get_response_body(&type_annotated_value).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::text().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::U8(10);
//         let response_body = internal::get_response_body(&type_annotated_value).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::text().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::List {
//             values: vec![TypeAnnotatedValue::U8(10)],
//             typ: AnalysedType::U8,
//         };
//         let response_body = internal::get_response_body(&type_annotated_value).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::octet_stream().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::List {
//             values: vec![TypeAnnotatedValue::U8(10)],
//             typ: AnalysedType::U16,
//         };
//         let response_body = internal::get_response_body(&type_annotated_value).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::text().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::Record {
//             typ: vec![("name".to_string(), AnalysedType::Str)],
//             value: vec![("name".to_string(), TypeAnnotatedValue::Str("Hello".to_string()))],
//         };
//         let response_body = internal::get_response_body(&type_annotated_value).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::json().to_string());
//     }
//
//     #[test]
//     fn test_get_response_body_from_content_type() {
//         let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
//         let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::json().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::U8(10);
//         let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
//         assert_eq!(response_body.into_response().content_type(), Some(ContentType::json().to_string()));
//
//         let type_annotated_value = TypeAnnotatedValue::List {
//             values: vec![TypeAnnotatedValue::U8(10)],
//             typ: AnalysedType::U8,
//         };
//         let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::octet_stream().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::List {
//             values: vec![TypeAnnotatedValue::U8(10)],
//             typ: AnalysedType::U16,
//         };
//         let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::json().to_string());
//
//         let type_annotated_value = TypeAnnotatedValue::Record {
//             typ: vec![("name".to_string(), AnalysedType::Str)],
//             value: vec![("name".to_string(), TypeAnnotatedValue::Str("Hello".to_string()))],
//         };
//         let response_body = internal::get_response_body_from_content_type(&type_annotated_value, &ContentType::json()).unwrap();
//         assert_eq!(response_body.content_type(), ContentType::json().to_string());
//     }
// }
