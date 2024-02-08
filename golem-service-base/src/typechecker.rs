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

use std::collections::HashMap;

use bigdecimal::BigDecimal;
use golem_api_grpc::proto::golem::template::r#type::Type;
use golem_api_grpc::proto::golem::template::{FunctionParameter, FunctionResult, PrimitiveType};
use golem_api_grpc::proto::golem::worker::val::Val;
use golem_api_grpc::proto::golem::worker::{
    Val as VVal, ValEnum, ValFlags, ValList, ValOption, ValRecord, ValResult, ValTuple, ValVariant,
};
use golem_common::model::CallingConvention;
use num_traits::cast::FromPrimitive;
use num_traits::ToPrimitive;
use serde_json::{Number, Value};
use std::str::FromStr;

pub trait TypeCheckIn {
    fn validate_function_parameters(
        &self,
        expected_parameters: Vec<FunctionParameter>,
        calling_convention: CallingConvention,
    ) -> Result<Vec<VVal>, Vec<String>>;
}

impl TypeCheckIn for Value {
    fn validate_function_parameters(
        &self,
        expected_parameters: Vec<FunctionParameter>,
        calling_convention: CallingConvention,
    ) -> Result<Vec<VVal>, Vec<String>> {
        match calling_convention {
            CallingConvention::Component => {
                let parameters = self
                    .as_array()
                    .ok_or(vec!["Expecting an array for fn_params".to_string()])?;

                let mut results = vec![];
                let mut errors = vec![];

                if parameters.len() == expected_parameters.len() {
                    for (json, fp) in parameters.iter().zip(expected_parameters.iter()) {
                        match &fp.tpe {
                            Some(tpe) => match &tpe.r#type {
                                Some(inner_type) => {
                                    let result =
                                        validate_function_parameters(json, inner_type.clone())?;
                                    let wrapped = VVal { val: Some(result) };
                                    results.push(wrapped);
                                }
                                None => errors.push(
                                    "Failed to retrieve inner type during json validation"
                                        .to_string(),
                                ),
                            },

                            None => errors.push(
                                "Failed to retrieve outer type during json validation".to_string(),
                            ),
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
            CallingConvention::Stdio | CallingConvention::StdioEventloop => {
                if expected_parameters.is_empty() {
                    let vval: VVal = VVal {
                        val: Some(Val::String(self.to_string())),
                    };

                    Ok(vec![vval])
                } else {
                    Err(vec!["The exported function should not have any parameters when using the stdio calling convention".to_string()])
                }
            }
        }
    }
}

impl TypeCheckIn for Vec<VVal> {
    fn validate_function_parameters(
        &self,
        expected_parameters: Vec<FunctionParameter>,
        calling_convention: CallingConvention,
    ) -> Result<Vec<VVal>, Vec<String>> {
        match calling_convention {
            CallingConvention::Component => {
                if self.len() == expected_parameters.len() {
                    Ok(self.clone())
                } else {
                    Err(vec![format!(
                        "Unexpected number of parameters (got {}, expected: {})",
                        self.len(),
                        expected_parameters.len()
                    )])
                }
            }
            CallingConvention::Stdio | CallingConvention::StdioEventloop => {
                if expected_parameters.is_empty() {
                    if self.len() == 1 {
                        match &self[0].val {
                            Some(Val::String(_)) => Ok(self.clone()),
                            _ => Err(vec!["The exported function should be called with a single string parameter".to_string()])
                        }
                    } else {
                        Err(vec![
                            "The exported function should be called with a single string parameter"
                                .to_string(),
                        ])
                    }
                } else {
                    Err(vec!["The exported function should not have any parameters when using the stdio calling convention".to_string()])
                }
            }
        }
    }
}

pub trait TypeCheckOut {
    fn validate_function_result(
        &self,
        expected_types: Vec<FunctionResult>,
        calling_convention: CallingConvention,
    ) -> Result<Value, Vec<String>>;
}

impl TypeCheckOut for Vec<VVal> {
    fn validate_function_result(
        &self,
        expected_types: Vec<FunctionResult>,
        calling_convention: CallingConvention,
    ) -> Result<Value, Vec<String>> {
        match calling_convention {
            CallingConvention::Component => {
                if self.len() != expected_types.len() {
                    Err(vec![format!(
                        "Unexpected number of result values (got {}, expected: {})",
                        self.len(),
                        expected_types.len()
                    )])
                } else {
                    let mut results = vec![];
                    let mut errors = vec![];

                    for (value, expected) in self.iter().zip(expected_types.iter()) {
                        let outer_type = expected.tpe.clone().ok_or(vec![
                            "Unable to retrieve type during function result validation".to_string(),
                        ])?;

                        let inner_type = outer_type.r#type.clone().ok_or(vec![
                            "Unable to retrieve inner type during function result validation"
                                .to_string(),
                        ])?;

                        let inner_val = value.val.clone().ok_or(vec![
                            "Unable to retrieve inner val during function result validation"
                                .to_string(),
                        ])?;

                        let result = validate_function_result(&inner_val, inner_type);

                        match result {
                            Ok(value) => results.push(value),
                            Err(err) => errors.extend(err),
                        }
                    }

                    let all_without_names = expected_types.iter().all(|t| t.name.is_none());

                    if all_without_names {
                        Ok(Value::Array(results))
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

                        Ok(Value::Object(mapped_values))
                    }
                }
            }

            CallingConvention::Stdio | CallingConvention::StdioEventloop => {
                if self.len() == 1 {
                    let value_opt = &self[0].val;

                    match value_opt {
                        Some(Val::String(s)) => {
                            if s.is_empty()  {
                                Ok(Value::Null)
                            } else {
                                let result: Value = serde_json::from_str(s).unwrap_or(Value::String(s.to_string()));
                                Ok(result)
                            }
                        }
                        _ => Err(vec!["Expecting a single string as the result value when using stdio calling convention".to_string()]),
                    }
                } else {
                    Err(vec!["Expecting a single string as the result value when using stdio calling convention".to_string()])
                }
            }
        }
    }
}

fn validate_function_result(val: &Val, expected_type: Type) -> Result<Value, Vec<String>> {
    match val {
        Val::Bool(bool) => Ok(Value::Bool(*bool)),
        Val::S8(value) => Ok(Value::Number(Number::from(*value))),
        Val::U8(value) => Ok(Value::Number(Number::from(*value))),
        Val::U32(value) => Ok(Value::Number(Number::from(*value))),
        Val::S16(value) => Ok(Value::Number(Number::from(*value))),
        Val::U16(value) => Ok(Value::Number(Number::from(*value))),
        Val::S32(value) => Ok(Value::Number(Number::from(*value))),
        Val::S64(value) => Ok(Value::Number(Number::from(*value))),
        Val::U64(value) => Ok(Value::Number(Number::from(*value))),
        Val::F32(value) => Ok(Value::Number(Number::from_f64(*value as f64).unwrap())),
        Val::F64(value) => Ok(Value::Number(Number::from_f64(*value).unwrap())),
        Val::Char(value) => Ok(Value::Number(Number::from(*value as u32))),
        Val::String(value) => Ok(Value::String(value.to_string())),

        Val::Enum(value) => match expected_type {
            Type::Enum(en) => match en.names.get(value.discriminant as usize) {
                Some(str) => Ok(Value::String(str.clone())),
                None => Err(vec![format!("Invalid enum {}", value.discriminant)]),
            },
            _ => Err(vec![format!("Unexpected enum {}", value.discriminant)]),
        },

        Val::Option(value) => match expected_type {
            Type::Option(inner_type) => {
                let outer_type = inner_type.elem.ok_or(vec![
                    "Missing outer type information for Option.".to_string(),
                ])?;
                let inner_type = outer_type.r#type.ok_or(vec![
                    "Missing inner type information for Option.".to_string(),
                ])?;

                match &value.value {
                    Some(value) => match &value.val {
                        Some(v) => validate_function_result(v, inner_type),
                        None => Ok(Value::Null),
                    },

                    None => Ok(Value::Null),
                }
            }

            _ => Err(vec!["Unexpected type; expected an Option type.".to_string()]),
        },

