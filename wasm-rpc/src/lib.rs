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

#[allow(unused)]
#[rustfmt::skip]
#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
mod bindings;

/// Implements bincode encoders and decoders for WitValue instances
#[cfg(feature = "bincode")]
pub mod bincode;
/// A builder interface for WitValue instances
mod builder;

/// Extension methods for extracting values from WitValue instances
mod extractor;

/// Conversion to and from JSON, in the presence of golem-wasm-ast generated type information
#[cfg(feature = "json")]
pub mod json;

/// Protobuf-defined value types and conversion to them
#[cfg(feature = "protobuf")]
pub mod protobuf;

/// Serde instances for WitValue
#[cfg(feature = "serde")]
pub mod serde;

#[cfg(feature = "text")]
mod text;

#[cfg(feature = "wasmtime")]
pub mod wasmtime;

use crate::builder::WitValueBuilder;
pub use builder::{NodeBuilder, WitValueBuilderExtensions};
pub use extractor::{WitNodePointer, WitValueExtractor};
use std::ops::Deref;

#[cfg(feature = "host")]
use ::wasmtime::component::bindgen;

#[cfg(feature = "typeinfo")]
use golem_wasm_ast::analysis::{
    AnalysedFunctionResult, AnalysedResourceId, AnalysedResourceMode, AnalysedType,
};

#[cfg(feature = "host")]
bindgen!({
    path: "wit",
    interfaces: "
      import golem:rpc/types@0.1.0;
    ",
    tracing: false,
    async: true,
    with: {
        "golem:rpc/types/wasm-rpc": WasmRpcEntry
    }
});

#[cfg(feature = "host")]
pub use golem::rpc::types::{Host, HostWasmRpc, NodeIndex, RpcError, Uri, WitNode, WitValue};

