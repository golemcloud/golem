// Copyright 2024-2025 Golem Cloud
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

use crate::protobuf::typed_result::ResultValue;
use crate::Value;
use golem_wasm_ast::analysis::{AnalysedFunctionParameter, AnalysedType};

include!(concat!(env!("OUT_DIR"), "/wasm.rpc.rs"));

pub use golem_wasm_ast::analysis::protobuf::*;

// Conversion from WIT WitValue to Protobuf WitValue

impl From<super::WitValue> for WitValue {
    fn from(value: super::WitValue) -> Self {
        WitValue {
            nodes: value.nodes.into_iter().map(|node| node.into()).collect(),
        }
    }
}

impl From<super::WitNode> for WitNode {
    fn from(value: super::WitNode) -> Self {
        match value {
            super::WitNode::RecordValue(fields) => WitNode {
                value: Some(wit_node::Value::Record(WitRecordNode { fields })),
            },
            super::WitNode::VariantValue((case_index, case_value)) => WitNode {
                value: Some(wit_node::Value::Variant(WitVariantNode {
                    case_index,
                    case_value,
                })),
            },
            super::WitNode::EnumValue(value) => WitNode {
                value: Some(wit_node::Value::Enum(WitEnumNode { value })),
            },
            super::WitNode::FlagsValue(flags) => WitNode {
                value: Some(wit_node::Value::Flags(WitFlagsNode { flags })),
            },
            super::WitNode::TupleValue(values) => WitNode {
                value: Some(wit_node::Value::Tuple(WitTupleNode { values })),
            },
            super::WitNode::ListValue(values) => WitNode {
                value: Some(wit_node::Value::List(WitListNode { values })),
            },
            super::WitNode::OptionValue(value) => WitNode {
                value: Some(wit_node::Value::Option(WitOptionNode { value })),
            },
            super::WitNode::ResultValue(type_idx) => WitNode {
                value: Some(wit_node::Value::Result(WitResultNode {
                    discriminant: if type_idx.is_ok() { 0 } else { 1 },
                    value: type_idx.unwrap_or_else(|index| index),
                })),
            },
            super::WitNode::PrimU8(value) => WitNode {
                value: Some(wit_node::Value::U8(WitPrimU8Node {
                    value: value as u32,
                })),
            },
            super::WitNode::PrimU16(value) => WitNode {
                value: Some(wit_node::Value::U16(WitPrimU16Node {
                    value: value as u32,
                })),
            },
            super::WitNode::PrimU32(value) => WitNode {
                value: Some(wit_node::Value::U32(WitPrimU32Node { value })),
            },
            super::WitNode::PrimU64(value) => WitNode {
                value: Some(wit_node::Value::U64(WitPrimU64Node { value })),
            },
            super::WitNode::PrimS8(value) => WitNode {
                value: Some(wit_node::Value::I8(WitPrimI8Node {
                    value: value as i32,
                })),
            },
            super::WitNode::PrimS16(value) => WitNode {
                value: Some(wit_node::Value::I16(WitPrimI16Node {
                    value: value as i32,
                })),
            },
            super::WitNode::PrimS32(value) => WitNode {
                value: Some(wit_node::Value::I32(WitPrimI32Node { value })),
            },
            super::WitNode::PrimS64(value) => WitNode {
                value: Some(wit_node::Value::I64(WitPrimI64Node { value })),
            },
            super::WitNode::PrimFloat32(value) => WitNode {
                value: Some(wit_node::Value::F32(WitPrimF32Node { value })),
            },
            super::WitNode::PrimFloat64(value) => WitNode {
                value: Some(wit_node::Value::F64(WitPrimF64Node { value })),
            },
            super::WitNode::PrimChar(value) => WitNode {
                value: Some(wit_node::Value::Char(WitPrimCharNode {
                    value: value as u32,
                })),
            },
            super::WitNode::PrimBool(value) => WitNode {
                value: Some(wit_node::Value::Bool(WitPrimBoolNode { value })),
            },
            super::WitNode::PrimString(value) => WitNode {
                value: Some(wit_node::Value::String(WitPrimStringNode { value })),
            },
            super::WitNode::Handle((uri, value)) => WitNode {
                value: Some(wit_node::Value::Handle(WitHandleNode {
                    uri: uri.value,
                    value,
                })),
            },
        }
    }
}