        Val::Tuple(value) => match expected_type {
            Type::Tuple(types) => {
                let tuple_values = value.values.clone();
                let types = types.elems;

                if tuple_values.len() != types.len() {
                    return Err(vec![format!(
                        "Tuple has unexpected number of elements: {} vs {}",
                        tuple_values.len(),
                        types.len(),
                    )]);
                }

                let mut errors = vec![];
                let mut results = vec![];

                for (v, tpe) in tuple_values.iter().zip(types.iter()) {
                    match &v.val {
                        Some(val) => {
                            let inner_type = tpe.clone().r#type.ok_or(vec![
                                "Missing inner type information for tuple element.".to_string(),
                            ])?;

                            let result = validate_function_result(val, inner_type.clone())?;

                            results.push(result);
                        }

                        None => errors
                            .push("Unexpected absence of value for tuple element.".to_string()),
                    }
                }

                if errors.is_empty() {
                    Ok(Value::Array(results))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a tuple type.".to_string()]),
        },

        Val::List(value) => match expected_type {
            Type::List(elem_type) => {
                let mut errors = vec![];
                let mut results = vec![];

                let outer_type = elem_type.elem.ok_or(vec![
                    "Missing outer type information for list elements.".to_string(),
                ])?;
                let inner_type = outer_type.r#type.ok_or(vec![
                    "Missing inner type information for list elements.".to_string(),
                ])?;

                for v in value.values.clone() {
                    match v.val {
                        Some(v) => {
                            let result = validate_function_result(&v, inner_type.clone());

                            match result {
                                Ok(value) => results.push(value),
                                Err(errs) => errors.extend(errs),
                            }
                        }

                        None => errors.push("Unexpected absence of value in the list.".to_string()),
                    }
                }

                if errors.is_empty() {
                    Ok(Value::Array(results))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a list type.".to_string()]),
        },

        Val::Record(value) => match expected_type {
            Type::Record(record) => {
                let fields = record.fields;
                let field_values = value.values.clone();

                if field_values.len() != fields.len() {
                    return Err(vec!["The total number of field values is zero".to_string()]);
                }

                let mut errors = vec![];
                let mut results = serde_json::Map::new();

                for (v, np) in field_values.iter().zip(fields) {
                    let outer_type = np.typ.ok_or(vec![
                        "Missing outer type information for record field.".to_string(),
                    ])?;
                    let inner_type = outer_type.r#type.ok_or(vec![
                        "Missing inner type information for record field.".to_string(),
                    ])?;

                    match &v.val {
                        Some(val) => {
                            let result = validate_function_result(val, inner_type);
                            match result {
                                Ok(res) => {
                                    results.insert(np.name, res);
                                }
                                Err(errs) => errors.extend(errs),
                            }
                        }

                        None => errors.push("Unexpected record".to_string()),
                    }
                }

                if errors.is_empty() {
                    Ok(Value::Object(results))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
        },

