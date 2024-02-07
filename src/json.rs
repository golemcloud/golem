use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};
use golem_wasm_ast::analysis::{AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedType};
use serde_json::{Number, Value as JsonValue};
use std::collections::HashMap;
use std::str::FromStr;

use crate::Value;

// TODO: reduce clones
// TODO: get rid of unwraps

pub fn function_parameters(
    value: &JsonValue,
    expected_parameters: Vec<AnalysedFunctionParameter>,
) -> Result<Vec<Value>, Vec<String>> {
    let parameters = value
        .as_array()
        .ok_or(vec!["Expecting an array for fn_params".to_string()])?;

    let mut results = vec![];
    let errors = vec![];

    if parameters.len() == expected_parameters.len() {
        for (json, fp) in parameters.iter().zip(expected_parameters.iter()) {
            let result = validate_function_parameter(json, fp.typ.clone())?;
            results.push(result);
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
    expected_types: Vec<AnalysedFunctionResult>,
) -> Result<JsonValue, Vec<String>> {
    if values.len() != expected_types.len() {
        Err(vec![format!(
            "Unexpected number of result values (got {}, expected: {})",
            values.len(),
            expected_types.len()
        )])
    } else {
        let mut results = vec![];
        let mut errors = vec![];

        for (value, expected) in values.iter().zip(expected_types.iter()) {
            let result = validate_function_result(value, expected.typ.clone());

            match result {
                Ok(value) => results.push(value),
                Err(err) => errors.extend(err),
            }
        }

        let all_without_names = expected_types.iter().all(|t| t.name.is_none());

        if all_without_names {
            Ok(serde_json::Value::Array(results))
        } else {
            let mapped_values = results
                .iter()
                .zip(expected_types.iter())
                .enumerate()
                .map(|(idx, (json, result_def))| {
                    (
                        if let Some(name) = &result_def.name {
                            name.clone()
                        } else {
                            idx.to_string()
                        },
                        json.clone(),
                    )
                })
                .collect();

            Ok(serde_json::Value::Object(mapped_values))
        }
    }
}

fn validate_function_parameter(
    input_json: &JsonValue,
    expected_type: AnalysedType,
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
            bigdecimal(input_json).map(|num| Value::F64(num.to_string().parse().unwrap()))
        }
        AnalysedType::F32 => get_f32(input_json),
        AnalysedType::Chr => get_char(input_json).map(Value::Char),
        AnalysedType::Str => get_string(input_json).map(Value::String),

        AnalysedType::Enum(names) => get_enum(input_json, names).map(Value::Enum),

        AnalysedType::Flags(names) => get_flag(input_json, names).map(Value::Flags),

        AnalysedType::List(elem) => get_list(input_json, *elem).map(Value::List),

        AnalysedType::Option(elem) => {
            get_option(input_json, *elem).map(Value::Option)
        }

        AnalysedType::Result { ok, error } => {
            get_result(input_json, ok.map(|t| *t), error.map(|t| *t))
                .map(Value::Result)
        }

        AnalysedType::Record(fields) => get_record(input_json, &fields).map(Value::Record),

        AnalysedType::Variant(cases) => {
            get_variant(input_json, &cases).map(|result| Value::Variant {
                case_idx: result.0,
                case_value: result.1,
            })
        }
        AnalysedType::Tuple(elems) => get_tuple(input_json, elems).map(Value::Tuple),
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
        BigDecimal::from_i8(i8::MIN).unwrap(),
        BigDecimal::from_i8(i8::MAX).unwrap(),
    )
    .map(|num| Value::S8(num.to_i8().unwrap()))
}

fn get_u8(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u8(u8::MIN).unwrap(),
        BigDecimal::from_u8(u8::MAX).unwrap(),
    )
    .map(|num| Value::U8(num.to_u8().unwrap()))
}

fn get_s16(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i16(i16::MIN).unwrap(),
        BigDecimal::from_i16(i16::MAX).unwrap(),
    )
    .map(|num| Value::S16(num.to_i16().unwrap()))
}

fn get_u16(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u16(u16::MIN).unwrap(),
        BigDecimal::from_u16(u16::MAX).unwrap(),
    )
    .map(|num| Value::U16(num.to_u16().unwrap()))
}

fn get_s32(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i32(i32::MIN).unwrap(),
        BigDecimal::from_i32(i32::MAX).unwrap(),
    )
    .map(|num| Value::S32(num.to_i32().unwrap()))
}

fn get_u32(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u32(u32::MIN).unwrap(),
        BigDecimal::from_u32(u32::MAX).unwrap(),
    )
    .map(|num| Value::U32(num.to_u32().unwrap()))
}