// Conversion from Protobuf WitValue to WIT WitValue
impl TryFrom<WitValue> for super::WitValue {
    type Error = String;

    fn try_from(value: WitValue) -> Result<Self, Self::Error> {
        Ok(super::WitValue {
            nodes: value
                .nodes
                .into_iter()
                .map(|node| node.try_into())
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl TryFrom<WitNode> for super::WitNode {
    type Error = String;

    fn try_from(value: WitNode) -> Result<Self, Self::Error> {
        match value.value {
            None => Err("Protobuf WitNode has no value".to_string()),
            Some(wit_node::Value::Record(WitRecordNode { fields })) => {
                Ok(super::WitNode::RecordValue(fields))
            }
            Some(wit_node::Value::Variant(WitVariantNode {
                case_index,
                case_value,
            })) => Ok(super::WitNode::VariantValue((case_index, case_value))),
            Some(wit_node::Value::Enum(WitEnumNode { value })) => {
                Ok(super::WitNode::EnumValue(value))
            }
            Some(wit_node::Value::Flags(WitFlagsNode { flags })) => {
                Ok(super::WitNode::FlagsValue(flags))
            }
            Some(wit_node::Value::Tuple(WitTupleNode { values })) => {
                Ok(super::WitNode::TupleValue(values))
            }
            Some(wit_node::Value::List(WitListNode { values })) => {
                Ok(super::WitNode::ListValue(values))
            }
            Some(wit_node::Value::Option(WitOptionNode { value })) => {
                Ok(super::WitNode::OptionValue(value))
            }
            Some(wit_node::Value::Result(WitResultNode {
                discriminant,
                value,
            })) => match discriminant {
                0 => Ok(super::WitNode::ResultValue(Ok(value))),
                1 => Ok(super::WitNode::ResultValue(Err(value))),
                _ => Err("Protobuf WitResultNode has invalid discriminant".to_string()),
            },
            Some(wit_node::Value::U8(WitPrimU8Node { value })) => {
                Ok(super::WitNode::PrimU8(value as u8))
            }
            Some(wit_node::Value::U16(WitPrimU16Node { value })) => {
                Ok(super::WitNode::PrimU16(value as u16))
            }
            Some(wit_node::Value::U32(WitPrimU32Node { value })) => {
                Ok(super::WitNode::PrimU32(value))
            }
            Some(wit_node::Value::U64(WitPrimU64Node { value })) => {
                Ok(super::WitNode::PrimU64(value))
            }
            Some(wit_node::Value::I8(WitPrimI8Node { value })) => {
                Ok(super::WitNode::PrimS8(value as i8))
            }
            Some(wit_node::Value::I16(WitPrimI16Node { value })) => {
                Ok(super::WitNode::PrimS16(value as i16))
            }
            Some(wit_node::Value::I32(WitPrimI32Node { value })) => {
                Ok(super::WitNode::PrimS32(value))
            }
            Some(wit_node::Value::I64(WitPrimI64Node { value })) => {
                Ok(super::WitNode::PrimS64(value))
            }
            Some(wit_node::Value::F32(WitPrimF32Node { value })) => {
                Ok(super::WitNode::PrimFloat32(value))
            }
            Some(wit_node::Value::F64(WitPrimF64Node { value })) => {
                Ok(super::WitNode::PrimFloat64(value))
            }
            Some(wit_node::Value::Char(WitPrimCharNode { value })) => Ok(super::WitNode::PrimChar(
                char::from_u32(value)
                    .ok_or("Protobuf WitPrimCharNode has invalid value".to_string())?,
            )),
            Some(wit_node::Value::Bool(WitPrimBoolNode { value })) => {
                Ok(super::WitNode::PrimBool(value))
            }
            Some(wit_node::Value::String(WitPrimStringNode { value })) => {
                Ok(super::WitNode::PrimString(value))
            }
            Some(wit_node::Value::Handle(WitHandleNode { uri, value })) => {
                Ok(super::WitNode::Handle((super::Uri { value: uri }, value)))
            }
        }
    }
}

// Conversion from WitValue to protobuf Val
impl From<super::WitValue> for Val {
    fn from(value: super::WitValue) -> Self {
        let value: Value = value.into();
        value.into()
    }
}

impl TryFrom<&type_annotated_value::TypeAnnotatedValue> for Type {
    type Error = String;

    fn try_from(value: &type_annotated_value::TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let r#type = match value {
            type_annotated_value::TypeAnnotatedValue::Bool(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::Bool as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::S8(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::S8 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::U8(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::U8 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::S16(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::S16 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::U16(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::U16 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::S32(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::S32 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::U32(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::U32 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::S64(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::S64 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::U64(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::U64 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::F32(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::F32 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::F64(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::F64 as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::Char(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::Chr as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::Str(_) => {
                Ok(r#type::Type::Primitive(TypePrimitive {
                    primitive: PrimitiveType::Str as i32,
                }))
            }
            type_annotated_value::TypeAnnotatedValue::List(TypedList { typ, values: _ }) => {
                if let Some(typ) = typ.clone() {
                    Ok(r#type::Type::List(Box::new(TypeList {
                        elem: Some(Box::new(typ)),
                    })))
                } else {
                    Err("Missing type for List".to_string())
                }
            }
            type_annotated_value::TypeAnnotatedValue::Tuple(TypedTuple { typ, value: _ }) => {
                Ok(r#type::Type::Tuple(TypeTuple { elems: typ.clone() }))
            }
            type_annotated_value::TypeAnnotatedValue::Record(TypedRecord { typ, value: _ }) => {
                Ok(r#type::Type::Record(TypeRecord {
                    fields: typ.clone(),
                }))
            }
            type_annotated_value::TypeAnnotatedValue::Flags(TypedFlags { typ, values: _ }) => {
                Ok(r#type::Type::Flags(TypeFlags { names: typ.clone() }))
            }
            type_annotated_value::TypeAnnotatedValue::Enum(TypedEnum { typ, value: _ }) => {
                Ok(r#type::Type::Enum(TypeEnum { names: typ.clone() }))
            }
            type_annotated_value::TypeAnnotatedValue::Option(option) => {
                let typ = option.typ.clone();
                Ok(r#type::Type::Option(Box::new(TypeOption {
                    elem: typ.map(Box::new),
                })))
            }
            type_annotated_value::TypeAnnotatedValue::Result(result0) => {
                Ok(r#type::Type::Result(Box::new(TypeResult {
                    ok: result0.ok.clone().map(Box::new),
                    err: result0.error.clone().map(Box::new),
                })))
            }
            type_annotated_value::TypeAnnotatedValue::Handle(TypedHandle { typ, .. }) => {
                if let Some(typ) = *typ {
                    Ok(r#type::Type::Handle(typ))
                } else {
                    Err("Missing type for Handle".to_string())
                }
            }
            type_annotated_value::TypeAnnotatedValue::Variant(variant) => {
                if let Some(typ) = variant.clone().typ {
                    Ok(r#type::Type::Variant(typ))
                } else {
                    Err("Missing type for Variant".to_string())
                }
            }
        };

        r#type.map(|r#type| Type {
            r#type: Some(r#type),
        })
    }
}

impl TryFrom<&type_annotated_value::TypeAnnotatedValue> for AnalysedType {
    type Error = String;
    fn try_from(value: &type_annotated_value::TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let typ = Type::try_from(value)?;
        AnalysedType::try_from(&typ)
    }
}

impl TryFrom<type_annotated_value::TypeAnnotatedValue> for super::WitValue {
    type Error = String;

    fn try_from(value: type_annotated_value::TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let value: Value = value.try_into()?;
        Ok(super::WitValue::from(value))
    }
}

impl TryFrom<type_annotated_value::TypeAnnotatedValue> for Value {
    type Error = String;

    fn try_from(value: type_annotated_value::TypeAnnotatedValue) -> Result<Self, Self::Error> {
        match value {
            type_annotated_value::TypeAnnotatedValue::Bool(value) => Ok(Value::Bool(value)),
            type_annotated_value::TypeAnnotatedValue::S8(value) => Ok(Value::S8(value as i8)),
            type_annotated_value::TypeAnnotatedValue::U8(value) => Ok(Value::U8(value as u8)),
            type_annotated_value::TypeAnnotatedValue::S16(value) => Ok(Value::S16(value as i16)),
            type_annotated_value::TypeAnnotatedValue::U16(value) => Ok(Value::U16(value as u16)),
            type_annotated_value::TypeAnnotatedValue::S32(value) => Ok(Value::S32(value)),
            type_annotated_value::TypeAnnotatedValue::U32(value) => Ok(Value::U32(value)),
            type_annotated_value::TypeAnnotatedValue::S64(value) => Ok(Value::S64(value)),
            type_annotated_value::TypeAnnotatedValue::U64(value) => Ok(Value::U64(value)),
            type_annotated_value::TypeAnnotatedValue::F32(value) => Ok(Value::F32(value)),
            type_annotated_value::TypeAnnotatedValue::F64(value) => Ok(Value::F64(value)),
            type_annotated_value::TypeAnnotatedValue::Char(value) => char::from_u32(value as u32)
                .map(Value::Char)
                .ok_or_else(|| "Invalid char value".to_string()),
            type_annotated_value::TypeAnnotatedValue::Str(value) => Ok(Value::String(value)),
            type_annotated_value::TypeAnnotatedValue::List(TypedList { typ: _, values }) => {
                let mut list_values = Vec::new();
                for value in values {
                    let type_annotated_value = value
                        .clone()
                        .type_annotated_value
                        .ok_or_else(|| "Missing type_annotated_value in List".to_string())?;
                    list_values.push(type_annotated_value.try_into()?);
                }
                Ok(Value::List(list_values))
            }
            type_annotated_value::TypeAnnotatedValue::Tuple(TypedTuple { typ: _, value }) => {
                let mut tuple_values = Vec::new();
                for value in value {
                    let type_annotated_value = value
                        .type_annotated_value
                        .ok_or_else(|| "Missing type_annotated_value in Tuple".to_string())?;
                    tuple_values.push(type_annotated_value.try_into()?);
                }
                Ok(Value::Tuple(tuple_values))
            }
            type_annotated_value::TypeAnnotatedValue::Record(TypedRecord { typ: _, value }) => {
                let mut record_values = Vec::new();
                for name_value in value {
                    let type_annotated_value = name_value
                        .value
                        .ok_or_else(|| "Missing value in Record".to_string())?
                        .type_annotated_value
                        .ok_or_else(|| "Missing type_annotated_value in Record".to_string())?;
                    record_values.push(type_annotated_value.try_into()?);
                }
                Ok(Value::Record(record_values))
            }
            type_annotated_value::TypeAnnotatedValue::Flags(TypedFlags { typ, values }) => {
                let mut bools = Vec::new();
                for expected_flag in typ {
                    bools.push(values.contains(&expected_flag));
                }
                Ok(Value::Flags(bools))
            }
            type_annotated_value::TypeAnnotatedValue::Enum(TypedEnum { typ, value }) => typ
                .iter()
                .position(|expected_enum| expected_enum == &value)
                .map(|index| Value::Enum(index as u32))
                .ok_or_else(|| "Enum value not found in the list of expected enums".to_string()),
            type_annotated_value::TypeAnnotatedValue::Option(option) => match option.value {
                Some(value) => {
                    let type_annotated_value = value
                        .type_annotated_value
                        .ok_or_else(|| "Missing type_annotated_value in Option".to_string())?;
                    let value = type_annotated_value.try_into()?;
                    Ok(Value::Option(Some(Box::new(value))))
                }
                None => Ok(Value::Option(None)),
            },
            type_annotated_value::TypeAnnotatedValue::Result(result) => {
                let value = result
                    .result_value
                    .ok_or_else(|| "Missing value in Result".to_string())?;

                match value {
                    ResultValue::OkValue(ok_value) => {
                        let type_annotated_value = ok_value.type_annotated_value;

                        match type_annotated_value {
                            Some(v) => {
                                let value = v.try_into()?;
                                Ok(Value::Result(Ok(Some(Box::new(value)))))
                            }
                            None => Ok(Value::Result(Ok(None))),
                        }
                    }

                    ResultValue::ErrorValue(err_value) => {
                        let type_annotated_value = err_value.type_annotated_value;

                        match type_annotated_value {
                            Some(v) => {
                                let value = v.try_into()?;
                                Ok(Value::Result(Err(Some(Box::new(value)))))
                            }

                            None => Ok(Value::Result(Err(None))),
                        }
                    }
                }
            }
            type_annotated_value::TypeAnnotatedValue::Handle(handle) => Ok(Value::Handle {
                uri: handle.uri,
                resource_id: handle.resource_id,
            }),
            type_annotated_value::TypeAnnotatedValue::Variant(variant) => {
                let case_value = variant.case_value;
                let case_name = variant.case_name;
                let typ = variant
                    .typ
                    .ok_or_else(|| "Missing typ in Variant".to_string())?
                    .cases
                    .iter()
                    .map(|nt| (nt.name.clone(), nt.typ.clone()))
                    .collect::<Vec<_>>();

                let case_idx = typ
                    .iter()
                    .position(|(name, _)| name == &case_name)
                    .ok_or_else(|| "Case name not found in Variant".to_string())?
                    as u32;

                match case_value {
                    Some(value) => {
                        let type_annotated_value = value
                            .type_annotated_value
                            .ok_or_else(|| "Missing type_annotated_value in Variant".to_string())?;
                        let result = type_annotated_value.try_into()?;
                        Ok(Value::Variant {
                            case_idx,
                            case_value: Some(Box::new(result)),
                        })
                    }
                    None => Ok(Value::Variant {
                        case_idx,
                        case_value: None,
                    }),
                }
            }
        }
    }
}

impl From<Value> for Val {
    fn from(value: Value) -> Self {
        match value {
            Value::Bool(value) => Val {
                val: Some(val::Val::Bool(value)),
            },
            Value::U8(value) => Val {
                val: Some(val::Val::U8(value as i32)),
            },
            Value::U16(value) => Val {
                val: Some(val::Val::U16(value as i32)),
            },
            Value::U32(value) => Val {
                val: Some(val::Val::U32(value as i64)),
            },
            Value::U64(value) => Val {
                val: Some(val::Val::U64(value as i64)),
            },
            Value::S8(value) => Val {
                val: Some(val::Val::S8(value as i32)),
            },
            Value::S16(value) => Val {
                val: Some(val::Val::S16(value as i32)),
            },
            Value::S32(value) => Val {
                val: Some(val::Val::S32(value)),
            },
            Value::S64(value) => Val {
                val: Some(val::Val::S64(value)),
            },
            Value::F32(value) => Val {
                val: Some(val::Val::F32(value)),
            },
            Value::F64(value) => Val {
                val: Some(val::Val::F64(value)),
            },
            Value::Char(value) => Val {
                val: Some(val::Val::Char(value as i32)),
            },
            Value::String(value) => Val {
                val: Some(val::Val::String(value)),
            },
            Value::List(items) => Val {
                val: Some(val::Val::List(ValList {
                    values: items.into_iter().map(|item| item.into()).collect(),
                })),
            },
            Value::Tuple(items) => Val {
                val: Some(val::Val::Tuple(ValTuple {
                    values: items.into_iter().map(|item| item.into()).collect(),
                })),
            },
            Value::Record(fields) => Val {
                val: Some(val::Val::Record(ValRecord {
                    values: fields.into_iter().map(|value| value.into()).collect(),
                })),
            },
            Value::Variant {
                case_idx,
                case_value,
            } => Val {
                val: Some(val::Val::Variant(Box::new(ValVariant {
                    discriminant: case_idx as i32,
                    value: case_value.map(|case_value| Box::new((*case_value).into())),
                }))),
            },
            Value::Enum(value) => Val {
                val: Some(val::Val::Enum(ValEnum {
                    discriminant: value as i32,
                })),
            },
            Value::Flags(values) => {
                let mut indexes = Vec::with_capacity(values.len());
                for (i, value) in values.iter().enumerate() {
                    if *value {
                        indexes.push(i as i32);
                    }
                }
                Val {
                    val: Some(val::Val::Flags(ValFlags {
                        count: values.len() as i32,
                        value: indexes,
                    })),
                }
            }
            Value::Option(Some(value)) => Val {
                val: Some(val::Val::Option(Box::new(ValOption {
                    discriminant: 1,
                    value: Some(Box::new((*value).into())),
                }))),
            },
            Value::Option(None) => Val {
                val: Some(val::Val::Option(Box::new(ValOption {
                    discriminant: 0,
                    value: None,
                }))),
            },
            Value::Result(Ok(value)) => Val {
                val: Some(val::Val::Result(Box::new(ValResult {
                    discriminant: 0,
                    value: value.map(|value| Box::new((*value).into())),
                }))),
            },
            Value::Result(Err(value)) => Val {
                val: Some(val::Val::Result(Box::new(ValResult {
                    discriminant: 1,
                    value: value.map(|value| Box::new((*value).into())),
                }))),
            },
            Value::Handle { uri, resource_id } => Val {
                val: Some(val::Val::Handle(ValHandle {
                    uri,
                    value: resource_id,
                })),
            },
        }
    }
}

// Conversion from protobuf Val to WitValue
impl TryFrom<Val> for super::WitValue {
    type Error = String;

    fn try_from(value: Val) -> Result<Self, Self::Error> {
        let value: Value = value.try_into()?;
        Ok(value.into())
    }
}

impl TryFrom<Val> for Value {
    type Error = String;

    fn try_from(value: Val) -> Result<Self, Self::Error> {
        match value.val {
            None => Err("Protobuf Val has no value".to_string()),
            Some(val::Val::Bool(value)) => Ok(Value::Bool(value)),
            Some(val::Val::U8(value)) => Ok(Value::U8(value as u8)),
            Some(val::Val::U16(value)) => Ok(Value::U16(value as u16)),
            Some(val::Val::U32(value)) => Ok(Value::U32(value as u32)),
            Some(val::Val::U64(value)) => Ok(Value::U64(value as u64)),
            Some(val::Val::S8(value)) => Ok(Value::S8(value as i8)),
            Some(val::Val::S16(value)) => Ok(Value::S16(value as i16)),
            Some(val::Val::S32(value)) => Ok(Value::S32(value)),
            Some(val::Val::S64(value)) => Ok(Value::S64(value)),
            Some(val::Val::F32(value)) => Ok(Value::F32(value)),
            Some(val::Val::F64(value)) => Ok(Value::F64(value)),
            Some(val::Val::Char(value)) => Ok(Value::Char(
                char::from_u32(value as u32)
                    .ok_or("Protobuf WitPrimCharNode has invalid value".to_string())?,
            )),
            Some(val::Val::String(value)) => Ok(Value::String(value)),
            Some(val::Val::List(ValList { values })) => Ok(Value::List(
                values
                    .into_iter()
                    .map(|value| value.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Some(val::Val::Tuple(ValTuple { values })) => Ok(Value::Tuple(
                values
                    .into_iter()
                    .map(|value| value.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Some(val::Val::Record(ValRecord { values })) => Ok(Value::Record(
                values
                    .into_iter()
                    .map(|value| value.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Some(val::Val::Variant(variant)) => {
                let discriminant = variant.discriminant as u32;
                match variant.value {
                    Some(value) => Ok(Value::Variant {
                        case_idx: discriminant,
                        case_value: Some(Box::new((*value).try_into()?)),
                    }),
                    None => Ok(Value::Variant {
                        case_idx: discriminant,
                        case_value: None,
                    }),
                }
            }
            Some(val::Val::Enum(ValEnum { discriminant })) => Ok(Value::Enum(discriminant as u32)),
            Some(val::Val::Flags(ValFlags { count, value })) => {
                let mut flags = vec![false; count as usize];
                for i in value {
                    flags[i as usize] = true;
                }
                Ok(Value::Flags(flags))
            }
            Some(val::Val::Option(inner)) => {
                let ValOption {
                    discriminant,
                    value,
                } = *inner;
                match (discriminant, value) {
                    (0, None) => Ok(Value::Option(None)),
                    (1, Some(value)) => Ok(Value::Option(Some(Box::new((*value).try_into()?)))),
                    _ => Err("Protobuf ValOption has invalid discriminant or value".to_string()),
                }
            }
            Some(val::Val::Result(inner)) => {
                let ValResult {
                    discriminant,
                    value,
                } = *inner;
                match (discriminant, value) {
                    (0, Some(value)) => Ok(Value::Result(Ok(Some(Box::new((*value).try_into()?))))),
                    (0, None) => Ok(Value::Result(Ok(None))),
                    (1, Some(value)) => {
                        Ok(Value::Result(Err(Some(Box::new((*value).try_into()?)))))
                    }
                    (1, None) => Ok(Value::Result(Err(None))),
                    _ => Err("Protobuf ValResult has invalid discriminant or value".to_string()),
                }
            }
            Some(val::Val::Handle(ValHandle { uri, value })) => Ok(Value::Handle {
                uri,
                resource_id: value,
            }),
        }
    }
}

#[cfg(feature = "typeinfo")]
pub fn function_parameters(
    parameters: &[Val],
    expected_parameters: Vec<AnalysedFunctionParameter>,
) -> Result<(), Vec<String>> {
    if parameters.len() == expected_parameters.len() {
        Ok(())
    } else {
        Err(vec![format!(
            "Unexpected number of parameters (got {}, expected: {})",
            parameters.len(),
            expected_parameters.len()
        )])
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::{Val, WitValue};
    use crate::Value;
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb_sized;

    const CASES: u32 = 10000;
    const SIZE: usize = 4096;

    proptest! {

        #![proptest_config(ProptestConfig {
            cases: CASES, .. ProptestConfig::default()
        })]
        #[test]
        fn round_trip_wit_value(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let wit_value: crate::WitValue = value.clone().into();
            let protobuf_wit_value: WitValue = wit_value.into();
            let round_trip_wit_value: crate::WitValue = protobuf_wit_value.try_into().unwrap();
            let round_trip_value: Value = round_trip_wit_value.into();
            prop_assert_eq!(value, round_trip_value);
        }

        #[test]
        fn round_trip_val(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let wit_value: crate::WitValue = value.clone().into();

            let protobuf_val: Val = wit_value.into();
            let round_trip_wit_value: crate::WitValue = protobuf_val.try_into().unwrap();
            let round_trip_value: Value = round_trip_wit_value.into();
            prop_assert_eq!(value, round_trip_value);
        }
    }
}
