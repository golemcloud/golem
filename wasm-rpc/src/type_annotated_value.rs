use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};

use bigdecimal::{BigDecimal, FromPrimitive, ToPrimitive};

use golem_wasm_ast::analysis::{
    AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceId, AnalysedResourceMode,
    AnalysedType,
};

use crate::{Uri, Value, WitValue};

use serde_json::value::Value as JsonValue;

use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeAnnotatedValue {
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
    List {
        typ: AnalysedType,
        values: Vec<TypeAnnotatedValue>,
    },
    Tuple {
        typ: Vec<AnalysedType>,
        value: Vec<TypeAnnotatedValue>,
    },
    Record {
        typ: Vec<(String, AnalysedType)>,
        value: Vec<(String, TypeAnnotatedValue)>,
    },
    Flags {
        typ: Vec<String>,
        values: Vec<String>,
    },
    Variant {
        typ: Vec<(String, Option<AnalysedType>)>,
        case_name: String,
        case_value: Option<Box<TypeAnnotatedValue>>,
    },
    Enum {
        typ: Vec<String>,
        value: String,
    },
    Option {
        typ: AnalysedType,
        value: Option<Box<TypeAnnotatedValue>>,
    },
    Result {
        ok: Option<Box<AnalysedType>>,
        error: Option<Box<AnalysedType>>,
        value: Result<Option<Box<TypeAnnotatedValue>>, Option<Box<TypeAnnotatedValue>>>,
    },
    Handle {
        id: AnalysedResourceId,
        resource_mode: AnalysedResourceMode,
        uri: Uri,
        resource_id: u64,
    },
}

