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
use golem_wasm_ast::analysis::{
    AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceId, AnalysedResourceMode,
    AnalysedType,
};
use serde_json::{Number, Value as JsonValue};
use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;

use crate::{TypeAnnotatedValue, Uri, Value};

pub fn function_parameters(
    value: &JsonValue,
    expected_parameters: &[AnalysedFunctionParameter],
) -> Result<Vec<Value>, Vec<String>> {
    let typed_values = function_parameters_typed(value, expected_parameters)?;

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

pub fn function_parameters_typed(
    value: &JsonValue,
    expected_parameters: &[AnalysedFunctionParameter],
) -> Result<Vec<TypeAnnotatedValue>, Vec<String>> {
    let parameters = value
        .as_array()
        .ok_or(vec!["Expecting an array for fn_params".to_string()])?;

    let mut results = vec![];
    let mut errors = vec![];

    if parameters.len() == expected_parameters.len() {
        for (json, fp) in parameters.iter().zip(expected_parameters.iter()) {
            match get_typed_value_from_json(json, &fp.typ) {
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
    function_result_typed(values, expected_types).map(|result| Json::from(result).0)
}

pub fn function_result_typed(
    values: Vec<Value>,
    expected_types: &[AnalysedFunctionResult],
) -> Result<TypeAnnotatedValue, Vec<String>> {
    if values.len() != expected_types.len() {
        Err(vec![format!(
            "Unexpected number of result values (got {}, expected: {})",
            values.len(),
            expected_types.len()
        )])
    } else {
        let mut results = vec![];
        let mut errors = vec![];

        for (value, expected) in values.into_iter().zip(expected_types.iter()) {
            let result = TypeAnnotatedValue::from_value(&value, &expected.typ);

            match result {
                Ok(value) => {
                    results.push((value, expected.typ.clone()));
                }
                Err(err) => errors.extend(err),
            }
        }

        let all_without_names = expected_types.iter().all(|t| t.name.is_none());

        if all_without_names {
            Ok(TypeAnnotatedValue::Tuple {
                typ: results.iter().map(|(_, typ)| typ.clone()).collect(),
                value: results.into_iter().map(|(v, _)| v).collect(),
            })
        } else {
            let mut named_typs: Vec<(String, AnalysedType)> = vec![];
            let mut named_values: Vec<(String, TypeAnnotatedValue)> = vec![];

            for (index, ((typed_value, typ), expected)) in
                results.into_iter().zip(expected_types.iter()).enumerate()
            {
                let name = if let Some(name) = &expected.name {
                    name.clone()
                } else {
                    index.to_string()
                };

                named_typs.push((name.clone(), typ.clone()));
                named_values.push((name, typed_value));
            }

            Ok(TypeAnnotatedValue::Record {
                typ: named_typs,
                value: named_values,
            })
        }
    }
}

pub fn get_typed_value_from_json(
    json_val: &JsonValue,
    analysed_type: &AnalysedType,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    match analysed_type {
        AnalysedType::Bool => get_bool(json_val),
        AnalysedType::S8 => get_s8(json_val),
        AnalysedType::U8 => get_u8(json_val),
        AnalysedType::S16 => get_s16(json_val),
        AnalysedType::U16 => get_u16(json_val),
        AnalysedType::S32 => get_s32(json_val),
        AnalysedType::U32 => get_u32(json_val),
        AnalysedType::S64 => get_s64(json_val),
        AnalysedType::U64 => get_u64(json_val),
        AnalysedType::F64 => get_f64(json_val),
        AnalysedType::F32 => get_f32(json_val),
        AnalysedType::Chr => get_char(json_val),
        AnalysedType::Str => get_string(json_val),
        AnalysedType::Enum(names) => get_enum(json_val, names),
        AnalysedType::Flags(names) => get_flag(json_val, names),
        AnalysedType::List(elem) => get_list(json_val, elem),
        AnalysedType::Option(elem) => get_option(json_val, elem),
        AnalysedType::Result { ok, error } => get_result(json_val, ok, error),
        AnalysedType::Record(fields) => get_record(json_val, fields),
        AnalysedType::Variant(cases) => get_variant(json_val, cases),
        AnalysedType::Tuple(elems) => get_tuple(json_val, elems),
        AnalysedType::Resource { id, resource_mode } => {
            get_handle(json_val, id.clone(), resource_mode.clone())
        }
    }
}

fn get_bool(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    match json {
        JsonValue::Bool(bool_val) => Ok(TypeAnnotatedValue::Bool(*bool_val)),
        _ => {
            let type_description = type_description(json);
            Err(vec![format!(
                "Expected function parameter type is Boolean. But found {}",
                type_description
            )])
        }
    }
}

fn get_s8(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i8(i8::MIN).expect("Failed to convert i8::MIN to BigDecimal"),
        BigDecimal::from_i8(i8::MAX).expect("Failed to convert i8::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::S8(num.to_i8().expect("Failed to convert BigDecimal to i8")))
}

fn get_u8(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u8(u8::MIN).expect("Failed to convert u8::MIN to BigDecimal"),
        BigDecimal::from_u8(u8::MAX).expect("Failed to convert u8::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::U8(num.to_u8().expect("Failed to convert BigDecimal to u8")))
}

fn get_s16(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i16(i16::MIN).expect("Failed to convert i16::MIN to BigDecimal"),
        BigDecimal::from_i16(i16::MAX).expect("Failed to convert i16::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::S16(num.to_i16().expect("Failed to convert BigDecimal to i16")))
}

fn get_u16(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u16(u16::MIN).expect("Failed to convert u16::MIN to BigDecimal"),
        BigDecimal::from_u16(u16::MAX).expect("Failed to convert u16::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::U16(num.to_u16().expect("Failed to convert BigDecimal to u16")))
}

fn get_s32(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i32(i32::MIN).expect("Failed to convert i32::MIN to BigDecimal"),
        BigDecimal::from_i32(i32::MAX).expect("Failed to convert i32::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::S32(num.to_i32().expect("Failed to convert BigDecimal to i32")))
}

fn get_u32(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u32(u32::MIN).expect("Failed to convert u32::MIN to BigDecimal"),
        BigDecimal::from_u32(u32::MAX).expect("Failed to convert u32::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::U32(num.to_u32().expect("Failed to convert BigDecimal to u32")))
}

fn get_s64(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i64(i64::MIN).expect("Failed to convert i64::MIN to BigDecimal"),
        BigDecimal::from_i64(i64::MAX).expect("Failed to convert i64::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::S64(num.to_i64().expect("Failed to convert BigDecimal to i64")))
}

fn get_f32(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_f32(f32::MIN).expect("Failed to convert f32::MIN to BigDecimal"),
        BigDecimal::from_f32(f32::MAX).expect("Failed to convert f32::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::F32(num.to_f32().expect("Failed to convert BigDecimal to f32")))
}

fn get_f64(json_val: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    let num = get_big_decimal(json_val)?;
    let value = TypeAnnotatedValue::F64(
        num.to_string()
            .parse()
            .map_err(|err| vec![format!("Failed to parse f64: {}", err)])?,
    );
    Ok(value)
}

fn get_string(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    if let Some(str_value) = json.as_str() {
        // If the JSON value is a string, return it
        Ok(TypeAnnotatedValue::Str(str_value.to_string()))
    } else {
        // If the JSON value is not a string, return an error with type information
        let type_description = type_description(json);
        Err(vec![format!(
            "Expected function parameter type is String. But found {}",
            type_description
        )])
    }
}

fn get_char(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    if let Some(num_u64) = json.as_u64() {
        if num_u64 > u32::MAX as u64 {
            Err(vec![format!(
                "The value {} is too large to be converted to a char",
                num_u64
            )])
        } else {
            char::from_u32(num_u64 as u32)
                .ok_or(vec![format!(
                    "The value {} is not a valid unicode character",
                    num_u64
                )])
                .map(TypeAnnotatedValue::Chr)
        }
    } else {
        let type_description = type_description(json);

        Err(vec![format!(
            "Expected function parameter type is Char. But found {}",
            type_description
        )])
    }
}

fn get_tuple(
    input_json: &JsonValue,
    types: &[AnalysedType],
) -> Result<TypeAnnotatedValue, Vec<String>> {
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
    let mut vals: Vec<TypeAnnotatedValue> = vec![];

    for (json, tpe) in json_array.iter().zip(types.iter()) {
        match get_typed_value_from_json(json, tpe) {
            Ok(result) => vals.push(result),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(TypeAnnotatedValue::Tuple {
            typ: types.to_vec(),
            value: vals,
        })
    } else {
        Err(errors)
    }
}

fn get_option(
    input_json: &JsonValue,
    tpe: &AnalysedType,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    match input_json.as_null() {
        Some(_) => Ok(TypeAnnotatedValue::Option {
            typ: tpe.clone(),
            value: None,
        }),

        None => {
            get_typed_value_from_json(input_json, tpe).map(|result| TypeAnnotatedValue::Option {
                typ: tpe.clone(),
                value: Some(Box::new(result)),
            })
        }
    }
}

fn get_list(input_json: &JsonValue, tpe: &AnalysedType) -> Result<TypeAnnotatedValue, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<TypeAnnotatedValue> = vec![];

    for json in json_array {
        match get_typed_value_from_json(json, tpe) {
            Ok(result) => vals.push(result),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(TypeAnnotatedValue::List {
            typ: tpe.clone(),
            values: vals,
        })
    } else {
        Err(errors)
    }
}

fn get_enum(input_json: &JsonValue, names: &[String]) -> Result<TypeAnnotatedValue, Vec<String>> {
    let input_enum_value = input_json
        .as_str()
        .ok_or(vec![format!("Input {} is not string", input_json)])?;

    if names.contains(&input_enum_value.to_string()) {
        Ok(TypeAnnotatedValue::Enum {
            typ: names.to_vec(),
            value: input_enum_value.to_string(),
        })
    } else {
        Err(vec![format!(
            "Invalid input {}. Valid values are {}",
            input_enum_value,
            names.join(",")
        )])
    }
}

#[allow(clippy::type_complexity)]
fn get_result(
    input_json: &JsonValue,
    ok_type: &Option<Box<AnalysedType>>,
    err_type: &Option<Box<AnalysedType>>,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    fn validate(
        typ: &Option<Box<AnalysedType>>,
        input_json: &JsonValue,
    ) -> Result<Option<Box<TypeAnnotatedValue>>, Vec<String>> {
        if let Some(typ) = typ {
            get_typed_value_from_json(input_json, typ).map(|v| Some(Box::new(v)))
        } else if input_json.is_null() {
            Ok(None)
        } else {
            Err(vec![
                "The type of ok is absent, but some JSON value was provided".to_string(),
            ])
        }
    }

    match input_json.get("ok") {
        Some(value) => Ok(TypeAnnotatedValue::Result {
            ok: ok_type.clone(),
            error: err_type.clone(),
            value: Ok(validate(ok_type, value)?),
        }),
        None => match input_json.get("err") {
            Some(value) => Ok(TypeAnnotatedValue::Result {
                ok: ok_type.clone(),
                error: err_type.clone(),
                value: Err(validate(err_type, value)?),
            }),
            None => Err(vec![
                "Failed to retrieve either ok value or err value".to_string()
            ]),
        },
    }
}

fn get_record(
    input_json: &JsonValue,
    name_type_pairs: &[(String, AnalysedType)],
) -> Result<TypeAnnotatedValue, Vec<String>> {
    let json_map = input_json.as_object().ok_or(vec![format!(
        "The input {} is not a json object",
        input_json
    )])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<(String, TypeAnnotatedValue)> = vec![];

    for (name, tpe) in name_type_pairs {
        if let Some(json_value) = json_map.get(name) {
            match get_typed_value_from_json(json_value, tpe) {
                Ok(result) => vals.push((name.clone(), result)),
                Err(value_errors) => errors.extend(
                    value_errors
                        .iter()
                        .map(|err| format!("Invalid value for the key {}. Error: {}", name, err))
                        .collect::<Vec<_>>(),
                ),
            }
        } else {
            match tpe {
                AnalysedType::Option(_) => vals.push((
                    name.clone(),
                    TypeAnnotatedValue::Option {
                        typ: tpe.clone(),
                        value: None,
                    },
                )),
                _ => errors.push(format!("Key '{}' not found in json_map", name)),
            }
        }
    }

    if errors.is_empty() {
        Ok(TypeAnnotatedValue::Record {
            typ: name_type_pairs.to_vec(),
            value: vals,
        })
    } else {
        Err(errors)
    }
}

fn get_flag(input_json: &JsonValue, names: &[String]) -> Result<TypeAnnotatedValue, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<String> = vec![];

    for json in json_array.iter() {
        let flag: String = json
            .as_str()
            .map(|x| x.to_string())
            .or_else(|| json.as_bool().map(|b| b.to_string()))
            .or_else(|| json.as_number().map(|n| n.to_string()))
            .ok_or(vec![format!(
                "Input {} is not a string or boolean or number",
                json
            )])?;

        if names.contains(&flag) {
            vals.push(flag);
        } else {
            errors.push(format!(
                "Invalid input {}. Valid values are {}",
                flag,
                names.join(",")
            ));
        }
    }

    if errors.is_empty() {
        Ok(TypeAnnotatedValue::Flags {
            typ: names.to_vec(),
            values: vals,
        })
    } else {
        Err(errors)
    }
}

fn get_variant(
    input_json: &JsonValue,
    types: &[(String, Option<AnalysedType>)],
) -> Result<TypeAnnotatedValue, Vec<String>> {
    let mut possible_mapping_indexed: HashMap<&String, &Option<AnalysedType>> = HashMap::new();

    for (name, optional_type) in types.iter() {
        possible_mapping_indexed.insert(name, optional_type);
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
        Some(Some(tpe)) => {
            get_typed_value_from_json(json, tpe).map(|result| TypeAnnotatedValue::Variant {
                typ: types.to_vec(),
                case_name: key.clone(),
                case_value: Some(Box::new(result)),
            })
        }
        Some(None) if json.is_null() => Ok(TypeAnnotatedValue::Variant {
            typ: types.to_vec(),
            case_name: key.clone(),
            case_value: None,
        }),
        Some(None) => Err(vec![format!("Unit variant {key} has non-null JSON value")]),
        None => Err(vec![format!("Unknown key {key} in the variant")]),
    }
}

fn get_handle(
    value: &JsonValue,
    id: AnalysedResourceId,
    resource_mode: AnalysedResourceMode,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    match value.as_str() {
        Some(str) => {
            // not assuming much about the url format, just checking it ends with a /<resource-id-u64>
            let parts: Vec<&str> = str.split('/').collect();
            if parts.len() >= 2 {
                match u64::from_str(parts[parts.len() - 1]) {
                    Ok(resource_id) => {
                        let uri = parts[0..(parts.len() - 1)].join("/");
                        Ok(TypeAnnotatedValue::Handle { id, resource_mode, uri: Uri { value: uri }, resource_id })
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

fn ensure_range(
    value: &JsonValue,
    min: BigDecimal,
    max: BigDecimal,
) -> Result<BigDecimal, Vec<String>> {
    let num = get_big_decimal(value)?;
    if num >= min && num <= max {
        Ok(num)
    } else {
        Err(vec![format!(
            "value {} is not within the range of {} to {}",
            value, min, max
        )])
    }
}

fn get_big_decimal(value: &JsonValue) -> Result<BigDecimal, Vec<String>> {
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

fn get_u64(value: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    match value {
        JsonValue::Number(num) => {
            if let Some(u64) = num.as_u64() {
                Ok(TypeAnnotatedValue::U64(u64))
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
    use crate::json::{get_typed_value_from_json, Json};
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
        match get_typed_value_from_json(json, expected_type) {
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
