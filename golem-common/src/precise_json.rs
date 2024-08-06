use async_trait::async_trait;
use golem_wasm_rpc::protobuf::type_annotated_value;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::typed_result::ResultValue;
use golem_wasm_rpc::Uri;
use poem_openapi::types::{ParseFromJSON, ToJSON};
use poem_openapi::{registry, types};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use thiserror::Error;

// This is different to wasm_rpc::Value mainly for the type `Record` that it holds `Keys`
// TODO; Evaluate the need of having the record know about keys, before moving this to wasm-rpc
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PreciseJson {
    Bool(bool),
    S8(i8),
    U8(u8),
    S16(i16),
    U16(u16),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Chr(char),
    Str(String),
    // List have the possibility of holding heterogeneous types here, but easy for users to understand this encoding
    List(Vec<PreciseJson>),
    Tuple(Vec<PreciseJson>),
    Record(HashMap<String, PreciseJson>),
    Variant {
        case_idx: u32,
        case_value: Box<PreciseJson>,
    },
    Enum(u32),
    Flags(Vec<bool>),
    Option(Option<Box<PreciseJson>>),
    Result(Result<Box<PreciseJson>, Box<PreciseJson>>),
    Handle {
        uri: String,
        resource_id: u64,
    },
}

impl Display for PreciseJson {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.to_json_string().fmt(f)
    }
}

impl types::Type for PreciseJson {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "OpenApiDefinition".into()
    }

    fn schema_ref() -> registry::MetaSchemaRef {
        registry::MetaSchemaRef::Inline(Box::new(registry::MetaSchema::ANY))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ToJSON for PreciseJson {
    fn to_json(&self) -> Option<Value> {
        serde_json::to_value(self).ok()
    }
}

#[async_trait]
impl ParseFromJSON for PreciseJson {
    fn parse_from_json(value: Option<serde_json::Value>) -> types::ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<PreciseJson>(value) {
                Ok(precise_json) => Ok(precise_json),
                Err(e) => Err(types::ParseError::<Self>::custom(format!(
                    "Failed to parse PreciseJson: {}",
                    e
                ))),
            },

            _ => Err(poem_openapi::types::ParseError::<Self>::custom(
                "Precise JSON missing".to_string(),
            )),
        }
    }
}

#[derive(Error, Debug)]
pub enum PreciseJsonConversionError {
    #[error("Missing field `{0}`")]
    MissingField(String),
    #[error("Invalid value: {0}")]
    InvalidValue(String),
    #[error("Invalid type annotation: {0}")]
    InvalidTypeAnnotation(String),
}

impl From<PreciseJson> for golem_wasm_rpc::Value {
    fn from(value: PreciseJson) -> Self {
        match value {
            PreciseJson::Bool(b) => golem_wasm_rpc::Value::Bool(b),
            PreciseJson::S8(i) => golem_wasm_rpc::Value::S8(i),
            PreciseJson::U8(u) => golem_wasm_rpc::Value::U8(u),
            PreciseJson::S16(i) => golem_wasm_rpc::Value::S16(i),
            PreciseJson::U16(u) => golem_wasm_rpc::Value::U16(u),
            PreciseJson::S32(i) => golem_wasm_rpc::Value::S32(i),
            PreciseJson::U32(u) => golem_wasm_rpc::Value::U32(u),
            PreciseJson::S64(i) => golem_wasm_rpc::Value::S64(i),
            PreciseJson::U64(u) => golem_wasm_rpc::Value::U64(u),
            PreciseJson::F32(f) => golem_wasm_rpc::Value::F32(f),
            PreciseJson::F64(f) => golem_wasm_rpc::Value::F64(f),
            PreciseJson::Chr(c) => golem_wasm_rpc::Value::Char(c),
            PreciseJson::Str(s) => golem_wasm_rpc::Value::String(s),
            PreciseJson::List(l) => golem_wasm_rpc::Value::List(
                l.into_iter().map(golem_wasm_rpc::Value::from).collect(),
            ),
            PreciseJson::Tuple(t) => golem_wasm_rpc::Value::Tuple(
                t.into_iter().map(golem_wasm_rpc::Value::from).collect(),
            ),
            PreciseJson::Record(r) => golem_wasm_rpc::Value::Record(
                r.into_values().map(golem_wasm_rpc::Value::from).collect(),
            ),
            PreciseJson::Variant {
                case_idx,
                case_value,
            } => golem_wasm_rpc::Value::Variant {
                case_idx,
                case_value: Some(Box::new(golem_wasm_rpc::Value::from(*case_value))),
            },
            PreciseJson::Enum(e) => golem_wasm_rpc::Value::Enum(e),
            PreciseJson::Flags(f) => golem_wasm_rpc::Value::Flags(f),

            PreciseJson::Option(option) => golem_wasm_rpc::Value::Option(
                option.map(|v| Box::new(golem_wasm_rpc::Value::from(*v))),
            ),
            PreciseJson::Result(result) => match result {
                Ok(precise_json) => golem_wasm_rpc::Value::Result(Ok(Some(Box::new(
                    golem_wasm_rpc::Value::from(*precise_json),
                )))),
                Err(precise_json) => golem_wasm_rpc::Value::Result(Err(Some(Box::new(
                    golem_wasm_rpc::Value::from(*precise_json),
                )))),
            },
            PreciseJson::Handle { uri, resource_id } => golem_wasm_rpc::Value::Handle {
                uri: Uri { value: uri },
                resource_id,
            },
        }
    }
}