        Val::Variant(value) => match expected_type {
            Type::Variant(cases) => {
                let cases = cases.cases;
                let discriminant = value.discriminant;

                if (discriminant as usize) < cases.len() {
                    let pair = match cases.get(discriminant as usize) {
                        Some(tpe) => Ok(tpe),
                        None => Err(vec!["Variant not found in the expected types.".to_string()]),
                    }?;

                    let vvalue = &value.value;

                    match vvalue {
                        Some(v) => match &v.val {
                            Some(vval) => {
                                let typ_opt = &pair.typ;
                                match typ_opt {
                                    Some(tpe) => {
                                        let outer_type = tpe.clone().r#type.ok_or(vec![
                                            "Missing inner type information.".to_string(),
                                        ])?;

                                        let result = validate_function_result(vval, outer_type)?;
                                        let mut map = serde_json::Map::new();
                                        map.insert(pair.name.clone(), result);
                                        Ok(Value::Object(map))
                                    }
                                    None => {
                                        Err(vec!["Missing inner type information.".to_string()])
                                    }
                                }
                            }
                            None => Err(vec![
                                "Unexpected absence of value in the variant type.".to_string()
                            ]),
                        },

                        None => Err(vec![
                            "Unexpected absence of value in the variant type.".to_string()
                        ]),
                    }
                } else {
                    Err(vec![
                        "Invalid discriminant value for the variant.".to_string()
                    ])
                }
            }

            _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
        },

        Val::Flags(value) => match expected_type {
            Type::Flags(values) => {
                let discriminants = &value.value;
                let values = values.names;

                let mut errors = vec![];
                let mut result = vec![];

                for discriminant in discriminants {
                    let discriminant = *discriminant as usize;

                    if discriminant < values.len() {
                        match values.get(discriminant) {
                            Some(v) => result.push(Value::String(v.clone())),
                            None => errors.push(format!("Invalid discriminant: {}", discriminant)),
                        }
                    } else {
                        errors.push(format!("Invalid discriminant: {}", discriminant));
                    }
                }

                if errors.is_empty() {
                    Ok(Value::Array(result))
                } else {
                    Err(errors)
                }
            }

            _ => Err(vec!["Unexpected type; expected a flags type.".to_string()]),
        },

        Val::Result(value) => match expected_type {
            Type::Result(type_result) => {
                let maybe_ok = &type_result.ok;
                let maybe_err = &type_result.err;

                match (value.discriminant, maybe_ok, maybe_err) {
                    (0, Some(ok_type), _) => {
                        let ok_type_inner = ok_type.clone().r#type.ok_or(vec![
                            "Missing inner type information for 'ok' variant.".to_string(),
                        ])?;

                        let mut map: serde_json::Map<String, Value> = serde_json::Map::new();
                        match value.value.clone() {
                            Some(vvalue) => {
                                let vval = vvalue.val.ok_or(vec![
                                    "Missing value information for 'ok' variant.".to_string(),
                                ])?;
                                let result = validate_function_result(&vval, ok_type_inner)?;

                                map.insert("ok".to_string(), result);

                                Ok(Value::Object(map))
                            }
                            None => Err(vec![
                                "Unexpected absence of value for 'ok' variant.".to_string()
                            ]),
                        }
                    }

                    (0, None, _) => {
                        let mut map: serde_json::Map<String, Value> = serde_json::Map::new();

                        map.insert("ok".to_string(), Value::Null);

                        Ok(Value::Object(map))
                    }

                    (1, _, Some(err_type)) => {
                        let err_type_inner = err_type.clone().r#type.ok_or(vec![
                            "Missing inner type information for 'err' variant.".to_string(),
                        ])?;

                        let mut map: serde_json::Map<String, Value> = serde_json::Map::new();

                        match value.value.clone() {
                            Some(vvalue) => {
                                let vval = vvalue.val.ok_or(vec![
                                    "Missing value information for 'err' variant.".to_string(),
                                ])?;

                                let result = validate_function_result(&vval, err_type_inner)?;

                                map.insert("err".to_string(), result);

                                Ok(Value::Object(map))
                            }
                            None => Err(vec![
                                "Unexpected absence of value for 'err' variant.".to_string()
                            ]),
                        }
                    }

                    (1, _, None) => {
                        let mut map: serde_json::Map<String, Value> = serde_json::Map::new();

                        map.insert("err".to_string(), Value::Null);

                        Ok(Value::Object(map))
                    }

                    _ => Err(vec!["Invalid discriminant for Result type.".to_string()]),
                }
            }

            _ => Err(vec!["Unexpected type; expected a Result type.".to_string()]),
        },
    }
}

fn validate_function_parameters(
    input_json: &serde_json::Value,
    expected_type: Type,
) -> Result<Val, Vec<String>> {
    match expected_type {
        Type::Primitive(r) => {
            let x = r.primitive();
            match x {
                PrimitiveType::Bool => get_bool(input_json),
                PrimitiveType::S8 => get_s8(input_json),
                PrimitiveType::U8 => get_u8(input_json),
                PrimitiveType::S16 => get_s16(input_json),
                PrimitiveType::U16 => get_u16(input_json),
                PrimitiveType::S32 => get_s32(input_json),
                PrimitiveType::U32 => get_u32(input_json),
                PrimitiveType::S64 => get_s64(input_json),
                PrimitiveType::U64 => get_i64(input_json).map(Val::U64),
                PrimitiveType::F64 => {
                    bigdecimal(input_json).map(|num| Val::F64(num.to_string().parse().unwrap()))
                }
                PrimitiveType::F32 => get_f32(input_json),
                PrimitiveType::Chr => get_char(input_json).map(Val::Char),
                PrimitiveType::Str => get_string(input_json).map(Val::String),
            }
        }

        Type::Enum(type_enum) => get_enum(input_json, type_enum.names).map(Val::Enum),

        Type::Flags(flags) => get_flag(input_json, flags.names).map(Val::Flags),

        Type::List(list) => {
            let outer_tpe = list.elem.ok_or(vec![
                "Unable to obtain type of list in typechecker".to_string(),
            ])?;

            let tpe = outer_tpe
                .r#type
                .ok_or(vec!["Internal error. Unable to obtain type".to_string()])?;

            get_list(input_json, tpe).map(Val::List)
        }

        Type::Option(option) => {
            if let Some(tpe) = option.elem {
                let tpe = tpe.r#type.ok_or(vec![
                    "Unable to obtain type of option in typechecker".to_string(),
                ])?;
                get_option(input_json, tpe).map(|result| Val::Option(Box::new(result)))
            } else {
                Err(vec![
                    "Unable to obtain type of option in typechecker".to_string()
                ])
            }
        }

        Type::Result(result) => {
            if let Some(ok_type) = result.ok {
                if let Some(err_type) = result.err {
                    get_result(input_json, ok_type.r#type, err_type.r#type)
                        .map(|result| Val::Result(Box::new(result)))
                } else {
                    Err(vec!["Unable to obtain type of error in Result".to_string()])
                }
            } else {
                Err(vec!["Unable to obtain type of ok in Result".to_string()])
            }
        }

        Type::Record(record) => {
            let mut pairs: Vec<(&String, &Type)> = vec![];

            for field in &record.fields {
                if let Some(typ) = field.typ.as_ref().and_then(|t| t.r#type.as_ref()) {
                    pairs.push((&field.name, typ));
                }
            }

            get_record(input_json, pairs).map(Val::Record)
        }

        Type::Variant(variant) => {
            let mut pairs: Vec<(String, Option<Type>)> = vec![];
            for pair in variant.cases {
                if let Some(typ) = pair.typ {
                    pairs.push((pair.name, typ.r#type));
                }
            }

            get_variant(input_json, pairs).map(|result| Val::Variant(Box::new(result)))
        }
        Type::Tuple(tuple) => {
            let mut types = vec![];
            for elem in tuple.elems {
                if let Some(typ) = elem.r#type {
                    types.push(typ)
                }
            }

            get_tuple(input_json, types).map(Val::Tuple)
        }
    }
}

