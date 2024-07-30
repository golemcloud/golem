use golem_wasm_rpc::protobuf::type_annotated_value;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::typed_result::ResultValue;
use golem_wasm_rpc::Uri;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::str::FromStr;

use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    Record(Vec<(String, PreciseJson)>),
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

#[derive(Error, Debug)]
pub enum PreciseJsonConversionError {
    #[error("Missing field `{0}`")]
    MissingField(String),
    #[error("Invalid value: {0}")]
    InvalidValue(String),
    #[error("Invalid type annotation: {0}")]
    InvalidTypeAnnotation(String),
}

impl Into<JsonValue> for PreciseJson {
    fn into(self) -> JsonValue {
        match self {
            PreciseJson::Bool(b) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "bool".to_string(),
                JsonValue::Bool(b),
            )])),
            PreciseJson::S8(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "s8".to_string(),
                JsonValue::Number((n as i64).into()),
            )])),
            PreciseJson::U8(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "u8".to_string(),
                JsonValue::Number((n as u64).into()),
            )])),
            PreciseJson::S16(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "s16".to_string(),
                JsonValue::Number((n as i64).into()),
            )])),
            PreciseJson::U16(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "u16".to_string(),
                JsonValue::Number((n as u64).into()),
            )])),
            PreciseJson::S32(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "s32".to_string(),
                JsonValue::Number((n as i64).into()),
            )])),
            PreciseJson::U32(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "u32".to_string(),
                JsonValue::Number((n as u64).into()),
            )])),
            PreciseJson::S64(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "s64".to_string(),
                JsonValue::Number(n.into()),
            )])),
            PreciseJson::U64(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "u64".to_string(),
                JsonValue::Number(n.into()),
            )])),
            PreciseJson::F32(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "f32".to_string(),
                JsonValue::Number(serde_json::Number::from_f64(n as f64).unwrap()),
            )])),
            PreciseJson::F64(n) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "f64".to_string(),
                JsonValue::Number(serde_json::Number::from_f64(n).unwrap()),
            )])),
            PreciseJson::Chr(c) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "chr".to_string(),
                JsonValue::String(c.to_string()),
            )])),
            PreciseJson::Str(s) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "str".to_string(),
                JsonValue::String(s),
            )])),
            PreciseJson::List(lst) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "list".to_string(),
                JsonValue::Array(lst.into_iter().map(|v| v.into()).collect()),
            )])),
            PreciseJson::Tuple(tpl) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "tuple".to_string(),
                JsonValue::Array(tpl.into_iter().map(|v| v.into()).collect()),
            )])),
            PreciseJson::Record(rec) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "record".to_string(),
                JsonValue::Object(rec.into_iter().map(|(k, v)| (k, v.into())).collect()),
            )])),
            PreciseJson::Variant {
                case_idx,
                case_value,
            } => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "variant".to_string(),
                JsonValue::Object(serde_json::Map::from_iter(vec![
                    ("case_idx".to_string(), JsonValue::Number(case_idx.into())),
                    ("case_value".to_string(), (*case_value).into()),
                ])),
            )])),
            PreciseJson::Enum(e) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "enum".to_string(),
                JsonValue::Number(e.into()),
            )])),
            PreciseJson::Flags(flags) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "flags".to_string(),
                JsonValue::Array(flags.into_iter().map(JsonValue::Bool).collect()),
            )])),
            PreciseJson::Option(opt) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "option".to_string(),
                match opt {
                    Some(boxed) => (*boxed).into(),
                    None => JsonValue::Null,
                },
            )])),
            PreciseJson::Result(res) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                "result".to_string(),
                match res {
                    Ok(boxed) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                        "ok".to_string(),
                        (*boxed).into(),
                    )])),
                    Err(boxed) => JsonValue::Object(serde_json::Map::from_iter(vec![(
                        "err".to_string(),
                        (*boxed).into(),
                    )])),
                },
            )])),
            PreciseJson::Handle { uri, resource_id } => {
                JsonValue::Object(serde_json::Map::from_iter(vec![(
                    "handle".to_string(),
                    JsonValue::String(format!("{}/{}", uri, resource_id)),
                )]))
            }
        }
    }
}

impl TryFrom<JsonValue> for PreciseJson {
    type Error = PreciseJsonConversionError;

