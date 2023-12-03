use std::ops::Deref;

use golem_common::proto::golem;
use wasmtime::component::*;

use crate::error::GolemError;

pub fn decode_param(param: &golem::Val, param_type: &Type) -> Result<Val, GolemError> {
    match param_type {
        Type::Bool => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Bool(bool) => Ok(Val::Bool(bool)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::S8 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::S8(s8) => Ok(Val::S8(s8 as i8)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::U8 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::U8(u8) => Ok(Val::U8(u8 as u8)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::S16 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::S16(s16) => Ok(Val::S16(s16 as i16)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::U16 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::U16(u16) => Ok(Val::U16(u16 as u16)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::S32 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::S32(s32) => Ok(Val::S32(s32)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::U32 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::U32(u32) => Ok(Val::U32(u32 as u32)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::S64 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::S64(s64) => Ok(Val::S64(s64)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::U64 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::U64(u64) => Ok(Val::U64(u64 as u64)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Float32 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::F32(f32) => Ok(Val::Float32(f32)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Float64 => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::F64(f64) => Ok(Val::Float64(f64)),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Char => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Char(i32) => {
                    let char =
                        std::char::from_u32(i32 as u32).ok_or(GolemError::ValueMismatch {
                            details: format!("could not convert {} to char", i32),
                        })?;
                    Ok(Val::Char(char))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::String => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::String(string) => Ok(Val::String(string.into_boxed_str())),
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::List(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::List(golem::ValList { values, .. }) => {
                    let decoded_values = values
                        .iter()
                        .map(|v| decode_param(v, &ty.ty()))
                        .collect::<Result<Vec<Val>, GolemError>>()?;
                    let list = List::new(ty, decoded_values.into_boxed_slice())
                        .expect("Type mismatch in decode_param");
                    Ok(Val::List(list))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Record(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Record(golem::ValRecord { values, .. }) => {
                    let record_values = values
                        .iter()
                        .zip(ty.fields())
                        .map(|(value, field)| {
                            let decoded_param = decode_param(value, &field.ty)?;
                            Ok((field.name, decoded_param))
                        })
                        .collect::<Result<Vec<(&str, Val)>, GolemError>>()?;
                    let record =
                        Record::new(ty, record_values).expect("Type mismatch in decode_param");
                    Ok(Val::Record(record))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Tuple(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Tuple(golem::ValTuple { values, .. }) => {
                    let tuple_values: Vec<Val> = values
                        .iter()
                        .zip(ty.types())
                        .map(|(value, ty)| decode_param(value, &ty))
                        .collect::<Result<Vec<Val>, GolemError>>()?;
                    let tuple = Tuple::new(ty, tuple_values.into_boxed_slice())
                        .expect("Type mismatch in decode_param");
                    Ok(Val::Tuple(tuple))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Variant(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Variant(variant) => {
                    let golem::ValVariant {
                        discriminant,
                        value,
                        ..
                    } = *variant;
                    let cases: Vec<types::Case> = ty.cases().collect();
                    let case =
                        cases
                            .get(discriminant as usize)
                            .ok_or(GolemError::ValueMismatch {
                                details: format!(
                                    "could not get case for discriminant {}",
                                    discriminant
                                ),
                            })?;
                    let name = case.name;
                    let case_ty = match case.ty {
                        Some(ref ty) => Ok(ty),
                        None => Err(GolemError::ValueMismatch {
                            details: format!("could not get type information for case {}", name),
                        }),
                    }?;
                    let decoded_value = match value {
                        Some(value) => Some(decode_param(&value, case_ty)?),
                        None => None,
                    };
                    let variant = Variant::new(ty, name, decoded_value)
                        .expect("Type mismatch in decode_param");
                    Ok(Val::Variant(variant))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Enum(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Enum(golem::ValEnum { discriminant, .. }) => {
                    let names: Vec<&str> = ty.names().collect();
                    let name: &str =
                        names
                            .get(discriminant as usize)
                            .ok_or(GolemError::ValueMismatch {
                                details: format!(
                                    "could not get name for discriminant {}",
                                    discriminant
                                ),
                            })?;
                    let enum0 = Enum::new(ty, name).expect("Type mismatch in decode_param");
                    Ok(Val::Enum(enum0))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Option(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Option(option) => {
                    let golem::ValOption { value, .. } = *option;
                    match value {
                        Some(value) => {
                            let decoded_value = decode_param(&value, &ty.ty())?;
                            let option = OptionVal::new(ty, Some(decoded_value))
                                .expect("Type mismatch in decode_param");
                            Ok(Val::Option(option))
                        }
                        None => {
                            let option =
                                OptionVal::new(ty, None).expect("Type mismatch in decode_param");
                            Ok(Val::Option(option))
                        }
                    }
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Result(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Result(result) => {
                    let golem::ValResult {
                        discriminant,
                        value,
                        ..
                    } = *result;
                    if discriminant == 0 {
                        let ok_ty = ty.ok().ok_or(GolemError::ValueMismatch {
                            details: "could not get ok type".to_string(),
                        })?;
                        let decoded_value = value
                            .map(|value| decode_param(&value, &ok_ty))
                            .transpose()?;
                        let result = ResultVal::new(ty, Ok(decoded_value))
                            .expect("Type mismatch in decode_param");
                        Ok(Val::Result(result))
                    } else {
                        let err_ty = ty.err().ok_or(GolemError::ValueMismatch {
                            details: "could not get err type".to_string(),
                        })?;
                        let decoded_value = value
                            .map(|value| decode_param(&value, &err_ty))
                            .transpose()?;
                        let result = ResultVal::new(ty, Err(decoded_value))
                            .expect("Type mismatch in decode_param");
                        Ok(Val::Result(result))
                    }
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Flags(ty) => {
            let val = &param.val;
            match val.clone().ok_or(GolemError::NoValueInMessage)? {
                golem::val::Val::Flags(flags) => {
                    let flag_names = ty.names().collect::<Vec<&str>>();
                    let active_flags: Vec<&str> = flag_names
                        .iter()
                        .enumerate()
                        .filter_map(|(i, flag)| {
                            let z: i32 = i.try_into().ok()?;
                            if flags.value.contains(&z) {
                                Some(*flag)
                            } else {
                                None
                            }
                        })
                        .collect();
                    let flags = Flags::new(ty, active_flags.as_slice())
                        .expect("Type mismatch in decode_param");
                    Ok(Val::Flags(flags))
                }
                _ => Err(GolemError::ParamTypeMismatch),
            }
        }
        Type::Own(_) => Err(GolemError::ParamTypeMismatch),
        Type::Borrow(_) => Err(GolemError::ParamTypeMismatch),
    }
}

pub fn encode_output(value: &Val) -> Result<golem::Val, GolemError> {
    match value {
        Val::Bool(bool) => Ok(golem::Val {
            val: Some(golem::val::Val::Bool(*bool)),
        }),
        Val::S8(i8) => Ok(golem::Val {
            val: Some(golem::val::Val::S8(*i8 as i32)),
        }),
        Val::U8(u8) => Ok(golem::Val {
            val: Some(golem::val::Val::U8(*u8 as i32)),
        }),
        Val::S16(i16) => Ok(golem::Val {
            val: Some(golem::val::Val::S16(*i16 as i32)),
        }),
        Val::U16(u16) => Ok(golem::Val {
            val: Some(golem::val::Val::U16(*u16 as i32)),
        }),
        Val::S32(i32) => Ok(golem::Val {
            val: Some(golem::val::Val::S32(*i32)),
        }),
        Val::U32(u32) => {
            let i32 = *u32 as i64;
            Ok(golem::Val {
                val: Some(golem::val::Val::U32(i32)),
            })
        }
        Val::S64(i64) => Ok(golem::Val {
            val: Some(golem::val::Val::S64(*i64)),
        }),
        Val::U64(u64) => {
            let i64 = *u64 as i64;
            Ok(golem::Val {
                val: Some(golem::val::Val::U64(i64)),
            })
        }
        Val::Float32(f32) => Ok(golem::Val {
            val: Some(golem::val::Val::F32(*f32)),
        }),
        Val::Float64(f64) => Ok(golem::Val {
            val: Some(golem::val::Val::F64(*f64)),
        }),
        Val::Char(char) => Ok(golem::Val {
            val: Some(golem::val::Val::Char(*char as i32)),
        }),
        Val::String(string) => Ok(golem::Val {
            val: Some(golem::val::Val::String(string.to_string())),
        }),
        Val::List(list) => {
            let values = list.deref();
            let mut encoded_values = Vec::new();
            for value in values.iter() {
                encoded_values.push(encode_output(value)?);
            }
            Ok(golem::Val {
                val: Some(golem::val::Val::List(golem::ValList {
                    values: encoded_values,
                })),
            })
        }
        Val::Record(record) => {
            let encoded_values = record
                .fields()
                .map(|(_, value)| encode_output(value))
                .collect::<Result<Vec<golem::Val>, GolemError>>()?;
            Ok(golem::Val {
                val: Some(golem::val::Val::Record(golem::ValRecord {
                    values: encoded_values,
                })),
            })
        }
        Val::Tuple(tuple) => {
            let encoded_values = tuple
                .values()
                .iter()
                .map(encode_output)
                .collect::<Result<Vec<golem::Val>, GolemError>>()?;
            Ok(golem::Val {
                val: Some(golem::val::Val::Tuple(golem::ValTuple {
                    values: encoded_values,
                })),
            })
        }
        Val::Variant(variant) => {
            let wasm_variant = unsafe { std::mem::transmute(variant.clone()) };
            let WasmVariant {
                ty: _,
                discriminant,
                value,
            } = wasm_variant;
            let encoded_output = value.map(|value| encode_output(&value)).transpose()?;
            Ok(golem::Val {
                val: Some(golem::val::Val::Variant(Box::new(golem::ValVariant {
                    discriminant: discriminant as i32,
                    value: encoded_output.map(Box::new),
                }))),
            })
        }
        Val::Enum(enum0) => {
            let wasm_enum = unsafe { std::mem::transmute(enum0.clone()) };
            let WasmEnum {
                ty: _,
                discriminant,
            } = wasm_enum;
            Ok(golem::Val {
                val: Some(golem::val::Val::Enum(golem::ValEnum {
                    discriminant: discriminant as i32,
                })),
            })
        }
        Val::Option(option) => match option.value() {
            Some(value) => {
                let encoded_output = encode_output(value)?;
                Ok(golem::Val {
                    val: Some(golem::val::Val::Option(Box::new(golem::ValOption {
                        discriminant: 1,
                        value: Some(Box::new(encoded_output)),
                    }))),
                })
            }
            None => Ok(golem::Val {
                val: Some(golem::val::Val::Option(Box::new(golem::ValOption {
                    discriminant: 0,
                    value: None,
                }))),
            }),
        },
        Val::Result(result) => {
            log::debug!("encoding result: {:?}", result);
            match result.value() {
                Ok(value) => {
                    let encoded_output = value.map(encode_output).transpose()?;
                    Ok(golem::Val {
                        val: Some(golem::val::Val::Result(Box::new(golem::ValResult {
                            discriminant: 0,
                            value: encoded_output.map(Box::new),
                        }))),
                    })
                }
                Err(value) => {
                    let encoded_output = value.map(encode_output).transpose()?;
                    Ok(golem::Val {
                        val: Some(golem::val::Val::Result(Box::new(golem::ValResult {
                            discriminant: 1,
                            value: encoded_output.map(Box::new),
                        }))),
                    })
                }
            }
        }
        Val::Flags(flags) => {
            let wasm_flags = unsafe { std::mem::transmute(flags.clone()) };
            let WasmFlags {
                ty: _,
                count,
                value,
            } = wasm_flags;
            let mut encoded_value = Vec::new();

            for v in value.iter() {
                for n in 0..count {
                    let flag = 1 << n;
                    if flag & *v as i32 != 0 {
                        encoded_value.push(n as i32);
                    }
                }
            }
            Ok(golem::Val {
                val: Some(golem::val::Val::Flags(golem::ValFlags {
                    count: count as i32,
                    value: encoded_value,
                })),
            })
        }
        Val::Resource(_) => Err(GolemError::Unknown {
            details: "resource values are not supported yet".to_string(),
        }),
    }
}

#[allow(unused)]
pub struct WasmVariant {
    ty: wasmtime::component::types::Variant,
    discriminant: u32,
    value: Option<Box<wasmtime::component::Val>>,
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
