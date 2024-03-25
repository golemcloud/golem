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

use std::collections::HashSet;
use crate::builder::WitValueBuilder;
pub use builder::{NodeBuilder, WitValueBuilderExtensions};
pub use extractor::{WitNodePointer, WitValueExtractor};
use std::ops::Deref;

#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
pub use bindings::golem::rpc::types::{NodeIndex, RpcError, Uri, WasmRpc, WitNode, WitValue};

#[cfg(feature = "host")]
use ::wasmtime::component::bindgen;
use golem_wasm_ast::analysis::{AnalysedResourceId, AnalysedResourceMode};
use wasm_wave::wasm::{WasmType, WasmValue, WasmValueError};

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

use crate::text::{AnalysedType, TypedValue};
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
            TypeAnnotatedValue::List(list_value) => Value::List(
                list_value
                    .values
                    .into_iter()
                    .map(|value| value.into())
                    .collect(),
            ),
            TypeAnnotatedValue::Tuple(tuple_value) => Value::Tuple(
                tuple_value
                    .value
                    .into_iter()
                    .map(|value| value.into())
                    .collect(),
            ),
            TypeAnnotatedValue::Record(record_value) => Value::Record(
                record_value
                    .value
                    .into_iter()
                    .map(|(name, value)| value.into())
                    .collect::<Vec<Value>>(),
            ),
            TypeAnnotatedValue::Flags(flag_value) => Value::Flags(flag_value.value),
            TypeAnnotatedValue::Enum(enum_value) => Value::Enum(enum_value.value),
            TypeAnnotatedValue::Option(optional_value) => {
                Value::Option(match optional_value.value {
                    Some(value) => Some(Box::new(value.deref().clone().into())),
                    None => None,
                })
            }
            TypeAnnotatedValue::Result(result_value) => Value::Result(match result_value.value {
                Ok(value) => Ok(Some(Box::new(value.deref().clone().into()))),
                Err(value) => Err(Some(Box::new(value.deref().clone().into()))),
            }),
            TypeAnnotatedValue::Handle(value) => todo!(),
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

// An efficient read representation of a value with type information at every node
// Note that we do an extra annotation of AnalysedType at complex structures to fetch their type information with 0(1)
// However the typical use of TypeAnnotatedValue is similar to using a complete json structure
#[derive(Clone)]
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
    List(ListValue),
    Tuple(TupleValue),
    Record(RecordValue),
    Flags(FlagValue),
    Variant(VariantValue),
    Enum(EnumValue),
    Option(OptionValue),
    Result(ResultValue),
    Handle(ResourceValue),
}

impl TypeAnnotatedValue {
    pub fn analysed_typ(&self) -> AnalysedType {
        match self {
            TypeAnnotatedValue::Bool(_) => {
                AnalysedType(golem_wasm_ast::analysis::AnalysedType::Bool)
            }
            TypeAnnotatedValue::S8(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::S8),
            TypeAnnotatedValue::U8(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::U8),
            TypeAnnotatedValue::S16(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::S16),
            TypeAnnotatedValue::U16(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::U16),
            TypeAnnotatedValue::S32(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::S32),
            TypeAnnotatedValue::U32(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::U32),
            TypeAnnotatedValue::S64(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::S64),
            TypeAnnotatedValue::U64(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::U64),
            TypeAnnotatedValue::F32(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::F32),
            TypeAnnotatedValue::F64(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::F64),
            TypeAnnotatedValue::Chr(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::Chr),
            TypeAnnotatedValue::Str(_) => AnalysedType(golem_wasm_ast::analysis::AnalysedType::Str),
            TypeAnnotatedValue::List(value) => value.clone().typ,
            TypeAnnotatedValue::Tuple(value) => value.clone().typ,
            TypeAnnotatedValue::Record(value) => value.clone().typ,
            TypeAnnotatedValue::Flags(value) => AnalysedType(
                golem_wasm_ast::analysis::AnalysedType::Flags(value.clone().typ),
            ),
            TypeAnnotatedValue::Enum(value) => value.clone().typ,
            TypeAnnotatedValue::Option(value) => value.clone().typ,
            TypeAnnotatedValue::Result(value) => {
                AnalysedType(golem_wasm_ast::analysis::AnalysedType::Result {
                    ok: value.clone().ok.map(|value| Box::new(value.0)),
                    error: value.clone().error.map(|value| Box::new(value.0)),
                })
            }
            TypeAnnotatedValue::Handle(value) => {
                AnalysedType(golem_wasm_ast::analysis::AnalysedType::Resource {
                    id: value.clone().id,
                    resource_mode: value.clone().resource_mode,
                })
            }
            TypeAnnotatedValue::Variant(value) => {
                AnalysedType(golem_wasm_ast::analysis::AnalysedType::Variant(
                    value.clone().typ.into_iter().map(|(name, ty)| (name, ty.map(|t| t.0))).collect(),
                ))
            }
        }
    }
}

impl From<TypeAnnotatedValue> for WitValue {
    fn from(value: TypeAnnotatedValue) -> Self {
        let value: Value = value.into();
        value.into()
    }
}

impl WasmValue for TypeAnnotatedValue {
    type Type = AnalysedType;
    fn ty(&self) -> Self::Type {
        self.typ.clone()
    }

    fn make_bool(val: bool) -> Self {
        TypeAnnotatedValue::Bool(val)
    }

