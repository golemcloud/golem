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

use crate::{Uri, Value};
use async_recursion::async_recursion;
use async_trait::async_trait;
use golem_wasm_ast::analysis::analysed_type::{
    bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, s16, s32, s64, s8, str,
    tuple, u16, u32, u64, u8, unit_case, variant,
};
use golem_wasm_ast::analysis::{AnalysedType, TypeResult};
use std::fmt;
use std::fmt::{Display, Formatter};
use wasmtime::component::{types, ResourceAny, Type, Val};

#[derive(Debug)]
pub enum EncodingError {
    ParamTypeMismatch { details: String },
    ValueMismatch { details: String },
    Unknown { details: String },
}

impl Display for EncodingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            EncodingError::ParamTypeMismatch { details } => {
                write!(f, "Parameter type mismatch: {details}")
            }
            EncodingError::ValueMismatch { details } => write!(f, "Value mismatch: {details}"),
            EncodingError::Unknown { details } => write!(f, "Unknown error: {details}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "bincode", derive(bincode::Encode, bincode::Decode))]
pub struct ResourceTypeId {
    /// Name of the WIT resource
    pub name: String,
    /// Owner of the resource, either an interface in a WIT package or a name of a world
    pub owner: String,
}

#[async_trait]
pub trait ResourceStore {
    fn self_uri(&self) -> Uri;
    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64;
    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)>;
    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)>;
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
pub async fn decode_param(
    param: &Value,
    param_type: &Type,
    resource_store: &mut (impl ResourceStore + Send),
) -> Result<DecodeParamResult, EncodingError> {
    decode_param_impl(param, param_type, resource_store, "$").await
}

