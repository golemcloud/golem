// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::json::ValueAndTypeJsonExtensions;
use crate::{IntoValueAndType, Value, ValueAndType};
use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};
use golem_wasm_ast::analysis::analysed_type::{list, option, record, tuple, variant};
use golem_wasm_ast::analysis::{
    AnalysedResourceId, AnalysedResourceMode, AnalysedType, NameOptionTypePair, NameTypePair,
    TypeEnum, TypeFlags, TypeHandle, TypeList, TypeOption, TypeRecord, TypeResult, TypeTuple,
    TypeVariant,
};
use serde_json::{Number, Value as JsonValue};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

impl ValueAndTypeJsonExtensions for ValueAndType {
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
            AnalysedType::Enum(TypeEnum { cases, .. }) => get_enum(json_val, cases),
            AnalysedType::Flags(TypeFlags { names, .. }) => get_flag(json_val, names),
            AnalysedType::List(TypeList { inner, .. }) => get_list(json_val, inner),
            AnalysedType::Option(TypeOption { inner, .. }) => get_option(json_val, inner),
            AnalysedType::Result(TypeResult { ok, err, .. }) => get_result(json_val, ok, err),
            AnalysedType::Record(TypeRecord { fields, .. }) => get_record(json_val, fields),
            AnalysedType::Variant(TypeVariant { cases, .. }) => get_variant(json_val, cases),
            AnalysedType::Tuple(TypeTuple { items, .. }) => get_tuple(json_val, items),
            AnalysedType::Handle(TypeHandle {
                resource_id, mode, ..
            }) => get_handle(json_val, *resource_id, mode.clone()),
        }
    }

    fn to_json_value(&self) -> Result<JsonValue, String> {
        match (&self.typ, &self.value) {
            (AnalysedType::Bool(_), Value::Bool(bool_val)) => Ok(JsonValue::Bool(*bool_val)),
            (AnalysedType::S8(_), Value::S8(value)) => Ok(JsonValue::Number(Number::from(*value))),
            (AnalysedType::U8(_), Value::U8(value)) => Ok(JsonValue::Number(Number::from(*value))),
            (AnalysedType::S16(_), Value::S16(value)) => {
                Ok(JsonValue::Number(Number::from(*value)))
            }
            (AnalysedType::U16(_), Value::U16(value)) => {
                Ok(JsonValue::Number(Number::from(*value)))
            }
            (AnalysedType::S32(_), Value::S32(value)) => {
                Ok(JsonValue::Number(Number::from(*value)))
            }
            (AnalysedType::U32(_), Value::U32(value)) => {
                Ok(JsonValue::Number(Number::from(*value)))
            }
            (AnalysedType::S64(_), Value::S64(value)) => {
                Ok(JsonValue::Number(Number::from(*value)))
            }
            (AnalysedType::U64(_), Value::U64(value)) => {
                Ok(JsonValue::Number(Number::from(*value)))
            }
            (AnalysedType::F32(_), Value::F32(value)) => Ok(JsonValue::Number(
                Number::from_f64(*value as f64)
                    .ok_or_else(|| "Failed to encode f32 as JSON number".to_string())?,
            )),
            (AnalysedType::F64(_), Value::F64(value)) => Ok(JsonValue::Number(
                Number::from_f64(*value)
                    .ok_or_else(|| "Failed to encode f64 as JSON number".to_string())?,
            )),
            (AnalysedType::Chr(_), Value::Char(value)) => {
                Ok(JsonValue::Number(Number::from(*value as u32)))
            }
            (AnalysedType::Str(_), Value::String(value)) => Ok(JsonValue::String(value.clone())),
            (AnalysedType::Enum(TypeEnum { cases, .. }), Value::Enum(value)) => {
                if let Some(case) = cases.get(*value as usize) {
                    Ok(JsonValue::String(case.clone()))
                } else {
                    Err(format!("Invalid enum index '{value}'"))
                }
            }
            (AnalysedType::Flags(TypeFlags { names, .. }), Value::Flags(value)) => {
                let values: Vec<JsonValue> = value
                    .iter()
                    .zip(names)
                    .filter_map(|(enabled, name)| {
                        if *enabled {
                            Some(JsonValue::String(name.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(JsonValue::Array(values))
            }
            (AnalysedType::Option(TypeOption { inner, .. }), Value::Option(value)) => {
                if let Some(inner_value) = value {
                    let inner_vnt = ValueAndType::new((**inner_value).clone(), (**inner).clone());
                    inner_vnt.to_json_value()
                } else {
                    Ok(JsonValue::Null)
                }
            }
            (AnalysedType::Tuple(TypeTuple { items, .. }), Value::Tuple(values)) => {
                let item_jsons = items
                    .iter()
                    .zip(values)
                    .map(|(item_type, item_value)| {
                        let item_vnt = ValueAndType::new(item_value.clone(), item_type.clone());
                        item_vnt.to_json_value()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(JsonValue::Array(item_jsons))
            }
            (AnalysedType::List(TypeList { inner, .. }), Value::List(values)) => {
                let item_jsons = values
                    .iter()
                    .map(|item_value| {
                        let item_vnt = ValueAndType::new(item_value.clone(), (**inner).clone());
                        item_vnt.to_json_value()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(JsonValue::Array(item_jsons))
            }
            (AnalysedType::Record(TypeRecord { fields, .. }), Value::Record(field_values)) => {
                let fields = fields
                    .iter()
                    .zip(field_values)
                    .map(|(field_type, field_value)| {
                        let field_vnt =
                            ValueAndType::new(field_value.clone(), field_type.typ.clone());
                        field_vnt
                            .to_json_value()
                            .map(|json| (field_type.name.clone(), json))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(JsonValue::Object(fields.into_iter().collect()))
            }
            (
                AnalysedType::Variant(TypeVariant { cases, .. }),
                Value::Variant {
                    case_idx,
                    case_value,
                },
            ) => {
                if let Some(case) = cases.get(*case_idx as usize) {
                    let mut map = serde_json::Map::new();
                    match &case.typ {
                        Some(case_typ) => {
                            if let Some(value) = case_value {
                                let value_vnt =
                                    ValueAndType::new((**value).clone(), case_typ.clone());
                                map.insert(case.name.clone(), value_vnt.to_json_value()?);
                            } else {
                                map.insert(case.name.clone(), JsonValue::Null);
                            }
                        }
                        None => {
                            map.insert(case.name.clone(), JsonValue::Null);
                        }
                    }
                    Ok(JsonValue::Object(map))
                } else {
                    Err(format!("Invalid variant index '{case_idx}'"))
                }
            }
            (AnalysedType::Result(TypeResult { ok, err, .. }), Value::Result(result)) => {
                match result {
                    Ok(None) => Ok(JsonValue::Object(
                        vec![("ok".to_string(), JsonValue::Null)]
                            .into_iter()
                            .collect(),
                    )),
                    Ok(Some(ok_value)) => {
                        if let Some(ok_type) = ok {
                            let ok_vnt =
                                ValueAndType::new((**ok_value).clone(), (**ok_type).clone());
                            Ok(JsonValue::Object(
                                vec![("ok".to_string(), ok_vnt.to_json_value()?)]
                                    .into_iter()
                                    .collect(),
                            ))
                        } else {
                            Err("Missing ok value in Result".to_string())
                        }
                    }
                    Err(None) => Ok(JsonValue::Object(
                        vec![("err".to_string(), JsonValue::Null)]
                            .into_iter()
                            .collect(),
                    )),
                    Err(Some(err_value)) => {
                        if let Some(err_type) = err {
                            let err_vnt =
                                ValueAndType::new((**err_value).clone(), (**err_type).clone());
                            Ok(JsonValue::Object(
                                vec![("err".to_string(), err_vnt.to_json_value()?)]
                                    .into_iter()
                                    .collect(),
                            ))
                        } else {
                            Err("Missing err value in Result".to_string())
                        }
                    }
                }
            }
            (AnalysedType::Handle(TypeHandle { .. }), Value::Handle { uri, resource_id }) => {
                Ok(JsonValue::String(format!("{uri}/{resource_id}")))
            }
            _ => Err(format!(
                "Type and value mismatch (type is {:?}, value is {:?})",
                self.typ, self.value
            )),
        }
    }
}

fn get_bool(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    match json {
        JsonValue::Bool(bool_val) => Ok(bool_val.into_value_and_type()),
        _ => {
            let type_description = type_description(json);
            Err(vec![format!("expected bool, found {}", type_description)])
        }
    }
}

fn get_s8(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i8(i8::MIN).expect("Failed to convert i8::MIN to BigDecimal"),
        BigDecimal::from_i8(i8::MAX).expect("Failed to convert i8::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_i8()
            .ok_or_else(|| vec!["Failed to convert number to i8".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_u8(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u8(u8::MIN).expect("Failed to convert u8::MIN to BigDecimal"),
        BigDecimal::from_u8(u8::MAX).expect("Failed to convert u8::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_u8()
            .ok_or_else(|| vec!["Failed to convert number to u8".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_s16(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i16(i16::MIN).expect("Failed to convert i16::MIN to BigDecimal"),
        BigDecimal::from_i16(i16::MAX).expect("Failed to convert i16::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_i16()
            .ok_or_else(|| vec!["Failed to convert number to i16".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_u16(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u16(u16::MIN).expect("Failed to convert u16::MIN to BigDecimal"),
        BigDecimal::from_u16(u16::MAX).expect("Failed to convert u16::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_u16()
            .ok_or_else(|| vec!["Failed to convert number to u16".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_s32(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i32(i32::MIN).expect("Failed to convert i32::MIN to BigDecimal"),
        BigDecimal::from_i32(i32::MAX).expect("Failed to convert i32::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_i32()
            .ok_or_else(|| vec!["Failed to convert number to i32".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_u32(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u32(u32::MIN).expect("Failed to convert u32::MIN to BigDecimal"),
        BigDecimal::from_u32(u32::MAX).expect("Failed to convert u32::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_u32()
            .ok_or_else(|| vec!["Failed to convert number to u32".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_s64(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i64(i64::MIN).expect("Failed to convert i64::MIN to BigDecimal"),
        BigDecimal::from_i64(i64::MAX).expect("Failed to convert i64::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_i64()
            .ok_or_else(|| vec!["Failed to convert number to i64".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_f32(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_f32(f32::MIN).expect("Failed to convert f32::MIN to BigDecimal"),
        BigDecimal::from_f32(f32::MAX).expect("Failed to convert f32::MAX to BigDecimal"),
    )
    .and_then(|num| {
        num.to_f32()
            .ok_or_else(|| vec!["Failed to convert number to f32".to_string()])
    })
    .map(|num| num.into_value_and_type())
}

fn get_f64(json_val: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    let num = get_big_decimal(json_val)?;
    let num: f64 = num
        .to_string()
        .parse()
        .map_err(|err| vec![format!("Failed to convert number to f64: {err}")])?;
    Ok(num.into_value_and_type())
}

fn get_string(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    if let Some(str_value) = json.as_str() {
        // If the JSON value is a string, return it
        Ok(str_value.to_string().into_value_and_type())
    } else {
        // If the JSON value is not a string, return an error with type information
        let type_description = type_description(json);
        Err(vec![format!("expected string, found {}", type_description)])
    }
}

fn get_char(json: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    if let Some(num_u64) = json.as_u64() {
        if num_u64 > u32::MAX as u64 {
            Err(vec![format!(
                "The value {} is too large to be converted to a char",
                num_u64
            )])
        } else if let Some(ch) = char::from_u32(num_u64 as u32) {
            Ok(ch.into_value_and_type())
        } else {
            Err(vec![format!(
                "The value {} cannot be converted to a char",
                num_u64
            )])
        }
    } else {
        let type_description = type_description(json);

        Err(vec![format!("expected char, found {}", type_description)])
    }
}

fn get_tuple(input_json: &JsonValue, types: &[AnalysedType]) -> Result<ValueAndType, Vec<String>> {
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
    let mut tpes: Vec<AnalysedType> = vec![];

    for (json, tpe) in json_array.iter().zip(types.iter()) {
        match ValueAndType::parse_with_type(json, tpe) {
            Ok(result) => {
                vals.push(result.value);
                tpes.push(result.typ);
            }
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(ValueAndType::new(Value::Tuple(vals), tuple(tpes)))
    } else {
        Err(errors)
    }
}

fn get_option(input_json: &JsonValue, tpe: &AnalysedType) -> Result<ValueAndType, Vec<String>> {
    match input_json.as_null() {
        Some(_) => Ok(ValueAndType::new(Value::Option(None), option(tpe.clone()))),

        None => ValueAndType::parse_with_type(input_json, tpe).map(|result| {
            ValueAndType::new(
                Value::Option(Some(Box::new(result.value))),
                option(tpe.clone()),
            )
        }),
    }
}

fn get_list(input_json: &JsonValue, tpe: &AnalysedType) -> Result<ValueAndType, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<Value> = vec![];

    for json in json_array {
        match ValueAndType::parse_with_type(json, tpe) {
            Ok(result) => vals.push(result.value),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(ValueAndType::new(Value::List(vals), list(tpe.clone())))
    } else {
        Err(errors)
    }
}

fn get_enum(input_json: &JsonValue, names: &[String]) -> Result<ValueAndType, Vec<String>> {
    let input_enum_value = input_json
        .as_str()
        .ok_or(vec![format!("Input {} is not string", input_json)])?;

    let enum_value = names
        .iter()
        .position(|n| n == input_enum_value)
        .ok_or_else(|| {
            vec![format!(
                "Invalid input {}. Valid values are {}",
                input_enum_value,
                names.join(",")
            )]
        })?;

    Ok(ValueAndType::new(
        Value::Enum(enum_value as u32),
        AnalysedType::Enum(TypeEnum {
            name: None,
            owner: None,
            cases: names.to_vec(),
        }),
    ))
}

#[allow(clippy::type_complexity)]
fn get_result(
    input_json: &JsonValue,
    ok_type: &Option<Box<AnalysedType>>,
    err_type: &Option<Box<AnalysedType>>,
) -> Result<ValueAndType, Vec<String>> {
    fn validate(
        typ: &Option<Box<AnalysedType>>,
        input_json: &JsonValue,
    ) -> Result<Option<Box<Value>>, Vec<String>> {
        if let Some(typ) = typ {
            ValueAndType::parse_with_type(input_json, typ).map(|v| Some(Box::new(v.value)))
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

            Ok(ValueAndType::new(
                Value::Result(Ok(value)),
                AnalysedType::Result(TypeResult {
                    ok: ok_type.clone(),
                    err: err_type.clone(),
                    name: None,
                    owner: None,
                }),
            ))
        }
        None => match input_json.get("err") {
            Some(value) => {
                let value = validate(err_type, value)?;

                Ok(ValueAndType::new(
                    Value::Result(Err(value)),
                    AnalysedType::Result(TypeResult {
                        ok: ok_type.clone(),
                        err: err_type.clone(),
                        name: None,
                        owner: None,
                    }),
                ))
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
) -> Result<ValueAndType, Vec<String>> {
    let json_map = input_json.as_object().ok_or(vec![format!(
        "The input {} is not a json object",
        input_json
    )])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<Value> = vec![];

    for NameTypePair { name, typ } in name_type_pairs {
        if let Some(json_value) = json_map.get(name) {
            match ValueAndType::parse_with_type(json_value, typ) {
                Ok(result) => vals.push(result.value),
                Err(value_errors) => errors.extend(
                    value_errors
                        .iter()
                        .map(|err| format!("invalid value for key {name}: {err}"))
                        .collect::<Vec<_>>(),
                ),
            }
        } else {
            match typ {
                AnalysedType::Option(_) => {
                    vals.push(Value::Option(None));
                }
                _ => errors.push(format!("key '{name}' not found")),
            }
        }
    }

    if errors.is_empty() {
        Ok(ValueAndType::new(
            Value::Record(vals),
            record(name_type_pairs.to_vec()),
        ))
    } else {
        Err(errors)
    }
}

fn get_flag(input_json: &JsonValue, names: &[String]) -> Result<ValueAndType, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: HashSet<String> = HashSet::new();

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
            vals.insert(flag);
        } else {
            errors.push(format!(
                "Invalid input {}. Valid values are {}",
                flag,
                names.join(",")
            ));
        }
    }

    if errors.is_empty() {
        let mut bitmap = vec![false; names.len()];
        for (i, name) in names.iter().enumerate() {
            bitmap[i] = vals.contains(name);
        }

        Ok(ValueAndType::new(
            Value::Flags(bitmap),
            AnalysedType::Flags(TypeFlags {
                names: names.to_vec(),
                name: None,
                owner: None,
            }),
        ))
    } else {
        Err(errors)
    }
}

fn get_variant(
    input_json: &JsonValue,
    types: &[NameOptionTypePair],
) -> Result<ValueAndType, Vec<String>> {
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

    let case_idx = types
        .iter()
        .position(|pair| pair.name == *key)
        .ok_or_else(|| vec![format!("Unknown key {key} in the variant")])?
        as u32;

    match possible_mapping_indexed.get(key) {
        Some(Some(tpe)) => {
            let result = ValueAndType::parse_with_type(json, tpe)?;

            Ok(ValueAndType::new(
                Value::Variant {
                    case_idx,
                    case_value: Some(Box::new(result.value)),
                },
                variant(types.to_vec()),
            ))
        }
        Some(None) if json.is_null() => Ok(ValueAndType::new(
            Value::Variant {
                case_idx,
                case_value: None,
            },
            variant(types.to_vec()),
        )),
        Some(None) => Err(vec![format!("Unit variant {key} has non-null JSON value")]),
        None => Err(vec![format!("Unknown key {key} in the variant")]),
    }
}

fn get_handle(
    value: &JsonValue,
    id: AnalysedResourceId,
    resource_mode: AnalysedResourceMode,
) -> Result<ValueAndType, Vec<String>> {
    match value.as_str() {
        Some(str) => {
            // not assuming much about the url format, just checking it ends with a /<resource-id-u64>
            let parts: Vec<&str> = str.split('/').collect();
            if parts.len() >= 2 {
                match u64::from_str(parts[parts.len() - 1]) {
                    Ok(resource_id) => {
                        let uri = parts[0..(parts.len() - 1)].join("/");

                        Ok(ValueAndType::new(
                            Value::Handle { resource_id, uri },
                            AnalysedType::Handle(TypeHandle {
                                resource_id: id,
                                mode: resource_mode,
                                name: None,
                                owner: None,
                            }),
                        ))
                    }
                    Err(err) => Err(vec![format!(
                        "Failed to parse resource-id section of the handle value: {}",
                        err
                    )]),
                }
            } else {
                Err(vec![format!(
                    "expected handle, represented by a worker-url/resource-id string, found {}",
                    str
                )])
            }
        }
        None => Err(vec![format!(
            "expected handle, represented by a worker-url/resource-id string, found {}",
            type_description(value)
        )]),
    }
}

fn type_description(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "list",
        JsonValue::Object(_) => "record",
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
                Err(vec![format!("cannot convert {} to f64", num)])
            }
        }
        _ => {
            let type_description = type_description(value);
            Err(vec![format!("expected number, found {}", type_description)])
        }
    }
}

fn get_u64(value: &JsonValue) -> Result<ValueAndType, Vec<String>> {
    match value {
        JsonValue::Number(num) => {
            if let Some(u64) = num.as_u64() {
                Ok(u64.into_value_and_type())
            } else {
                Err(vec![format!("Cannot convert {} to u64", num)])
            }
        }
        _ => {
            let type_description = type_description(value);
            Err(vec![format!("expected u64, found {}", type_description)])
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

    use crate::json::ValueAndTypeJsonExtensions;
    use crate::{Value, ValueAndType};

    fn validate_function_result(
        val: Value,
        expected_type: &AnalysedType,
    ) -> Result<JsonValue, Vec<String>> {
        ValueAndType::new(val, expected_type.clone())
            .to_json_value()
            .map_err(|s| vec![s])
    }

    fn validate_function_parameter(
        json: &JsonValue,
        expected_type: &AnalysedType,
    ) -> Result<Value, Vec<String>> {
        match ValueAndType::parse_with_type(json, expected_type) {
            Ok(result) => Ok(result.value),
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