    fn make_s8(val: i8) -> Self {
        TypeAnnotatedValue::S8(val)
    }

    fn make_s16(val: i16) -> Self {
        TypeAnnotatedValue::S16(val)
    }

    fn make_s32(val: i32) -> Self {
        TypeAnnotatedValue::S32(val)
    }

    fn make_s64(val: i64) -> Self {
        TypeAnnotatedValue::S64(val)
    }

    fn make_u8(val: u8) -> Self {
        TypeAnnotatedValue::U8(val)
    }

    fn make_u16(val: u16) -> Self {
        TypeAnnotatedValue::U16(val)
    }

    fn make_u32(val: u32) -> Self {
        TypeAnnotatedValue::U32(val)
    }

    fn make_u64(val: u64) -> Self {
        TypeAnnotatedValue::U64(val)
    }

    fn make_float32(val: f32) -> Self {
        TypeAnnotatedValue::F32(val)
    }

    fn make_float64(val: f64) -> Self {
        TypeAnnotatedValue::F64(val)
    }

    fn make_char(val: char) -> Self {
        TypeAnnotatedValue::Chr(val)
    }

    fn make_string(val: String) -> Self {
        TypeAnnotatedValue::Str(val)
    }

    fn make_list(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypeAnnotatedValue::List(ListValue {
            values: vals.into_iter().collect(),
            typ: ty.clone(),
        }))
    }

    fn make_record<'a>(
        ty: &Self::Type,
        fields: impl IntoIterator<Item = (&'a str, Self)>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypeAnnotatedValue::Record(RecordValue {
            value: fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
            typ: ty.clone(),
        }))
    }

    fn make_tuple(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypeAnnotatedValue::Tuple(TupleValue {
            value: vals.into_iter().collect(),
            typ: ty.clone(),
        }))
    }

    fn make_variant(
        ty: &Self::Type,
        case: &str,
        val: Option<Self>,
    ) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::Variant(cases) = &ty.0 {
            let case_type = cases
                .iter()
                .enumerate()
                .find_map(
                    |(idx, (name, case_type))| if name == case { Some(case_type) } else { None },
                );
            if let Some(case_type) = case_type {
                Ok(TypeAnnotatedValue::Variant(VariantValue {
                    typ: cases
                        .clone()
                        .iter()
                        .map(|(name, case_type)| {
                            (name.clone(), case_type.clone().map(|ty| AnalysedType(ty)))
                        })
                        .collect::<Vec<_>>(),
                    case_name: case.to_string(),
                    case_value: val.map(|v| Box::new(v)),
                }))
            } else {
                Err(WasmValueError::UnknownCase(case.to_string()))
            }
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_enum(ty: &Self::Type, case: &str) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::Enum(cases) = &ty.0 {
            if cases.contains(&case.to_string()) {
                Ok(TypeAnnotatedValue::Enum(
                    EnumValue {
                        typ: cases.clone(),
                        value: case.to_string(),
                    },
                ))
            } else {
                Err(WasmValueError::UnknownCase(case.to_string()))
            }
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_flags<'a>(
        ty: &Self::Type,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::Flags(all_names) = &ty.0 {

            let invalid_names: Vec<&String> = names.iter()
                .filter(|name| !all_names.contains(*name))
                .collect();

            if invalid_names.is_empty(){
                Ok(
                    TypeAnnotatedValue::Flags(FlagValue{
                        typ: all_names.clone(),
                        value: names.into_iter().map(|name| name.to_string()).collect()

                    })
                )
            } else {
                Err(WasmValueError::UnknownCase(invalid_names.join(", ")))
            }

        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }
}

#[derive(Clone)]
pub struct EnumValue {
    typ: Vec<String>,
    value: String,
}

#[derive(Clone)]
pub struct OptionValue {
    typ: AnalysedType,
    value: Option<Box<TypeAnnotatedValue>>,
}

#[derive(Clone)]
pub struct FlagValue {
    typ: Vec<String>,
    value: Vec<String>, // value should be a subset of typ field here.
}

pub struct VariantValue {
    typ: Vec<(String, Option<AnalysedType>)>,
    case_name: String,
    case_value: Option<Box<TypeAnnotatedValue>>,
}

#[derive(Clone)]
pub struct TupleValue {
    typ: AnalysedType,
    value: Vec<TypeAnnotatedValue>,
}

#[derive(Clone)]
pub struct RecordValue {
    typ: AnalysedType,
    value: Vec<(String, TypeAnnotatedValue)>,
}

//
// The law here is:
//     let mut types = Vec::new();
//       for value in values {
//          types.push(value.analysed_typ());
//       }
//       let head = types.map(|value| value.into())).head
//       types.forall(types == head)
//
#[derive(Clone)]
pub struct ListValue {
    typ: AnalysedType,
    values: Vec<TypeAnnotatedValue>,
}

#[derive(Clone)]
pub struct ResultValue {
    ok: Option<Box<AnalysedType>>,
    error: Option<Box<AnalysedType>>,
    value: Result<Box<TypeAnnotatedValue>, Box<TypeAnnotatedValue>>,
}

#[derive(Clone)]
pub struct ResourceValue {
    id: AnalysedResourceId,
    resource_mode: AnalysedResourceMode,
    uri: Uri,
    resource_id: u64,
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
