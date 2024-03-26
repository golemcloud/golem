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

use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};
use golem_wasm_ast::analysis::{AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedType};
use serde_json::{Number, Value as JsonValue};
use std::collections::HashMap;
use std::str::FromStr;

use crate::{TypeAnnotatedValue, TypeAnnotatedValueResult, Uri, Value};

pub fn function_parameters(
    value: &JsonValue,
    expected_parameters: &[AnalysedFunctionParameter],
) -> Result<Vec<Value>, Vec<String>> {
    let parameters = value
        .as_array()
        .ok_or(vec!["Expecting an array for fn_params".to_string()])?;

    let mut results = vec![];
    let mut errors = vec![];

    if parameters.len() == expected_parameters.len() {
        for (json, fp) in parameters.iter().zip(expected_parameters.iter()) {
            match validate_function_parameter(json, &fp.typ) {
                Ok(result) => results.push(result),
                Err(err) => errors.extend(err),
            }
        }

        if errors.is_empty() {
            Ok(results)
        } else {
            Err(errors)
        }
    } else {
        Err(vec![format!(
            "Unexpected number of parameters (got {}, expected: {})",
            parameters.len(),
            expected_parameters.len()
        )])
    }
}

pub fn function_result(
    values: Vec<Value>,
    expected_types: &[AnalysedFunctionResult],
) -> Result<JsonValue, Vec<String>> {
    TypeAnnotatedValueResult::from_values(values, expected_types).map(|result| match result {
        TypeAnnotatedValueResult::WithoutNames(values) => JsonValue::Array(
            values
                .into_iter()
                .map(|v| JsonFunctionResult::from(v).0)
                .collect(),
        ),
        TypeAnnotatedValueResult::WithNames(values) => {
            let mut map = serde_json::Map::new();
            for (name, value) in values {
                map.insert(name, JsonFunctionResult::from(value).0);
            }
            JsonValue::Object(map)
        }
    })
}

fn validate_function_parameter(
    input_json: &JsonValue,
    expected_type: &AnalysedType,
) -> Result<Value, Vec<String>> {
    match expected_type {
        AnalysedType::Bool => get_bool(input_json),
        AnalysedType::S8 => get_s8(input_json),
        AnalysedType::U8 => get_u8(input_json),
        AnalysedType::S16 => get_s16(input_json),
        AnalysedType::U16 => get_u16(input_json),
        AnalysedType::S32 => get_s32(input_json),
        AnalysedType::U32 => get_u32(input_json),
        AnalysedType::S64 => get_s64(input_json),
        AnalysedType::U64 => get_u64(input_json).map(Value::U64),
        AnalysedType::F64 => {
            let num = bigdecimal(input_json)?;
            let value = Value::F64(
                num.to_string()
                    .parse()
                    .map_err(|err| vec![format!("Failed to parse f64: {}", err)])?,
            );
            Ok(value)
        }
        AnalysedType::F32 => get_f32(input_json),
        AnalysedType::Chr => get_char(input_json).map(Value::Char),
        AnalysedType::Str => get_string(input_json).map(Value::String),
        AnalysedType::Enum(names) => get_enum(input_json, names).map(Value::Enum),
        AnalysedType::Flags(names) => get_flag(input_json, names).map(Value::Flags),
        AnalysedType::List(elem) => get_list(input_json, elem).map(Value::List),
        AnalysedType::Option(elem) => get_option(input_json, elem).map(Value::Option),
        AnalysedType::Result { ok, error } => get_result(input_json, ok, error).map(Value::Result),
        AnalysedType::Record(fields) => get_record(input_json, fields).map(Value::Record),
        AnalysedType::Variant(cases) => {
            get_variant(input_json, cases).map(|result| Value::Variant {
                case_idx: result.0,
                case_value: result.1,
            })
        }
        AnalysedType::Tuple(elems) => get_tuple(input_json, elems).map(Value::Tuple),
        AnalysedType::Resource { .. } => get_handle(input_json),
    }
}

fn get_bool(json: &JsonValue) -> Result<Value, Vec<String>> {
    match json {
        JsonValue::Bool(bool_val) => Ok(Value::Bool(*bool_val)),
        _ => {
            let type_description = type_description(json);
            Err(vec![format!(
                "Expected function parameter type is Boolean. But found {}",
                type_description
            )])
        }
    }
}

