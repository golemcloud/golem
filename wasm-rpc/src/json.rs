// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Display;
use golem_wasm_ast::analysis::{AnalysedFunctionParameter, AnalysedFunctionResult};
use serde_json::{Number, Value as JsonValue};

use crate::{TypeAnnotatedValue, Value};

pub fn function_parameters(
    value: &JsonValue,
    expected_parameters: &[AnalysedFunctionParameter],
) -> Result<Vec<Value>, Vec<String>> {
    let typed_values = TypeAnnotatedValue::from_function_parameters(value, expected_parameters)?;

    let mut errors = vec![];
    let mut values = vec![];

    for typed_value in typed_values {
        match Value::try_from(typed_value) {
            Ok(value) => {
                values.push(value);
            }
            Err(err) => {
                errors.push(err);
            }
        }
    }

    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

pub fn function_result(
    values: Vec<Value>,
    expected_types: &[AnalysedFunctionResult],
) -> Result<JsonValue, Vec<String>> {
    TypeAnnotatedValue::from_function_results(values, expected_types)
        .map(|result| Json::from(result).0)
}

pub struct Json(pub serde_json::Value);

impl Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<TypeAnnotatedValue> for Json {
    fn from(value: TypeAnnotatedValue) -> Self {
        match value {
            TypeAnnotatedValue::Bool(bool) => Json(serde_json::Value::Bool(bool)),
            TypeAnnotatedValue::Flags { typ: _, values } => Json(JsonValue::Array(
                values.into_iter().map(JsonValue::String).collect(),
            )),
            TypeAnnotatedValue::S8(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::U8(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::S16(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::U16(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::S32(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::U32(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::S64(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::U64(value) => Json(JsonValue::Number(Number::from(value))),
            TypeAnnotatedValue::F32(value) => {
                Json(JsonValue::Number(Number::from_f64(value as f64).unwrap()))
            }
            TypeAnnotatedValue::F64(value) => {
                Json(JsonValue::Number(Number::from_f64(value).unwrap()))
            }
            TypeAnnotatedValue::Chr(value) => Json(JsonValue::Number(Number::from(value as u32))),
            TypeAnnotatedValue::Str(value) => Json(JsonValue::String(value)),
            TypeAnnotatedValue::Enum { typ: _, value } => Json(JsonValue::String(value)),
            TypeAnnotatedValue::Option { typ: _, value } => match value {
                Some(value) => Json::from(*value),
                None => Json(JsonValue::Null),
            },
            TypeAnnotatedValue::Tuple { typ: _, value } => {
                let values: Vec<serde_json::Value> =
                    value.into_iter().map(Json::from).map(|v| v.0).collect();
                Json(JsonValue::Array(values))
            }
            TypeAnnotatedValue::List { typ: _, values } => {
                let values: Vec<serde_json::Value> =
                    values.into_iter().map(Json::from).map(|v| v.0).collect();
                Json(JsonValue::Array(values))
            }

            TypeAnnotatedValue::Record { typ: _, value } => {
                let mut map = serde_json::Map::new();
                for (key, value) in value {
                    map.insert(key, Json::from(value).0);
                }
                Json(JsonValue::Object(map))
            }

            TypeAnnotatedValue::Variant {
                typ: _,
                case_name,
                case_value,
            } => {
                let mut map = serde_json::Map::new();
                map.insert(
                    case_name,
                    case_value
                        .map(|x| Json::from(*x))
                        .map(|v| v.0)
                        .unwrap_or(JsonValue::Null),
                );
                Json(JsonValue::Object(map))
            }

            TypeAnnotatedValue::Result {
                ok: _,
                error: _,
                value,
            } => {
                let mut map = serde_json::Map::new();
                match value {
                    Ok(ok_value) => {
                        map.insert(
                            "ok".to_string(),
                            ok_value
                                .map(|x| Json::from(*x))
                                .map(|v| v.0)
                                .unwrap_or(JsonValue::Null),
                        );
                    }
                    Err(err_value) => {
                        map.insert(
                            "err".to_string(),
                            err_value
                                .map(|x| Json::from(*x))
                                .map(|v| v.0)
                                .unwrap_or(JsonValue::Null),
                        );
                    }
                }
                Json(JsonValue::Object(map))
            }

            TypeAnnotatedValue::Handle {
                id: _,
                resource_mode: _,
                uri,
                resource_id,
            } => Json(JsonValue::String(format!("{}/{}", uri.value, resource_id))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::json::Json;
    use crate::{TypeAnnotatedValue, Value};
    use golem_wasm_ast::analysis::AnalysedType;
    use proptest::prelude::*;
    use serde_json::{Number, Value as JsonValue};
    use std::collections::HashSet;

    fn validate_function_result(
        val: Value,
        expected_type: &AnalysedType,
    ) -> Result<JsonValue, Vec<String>> {
        TypeAnnotatedValue::from_value(&val, expected_type).map(|result| Json::from(result).0)
    }

    fn validate_function_parameter(
        json: &JsonValue,
        expected_type: &AnalysedType,
    ) -> Result<Value, Vec<String>> {
        match TypeAnnotatedValue::from_json_value(json, expected_type) {
            Ok(result) => match Value::try_from(result) {
                Ok(value) => Ok(value),
                Err(err) => Err(vec![err]),
            },
            Err(err) => Err(err),
        }
    }

    proptest! {
        #[test]
        fn test_u8_param(value: u8) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::U8);
            prop_assert_eq!(result, Ok(Value::U8(value)));
        }

        #[test]
        fn test_u16_param(value: u16) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::U16);
            prop_assert_eq!(result, Ok(Value::U16(value)));
        }

        #[test]
        fn test_u32_param(value: u32) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::U32);
            prop_assert_eq!(result, Ok(Value::U32(value)));
        }

        #[test]
        fn test_u64_param(value: u64) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::U64);
            prop_assert_eq!(result, Ok(Value::U64(value)));
        }

        #[test]
        fn test_s8_param(value: i8) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::S8);
            prop_assert_eq!(result, Ok(Value::S8(value)));
        }

        #[test]
        fn test_s16_param(value: i16) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::S16);
            prop_assert_eq!(result, Ok(Value::S16(value)));
        }

        #[test]
        fn test_s32_param(value: i32) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::S32);
            prop_assert_eq!(result, Ok(Value::S32(value)));
        }

        #[test]
        fn test_s64_param(value: i64) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &AnalysedType::S64);
            prop_assert_eq!(result, Ok(Value::S64(value)));
        }

        #[test]
        fn test_f32_param(value: f32) {
            let json = JsonValue::Number(Number::from_f64(value as f64).unwrap());
            let result = validate_function_parameter(&json, &AnalysedType::F32);
            prop_assert_eq!(result, Ok(Value::F32(value)));
        }

        #[test]
        fn test_f64_param(value: f64) {
            let json = JsonValue::Number(Number::from_f64(value).unwrap());
            let result = validate_function_parameter(&json, &AnalysedType::F64);
            prop_assert_eq!(result, Ok(Value::F64(value)));
        }

        #[test]
        fn test_char_param(value: char) {
            let json = JsonValue::Number(Number::from(value as u32));
            let result = validate_function_parameter(&json, &AnalysedType::Chr);
            prop_assert_eq!(result, Ok(Value::Char(value)));
        }

        #[test]
        fn test_string_param(value: String) {
            let json = JsonValue::String(value.clone());
            let result = validate_function_parameter(&json, &AnalysedType::Str);
            prop_assert_eq!(result, Ok(Value::String(value)));
        }

        #[test]
        fn test_list_u8_param(value: Vec<u8>) {
            let json = JsonValue::Array(value.iter().map(|v| JsonValue::Number(Number::from(*v))).collect());
            let result = validate_function_parameter(&json, &AnalysedType::List(Box::new(AnalysedType::U8)));
            prop_assert_eq!(result, Ok(Value::List(value.into_iter().map(Value::U8).collect())));
        }

        #[test]
        fn test_list_list_u64_param(value: Vec<Vec<u64>>) {
            let json = JsonValue::Array(value.iter().map(|v| JsonValue::Array(v.iter().map(|n| JsonValue::Number(Number::from(*n))).collect())).collect());
            let result = validate_function_parameter(&json, &AnalysedType::List(Box::new(AnalysedType::List(Box::new(AnalysedType::U64)))));
            prop_assert_eq!(result, Ok(Value::List(value.into_iter().map(|v| Value::List(v.into_iter().map(Value::U64).collect())).collect())));
        }

        #[test]
        fn test_tuple_int_char_string_param(value: (i32, char, String)) {
            let json = JsonValue::Array(
                vec![
                    JsonValue::Number(Number::from(value.0)),
                    JsonValue::Number(Number::from(value.1 as u32)),
                    JsonValue::String(value.2.clone()),
                ]);
            let result = validate_function_parameter(&json, &AnalysedType::Tuple(vec![
                AnalysedType::S32,
                AnalysedType::Chr,
                AnalysedType::Str,
            ]));
            prop_assert_eq!(result, Ok(Value::Tuple(
                vec![
                    Value::S32(value.0),
                    Value::Char(value.1),
                    Value::String(value.2),
                ])));
        }

        #[test]
        fn test_record_bool_fields_param(value in
            any::<Vec<(String, bool)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<std::collections::HashSet<_>>().len() == pairs.len())
        ) {
            let json = JsonValue::Object(
                value.iter().map(|(k, v)| (k.clone(), JsonValue::Bool(*v))).collect());
            let result = validate_function_parameter(&json, &AnalysedType::Record(
                value.iter().map(|(k, _)| (k.clone(), AnalysedType::Bool)).collect()));
            prop_assert_eq!(result, Ok(Value::Record(
                value.iter().map(|(_, v)| Value::Bool(*v)).collect())));
        }

        #[test]
        fn test_flags_param(value in
            any::<Vec<(String, bool)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<std::collections::HashSet<_>>().len() == pairs.len())
            ) {
            let enabled: Vec<String> = value.iter().filter(|(_, v)| *v).map(|(k, _)| k.clone()).collect();
            let json = JsonValue::Array(enabled.iter().map(|v| JsonValue::String(v.clone())).collect());
            let result = validate_function_parameter(&json, &AnalysedType::Flags(
                value.iter().map(|(k, _)| k.clone()).collect()));
            prop_assert_eq!(result, Ok(Value::Flags(
                value.iter().map(|(_, v)| *v).collect())
            ));
        }

        #[test]
        fn test_enum_param((names, idx) in (any::<HashSet<String>>().prop_filter("Name list is non empty", |names| !names.is_empty()), any::<usize>())) {
            let names: Vec<String> = names.into_iter().collect();
            let idx = idx % names.len();
            let json = JsonValue::String(names[idx].clone());
            let result = validate_function_parameter(&json, &AnalysedType::Enum(names.into_iter().collect()));
            prop_assert_eq!(result, Ok(Value::Enum(idx as u32)));
        }

        #[test]
        fn test_option_string_param(value: Option<String>) {
            let json = match &value {
                Some(v) => JsonValue::String(v.clone()),
                None => JsonValue::Null,
            };
            let result = validate_function_parameter(&json, &AnalysedType::Option(Box::new(AnalysedType::Str)));
            prop_assert_eq!(result, Ok(Value::Option(value.map(|v| Box::new(Value::String(v))))));
        }

        #[test]
        fn test_result_option_s32_string_param(value: Result<Option<i32>, String>) {
            let json = match &value {
                Ok(None) => JsonValue::Object(vec![("ok".to_string(), JsonValue::Null)].into_iter().collect()),
                Ok(Some(v)) => JsonValue::Object(vec![("ok".to_string(), JsonValue::Number(Number::from(*v)))].into_iter().collect()),
                Err(e) => JsonValue::Object(vec![("err".to_string(), JsonValue::String(e.clone()))].into_iter().collect()),
            };
            let result = validate_function_parameter(&json, &AnalysedType::Result {
                ok: Some(Box::new(AnalysedType::Option(Box::new(AnalysedType::S32)))),
                error: Some(Box::new(AnalysedType::Str)),
            });
            prop_assert_eq!(result, Ok(Value::Result(
                match value {
                    Ok(None) => Ok(Some(Box::new(Value::Option(None)))),
                    Ok(Some(v)) => Ok(Some(Box::new(Value::Option(Some(Box::new(Value::S32(v))))))),
                    Err(e) => Err(Some(Box::new(Value::String(e)))),
                }
            )));
        }

        #[test]
        fn test_variant_u8tuple_string_param(first: (u32, u32), second: String, discriminator in 0i32..1i32) {
            let json = match discriminator {
                0 => JsonValue::Object(vec![
                    ("first".to_string(), JsonValue::Array(vec![
                        JsonValue::Number(Number::from(first.0)),
                        JsonValue::Number(Number::from(first.1)),
                    ])),
                ].into_iter().collect()),
                1 => JsonValue::Object(vec![
                    ("second".to_string(), JsonValue::String(second.clone())),
                ].into_iter().collect()),
                _ => panic!("Invalid discriminator value"),
            };
            let result = validate_function_parameter(&json, &AnalysedType::Variant(vec![
                ("first".to_string(), Some(AnalysedType::Tuple(vec![AnalysedType::U32, AnalysedType::U32]))),
                ("second".to_string(), Some(AnalysedType::Str)),
            ]));
            prop_assert_eq!(result, Ok(Value::Variant {
                case_idx: discriminator as u32,
                case_value: match discriminator {
                    0 => Some(Box::new(Value::Tuple(vec![Value::U32(first.0), Value::U32(first.1)]))),
                    1 => Some(Box::new(Value::String(second))),
                    _ => panic!("Invalid discriminator value"),
                }
            }));
        }

        #[test]
        fn test_u8_result(value: u8) {
            let result = Value::U8(value);
            let expected_type = AnalysedType::U8;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_u16_result(value: u16) {
            let result = Value::U16(value);
            let expected_type = AnalysedType::U16;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_u32_result(value: u32) {
            let result = Value::U32(value);
            let expected_type = AnalysedType::U32;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_u64_result(value: u64) {
            let result = Value::U64(value);
            let expected_type = AnalysedType::U64;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s8_result(value: i8) {
            let result = Value::S8(value);
            let expected_type = AnalysedType::S8;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s16_result(value: i16) {
            let result = Value::S16(value);
            let expected_type = AnalysedType::S16;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s32_result(value: i32) {
            let result = Value::S32(value);
            let expected_type = AnalysedType::S32;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s64_result(value: i64) {
            let result = Value::S64(value);
            let expected_type = AnalysedType::S64;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_f32_result(value: f32) {
            let result = Value::F32(value);
            let expected_type = AnalysedType::F32;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from_f64(value as f64).unwrap())));
        }

        #[test]
        fn test_f64_result(value: f64) {
            let result = Value::F64(value);
            let expected_type = AnalysedType::F64;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from_f64(value).unwrap())));
        }

        #[test]
        fn test_char_result(value: char) {
            let result = Value::Char(value);
            let expected_type = AnalysedType::Chr;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value as u32))));
        }

        #[test]
        fn test_string_result(value: String) {
            let result = Value::String(value.clone());
            let expected_type = AnalysedType::Str;
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::String(value)));
        }

        #[test]
        fn test_list_i32_result(value: Vec<i32>) {
            let result = Value::List(value.iter().map(|v| Value::S32(*v)).collect());
            let expected_type = AnalysedType::List(Box::new(AnalysedType::S32));
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Array(value.into_iter().map(|v| JsonValue::Number(Number::from(v))).collect())));
        }

        #[test]
        fn test_tuple_string_bool_result(value: (String, bool)) {
            let result = Value::Tuple(vec![Value::String(value.0.clone()), Value::Bool(value.1)]);
            let expected_type = AnalysedType::Tuple(vec![AnalysedType::Str, AnalysedType::Bool]);
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Array(vec![JsonValue::String(value.0), JsonValue::Bool(value.1)])));
        }

        #[test]
        fn test_record_list_u8_fields(value in any::<Vec<(String, Vec<u8>)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<std::collections::HashSet<_>>().len() == pairs.len())
        ) {
            let result = Value::Record(
                value.iter().map(|(_, v)| Value::List(v.iter().map(|n| Value::U8(*n)).collect())).collect());
            let expected_type = AnalysedType::Record(
                value.iter().map(|(k, _)| (k.clone(), AnalysedType::List(Box::new(AnalysedType::U8)))).collect());
            let json = validate_function_result(result, &expected_type);
            let expected_json = JsonValue::Object(
                value.iter().map(|(k, v)| (k.clone(), JsonValue::Array(v.iter().map(|n| JsonValue::Number(Number::from(*n))).collect()))).collect());
            prop_assert_eq!(json, Ok(expected_json));
        }

        #[test]
        fn test_flags_result(pairs in
            any::<Vec<(String, bool)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<std::collections::HashSet<_>>().len() == pairs.len())
            ) {
            let enabled: Vec<String> = pairs.iter().filter(|(_, v)| *v).map(|(k, _)| k.clone()).collect();
            let value = Value::Flags(pairs.iter().map(|(_, v)| *v).collect());
            let result = validate_function_result(value, &AnalysedType::Flags(
                pairs.iter().map(|(k, _)| k.clone()).collect()));
            prop_assert_eq!(result, Ok(
                JsonValue::Array(enabled.iter().map(|v| JsonValue::String(v.clone())).collect())
            ));
        }

        #[test]
        fn test_enum_result((names, idx) in (any::<HashSet<String>>().prop_filter("Name list is non empty", |names| !names.is_empty()), any::<usize>())) {
            let names: Vec<String> = names.into_iter().collect();
            let idx = idx % names.len();
            let value = Value::Enum(idx as u32);
            let result = validate_function_result(value, &AnalysedType::Enum(names.clone()));
            prop_assert_eq!(result, Ok(JsonValue::String(names[idx].clone())));
        }

        #[test]
        fn test_option_string_result(opt: Option<String>) {
            let value = Value::Option(opt.clone().map(|v| Box::new(Value::String(v))));
            let result = validate_function_result(value, &AnalysedType::Option(Box::new(AnalysedType::Str)));
            let json = match opt {
                Some(str) => Ok(JsonValue::String(str)),
                None => Ok(JsonValue::Null),
            };
            prop_assert_eq!(result, json);
        }

        #[test]
        fn test_variant_u8tuple_string_result(first: (u32, u32), second: String, discriminator in 0i32..1i32) {
            let value = Value::Variant {
                case_idx: discriminator as u32,
                case_value: match discriminator {
                    0 => Some(Box::new(Value::Tuple(vec![Value::U32(first.0), Value::U32(first.1)]))),
                    1 => Some(Box::new(Value::String(second.clone()))),
                    _ => panic!("Invalid discriminator value"),
                }
            };
            let result = validate_function_result(value, &AnalysedType::Variant(vec![
                ("first".to_string(), Some(AnalysedType::Tuple(vec![AnalysedType::U32, AnalysedType::U32]))),
                ("second".to_string(), Some(AnalysedType::Str)),
            ]));
            let json = match discriminator {
                0 => JsonValue::Object(vec![
                    ("first".to_string(), JsonValue::Array(vec![
                        JsonValue::Number(Number::from(first.0)),
                        JsonValue::Number(Number::from(first.1)),
                    ])),
                ].into_iter().collect()),
                1 => JsonValue::Object(vec![
                    ("second".to_string(), JsonValue::String(second)),
                ].into_iter().collect()),
                _ => panic!("Invalid discriminator value"),
            };
            prop_assert_eq!(result, Ok(json));
        }
    }

    #[test]
    fn json_null_works_as_none() {
        let json = JsonValue::Null;
        let result =
            validate_function_parameter(&json, &AnalysedType::Option(Box::new(AnalysedType::Str)));
        assert_eq!(result, Ok(Value::Option(None)));
    }

    #[test]
    fn missing_field_works_as_none() {
        let json = JsonValue::Object(
            vec![("x".to_string(), JsonValue::String("a".to_string()))]
                .into_iter()
                .collect(),
        );
        let result = validate_function_parameter(
            &json,
            &AnalysedType::Record(vec![
                ("x".to_string(), AnalysedType::Str),
                (
                    "y".to_string(),
                    AnalysedType::Option(Box::new(AnalysedType::Str)),
                ),
            ]),
        );
        assert_eq!(
            result,
            Ok(Value::Record(vec![
                Value::String("a".to_string()),
                Value::Option(None),
            ]))
        );
    }
}
