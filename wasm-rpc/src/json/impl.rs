// Copyright 2024-2025 Golem Cloud
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

use std::collections::HashMap;
use std::ops::Deref;
use std::str::FromStr;

use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};
use golem_wasm_ast::analysis::{
    AnalysedResourceId, AnalysedResourceMode, AnalysedType, NameOptionTypePair, NameTypePair,
    TypeEnum, TypeFlags, TypeHandle, TypeList, TypeOption, TypeRecord, TypeResult, TypeTuple,
    TypeVariant,
};
use serde_json::{Number, Value as JsonValue};

use crate::json::TypeAnnotatedValueJsonExtensions;
use crate::protobuf;
use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::protobuf::typed_result::ResultValue;

impl TypeAnnotatedValueJsonExtensions for TypeAnnotatedValue {
    fn parse_with_type(json_val: &JsonValue, typ: &AnalysedType) -> Result<Self, Vec<String>> {
        match typ {
            AnalysedType::Bool(_) => get_bool(json_val),
            AnalysedType::S8(_) => get_s8(json_val),
            AnalysedType::U8(_) => get_u8(json_val),
            AnalysedType::S16(_) => get_s16(json_val),
            AnalysedType::U16(_) => get_u16(json_val),
            AnalysedType::S32(_) => get_s32(json_val),
            AnalysedType::U32(_) => get_u32(json_val),
            AnalysedType::S64(_) => get_s64(json_val),
            AnalysedType::U64(_) => get_u64(json_val),
            AnalysedType::F64(_) => get_f64(json_val),
            AnalysedType::F32(_) => get_f32(json_val),
            AnalysedType::Chr(_) => get_char(json_val),
            AnalysedType::Str(_) => get_string(json_val),
            AnalysedType::Enum(TypeEnum { cases }) => get_enum(json_val, cases),
            AnalysedType::Flags(TypeFlags { names }) => get_flag(json_val, names),
            AnalysedType::List(TypeList { inner }) => get_list(json_val, inner),
            AnalysedType::Option(TypeOption { inner }) => get_option(json_val, inner),
            AnalysedType::Result(TypeResult { ok, err }) => get_result(json_val, ok, err),
            AnalysedType::Record(TypeRecord { fields }) => get_record(json_val, fields),
            AnalysedType::Variant(TypeVariant { cases }) => get_variant(json_val, cases),
            AnalysedType::Tuple(TypeTuple { items }) => get_tuple(json_val, items),
            AnalysedType::Handle(TypeHandle { resource_id, mode }) => {
                get_handle(json_val, resource_id.clone(), mode.clone())
            }
        }
    }

