

use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use poem::web::headers::ContentType;
use poem::{Body};
use std::fmt::{Display, Formatter};

pub trait GetHttpResponseBody {
    fn to_response_body(
        &self,
        content_type_opt: &Option<ContentType>,
    ) -> Result<Body, ContentTypeMapError>;
}

impl GetHttpResponseBody for TypeAnnotatedValue {
    fn to_response_body(
        &self,
        content_type: &Option<ContentType>,
    ) -> Result<Body, ContentTypeMapError> {
        match content_type {
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
            expected: vec![AnalysedType::List(Box::new(AnalysedType::U8)), AnalysedType::Str],
            actual: actual.clone()
        }
    }
}

impl Display for ContentTypeMapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentTypeMapError::UnsupportedWorkerFunctionResult { expected, actual: obtained } => {
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
    use crate::primitive::GetPrimitive;
    use crate::worker_bridge_execution::content_type_mapper::ContentTypeMapError;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use poem::web::headers::ContentType;
    use poem::{Body, IntoResponse};

    // Convert a type annotated value to Body, given no specific content type
    pub(crate) fn get_response_body(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> Result<Body, ContentTypeMapError> {
        match type_annotated_value {
            // If record, we must jsonify it given the user hasn't provided any other content type
            TypeAnnotatedValue::Record { .. } => get_json(type_annotated_value),
            // Given no content type, we must jsonify it if it's not a byte stream
            TypeAnnotatedValue::List { values, typ } => {
                match typ {
                    // If the elements are u8, we consider it as a byte stream
                    AnalysedType::U8 => {
                        let bytes = get_byte_stream(values)?;
                        Ok(Body::from_bytes(bytes::Bytes::from(bytes)))
                    }
                    _ => get_json(type_annotated_value),
                }
            }

            TypeAnnotatedValue::Bool(bool) => Ok(Body::from_string(bool.to_string())),
            TypeAnnotatedValue::S8(s8) => Ok(Body::from_string(s8.to_string())),
            TypeAnnotatedValue::U8(u8) => Ok(Body::from_string(u8.to_string())),
            TypeAnnotatedValue::S16(s16) => Ok(Body::from_string(s16.to_string())),
            TypeAnnotatedValue::U16(u16) => Ok(Body::from_string(u16.to_string())),
            TypeAnnotatedValue::S32(s32) => Ok(Body::from_string(s32.to_string())),
            TypeAnnotatedValue::U32(u32) => Ok(Body::from_string(u32.to_string())),
            TypeAnnotatedValue::S64(s64) => Ok(Body::from_string(s64.to_string())),
            TypeAnnotatedValue::U64(u64) => Ok(Body::from_string(u64.to_string())),
            TypeAnnotatedValue::F32(f32) => Ok(Body::from_string(f32.to_string())),
            TypeAnnotatedValue::F64(f64) => Ok(Body::from_string(f64.to_string())),
            TypeAnnotatedValue::Chr(char) => Ok(Body::from_string(char.to_string())),
            TypeAnnotatedValue::Str(string) => Ok(Body::from_string(string.to_string())),
            TypeAnnotatedValue::Tuple { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Flags { .. } => get_json(type_annotated_value),
            // Variant is considered as a record of 1 element with type name and details
            // Confirm this behaviour
            TypeAnnotatedValue::Variant { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Enum { value, .. } => Ok(Body::from_string(value.to_string())),
            // Confirm this behaviour, given there is no specific content type
            TypeAnnotatedValue::Option { value, .. } => match value {
                Some(value) => get_response_body(value),
                None => Ok(Body::empty()),
            },
            // Can be considered as a record
            TypeAnnotatedValue::Result { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Handle { .. } => get_json(type_annotated_value),
        }
    }

    pub(crate) fn get_response_body_from_content_type(
        type_annotated_value: &TypeAnnotatedValue,
        content_type: &ContentType,
    ) -> Result<Body, ContentTypeMapError> {
        if content_type == &ContentType::json() {
            get_json_or_binary_stream(type_annotated_value)
        } else {
            convert_only_if_binary_stream(type_annotated_value)
        }
    }

    fn convert_only_if_binary_stream(type_annotated_value: &TypeAnnotatedValue) -> Result<Body, ContentTypeMapError> {
       match type_annotated_value {
            TypeAnnotatedValue::List { typ, values } => match typ {
                // It is already a binary stream and we keep it as is
                AnalysedType::U8 => get_byte_stream(values).map(|bytes| Body::from_bytes(bytes::Bytes::from(bytes))),
                // Convert to Json otherwise
                other => Err(ContentTypeMapError::expect_only_binary_stream(other))
            },
            // A string is considered as a binary stream
            TypeAnnotatedValue::Str(str) => Ok(Body::from(str.to_string())),
            other => Err(ContentTypeMapError::expect_only_binary_stream(&AnalysedType::from(other)))
       }
    }


    // If the request Content-Type is application/json, then we can allow ANY component model type of value as the response.
    // However, only Rib return values which are not equivalent to a binary stream are automatically converted into JSON (jsonification).
    fn get_json_or_binary_stream(type_annotated_value: &TypeAnnotatedValue) -> Result<Body, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::List { typ, values } => match typ {
                // It is already a binary stream and we keep it as is
                AnalysedType::U8 => get_byte_stream(values).map(|bytes| Body::from_bytes(bytes::Bytes::from(bytes))),
                // Convert to Json otherwise
                _ => get_json(type_annotated_value),
            },
            // A string is considered as a binary stream
            TypeAnnotatedValue::Str(str) => Ok(Body::from(str.to_string())),
            _ => get_json(type_annotated_value),
        }
    }

    fn get_json(type_annotated_value: &TypeAnnotatedValue) -> Result<Body, ContentTypeMapError> {
        let json = get_json_from_typed_value(type_annotated_value);
        Body::from_json(json)
            .map_err(|_| ContentTypeMapError::internal("Failed to convert to json"))
    }

    fn get_text(type_annotated_value: &TypeAnnotatedValue) -> Body {
        if let Some(primitive) = type_annotated_value.get_primitive() {
            let string = primitive.as_string();
            Body::from_string(string)
        } else {
            let json = get_json_from_typed_value(type_annotated_value);
            let json_str = json.to_string();
            Body::from_string(json_str)
        }
    }

    fn get_byte_stream(values: &Vec<TypeAnnotatedValue>) -> Result<Vec<u8>, ContentTypeMapError> {
        let bytes = values
            .into_iter()
            .map(|v| match v {
                TypeAnnotatedValue::U8(u8) => Ok(*u8),
                _ => Err(ContentTypeMapError::internal(
                    "Internal error in fetching vec<u8>",
                )),
            })
            .collect::<Result<Vec<_>, ContentTypeMapError>>()?;

        Ok(bytes)
    }
}