fn get_s8(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i8(i8::MIN).expect("Failed to convert i8::MIN to BigDecimal"),
        BigDecimal::from_i8(i8::MAX).expect("Failed to convert i8::MAX to BigDecimal"),
    )
    .map(|num| Value::S8(num.to_i8().expect("Failed to convert BigDecimal to i8")))
}

fn get_u8(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u8(u8::MIN).expect("Failed to convert u8::MIN to BigDecimal"),
        BigDecimal::from_u8(u8::MAX).expect("Failed to convert u8::MAX to BigDecimal"),
    )
    .map(|num| Value::U8(num.to_u8().expect("Failed to convert BigDecimal to u8")))
}

fn get_s16(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i16(i16::MIN).expect("Failed to convert i16::MIN to BigDecimal"),
        BigDecimal::from_i16(i16::MAX).expect("Failed to convert i16::MAX to BigDecimal"),
    )
    .map(|num| Value::S16(num.to_i16().expect("Failed to convert BigDecimal to i16")))
}

fn get_u16(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u16(u16::MIN).expect("Failed to convert u16::MIN to BigDecimal"),
        BigDecimal::from_u16(u16::MAX).expect("Failed to convert u16::MAX to BigDecimal"),
    )
    .map(|num| Value::U16(num.to_u16().expect("Failed to convert BigDecimal to u16")))
}

fn get_s32(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i32(i32::MIN).expect("Failed to convert i32::MIN to BigDecimal"),
        BigDecimal::from_i32(i32::MAX).expect("Failed to convert i32::MAX to BigDecimal"),
    )
    .map(|num| Value::S32(num.to_i32().expect("Failed to convert BigDecimal to i32")))
}

fn get_u32(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u32(u32::MIN).expect("Failed to convert u32::MIN to BigDecimal"),
        BigDecimal::from_u32(u32::MAX).expect("Failed to convert u32::MAX to BigDecimal"),
    )
    .map(|num| Value::U32(num.to_u32().expect("Failed to convert BigDecimal to u32")))
}

fn get_s64(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i64(i64::MIN).expect("Failed to convert i64::MIN to BigDecimal"),
        BigDecimal::from_i64(i64::MAX).expect("Failed to convert i64::MAX to BigDecimal"),
    )
    .map(|num| Value::S64(num.to_i64().expect("Failed to convert BigDecimal to i64")))
}

fn get_f32(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_f32(f32::MIN).expect("Failed to convert f32::MIN to BigDecimal"),
        BigDecimal::from_f32(f32::MAX).expect("Failed to convert f32::MAX to BigDecimal"),
    )
    .map(|num| Value::F32(num.to_f32().expect("Failed to convert BigDecimal to f32")))
}

fn ensure_range(
    value: &JsonValue,
    min: BigDecimal,
    max: BigDecimal,
) -> Result<BigDecimal, Vec<String>> {
    let num = bigdecimal(value)?;
    if num >= min && num <= max {
        Ok(num)
    } else {
        Err(vec![format!(
            "value {} is not within the range of {} to {}",
            value, min, max
        )])
    }
}

fn get_u64(value: &JsonValue) -> Result<u64, Vec<String>> {
    match value {
        JsonValue::Number(num) => {
            if let Some(u64) = num.as_u64() {
                Ok(u64)
            } else {
                Err(vec![format!("Cannot convert {} to u64", num)])
            }
        }
        _ => {
            let type_description = type_description(value);
            Err(vec![format!(
                "Expected function parameter type is u64. But found {}",
                type_description
            )])
        }
    }
}

fn bigdecimal(value: &JsonValue) -> Result<BigDecimal, Vec<String>> {
    match value {
        JsonValue::Number(num) => {
            if let Ok(f64) = BigDecimal::from_str(num.to_string().as_str()) {
                Ok(f64)
            } else {
                Err(vec![format!("Cannot convert {} to f64", num)])
            }
        }
        _ => {
            let type_description = type_description(value);
            Err(vec![format!(
                "Expected function parameter type is BigDecimal. But found {}",
                type_description
            )])
        }
    }
}

fn get_char(json: &JsonValue) -> Result<char, Vec<String>> {
    if let Some(num_u64) = json.as_u64() {
        if num_u64 > u32::MAX as u64 {
            Err(vec![format!(
                "The value {} is too large to be converted to a char",
                num_u64
            )])
        } else {
            char::from_u32(num_u64 as u32).ok_or(vec![format!(
                "The value {} is not a valid unicode character",
                num_u64
            )])
        }
    } else {
        let type_description = type_description(json);

        Err(vec![format!(
            "Expected function parameter type is Char. But found {}",
            type_description
        )])
    }
}

