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

use crate::{Uri, Value};
use golem_wasm_ast::analysis::AnalysedType;
use wasmtime::component::{
    types, Enum, Flags, List, OptionVal, Record, ResourceAny, ResultVal, Tuple, Type, Val, Variant,
};

pub enum EncodingError {
    ParamTypeMismatch,
    ValueMismatch { details: String },
    Unknown { details: String },
}

pub trait ResourceStore {
    fn self_uri(&self) -> Uri;
    fn add(&mut self, resource: ResourceAny) -> u64;
    fn get(&mut self, resource_id: u64) -> Option<ResourceAny>;
    fn borrow(&self, resource_id: u64) -> Option<ResourceAny>;
}

pub struct DecodeParamResult {
    pub val: Val,
    pub resources_to_drop: Vec<ResourceAny>,
}

impl DecodeParamResult {
    pub fn simple(val: Val) -> Self {
        Self {
            val,
            resources_to_drop: Vec::new(),
        }
    }
}

/// Converts a Value to a wasmtime Val based on the available type information.
pub fn decode_param(
    param: &Value,
    param_type: &Type,
    resource_store: &mut impl ResourceStore,
) -> Result<DecodeParamResult, EncodingError> {
    match param_type {
        Type::Bool => match param {
            Value::Bool(bool) => Ok(DecodeParamResult::simple(Val::Bool(*bool))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S8 => match param {
            Value::S8(s8) => Ok(DecodeParamResult::simple(Val::S8(*s8))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U8 => match param {
            Value::U8(u8) => Ok(DecodeParamResult::simple(Val::U8(*u8))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S16 => match param {
            Value::S16(s16) => Ok(DecodeParamResult::simple(Val::S16(*s16))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U16 => match param {
            Value::U16(u16) => Ok(DecodeParamResult::simple(Val::U16(*u16))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S32 => match param {
            Value::S32(s32) => Ok(DecodeParamResult::simple(Val::S32(*s32))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U32 => match param {
            Value::U32(u32) => Ok(DecodeParamResult::simple(Val::U32(*u32))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::S64 => match param {
            Value::S64(s64) => Ok(DecodeParamResult::simple(Val::S64(*s64))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::U64 => match param {
            Value::U64(u64) => Ok(DecodeParamResult::simple(Val::U64(*u64))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Float32 => match param {
            Value::F32(f32) => Ok(DecodeParamResult::simple(Val::Float32(*f32))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Float64 => match param {
            Value::F64(f64) => Ok(DecodeParamResult::simple(Val::Float64(*f64))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Char => match param {
            Value::Char(char) => Ok(DecodeParamResult::simple(Val::Char(*char))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::String => match param {
            Value::String(string) => Ok(DecodeParamResult::simple(Val::String(
                string.clone().into_boxed_str(),
            ))),
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::List(ty) => match param {
            Value::List(values) => {
                let mut decoded_values = Vec::new();
                let mut resource_ids_to_drop = Vec::new();
                for value in values {
                    let decoded_param = decode_param(value, &ty.ty(), resource_store)?;
                    decoded_values.push(decoded_param.val);
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }
                let list = List::new(ty, decoded_values.into_boxed_slice())
                    .expect("Type mismatch in decode_param");
                Ok(DecodeParamResult {
                    val: Val::List(list),
                    resources_to_drop: resource_ids_to_drop,
                })
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Record(ty) => match param {
            Value::Record(values) => {
                let mut record_values = Vec::new();
                let mut resource_ids_to_drop = Vec::new();

                for (value, field) in values.iter().zip(ty.fields()) {
                    let decoded_param = decode_param(value, &field.ty, resource_store)?;
                    record_values.push((field.name, decoded_param.val));
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }

                let record = Record::new(ty, record_values).expect("Type mismatch in decode_param");
                Ok(DecodeParamResult {
                    val: Val::Record(record),
                    resources_to_drop: resource_ids_to_drop,
                })
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Tuple(ty) => match param {
            Value::Tuple(values) => {
                let mut tuple_values = Vec::new();
                let mut resource_ids_to_drop = Vec::new();

                for (value, ty) in values.iter().zip(ty.types()) {
                    let decoded_param = decode_param(value, &ty, resource_store)?;
                    tuple_values.push(decoded_param.val);
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }

                let tuple = Tuple::new(ty, tuple_values.into_boxed_slice())
                    .expect("Type mismatch in decode_param");
                Ok(DecodeParamResult {
                    val: Val::Tuple(tuple),
                    resources_to_drop: resource_ids_to_drop,
                })
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
                let decoded_value = case_value
                    .as_ref()
                    .map(|v| decode_param(v, case_ty, resource_store))
                    .transpose()?;
                match decoded_value {
                    Some(decoded_value) => {
                        let variant = Variant::new(ty, name, Some(decoded_value.val))
                            .expect("Type mismatch in decode_param");
                        Ok(DecodeParamResult {
                            val: Val::Variant(variant),
                            resources_to_drop: decoded_value.resources_to_drop,
                        })
                    }
                    None => {
                        let variant =
                            Variant::new(ty, name, None).expect("Type mismatch in decode_param");
                        Ok(DecodeParamResult::simple(Val::Variant(variant)))
                    }
                }
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
                Ok(DecodeParamResult::simple(Val::Enum(enum0)))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Option(ty) => match param {
            Value::Option(value) => match value {
                Some(value) => {
                    let decoded_value = decode_param(value, &ty.ty(), resource_store)?;
                    let option = OptionVal::new(ty, Some(decoded_value.val))
                        .expect("Type mismatch in decode_param");
                    Ok(DecodeParamResult {
                        val: Val::Option(option),
                        resources_to_drop: decoded_value.resources_to_drop,
                    })
                }
                None => {
                    let option = OptionVal::new(ty, None).expect("Type mismatch in decode_param");
                    Ok(DecodeParamResult::simple(Val::Option(option)))
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
                    let decoded_value = value
                        .as_ref()
                        .map(|v| decode_param(v, &ok_ty, resource_store))
                        .transpose()?;
                    match decoded_value {
                        Some(decoded_value) => {
                            let result = ResultVal::new(ty, Ok(Some(decoded_value.val)))
                                .expect("Type mismatch in decode_param");
                            Ok(DecodeParamResult {
                                val: Val::Result(result),
                                resources_to_drop: decoded_value.resources_to_drop,
                            })
                        }
                        None => {
                            let result = ResultVal::new(ty, Ok(None))
                                .expect("Type mismatch in decode_param");
                            Ok(DecodeParamResult::simple(Val::Result(result)))
                        }
                    }
                }
                Err(value) => {
                    let err_ty = ty.err().ok_or(EncodingError::ValueMismatch {
                        details: "could not get err type".to_string(),
                    })?;
                    let decoded_value = value
                        .as_ref()
                        .map(|v| decode_param(v, &err_ty, resource_store))
                        .transpose()?;
                    match decoded_value {
                        Some(decoded_value) => {
                            let result = ResultVal::new(ty, Err(Some(decoded_value.val)))
                                .expect("Type mismatch in decode_param");
                            Ok(DecodeParamResult {
                                val: Val::Result(result),
                                resources_to_drop: decoded_value.resources_to_drop,
                            })
                        }
                        None => {
                            let result = ResultVal::new(ty, Err(None))
                                .expect("Type mismatch in decode_param");
                            Ok(DecodeParamResult::simple(Val::Result(result)))
                        }
                    }
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
                Ok(DecodeParamResult::simple(Val::Flags(flags)))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Own(_) => match param {
            Value::Handle { uri, resource_id } => {
                if resource_store.self_uri() == *uri {
                    match resource_store.get(*resource_id) {
                        Some(resource) => Ok(DecodeParamResult {
                            val: Val::Resource(resource),
                            resources_to_drop: vec![resource],
                        }),
                        None => Err(EncodingError::ValueMismatch {
                            details: "resource not found".to_string(),
                        }),
                    }
                } else {
                    Err(EncodingError::ValueMismatch {
                        details: "cannot resolve handle belonging to a different worker"
                            .to_string(),
                    })
                }
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Borrow(_) => match param {
            Value::Handle { uri, resource_id } => {
                if resource_store.self_uri() == *uri {
                    match resource_store.borrow(*resource_id) {
                        Some(resource) => Ok(DecodeParamResult::simple(Val::Resource(resource))),
                        None => Err(EncodingError::ValueMismatch {
                            details: "resource not found".to_string(),
                        }),
                    }
                } else {
                    Err(EncodingError::ValueMismatch {
                        details: "cannot resolve handle belonging to a different worker"
                            .to_string(),
                    })
                }
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
    }
}

/// Converts a wasmtime Val to a Golem protobuf Val
pub fn encode_output(
    value: &Val,
    resource_store: &mut impl ResourceStore,
) -> Result<Value, EncodingError> {
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
                encoded_values.push(encode_output(value, resource_store)?);
            }
            Ok(Value::List(encoded_values))
        }
        Val::Record(record) => {
            let encoded_values = record
                .fields()
                .map(|(_, value)| encode_output(value, resource_store))
                .collect::<Result<Vec<Value>, EncodingError>>()?;
            Ok(Value::Record(encoded_values))
        }
        Val::Tuple(tuple) => {
            let encoded_values = tuple
                .values()
                .iter()
                .map(|v| encode_output(v, resource_store))
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
            let encoded_output = value
                .map(|v| encode_output(&v, resource_store))
                .transpose()?;
            Ok(Value::Variant {
                case_idx: discriminant,
                case_value: encoded_output.map(Box::new),
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
                let encoded_output = encode_output(value, resource_store)?;
                Ok(Value::Option(Some(Box::new(encoded_output))))
            }
            None => Ok(Value::Option(None)),
        },
        Val::Result(result) => match result.value() {
            Ok(value) => {
                let encoded_output = value
                    .map(|v| encode_output(v, resource_store))
                    .transpose()?;
                Ok(Value::Result(Ok(encoded_output.map(Box::new))))
            }
            Err(value) => {
                let encoded_output = value
                    .map(|v| encode_output(v, resource_store))
                    .transpose()?;
                Ok(Value::Result(Err(encoded_output.map(Box::new))))
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
        Val::Resource(resource) => {
            let id = resource_store.add(*resource);
            Ok(Value::Handle {
                uri: resource_store.self_uri(),
                resource_id: id,
            })
        }
    }
}

#[allow(unused)]
pub struct WasmVariant {
    ty: types::Variant,
    discriminant: u32,
    value: Option<Box<Val>>,
}

#[allow(unused)]
pub struct WasmEnum {
    ty: types::Variant,
    discriminant: u32,
}

#[allow(unused)]
pub struct WasmFlags {
    ty: types::Flags,
    count: u32,
    value: Box<[u32]>,
}

pub fn type_to_analysed_type(typ: &Type) -> Result<AnalysedType, String> {
    match typ {
        Type::Bool => Ok(AnalysedType::Bool),
        Type::S8 => Ok(AnalysedType::S8),
        Type::U8 => Ok(AnalysedType::U8),
        Type::S16 => Ok(AnalysedType::S16),
        Type::U16 => Ok(AnalysedType::U16),
        Type::S32 => Ok(AnalysedType::S32),
        Type::U32 => Ok(AnalysedType::U32),
        Type::S64 => Ok(AnalysedType::S64),
        Type::U64 => Ok(AnalysedType::U64),
        Type::Float32 => Ok(AnalysedType::F32),
        Type::Float64 => Ok(AnalysedType::F64),
        Type::Char => Ok(AnalysedType::Chr),
        Type::String => Ok(AnalysedType::Str),
        Type::List(list) => {
            let inner = type_to_analysed_type(&list.ty())?;
            Ok(AnalysedType::List(Box::new(inner)))
        }
        Type::Record(record) => {
            let fields = record
                .fields()
                .map(|field| type_to_analysed_type(&field.ty).map(|t| (field.name.to_string(), t)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AnalysedType::Record(fields))
        }
        Type::Tuple(tuple) => {
            let types = tuple
                .types()
                .map(|ty| type_to_analysed_type(&ty))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AnalysedType::Tuple(types))
        }
        Type::Variant(variant) => {
            let cases = variant
                .cases()
                .map(|case| match case.ty {
                    Some(ty) => {
                        type_to_analysed_type(&ty).map(|t| (case.name.to_string(), Some(t)))
                    }
                    None => Ok((case.name.to_string(), None)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AnalysedType::Variant(cases))
        }
        Type::Enum(enm) => {
            let names = enm.names().map(|name| name.to_string()).collect();
            Ok(AnalysedType::Enum(names))
        }
        Type::Option(option) => {
            let inner = type_to_analysed_type(&option.ty())?;
            Ok(AnalysedType::Option(Box::new(inner)))
        }
        Type::Result(result) => {
            let ok = match result.ok() {
                Some(ty) => Some(Box::new(type_to_analysed_type(&ty)?)),
                None => None,
            };
            let error = match result.err() {
                Some(ty) => Some(Box::new(type_to_analysed_type(&ty)?)),
                None => None,
            };
            Ok(AnalysedType::Result { ok, error })
        }
        Type::Flags(flags) => {
            let names = flags.names().map(|name| name.to_string()).collect();
            Ok(AnalysedType::Flags(names))
        }
        Type::Own(_) => Err("Cannot extract information about owned resource type".to_string()),
        Type::Borrow(_) => {
            Err("Cannot extract information about borrowed resource type".to_string())
        }
    }
}