fn get_bool(json: &Value) -> Result<Val, Vec<String>> {
    match json {
        Value::Bool(bool_val) => Ok(Val::Bool(*bool_val)),
        _ => {
            let type_description = type_description(json);
            Err(vec![format!(
                "Expected function parameter type is Boolean. But found {}",
                type_description
            )])
        }
    }
}

fn get_s8(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i8(i8::MIN).unwrap(),
        BigDecimal::from_i8(i8::MAX).unwrap(),
    )
    .map(|num| Val::S8(num.to_i32().unwrap()))
}

fn get_u8(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u8(u8::MIN).unwrap(),
        BigDecimal::from_u8(u8::MAX).unwrap(),
    )
    .map(|num| Val::U8(num.to_i32().unwrap()))
}

fn get_s16(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i16(i16::MIN).unwrap(),
        BigDecimal::from_i16(i16::MAX).unwrap(),
    )
    .map(|num| Val::S16(num.to_i32().unwrap()))
}

fn get_u16(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u16(u16::MIN).unwrap(),
        BigDecimal::from_u16(u16::MAX).unwrap(),
    )
    .map(|num| Val::U16(num.to_i32().unwrap()))
}

fn get_s32(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i32(i32::MIN).unwrap(),
        BigDecimal::from_i32(i32::MAX).unwrap(),
    )
    .map(|num| Val::S32(num.to_i32().unwrap()))
}

fn get_u32(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_u32(u32::MIN).unwrap(),
        BigDecimal::from_u32(u32::MAX).unwrap(),
    )
    .map(|num| Val::U32(num.to_i64().unwrap()))
}

fn get_s64(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_i64(i64::MIN).unwrap(),
        BigDecimal::from_i64(i64::MAX).unwrap(),
    )
    .map(|num| Val::S64(num.to_i64().unwrap()))
}

fn get_f32(json: &Value) -> Result<Val, Vec<String>> {
    ensure_range(
        json,
        BigDecimal::from_f32(f32::MIN).unwrap(),
        BigDecimal::from_f32(f32::MAX).unwrap(),
    )
    .map(|num| Val::F32(num.to_f32().unwrap()))
}

