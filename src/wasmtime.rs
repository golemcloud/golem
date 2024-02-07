use crate::Value;
use wasmtime::component::{
    types, Enum, Flags, List, OptionVal, Record, ResultVal, Tuple, Type, Val, Variant,
};

enum EncodingError {
    ParamTypeMismatch,
    ValueMismatch { details: String },
    Unknown { details: String },
}

/// Converts a Value to a wasmtime Val based on the available type information.
pub fn decode_param(param: &Value, param_type: &Type) -> Result<Val, EncodingError> {
    match param_type {
        Type::Bool => match param {
            Value::Bool(bool) => Ok(Val::Bool(*bool)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S8 => match param {
            Value::S8(s8) => Ok(Val::S8(*s8)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U8 => match param {
            Value::U8(u8) => Ok(Val::U8(*u8)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S16 => match param {
            Value::S16(s16) => Ok(Val::S16(*s16)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U16 => match param {
            Value::U16(u16) => Ok(Val::U16(*u16)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S32 => match param {
            Value::S32(s32) => Ok(Val::S32(*s32)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U32 => match param {
            Value::U32(u32) => Ok(Val::U32(*u32)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S64 => match param {
            Value::S64(s64) => Ok(Val::S64(*s64)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U64 => match param {
            Value::U64(u64) => Ok(Val::U64(*u64)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Float32 => match param {
            Value::F32(f32) => Ok(Val::Float32(*f32)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Float64 => match param {
            Value::F64(f64) => Ok(Val::Float64(*f64)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Char => match param {
            Value::Char(char) => Ok(Val::Char(*char)),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::String => match param {
            Value::String(string) => Ok(Val::String(string.clone().into_boxed_str())),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::List(ty) => match param {
            Value::List(values) => {
                let decoded_values = values
                    .iter()
                    .map(|v| decode_param(v, &ty.ty()))
                    .collect::<Result<Vec<Val>, EncodingError>>()?;
                let list = List::new(ty, decoded_values.into_boxed_slice())
                    .expect("Type mismatch in decode_param");
                Ok(Val::List(list))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Record(ty) => match param {
            Value::Record(values) => {
                let record_values = values
                    .iter()
                    .zip(ty.fields())
                    .map(|(value, field)| {
                        let decoded_param = decode_param(value, &field.ty)?;
                        Ok((field.name, decoded_param))
                    })
                    .collect::<Result<Vec<(&str, Val)>, EncodingError>>()?;
                let record = Record::new(ty, record_values).expect("Type mismatch in decode_param");
                Ok(Val::Record(record))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Tuple(ty) => match param {
            Value::Tuple(values) => {
                let tuple_values: Vec<Val> = values
                    .iter()
                    .zip(ty.types())
                    .map(|(value, ty)| decode_param(value, &ty))
                    .collect::<Result<Vec<Val>, EncodingError>>()?;
                let tuple = Tuple::new(ty, tuple_values.into_boxed_slice())
                    .expect("Type mismatch in decode_param");
                Ok(Val::Tuple(tuple))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Variant(ty) => match param {
            Value::Variant {
                case_idx,
                case_value,
            } => {
                let cases: Vec<types::Case> = ty.cases().collect();
                let case = cases
                    .get(*case_idx as usize)
                    .ok_or(EncodingError::ValueMismatch {
                        details: format!("could not get case for discriminant {}", case_idx),
                    })?;
                let name = case.name;
                let case_ty = match case.ty {
                    Some(ref ty) => Ok(ty),
                    None => Err(EncodingError::ValueMismatch {
                        details: format!("could not get type information for case {}", name),
                    }),
                }?;
                let decoded_value = decode_param(&case_value, case_ty)?;
                let variant = Variant::new(ty, name, Some(decoded_value))
                    .expect("Type mismatch in decode_param");
                Ok(Val::Variant(variant))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Enum(ty) => match param {
            Value::Enum(discriminant) => {
                let names: Vec<&str> = ty.names().collect();
                let name: &str =
                    names
                        .get(*discriminant as usize)
                        .ok_or(EncodingError::ValueMismatch {
                            details: format!(
                                "could not get name for discriminant {}",
                                discriminant
                            ),
                        })?;
                let enum0 = Enum::new(ty, name).expect("Type mismatch in decode_param");
                Ok(Val::Enum(enum0))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Option(ty) => match param {
            Value::Option(value) => match value {
                Some(value) => {
                    let decoded_value = decode_param(&value, &ty.ty())?;
                    let option = OptionVal::new(ty, Some(decoded_value))
                        .expect("Type mismatch in decode_param");
                    Ok(Val::Option(option))
                }
                None => {
                    let option = OptionVal::new(ty, None).expect("Type mismatch in decode_param");
                    Ok(Val::Option(option))
                }
            },
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Result(ty) => match param {
            Value::Result(result) => match result {
                Ok(value) => {
                    let ok_ty = ty.ok().ok_or(EncodingError::ValueMismatch {
                        details: "could not get ok type".to_string(),
                    })?;
                    let decoded_value = decode_param(&value, &ok_ty)?;
                    let result = ResultVal::new(ty, Ok(Some(decoded_value)))
                        .expect("Type mismatch in decode_param");
                    Ok(Val::Result(result))
                }
                Err(value) => {
                    let err_ty = ty.err().ok_or(EncodingError::ValueMismatch {
                        details: "could not get err type".to_string(),
                    })?;
                    let decoded_value = decode_param(&value, &err_ty)?;
                    let result = ResultVal::new(ty, Err(Some(decoded_value)))
                        .expect("Type mismatch in decode_param");
                    Ok(Val::Result(result))
                }
            },
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Flags(ty) => match param {
            Value::Flags(flags) => {
                let flag_names = ty.names().collect::<Vec<&str>>();
                let active_flags: Vec<&str> = flag_names
                    .iter()
                    .zip(flags)
                    .filter_map(|(name, enabled)| if *enabled { Some(*name) } else { None })
                    .collect();
                let flags =
                    Flags::new(ty, active_flags.as_slice()).expect("Type mismatch in decode_param");
                Ok(Val::Flags(flags))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Own(_) => Err(EncodingError::ParamTypeMismatch),
        Type::Borrow(_) => Err(EncodingError::ParamTypeMismatch),
    }
}

/// Converts a wasmtime Val to a Golem protobuf Val
pub fn encode_output(value: &Val) -> Result<Value, EncodingError> {
    match value {
        Val::Bool(bool) => Ok(Value::Bool(*bool)),
        Val::S8(i8) => Ok(Value::S8(*i8)),
        Val::U8(u8) => Ok(Value::U8(*u8)),
        Val::S16(i16) => Ok(Value::S16(*i16)),
        Val::U16(u16) => Ok(Value::U16(*u16)),
        Val::S32(i32) => Ok(Value::S32(*i32)),
        Val::U32(u32) => Ok(Value::U32(*u32)),
        Val::S64(i64) => Ok(Value::S64(*i64)),
        Val::U64(u64) => Ok(Value::U64(*u64)),
        Val::Float32(f32) => Ok(Value::F32(*f32)),
        Val::Float64(f64) => Ok(Value::F64(*f64)),
        Val::Char(char) => Ok(Value::Char(*char)),
        Val::String(string) => Ok(Value::String(string.to_string())),
        Val::List(list) => {
            let mut encoded_values = Vec::new();
            for value in (*list).iter() {
                encoded_values.push(encode_output(value)?);
            }
            Ok(Value::List(encoded_values))
        }
        Val::Record(record) => {
            let encoded_values = record
                .fields()
                .map(|(_, value)| encode_output(value))
                .collect::<Result<Vec<Value>, EncodingError>>()?;
            Ok(Value::Record(encoded_values))
        }
        Val::Tuple(tuple) => {
            let encoded_values = tuple
                .values()
                .iter()
                .map(|value| encode_output(value))
                .collect::<Result<Vec<Value>, EncodingError>>()?;
            Ok(Value::Tuple(encoded_values))
        }
        Val::Variant(variant) => {
            let wasm_variant = unsafe { std::mem::transmute(variant.clone()) };
            let WasmVariant {
                ty: _,
                discriminant,
                value,
            } = wasm_variant;
            let encoded_output = encode_output(value.as_ref().ok_or(EncodingError::Unknown {
                details: "Missing value in variant".to_string(),
            })?)?;
            Ok(Value::Variant {
                case_idx: discriminant,
                case_value: Box::new(encoded_output),
            })
        }
        Val::Enum(enum0) => {
            let wasm_enum = unsafe { std::mem::transmute(enum0.clone()) };
            let WasmEnum {
                ty: _,
                discriminant,
            } = wasm_enum;
            Ok(Value::Enum(discriminant))
        }
        Val::Option(option) => match option.value() {
            Some(value) => {
                let encoded_output = encode_output(value)?;
                Ok(Value::Option(Some(Box::new(encoded_output))))
            }
            None => Ok(Value::Option(None)),
        },
        Val::Result(result) => match result.value() {
            Ok(value) => {
                let encoded_output = encode_output(value.ok_or(EncodingError::Unknown {
                    details: "Missing Ok value in result".to_string(),
                })?)?; // TODO: error
                Ok(Value::Result(Ok(Box::new(encoded_output))))
            }
            Err(value) => {
                let encoded_output = encode_output(value.ok_or(EncodingError::Unknown {
                    details: "Missing Err value in result".to_string(),
                })?)?; // TODO: error
                Ok(Value::Result(Err(Box::new(encoded_output))))
            }
        },
        Val::Flags(flags) => {
            let wasm_flags = unsafe { std::mem::transmute(flags.clone()) };
            let WasmFlags {
                ty: _,
                count,
                value,
            } = wasm_flags;
            let mut encoded_value = vec![false; count as usize];

            for v in value.iter() {
                for n in 0..count {
                    let flag = 1 << n;
                    if flag & *v as i32 != 0 {
                        encoded_value[n as usize] = true;
                    }
                }
            }
            Ok(Value::Flags(encoded_value))
        }
        Val::Resource(_) => Err(EncodingError::Unknown {
            details: "resource values are not supported yet".to_string(),
        }),
    }
}

#[allow(unused)]
pub struct WasmVariant {
    ty: wasmtime::component::types::Variant,
    discriminant: u32,
    value: Option<Box<Val>>,
}

#[allow(unused)]
pub struct WasmEnum {
    ty: wasmtime::component::types::Variant,
    discriminant: u32,
}

#[allow(unused)]
pub struct WasmFlags {
    ty: wasmtime::component::types::Flags,
    count: u32,
    value: Box<[u32]>,
}