#[async_recursion]
async fn decode_param_impl(
    param: &Value,
    param_type: &Type,
    resource_store: &mut (impl ResourceStore + Send),
    context: &str,
) -> Result<DecodeParamResult, EncodingError> {
    match param_type {
        Type::Bool => match param {
            Value::Bool(bool) => Ok(DecodeParamResult::simple(Val::Bool(*bool))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected bool, got {}", param.type_case_name()),
            }),
        },
        Type::S8 => match param {
            Value::S8(s8) => Ok(DecodeParamResult::simple(Val::S8(*s8))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected s8, got {}", param.type_case_name()),
            }),
        },
        Type::U8 => match param {
            Value::U8(u8) => Ok(DecodeParamResult::simple(Val::U8(*u8))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected u8, got {}", param.type_case_name()),
            }),
        },
        Type::S16 => match param {
            Value::S16(s16) => Ok(DecodeParamResult::simple(Val::S16(*s16))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected s16, got {}", param.type_case_name()),
            }),
        },
        Type::U16 => match param {
            Value::U16(u16) => Ok(DecodeParamResult::simple(Val::U16(*u16))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected u16, got {}", param.type_case_name()),
            }),
        },
        Type::S32 => match param {
            Value::S32(s32) => Ok(DecodeParamResult::simple(Val::S32(*s32))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected s32, got {}", param.type_case_name()),
            }),
        },
        Type::U32 => match param {
            Value::U32(u32) => Ok(DecodeParamResult::simple(Val::U32(*u32))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected u32, got {}", param.type_case_name()),
            }),
        },
        Type::S64 => match param {
            Value::S64(s64) => Ok(DecodeParamResult::simple(Val::S64(*s64))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected s64, got {}", param.type_case_name()),
            }),
        },
        Type::U64 => match param {
            Value::U64(u64) => Ok(DecodeParamResult::simple(Val::U64(*u64))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected u64, got {}", param.type_case_name()),
            }),
        },
        Type::Float32 => match param {
            Value::F32(f32) => Ok(DecodeParamResult::simple(Val::Float32(*f32))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected f32, got {}", param.type_case_name()),
            }),
        },
        Type::Float64 => match param {
            Value::F64(f64) => Ok(DecodeParamResult::simple(Val::Float64(*f64))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected f64, got {}", param.type_case_name()),
            }),
        },
        Type::Char => match param {
            Value::Char(char) => Ok(DecodeParamResult::simple(Val::Char(*char))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected char, got {}", param.type_case_name()),
            }),
        },
        Type::String => match param {
            Value::String(string) => Ok(DecodeParamResult::simple(Val::String(string.clone()))),
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected string, got {}",
                    param.type_case_name()
                ),
            }),
        },
        Type::List(ty) => match param {
            Value::List(values) => {
                let mut decoded_values = Vec::new();
                let mut resource_ids_to_drop = Vec::new();
                for (idx, value) in values.iter().enumerate() {
                    let decoded_param = decode_param_impl(
                        value,
                        &ty.ty(),
                        resource_store,
                        &format!("{context}.[{idx}]"),
                    )
                    .await?;
                    decoded_values.push(decoded_param.val);
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }
                Ok(DecodeParamResult {
                    val: Val::List(decoded_values),
                    resources_to_drop: resource_ids_to_drop,
                })
            }
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected list, got {}", param.type_case_name()),
            }),
        },
        Type::Record(ty) => match param {
            Value::Record(values) => {
                let mut record_values = Vec::new();
                let mut resource_ids_to_drop = Vec::new();

                for (value, field) in values.iter().zip(ty.fields()) {
                    let decoded_param = decode_param_impl(
                        value,
                        &field.ty,
                        resource_store,
                        &format!("{context}.{}", field.name),
                    )
                    .await?;
                    record_values.push((field.name.to_string(), decoded_param.val));
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }

                Ok(DecodeParamResult {
                    val: Val::Record(record_values),
                    resources_to_drop: resource_ids_to_drop,
                })
            }
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected record, got {}",
                    param.type_case_name()
                ),
            }),
        },
        Type::Tuple(ty) => match param {
            Value::Tuple(values) => {
                let mut tuple_values = Vec::new();
                let mut resource_ids_to_drop = Vec::new();

                for (idx, (value, ty)) in values.iter().zip(ty.types()).enumerate() {
                    let decoded_param =
                        decode_param_impl(value, &ty, resource_store, &format!("{context}.{idx}"))
                            .await?;
                    tuple_values.push(decoded_param.val);
                    resource_ids_to_drop.extend(decoded_param.resources_to_drop);
                }

                Ok(DecodeParamResult {
                    val: Val::Tuple(tuple_values),
                    resources_to_drop: resource_ids_to_drop,
                })
            }
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected tuple, got {}",
                    param.type_case_name()
                ),
            }),
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
                        details: format!(
                            "in {context} could not get case for discriminant {case_idx}"
                        ),
                    })?;
                let name = case.name;
                match case.ty {
                    Some(ref case_ty) => {
                        let decoded_value = match case_value {
                            Some(v) => Some(
                                decode_param_impl(
                                    v,
                                    case_ty,
                                    resource_store,
                                    &format!("{context}.{name}"),
                                )
                                .await?,
                            ),
                            None => None,
                        };
                        match decoded_value {
                            Some(decoded_value) => Ok(DecodeParamResult {
                                val: Val::Variant(
                                    name.to_string(),
                                    Some(Box::new(decoded_value.val)),
                                ),
                                resources_to_drop: decoded_value.resources_to_drop,
                            }),
                            None => Ok(DecodeParamResult::simple(Val::Variant(
                                name.to_string(),
                                None,
                            ))),
                        }
                    }
                    None => match case_value {
                        Some(_) => Err(EncodingError::ValueMismatch {
                            details: format!("in {context} expected no value for unit variant"),
                        }),
                        None => Ok(DecodeParamResult::simple(Val::Variant(
                            name.to_string(),
                            None,
                        ))),
                    },
                }
            }
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected variant, got {}",
                    param.type_case_name()
                ),
            }),
        },
        Type::Enum(ty) => match param {
            Value::Enum(discriminant) => {
                let names: Vec<&str> = ty.names().collect();
                let name: &str =
                    names
                        .get(*discriminant as usize)
                        .ok_or(EncodingError::ValueMismatch {
                            details: format!(
                                "in {context} could not get name for discriminant {discriminant}"
                            ),
                        })?;

                Ok(DecodeParamResult::simple(Val::Enum(name.to_string())))
            }
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!("in {context} expected enum, got {}", param.type_case_name()),
            }),
        },
        Type::Option(ty) => match param {
            Value::Option(value) => match value {
                Some(value) => {
                    let decoded_value = decode_param_impl(
                        value,
                        &ty.ty(),
                        resource_store,
                        &format!("{context}.some"),
                    )
                    .await?;
                    Ok(DecodeParamResult {
                        val: Val::Option(Some(Box::new(decoded_value.val))),
                        resources_to_drop: decoded_value.resources_to_drop,
                    })
                }
                None => Ok(DecodeParamResult::simple(Val::Option(None))),
            },
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected option, got {}",
                    param.type_case_name()
                ),
            }),
        },
        Type::Result(ty) => match param {
            Value::Result(result) => match result {
                Ok(value) => {
                    let decoded_value = match value {
                        Some(v) => {
                            let ok_ty = ty.ok().ok_or(EncodingError::ValueMismatch {
                                details: format!("in {context} could not get ok type"),
                            })?;
                            Some(
                                decode_param_impl(
                                    v,
                                    &ok_ty,
                                    resource_store,
                                    &format!("{context}.ok"),
                                )
                                .await?,
                            )
                        }
                        None => None,
                    };
                    match decoded_value {
                        Some(decoded_value) => Ok(DecodeParamResult {
                            val: Val::Result(Ok(Some(Box::new(decoded_value.val)))),
                            resources_to_drop: decoded_value.resources_to_drop,
                        }),
                        None => Ok(DecodeParamResult::simple(Val::Result(Ok(None)))),
                    }
                }
                Err(value) => {
                    let decoded_value = match value {
                        Some(v) => {
                            let err_ty = ty.err().ok_or(EncodingError::ValueMismatch {
                                details: format!("in {context} could not get err type"),
                            })?;
                            Some(
                                decode_param_impl(
                                    v,
                                    &err_ty,
                                    resource_store,
                                    &format!("{context}.err"),
                                )
                                .await?,
                            )
                        }
                        None => None,
                    };

                    match decoded_value {
                        Some(decoded_value) => Ok(DecodeParamResult {
                            val: Val::Result(Err(Some(Box::new(decoded_value.val)))),
                            resources_to_drop: decoded_value.resources_to_drop,
                        }),
                        None => Ok(DecodeParamResult::simple(Val::Result(Err(None)))),
                    }
                }
            },
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected result, got {}",
                    param.type_case_name()
                ),
            }),
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
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected flags, got {}",
                    param.type_case_name()
                ),
            }),
        },
        Type::Own(_) => {
            match param {
                Value::Handle { uri, resource_id } => {
                    let uri = Uri { value: uri.clone() };
                    if resource_store.self_uri() == uri {
                        match resource_store.get(*resource_id).await {
                            Some((_, resource)) => Ok(DecodeParamResult {
                                val: Val::Resource(resource),
                                resources_to_drop: vec![resource],
                            }),
                            None => Err(EncodingError::ValueMismatch {
                                details: format!("in {context} resource not found"),
                            }),
                        }
                    } else {
                        Err(EncodingError::ValueMismatch {
                            details: format!("in {context} cannot resolve handle belonging to a different worker"),
                        })
                    }
                }
                _ => Err(EncodingError::ParamTypeMismatch {
                    details: format!(
                        "in {context} expected handle, got {}",
                        param.type_case_name()
                    ),
                }),
            }
        }
        Type::Borrow(_) => match param {
            Value::Handle { uri, resource_id } => {
                let uri = Uri { value: uri.clone() };
                if resource_store.self_uri() == uri {
                    match resource_store.borrow(*resource_id).await {
                        Some((_, resource)) => {
                            Ok(DecodeParamResult::simple(Val::Resource(resource)))
                        }
                        None => Err(EncodingError::ValueMismatch {
                            details: format!("in {context} resource not found"),
                        }),
                    }
                } else {
                    Err(EncodingError::ValueMismatch {
                        details: format!(
                            "in {context} cannot resolve handle belonging to a different worker"
                        ),
                    })
                }
            }
            _ => Err(EncodingError::ParamTypeMismatch {
                details: format!(
                    "in {context} expected handle, got {}",
                    param.type_case_name()
                ),
            }),
        },
    }
}