fn get_string(input_json: &JsonValue) -> Result<String, Vec<String>> {
    if let Some(str_value) = input_json.as_str() {
        // If the JSON value is a string, return it
        Ok(str_value.to_string())
    } else {
        // If the JSON value is not a string, return an error with type information
        let type_description = type_description(input_json);
        Err(vec![format!(
            "Expected function parameter type is String. But found {}",
            type_description
        )])
    }
}

fn type_description(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "Null",
        JsonValue::Bool(_) => "Boolean",
        JsonValue::Number(_) => "Number",
        JsonValue::String(_) => "String",
        JsonValue::Array(_) => "Array",
        JsonValue::Object(_) => "Object",
    }
}

#[allow(clippy::type_complexity)]
fn get_result(
    input_json: &JsonValue,
    ok_type: &Option<Box<AnalysedType>>,
    err_type: &Option<Box<AnalysedType>>,
) -> Result<Result<Option<Box<Value>>, Option<Box<Value>>>, Vec<String>> {
    fn validate(
        typ: &Option<Box<AnalysedType>>,
        input_json: &JsonValue,
    ) -> Result<Option<Box<Value>>, Vec<String>> {
        if let Some(typ) = typ {
            validate_function_parameter(input_json, typ).map(|v| Some(Box::new(v)))
        } else if input_json.is_null() {
            Ok(None)
        } else {
            Err(vec![
                "The type of ok is absent, but some JSON value was provided".to_string(),
            ])
        }
    }

    match input_json.get("ok") {
        Some(value) => Ok(Ok(validate(ok_type, value)?)),
        None => match input_json.get("err") {
            Some(value) => Ok(Err(validate(err_type, value)?)),
            None => Err(vec![
                "Failed to retrieve either ok value or err value".to_string()
            ]),
        },
    }
}

fn get_option(
    input_json: &JsonValue,
    tpe: &AnalysedType,
) -> Result<Option<Box<Value>>, Vec<String>> {
    match input_json.as_null() {
        Some(_) => Ok(None),

        None => validate_function_parameter(input_json, tpe).map(|result| Some(Box::new(result))),
    }
}

fn get_list(input_json: &JsonValue, tpe: &AnalysedType) -> Result<Vec<Value>, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<Value> = vec![];

    for json in json_array {
        match validate_function_parameter(json, tpe) {
            Ok(result) => vals.push(result),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(vals)
    } else {
        Err(errors)
    }
}