impl TypeAnnotatedValue {
    pub fn from_function_parameters(
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
                match TypeAnnotatedValue::from_json_value(json, &fp.typ) {
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

    pub fn from_function_results(
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

    pub fn from_value(
        val: &Value,
        analysed_type: &AnalysedType,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        match val {
            Value::Bool(bool) => Ok(TypeAnnotatedValue::Bool(*bool)),
            Value::S8(value) => Ok(TypeAnnotatedValue::S8(*value)),
            Value::U8(value) => Ok(TypeAnnotatedValue::U8(*value)),
            Value::U32(value) => Ok(TypeAnnotatedValue::U32(*value)),
            Value::S16(value) => Ok(TypeAnnotatedValue::S16(*value)),
            Value::U16(value) => Ok(TypeAnnotatedValue::U16(*value)),
            Value::S32(value) => Ok(TypeAnnotatedValue::S32(*value)),
            Value::S64(value) => Ok(TypeAnnotatedValue::S64(*value)),
            Value::U64(value) => Ok(TypeAnnotatedValue::U64(*value)),
            Value::F32(value) => Ok(TypeAnnotatedValue::F32(*value)),
            Value::F64(value) => Ok(TypeAnnotatedValue::F64(*value)),
            Value::Char(value) => Ok(TypeAnnotatedValue::Chr(*value)),
            Value::String(value) => Ok(TypeAnnotatedValue::Str(value.clone())),

            Value::Enum(value) => match analysed_type {
                AnalysedType::Enum(names) => match names.get(*value as usize) {
                    Some(str) => Ok(TypeAnnotatedValue::Enum {
                        typ: names.clone(),
                        value: str.to_string(),
                    }),
                    None => Err(vec![format!("Invalid enum {}", value)]),
                },
                _ => Err(vec![format!("Unexpected enum {}", value)]),
            },

            Value::Option(value) => match analysed_type {
                AnalysedType::Option(elem) => Ok(TypeAnnotatedValue::Option {
                    typ: *elem.clone(),
                    value: match value {
                        Some(value) => Some(Box::new(Self::from_value(value, elem)?)),
                        None => None,
                    },
                }),

                _ => Err(vec!["Unexpected type; expected an Option type.".to_string()]),
            },

            Value::Tuple(values) => match analysed_type {
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

                    for (value, tpe) in values.iter().zip(types.iter()) {
                        match Self::from_value(value, tpe) {
                            Ok(result) => results.push(result),
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::Tuple {
                            typ: types.clone(),
                            value: results,
                        })
                    } else {
                        Err(errors)
                    }
                }

                _ => Err(vec!["Unexpected type; expected a tuple type.".to_string()]),
            },

            Value::List(values) => match analysed_type {
                AnalysedType::List(elem) => {
                    let mut errors = vec![];
                    let mut results = vec![];

                    for value in values {
                        match Self::from_value(value, elem) {
                            Ok(value) => results.push(value),
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::List {
                            typ: *elem.clone(),
                            values: results,
                        })
                    } else {
                        Err(errors)
                    }
                }

                _ => Err(vec!["Unexpected type; expected a list type.".to_string()]),
            },

            Value::Record(values) => match analysed_type {
                AnalysedType::Record(fields) => {
                    if values.len() != fields.len() {
                        return Err(vec!["The total number of field values is zero".to_string()]);
                    }

                    let mut errors = vec![];
                    let mut results: Vec<(String, TypeAnnotatedValue)> = vec![];

                    for (value, (field_name, typ)) in values.iter().zip(fields) {
                        match TypeAnnotatedValue::from_value(value, typ) {
                            Ok(res) => {
                                results.push((field_name.clone(), res));
                            }
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::Record {
                            typ: fields.clone(),
                            value: results,
                        })
                    } else {
                        Err(errors)
                    }
                }

                _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
            },

            Value::Variant {
                case_idx,
                case_value,
            } => match analysed_type {
                AnalysedType::Variant(cases) => {
                    if (*case_idx as usize) < cases.len() {
                        let (case_name, case_type) = match cases.get(*case_idx as usize) {
                            Some(tpe) => Ok(tpe),
                            None => {
                                Err(vec!["Variant not found in the expected types.".to_string()])
                            }
                        }?;

                        match case_type {
                            Some(tpe) => match case_value {
                                Some(case_value) => {
                                    let result = Self::from_value(case_value, tpe)?;
                                    Ok(TypeAnnotatedValue::Variant {
                                        typ: cases.clone(),
                                        case_name: case_name.clone(),
                                        case_value: Some(Box::new(result)),
                                    })
                                }
                                None => Err(vec![format!("Missing value for case {case_name}")]),
                            },
                            None => Ok(TypeAnnotatedValue::Variant {
                                typ: cases.clone(),
                                case_name: case_name.clone(),
                                case_value: None,
                            }),
                        }
                    } else {
                        Err(vec![
                            "Invalid discriminant value for the variant.".to_string()
                        ])
                    }
                }

                _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
            },

            Value::Flags(values) => match analysed_type {
                AnalysedType::Flags(names) => {
                    let mut results = vec![];

                    if values.len() != names.len() {
                        Err(vec![format!(
                            "Unexpected number of flag states: {:?} vs {:?}",
                            values, names
                        )])
                    } else {
                        for (enabled, name) in values.iter().zip(names) {
                            if *enabled {
                                results.push(name.clone());
                            }
                        }

                        Ok(TypeAnnotatedValue::Flags {
                            typ: names.clone(),
                            values: results,
                        })
                    }
                }
                _ => Err(vec!["Unexpected type; expected a flags type.".to_string()]),
            },

            Value::Result(value) => match analysed_type {
                golem_wasm_ast::analysis::AnalysedType::Result { ok, error } => {
                    match (value, ok, error) {
                        (Ok(Some(value)), Some(ok_type), _) => {
                            let result = Self::from_value(value, ok_type)?;

                            Ok(TypeAnnotatedValue::Result {
                                value: Ok(Some(Box::new(result))),
                                ok: ok.clone(),
                                error: error.clone(),
                            })
                        }

                        (Ok(None), Some(_), _) => {
                            Err(vec!["Non-unit ok result has no value".to_string()])
                        }

                        (Ok(None), None, _) => Ok(TypeAnnotatedValue::Result {
                            value: Ok(None),
                            ok: ok.clone(),
                            error: error.clone(),
                        }),

                        (Ok(Some(_)), None, _) => {
                            Err(vec!["Unit ok result has a value".to_string()])
                        }

                        (Err(Some(value)), _, Some(err_type)) => {
                            let result = Self::from_value(value, err_type)?;

                            Ok(TypeAnnotatedValue::Result {
                                value: Err(Some(Box::new(result))),
                                ok: ok.clone(),
                                error: error.clone(),
                            })
                        }

                        (Err(None), _, Some(_)) => {
                            Err(vec!["Non-unit error result has no value".to_string()])
                        }

                        (Err(None), _, None) => Ok(TypeAnnotatedValue::Result {
                            value: Err(None),
                            ok: ok.clone(),
                            error: error.clone(),
                        }),

                        (Err(Some(_)), _, None) => {
                            Err(vec!["Unit error result has a value".to_string()])
                        }
                    }
                }

                _ => Err(vec!["Unexpected type; expected a Result type.".to_string()]),
            },
            Value::Handle { uri, resource_id } => match analysed_type {
                AnalysedType::Resource { id, resource_mode } => Ok(TypeAnnotatedValue::Handle {
                    id: id.clone(),
                    resource_mode: resource_mode.clone(),
                    uri: uri.clone(),
                    resource_id: *resource_id,
                }),
                _ => Err(vec!["Unexpected type; expected a Handle type.".to_string()]),
            },
        }
    }

    pub fn from_json_value(
        json_val: &JsonValue,
        analysed_type: &AnalysedType,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        match analysed_type {
            AnalysedType::Bool => TypeAnnotatedValue::get_bool(json_val),
            AnalysedType::S8 => TypeAnnotatedValue::get_s8(json_val),
            AnalysedType::U8 => TypeAnnotatedValue::get_u8(json_val),
            AnalysedType::S16 => TypeAnnotatedValue::get_s16(json_val),
            AnalysedType::U16 => TypeAnnotatedValue::get_u16(json_val),
            AnalysedType::S32 => TypeAnnotatedValue::get_s32(json_val),
            AnalysedType::U32 => TypeAnnotatedValue::get_u32(json_val),
            AnalysedType::S64 => TypeAnnotatedValue::get_s64(json_val),
            AnalysedType::U64 => TypeAnnotatedValue::get_u64(json_val),
            AnalysedType::F64 => TypeAnnotatedValue::get_f64(json_val),
            AnalysedType::F32 => TypeAnnotatedValue::get_f32(json_val),
            AnalysedType::Chr => TypeAnnotatedValue::get_char(json_val),
            AnalysedType::Str => TypeAnnotatedValue::get_string(json_val),
            AnalysedType::Enum(names) => TypeAnnotatedValue::get_enum(json_val, names),
            AnalysedType::Flags(names) => TypeAnnotatedValue::get_flag(json_val, names),
            AnalysedType::List(elem) => TypeAnnotatedValue::get_list(json_val, elem),
            AnalysedType::Option(elem) => TypeAnnotatedValue::get_option(json_val, elem),
            AnalysedType::Result { ok, error } => {
                TypeAnnotatedValue::get_result(json_val, ok, error)
            }
            AnalysedType::Record(fields) => TypeAnnotatedValue::get_record(json_val, fields),
            AnalysedType::Variant(cases) => TypeAnnotatedValue::get_variant(json_val, cases),
            AnalysedType::Tuple(elems) => TypeAnnotatedValue::get_tuple(json_val, elems),
            AnalysedType::Resource { id, resource_mode } => {
                TypeAnnotatedValue::get_handle(json_val, id.clone(), resource_mode.clone())
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
        .map(|num| {
            TypeAnnotatedValue::S16(num.to_i16().expect("Failed to convert BigDecimal to i16"))
        })
    }

    fn get_u16(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        ensure_range(
            json,
            BigDecimal::from_u16(u16::MIN).expect("Failed to convert u16::MIN to BigDecimal"),
            BigDecimal::from_u16(u16::MAX).expect("Failed to convert u16::MAX to BigDecimal"),
        )
        .map(|num| {
            TypeAnnotatedValue::U16(num.to_u16().expect("Failed to convert BigDecimal to u16"))
        })
    }

    fn get_s32(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        ensure_range(
            json,
            BigDecimal::from_i32(i32::MIN).expect("Failed to convert i32::MIN to BigDecimal"),
            BigDecimal::from_i32(i32::MAX).expect("Failed to convert i32::MAX to BigDecimal"),
        )
        .map(|num| {
            TypeAnnotatedValue::S32(num.to_i32().expect("Failed to convert BigDecimal to i32"))
        })
    }

    fn get_u32(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        ensure_range(
            json,
            BigDecimal::from_u32(u32::MIN).expect("Failed to convert u32::MIN to BigDecimal"),
            BigDecimal::from_u32(u32::MAX).expect("Failed to convert u32::MAX to BigDecimal"),
        )
        .map(|num| {
            TypeAnnotatedValue::U32(num.to_u32().expect("Failed to convert BigDecimal to u32"))
        })
    }

    fn get_s64(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        ensure_range(
            json,
            BigDecimal::from_i64(i64::MIN).expect("Failed to convert i64::MIN to BigDecimal"),
            BigDecimal::from_i64(i64::MAX).expect("Failed to convert i64::MAX to BigDecimal"),
        )
        .map(|num| {
            TypeAnnotatedValue::S64(num.to_i64().expect("Failed to convert BigDecimal to i64"))
        })
    }

    fn get_f32(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        ensure_range(
            json,
            BigDecimal::from_f32(f32::MIN).expect("Failed to convert f32::MIN to BigDecimal"),
            BigDecimal::from_f32(f32::MAX).expect("Failed to convert f32::MAX to BigDecimal"),
        )
        .map(|num| {
            TypeAnnotatedValue::F32(num.to_f32().expect("Failed to convert BigDecimal to f32"))
        })
    }

    fn get_f64(json_val: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        let num = bigdecimal(json_val)?;
        let value = TypeAnnotatedValue::F64(
            num.to_string()
                .parse()
                .map_err(|err| vec![format!("Failed to parse f64: {}", err)])?,
        );
        Ok(value)
    }

    fn get_string(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        get_string(json).map(TypeAnnotatedValue::Str)
    }

    fn get_u64(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        get_u64(json).map(TypeAnnotatedValue::U64)
    }

    fn get_char(json: &JsonValue) -> Result<TypeAnnotatedValue, Vec<String>> {
        get_char(json).map(TypeAnnotatedValue::Chr)
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
            match TypeAnnotatedValue::from_json_value(json, tpe) {
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

            None => TypeAnnotatedValue::from_json_value(input_json, tpe).map(|result| {
                TypeAnnotatedValue::Option {
                    typ: tpe.clone(),
                    value: Some(Box::new(result)),
                }
            }),
        }
    }

    fn get_list(
        input_json: &JsonValue,
        tpe: &AnalysedType,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let json_array = input_json
            .as_array()
            .ok_or(vec![format!("Input {} is not an array", input_json)])?;

        let mut errors: Vec<String> = vec![];
        let mut vals: Vec<TypeAnnotatedValue> = vec![];

        for json in json_array {
            match TypeAnnotatedValue::from_json_value(json, tpe) {
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

    fn get_enum(
        input_json: &JsonValue,
        names: &[String],
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
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
                TypeAnnotatedValue::from_json_value(input_json, typ).map(|v| Some(Box::new(v)))
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
                match TypeAnnotatedValue::from_json_value(json_value, tpe) {
                    Ok(result) => vals.push((name.clone(), result)),
                    Err(value_errors) => errors.extend(
                        value_errors
                            .iter()
                            .map(|err| {
                                format!("Invalid value for the key {}. Error: {}", name, err)
                            })
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

    fn get_flag(
        input_json: &JsonValue,
        names: &[String],
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
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
            Some(Some(tpe)) => TypeAnnotatedValue::from_json_value(json, tpe).map(|result| {
                TypeAnnotatedValue::Variant {
                    typ: types.to_vec(),
                    case_name: key.clone(),
                    case_value: Some(Box::new(result)),
                }
            }),
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

impl From<TypeAnnotatedValue> for AnalysedType {
    fn from(value: TypeAnnotatedValue) -> Self {
        match value {
            TypeAnnotatedValue::Bool(_) => AnalysedType::Bool,
            TypeAnnotatedValue::S8(_) => AnalysedType::S8,
            TypeAnnotatedValue::U8(_) => AnalysedType::U8,
            TypeAnnotatedValue::S16(_) => AnalysedType::S16,
            TypeAnnotatedValue::U16(_) => AnalysedType::U16,
            TypeAnnotatedValue::S32(_) => AnalysedType::S32,
            TypeAnnotatedValue::U32(_) => AnalysedType::U32,
            TypeAnnotatedValue::S64(_) => AnalysedType::S64,
            TypeAnnotatedValue::U64(_) => AnalysedType::U64,
            TypeAnnotatedValue::F32(_) => AnalysedType::F32,
            TypeAnnotatedValue::F64(_) => AnalysedType::F64,
            TypeAnnotatedValue::Chr(_) => AnalysedType::Chr,
            TypeAnnotatedValue::Str(_) => AnalysedType::Str,
            TypeAnnotatedValue::List { typ, values: _ } => AnalysedType::List(Box::new(typ)),
            TypeAnnotatedValue::Tuple { typ, value: _ } => AnalysedType::Tuple(typ),
            TypeAnnotatedValue::Record { typ, value: _ } => AnalysedType::Record(typ),
            TypeAnnotatedValue::Flags { typ, values: _ } => AnalysedType::Flags(typ),
            TypeAnnotatedValue::Enum { typ, value: _ } => AnalysedType::Enum(typ),
            TypeAnnotatedValue::Option { typ, value: _ } => AnalysedType::Option(Box::new(typ)),
            TypeAnnotatedValue::Result {
                ok,
                error,
                value: _,
            } => AnalysedType::Result { ok, error },
            TypeAnnotatedValue::Handle {
                id,
                resource_mode,
                uri: _,
                resource_id: _,
            } => AnalysedType::Resource { id, resource_mode },
            TypeAnnotatedValue::Variant {
                typ,
                case_name: _,
                case_value: _,
            } => AnalysedType::Variant(typ),
        }
    }
}
impl TryFrom<TypeAnnotatedValue> for WitValue {
    type Error = String;
    fn try_from(value: TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let value: Result<Value, String> = value.try_into();
        value.map(|v| v.into())
    }
}

impl TryFrom<TypeAnnotatedValue> for Value {
    type Error = String;

    fn try_from(value: TypeAnnotatedValue) -> Result<Self, Self::Error> {
        match value {
            TypeAnnotatedValue::Bool(value) => Ok(Value::Bool(value)),
            TypeAnnotatedValue::S8(value) => Ok(Value::S8(value)),
            TypeAnnotatedValue::U8(value) => Ok(Value::U8(value)),
            TypeAnnotatedValue::S16(value) => Ok(Value::S16(value)),
            TypeAnnotatedValue::U16(value) => Ok(Value::U16(value)),
            TypeAnnotatedValue::S32(value) => Ok(Value::S32(value)),
            TypeAnnotatedValue::U32(value) => Ok(Value::U32(value)),
            TypeAnnotatedValue::S64(value) => Ok(Value::S64(value)),
            TypeAnnotatedValue::U64(value) => Ok(Value::U64(value)),
            TypeAnnotatedValue::F32(value) => Ok(Value::F32(value)),
            TypeAnnotatedValue::F64(value) => Ok(Value::F64(value)),
            TypeAnnotatedValue::Chr(value) => Ok(Value::Char(value)),
            TypeAnnotatedValue::Str(value) => Ok(Value::String(value)),
            TypeAnnotatedValue::List { typ: _, values } => {
                let mut list_values = Vec::new();
                for value in values {
                    match value.try_into() {
                        Ok(v) => list_values.push(v),
                        Err(err) => return Err(err),
                    }
                }
                Ok(Value::List(list_values))
            }
            TypeAnnotatedValue::Tuple { typ: _, value } => {
                let mut tuple_values = Vec::new();
                for value in value {
                    match value.try_into() {
                        Ok(v) => tuple_values.push(v),
                        Err(err) => return Err(err),
                    }
                }
                Ok(Value::Tuple(tuple_values))
            }
            TypeAnnotatedValue::Record { typ: _, value } => {
                let mut record_values = Vec::new();
                for (_, value) in value {
                    match value.try_into() {
                        Ok(v) => record_values.push(v),
                        Err(err) => return Err(err),
                    }
                }
                Ok(Value::Record(record_values))
            }
            TypeAnnotatedValue::Flags { typ, values } => {
                let mut bools = Vec::new();

                for expected_flag in typ {
                    if values.contains(&expected_flag) {
                        bools.push(true);
                    } else {
                        bools.push(false);
                    }
                }
                Ok(Value::Flags(bools))
            }
            TypeAnnotatedValue::Enum { typ, value } => typ
                .iter()
                .position(|expected_enum| expected_enum == &value)
                .map(|index| Value::Enum(index as u32))
                .ok_or_else(|| "Enum value not found in the list of expected enums".to_string()),

            TypeAnnotatedValue::Option { typ: _, value } => match value {
                Some(value) => {
                    let value: Value = value.deref().clone().try_into()?;
                    Ok(Value::Option(Some(Box::new(value))))
                }
                None => Ok(Value::Option(None)),
            },
            TypeAnnotatedValue::Result {
                ok: _,
                error: _,
                value,
            } => Ok(Value::Result(match value {
                Ok(value) => match value {
                    Some(v) => {
                        let value: Value = v.deref().clone().try_into()?;
                        Ok(Some(Box::new(value)))
                    }

                    None => Ok(None),
                },
                Err(value) => match value {
                    Some(v) => {
                        let value: Value = v.deref().clone().try_into()?;
                        Err(Some(Box::new(value)))
                    }

                    None => Err(None),
                },
            })),
            TypeAnnotatedValue::Handle {
                id: _,
                resource_mode: _,
                uri,
                resource_id,
            } => Ok(Value::Handle { uri, resource_id }),
            TypeAnnotatedValue::Variant {
                typ,
                case_name,
                case_value,
            } => match case_value {
                Some(value) => {
                    let result: Value = value.deref().clone().try_into()?;
                    Ok(Value::Variant {
                        case_idx: typ.iter().position(|(name, _)| name == &case_name).unwrap()
                            as u32,
                        case_value: Some(Box::new(result)),
                    })
                }
                None => Ok(Value::Variant {
                    case_idx: typ.iter().position(|(name, _)| name == &case_name).unwrap() as u32,
                    case_value: None,
                }),
            },
        }
    }
}