/// Converts a wasmtime Val to a wasm-rpc Value
#[async_recursion]
pub async fn encode_output(
    value: &Val,
    typ: &Type,
    analysed_typ: &AnalysedType,
    resource_store: &mut (impl ResourceStore + Send),
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
                let inner_analysed_typ = if let AnalysedType::List(inner) = analysed_typ {
                    Ok(&*inner.inner)
                } else {
                    Err(EncodingError::ValueMismatch {
                        details: "Expected a List type for list value".to_string(),
                    })
                }?;

                for value in (*list).iter() {
                    encoded_values.push(
                        encode_output(value, &list_type.ty(), inner_analysed_typ, resource_store)
                            .await?,
                    );
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
                let mut encoded_values = Vec::new();
                for (idx, ((_name, value), field)) in
                    record.iter().zip(record_type.fields()).enumerate()
                {
                    let field_analysed_type = if let AnalysedType::Record(inner) = analysed_typ {
                        Ok(&inner.fields[idx].typ)
                    } else {
                        Err(EncodingError::ValueMismatch {
                            details: "Expected a Record type for record value".to_string(),
                        })
                    }?;

                    let field =
                        encode_output(value, &field.ty, field_analysed_type, resource_store)
                            .await?;
                    encoded_values.push(field);
                }
                Ok(Value::Record(encoded_values))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Record value for non-record result type".to_string(),
                })
            }
        }
        Val::Tuple(tuple) => {
            if let Type::Tuple(tuple_type) = typ {
                let mut encoded_values = Vec::new();
                for (idx, (v, t)) in tuple.iter().zip(tuple_type.types()).enumerate() {
                    let item_analysed_type = if let AnalysedType::Tuple(inner) = analysed_typ {
                        Ok(&inner.items[idx])
                    } else {
                        Err(EncodingError::ValueMismatch {
                            details: "Expected a Tuple type for tuple value".to_string(),
                        })
                    }?;

                    let value = encode_output(v, &t, item_analysed_type, resource_store).await?;
                    encoded_values.push(value);
                }
                Ok(Value::Tuple(encoded_values))
            } else {
                Err(EncodingError::ValueMismatch {
                    details: "Got a Tuple value for non-tuple result type".to_string(),
                })
            }
        }
        Val::Variant(name, value) => {
            if let Type::Variant(variant_type) = typ {
                let (discriminant, case, analysed_case_type) = variant_type
                    .cases()
                    .enumerate()
                    .find(|(_idx, case)| case.name == *name)
                    .map(|(idx, case)| {
                        if let AnalysedType::Variant(inner) = analysed_typ {
                            Ok((idx, case, &inner.cases[idx].typ))
                        } else {
                            Err(EncodingError::ValueMismatch {
                                details: "Expected a Variant type for variant value".to_string(),
                            })
                        }
                    })
                    .transpose()?
                    .ok_or(EncodingError::ValueMismatch {
                        details: format!("Could not find case for variant {name}"),
                    })?;

                let encoded_output = match value {
                    Some(v) => Some(
                        encode_output(
                            v,
                            &case.ty.ok_or(EncodingError::ValueMismatch {
                                details: "Could not get type information for case".to_string(),
                            })?,
                            analysed_case_type
                                .as_ref()
                                .ok_or(EncodingError::ValueMismatch {
                                    details: "Could not get type information for case".to_string(),
                                })?,
                            resource_store,
                        )
                        .await?,
                    ),
                    None => None,
                };

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
                        details: format!("Could not find discriminant for enum {name}"),
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
                    let analysed_inner_type = if let AnalysedType::Option(inner) = analysed_typ {
                        Ok(&*inner.inner)
                    } else {
                        Err(EncodingError::ValueMismatch {
                            details: "Expected an Option type for option value".to_string(),
                        })
                    }?;

                    let encoded_output = encode_output(
                        value,
                        &option_type.ty(),
                        analysed_inner_type,
                        resource_store,
                    )
                    .await?;
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
                        let encoded_output = match value {
                            Some(v) => {
                                let t = result_type.ok().ok_or(EncodingError::ValueMismatch {
                                    details: "Could not get ok type for result".to_string(),
                                })?;

                                let analysed_ok_type =
                                    if let AnalysedType::Result(inner) = analysed_typ {
                                        Ok(inner.ok.as_ref().ok_or_else(|| {
                                            EncodingError::ValueMismatch {
                                                details: "Expected a Result type for result value"
                                                    .to_string(),
                                            }
                                        })?)
                                    } else {
                                        Err(EncodingError::ValueMismatch {
                                            details: "Expected a Result type for result value"
                                                .to_string(),
                                        })
                                    }?;

                                Some(encode_output(v, &t, analysed_ok_type, resource_store).await?)
                            }
                            None => None,
                        };
                        Ok(Value::Result(Ok(encoded_output.map(Box::new))))
                    }
                    Err(value) => {
                        let encoded_output = match value {
                            Some(v) => {
                                let t = result_type.err().ok_or(EncodingError::ValueMismatch {
                                    details: "Could not get error type for result".to_string(),
                                })?;

                                let analysed_err_type =
                                    if let AnalysedType::Result(inner) = analysed_typ {
                                        Ok(inner.err.as_ref().ok_or_else(|| {
                                            EncodingError::ValueMismatch {
                                                details: "Expected a Result type for result value"
                                                    .to_string(),
                                            }
                                        })?)
                                    } else {
                                        Err(EncodingError::ValueMismatch {
                                            details: "Expected a Result type for result value"
                                                .to_string(),
                                        })
                                    }?;

                                Some(encode_output(v, &t, analysed_err_type, resource_store).await?)
                            }
                            None => None,
                        };
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
            let type_id = analysed_typ
                .name()
                .and_then(|name| {
                    analysed_typ.owner().map(|owner| ResourceTypeId {
                        name: name.to_string(),
                        owner: owner.to_string(),
                    })
                })
                .ok_or_else(|| EncodingError::ValueMismatch {
                    details: "Resource type information is missing for resource value".to_string(),
                })?;

            let id = resource_store.add(*resource, type_id).await;
            Ok(Value::Handle {
                uri: resource_store.self_uri().value,
                resource_id: id,
            })
        }
    }
}