fn get_tuple(input_json: &JsonValue, types: &[AnalysedType]) -> Result<Vec<Value>, Vec<String>> {
    let json_array = input_json.as_array().ok_or(vec![format!(
        "Input {} is not an array representing tuple",
        input_json
    )])?;

    if json_array.len() != types.len() {
        return Err(vec![format!(
            "The length of types in template is not equal to the length of tuple (array) in  {}",
            input_json,
        )]);
    }

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<Value> = vec![];

    for (json, tpe) in json_array.iter().zip(types.iter()) {
        match validate_function_parameter(json, tpe) {
            Ok(result) => vals.push(result),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(vals)
    } else {
        Err(errors)
    }
}

fn get_record(
    input_json: &JsonValue,
    name_type_pairs: &[(String, AnalysedType)],
) -> Result<Vec<Value>, Vec<String>> {
    let json_map = input_json.as_object().ok_or(vec![format!(
        "The input {} is not a json object",
        input_json
    )])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<Value> = vec![];

    for (name, tpe) in name_type_pairs {
        if let Some(json_value) = json_map.get(name) {
            match validate_function_parameter(json_value, tpe) {
                Ok(result) => vals.push(result),
                Err(value_errors) => errors.extend(
                    value_errors
                        .iter()
                        .map(|err| format!("Invalid value for the key {}. Error: {}", name, err))
                        .collect::<Vec<_>>(),
                ),
            }
        } else {
            match tpe {
                AnalysedType::Option(_) => vals.push(Value::Option(None)),
                _ => errors.push(format!("Key '{}' not found in json_map", name)),
            }
        }
    }

    if errors.is_empty() {
        Ok(vals)
    } else {
        Err(errors)
    }
}

fn get_enum(input_json: &JsonValue, names: &[String]) -> Result<u32, Vec<String>> {
    let input_enum_value = input_json
        .as_str()
        .ok_or(vec![format!("Input {} is not string", input_json)])?;

    let mut discriminant: Option<i32> = None;

    for (pos, name) in names.iter().enumerate() {
        if input_enum_value == name {
            discriminant = Some(pos as i32)
        }
    }

    if let Some(d) = discriminant {
        Ok(d as u32)
    } else {
        Err(vec![format!(
            "Invalid input {}. Valid values are {}",
            input_enum_value,
            names.join(",")
        )])
    }
}

fn get_flag(input_json: &JsonValue, names: &[String]) -> Result<Vec<bool>, Vec<String>> {
    let input_flag_values = input_json.as_array().ok_or(vec![format!(
        "Input {} is not an array to be parsed as flags",
        input_json
    )])?;

    let mut discriminant_map: HashMap<&str, usize> = HashMap::new();

    for (pos, name) in names.iter().enumerate() {
        discriminant_map.insert(name.as_str(), pos);
    }

    let mut result: Vec<bool> = vec![false; names.len()];

    for i in input_flag_values {
        let json_str = i
            .as_str()
            .ok_or(vec![format!("{} is not a valid string", i)])?;
        if let Some(d) = discriminant_map.get(json_str) {
            result[*d] = true;
        } else {
            return Err(vec![format!(
                "Invalid input {}. It should be one of {}",
                json_str,
                names.join(",")
            )]);
        }
    }

    Ok(result)
}

fn get_variant(
    input_json: &JsonValue,
    types: &[(String, Option<AnalysedType>)],
) -> Result<(u32, Option<Box<Value>>), Vec<String>> {
    let mut possible_mapping_indexed: HashMap<&String, (usize, &Option<AnalysedType>)> =
        HashMap::new();

    for (pos, (name, optional_type)) in types.iter().enumerate() {
        possible_mapping_indexed.insert(name, (pos, optional_type));
    }

    let json_obj = input_json
        .as_object()
        .ok_or(vec![format!("Input {} is not an object", input_json)])?;

    let (key, json) = if json_obj.is_empty() {
        Err(vec!["Zero variants in in the input".to_string()])
    } else {
        Ok(json_obj.iter().next().unwrap())
    }?;

    match possible_mapping_indexed.get(key) {
        Some((index, Some(tpe))) => validate_function_parameter(json, tpe)
            .map(|result| (*index as u32, Some(Box::new(result)))),
        Some((index, None)) if json.is_null() => Ok((*index as u32, None)),
        Some((_, None)) => Err(vec![format!("Unit variant {key} has non-null JSON value")]),
        None => Err(vec![format!("Unknown key {key} in the variant")]),
    }
}

fn get_handle(value: &JsonValue) -> Result<Value, Vec<String>> {
    match value.as_str() {
        Some(str) => {
            // not assuming much about the url format, just checking it ends with a /<resource-id-u64>
            let parts: Vec<&str> = str.split('/').collect();
            if parts.len() >= 2 {
                match u64::from_str(parts[parts.len() - 1]) {
                    Ok(resource_id) => {
                        let uri = parts[0..(parts.len() - 1)].join("/");
                        Ok(Value::Handle { uri: Uri { value: uri }, resource_id })
                    }
                    Err(err) => {
                        Err(vec![format!("Failed to parse resource-id section of the handle value: {}", err)])
                    }
                }
            } else {
                Err(vec![format!(
                    "Expected function parameter type is Handle, represented by a worker-url/resource-id string. But found {}",
                    str
                )])
            }
        }
        None => Err(vec![format!(
            "Expected function parameter type is Handle, represented by a worker-url/resource-id string. But found {}",
            type_description(value)
        )]),
    }
}

pub struct JsonFunctionResult(pub serde_json::Value);

impl From<TypeAnnotatedValue> for JsonFunctionResult {
    fn from(value: TypeAnnotatedValue) -> Self {
        match value {
            TypeAnnotatedValue::Bool(bool) => JsonFunctionResult(serde_json::Value::Bool(bool)),
            TypeAnnotatedValue::Flags(flag_value) => JsonFunctionResult(JsonValue::Array(
                flag_value
                    .value
                    .into_iter()
                    .map(JsonValue::String)
                    .collect(),
            )),
            TypeAnnotatedValue::S8(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::U8(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::S16(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::U16(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::S32(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::U32(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::S64(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::U64(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value)))
            }
            TypeAnnotatedValue::F32(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from_f64(value as f64).unwrap()))
            }
            TypeAnnotatedValue::F64(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from_f64(value).unwrap()))
            }
            TypeAnnotatedValue::Chr(value) => {
                JsonFunctionResult(JsonValue::Number(Number::from(value as u32)))
            }
            TypeAnnotatedValue::Str(value) => JsonFunctionResult(JsonValue::String(value)),
            TypeAnnotatedValue::Enum(value) => JsonFunctionResult(JsonValue::String(value.value)),
            TypeAnnotatedValue::Option(value) => match value.value {
                Some(value) => JsonFunctionResult::from(*value),
                None => JsonFunctionResult(JsonValue::Null),
            },
            TypeAnnotatedValue::Tuple(values) => {
                let values: Vec<serde_json::Value> = values
                    .value
                    .into_iter()
                    .map(JsonFunctionResult::from)
                    .map(|v| v.0)
                    .collect();
                JsonFunctionResult(JsonValue::Array(values))
            }
            TypeAnnotatedValue::List(value) => {
                let values: Vec<serde_json::Value> = value
                    .values
                    .into_iter()
                    .map(JsonFunctionResult::from)
                    .map(|v| v.0)
                    .collect();
                JsonFunctionResult(JsonValue::Array(values))
            }

            TypeAnnotatedValue::Record(record) => {
                let mut map = serde_json::Map::new();
                for (key, value) in record.value {
                    map.insert(key, JsonFunctionResult::from(value).0);
                }
                JsonFunctionResult(JsonValue::Object(map))
            }

            TypeAnnotatedValue::Variant(variant) => {
                let mut map = serde_json::Map::new();
                map.insert(
                    variant.case_name,
                    variant
                        .case_value
                        .map(|x| JsonFunctionResult::from(*x))
                        .map(|v| v.0)
                        .unwrap_or(JsonValue::Null),
                );
                JsonFunctionResult(JsonValue::Object(map))
            }

            TypeAnnotatedValue::Result(result) => {
                let mut map = serde_json::Map::new();
                match result.value {
                    Ok(value) => {
                        map.insert(
                            "ok".to_string(),
                            value
                                .map(|x| JsonFunctionResult::from(*x))
                                .map(|v| v.0)
                                .unwrap_or(JsonValue::Null),
                        );
                    }
                    Err(value) => {
                        map.insert(
                            "err".to_string(),
                            value
                                .map(|x| JsonFunctionResult::from(*x))
                                .map(|v| v.0)
                                .unwrap_or(JsonValue::Null),
                        );
                    }
                }
                JsonFunctionResult(JsonValue::Object(map))
            }

            TypeAnnotatedValue::Handle(handle) => JsonFunctionResult(JsonValue::String(format!(
                "{}/{}",
                handle.uri.value, handle.resource_id
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::json::{get_record, validate_function_parameter, JsonFunctionResult};
    use crate::{TypeAnnotatedValue, Value};
    use golem_wasm_ast::analysis::AnalysedType;
    use proptest::prelude::*;
    use serde_json::{json, Number, Value as JsonValue};
    use std::collections::HashSet;

    fn validate_function_result(
        val: Value,
        expected_type: &AnalysedType,
    ) -> Result<JsonValue, Vec<String>> {
        TypeAnnotatedValue::from_value(val, expected_type)
            .map(|result| JsonFunctionResult::from(result).0)
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

    #[test]
    fn test_get_record() {
        // Test case where all keys are present
        let input_json = json!({
            "key1": "value1",
            "key2": "value2",
        });

        let key1 = "key1".to_string();
        let key2 = "key2".to_string();

        let name_type_pairs: Vec<(String, AnalysedType)> = vec![
            (key1.clone(), AnalysedType::Str),
            (key2.clone(), AnalysedType::Str),
        ];

        let result = get_record(&input_json, &name_type_pairs);
        let expected_result = Ok(vec![
            Value::String("value1".to_string()),
            Value::String("value2".to_string()),
        ]);
        assert_eq!(result, expected_result);

        // Test case where a key is missing
        let input_json = json!({
            "key1": "value1",
        });

        let result = get_record(&input_json, &name_type_pairs);
        assert!(result.is_err());
    }
}