#[cfg(feature = "host")]
pub struct WasmRpcEntry {
    pub payload: Box<dyn std::any::Any + Send + Sync>,
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for Uri {
    fn arbitrary(u: &mut arbitrary::Unstructured) -> arbitrary::Result<Self> {
        let uri = u.arbitrary::<String>()?;
        Ok(Uri { value: uri })
    }
}

impl PartialEq for Uri {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// A tree representation of Value - isomorphic to the protobuf Val type but easier to work with in Rust
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Record(Vec<Value>),
    Variant {
        case_idx: u32,
        case_value: Option<Box<Value>>,
    },
    Enum(u32),
    Flags(Vec<bool>),
    Option(Option<Box<Value>>),
    Result(Result<Option<Box<Value>>, Option<Box<Value>>>),
    Handle {
        uri: Uri,
        resource_id: u64,
    },
}

impl From<Value> for WitValue {
    fn from(value: Value) -> Self {
        let mut builder = WitValueBuilder::new();
        build_wit_value(value, &mut builder);
        builder.build()
    }
}

fn build_wit_value(value: Value, builder: &mut WitValueBuilder) -> NodeIndex {
    match value {
        Value::Bool(value) => builder.add_bool(value),
        Value::U8(value) => builder.add_u8(value),
        Value::U16(value) => builder.add_u16(value),
        Value::U32(value) => builder.add_u32(value),
        Value::U64(value) => builder.add_u64(value),
        Value::S8(value) => builder.add_s8(value),
        Value::S16(value) => builder.add_s16(value),
        Value::S32(value) => builder.add_s32(value),
        Value::S64(value) => builder.add_s64(value),
        Value::F32(value) => builder.add_f32(value),
        Value::F64(value) => builder.add_f64(value),
        Value::Char(value) => builder.add_char(value),
        Value::String(value) => builder.add_string(&value),
        Value::List(values) => {
            let list_idx = builder.add_list();
            let mut items = Vec::new();
            for value in values {
                let item_idx = build_wit_value(value, builder);
                items.push(item_idx);
            }
            builder.finish_seq(items, list_idx);
            list_idx
        }
        Value::Tuple(values) => {
            let tuple_idx = builder.add_tuple();
            let mut items = Vec::new();
            for value in values {
                let item_idx = build_wit_value(value, builder);
                items.push(item_idx);
            }
            builder.finish_seq(items, tuple_idx);
            tuple_idx
        }
        Value::Record(fields) => {
            let record_idx = builder.add_record();
            let mut items = Vec::new();
            for value in fields {
                let item_idx = build_wit_value(value, builder);
                items.push(item_idx);
            }
            builder.finish_seq(items, record_idx);
            record_idx
        }
        Value::Variant {
            case_idx,
            case_value: Some(case_value),
        } => {
            let variant_idx = builder.add_variant(case_idx, -1);
            let inner_idx = build_wit_value(*case_value, builder);
            builder.finish_child(inner_idx, variant_idx);
            variant_idx
        }
        Value::Variant {
            case_idx,
            case_value: None,
        } => builder.add_variant_unit(case_idx),
        Value::Enum(value) => builder.add_enum_value(value),
        Value::Flags(values) => builder.add_flags(values),
        Value::Option(value) => {
            if let Some(value) = value {
                let option_idx = builder.add_option_some();
                let inner_idx = build_wit_value(*value, builder);
                builder.finish_child(inner_idx, option_idx);
                option_idx
            } else {
                builder.add_option_none()
            }
        }
        Value::Result(result) => match result {
            Ok(Some(ok)) => {
                let result_idx = builder.add_result_ok();
                let inner_idx = build_wit_value(*ok, builder);
                builder.finish_child(inner_idx, result_idx);
                result_idx
            }
            Ok(None) => builder.add_result_ok_unit(),
            Err(Some(err)) => {
                let result_idx = builder.add_result_err();
                let inner_idx = build_wit_value(*err, builder);
                builder.finish_child(inner_idx, result_idx);
                result_idx
            }
            Err(None) => builder.add_result_err_unit(),
        },
        Value::Handle { uri, resource_id } => builder.add_handle(uri, resource_id),
    }
}

impl From<TypeAnnotatedValue> for Value {
    fn from(value: TypeAnnotatedValue) -> Self {
        match value {
            TypeAnnotatedValue::Bool(value) => Value::Bool(value),
            TypeAnnotatedValue::S8(value) => Value::S8(value),
            TypeAnnotatedValue::U8(value) => Value::U8(value),
            TypeAnnotatedValue::S16(value) => Value::S16(value),
            TypeAnnotatedValue::U16(value) => Value::U16(value),
            TypeAnnotatedValue::S32(value) => Value::S32(value),
            TypeAnnotatedValue::U32(value) => Value::U32(value),
            TypeAnnotatedValue::S64(value) => Value::S64(value),
            TypeAnnotatedValue::U64(value) => Value::U64(value),
            TypeAnnotatedValue::F32(value) => Value::F32(value),
            TypeAnnotatedValue::F64(value) => Value::F64(value),
            TypeAnnotatedValue::Chr(value) => Value::Char(value),
            TypeAnnotatedValue::Str(value) => Value::String(value),
            TypeAnnotatedValue::List { typ: _, values } => {
                Value::List(values.into_iter().map(|value| value.into()).collect())
            }
            TypeAnnotatedValue::Tuple { typ: _, value } => {
                Value::Tuple(value.into_iter().map(|value| value.into()).collect())
            }
            TypeAnnotatedValue::Record { typ: _, value } => Value::Record(
                value
                    .into_iter()
                    .map(|(_, value)| value.into())
                    .collect::<Vec<Value>>(),
            ),
            TypeAnnotatedValue::Flags { typ, values } => {
                let mut bools = Vec::new();

                for expected_flag in typ {
                    if values.contains(&expected_flag) {
                        bools.push(true);
                    } else {
                        bools.push(false);
                    }
                }
                Value::Flags(bools)
            }
            TypeAnnotatedValue::Enum { typ, value } => {
                for (index, expected_enum) in typ.iter().enumerate() {
                    if expected_enum.clone() == value {
                        return Value::Enum(index as u32);
                    }
                }

                panic!("Enum value not found in the list of expected enums")
            }
            TypeAnnotatedValue::Option { typ: _, value } => Value::Option(value.map(|value| Box::new(value.deref().clone().into()))),
            TypeAnnotatedValue::Result { ok:_, error:_, value } => Value::Result(match value {
                Ok(value) => Ok(value.map(|value| Box::new(value.deref().clone().into()))),
                Err(value) => Err(value.map(|value| Box::new(value.deref().clone().into()))),
            }),
            TypeAnnotatedValue::Handle {
                id: _,
                resource_mode: _,
                uri,
                resource_id,
            } => Value::Handle { uri, resource_id },
            TypeAnnotatedValue::Variant {
                typ,
                case_name,
                case_value,
            } => {
                let case_value = case_value.map(|value| Box::new(value.deref().clone().into()));
                Value::Variant {
                    case_idx: typ.iter().position(|(name, _)| name == &case_name).unwrap() as u32,
                    case_value,
                }
            }
        }
    }
}

impl From<WitValue> for Value {
    fn from(value: WitValue) -> Self {
        assert!(!value.nodes.is_empty());
        build_tree(&value.nodes[0], &value.nodes)
    }
}

fn build_tree(node: &WitNode, nodes: &[WitNode]) -> Value {
    match node {
        WitNode::RecordValue(field_indices) => {
            let mut fields = Vec::new();
            for index in field_indices {
                let value = build_tree(&nodes[*index as usize], nodes);
                fields.push(value);
            }
            Value::Record(fields)
        }
        WitNode::VariantValue((case_idx, Some(inner_idx))) => {
            let value = build_tree(&nodes[*inner_idx as usize], nodes);
            Value::Variant {
                case_idx: *case_idx,
                case_value: Some(Box::new(value)),
            }
        }
        WitNode::VariantValue((case_idx, None)) => Value::Variant {
            case_idx: *case_idx,
            case_value: None,
        },
        WitNode::EnumValue(value) => Value::Enum(*value),
        WitNode::FlagsValue(values) => Value::Flags(values.clone()),
        WitNode::TupleValue(indices) => {
            let mut values = Vec::new();
            for index in indices {
                let value = build_tree(&nodes[*index as usize], nodes);
                values.push(value);
            }
            Value::Tuple(values)
        }
        WitNode::ListValue(indices) => {
            let mut values = Vec::new();
            for index in indices {
                let value = build_tree(&nodes[*index as usize], nodes);
                values.push(value);
            }
            Value::List(values)
        }
        WitNode::OptionValue(Some(index)) => {
            let value = build_tree(&nodes[*index as usize], nodes);
            Value::Option(Some(Box::new(value)))
        }
        WitNode::OptionValue(None) => Value::Option(None),
        WitNode::ResultValue(Ok(Some(index))) => {
            let value = build_tree(&nodes[*index as usize], nodes);
            Value::Result(Ok(Some(Box::new(value))))
        }
        WitNode::ResultValue(Ok(None)) => Value::Result(Ok(None)),
        WitNode::ResultValue(Err(Some(index))) => {
            let value = build_tree(&nodes[*index as usize], nodes);
            Value::Result(Err(Some(Box::new(value))))
        }
        WitNode::ResultValue(Err(None)) => Value::Result(Err(None)),
        WitNode::PrimU8(value) => Value::U8(*value),
        WitNode::PrimU16(value) => Value::U16(*value),
        WitNode::PrimU32(value) => Value::U32(*value),
        WitNode::PrimU64(value) => Value::U64(*value),
        WitNode::PrimS8(value) => Value::S8(*value),
        WitNode::PrimS16(value) => Value::S16(*value),
        WitNode::PrimS32(value) => Value::S32(*value),
        WitNode::PrimS64(value) => Value::S64(*value),
        WitNode::PrimFloat32(value) => Value::F32(*value),
        WitNode::PrimFloat64(value) => Value::F64(*value),
        WitNode::PrimChar(value) => Value::Char(*value),
        WitNode::PrimBool(value) => Value::Bool(*value),
        WitNode::PrimString(value) => Value::String(value.clone()),
        WitNode::Handle((uri, value)) => Value::Handle {
            uri: uri.clone(),
            resource_id: *value,
        },
    }
}

#[derive(Clone, Debug)]
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
    pub fn from_value(
        val: Value,
        analysed_type: &AnalysedType,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        match val {
            Value::Bool(bool) => Ok(TypeAnnotatedValue::Bool(bool)),
            Value::S8(value) => Ok(TypeAnnotatedValue::S8(value)),
            Value::U8(value) => Ok(TypeAnnotatedValue::U8(value)),
            Value::U32(value) => Ok(TypeAnnotatedValue::U32(value)),
            Value::S16(value) => Ok(TypeAnnotatedValue::S16(value)),
            Value::U16(value) => Ok(TypeAnnotatedValue::U16(value)),
            Value::S32(value) => Ok(TypeAnnotatedValue::S32(value)),
            Value::S64(value) => Ok(TypeAnnotatedValue::S64(value)),
            Value::U64(value) => Ok(TypeAnnotatedValue::U64(value)),
            Value::F32(value) => Ok(TypeAnnotatedValue::F32(value)),
            Value::F64(value) => Ok(TypeAnnotatedValue::F64(value)),
            Value::Char(value) => Ok(TypeAnnotatedValue::Chr(value)),
            Value::String(value) => Ok(TypeAnnotatedValue::Str(value)),

            Value::Enum(value) => match analysed_type {
                AnalysedType::Enum(names) => match names.get(value as usize) {
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
                        Some(value) => Some(Box::new(Self::from_value(*value, elem)?)),
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

                    for (value, tpe) in values.into_iter().zip(types.iter()) {
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

                    for (value, (field_name, typ)) in values.into_iter().zip(fields) {
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
                    if (case_idx as usize) < cases.len() {
                        let (case_name, case_type) = match cases.get(case_idx as usize) {
                            Some(tpe) => Ok(tpe),
                            None => {
                                Err(vec!["Variant not found in the expected types.".to_string()])
                            }
                        }?;

                        match case_type {
                            Some(tpe) => match case_value {
                                Some(case_value) => {
                                    let result = Self::from_value(*case_value, tpe)?;
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
                            let result = Self::from_value(*value, ok_type)?;

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
                            let result = Self::from_value(*value, err_type)?;

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
                    uri,
                    resource_id,
                }),
                _ => Err(vec!["Unexpected type; expected a Handle type.".to_string()]),
            },
        }
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

impl From<TypeAnnotatedValue> for WitValue {
    fn from(value: TypeAnnotatedValue) -> Self {
        let value: Value = value.into();
        value.into()
    }
}
pub enum TypeAnnotatedValueResult {
    WithoutNames(Vec<TypeAnnotatedValue>),
    WithNames(Vec<(String, TypeAnnotatedValue)>),
}

impl TypeAnnotatedValueResult {
    pub fn from_values(
        values: Vec<Value>,
        expected_types: &[AnalysedFunctionResult],
    ) -> Result<TypeAnnotatedValueResult, Vec<String>> {
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
                let result = TypeAnnotatedValue::from_value(value, &expected.typ);

                match result {
                    Ok(value) => results.push(value),
                    Err(err) => errors.extend(err),
                }
            }

            let all_without_names = expected_types.iter().all(|t| t.name.is_none());

            if all_without_names {
                Ok(TypeAnnotatedValueResult::WithoutNames(results))
            } else {
                let mapped_values = results
                    .iter()
                    .zip(expected_types.iter())
                    .enumerate()
                    .map(|(idx, (type_annotated_value, result_def))| {
                        (
                            if let Some(name) = &result_def.name {
                                name.clone()
                            } else {
                                idx.to_string()
                            },
                            type_annotated_value.clone(),
                        )
                    })
                    .collect::<Vec<(String, TypeAnnotatedValue)>>();

                Ok(TypeAnnotatedValueResult::WithNames(mapped_values))
            }
        }
    }
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for WitValue {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let arbitrary_value = u.arbitrary::<Value>()?;
        Ok(arbitrary_value.into())
    }
}

#[cfg(feature = "host")]
pub const WASM_RPC_WIT: &str = include_str!("../wit/wasm-rpc.wit");

pub const WASM_RPC_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use crate::{Value, WitValue};
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb_sized;

    const CASES: u32 = 10000;
    const SIZE: usize = 4096;

    proptest! {

        #![proptest_config(ProptestConfig {
            cases: CASES, .. ProptestConfig::default()
        })]
        #[test]
        fn round_trip(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let wit_value: WitValue = value.clone().into();
            let round_trip_value: Value = wit_value.into();
            prop_assert_eq!(value, round_trip_value);
        }
    }
}