// There is no reason to fail when going from TypeAnnotatedValue To PreciseJson
// Any unwraps are almost "never-possible" cases, or else bugs in wasm-rpc
impl From<type_annotated_value::TypeAnnotatedValue> for PreciseJson {
    fn from(value: TypeAnnotatedValue) -> Self {
        match value {
            TypeAnnotatedValue::Bool(bool) => PreciseJson::Bool(bool),
            TypeAnnotatedValue::S8(s8) => PreciseJson::S8(s8 as i8),
            TypeAnnotatedValue::U8(u8) => PreciseJson::U8(u8 as u8),
            TypeAnnotatedValue::S16(s16) => PreciseJson::S16(s16 as i16),
            TypeAnnotatedValue::U16(u16) => PreciseJson::U16(u16 as u16),
            TypeAnnotatedValue::S32(s32) => PreciseJson::S32(s32),
            TypeAnnotatedValue::U32(u32) => PreciseJson::U32(u32),
            TypeAnnotatedValue::S64(s64) => PreciseJson::S64(s64),
            TypeAnnotatedValue::U64(u64) => PreciseJson::U64(u64),
            TypeAnnotatedValue::F32(f32) => PreciseJson::F32(f32),
            TypeAnnotatedValue::F64(f64) => PreciseJson::F64(f64),
            TypeAnnotatedValue::Char(chr) => {
                char::from_u32(chr as u32).map(PreciseJson::Chr).unwrap()
            }
            TypeAnnotatedValue::Str(str) => PreciseJson::Str(str),
            TypeAnnotatedValue::List(list) => PreciseJson::List(
                list.values
                    .into_iter()
                    .map(|v| PreciseJson::from(v.type_annotated_value.unwrap()))
                    .collect(),
            ),
            TypeAnnotatedValue::Tuple(tuple) => PreciseJson::Tuple(
                tuple
                    .value
                    .into_iter()
                    .map(|v| PreciseJson::from(v.type_annotated_value.unwrap()))
                    .collect(),
            ),
            TypeAnnotatedValue::Record(record) => PreciseJson::Record(
                record
                    .value
                    .into_iter()
                    .map(|typed_record| {
                        (
                            typed_record.name,
                            PreciseJson::from(
                                typed_record
                                    .value
                                    .and_then(|v| v.type_annotated_value)
                                    .unwrap(),
                            ),
                        )
                    })
                    .collect(),
            ),
            TypeAnnotatedValue::Variant(variant) => {
                let index = variant
                    .typ
                    .unwrap()
                    .cases
                    .iter()
                    .enumerate()
                    .find(|(_, c)| (c.name.clone() == variant.case_name))
                    .map(|(i, _)| i);

                let type_annotated_value =
                    variant.case_value.unwrap().type_annotated_value.unwrap();

                PreciseJson::Variant {
                    case_idx: index.unwrap() as u32,
                    case_value: Box::new(PreciseJson::from(type_annotated_value)),
                }
            }
            TypeAnnotatedValue::Enum(e) => {
                let all_values = e.typ;
                let index = all_values
                    .into_iter()
                    .enumerate()
                    .find(|(_, v)| (v.clone() == e.value))
                    .map(|(i, _)| i)
                    .unwrap();

                PreciseJson::Enum(index as u32)
            }
            TypeAnnotatedValue::Flags(flags) => {
                let values = flags.values;

                let mut boolean_flags = vec![];

                for i in flags.typ {
                    if values.contains(&i) {
                        boolean_flags.push(true);
                    } else {
                        boolean_flags.push(false);
                    }
                }

                PreciseJson::Flags(boolean_flags)
            }

            TypeAnnotatedValue::Option(option) => PreciseJson::Option(
                option
                    .value
                    .map(|v| Box::new(PreciseJson::from(v.type_annotated_value.unwrap()))),
            ),
            TypeAnnotatedValue::Result(result) => {
                let result_value = result.result_value.unwrap();

                match result_value {
                    ResultValue::OkValue(ok) => PreciseJson::Result(Ok(Box::new(
                        PreciseJson::from(ok.type_annotated_value.unwrap()),
                    ))),
                    ResultValue::ErrorValue(err) => PreciseJson::Result(Err(Box::new(
                        PreciseJson::from(err.type_annotated_value.unwrap()),
                    ))),
                }
            }

            TypeAnnotatedValue::Handle(handle) => PreciseJson::Handle {
                uri: handle.uri,
                resource_id: handle.resource_id,
            },
        }
    }
}