fn get_s64(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i64(i64::MIN).unwrap(),
        BigDecimal::from_i64(i64::MAX).unwrap(),
    )
    .map(|num| Value::S64(num.to_i64().unwrap()))
}

fn get_f32(json: &JsonValue) -> Result<Value, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_f32(f32::MIN).unwrap(),
        BigDecimal::from_f32(f32::MAX).unwrap(),
    )
    .map(|num| Value::F32(num.to_f32().unwrap()))
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

fn get_result(
    input_json: &JsonValue,
    ok_type: Option<AnalysedType>,
    err_type: Option<AnalysedType>,
) -> Result<Result<Box<Value>, Box<Value>>, Vec<String>> {
    fn validate(
        typ: Option<AnalysedType>,
        input_json: &JsonValue,
    ) -> Result<Box<Value>, Vec<String>> {
        if let Some(typ) = typ {
            validate_function_parameter(input_json, typ).map(Box::new)
        } else {
            Err(vec!["The type of ok is absent".to_string()])
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
    tpe: AnalysedType,
) -> Result<Option<Box<Value>>, Vec<String>> {
    match input_json.as_null() {
        Some(_) => Ok(None),

        None => validate_function_parameter(input_json, tpe).map(|result| Some(Box::new(result))),
    }
}

fn get_list(input_json: &JsonValue, tpe: AnalysedType) -> Result<Vec<Value>, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<Value> = vec![];

    for json in json_array {
        match validate_function_parameter(json, tpe.clone()) {
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

fn get_tuple(input_json: &JsonValue, types: Vec<AnalysedType>) -> Result<Vec<Value>, Vec<String>> {
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
        match validate_function_parameter(json, tpe.clone()) {
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
            match validate_function_parameter(json_value, tpe.clone()) {
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

fn get_enum(input_json: &JsonValue, names: Vec<String>) -> Result<u32, Vec<String>> {
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

fn get_flag(input_json: &JsonValue, names: Vec<String>) -> Result<Vec<bool>, Vec<String>> {
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
) -> Result<(u32, Box<Value>), Vec<String>> {
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
        Some((index, Some(tpe))) => validate_function_parameter(json, tpe.clone())
            .map(|result| (*index as u32, Box::new(result))),
        Some((_, None)) => Err(vec![format!("Unknown json {} in the variant", input_json)]),
        None => Err(vec![format!("Unknown key {} in the variant", key)]),
    }
}

fn validate_function_result(
    val: &Value,
    expected_type: AnalysedType,
) -> Result<JsonValue, Vec<String>> {
    match val {
        Value::Bool(bool) => Ok(serde_json::Value::Bool(*bool)),
        Value::S8(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::U8(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::U32(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::S16(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::U16(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::S32(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::S64(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::U64(value) => Ok(serde_json::Value::Number(Number::from(*value))),
        Value::F32(value) => Ok(serde_json::Value::Number(
            Number::from_f64(*value as f64).unwrap(),
        )),
        Value::F64(value) => Ok(serde_json::Value::Number(Number::from_f64(*value).unwrap())),
        Value::Char(value) => Ok(serde_json::Value::Number(Number::from(*value as u32))),
        Value::String(value) => Ok(serde_json::Value::String(value.to_string())),

        Value::Enum(value) => match expected_type {
            AnalysedType::Enum(names) => match names.get(*value as usize) {
                Some(str) => Ok(serde_json::Value::String(str.clone())),
                None => Err(vec![format!("Invalid enum {}", value)]),
            },
            _ => Err(vec![format!("Unexpected enum {}", value)]),
        },

        Value::Option(value) => match expected_type {
            AnalysedType::Option(elem) => match &value {
                Some(value) => validate_function_result(value, *elem),
                None => Ok(serde_json::Value::Null),
            },

            _ => Err(vec!["Unexpected type; expected an Option type.".to_string()]),
        },

        Value::Tuple(values) => match expected_type {
            AnalysedType::Tuple(types) => {
                if values.len() != types.len() {
                    return Err(vec![format!(
                        "Tuple has unexpected number of elements: {} vs {}",
                        values.len(),
                        types.len(),
                    )]);
                }

                let mut errors = vec![];
                let mut results = vec![];

                for (v, tpe) in values.iter().zip(types.iter()) {
                    match validate_function_result(v, tpe.clone()) {
                        Ok(result) => results.push(result),
                        Err(errs) => errors.extend(errs),
                    }
                }

                if errors.is_empty() {
                    Ok(serde_json::Value::Array(results))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a tuple type.".to_string()]),
        },

        Value::List(values) => match expected_type {
            AnalysedType::List(elem) => {
                let mut errors = vec![];
                let mut results = vec![];

                for v in values.clone() {
                    match validate_function_result(&v, (*elem).clone()) {
                        Ok(value) => results.push(value),
                        Err(errs) => errors.extend(errs),
                    }
                }

                if errors.is_empty() {
                    Ok(serde_json::Value::Array(results))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a list type.".to_string()]),
        },

        Value::Record(values) => match expected_type {
            AnalysedType::Record(fields) => {
                if values.len() != fields.len() {
                    return Err(vec!["The total number of field values is zero".to_string()]);
                }

                let mut errors = vec![];
                let mut results = serde_json::Map::new();

                for (v, (field_name, typ)) in values.iter().zip(fields) {
                    match validate_function_result(v, typ) {
                        Ok(res) => {
                            results.insert(field_name, res);
                        }
                        Err(errs) => errors.extend(errs),
                    }
                }

                if errors.is_empty() {
                    Ok(serde_json::Value::Object(results))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
        },

        Value::Variant {
            case_idx,
            case_value,
        } => match expected_type {
            AnalysedType::Variant(cases) => {
                if (*case_idx as usize) < cases.len() {
                    let (case_name, case_type) = match cases.get(*case_idx as usize) {
                        Some(tpe) => Ok(tpe),
                        None => Err(vec!["Variant not found in the expected types.".to_string()]),
                    }?;

                    match case_type {
                        Some(tpe) => {
                            let result = validate_function_result(case_value, tpe.clone())?;
                            let mut map = serde_json::Map::new();
                            map.insert(case_name.clone(), result);
                            Ok(serde_json::Value::Object(map))
                        }
                        None => Err(vec!["Missing inner type information.".to_string()]),
                    }
                } else {
                    Err(vec![
                        "Invalid discriminant value for the variant.".to_string()
                    ])
                }
            }

            _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
        },

        Value::Flags(values) => match expected_type {
            AnalysedType::Flags(names) => {
                let mut result = vec![];

                if values.len() != names.len() {
                    Err(vec![format!(
                        "Unexpected number of flag states: {:?} vs {:?}",
                        values, names
                    )])
                } else {
                    for (enabled, name) in values.iter().zip(names) {
                        if *enabled {
                            result.push(JsonValue::String(name));
                        }
                    }

                    Ok(JsonValue::Array(result))
                }
            }
            _ => Err(vec!["Unexpected type; expected a flags type.".to_string()]),
        },

        Value::Result(value) => match expected_type {
            AnalysedType::Result { ok, error } => match (value, ok, error) {
                (Ok(value), Some(ok_type), _) => {
                    let mut map: serde_json::Map<String, serde_json::Value> =
                        serde_json::Map::new();

                    let result = validate_function_result(value, *ok_type)?;
                    map.insert("ok".to_string(), result);
                    Ok(serde_json::Value::Object(map))
                }

                (Ok(_), None, _) => {
                    let mut map: serde_json::Map<String, serde_json::Value> =
                        serde_json::Map::new();

                    map.insert("ok".to_string(), serde_json::Value::Null);

                    Ok(serde_json::Value::Object(map))
                }

                (Err(value), _, Some(err_type)) => {
                    let mut map: serde_json::Map<String, serde_json::Value> =
                        serde_json::Map::new();

                    let result = validate_function_result(value, *err_type)?;
                    map.insert("err".to_string(), result);

                    Ok(serde_json::Value::Object(map))
                }

                (Err(_), _, None) => {
                    let mut map: serde_json::Map<String, serde_json::Value> =
                        serde_json::Map::new();

                    map.insert("err".to_string(), serde_json::Value::Null);

                    Ok(serde_json::Value::Object(map))
                }
            },

            _ => Err(vec!["Unexpected type; expected a Result type.".to_string()]),
        },
    }
}

// TODO: reenable tests
// #[cfg(test)]
// mod tests {
//     use serde_json::json;
//     use std::collections::HashSet;
//
//     use proptest::prelude::*;
//     use serde::Serialize;
//     use serde_json::{Number, Value as JsonValue};
//
//     use crate::Value;
//     use super::*;
//
//     #[derive(Debug, Clone, PartialEq)]
//     struct RandomData {
//         string: String,
//         number: f64,
//         nullable: Option<String>,
//         collection: Vec<String>,
//         boolean: bool,
//         object: InnerObj,
//     }
//
//     #[derive(Debug, Clone, PartialEq, Serialize)]
//     struct InnerObj {
//         nested: String,
//     }
//
//     impl RandomData {
//         fn get_type() -> AnalysedType {
//             AnalysedType::Record(golem_api_grpc::proto::golem::template::TypeRecord {
//                 fields: vec![
//                     golem_api_grpc::proto::golem::template::NameTypePair {
//                         name: "string".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Primitive(TypePrimitive {
//                                 primitive: PrimitiveType::Str as i32,
//                             })),
//                         }),
//                     },
//                     golem_api_grpc::proto::golem::template::NameTypePair {
//                         name: "number".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Primitive(TypePrimitive {
//                                 primitive: PrimitiveType::F64 as i32,
//                             })),
//                         }),
//                     },
//                     golem_api_grpc::proto::golem::template::NameTypePair {
//                         name: "nullable".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Option(Box::new(
//                                 golem_api_grpc::proto::golem::template::TypeOption {
//                                     elem: Some(Box::new(
//                                         golem_api_grpc::proto::golem::template::Type {
//                                             r#type: Some(Type::Primitive(TypePrimitive {
//                                                 primitive: PrimitiveType::Str as i32,
//                                             })),
//                                         },
//                                     )),
//                                 },
//                             ))),
//                         }),
//                     },
//                     golem_api_grpc::proto::golem::template::NameTypePair {
//                         name: "collection".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::List(Box::new(
//                                 golem_api_grpc::proto::golem::template::TypeList {
//                                     elem: Some(Box::new(
//                                         golem_api_grpc::proto::golem::template::Type {
//                                             r#type: Some(Type::Primitive(TypePrimitive {
//                                                 primitive: PrimitiveType::Str as i32,
//                                             })),
//                                         },
//                                     )),
//                                 },
//                             ))),
//                         }),
//                     },
//                     golem_api_grpc::proto::golem::template::NameTypePair {
//                         name: "boolean".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Primitive(TypePrimitive {
//                                 primitive: PrimitiveType::Bool as i32,
//                             })),
//                         }),
//                     },
//                     // one field is missing
//                     golem_api_grpc::proto::golem::template::NameTypePair {
//                         name: "object".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Record(
//                                 golem_api_grpc::proto::golem::template::TypeRecord {
//                                     fields: vec![
//                                         golem_api_grpc::proto::golem::template::NameTypePair {
//                                             name: "nested".to_string(),
//                                             typ: Some(
//                                                 golem_api_grpc::proto::golem::template::Type {
//                                                     r#type: Some(Type::Primitive(TypePrimitive {
//                                                         primitive: PrimitiveType::Str as i32,
//                                                     })),
//                                                 },
//                                             ),
//                                         },
//                                     ],
//                                 },
//                             )),
//                         }),
//                     },
//                 ],
//             })
//         }
//     }
//
//     impl Arbitrary for RandomData {
//         type Parameters = ();
//         type Strategy = BoxedStrategy<Self>;
//
//         fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
//             (
//                 any::<String>(),
//                 any::<f64>(),
//                 any::<Option<String>>(),
//                 any::<Vec<String>>(),
//                 any::<bool>(),
//                 any::<InnerObj>(),
//             )
//                 .prop_map(
//                     |(string, number, nullable, collection, boolean, object)| RandomData {
//                         string,
//                         number,
//                         nullable,
//                         collection,
//                         boolean,
//                         object,
//                     },
//                 )
//                 .boxed()
//         }
//     }
//
//     impl Arbitrary for InnerObj {
//         type Parameters = ();
//         type Strategy = BoxedStrategy<Self>;
//
//         fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
//             any::<String>()
//                 .prop_map(|nested| InnerObj { nested })
//                 .boxed()
//         }
//     }
//
//     #[derive(Debug, Clone, PartialEq, Serialize)]
//     struct FunctionOutputTestResult {
//         val: Val,
//         expected_type: Type,
//     }
//
//     #[derive(Debug, Clone, PartialEq)]
//     struct PrimitiveVal {
//         val: Value,
//         expected_type: AnalysedType,
//     }
//
//     impl Arbitrary for PrimitiveVal {
//         type Parameters = ();
//         type Strategy = BoxedStrategy<Self>;
//
//         fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
//             prop_oneof![
//                 any::<i32>().prop_map(|val| PrimitiveVal {
//                     val: Value::S32(val),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::S32 as i32
//                     })
//                 }),
//                 any::<i8>().prop_map(|val| PrimitiveVal {
//                     val: Value::S8(val as i32),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::S8 as i32
//                     })
//                 }),
//                 any::<i16>().prop_map(|val| PrimitiveVal {
//                     val: Value::S16(val as i32),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::S16 as i32
//                     })
//                 }),
//                 any::<i64>().prop_map(|val| PrimitiveVal {
//                     val: Value::S64(val),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::S64 as i32
//                     })
//                 }),
//                 any::<u8>().prop_map(|val| PrimitiveVal {
//                     val: Value::U8(val as i32),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::U8 as i32
//                     })
//                 }),
//                 any::<u16>().prop_map(|val| PrimitiveVal {
//                     val: Value::U16(val as i32),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::U16 as i32
//                     })
//                 }),
//                 any::<u32>().prop_map(|val| PrimitiveVal {
//                     val: Value::U32(val as i64),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::U32 as i32
//                     })
//                 }),
//                 any::<u64>().prop_map(|val| PrimitiveVal {
//                     val: Value::U64(val as i64),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::U64 as i32
//                     })
//                 }),
//                 any::<f32>().prop_map(|val| PrimitiveVal {
//                     val: Value::F32(val),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::F32 as i32
//                     })
//                 }),
//                 any::<f64>().prop_map(|val| PrimitiveVal {
//                     val: Value::F64(val),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::F64 as i32
//                     })
//                 }),
//                 any::<bool>().prop_map(|val| PrimitiveVal {
//                     val: Value::Bool(val),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::Bool as i32
//                     })
//                 }),
//                 any::<u16>().prop_map(|val| val).prop_map(|_| {
//                     PrimitiveVal {
//                         val: Value::Char('a' as i32),
//                         expected_type: Type::Primitive(TypePrimitive {
//                             primitive: PrimitiveType::Chr as i32,
//                         }),
//                     }
//                 }),
//                 any::<String>().prop_map(|val| PrimitiveVal {
//                     val: Value::String(val),
//                     expected_type: Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::Str as i32
//                     })
//                 }),
//             ]
//             .boxed()
//         }
//     }
//
//     fn distinct_by<T, F>(vec: Vec<T>, key_fn: F) -> Vec<T>
//     where
//         F: Fn(&T) -> String,
//         T: Clone + PartialEq,
//     {
//         let mut seen = HashSet::new();
//         vec.into_iter()
//             .filter(|item| seen.insert(key_fn(item)))
//             .collect()
//     }
//
//     impl Arbitrary for FunctionOutputTestResult {
//         type Parameters = ();
//         type Strategy = BoxedStrategy<Self>;
//
//         fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
//             prop_oneof![
//                 any::<(Vec<String>, u8)>().prop_map(|(values, disc)| {
//                     let unique_values = distinct_by(values, |x| x.clone());
//
//                     FunctionOutputTestResult {
//                         val: Value::Enum(ValEnum {
//                             discriminant: if (disc as usize) < unique_values.len() {
//                                 disc.into()
//                             } else {
//                                 0
//                             },
//                         }),
//                         expected_type: Type::Enum(
//                             golem_api_grpc::proto::golem::template::TypeEnum {
//                                 names: if unique_values.is_empty() {
//                                     vec!["const_name".to_string()]
//                                 } else {
//                                     unique_values.iter().map(|name| name.to_string()).collect()
//                                 },
//                             },
//                         ),
//                     }
//                 }),
//                 any::<Vec<String>>().prop_map(|values| {
//                     let unique_values = distinct_by(values, |x| x.clone());
//
//                     FunctionOutputTestResult {
//                         val: Value::Flags(ValFlags {
//                             count: unique_values.len() as i32,
//                             value: unique_values
//                                 .iter()
//                                 .enumerate()
//                                 .map(|(index, _)| index as i32)
//                                 .collect(),
//                         }),
//                         expected_type: Type::Flags(
//                             golem_api_grpc::proto::golem::template::TypeFlags {
//                                 names: unique_values.iter().map(|name| name.to_string()).collect(),
//                             },
//                         ),
//                     }
//                 }),
//                 any::<Vec<PrimitiveVal>>().prop_map(|values| {
//                     let expected_type = if values.is_empty() {
//                         Type::Primitive(TypePrimitive {
//                             primitive: PrimitiveType::Str as i32,
//                         })
//                     } else {
//                         values[0].expected_type.clone()
//                     };
//
//                     let vals_with_same_type = values
//                         .iter()
//                         .filter(|prim| prim.expected_type == expected_type)
//                         .cloned()
//                         .collect::<Vec<PrimitiveVal>>();
//
//                     FunctionOutputTestResult {
//                         val: Value::List(ValList {
//                             values: vals_with_same_type
//                                 .iter()
//                                 .map(|prim| VVal {
//                                     val: Some(prim.val.clone()),
//                                 })
//                                 .collect(),
//                         }),
//                         expected_type: Type::List(Box::new(
//                             golem_api_grpc::proto::golem::template::TypeList {
//                                 elem: Some(Box::new(
//                                     golem_api_grpc::proto::golem::template::Type {
//                                         r#type: expected_type.into(),
//                                     },
//                                 )),
//                             },
//                         )),
//                     }
//                 }),
//                 any::<Vec<PrimitiveVal>>().prop_map(|values| FunctionOutputTestResult {
//                     val: Value::Tuple(ValTuple {
//                         values: values
//                             .iter()
//                             .map(|x| VVal {
//                                 val: Some(x.val.clone())
//                             })
//                             .collect()
//                     }),
//                     expected_type: Type::Tuple(golem_api_grpc::proto::golem::template::TypeTuple {
//                         elems: values
//                             .iter()
//                             .map(|x| golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(x.expected_type.clone())
//                             })
//                             .collect()
//                     })
//                 }),
//                 any::<Vec<(String, PrimitiveVal)>>().prop_map(|values| {
//                     let new_values = distinct_by(values, |(x, _)| x.clone());
//
//                     FunctionOutputTestResult {
//                         val: {
//                             Value::Record(ValRecord {
//                                 values: new_values
//                                     .iter()
//                                     .map(|(_, val)| VVal {
//                                         val: Some(val.val.clone()),
//                                     })
//                                     .collect(),
//                             })
//                         },
//                         expected_type: Type::Record(
//                             golem_api_grpc::proto::golem::template::TypeRecord {
//                                 fields: new_values
//                                     .iter()
//                                     .map(|(name, val)| {
//                                         golem_api_grpc::proto::golem::template::NameTypePair {
//                                             name: name.to_string(),
//                                             typ: Some(
//                                                 golem_api_grpc::proto::golem::template::Type {
//                                                     r#type: Some(val.expected_type.clone()),
//                                                 },
//                                             ),
//                                         }
//                                     })
//                                     .collect(),
//                             },
//                         ),
//                     }
//                 }),
//                 any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
//                     val: Value::Option(Box::new(ValOption {
//                         discriminant: 1,
//                         value: Some(Box::new(VVal {
//                             val: Some(val.val.clone())
//                         }))
//                     })),
//                     expected_type: Type::Option(Box::new(
//                         golem_api_grpc::proto::golem::template::TypeOption {
//                             elem: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(val.expected_type.clone())
//                             }))
//                         }
//                     ))
//                 }),
//                 Just(FunctionOutputTestResult {
//                     val: Value::Option(Box::new(ValOption {
//                         discriminant: 0,
//                         value: None
//                     })),
//                     expected_type: Type::Option(Box::new(
//                         golem_api_grpc::proto::golem::template::TypeOption {
//                             elem: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(Type::Primitive(TypePrimitive {
//                                     primitive: PrimitiveType::Str as i32
//                                 }))
//                             }))
//                         }
//                     ))
//                 }),
//                 any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
//                     val: Value::Result(Box::new(ValResult {
//                         discriminant: 0,
//                         value: Some(Box::new(VVal {
//                             val: Some(val.val.clone())
//                         }))
//                     })),
//                     expected_type: Type::Result(Box::new(
//                         golem_api_grpc::proto::golem::template::TypeResult {
//                             ok: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(val.expected_type.clone())
//                             })),
//                             err: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(Type::Primitive(TypePrimitive {
//                                     primitive: PrimitiveType::Str as i32
//                                 }))
//                             }))
//                         }
//                     ))
//                 }),
//                 any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
//                     val: Value::Result(Box::new(ValResult {
//                         discriminant: 1,
//                         value: Some(Box::new(VVal {
//                             val: Some(val.val.clone())
//                         }))
//                     })),
//                     expected_type: Type::Result(Box::new(
//                         golem_api_grpc::proto::golem::template::TypeResult {
//                             ok: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(Type::Primitive(TypePrimitive {
//                                     primitive: PrimitiveType::Str as i32
//                                 }))
//                             })),
//                             err: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                                 r#type: Some(val.expected_type.clone())
//                             }))
//                         }
//                     ))
//                 }),
//                 any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
//                     val: val.val.clone(),
//                     expected_type: val.expected_type.clone()
//                 }),
//             ]
//             .boxed()
//         }
//     }
//
//     fn test_type_checker_string(data: String) {
//         let json = Value::String(data.clone());
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::Str as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::String(data)));
//     }
//
//     fn test_type_checker_s8(data: i32) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::S8 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::S8(data)));
//     }
//     fn test_type_checker_u8(data: i32) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::U8 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::U8(data)));
//     }
//
//     fn test_type_checker_s16(data: i32) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::S16 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::S16(data)));
//     }
//
//     fn test_type_checker_u16(data: i32) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::U16 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::U16(data)));
//     }
//
//     fn test_type_checker_s32(data: i32) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::S32 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::S32(data)));
//     }
//
//     fn test_type_checker_u32(data: i64) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::U32 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::U32(data)));
//     }
//
//     fn test_type_checker_s64(data: i64) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::S64 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::S64(data)));
//     }
//
//     fn test_type_checker_u64(data: i64) {
//         let json = Value::Number(Number::from(data));
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::U64 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::U64(data)));
//     }
//
//     fn test_type_checker_f32(data: f32) {
//         let json = Value::Number(Number::from_f64(data as f64).unwrap());
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::F32 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::F32(data)));
//     }
//
//     fn test_type_checker_f64(data: f64) {
//         let json = Value::Number(Number::from_f64(data).unwrap());
//         let result = validate_function_parameter(
//             &json,
//             Type::Primitive(TypePrimitive {
//                 primitive: PrimitiveType::F64 as i32,
//             }),
//         );
//         assert_eq!(result, Ok(Value::F64(data)));
//     }
//
//     fn test_type_checker_record(data: &RandomData) {
//         let json = serde_json::json!({
//             "string": data.string.clone(),
//             "number": data.number,
//             "nullable": data.nullable.clone(),
//             "collection": data.collection.clone(),
//             "boolean": data.boolean,
//             "object": data.object.clone()
//         });
//
//         let result = validate_function_parameter(&json, RandomData::get_type());
//
//         assert_eq!(
//             result,
//             Ok(Value::Record(ValRecord {
//                 values: vec![
//                     VVal {
//                         val: Some(Value::String(data.string.clone()))
//                     },
//                     VVal {
//                         val: Some(Value::F64(data.number))
//                     },
//                     VVal {
//                         val: match &data.nullable {
//                             Some(place) => Some(Value::Option(Box::new(ValOption {
//                                 discriminant: 1,
//                                 value: Some(Box::new(VVal {
//                                     val: Some(Value::String(place.clone()))
//                                 }))
//                             }))),
//                             None => Some(Value::Option(Box::new(ValOption {
//                                 discriminant: 0,
//                                 value: None
//                             }))),
//                         }
//                     },
//                     VVal {
//                         val: Some(Value::List(ValList {
//                             values: data
//                                 .collection
//                                 .clone()
//                                 .into_iter()
//                                 .map(|friend| VVal {
//                                     val: Some(Value::String(friend))
//                                 })
//                                 .collect()
//                         }))
//                     },
//                     VVal {
//                         val: Some(Value::Bool(data.boolean))
//                     },
//                     VVal {
//                         val: Some(Value::Record(ValRecord {
//                             values: vec![VVal {
//                                 val: Some(Value::String(data.object.nested.clone()))
//                             }]
//                         }))
//                     }
//                 ]
//             }))
//         );
//     }
//
//     proptest! {
//         #[test]
//         fn test3(data in 0..=255) {
//             test_type_checker_u8(data);
//         }
//
//         #[test]
//         fn test_s8(data in -127..=127) {
//             test_type_checker_s8(data);
//         }
//
//         #[test]
//         fn test4(data in -32768..=32767) {
//             test_type_checker_s16(data);
//         }
//
//         #[test]
//         fn test5(data in 0..=65535) {
//             test_type_checker_u16(data);
//         }
//
//         #[test]
//         fn test6(data in -2147483648..=2147483647) {
//             test_type_checker_s32(data);
//         }
//
//         #[test]
//         fn test7(data in 0..=u32::MAX) {
//             test_type_checker_u32(data as i64);
//         }
//
//         #[test]
//         fn test8(data in -9100645029148136..=9136655737043548_i64) {
//             test_type_checker_s64(data);
//         }
//
//         // TODO; Value::U64 takes an i64
//         #[test]
//         fn test9(data in 0..=i64::MAX) {
//             test_type_checker_u64(data);
//         }
//
//         #[test]
//         fn test10(data in f32::MIN..=f32::MAX) {
//             test_type_checker_f32(data);
//         }
//
//         #[test]
//         fn test11(data in f64::MIN..=f64::MAX) {
//             test_type_checker_f64(data);
//         }
//
//         #[test]
//         fn test_process_record(data in any::<RandomData>()) {
//             test_type_checker_record(&data);
//         }
//
//         #[test]
//         fn test_round_trip(fun_output in any::<FunctionOutputTestResult>()) {
//             let validated_output = validate_function_result(&fun_output.val, fun_output.expected_type.clone());
//
//             let validated_input = validate_function_parameter(
//                 &validated_output.expect("Failed to validate function result"),
//                 fun_output.expected_type.clone(),
//             );
//
//             assert_eq!(validated_input, Ok(fun_output.val.clone()));
//         }
//
//         #[test]
//         fn test_string(data in any::<String>()) {
//             test_type_checker_string(data);
//         }
//     }
//
//     #[test]
//     fn test_validate_function_result_stdio() {
//         let str_val = vec![VVal {
//             val: Some(Value::String("str".to_string())),
//         }];
//
//         let res = str_val.validate_function_result(
//             vec![FunctionResult {
//                 name: Some("a".to_string()),
//                 tpe: Some(golem_api_grpc::proto::golem::template::Type {
//                     r#type: Some(Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::Str as i32,
//                     })),
//                 }),
//             }],
//             CallingConvention::Stdio,
//         );
//
//         assert!(res.is_ok_and(|r| r == Value::String("str".to_string())));
//
//         let num_val = vec![VVal {
//             val: Some(Value::String("12.3".to_string())),
//         }];
//
//         let res = num_val.validate_function_result(
//             vec![FunctionResult {
//                 name: Some("a".to_string()),
//                 tpe: Some(golem_api_grpc::proto::golem::template::Type {
//                     r#type: Some(Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::F64 as i32,
//                     })),
//                 }),
//             }],
//             CallingConvention::Stdio,
//         );
//
//         assert!(res.is_ok_and(|r| r == Value::Number(serde_json::Number::from_f64(12.3).unwrap())));
//
//         let bool_val = vec![VVal {
//             val: Some(Value::String("true".to_string())),
//         }];
//
//         let res = bool_val.validate_function_result(
//             vec![FunctionResult {
//                 name: Some("a".to_string()),
//                 tpe: Some(golem_api_grpc::proto::golem::template::Type {
//                     r#type: Some(Type::Primitive(TypePrimitive {
//                         primitive: PrimitiveType::Bool as i32,
//                     })),
//                 }),
//             }],
//             CallingConvention::Stdio,
//         );
//
//         assert!(res.is_ok_and(|r| r == Value::Bool(true)));
//     }
//
//     #[test]
//     fn json_null_works_as_none() {
//         let json = Value::Null;
//         let result = validate_function_parameter(
//             &json,
//             Type::Option(Box::new(
//                 golem_api_grpc::proto::golem::template::TypeOption {
//                     elem: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
//                         r#type: Some(Type::Primitive(TypePrimitive {
//                             primitive: PrimitiveType::Str as i32,
//                         })),
//                     })),
//                 },
//             )),
//         );
//         assert_eq!(
//             result,
//             Ok(Value::Option(Box::new(ValOption {
//                 discriminant: 0,
//                 value: None
//             })))
//         );
//     }
//
//     #[test]
//     fn missing_field_works_as_none() {
//         let json = Value::Object(
//             vec![("x".to_string(), Value::String("a".to_string()))]
//                 .into_iter()
//                 .collect(),
//         );
//         let result = validate_function_parameter(
//             &json,
//             Type::Record(TypeRecord {
//                 fields: vec![
//                     NameTypePair {
//                         name: "x".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Primitive(TypePrimitive {
//                                 primitive: PrimitiveType::Str as i32,
//                             })),
//                         }),
//                     },
//                     NameTypePair {
//                         name: "y".to_string(),
//                         typ: Some(golem_api_grpc::proto::golem::template::Type {
//                             r#type: Some(Type::Option(Box::new(
//                                 golem_api_grpc::proto::golem::template::TypeOption {
//                                     elem: Some(Box::new(
//                                         golem_api_grpc::proto::golem::template::Type {
//                                             r#type: Some(Type::Primitive(TypePrimitive {
//                                                 primitive: PrimitiveType::Str as i32,
//                                             })),
//                                         },
//                                     )),
//                                 },
//                             ))),
//                         }),
//                     },
//                 ],
//             }),
//         );
//         assert_eq!(
//             result,
//             Ok(Value::Record(ValRecord {
//                 values: vec![
//                     VVal {
//                         val: Some(Value::String("a".to_string()))
//                     },
//                     VVal {
//                         val: Some(Value::Option(Box::new(ValOption {
//                             discriminant: 0,
//                             value: None
//                         })))
//                     }
//                 ]
//             }))
//         );
//     }
//
//     #[test]
//     fn test_get_record() {
//         // Test case where all keys are present
//         let input_json = json!({
//             "key1": "value1",
//             "key2": "value2",
//         });
//
//         let key1 = "key1".to_string();
//         let key2 = "key2".to_string();
//
//         let name_type_pairs: Vec<(&String, &AnalysedType)> = vec![
//             (
//                 &key1,
//                 &Type::Primitive(TypePrimitive {
//                     primitive: PrimitiveType::Str as i32,
//                 }),
//             ),
//             (
//                 &key2,
//                 &Type::Primitive(TypePrimitive {
//                     primitive: PrimitiveType::Str as i32,
//                 }),
//             ),
//         ];
//
//         let result = get_record(&input_json, name_type_pairs.clone());
//         let expected_result = Ok(ValRecord {
//             values: vec![
//                 VVal {
//                     val: Some(Value::String("value1".to_string())),
//                 },
//                 VVal {
//                     val: Some(Value::String("value2".to_string())),
//                 },
//             ],
//         });
//         assert_eq!(result, expected_result);
//
//         // Test case where a key is missing
//         let input_json = json!({
//             "key1": "value1",
//         });
//
//         let result = get_record(&input_json, name_type_pairs.clone());
//         assert!(result.is_err());
//     }
// }