    fn try_from(value: JsonValue) -> Result<Self, Self::Error> {
        match value {
            JsonValue::Object(obj) => {
                if obj.len() != 1 {
                    return Err(PreciseJsonConversionError::InvalidTypeAnnotation(format!(
                        "Expected a single key, found {} keys",
                        obj.len()
                    )));
                }

                let (key, value) = obj.into_iter().next().unwrap();
                match key.as_str() {
                    "bool" => match value {
                        JsonValue::Bool(b) => Ok(PreciseJson::Bool(b)),
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected boolean value".to_string(),
                        )),
                    },
                    "s8" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S8(n.as_i64().unwrap() as i8))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected s8 value".to_string(),
                        )),
                    },
                    "u8" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U8(n.as_u64().unwrap() as u8))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected u8 value".to_string(),
                        )),
                    },
                    "s16" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S16(n.as_i64().unwrap() as i16))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected s16 value".to_string(),
                        )),
                    },
                    "u16" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U16(n.as_u64().unwrap() as u16))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected u16 value".to_string(),
                        )),
                    },
                    "s32" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S32(n.as_i64().unwrap() as i32))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected s32 value".to_string(),
                        )),
                    },
                    "u32" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U32(n.as_u64().unwrap() as u32))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected u32 value".to_string(),
                        )),
                    },
                    "s64" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S64(n.as_i64().unwrap()))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected s64 value".to_string(),
                        )),
                    },
                    "u64" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U64(n.as_u64().unwrap()))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected u64 value".to_string(),
                        )),
                    },
                    "f32" => match value {
                        JsonValue::Number(n) if n.is_f64() => {
                            Ok(PreciseJson::F32(n.as_f64().unwrap() as f32))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected f32 value".to_string(),
                        )),
                    },
                    "f64" => match value {
                        JsonValue::Number(n) if n.is_f64() => {
                            Ok(PreciseJson::F64(n.as_f64().unwrap()))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected f64 value".to_string(),
                        )),
                    },
                    "chr" => match value {
                        JsonValue::String(s) if s.chars().count() == 1 => {
                            Ok(PreciseJson::Chr(s.chars().next().unwrap()))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected single character".to_string(),
                        )),
                    },
                    "str" => match value {
                        JsonValue::String(s) => Ok(PreciseJson::Str(s)),
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected string value".to_string(),
                        )),
                    },
                    "list" => match value {
                        JsonValue::Array(arr) => {
                            let elems: Result<Vec<PreciseJson>, _> =
                                arr.into_iter().map(PreciseJson::try_from).collect();
                            elems.map(PreciseJson::List)
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected list value".to_string(),
                        )),
                    },
                    "tuple" => match value {
                        JsonValue::Array(arr) => {
                            let elems: Result<Vec<PreciseJson>, _> =
                                arr.into_iter().map(PreciseJson::try_from).collect();
                            elems.map(PreciseJson::Tuple)
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected tuple value".to_string(),
                        )),
                    },
                    "record" => match value {
                        JsonValue::Object(record) => {
                            let record_elems: Result<Vec<(String, PreciseJson)>, _> = record
                                .into_iter()
                                .map(|(k, v)| PreciseJson::try_from(v).map(|p| (k, p)))
                                .collect();
                            record_elems.map(PreciseJson::Record)
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected record value".to_string(),
                        )),
                    },
                    "variant" => match value {
                        JsonValue::Object(variant) => {
                            let case_idx = variant
                                .get("case_idx")
                                .and_then(|v| v.as_number().and_then(|n| n.as_i64()))
                                .ok_or_else(|| {
                                    PreciseJsonConversionError::MissingField("case_idx".to_string())
                                })
                                .and_then(|idx| {
                                    u32::try_from(idx).map_err(|_| {
                                        PreciseJsonConversionError::InvalidValue(
                                            "Invalid index for variant".to_string(),
                                        )
                                    })
                                })?;

                            let case_value = variant
                                .get("case_value")
                                .ok_or(PreciseJsonConversionError::MissingField(
                                    "case_value".to_string(),
                                ))
                                .and_then(|v| PreciseJson::try_from(v.clone()))?;
                            Ok(PreciseJson::Variant {
                                case_idx,
                                case_value: Box::new(case_value),
                            })
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected variant value".to_string(),
                        )),
                    },
                    "enum" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::Enum(n.as_u64().unwrap() as u32))
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected enum value".to_string(),
                        )),
                    },
                    "flags" => match value {
                        JsonValue::Array(arr) => {
                            let flags: Result<Vec<bool>, _> = arr
                                .into_iter()
                                .map(|v| match v {
                                    JsonValue::Bool(b) => Ok(b),
                                    _ => Err(PreciseJsonConversionError::InvalidValue(
                                        "Expected boolean value in flags".to_string(),
                                    )),
                                })
                                .collect();
                            flags.map(PreciseJson::Flags)
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected flags value".to_string(),
                        )),
                    },
                    "option" => match value {
                        JsonValue::Null => Ok(PreciseJson::Option(None)),
                        _ => {
                            let boxed = Box::new(PreciseJson::try_from(value)?);
                            Ok(PreciseJson::Option(Some(boxed)))
                        }
                    },
                    "result" => match value {
                        JsonValue::Object(result) => {
                            if result.len() != 1 {
                                return Err(PreciseJsonConversionError::InvalidValue(
                                    "Expected result object with exactly one field".to_string(),
                                ));
                            }
                            let (k, v) = result.into_iter().next().unwrap();
                            match k.as_str() {
                                "ok" => {
                                    Ok(PreciseJson::Result(Ok(Box::new(PreciseJson::try_from(v)?))))
                                }
                                "err" => Ok(PreciseJson::Result(Err(Box::new(
                                    PreciseJson::try_from(v)?,
                                )))),
                                _ => Err(PreciseJsonConversionError::InvalidValue(
                                    "Expected result key to be 'Ok' or 'Err'".to_string(),
                                )),
                            }
                        }
                        _ => Err(PreciseJsonConversionError::InvalidValue(
                            "Expected result key to be 'Ok' or 'Err'".to_string(),
                        )),
                    },
                    "handle" => {
                        match value.as_str() {
                            Some(str) => {
                                // not assuming much about the url format, just checking it ends with a /<resource-id-u64>
                                let parts: Vec<&str> = str.split('/').collect();
                                if parts.len() >= 2 {
                                    match u64::from_str(parts[parts.len() - 1]) {
                                        Ok(resource_id) => {
                                            let uri = parts[0..(parts.len() - 1)].join("/");

                                            let handle = PreciseJson::Handle {
                                                uri,
                                                resource_id
                                            };
                                            Ok(handle)
                                        }
                                        Err(err) => {
                                            Err(PreciseJsonConversionError::InvalidValue("Failed to parse resource-id section of the handle value".to_string()))
                                        }
                                    }
                                } else {
                                    Err(PreciseJsonConversionError::InvalidValue(
                                        "Expected function parameter type is Handle, represented by a worker-url/resource-id string".to_string(),
                                    ))
                                }
                            }
                            None => Err(PreciseJsonConversionError::InvalidValue(
                                "Expected function parameter type is Handle, represented by a worker-url/resource-id string".to_string()
                            )),
                        }
                    }
                    _ => Err(PreciseJsonConversionError::InvalidValue(
                        "Expected result object".to_string(),
                    )),
                }
            }
            _ => Err(PreciseJsonConversionError::InvalidValue(
                "Expected object".to_string(),
            )),
        }
    }
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
                r.into_iter()
                    .map(|(_, v)| golem_wasm_rpc::Value::from(v))
                    .collect(),
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
        let json_value = json!({ "u32": 1 });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(precise_json, PreciseJson::U32(1));
    }

    #[test]
    fn test_precise_json_bool() {
        let json_value = json!({ "bool": true });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(precise_json, PreciseJson::Bool(true));
    }

    #[test]
    fn test_precise_json_str() {
        let json_value = json!({ "str": "hello" });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(precise_json, PreciseJson::Str("hello".to_string()));
    }

    #[test]
    fn test_precise_json_list() {
        let json_value = json!({ "list": [{ "u32": 1 }, { "str": "hello" }] });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
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
        let json_value = json!({ "record": { "foo": { "u32": 2 }, "bar": { "u64": 10 } } });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(
            precise_json,
            PreciseJson::Record(vec![
                ("bar".to_string(), PreciseJson::U64(10)),
                ("foo".to_string(), PreciseJson::U32(2)),
            ])
        );
    }

    #[test]
    fn test_precise_json_variant() {
        let json_value = json!({
            "variant": {
                "case_idx": 1,
                "case_value": { "u32": 42 }
            }
        });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
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
        let json_value = json!({
            "result": { "ok": { "str": "success" } }
        });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(
            precise_json,
            PreciseJson::Result(Ok(Box::new(PreciseJson::Str("success".to_string()))))
        );
    }

    #[test]
    fn test_precise_json_result_err() {
        let json_value = json!({
            "result": { "err": { "str": "failure" } }
        });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(
            precise_json,
            PreciseJson::Result(Err(Box::new(PreciseJson::Str("failure".to_string()))))
        );
    }

    #[test]
    fn test_precise_json_option_none() {
        let json_value = json!({ "option": null });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(precise_json, PreciseJson::Option(None));
    }

    #[test]
    fn test_precise_json_option_some() {
        let json_value = json!({ "option": { "u32": 42 } });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(
            precise_json,
            PreciseJson::Option(Some(Box::new(PreciseJson::U32(42))))
        );
    }
}