#[cfg(test)]
mod typed_json_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_precise_json_u32() {
        let json_value = json!({ "type": "U32", "value" : 1 });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();
        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());
        assert_eq!(precise_json, PreciseJson::U32(1));
    }

    #[test]
    fn test_precise_json_bool() {
        let json_value = json!({ "type" : "Bool", "value": true });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();
        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(precise_json, PreciseJson::Bool(true));
    }

    #[test]
    fn test_precise_json_str() {
        let json_value = json!({ "type" : "Str", "value" : "hello" });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(precise_json, PreciseJson::Str("hello".to_string()));
    }

    #[test]
    fn test_precise_json_list() {
        let json_value = json!({  "type" : "List", "value": [{ "type": "U32", "value" : 1 }, { "type": "Str", "value" : "hello" }] });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(
            precise_json,
            PreciseJson::List(vec![
                PreciseJson::U32(1),
                PreciseJson::Str("hello".to_string())
            ])
        );
    }

    #[test]
    fn test_precise_json_record() {
        let json_value = json!({  "type":  "Record", "value": { "foo": { "type": "U32", "value": 2 }, "bar": { "type": "U64", "value" : 10 } } });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let mut map = HashMap::new();
        map.insert("bar".to_string(), PreciseJson::U64(10));
        map.insert("foo".to_string(), PreciseJson::U32(2));

        assert_eq!(precise_json, PreciseJson::Record(map));
    }

    #[test]
    fn test_precise_json_variant() {
        let json_value = json!({
            "type" : "Variant",
            "value": {
                "case_idx": 1,
                "case_value": { "type": "U32", "value" : 42 }
            }
        });

        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(
            precise_json,
            PreciseJson::Variant {
                case_idx: 1,
                case_value: Box::new(PreciseJson::U32(42))
            }
        );
    }

    #[test]
    fn test_precise_json_result_ok() {
        let json_value =
            json!({"type": "Result", "value":{"Ok":{"type": "Str", "value": "success"}}});

        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(
            precise_json,
            PreciseJson::Result(Ok(Box::new(PreciseJson::Str("success".to_string()))))
        );
    }

    #[test]
    fn test_precise_json_result_err() {
        let json_value =
            json!({"type": "Result", "value":{"Err":{"type": "Str", "value": "failure"}}});
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(
            precise_json,
            PreciseJson::Result(Err(Box::new(PreciseJson::Str("failure".to_string()))))
        );
    }

    #[test]
    fn test_precise_json_option_none() {
        let json_value = json!({ "type": "Option", "value" : null });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(precise_json, PreciseJson::Option(None));
    }

    #[test]
    fn test_precise_json_option_some() {
        let json_value = json!({ "type": "Option", "value" : {"type" : "U32", "value" : 42} });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        let written_json_str = serde_json::to_string(&precise_json).unwrap();

        assert_eq!(written_json_str, json_value.to_string());

        assert_eq!(
            precise_json,
            PreciseJson::Option(Some(Box::new(PreciseJson::U32(42))))
        );
    }

    #[test]
    fn test_precise_json_handle() {
        let json_value = json!({ "type": "Handle", "value": { "uri": "http://example.com", "resource_id": 42 } });
        let precise_json: PreciseJson = serde_json::from_value(json_value.clone()).unwrap();

        assert_eq!(
            precise_json,
            PreciseJson::Handle {
                uri: "http://example.com".to_string(),
                resource_id: 42
            }
        );
    }
}
