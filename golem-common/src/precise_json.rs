use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

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
}

#[derive(Error, Debug)]
pub enum PreciseJsonError {
    #[error("Unexpected JSON type")]
    UnexpectedJsonType,
    #[error("Missing field `{0}`")]
    MissingField(String),
    #[error("Invalid value: {0}")]
    InvalidValue(String),
    #[error("Invalid type annotation: {0}")]
    InvalidTypeAnnotation(String),
}

impl TryFrom<JsonValue> for PreciseJson {
    type Error = PreciseJsonError;

    fn try_from(value: JsonValue) -> Result<Self, Self::Error> {
        match value {
            JsonValue::Object(obj) => {
                if obj.len() != 1 {
                    return Err(PreciseJsonError::InvalidTypeAnnotation(format!(
                        "Expected a single key, found {} keys",
                        obj.len()
                    )));
                }

                let (key, value) = obj.into_iter().next().unwrap();
                match key.as_str() {
                    "bool" => match value {
                        JsonValue::Bool(b) => Ok(PreciseJson::Bool(b)),
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected boolean value".to_string(),
                        )),
                    },
                    "s8" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S8(n.as_i64().unwrap() as i8))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected s8 value".to_string(),
                        )),
                    },
                    "u8" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U8(n.as_u64().unwrap() as u8))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected u8 value".to_string(),
                        )),
                    },
                    "s16" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S16(n.as_i64().unwrap() as i16))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected s16 value".to_string(),
                        )),
                    },
                    "u16" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U16(n.as_u64().unwrap() as u16))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected u16 value".to_string(),
                        )),
                    },
                    "s32" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S32(n.as_i64().unwrap() as i32))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected s32 value".to_string(),
                        )),
                    },
                    "u32" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U32(n.as_u64().unwrap() as u32))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected u32 value".to_string(),
                        )),
                    },
                    "s64" => match value {
                        JsonValue::Number(n) if n.is_i64() => {
                            Ok(PreciseJson::S64(n.as_i64().unwrap()))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected s64 value".to_string(),
                        )),
                    },
                    "u64" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::U64(n.as_u64().unwrap()))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected u64 value".to_string(),
                        )),
                    },
                    "f32" => match value {
                        JsonValue::Number(n) if n.is_f64() => {
                            Ok(PreciseJson::F32(n.as_f64().unwrap() as f32))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected f32 value".to_string(),
                        )),
                    },
                    "f64" => match value {
                        JsonValue::Number(n) if n.is_f64() => {
                            Ok(PreciseJson::F64(n.as_f64().unwrap()))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected f64 value".to_string(),
                        )),
                    },
                    "chr" => match value {
                        JsonValue::String(s) if s.chars().count() == 1 => {
                            Ok(PreciseJson::Chr(s.chars().next().unwrap()))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected single character".to_string(),
                        )),
                    },
                    "str" => match value {
                        JsonValue::String(s) => Ok(PreciseJson::Str(s)),
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected string value".to_string(),
                        )),
                    },
                    "list" => match value {
                        JsonValue::Array(arr) => {
                            let elems: Result<Vec<PreciseJson>, _> =
                                arr.into_iter().map(PreciseJson::try_from).collect();
                            elems.map(PreciseJson::List)
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected list value".to_string(),
                        )),
                    },
                    "tuple" => match value {
                        JsonValue::Array(arr) => {
                            let elems: Result<Vec<PreciseJson>, _> =
                                arr.into_iter().map(PreciseJson::try_from).collect();
                            elems.map(PreciseJson::Tuple)
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
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
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected record value".to_string(),
                        )),
                    },
                    "variant" => match value {
                        JsonValue::Object(variant) => {
                            let case_name = variant
                                .get("case_name")
                                .and_then(|v| v.as_str())
                                .ok_or(PreciseJsonError::MissingField("case_name".to_string()))?
                                .to_string();
                            let case_value = variant
                                .get("case_value")
                                .ok_or(PreciseJsonError::MissingField("case_value".to_string()))
                                .and_then(|v| PreciseJson::try_from(v.clone()))?;
                            Ok(PreciseJson::Variant {
                                case_name,
                                case_value: Box::new(case_value),
                            })
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected variant value".to_string(),
                        )),
                    },
                    "enum" => match value {
                        JsonValue::Number(n) if n.is_u64() => {
                            Ok(PreciseJson::Enum(n.as_u64().unwrap() as u32))
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected enum value".to_string(),
                        )),
                    },
                    "flags" => match value {
                        JsonValue::Array(arr) => {
                            let flags: Result<Vec<bool>, _> = arr
                                .into_iter()
                                .map(|v| match v {
                                    JsonValue::Bool(b) => Ok(b),
                                    _ => Err(PreciseJsonError::InvalidValue(
                                        "Expected boolean value in flags".to_string(),
                                    )),
                                })
                                .collect();
                            flags.map(PreciseJson::Flags)
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
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
                                return Err(PreciseJsonError::InvalidValue(
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
                                _ => Err(PreciseJsonError::InvalidValue(
                                    "Expected result key to be 'Ok' or 'Err'".to_string(),
                                )),
                            }
                        }
                        _ => Err(PreciseJsonError::InvalidValue(
                            "Expected result key to be 'Ok' or 'Err'".to_string(),
                        )),
                    },
                    _ => Err(PreciseJsonError::InvalidValue(
                        "Expected result object".to_string(),
                    )),
                }
            }
            _ => Err(PreciseJsonError::InvalidValue(
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
                    .map(|(k, v)| golem_wasm_rpc::Value::from(v))
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
                Ok(precise_json) => golem_wasm_rpc::Value::Result(Result::Ok(Some(Box::new(
                    golem_wasm_rpc::Value::from(*precise_json),
                )))),
                Err(precise_json) => golem_wasm_rpc::Value::Result(Result::Err(Some(Box::new(
                    golem_wasm_rpc::Value::from(*precise_json),
                )))),
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
                "case_name": "SomeCase",
                "case_value": { "u32": 42 }
            }
        });
        let precise_json = PreciseJson::try_from(json_value).unwrap();
        assert_eq!(
            precise_json,
            PreciseJson::Variant {
                case_name: "SomeCase".to_string(),
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