    fn to_json_value(&self) -> JsonValue {
        match self {
            TypeAnnotatedValue::Bool(bool) => JsonValue::Bool(*bool),
            TypeAnnotatedValue::Flags(protobuf::TypedFlags { typ: _, values }) => JsonValue::Array(
                values
                    .iter()
                    .map(|x| JsonValue::String(x.clone()))
                    .collect(),
            ),
            TypeAnnotatedValue::S8(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::U8(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::S16(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::U16(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::S32(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::U32(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::S64(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::U64(value) => JsonValue::Number(Number::from(*value)),
            TypeAnnotatedValue::F32(value) => {
                JsonValue::Number(Number::from_f64(*value as f64).unwrap())
            }
            TypeAnnotatedValue::F64(value) => JsonValue::Number(Number::from_f64(*value).unwrap()),
            TypeAnnotatedValue::Char(value) => JsonValue::Number(Number::from(*value as u32)),
            TypeAnnotatedValue::Str(value) => JsonValue::String(value.clone()),
            TypeAnnotatedValue::Enum(protobuf::TypedEnum { typ: _, value }) => {
                JsonValue::String(value.clone())
            }
            TypeAnnotatedValue::Option(option) => match &option.value {
                Some(value) => value.clone().type_annotated_value.unwrap().to_json_value(),
                None => JsonValue::Null,
            },
            TypeAnnotatedValue::Tuple(protobuf::TypedTuple { typ: _, value }) => {
                let values: Vec<serde_json::Value> = value
                    .iter()
                    .map(|v| v.type_annotated_value.clone().unwrap().to_json_value())
                    .collect();
                JsonValue::Array(values)
            }
            TypeAnnotatedValue::List(protobuf::TypedList { typ: _, values }) => {
                let values: Vec<serde_json::Value> = values
                    .iter()
                    .map(|v| v.type_annotated_value.clone().unwrap().to_json_value())
                    .collect();
                JsonValue::Array(values)
            }

            TypeAnnotatedValue::Record(protobuf::TypedRecord { typ: _, value }) => {
                let mut map = serde_json::Map::new();
                for name_value in value {
                    map.insert(
                        name_value.name.clone(),
                        name_value
                            .value
                            .clone()
                            .unwrap()
                            .type_annotated_value
                            .unwrap()
                            .to_json_value(),
                    );
                }
                JsonValue::Object(map)
            }

            TypeAnnotatedValue::Variant(variant) => {
                let mut map = serde_json::Map::new();
                map.insert(
                    variant.case_name.clone(),
                    variant
                        .case_value
                        .as_ref()
                        .map(|x| {
                            let value = x.clone().deref().type_annotated_value.clone().unwrap();
                            value.to_json_value()
                        })
                        .unwrap_or(JsonValue::Null),
                );
                JsonValue::Object(map)
            }

            TypeAnnotatedValue::Result(result0) => {
                let mut map = serde_json::Map::new();

                let result_value = result0.result_value.clone().unwrap();

                match result_value {
                    ResultValue::OkValue(value) => {
                        map.insert(
                            "ok".to_string(),
                            value
                                .type_annotated_value
                                .map_or(JsonValue::Null, |v| v.to_json_value()),
                        );
                    }
                    ResultValue::ErrorValue(value) => {
                        map.insert(
                            "err".to_string(),
                            value
                                .type_annotated_value
                                .map_or(JsonValue::Null, |v| v.to_json_value()),
                        );
                    }
                }

                JsonValue::Object(map)
            }

            TypeAnnotatedValue::Handle(protobuf::TypedHandle {
                typ: _,
                uri,
                resource_id,
            }) => JsonValue::String(format!("{}/{}", uri, resource_id)),
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
    .map(|num| TypeAnnotatedValue::S8(num.to_i32().expect("Failed to convert BigDecimal to i8")))
}

fn get_u8(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u8(u8::MIN).expect("Failed to convert u8::MIN to BigDecimal"),
        BigDecimal::from_u8(u8::MAX).expect("Failed to convert u8::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::U8(num.to_u32().expect("Failed to convert BigDecimal to u8")))
}

fn get_s16(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i16(i16::MIN).expect("Failed to convert i16::MIN to BigDecimal"),
        BigDecimal::from_i16(i16::MAX).expect("Failed to convert i16::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::S16(num.to_i32().expect("Failed to convert BigDecimal to i16")))
}

fn get_u16(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u16(u16::MIN).expect("Failed to convert u16::MIN to BigDecimal"),
        BigDecimal::from_u16(u16::MAX).expect("Failed to convert u16::MAX to BigDecimal"),
    )
    .map(|num| TypeAnnotatedValue::U16(num.to_u32().expect("Failed to convert BigDecimal to u16")))
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
            Ok(TypeAnnotatedValue::Char(num_u64 as i32))
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
        match TypeAnnotatedValue::parse_with_type(json, tpe) {
            Ok(result) => vals.push(result),
            Err(errs) => errors.extend(errs),
        }
    }

    let tuple = protobuf::TypedTuple {
        typ: types.iter().map(|t| t.into()).collect(),
        value: vals
            .iter()
            .map(|v| protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(v.clone()),
            })
            .collect(),
    };

    if errors.is_empty() {
        Ok(TypeAnnotatedValue::Tuple(tuple))
    } else {
        Err(errors)
    }
}

fn get_option(
    input_json: &JsonValue,
    tpe: &AnalysedType,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    match input_json.as_null() {
        Some(_) => {
            let option = protobuf::TypedOption {
                typ: Some(tpe.into()),
                value: None,
            };

            Ok(TypeAnnotatedValue::Option(Box::new(option)))
        }

        None => TypeAnnotatedValue::parse_with_type(input_json, tpe).map(|result| {
            let option = protobuf::TypedOption {
                typ: Some(tpe.into()),
                value: Some(Box::new(protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(result),
                })),
            };

            TypeAnnotatedValue::Option(Box::new(option))
        }),
    }
}

fn get_list(input_json: &JsonValue, tpe: &AnalysedType) -> Result<TypeAnnotatedValue, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<TypeAnnotatedValue> = vec![];

    for json in json_array {
        match TypeAnnotatedValue::parse_with_type(json, tpe) {
            Ok(result) => vals.push(result),
            Err(errs) => errors.extend(errs),
        }
    }

    let list = protobuf::TypedList {
        typ: Some(tpe.into()),
        values: vals
            .iter()
            .map(|v| protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(v.clone()),
            })
            .collect(),
    };

    if errors.is_empty() {
        Ok(TypeAnnotatedValue::List(list))
    } else {
        Err(errors)
    }
}

fn get_enum(input_json: &JsonValue, names: &[String]) -> Result<TypeAnnotatedValue, Vec<String>> {
    let input_enum_value = input_json
        .as_str()
        .ok_or(vec![format!("Input {} is not string", input_json)])?;

    let enum_value = protobuf::TypedEnum {
        typ: names.to_vec(),
        value: input_enum_value.to_string(),
    };
    if names.contains(&input_enum_value.to_string()) {
        Ok(TypeAnnotatedValue::Enum(enum_value))
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
            TypeAnnotatedValue::parse_with_type(input_json, typ).map(|v| Some(Box::new(v)))
        } else if input_json.is_null() {
            Ok(None)
        } else {
            Err(vec![
                "The type of ok is absent, but some JSON value was provided".to_string(),
            ])
        }
    }

    match input_json.get("ok") {
        Some(value) => {
            let value = validate(ok_type, value)?;

            let result_value = ResultValue::OkValue(Box::new(protobuf::TypeAnnotatedValue {
                type_annotated_value: value.map(|value| value.deref().clone()),
            }));

            let typed_result = protobuf::TypedResult {
                ok: ok_type.clone().map(|x| x.deref().into()),
                error: err_type.clone().map(|x| x.deref().into()),
                result_value: Some(result_value),
            };

            Ok(TypeAnnotatedValue::Result(Box::new(typed_result)))
        }
        None => match input_json.get("err") {
            Some(value) => {
                let value = validate(err_type, value)?;

                let result_value =
                    ResultValue::ErrorValue(Box::new(protobuf::TypeAnnotatedValue {
                        type_annotated_value: value.map(|value| value.deref().clone()),
                    }));

                let typed_result = protobuf::TypedResult {
                    ok: ok_type.clone().map(|x| x.deref().into()),
                    error: err_type.clone().map(|x| x.deref().into()),
                    result_value: Some(result_value),
                };

                Ok(TypeAnnotatedValue::Result(Box::new(typed_result)))
            }
            None => Err(vec![
                "Failed to retrieve either ok value or err value".to_string()
            ]),
        },
    }
}

fn get_record(
    input_json: &JsonValue,
    name_type_pairs: &[NameTypePair],
) -> Result<TypeAnnotatedValue, Vec<String>> {
    let json_map = input_json.as_object().ok_or(vec![format!(
        "The input {} is not a json object",
        input_json
    )])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<(String, TypeAnnotatedValue)> = vec![];

    for NameTypePair { name, typ } in name_type_pairs {
        if let Some(json_value) = json_map.get(name) {
            match TypeAnnotatedValue::parse_with_type(json_value, typ) {
                Ok(result) => vals.push((name.clone(), result)),
                Err(value_errors) => errors.extend(
                    value_errors
                        .iter()
                        .map(|err| format!("Invalid value for the key {}. Error: {}", name, err))
                        .collect::<Vec<_>>(),
                ),
            }
        } else {
            match typ {
                AnalysedType::Option(_) => {
                    let option = protobuf::TypedOption {
                        typ: Some(typ.into()),
                        value: None,
                    };

                    vals.push((name.clone(), TypeAnnotatedValue::Option(Box::new(option))))
                }
                _ => errors.push(format!("Key '{}' not found in json_map", name)),
            }
        }
    }

    if errors.is_empty() {
        let name_type_pairs = name_type_pairs
            .iter()
            .map(|pair| protobuf::NameTypePair {
                name: pair.name.clone(),
                typ: Some((&pair.typ).into()),
            })
            .collect::<Vec<_>>();

        let name_value_pairs = vals
            .iter()
            .map(|(name, value)| protobuf::NameValuePair {
                name: name.clone(),
                value: Some(protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(value.clone()),
                }),
            })
            .collect::<Vec<_>>();

        let record = protobuf::TypedRecord {
            typ: name_type_pairs,
            value: name_value_pairs,
        };

        Ok(TypeAnnotatedValue::Record(record))
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
        let flags = protobuf::TypedFlags {
            typ: names.to_vec(),
            values: vals,
        };
        Ok(TypeAnnotatedValue::Flags(flags))
    } else {
        Err(errors)
    }
}

fn get_variant(
    input_json: &JsonValue,
    types: &[NameOptionTypePair],
) -> Result<TypeAnnotatedValue, Vec<String>> {
    let mut possible_mapping_indexed: HashMap<&String, &Option<AnalysedType>> = HashMap::new();

    for NameOptionTypePair {
        name,
        typ: optional_type,
    } in types.iter()
    {
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
            let result = TypeAnnotatedValue::parse_with_type(json, tpe)?;
            let variant = protobuf::TypedVariant {
                typ: Some(protobuf::TypeVariant {
                    cases: types
                        .iter()
                        .map(|pair| protobuf::NameOptionTypePair {
                            name: pair.name.clone(),
                            typ: pair.typ.as_ref().map(|t| t.into()),
                        })
                        .collect(),
                }),
                case_name: key.clone(),
                case_value: Some(Box::new(protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(result),
                })),
            };

            Ok(TypeAnnotatedValue::Variant(Box::new(variant)))
        }
        Some(None) if json.is_null() => {
            let variant = protobuf::TypedVariant {
                typ: Some(protobuf::TypeVariant {
                    cases: types
                        .iter()
                        .map(|pair| protobuf::NameOptionTypePair {
                            name: pair.name.clone(),
                            typ: pair.typ.as_ref().map(|t| t.into()),
                        })
                        .collect(),
                }),
                case_name: key.clone(),
                case_value: None,
            };

            Ok(TypeAnnotatedValue::Variant(Box::new(variant)))
        }
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

                        let handle = protobuf::TypedHandle {
                            typ: Some(protobuf::TypeHandle {
                                resource_id: id.0,
                                mode: match resource_mode {
                                    AnalysedResourceMode::Owned => 1,
                                    AnalysedResourceMode::Borrowed => 2,
                                },
                            }),
                            uri,
                            resource_id
                        };
                        Ok(TypeAnnotatedValue::Handle(handle))
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

#[cfg(test)]
mod tests {
    use test_r::test;

    use std::collections::HashSet;

    use golem_wasm_ast::analysis::analysed_type::{
        bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result, s16, s32,
        s64, s8, str, tuple, u16, u32, u64, u8, variant,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use proptest::prelude::*;
    use serde_json::{Number, Value as JsonValue};

    use crate::json::TypeAnnotatedValueJsonExtensions;
    use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
    use crate::{TypeAnnotatedValueConstructors, Value};

    fn validate_function_result(
        val: Value,
        expected_type: &AnalysedType,
    ) -> Result<JsonValue, Vec<String>> {
        TypeAnnotatedValue::create(&val, expected_type).map(|result| result.to_json_value())
    }

    fn validate_function_parameter(
        json: &JsonValue,
        expected_type: &AnalysedType,
    ) -> Result<Value, Vec<String>> {
        match TypeAnnotatedValue::parse_with_type(json, expected_type) {
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
            let result = validate_function_parameter(&json, &u8());
            prop_assert_eq!(result, Ok(Value::U8(value)));
        }

        #[test]
        fn test_u16_param(value: u16) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &u16());
            prop_assert_eq!(result, Ok(Value::U16(value)));
        }

        #[test]
        fn test_u32_param(value: u32) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &u32());
            prop_assert_eq!(result, Ok(Value::U32(value)));
        }

        #[test]
        fn test_u64_param(value: u64) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &u64());
            prop_assert_eq!(result, Ok(Value::U64(value)));
        }

        #[test]
        fn test_s8_param(value: i8) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &s8());
            prop_assert_eq!(result, Ok(Value::S8(value)));
        }

        #[test]
        fn test_s16_param(value: i16) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &s16());
            prop_assert_eq!(result, Ok(Value::S16(value)));
        }

        #[test]
        fn test_s32_param(value: i32) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &s32());
            prop_assert_eq!(result, Ok(Value::S32(value)));
        }

        #[test]
        fn test_s64_param(value: i64) {
            let json = JsonValue::Number(Number::from(value));
            let result = validate_function_parameter(&json, &s64());
            prop_assert_eq!(result, Ok(Value::S64(value)));
        }

        #[test]
        fn test_f32_param(value: f32) {
            let json = JsonValue::Number(Number::from_f64(value as f64).unwrap());
            let result = validate_function_parameter(&json, &f32());
            prop_assert_eq!(result, Ok(Value::F32(value)));
        }

        #[test]
        fn test_f64_param(value: f64) {
            let json = JsonValue::Number(Number::from_f64(value).unwrap());
            let result = validate_function_parameter(&json, &f64());
            prop_assert_eq!(result, Ok(Value::F64(value)));
        }

        #[test]
        fn test_char_param(value: char) {
            let json = JsonValue::Number(Number::from(value as u32));
            let result = validate_function_parameter(&json, &chr());
            prop_assert_eq!(result, Ok(Value::Char(value)));
        }

        #[test]
        fn test_string_param(value: String) {
            let json = JsonValue::String(value.clone());
            let result = validate_function_parameter(&json, &str());
            prop_assert_eq!(result, Ok(Value::String(value)));
        }

        #[test]
        fn test_list_u8_param(value: Vec<u8>) {
            let json = JsonValue::Array(value.iter().map(|v| JsonValue::Number(Number::from(*v))).collect());
            let result = validate_function_parameter(&json, &list(u8()));
            prop_assert_eq!(result, Ok(Value::List(value.into_iter().map(Value::U8).collect())));
        }

        #[test]
        fn test_list_list_u64_param(value: Vec<Vec<u64>>) {
            let json = JsonValue::Array(value.iter().map(|v| JsonValue::Array(v.iter().map(|n| JsonValue::Number(Number::from(*n))).collect())).collect());
            let result = validate_function_parameter(&json, &list(list(u64())));
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
            let result = validate_function_parameter(&json, &tuple(vec![
                s32(),
                chr(),
                str(),
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
                pairs.iter().map(|(k, _)| k).collect::<HashSet<_>>().len() == pairs.len())
        ) {
            let json = JsonValue::Object(
                value.iter().map(|(k, v)| (k.clone(), JsonValue::Bool(*v))).collect());
            let result = validate_function_parameter(&json, &record(
                value.iter().map(|(k, _)| field(k, bool())).collect()
            ));
            prop_assert_eq!(result, Ok(Value::Record(
                value.iter().map(|(_, v)| Value::Bool(*v)).collect())));
        }

        #[test]
        fn test_flags_param(value in
            any::<Vec<(String, bool)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<HashSet<_>>().len() == pairs.len())
            ) {
            let enabled: Vec<String> = value.iter().filter(|(_, v)| *v).map(|(k, _)| k.clone()).collect();
            let json = JsonValue::Array(enabled.iter().map(|v| JsonValue::String(v.clone())).collect());
            let result = validate_function_parameter(&json, &flags(&value.iter().map(|(k, _)| k.as_str()).collect::<Vec<&str>>()));
            prop_assert_eq!(result, Ok(Value::Flags(
                value.iter().map(|(_, v)| *v).collect())
            ));
        }

        #[test]
        fn test_enum_param((names, idx) in (any::<HashSet<String>>().prop_filter("Name list is non empty", |names| !names.is_empty()), any::<usize>())) {
            let names: Vec<String> = names.into_iter().collect();
            let idx = idx % names.len();
            let json = JsonValue::String(names[idx].clone());
            let result = validate_function_parameter(&json, &r#enum(&names.iter().map(|s| s.as_str()).collect::<Vec<&str>>()));
            prop_assert_eq!(result, Ok(Value::Enum(idx as u32)));
        }

        #[test]
        fn test_option_string_param(value: Option<String>) {
            let json = match &value {
                Some(v) => JsonValue::String(v.clone()),
                None => JsonValue::Null,
            };
            let result = validate_function_parameter(&json, &option(str()));
            prop_assert_eq!(result, Ok(Value::Option(value.map(|v| Box::new(Value::String(v))))));
        }

        #[test]
        fn test_result_option_s32_string_param(value: Result<Option<i32>, String>) {
            let json = match &value {
                Ok(None) => JsonValue::Object(vec![("ok".to_string(), JsonValue::Null)].into_iter().collect()),
                Ok(Some(v)) => JsonValue::Object(vec![("ok".to_string(), JsonValue::Number(Number::from(*v)))].into_iter().collect()),
                Err(e) => JsonValue::Object(vec![("err".to_string(), JsonValue::String(e.clone()))].into_iter().collect()),
            };
            let result = validate_function_parameter(&json, &result(option(s32()), str()));
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
            let result = validate_function_parameter(&json, &variant(vec![
                case("first", tuple(vec![u32(), u32()])),
                case("second", str()),
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
            let expected_type = u8();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_u16_result(value: u16) {
            let result = Value::U16(value);
            let expected_type = u16();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_u32_result(value: u32) {
            let result = Value::U32(value);
            let expected_type = u32();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_u64_result(value: u64) {
            let result = Value::U64(value);
            let expected_type = u64();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s8_result(value: i8) {
            let result = Value::S8(value);
            let expected_type = s8();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s16_result(value: i16) {
            let result = Value::S16(value);
            let expected_type = s16();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s32_result(value: i32) {
            let result = Value::S32(value);
            let expected_type = s32();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_s64_result(value: i64) {
            let result = Value::S64(value);
            let expected_type = s64();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value))));
        }

        #[test]
        fn test_f32_result(value: f32) {
            let result = Value::F32(value);
            let expected_type = f32();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from_f64(value as f64).unwrap())));
        }

        #[test]
        fn test_f64_result(value: f64) {
            let result = Value::F64(value);
            let expected_type = f64();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from_f64(value).unwrap())));
        }

        #[test]
        fn test_char_result(value: char) {
            let result = Value::Char(value);
            let expected_type = chr();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Number(Number::from(value as u32))));
        }

        #[test]
        fn test_string_result(value: String) {
            let result = Value::String(value.clone());
            let expected_type = str();
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::String(value)));
        }

        #[test]
        fn test_list_i32_result(value: Vec<i32>) {
            let result = Value::List(value.iter().map(|v| Value::S32(*v)).collect());
            let expected_type = list(s32());
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Array(value.into_iter().map(|v| JsonValue::Number(Number::from(v))).collect())));
        }

        #[test]
        fn test_tuple_string_bool_result(value: (String, bool)) {
            let result = Value::Tuple(vec![Value::String(value.0.clone()), Value::Bool(value.1)]);
            let expected_type = tuple(vec![str(), bool()]);
            let json = validate_function_result(result, &expected_type);
            prop_assert_eq!(json, Ok(JsonValue::Array(vec![JsonValue::String(value.0), JsonValue::Bool(value.1)])));
        }

        #[test]
        fn test_record_list_u8_fields(value in any::<Vec<(String, Vec<u8>)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<HashSet<_>>().len() == pairs.len())
        ) {
            let result = Value::Record(
                value.iter().map(|(_, v)| Value::List(v.iter().map(|n| Value::U8(*n)).collect())).collect());
            let expected_type = record(
                value.iter().map(|(k, _)| field(k, list(u8()))).collect()
            );
            let json = validate_function_result(result, &expected_type);
            let expected_json = JsonValue::Object(
                value.iter().map(|(k, v)| (k.clone(), JsonValue::Array(v.iter().map(|n| JsonValue::Number(Number::from(*n))).collect()))).collect());
            prop_assert_eq!(json, Ok(expected_json));
        }

        #[test]
        fn test_flags_result(pairs in
            any::<Vec<(String, bool)>>().prop_filter("Keys are distinct", |pairs|
                pairs.iter().map(|(k, _)| k).collect::<HashSet<_>>().len() == pairs.len())
            ) {
            let enabled: Vec<String> = pairs.iter().filter(|(_, v)| *v).map(|(k, _)| k.clone()).collect();
            let value = Value::Flags(pairs.iter().map(|(_, v)| *v).collect());
            let result = validate_function_result(value, &flags(&pairs.iter().map(|(k, _)| k.as_str()).collect::<Vec<&str>>()));
            prop_assert_eq!(result, Ok(
                JsonValue::Array(enabled.iter().map(|v| JsonValue::String(v.clone())).collect())
            ));
        }

        #[test]
        fn test_enum_result((names, idx) in (any::<HashSet<String>>().prop_filter("Name list is non empty", |names| !names.is_empty()), any::<usize>())) {
            let names: Vec<String> = names.into_iter().collect();
            let idx = idx % names.len();
            let value = Value::Enum(idx as u32);
            let result = validate_function_result(value, &r#enum(&names.iter().map(|s| s.as_str()).collect::<Vec<&str>>()));
            prop_assert_eq!(result, Ok(JsonValue::String(names[idx].clone())));
        }

        #[test]
        fn test_option_string_result(opt: Option<String>) {
            let value = Value::Option(opt.clone().map(|v| Box::new(Value::String(v))));
            let result = validate_function_result(value, &option(str()));
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
            let result = validate_function_result(value, &variant(vec![
                case("first", tuple(vec![u32(), u32()])),
                case("second", str()),
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
        let result = validate_function_parameter(&json, &option(str()));
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
            &record(vec![field("x", str()), field("y", option(str()))]),
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