fn ensure_range(
    value: &Value,
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

fn get_i64(value: &Value) -> Result<i64, Vec<String>> {
    match value {
        Value::Number(num) => {
            if let Some(i64) = num.as_i64() {
                Ok(i64)
            } else {
                Err(vec![format!("Cannot convert {} to i64", num)])
            }
        }
        _ => {
            let type_description = type_description(value);
            Err(vec![format!(
                "Expected function parameter type is i64. But found {}",
                type_description
            )])
        }
    }
}

fn bigdecimal(value: &Value) -> Result<BigDecimal, Vec<String>> {
    match value {
        Value::Number(num) => {
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

fn get_char(json: &Value) -> Result<i32, Vec<String>> {
    if let Some(num_u64) = json.as_u64() {
        if num_u64 > u32::MAX as u64 {
            Err(vec![format!(
                "The value {} is too large to be converted to a char",
                num_u64
            )])
        } else {
            char::from_u32(num_u64 as u32)
                .map(|char| char as u32 as i32)
                .ok_or(vec![format!(
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

fn get_string(input_json: &Value) -> Result<String, Vec<String>> {
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

fn type_description(value: &Value) -> &'static str {
    match value {
        Value::Null => "Null",
        Value::Bool(_) => "Boolean",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
        Value::Array(_) => "Array",
        Value::Object(_) => "Object",
    }
}

fn get_result(
    input_json: &Value,
    ok_type: Option<Type>,
    err_type: Option<Type>,
) -> Result<ValResult, Vec<String>> {
    fn validate(
        typ: Option<Type>,
        input_json: &Value,
        discriminant: i32,
    ) -> Result<ValResult, Vec<String>> {
        if let Some(typ) = typ {
            validate_function_parameters(input_json, typ).map(|result| ValResult {
                discriminant,
                value: Some(Box::new(VVal { val: Some(result) })),
            })
        } else {
            Err(vec!["The type of ok is absent".to_string()])
        }
    }

    match input_json.get("ok") {
        Some(value) => validate(ok_type, value, 0),
        None => match input_json.get("err") {
            Some(value) => validate(err_type, value, 1),
            None => Err(vec![
                "Failed to retrieve either ok value or err value".to_string()
            ]),
        },
    }
}

fn get_option(input_json: &Value, tpe: Type) -> Result<ValOption, Vec<String>> {
    match input_json.as_null() {
        Some(_) => Ok(ValOption {
            discriminant: 0,
            value: None,
        }),

        None => validate_function_parameters(input_json, tpe).map(|result| ValOption {
            discriminant: 1,
            value: Some(Box::new(VVal { val: Some(result) })),
        }),
    }
}

fn get_list(input_json: &Value, tpe: Type) -> Result<ValList, Vec<String>> {
    let json_array = input_json
        .as_array()
        .ok_or(vec![format!("Input {} is not an array", input_json)])?;

    // We could use the functional library frunk (which looks good) - but there are a few concerns such as not enough enough instances.
    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<VVal> = vec![];

    for json in json_array {
        match validate_function_parameters(json, tpe.clone()) {
            Ok(result) => vals.push(VVal { val: Some(result) }),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(ValList { values: vals })
    } else {
        Err(errors)
    }
}

fn get_tuple(input_json: &Value, types: Vec<Type>) -> Result<ValTuple, Vec<String>> {
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

    // We could use the functional library frunk (which looks good) - but there are a few concerns such as not enough enough instances.
    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<VVal> = vec![];

    for (json, tpe) in json_array.iter().zip(types.iter()) {
        match validate_function_parameters(json, tpe.clone()) {
            Ok(result) => vals.push(VVal { val: Some(result) }),
            Err(errs) => errors.extend(errs),
        }
    }

    if errors.is_empty() {
        Ok(ValTuple { values: vals })
    } else {
        Err(errors)
    }
}

fn get_record(
    input_json: &Value,
    name_type_pairs: Vec<(&String, &Type)>,
) -> Result<ValRecord, Vec<String>> {
    let json_map = input_json.as_object().ok_or(vec![format!(
        "The input {} is not a json object",
        input_json
    )])?;

    let mut errors: Vec<String> = vec![];
    let mut vals: Vec<VVal> = vec![];

    for (name, tpe) in name_type_pairs {
        if let Some(json_value) = json_map.get(name) {
            match validate_function_parameters(json_value, tpe.clone()) {
                Ok(result) => vals.push(VVal { val: Some(result) }),
                Err(value_errors) => errors.extend(
                    value_errors
                        .iter()
                        .map(|err| format!("Invalid value for the key {}. Error: {}", name, err))
                        .collect::<Vec<_>>(),
                ),
            }
        } else {
            match tpe {
                Type::Option(_) => {
                    vals.push(VVal {
                        val: Some(Val::Option(Box::new(ValOption {
                            discriminant: 0,
                            value: None,
                        }))),
                    });
                }
                _ => errors.push(format!("Key '{}' not found in json_map", name)),
            }
        }
    }

    if errors.is_empty() {
        Ok(ValRecord { values: vals })
    } else {
        Err(errors)
    }
}

fn get_enum(input_json: &Value, names: Vec<String>) -> Result<ValEnum, Vec<String>> {
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
        Ok(ValEnum { discriminant: d })
    } else {
        Err(vec![format!(
            "Invalid input {}. Valid values are {}",
            input_enum_value,
            names.join(",")
        )])
    }
}

fn get_flag(input_json: &Value, names: Vec<String>) -> Result<ValFlags, Vec<String>> {
    let input_flag_values = input_json.as_array().ok_or(vec![format!(
        "Input {} is not an array to be parsed as flags",
        input_json
    )])?;

    let mut discriminant_map: HashMap<&str, usize> = HashMap::new();

    for (pos, name) in names.iter().enumerate() {
        discriminant_map.insert(name.as_str(), pos);
    }

    let mut discriminant: Vec<i32> = vec![];

    for i in input_flag_values {
        let json_str = i
            .as_str()
            .ok_or(vec![format!("{} is not a valid string", i)])?;
        if let Some(d) = discriminant_map.get(json_str) {
            discriminant.push(*d as i32);
        } else {
            return Err(vec![format!(
                "Invalid input {}. It should be one of {}",
                json_str,
                names.join(",")
            )]);
        }
    }

    Ok(ValFlags {
        count: names.len() as i32,
        value: discriminant,
    })
}

fn get_variant(
    input_json: &Value,
    types: Vec<(String, Option<Type>)>,
) -> Result<ValVariant, Vec<String>> {
    let mut possible_mapping_indexed: HashMap<&String, (usize, &Option<Type>)> = HashMap::new();

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
        Some((index, Some(tpe))) => {
            validate_function_parameters(json, tpe.clone()).map(|result| ValVariant {
                discriminant: *index as i32,
                value: Some(Box::new(VVal { val: Some(result) })),
            })
        }
        Some((_, None)) => Err(vec![format!("Unknown json {} in the variant", input_json)]),
        None => Err(vec![format!("Unknown key {} in the variant", key)]),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::collections::HashSet;

    use golem_api_grpc::proto::golem::template::{NameTypePair, TypePrimitive, TypeRecord};
    use proptest::prelude::*;
    use serde::Serialize;
    use serde_json::{Number, Value};

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct RandomData {
        string: String,
        number: f64,
        nullable: Option<String>,
        collection: Vec<String>,
        boolean: bool,
        object: InnerObj,
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct InnerObj {
        nested: String,
    }

    impl RandomData {
        fn get_type() -> Type {
            Type::Record(golem_api_grpc::proto::golem::template::TypeRecord {
                fields: vec![
                    golem_api_grpc::proto::golem::template::NameTypePair {
                        name: "string".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Primitive(TypePrimitive {
                                primitive: PrimitiveType::Str as i32,
                            })),
                        }),
                    },
                    golem_api_grpc::proto::golem::template::NameTypePair {
                        name: "number".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Primitive(TypePrimitive {
                                primitive: PrimitiveType::F64 as i32,
                            })),
                        }),
                    },
                    golem_api_grpc::proto::golem::template::NameTypePair {
                        name: "nullable".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Option(Box::new(
                                golem_api_grpc::proto::golem::template::TypeOption {
                                    elem: Some(Box::new(
                                        golem_api_grpc::proto::golem::template::Type {
                                            r#type: Some(Type::Primitive(TypePrimitive {
                                                primitive: PrimitiveType::Str as i32,
                                            })),
                                        },
                                    )),
                                },
                            ))),
                        }),
                    },
                    golem_api_grpc::proto::golem::template::NameTypePair {
                        name: "collection".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::List(Box::new(
                                golem_api_grpc::proto::golem::template::TypeList {
                                    elem: Some(Box::new(
                                        golem_api_grpc::proto::golem::template::Type {
                                            r#type: Some(Type::Primitive(TypePrimitive {
                                                primitive: PrimitiveType::Str as i32,
                                            })),
                                        },
                                    )),
                                },
                            ))),
                        }),
                    },
                    golem_api_grpc::proto::golem::template::NameTypePair {
                        name: "boolean".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Primitive(TypePrimitive {
                                primitive: PrimitiveType::Bool as i32,
                            })),
                        }),
                    },
                    // one field is missing
                    golem_api_grpc::proto::golem::template::NameTypePair {
                        name: "object".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Record(
                                golem_api_grpc::proto::golem::template::TypeRecord {
                                    fields: vec![
                                        golem_api_grpc::proto::golem::template::NameTypePair {
                                            name: "nested".to_string(),
                                            typ: Some(
                                                golem_api_grpc::proto::golem::template::Type {
                                                    r#type: Some(Type::Primitive(TypePrimitive {
                                                        primitive: PrimitiveType::Str as i32,
                                                    })),
                                                },
                                            ),
                                        },
                                    ],
                                },
                            )),
                        }),
                    },
                ],
            })
        }
    }

    impl Arbitrary for RandomData {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            (
                any::<String>(),
                any::<f64>(),
                any::<Option<String>>(),
                any::<Vec<String>>(),
                any::<bool>(),
                any::<InnerObj>(),
            )
                .prop_map(
                    |(string, number, nullable, collection, boolean, object)| RandomData {
                        string,
                        number,
                        nullable,
                        collection,
                        boolean,
                        object,
                    },
                )
                .boxed()
        }
    }

    impl Arbitrary for InnerObj {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            any::<String>()
                .prop_map(|nested| InnerObj { nested })
                .boxed()
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct FunctionOutputTestResult {
        val: Val,
        expected_type: Type,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct PrimitiveVal {
        val: Val,
        expected_type: Type,
    }

    impl Arbitrary for PrimitiveVal {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            prop_oneof![
                any::<i32>().prop_map(|val| PrimitiveVal {
                    val: Val::S32(val),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::S32 as i32
                    })
                }),
                any::<i8>().prop_map(|val| PrimitiveVal {
                    val: Val::S8(val as i32),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::S8 as i32
                    })
                }),
                any::<i16>().prop_map(|val| PrimitiveVal {
                    val: Val::S16(val as i32),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::S16 as i32
                    })
                }),
                any::<i64>().prop_map(|val| PrimitiveVal {
                    val: Val::S64(val),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::S64 as i32
                    })
                }),
                any::<u8>().prop_map(|val| PrimitiveVal {
                    val: Val::U8(val as i32),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::U8 as i32
                    })
                }),
                any::<u16>().prop_map(|val| PrimitiveVal {
                    val: Val::U16(val as i32),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::U16 as i32
                    })
                }),
                any::<u32>().prop_map(|val| PrimitiveVal {
                    val: Val::U32(val as i64),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::U32 as i32
                    })
                }),
                any::<u64>().prop_map(|val| PrimitiveVal {
                    val: Val::U64(val as i64),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::U64 as i32
                    })
                }),
                any::<f32>().prop_map(|val| PrimitiveVal {
                    val: Val::F32(val),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::F32 as i32
                    })
                }),
                any::<f64>().prop_map(|val| PrimitiveVal {
                    val: Val::F64(val),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::F64 as i32
                    })
                }),
                any::<bool>().prop_map(|val| PrimitiveVal {
                    val: Val::Bool(val),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::Bool as i32
                    })
                }),
                any::<u16>().prop_map(|val| val).prop_map(|_| {
                    PrimitiveVal {
                        val: Val::Char('a' as i32),
                        expected_type: Type::Primitive(TypePrimitive {
                            primitive: PrimitiveType::Chr as i32,
                        }),
                    }
                }),
                any::<String>().prop_map(|val| PrimitiveVal {
                    val: Val::String(val),
                    expected_type: Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::Str as i32
                    })
                }),
            ]
            .boxed()
        }
    }

    fn distinct_by<T, F>(vec: Vec<T>, key_fn: F) -> Vec<T>
    where
        F: Fn(&T) -> String,
        T: Clone + PartialEq,
    {
        let mut seen = HashSet::new();
        vec.into_iter()
            .filter(|item| seen.insert(key_fn(item)))
            .collect()
    }

    impl Arbitrary for FunctionOutputTestResult {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            prop_oneof![
                any::<(Vec<String>, u8)>().prop_map(|(values, disc)| {
                    let unique_values = distinct_by(values, |x| x.clone());

                    FunctionOutputTestResult {
                        val: Val::Enum(ValEnum {
                            discriminant: if (disc as usize) < unique_values.len() {
                                disc.into()
                            } else {
                                0
                            },
                        }),
                        expected_type: Type::Enum(
                            golem_api_grpc::proto::golem::template::TypeEnum {
                                names: if unique_values.is_empty() {
                                    vec!["const_name".to_string()]
                                } else {
                                    unique_values.iter().map(|name| name.to_string()).collect()
                                },
                            },
                        ),
                    }
                }),
                any::<Vec<String>>().prop_map(|values| {
                    let unique_values = distinct_by(values, |x| x.clone());

                    FunctionOutputTestResult {
                        val: Val::Flags(ValFlags {
                            count: unique_values.len() as i32,
                            value: unique_values
                                .iter()
                                .enumerate()
                                .map(|(index, _)| index as i32)
                                .collect(),
                        }),
                        expected_type: Type::Flags(
                            golem_api_grpc::proto::golem::template::TypeFlags {
                                names: unique_values.iter().map(|name| name.to_string()).collect(),
                            },
                        ),
                    }
                }),
                any::<Vec<PrimitiveVal>>().prop_map(|values| {
                    let expected_type = if values.is_empty() {
                        Type::Primitive(TypePrimitive {
                            primitive: PrimitiveType::Str as i32,
                        })
                    } else {
                        values[0].expected_type.clone()
                    };

                    let vals_with_same_type = values
                        .iter()
                        .filter(|prim| prim.expected_type == expected_type)
                        .cloned()
                        .collect::<Vec<PrimitiveVal>>();

                    FunctionOutputTestResult {
                        val: Val::List(ValList {
                            values: vals_with_same_type
                                .iter()
                                .map(|prim| VVal {
                                    val: Some(prim.val.clone()),
                                })
                                .collect(),
                        }),
                        expected_type: Type::List(Box::new(
                            golem_api_grpc::proto::golem::template::TypeList {
                                elem: Some(Box::new(
                                    golem_api_grpc::proto::golem::template::Type {
                                        r#type: expected_type.into(),
                                    },
                                )),
                            },
                        )),
                    }
                }),
                any::<Vec<PrimitiveVal>>().prop_map(|values| FunctionOutputTestResult {
                    val: Val::Tuple(ValTuple {
                        values: values
                            .iter()
                            .map(|x| VVal {
                                val: Some(x.val.clone())
                            })
                            .collect()
                    }),
                    expected_type: Type::Tuple(golem_api_grpc::proto::golem::template::TypeTuple {
                        elems: values
                            .iter()
                            .map(|x| golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(x.expected_type.clone())
                            })
                            .collect()
                    })
                }),
                any::<Vec<(String, PrimitiveVal)>>().prop_map(|values| {
                    let new_values = distinct_by(values, |(x, _)| x.clone());

                    FunctionOutputTestResult {
                        val: {
                            Val::Record(ValRecord {
                                values: new_values
                                    .iter()
                                    .map(|(_, val)| VVal {
                                        val: Some(val.val.clone()),
                                    })
                                    .collect(),
                            })
                        },
                        expected_type: Type::Record(
                            golem_api_grpc::proto::golem::template::TypeRecord {
                                fields: new_values
                                    .iter()
                                    .map(|(name, val)| {
                                        golem_api_grpc::proto::golem::template::NameTypePair {
                                            name: name.to_string(),
                                            typ: Some(
                                                golem_api_grpc::proto::golem::template::Type {
                                                    r#type: Some(val.expected_type.clone()),
                                                },
                                            ),
                                        }
                                    })
                                    .collect(),
                            },
                        ),
                    }
                }),
                any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
                    val: Val::Option(Box::new(ValOption {
                        discriminant: 1,
                        value: Some(Box::new(VVal {
                            val: Some(val.val.clone())
                        }))
                    })),
                    expected_type: Type::Option(Box::new(
                        golem_api_grpc::proto::golem::template::TypeOption {
                            elem: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(val.expected_type.clone())
                            }))
                        }
                    ))
                }),
                Just(FunctionOutputTestResult {
                    val: Val::Option(Box::new(ValOption {
                        discriminant: 0,
                        value: None
                    })),
                    expected_type: Type::Option(Box::new(
                        golem_api_grpc::proto::golem::template::TypeOption {
                            elem: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(Type::Primitive(TypePrimitive {
                                    primitive: PrimitiveType::Str as i32
                                }))
                            }))
                        }
                    ))
                }),
                any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
                    val: Val::Result(Box::new(ValResult {
                        discriminant: 0,
                        value: Some(Box::new(VVal {
                            val: Some(val.val.clone())
                        }))
                    })),
                    expected_type: Type::Result(Box::new(
                        golem_api_grpc::proto::golem::template::TypeResult {
                            ok: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(val.expected_type.clone())
                            })),
                            err: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(Type::Primitive(TypePrimitive {
                                    primitive: PrimitiveType::Str as i32
                                }))
                            }))
                        }
                    ))
                }),
                any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
                    val: Val::Result(Box::new(ValResult {
                        discriminant: 1,
                        value: Some(Box::new(VVal {
                            val: Some(val.val.clone())
                        }))
                    })),
                    expected_type: Type::Result(Box::new(
                        golem_api_grpc::proto::golem::template::TypeResult {
                            ok: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(Type::Primitive(TypePrimitive {
                                    primitive: PrimitiveType::Str as i32
                                }))
                            })),
                            err: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                                r#type: Some(val.expected_type.clone())
                            }))
                        }
                    ))
                }),
                any::<PrimitiveVal>().prop_map(|val| FunctionOutputTestResult {
                    val: val.val.clone(),
                    expected_type: val.expected_type.clone()
                }),
            ]
            .boxed()
        }
    }

    fn test_type_checker_string(data: String) {
        let json = Value::String(data.clone());
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::Str as i32,
            }),
        );
        assert_eq!(result, Ok(Val::String(data)));
    }

    fn test_type_checker_s8(data: i32) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S8 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::S8(data)));
    }
    fn test_type_checker_u8(data: i32) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U8 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::U8(data)));
    }

    fn test_type_checker_s16(data: i32) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S16 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::S16(data)));
    }

    fn test_type_checker_u16(data: i32) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U16 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::U16(data)));
    }

    fn test_type_checker_s32(data: i32) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S32 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::S32(data)));
    }

    fn test_type_checker_u32(data: i64) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U32 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::U32(data)));
    }

    fn test_type_checker_s64(data: i64) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::S64 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::S64(data)));
    }

    fn test_type_checker_u64(data: i64) {
        let json = Value::Number(Number::from(data));
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::U64 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::U64(data)));
    }

    fn test_type_checker_f32(data: f32) {
        let json = Value::Number(Number::from_f64(data as f64).unwrap());
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::F32 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::F32(data)));
    }

    fn test_type_checker_f64(data: f64) {
        let json = Value::Number(Number::from_f64(data).unwrap());
        let result = validate_function_parameters(
            &json,
            Type::Primitive(TypePrimitive {
                primitive: PrimitiveType::F64 as i32,
            }),
        );
        assert_eq!(result, Ok(Val::F64(data)));
    }

    fn test_type_checker_record(data: &RandomData) {
        let json = serde_json::json!({
            "string": data.string.clone(),
            "number": data.number,
            "nullable": data.nullable.clone(),
            "collection": data.collection.clone(),
            "boolean": data.boolean,
            "object": data.object.clone()
        });

        let result = validate_function_parameters(&json, RandomData::get_type());

        assert_eq!(
            result,
            Ok(Val::Record(ValRecord {
                values: vec![
                    VVal {
                        val: Some(Val::String(data.string.clone()))
                    },
                    VVal {
                        val: Some(Val::F64(data.number))
                    },
                    VVal {
                        val: match &data.nullable {
                            Some(place) => Some(Val::Option(Box::new(ValOption {
                                discriminant: 1,
                                value: Some(Box::new(VVal {
                                    val: Some(Val::String(place.clone()))
                                }))
                            }))),
                            None => Some(Val::Option(Box::new(ValOption {
                                discriminant: 0,
                                value: None
                            }))),
                        }
                    },
                    VVal {
                        val: Some(Val::List(ValList {
                            values: data
                                .collection
                                .clone()
                                .into_iter()
                                .map(|friend| VVal {
                                    val: Some(Val::String(friend))
                                })
                                .collect()
                        }))
                    },
                    VVal {
                        val: Some(Val::Bool(data.boolean))
                    },
                    VVal {
                        val: Some(Val::Record(ValRecord {
                            values: vec![VVal {
                                val: Some(Val::String(data.object.nested.clone()))
                            }]
                        }))
                    }
                ]
            }))
        );
    }

    proptest! {
        #[test]
        fn test3(data in 0..=255) {
            test_type_checker_u8(data);
        }

        #[test]
        fn test_s8(data in -127..=127) {
            test_type_checker_s8(data);
        }

        #[test]
        fn test4(data in -32768..=32767) {
            test_type_checker_s16(data);
        }

        #[test]
        fn test5(data in 0..=65535) {
            test_type_checker_u16(data);
        }

        #[test]
        fn test6(data in -2147483648..=2147483647) {
            test_type_checker_s32(data);
        }

        #[test]
        fn test7(data in 0..=u32::MAX) {
            test_type_checker_u32(data as i64);
        }

        #[test]
        fn test8(data in -9100645029148136..=9136655737043548_i64) {
            test_type_checker_s64(data);
        }

        // TODO; Val::U64 takes an i64
        #[test]
        fn test9(data in 0..=i64::MAX) {
            test_type_checker_u64(data);
        }

        #[test]
        fn test10(data in f32::MIN..=f32::MAX) {
            test_type_checker_f32(data);
        }

        #[test]
        fn test11(data in f64::MIN..=f64::MAX) {
            test_type_checker_f64(data);
        }

        #[test]
        fn test_process_record(data in any::<RandomData>()) {
            test_type_checker_record(&data);
        }

        #[test]
        fn test_round_trip(fun_output in any::<FunctionOutputTestResult>()) {
            let validated_output = validate_function_result(&fun_output.val, fun_output.expected_type.clone());

            let validated_input = validate_function_parameters(
                &validated_output.expect("Failed to validate function result"),
                fun_output.expected_type.clone(),
            );

            assert_eq!(validated_input, Ok(fun_output.val.clone()));
        }

        #[test]
        fn test_string(data in any::<String>()) {
            test_type_checker_string(data);
        }
    }

    #[test]
    fn test_validate_function_result_stdio() {
        let str_val = vec![VVal {
            val: Some(Val::String("str".to_string())),
        }];

        let res = str_val.validate_function_result(
            vec![FunctionResult {
                name: Some("a".to_string()),
                tpe: Some(golem_api_grpc::proto::golem::template::Type {
                    r#type: Some(Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::Str as i32,
                    })),
                }),
            }],
            CallingConvention::Stdio,
        );

        assert!(res.is_ok_and(|r| r == Value::String("str".to_string())));

        let num_val = vec![VVal {
            val: Some(Val::String("12.3".to_string())),
        }];

        let res = num_val.validate_function_result(
            vec![FunctionResult {
                name: Some("a".to_string()),
                tpe: Some(golem_api_grpc::proto::golem::template::Type {
                    r#type: Some(Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::F64 as i32,
                    })),
                }),
            }],
            CallingConvention::Stdio,
        );

        assert!(res.is_ok_and(|r| r == Value::Number(serde_json::Number::from_f64(12.3).unwrap())));

        let bool_val = vec![VVal {
            val: Some(Val::String("true".to_string())),
        }];

        let res = bool_val.validate_function_result(
            vec![FunctionResult {
                name: Some("a".to_string()),
                tpe: Some(golem_api_grpc::proto::golem::template::Type {
                    r#type: Some(Type::Primitive(TypePrimitive {
                        primitive: PrimitiveType::Bool as i32,
                    })),
                }),
            }],
            CallingConvention::Stdio,
        );

        assert!(res.is_ok_and(|r| r == Value::Bool(true)));
    }

    #[test]
    fn json_null_works_as_none() {
        let json = Value::Null;
        let result = validate_function_parameters(
            &json,
            Type::Option(Box::new(
                golem_api_grpc::proto::golem::template::TypeOption {
                    elem: Some(Box::new(golem_api_grpc::proto::golem::template::Type {
                        r#type: Some(Type::Primitive(TypePrimitive {
                            primitive: PrimitiveType::Str as i32,
                        })),
                    })),
                },
            )),
        );
        assert_eq!(
            result,
            Ok(Val::Option(Box::new(ValOption {
                discriminant: 0,
                value: None
            })))
        );
    }

    #[test]
    fn missing_field_works_as_none() {
        let json = Value::Object(
            vec![("x".to_string(), Value::String("a".to_string()))]
                .into_iter()
                .collect(),
        );
        let result = validate_function_parameters(
            &json,
            Type::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "x".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Primitive(TypePrimitive {
                                primitive: PrimitiveType::Str as i32,
                            })),
                        }),
                    },
                    NameTypePair {
                        name: "y".to_string(),
                        typ: Some(golem_api_grpc::proto::golem::template::Type {
                            r#type: Some(Type::Option(Box::new(
                                golem_api_grpc::proto::golem::template::TypeOption {
                                    elem: Some(Box::new(
                                        golem_api_grpc::proto::golem::template::Type {
                                            r#type: Some(Type::Primitive(TypePrimitive {
                                                primitive: PrimitiveType::Str as i32,
                                            })),
                                        },
                                    )),
                                },
                            ))),
                        }),
                    },
                ],
            }),
        );
        assert_eq!(
            result,
            Ok(Val::Record(ValRecord {
                values: vec![
                    VVal {
                        val: Some(Val::String("a".to_string()))
                    },
                    VVal {
                        val: Some(Val::Option(Box::new(ValOption {
                            discriminant: 0,
                            value: None
                        })))
                    }
                ]
            }))
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

        let name_type_pairs: Vec<(&String, &Type)> = vec![
            (
                &key1,
                &Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::Str as i32,
                }),
            ),
            (
                &key2,
                &Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::Str as i32,
                }),
            ),
        ];

        let result = get_record(&input_json, name_type_pairs.clone());
        let expected_result = Ok(ValRecord {
            values: vec![
                VVal {
                    val: Some(Val::String("value1".to_string())),
                },
                VVal {
                    val: Some(Val::String("value2".to_string())),
                },
            ],
        });
        assert_eq!(result, expected_result);

        // Test case where a key is missing
        let input_json = json!({
            "key1": "value1",
        });

        let result = get_record(&input_json, name_type_pairs.clone());
        assert!(result.is_err());
    }
}