pub fn type_to_analysed_type(typ: &Type) -> Result<AnalysedType, String> {
    match typ {
        Type::Bool => Ok(bool()),
        Type::S8 => Ok(s8()),
        Type::U8 => Ok(u8()),
        Type::S16 => Ok(s16()),
        Type::U16 => Ok(u16()),
        Type::S32 => Ok(s32()),
        Type::U32 => Ok(u32()),
        Type::S64 => Ok(s64()),
        Type::U64 => Ok(u64()),
        Type::Float32 => Ok(f32()),
        Type::Float64 => Ok(f64()),
        Type::Char => Ok(chr()),
        Type::String => Ok(str()),
        Type::List(wlist) => {
            let inner = type_to_analysed_type(&wlist.ty())?;
            Ok(list(inner))
        }
        Type::Record(wrecord) => {
            let fields = wrecord
                .fields()
                .map(|wfield| type_to_analysed_type(&wfield.ty).map(|t| field(wfield.name, t)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(record(fields))
        }
        Type::Tuple(wtuple) => {
            let items = wtuple
                .types()
                .map(|ty| type_to_analysed_type(&ty))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(tuple(items))
        }
        Type::Variant(wvariant) => {
            let cases = wvariant
                .cases()
                .map(|wcase| match wcase.ty {
                    Some(ty) => type_to_analysed_type(&ty).map(|t| case(wcase.name, t)),
                    None => Ok(unit_case(wcase.name)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(variant(cases))
        }
        Type::Enum(wenum) => Ok(r#enum(&wenum.names().collect::<Vec<_>>())),
        Type::Option(woption) => {
            let inner = type_to_analysed_type(&woption.ty())?;
            Ok(option(inner))
        }
        Type::Result(result) => {
            let ok = match result.ok() {
                Some(ty) => Some(Box::new(type_to_analysed_type(&ty)?)),
                None => None,
            };
            let err = match result.err() {
                Some(ty) => Some(Box::new(type_to_analysed_type(&ty)?)),
                None => None,
            };
            Ok(AnalysedType::Result(TypeResult {
                ok,
                err,
                name: None,
                owner: None,
            }))
        }
        Type::Flags(wflags) => Ok(flags(&wflags.names().collect::<Vec<_>>())),
        Type::Own(_) => Err("Cannot extract information about owned resource type".to_string()),
        Type::Borrow(_) => {
            Err("Cannot extract information about borrowed resource type".to_string())
        }
    }
}
