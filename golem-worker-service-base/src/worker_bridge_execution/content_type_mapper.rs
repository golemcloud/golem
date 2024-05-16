use crate::primitive::GetPrimitive;
use crate::worker_bridge_execution::to_response::ToResponse;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;
use poem::web::headers::ContentType;
use poem::{Body, IntoResponse};
use std::fmt::{Display, Formatter};

pub trait ContentTypeMapper {
    fn map(&self, content_type: &ContentType) -> Result<Body, ContentTypeMapError>;
}

impl ContentTypeMapper for TypeAnnotatedValue {
    fn map(&self, content_type: &ContentType) -> Result<Body, ContentTypeMapError> {
        if content_type == &ContentType::octet_stream() {
            get_vec_u8(self, Body::from_vec)
        } else if content_type == &ContentType::json() {
            Ok(get_json(self))
        } else if content_type == &ContentType::text() {
            Ok(get_text(self))
        } else if content_type == &ContentType::html() {
            Ok(get_text(self))
        } else if content_type == &ContentType::jpeg() {
            get_vec_u8(self, Body::from_vec)
        } else if content_type == &ContentType::text_utf8() {
            Ok(get_text(self))
        } else if content_type == &ContentType::xml() {
            Ok(get_text(self))
        } else if content_type == &ContentType::form_url_encoded() {
            Ok(get_text(self))
        } else if content_type == &ContentType::png() {
            get_vec_u8(self, Body::from_vec)
        } else {
            Err(ContentTypeMapError::internal("Unsupported content type"))
        }
    }
}

#[derive(PartialEq, Debug)]
enum ContentTypeMapError {
    UnsupportedWorkerFunctionResult {
        expected: AnalysedType,
        obtained: AnalysedType,
    },

    InternalError(String),
}

impl Display for ContentTypeMapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentTypeMapError::UnsupportedWorkerFunctionResult { expected, obtained } => {
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

impl ContentTypeMapError {
    fn internal(msg: &str) -> ContentTypeMapError {
        ContentTypeMapError::InternalError(msg.to_string())
    }
}

fn get_vec_u8<F>(type_annoted_value: &TypeAnnotatedValue, f: F) -> Result<Body, ContentTypeMapError>
    where
        F: Fn(Vec<u8>) -> Body,
{
    match type_annoted_value {
        TypeAnnotatedValue::List { values, typ } => match typ {
            AnalysedType::U8 => {
                let bytes = values
                    .into_iter()
                    .map(|v| match v {
                        TypeAnnotatedValue::U8(u8) => Ok(*u8),
                        _ => Err(ContentTypeMapError::internal(
                            "Internal error in fetching vec<u8>",
                        )),
                    })
                    .collect::<Result<Vec<_>, ContentTypeMapError>>()?;

                Ok(f(bytes))
            }
            other => Err(ContentTypeMapError::UnsupportedWorkerFunctionResult {
                expected: AnalysedType::List(Box::new(AnalysedType::U8)),
                obtained: AnalysedType::List(Box::new(other.clone())),
            }),
        },
        other => Err(ContentTypeMapError::UnsupportedWorkerFunctionResult {
            expected: AnalysedType::List(Box::new(AnalysedType::U8)),
            obtained: AnalysedType::from(other),
        }),
    }
}

fn get_json(type_annotated_value: &TypeAnnotatedValue) -> Body {
    let json = get_json_from_typed_value(type_annotated_value);
    Body::from_json(json).unwrap()
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
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_content_type_mapping() {
        // Create a TypeAnnotatedValue instance for testing
        let type_annotated_value = TypeAnnotatedValue::List {
            values: vec![
                TypeAnnotatedValue::U8(65),
                TypeAnnotatedValue::U8(66),
                TypeAnnotatedValue::U8(67),
            ],
            typ: AnalysedType::U8,
        };

        // // Test mapping for various content types
        //
        // // Test mapping for ContentType::octet_stream()
        // let result_octet_stream = type_annotated_value.map(&ContentType::octet_stream()).map(|x| x.into_string().awa);
        // assert_eq!(result_octet_stream, Ok(vec![65, 66, 67]));
        //
        // // Test mapping for ContentType::json()
        // let result_json = type_annotated_value.map(&ContentType::json());
        // assert_eq!(result_json, Ok(Body::from_json(json!([65, 66, 67])).unwrap()));
        //
        // // Test mapping for ContentType::text()
        // let result_text = type_annotated_value.map(&ContentType::text());
        // assert_eq!(result_text, Ok(Body::from_string("ABC".to_string())));
        //
        // // Test mapping for ContentType::html()
        // let result_html = type_annotated_value.map(&ContentType::html());
        // assert_eq!(result_html, Ok(Body::from_string("ABC".to_string())));
        //
        // // Test mapping for ContentType::jpeg()
        // let result_jpeg = type_annotated_value.map(&ContentType::jpeg());
        // assert_eq!(result_jpeg, Ok(Body::from_vec(vec![65, 66, 67])));
        //
        // // Test mapping for ContentType::text_utf8()
        // let result_text_utf8 = type_annotated_value.map(&ContentType::text_utf8());
        // assert_eq!(result_text_utf8, Ok(Body::from_string("ABC".to_string())));
        //
        // // Test mapping for ContentType::xml()
        // let result_xml = type_annotated_value.map(&ContentType::xml());
        // assert_eq!(result_xml, Ok(Body::from_string("ABC".to_string())));
        //
        // // Test mapping for ContentType::form_url_encoded()
        // let result_form_url_encoded = type_annotated_value.map(&ContentType::form_url_encoded());
        // assert_eq!(result_form_url_encoded, Ok(Body::from_string("ABC".to_string())));
        //
        // // Test mapping for ContentType::png()
        // let result_png = type_annotated_value.map(&ContentType::png());
        // assert_eq!(result_png, Ok(Body::from_vec(vec![65, 66, 67])));
        //
        // // Test mapping for an unsupported content type
        // let result_unsupported = type_annotated_value.map(&ContentType::unknown());
        // assert!(result_unsupported.is_err());
    }
}

