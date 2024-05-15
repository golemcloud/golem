use std::ops::Deref;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;
use poem::{Body, Response};
use poem::web::headers::ContentType;
use crate::primitive::GetPrimitive;
use crate::worker_bridge_execution::to_response::ToResponse;

trait ContentTypeMapper {
    fn map(&self, content_type: ContentType) -> Result<Body, ContentTypeMapError>;
}

impl ContentTypeMapper for TypeAnnotatedValue {
    fn map(&self, content_type: ContentType) -> Result<Body, ContentTypeMapError> {
        if content_type == ContentType::octet_stream() {
            get_vec_u8(self,  Body::from_vec)
        } else if content_type == ContentType::json() {
            Ok(get_json(self))
        } else if content_type == ContentType::text() {
            Ok(get_text(self))
        } else if content_type == ContentType::html() {
            Ok(get_text(self))
        } else if content_type == ContentType::jpeg() {
            get_vec_u8(self, Body::from_vec)
        } else if content_type == ContentType::text_utf8() {
            Ok(get_text(self))
        } else if content_type == ContentType::xml() {
            Ok(get_)
        }
    }
}
enum ContentTypeMapError {
    UnsupportedWorkerFunctionResult {
         expected: AnalysedType,
         obtained: AnalysedType,
    },

    InternalError(String)
}

impl ContentTypeMapError {
    fn internal(msg: &str) -> ContentTypeMapError {
        ContentTypeMapError::InternalError(msg.to_string())
    }
}

fn get_vec_u8<F>(type_annoted_value: &TypeAnnotatedValue, f: F) -> Result<Body, ContentTypeMapError> where F: Fn(Vec<u8>) -> Body {
   match type_annoted_value {
       TypeAnnotatedValue::List {values, typ} => {
           match typ {
               AnalysedType::U8 => {
                   let bytes = values.into_iter().map(|v|  match v {
                       TypeAnnotatedValue::U8(u8) => Ok(u8),
                       _ => Err(ContentTypeMapError::internal("Internal error in fetching vec<u8>"))
                   }).collect::<Result<Vec<u8>, ContentTypeMapError>>()?;

                   Ok(f(bytes))
               },
               other => Err(ContentTypeMapError::UnsupportedWorkerFunctionResult {
                   expected: AnalysedType::List(Box::new(AnalysedType::U8)),
                   obtained: AnalysedType::List(Box::new(other.clone()))
               })
           }
       },
       other => Err(ContentTypeMapError::UnsupportedWorkerFunctionResult {
           expected: AnalysedType::List(Box::new(AnalysedType::U8)),
           obtained: AnalysedType::from(&other)
       })
   }
}

fn get_json<F>(type_annotated_value: &TypeAnnotatedValue) -> Body {
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

