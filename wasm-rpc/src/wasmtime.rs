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
use wasmtime::component::{types, ResourceAny, Type, Val};

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
            Value::String(string) => Ok(DecodeParamResult::simple(Val::String(string.clone()))),
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
                Ok(DecodeParamResult {
                    val: Val::List(decoded_values),
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
                    record_values.push((field.name.to_string(), decoded_param.val));
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }

                Ok(DecodeParamResult {
                    val: Val::Record(record_values),
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

                Ok(DecodeParamResult {
                    val: Val::Tuple(tuple_values),
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
                    Some(decoded_value) => Ok(DecodeParamResult {
                        val: Val::Variant(name.to_string(), Some(Box::new(decoded_value.val))),
                        resources_to_drop: decoded_value.resources_to_drop,
                    }),
                    None => Ok(DecodeParamResult::simple(Val::Variant(
                        name.to_string(),
                        None,
                    ))),
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

                Ok(DecodeParamResult::simple(Val::Enum(name.to_string())))
            }
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Option(ty) => match param {
            Value::Option(value) => match value {
                Some(value) => {
                    let decoded_value = decode_param(value, &ty.ty(), resource_store)?;
                    Ok(DecodeParamResult {
                        val: Val::Option(Some(Box::new(decoded_value.val))),
                        resources_to_drop: decoded_value.resources_to_drop,
                    })
                }
                None => Ok(DecodeParamResult::simple(Val::Option(None))),
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
                        Some(decoded_value) => Ok(DecodeParamResult {
                            val: Val::Result(Ok(Some(Box::new(decoded_value.val)))),
                            resources_to_drop: decoded_value.resources_to_drop,
                        }),
                        None => Ok(DecodeParamResult::simple(Val::Result(Ok(None)))),
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
                        Some(decoded_value) => Ok(DecodeParamResult {
                            val: Val::Result(Err(Some(Box::new(decoded_value.val)))),
                            resources_to_drop: decoded_value.resources_to_drop,
                        }),
                        None => Ok(DecodeParamResult::simple(Val::Result(Err(None)))),
                    }
                }
            },
            _ => Err(EncodingError::ParamTypeMismatch),
        },
        Type::Flags(ty) => match param {
            Value::Flags(flags) => {
                let flag_names = ty.names().collect::<Vec<&str>>();
                let active_flags: Vec<String> = flag_names
                    .iter()
                    .zip(flags)
                    .filter_map(|(name, enabled)| {
                        if *enabled {
                            Some(name.to_string())
                        } else {
                            None
                        }
                    })
                    .collect();

                Ok(DecodeParamResult::simple(Val::Flags(active_flags)))
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
    typ: &Type,
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
            if let Type::List(list_type) = typ {
                let mut encoded_values = Vec::new();
                for value in (*list).iter() {
                    encoded_values.push(encode_output(value, &list_type.ty(), resource_store)?);
                }
                Ok(Value::List(encoded_values))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a List value for non-list result type".to_string(),
                })
            }
        }
        Val::Record(record) => {
            if let Type::Record(record_type) = typ {
                let encoded_values = record
                    .iter()
                    .zip(record_type.fields())
                    .map(|((_name, value), field)| encode_output(value, &field.ty, resource_store))
                    .collect::<Result<Vec<Value>, EncodingError>>()?;
                Ok(Value::Record(encoded_values))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Record value for non-record result type".to_string(),
                })
            }
        }
        Val::Tuple(tuple) => {
            if let Type::Tuple(tuple_type) = typ {
                let encoded_values = tuple
                    .iter()
                    .zip(tuple_type.types())
                    .map(|(v, t)| encode_output(v, &t, resource_store))
                    .collect::<Result<Vec<Value>, EncodingError>>()?;
                Ok(Value::Tuple(encoded_values))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Tuple value for non-tuple result type".to_string(),
                })
            }
        }
        Val::Variant(name, value) => {
            if let Type::Variant(variant_type) = typ {
                let (discriminant, case) = variant_type
                    .cases()
                    .enumerate()
                    .find(|(_idx, case)| case.name == *name)
                    .ok_or(EncodingError::ValueMismatch {
                        details: format!("Could not find case for variant {}", name),
                    })?;

                let encoded_output = value
                    .as_ref()
                    .map(|v| {
                        encode_output(
                            v,
                            &case.ty.ok_or(EncodingError::ValueMismatch {
                                details: "Could not get type information for case".to_string(),
                            })?,
                            resource_store,
                        )
                    })
                    .transpose()?;
                Ok(Value::Variant {
                    case_idx: discriminant as u32,
                    case_value: encoded_output.map(Box::new),
                })
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Variant value for non-variant result type".to_string(),
                })
            }
        }
        Val::Enum(name) => {
            if let Type::Enum(enum_type) = typ {
                let (discriminant, _name) = enum_type
                    .names()
                    .enumerate()
                    .find(|(_idx, n)| n == name)
                    .ok_or(EncodingError::ValueMismatch {
                        details: format!("Could not find discriminant for enum {}", name),
                    })?;
                Ok(Value::Enum(discriminant as u32))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got an Enum value for non-enum result type".to_string(),
                })
            }
        }
        Val::Option(option) => match option {
            Some(value) => {
                if let Type::Option(option_type) = typ {
                    let encoded_output = encode_output(value, &option_type.ty(), resource_store)?;
                    Ok(Value::Option(Some(Box::new(encoded_output))))
                } else {
                    Err(EncodingError::ValueMismatch {
                        details: "Got an Option value for non-option result type".to_string(),
                    })
                }
            }
            None => Ok(Value::Option(None)),
        },
        Val::Result(result) => {
            if let Type::Result(result_type) = typ {
                match result {
                    Ok(value) => {
                        let encoded_output = value
                            .as_ref()
                            .map(|v| {
                                result_type
                                    .ok()
                                    .ok_or(EncodingError::ValueMismatch {
                                        details: "Could not get ok type for result".to_string(),
                                    })
                                    .and_then(|t| encode_output(v, &t, resource_store))
                            })
                            .transpose()?;
                        Ok(Value::Result(Ok(encoded_output.map(Box::new))))
                    }
                    Err(value) => {
                        let encoded_output = value
                            .as_ref()
                            .map(|v| {
                                result_type
                                    .err()
                                    .ok_or(EncodingError::ValueMismatch {
                                        details: "Could not get error type for result".to_string(),
                                    })
                                    .and_then(|t| encode_output(v, &t, resource_store))
                            })
                            .transpose()?;
                        Ok(Value::Result(Err(encoded_output.map(Box::new))))
                    }
                }
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Result value for non-result result type".to_string(),
                })
            }
        }
        Val::Flags(flags) => {
            if let Type::Flags(flags_type) = typ {
                let mut encoded_value = vec![false; flags_type.names().count()];

                for (idx, name) in flags_type.names().enumerate() {
                    if flags.contains(&name.to_string()) {
                        encoded_value[idx] = true;
                    }
                }

                Ok(Value::Flags(encoded_value))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Flags value for non-flags result type".to_string(),
                })
            }
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
