use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use poem::web::headers::ContentType;
use poem::web::WithContentType;
use poem::Body;
use std::fmt::{Display, Formatter};

pub trait HttpContentTypeResponseMapper {
    fn to_http_response_body(
        &self,
        content_type_headers: ContentTypeHeaders,
    ) -> Result<WithContentType<Body>, ContentTypeMapError>;
}

pub enum ContentTypeHeaders {
    FromClientAccept(AcceptHeaders),
    FromUserDefinedResponseMapping(ContentType),
    Empty,
}

#[derive(Clone)]
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
        self.0
            .iter()
            .any(|v| v.contains(ContentType::json().to_string().as_str()))
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

impl HttpContentTypeResponseMapper for TypeAnnotatedValue {
    fn to_http_response_body(
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
    use crate::worker_bridge_execution::content_type_mapper::{
        AcceptHeaders, ContentTypeHeaderExt, ContentTypeMapError,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_json_from_typed_value;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use poem::web::headers::ContentType;
    use poem::web::WithContentType;
    use poem::{Body, IntoResponse};
    use std::fmt::Display;

    pub(crate) fn get_response_body_based_on_content_type<A: ContentTypeHeaderExt + Display>(
        type_annotated_value: &TypeAnnotatedValue,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::Record { .. } => {
                handle_record(type_annotated_value, content_header)
            }
            TypeAnnotatedValue::List { values, typ } => {
                handle_list(type_annotated_value, values, typ, content_header)
            }
            TypeAnnotatedValue::Bool(bool) => {
                handle_primitive(bool, &AnalysedType::Bool, content_header)
            }
            TypeAnnotatedValue::S8(s8) => handle_primitive(s8, &AnalysedType::S8, content_header),
            TypeAnnotatedValue::U8(u8) => handle_primitive(u8, &AnalysedType::U8, content_header),
            TypeAnnotatedValue::S16(s16) => {
                handle_primitive(s16, &AnalysedType::S16, content_header)
            }
            TypeAnnotatedValue::U16(u16) => {
                handle_primitive(u16, &AnalysedType::U16, content_header)
            }
            TypeAnnotatedValue::S32(s32) => {
                handle_primitive(s32, &AnalysedType::S32, content_header)
            }
            TypeAnnotatedValue::U32(u32) => {
                handle_primitive(u32, &AnalysedType::U32, content_header)
            }
            TypeAnnotatedValue::S64(s64) => {
                handle_primitive(s64, &AnalysedType::S64, content_header)
            }
            TypeAnnotatedValue::U64(u64) => {
                handle_primitive(u64, &AnalysedType::U64, content_header)
            }
            TypeAnnotatedValue::F32(f32) => {
                handle_primitive(f32, &AnalysedType::F32, content_header)
            }
            TypeAnnotatedValue::F64(f64) => {
                handle_primitive(f64, &AnalysedType::F64, content_header)
            }
            TypeAnnotatedValue::Chr(char) => {
                handle_primitive(char, &AnalysedType::Chr, content_header)
            }
            TypeAnnotatedValue::Str(string) => handle_string(string, content_header),

            TypeAnnotatedValue::Tuple { .. } => {
                handle_complex(type_annotated_value, content_header)
            }
            TypeAnnotatedValue::Flags { .. } => {
                handle_complex(type_annotated_value, content_header)
            }
            TypeAnnotatedValue::Variant { .. } => {
                handle_record(type_annotated_value, content_header)
            }
            TypeAnnotatedValue::Enum { value, .. } => handle_string(value, content_header),

            TypeAnnotatedValue::Option { value, .. } => match value {
                Some(value) => get_response_body_based_on_content_type(value, content_header),
                None => {
                    if content_header.has_application_json() {
                        get_json_null()
                    } else {
                        Err(ContentTypeMapError::illegal_mapping(
                            &AnalysedType::from(type_annotated_value),
                            content_header,
                        ))
                    }
                }
            },
            // Can be considered as a record
            TypeAnnotatedValue::Result { .. } => {
                handle_complex(type_annotated_value, content_header)
            }
            TypeAnnotatedValue::Handle { .. } => {
                handle_complex(type_annotated_value, content_header)
            }
        }
    }

    pub(crate) fn get_response_body(
        type_annotated_value: &TypeAnnotatedValue,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match type_annotated_value {
            TypeAnnotatedValue::Record { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::List { values, typ } => match typ {
                AnalysedType::U8 => get_byte_stream_body(values),
                _ => get_json(type_annotated_value),
            },

            TypeAnnotatedValue::Str(string) => Ok(Body::from_string(string.to_string())
                .with_content_type(ContentType::json().to_string())),

            TypeAnnotatedValue::Enum { value, .. } => Ok(Body::from_string(value.to_string())
                .with_content_type(ContentType::json().to_string())),

            TypeAnnotatedValue::Bool(bool) => get_json_of(bool),
            TypeAnnotatedValue::S8(s8) => get_json_of(s8),
            TypeAnnotatedValue::U8(u8) => get_json_of(u8),
            TypeAnnotatedValue::S16(s16) => get_json_of(s16),
            TypeAnnotatedValue::U16(u16) => get_json_of(u16),
            TypeAnnotatedValue::S32(s32) => get_json_of(s32),
            TypeAnnotatedValue::U32(u32) => get_json_of(u32),
            TypeAnnotatedValue::S64(s64) => get_json_of(s64),
            TypeAnnotatedValue::U64(u64) => get_json_of(u64),
            TypeAnnotatedValue::F32(f32) => get_json_of(f32),
            TypeAnnotatedValue::F64(f64) => get_json_of(f64),
            TypeAnnotatedValue::Chr(char) => get_json_of(char),
            TypeAnnotatedValue::Tuple { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Flags { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Variant { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Option { value, .. } => match value {
                Some(value) => get_response_body(value),
                None => get_json_null(),
            },
            // Can be considered as a record
            TypeAnnotatedValue::Result { .. } => get_json(type_annotated_value),
            TypeAnnotatedValue::Handle { .. } => get_json(type_annotated_value),
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

    fn get_byte_stream(values: &[TypeAnnotatedValue]) -> Result<Vec<u8>, ContentTypeMapError> {
        let bytes = values
            .iter()
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
        values: &[TypeAnnotatedValue],
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

    fn get_json_of<A: serde::Serialize + Display>(
        a: A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let json = serde_json::to_value(&a).map_err(|_| {
            ContentTypeMapError::internal(format!("Failed to serialise {} to json", a))
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
        complex: &TypeAnnotatedValue,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        if content_header.has_application_json() {
            get_json(complex)
        } else {
            Err(ContentTypeMapError::illegal_mapping(
                &AnalysedType::from(complex),
                content_header,
            ))
        }
    }

    fn handle_list<A: ContentTypeHeaderExt + Display>(
        original: &TypeAnnotatedValue,
        inner_values: &[TypeAnnotatedValue],
        list_type: &AnalysedType,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        match list_type {
            AnalysedType::U8 => {
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
                        list_type,
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
                ContentTypeMapError::internal(format!("Failed to convert {} to json body", input))
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
        record: &TypeAnnotatedValue,
        content_header: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        // if record, we prioritise JSON
        if content_header.has_application_json() {
            get_json(record)
        } else {
            // There is no way a Record can be properly serialised into any other formats to satisfy any other headers, therefore fail
            Err(ContentTypeMapError::illegal_mapping(
                &AnalysedType::from(record),
                content_header,
            ))
        }
    }

    fn handle_string<A: ContentTypeHeaderExt + Display>(
        string: &str,
        content_type: &A,
    ) -> Result<WithContentType<Body>, ContentTypeMapError> {
        let response_content_type = content_type.response_content_type()?;
        Ok(Body::from_bytes(bytes::Bytes::from(string.to_string()))
            .with_content_type(response_content_type.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::web::headers::ContentType;
    use poem::IntoResponse;
    use serde_json::Value;

    fn sample_record() -> TypeAnnotatedValue {
        TypeAnnotatedValue::Record {
            typ: vec![("name".to_string(), AnalysedType::Str)],
            value: vec![(
                "name".to_string(),
                TypeAnnotatedValue::Str("Hello".to_string()),
            )],
        }
    }

    #[cfg(test)]
    mod no_content_type_header {
        use super::*;

        fn get_content_type_and_body(input: &TypeAnnotatedValue) -> (Option<String>, Body) {
            let response_body = internal::get_response_body(input).unwrap();
            let response = response_body.into_response();
            let (parts, body) = response.into_parts();
            let content_type = parts
                .headers
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_string());
            (content_type, body)
        }

        #[tokio::test]
        async fn test_string_type() {
            let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            // Had it serialized as json, it would have been "\"Hello\""
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_singleton_u8_type() {
            let type_annotated_value = TypeAnnotatedValue::U8(10);
            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("10".to_string(), Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_list_u8_type() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U8(10)],
                typ: AnalysedType::U8,
            };
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

        #[tokio::test]
        async fn test_list_non_u8_type() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U16(10)],
                typ: AnalysedType::U16,
            };

            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: Value = serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(10),
            )]);
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_record_type() {
            let type_annotated_value = TypeAnnotatedValue::Record {
                typ: vec![("name".to_string(), AnalysedType::Str)],
                value: vec![(
                    "name".to_string(),
                    TypeAnnotatedValue::Str("Hello".to_string()),
                )],
            };

            let (content_type, body) = get_content_type_and_body(&type_annotated_value);
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: Value = serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::json!({"name": "Hello"});
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }
    }

    #[cfg(test)]
    mod with_response_type_header {
        use super::*;

        fn get_content_type_and_body(
            input: &TypeAnnotatedValue,
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

        #[tokio::test]
        async fn test_string_type_as_text() {
            let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::text());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            // Had it serialized as json, it would have been "\"Hello\""
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("text/plain".to_string()))
            );
        }

        #[tokio::test]
        async fn test_string_type_as_json() {
            let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_json_string_type_as_json() {
            let type_annotated_value = TypeAnnotatedValue::Str("\"Hello\"".to_string());
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                (
                    "\"Hello\"".to_string(),
                    Some("application/json".to_string())
                )
            );
        }

        #[tokio::test]
        async fn test_singleton_u8_type() {
            let type_annotated_value = TypeAnnotatedValue::U8(10);
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("10".to_string(), Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_list_u8_type() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U8(10)],
                typ: AnalysedType::U8,
            };
            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let result = &body.into_bytes().await.unwrap();
            let data_as_str = String::from_utf8_lossy(result).to_string();
            let result_json: Result<Value, _> = serde_json::from_str(data_as_str.as_str());

            assert_eq!(
                (result, content_type),
                (
                    &bytes::Bytes::from(vec![10]),
                    Some("application/json".to_string())
                )
            );
            assert!(result_json.is_err()); // That we haven't jsonified this case explicitly
        }

        #[tokio::test]
        async fn test_list_non_u8_type() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U16(10)],
                typ: AnalysedType::U16,
            };

            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: Value = serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(10),
            )]);
            // That we jsonify any list other than u8, and can be retrieveed as a valid JSON
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_record_type() {
            // Record
            let type_annotated_value = sample_record();

            let (content_type, body) =
                get_content_type_and_body(&type_annotated_value, &ContentType::json());
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: Value = serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::json!({"name": "Hello"});
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }
    }

    #[cfg(test)]
    mod with_accept_headers {
        use super::*;

        fn get_content_type_and_body(
            input: &TypeAnnotatedValue,
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

        #[tokio::test]
        async fn test_string_type_with_json() {
            let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_string_type_without_json() {
            let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let result = String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            assert_eq!(
                (result, content_type),
                ("Hello".to_string(), Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_string_type_with_html() {
            let type_annotated_value = TypeAnnotatedValue::Str("Hello".to_string());
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

        #[tokio::test]
        async fn test_singleton_u8_type_text() {
            let type_annotated_value = TypeAnnotatedValue::U8(10);
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

        #[tokio::test]
        async fn test_singleton_u8_type_json() {
            let type_annotated_value = TypeAnnotatedValue::U8(10);
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

        #[tokio::test]
        async fn test_singleton_u8_failed_content_mapping() {
            let type_annotated_value = TypeAnnotatedValue::U8(10);
            let result = internal::get_response_body_based_on_content_type(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html"),
            );

            assert!(matches!(
                result,
                Err(ContentTypeMapError::IllegalMapping { .. })
            ));
        }

        #[tokio::test]
        async fn test_list_u8_type_with_json() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U8(10)],
                typ: AnalysedType::U8,
            };
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.50"),
            );
            let result = &body.into_bytes().await.unwrap();
            let data_as_str = String::from_utf8_lossy(result).to_string();
            let result_json: Result<Value, _> = serde_json::from_str(data_as_str.as_str());

            assert_eq!(
                (result, content_type),
                (
                    &bytes::Bytes::from(vec![10]),
                    Some("application/json".to_string())
                )
            );
            assert!(result_json.is_err()); // That we haven't jsonified this case explicitly
        }

        #[tokio::test]
        async fn test_list_non_u8_type_with_json() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U16(10)],
                typ: AnalysedType::U16,
            };

            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: Value = serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(10),
            )]);
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[tokio::test]
        async fn test_list_non_u8_type_with_html_fail() {
            let type_annotated_value = TypeAnnotatedValue::List {
                values: vec![TypeAnnotatedValue::U16(10)],
                typ: AnalysedType::U16,
            };

            let result = internal::get_response_body_based_on_content_type(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html"),
            );

            assert!(matches!(
                result,
                Err(ContentTypeMapError::IllegalMapping { .. })
            ));
        }

        #[tokio::test]
        async fn test_record_type_json() {
            let type_annotated_value = sample_record();
            let (content_type, body) = get_content_type_and_body(
                &type_annotated_value,
                &AcceptHeaders::from_str("text/html;q=0.8, application/json;q=0.5"),
            );
            let data_as_str =
                String::from_utf8_lossy(&body.into_bytes().await.unwrap()).to_string();
            let result_json: Value = serde_json::from_str(data_as_str.as_str()).unwrap();
            let expected_json = serde_json::json!({"name": "Hello"});
            assert_eq!(
                (result_json, content_type),
                (expected_json, Some("application/json".to_string()))
            );
        }

        #[tokio::test]
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
