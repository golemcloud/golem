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

use crate::golem_rpc_0_2_x::types::NamedWitTypeNode;
use crate::{ResourceMode, RpcError, Value, WitNode, WitType, WitTypeNode, WitValue};
use golem_wasm_ast::analysis::analysed_type::{
    case, list, option, result, result_err, result_ok, str, tuple, u32, unit_case, variant,
};
use golem_wasm_ast::analysis::{
    analysed_type, AnalysedResourceId, AnalysedResourceMode, AnalysedType, NameTypePair, TypeEnum,
    TypeFlags,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bincode", derive(::bincode::Encode, ::bincode::Decode))]
pub struct ValueAndType {
    pub value: Value,
    pub typ: AnalysedType,
}

#[cfg(feature = "text")]
impl std::fmt::Display for ValueAndType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            crate::text::print_value_and_type(self).unwrap_or("<unprintable>".to_string())
        )
    }
}

impl ValueAndType {
    pub fn new(value: Value, typ: AnalysedType) -> Self {
        Self { value, typ }
    }

    pub fn into_list_items(self) -> Option<Vec<ValueAndType>> {
        match (self.value, self.typ) {
            (Value::List(items), AnalysedType::List(item_type)) => Some(
                items
                    .into_iter()
                    .map(|item| ValueAndType::new(item, (*item_type.inner).clone()))
                    .collect(),
            ),
            _ => None,
        }
    }
}

impl From<ValueAndType> for Value {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.value
    }
}

impl From<ValueAndType> for AnalysedType {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.typ
    }
}

#[cfg(feature = "host-bindings")]
impl From<ValueAndType> for WitValue {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.value.into()
    }
}

#[cfg(feature = "host-bindings")]
impl From<ValueAndType> for WitType {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.typ.into()
    }
}

/// Specific trait to convert a type into a pair of `Value` and `AnalysedType`.
pub trait IntoValue {
    fn into_value(self) -> Value;
    fn get_type() -> AnalysedType;
}

pub trait IntoValueAndType {
    fn into_value_and_type(self) -> ValueAndType;
}

impl<T: IntoValue + Sized> IntoValueAndType for T {
    fn into_value_and_type(self) -> ValueAndType {
        ValueAndType::new(self.into_value(), Self::get_type())
    }
}

impl IntoValue for u8 {
    fn into_value(self) -> Value {
        Value::U8(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::u8()
    }
}

impl IntoValue for u16 {
    fn into_value(self) -> Value {
        Value::U16(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::u16()
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Value {
        Value::U32(self)
    }

    fn get_type() -> AnalysedType {
        u32()
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::U64(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::u64()
    }
}

impl IntoValue for i8 {
    fn into_value(self) -> Value {
        Value::S8(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::s8()
    }
}

impl IntoValue for i16 {
    fn into_value(self) -> Value {
        Value::S16(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::s16()
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> Value {
        Value::S32(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::s32()
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Value {
        Value::S64(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::s64()
    }
}

impl IntoValue for f32 {
    fn into_value(self) -> Value {
        Value::F32(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::f32()
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Value {
        Value::F64(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::f64()
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::bool()
    }
}

impl IntoValue for char {
    fn into_value(self) -> Value {
        Value::Char(self)
    }

    fn get_type() -> AnalysedType {
        analysed_type::chr()
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }

    fn get_type() -> AnalysedType {
        str()
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }

    fn get_type() -> AnalysedType {
        str()
    }
}

impl<S: IntoValue, E: IntoValue> IntoValue for Result<S, E> {
    fn into_value(self) -> Value {
        match self {
            Ok(s) => Value::Result(Ok(Some(Box::new(s.into_value())))),
            Err(e) => Value::Result(Err(Some(Box::new(e.into_value())))),
        }
    }

    fn get_type() -> AnalysedType {
        result(S::get_type(), E::get_type())
    }
}

impl<E: IntoValue> IntoValue for Result<(), E> {
    fn into_value(self) -> Value {
        match self {
            Ok(_) => Value::Result(Ok(None)),
            Err(e) => Value::Result(Err(Some(Box::new(e.into_value())))),
        }
    }

    fn get_type() -> AnalysedType {
        result_err(E::get_type())
    }
}

impl<S: IntoValue> IntoValue for Result<S, ()> {
    fn into_value(self) -> Value {
        match self {
            Ok(s) => Value::Result(Ok(Some(Box::new(s.into_value())))),
            Err(_) => Value::Result(Err(None)),
        }
    }

    fn get_type() -> AnalysedType {
        result_ok(S::get_type())
    }
}

impl<T: IntoValue> IntoValue for Box<T> {
    fn into_value(self) -> Value {
        (*self).into_value()
    }

    fn get_type() -> AnalysedType {
        T::get_type()
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        match self {
            Some(t) => Value::Option(Some(Box::new(t.into_value()))),
            None => Value::Option(None),
        }
    }

    fn get_type() -> AnalysedType {
        option(T::get_type())
    }
}

impl<T: IntoValue> IntoValue for Bound<T> {
    fn into_value(self) -> Value {
        match self {
            Bound::Included(t) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(t.into_value())),
            },
            Bound::Excluded(t) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(t.into_value())),
            },
            Bound::Unbounded => Value::Variant {
                case_idx: 2,
                case_value: None,
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("included", T::get_type()),
            case("excluded", T::get_type()),
            unit_case("unbounded"),
        ])
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::List(self.into_iter().map(IntoValue::into_value).collect())
    }

    fn get_type() -> AnalysedType {
        list(T::get_type())
    }
}

impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn into_value(self) -> Value {
        Value::Tuple(vec![self.0.into_value(), self.1.into_value()])
    }

    fn get_type() -> AnalysedType {
        tuple(vec![A::get_type(), B::get_type()])
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue> IntoValue for (A, B, C) {
    fn into_value(self) -> Value {
        Value::Tuple(vec![
            self.0.into_value(),
            self.1.into_value(),
            self.2.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        tuple(vec![A::get_type(), B::get_type(), C::get_type()])
    }
}

impl<K: IntoValue, V: IntoValue> IntoValue for HashMap<K, V> {
    fn into_value(self) -> Value {
        Value::List(
            self.into_iter()
                .map(|(k, v)| Value::Tuple(vec![k.into_value(), v.into_value()]))
                .collect(),
        )
    }

    fn get_type() -> AnalysedType {
        list(tuple(vec![K::get_type(), V::get_type()]))
    }
}

impl<K: IntoValue, V: IntoValue> IntoValue for BTreeMap<K, V> {
    fn into_value(self) -> Value {
        Value::List(
            self.into_iter()
                .map(|(k, v)| Value::Tuple(vec![k.into_value(), v.into_value()]))
                .collect(),
        )
    }

    fn get_type() -> AnalysedType {
        list(tuple(vec![K::get_type(), V::get_type()]))
    }
}

impl<T: IntoValue> IntoValue for BTreeSet<T> {
    fn into_value(self) -> Value {
        Value::List(self.into_iter().map(IntoValue::into_value).collect())
    }

    fn get_type() -> AnalysedType {
        list(T::get_type())
    }
}

impl IntoValue for Uuid {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }

    fn get_type() -> AnalysedType {
        analysed_type::str()
    }
}

/// Helper for dynamically creating record ValueAndType values with String keys
pub struct Record<K: AsRef<str>>(pub Vec<(K, ValueAndType)>);

impl<K: AsRef<str>> IntoValueAndType for Record<K> {
    fn into_value_and_type(self) -> ValueAndType {
        let mut field_types = Vec::<NameTypePair>::with_capacity(self.0.len());
        let mut field_values = Vec::<Value>::with_capacity(self.0.len());

        for (field_name, value_and_type) in self.0 {
            field_types.push(NameTypePair {
                name: field_name.as_ref().to_string(),
                typ: value_and_type.typ,
            });
            field_values.push(value_and_type.value);
        }

        ValueAndType {
            value: Value::Record(field_values),
            typ: analysed_type::record(field_types),
        }
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for crate::WitValue {
    fn into_value(self) -> Value {
        // NOTE: this is different than From<WitValue> for Value. That conversion creates
        // the Value the WitValue describes, while this conversion creates a Value version of
        // the WitValue representation itself.
        Value::Record(vec![self.nodes.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field(
            "nodes",
            list(crate::WitNode::get_type()),
        )])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for WitNode {
    fn into_value(self) -> Value {
        use crate::WitNode;

        match self {
            WitNode::RecordValue(indices) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(indices.into_value())),
            },
            WitNode::VariantValue((idx, value)) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(Value::Tuple(vec![
                    idx.into_value(),
                    value
                        .map(IntoValue::into_value)
                        .unwrap_or(Value::Option(None)),
                ]))),
            },
            WitNode::EnumValue(idx) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(idx.into_value())),
            },
            WitNode::FlagsValue(flags) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(flags.into_value())),
            },
            WitNode::TupleValue(indices) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(indices.into_value())),
            },
            WitNode::ListValue(indices) => Value::Variant {
                case_idx: 5,
                case_value: Some(Box::new(indices.into_value())),
            },
            WitNode::OptionValue(index) => Value::Variant {
                case_idx: 6,
                case_value: Some(Box::new(index.into_value())),
            },
            WitNode::ResultValue(result) => Value::Variant {
                case_idx: 7,
                case_value: Some(Box::new(result.into_value())),
            },
            WitNode::PrimU8(value) => Value::Variant {
                case_idx: 8,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimU16(value) => Value::Variant {
                case_idx: 9,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimU32(value) => Value::Variant {
                case_idx: 10,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimU64(value) => Value::Variant {
                case_idx: 11,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimS8(value) => Value::Variant {
                case_idx: 12,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimS16(value) => Value::Variant {
                case_idx: 13,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimS32(value) => Value::Variant {
                case_idx: 14,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimS64(value) => Value::Variant {
                case_idx: 15,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimFloat32(value) => Value::Variant {
                case_idx: 16,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimFloat64(value) => Value::Variant {
                case_idx: 17,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimChar(value) => Value::Variant {
                case_idx: 18,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimBool(value) => Value::Variant {
                case_idx: 19,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::PrimString(value) => Value::Variant {
                case_idx: 20,
                case_value: Some(Box::new(value.into_value())),
            },
            WitNode::Handle((uri, resource_id)) => Value::Variant {
                case_idx: 21,
                case_value: Some(Box::new(Value::Tuple(vec![
                    uri.into_value(),
                    resource_id.into_value(),
                ]))),
            },
        }
    }

    fn get_type() -> AnalysedType {
        use crate::NodeIndex;
        use analysed_type::{case, variant};

        variant(vec![
            case("record-value", list(NodeIndex::get_type())),
            case(
                "variant-value",
                tuple(vec![analysed_type::u32(), option(NodeIndex::get_type())]),
            ),
            case("enum-value", analysed_type::u32()),
            case("flags-value", list(analysed_type::bool())),
            case("tuple-value", list(NodeIndex::get_type())),
            case("list-value", list(NodeIndex::get_type())),
            case("option-value", option(NodeIndex::get_type())),
            case(
                "result-value",
                result(option(NodeIndex::get_type()), option(NodeIndex::get_type())),
            ),
            case("prim-u8", analysed_type::u8()),
            case("prim-u16", analysed_type::u16()),
            case("prim-u32", analysed_type::u32()),
            case("prim-u64", analysed_type::u64()),
            case("prim-s8", analysed_type::s8()),
            case("prim-s16", analysed_type::s16()),
            case("prim-s32", analysed_type::s32()),
            case("prim-s64", analysed_type::s64()),
            case("prim-float32", analysed_type::f32()),
            case("prim-float64", analysed_type::f64()),
            case("prim-char", analysed_type::chr()),
            case("prim-bool", analysed_type::bool()),
            case("prim-string", analysed_type::str()),
            case(
                "handle",
                tuple(vec![crate::Uri::get_type(), analysed_type::u64()]),
            ),
        ])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for crate::Uri {
    fn into_value(self) -> Value {
        Value::Record(vec![Value::String(self.value)])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field("value", analysed_type::str())])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for WitType {
    fn into_value(self) -> Value {
        Value::Record(vec![self.nodes.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field(
            "nodes",
            list(WitTypeNode::get_type()),
        )])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for NamedWitTypeNode {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), self.type_.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("type", WitTypeNode::get_type()),
        ])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for WitTypeNode {
    fn into_value(self) -> Value {
        match self {
            WitTypeNode::RecordType(field_types) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(field_types.into_value())),
            },
            WitTypeNode::VariantType(case_types) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(case_types.into_value())),
            },
            WitTypeNode::EnumType(names) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(names.into_value())),
            },
            WitTypeNode::FlagsType(names) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(names.into_value())),
            },
            WitTypeNode::TupleType(types) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(types.into_value())),
            },
            WitTypeNode::ListType(elem) => Value::Variant {
                case_idx: 5,
                case_value: Some(Box::new(elem.into_value())),
            },
            WitTypeNode::OptionType(elem) => Value::Variant {
                case_idx: 6,
                case_value: Some(Box::new(elem.into_value())),
            },
            WitTypeNode::ResultType((ok, err)) => Value::Variant {
                case_idx: 7,
                case_value: Some(Box::new((ok, err).into_value())),
            },
            WitTypeNode::PrimU8Type => Value::Variant {
                case_idx: 8,
                case_value: None,
            },
            WitTypeNode::PrimU16Type => Value::Variant {
                case_idx: 9,
                case_value: None,
            },
            WitTypeNode::PrimU32Type => Value::Variant {
                case_idx: 10,
                case_value: None,
            },
            WitTypeNode::PrimU64Type => Value::Variant {
                case_idx: 11,
                case_value: None,
            },
            WitTypeNode::PrimS8Type => Value::Variant {
                case_idx: 12,
                case_value: None,
            },
            WitTypeNode::PrimS16Type => Value::Variant {
                case_idx: 13,
                case_value: None,
            },
            WitTypeNode::PrimS32Type => Value::Variant {
                case_idx: 14,
                case_value: None,
            },
            WitTypeNode::PrimS64Type => Value::Variant {
                case_idx: 15,
                case_value: None,
            },
            WitTypeNode::PrimF32Type => Value::Variant {
                case_idx: 16,
                case_value: None,
            },
            WitTypeNode::PrimF64Type => Value::Variant {
                case_idx: 17,
                case_value: None,
            },
            WitTypeNode::PrimCharType => Value::Variant {
                case_idx: 18,
                case_value: None,
            },
            WitTypeNode::PrimBoolType => Value::Variant {
                case_idx: 19,
                case_value: None,
            },
            WitTypeNode::PrimStringType => Value::Variant {
                case_idx: 20,
                case_value: None,
            },
            WitTypeNode::HandleType(handle) => Value::Variant {
                case_idx: 17,
                case_value: Some(Box::new(handle.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("record-type", list(tuple(vec![str(), u32()]))),
            case("variant-type", list(tuple(vec![str(), option(u32())]))),
            case("enum-type", list(str())),
            case("flags-type", list(str())),
            case("tuple-type", list(u32())),
            case("list-type", u32()),
            case("option-type", u32()),
            case("result-type", tuple(vec![option(u32()), option(u32())])),
            unit_case("prim-u8-type"),
            unit_case("prim-u16-type"),
            unit_case("prim-u32-type"),
            unit_case("prim-u64-type"),
            unit_case("prim-s8-type"),
            unit_case("prim-s16-type"),
            unit_case("prim-s32-type"),
            unit_case("prim-s64-type"),
            unit_case("prim-f32-type"),
            unit_case("prim-f64-type"),
            unit_case("prim-char-type"),
            unit_case("prim-bool-type"),
            unit_case("prim-string-type"),
            case(
                "handle-type",
                tuple(vec![analysed_type::u64(), ResourceMode::get_type()]),
            ),
        ])
    }
}

impl IntoValue for Instant {
    fn into_value(self) -> Value {
        Value::U64(self.elapsed().as_nanos() as u64)
    }

    fn get_type() -> AnalysedType {
        analysed_type::u64()
    }
}

impl IntoValue for Duration {
    fn into_value(self) -> Value {
        Value::U64(self.as_nanos() as u64)
    }

    fn get_type() -> AnalysedType {
        analysed_type::u64()
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for crate::ResourceMode {
    fn into_value(self) -> Value {
        match self {
            ResourceMode::Owned => Value::Enum(0),
            ResourceMode::Borrowed => Value::Enum(1),
        }
    }

    fn get_type() -> AnalysedType {
        analysed_type::r#enum(&["owned", "borrowed"])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for crate::RpcError {
    fn into_value(self) -> Value {
        match self {
            RpcError::ProtocolError(value) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(Value::String(value))),
            },
            RpcError::Denied(value) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(Value::String(value))),
            },
            RpcError::NotFound(value) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(Value::String(value))),
            },
            RpcError::RemoteInternalError(value) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(Value::String(value))),
            },
        }
    }

    fn get_type() -> AnalysedType {
        use analysed_type::case;

        variant(vec![
            case("protocol-error", analysed_type::str()),
            case("denied", analysed_type::str()),
            case("not-found", analysed_type::str()),
            case("remote-internal-error", analysed_type::str()),
        ])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for ValueAndType {
    fn into_value(self) -> Value {
        let wit_value: WitValue = self.value.into();
        let wit_type: WitType = self.typ.into();
        Value::Record(vec![wit_value.into_value(), wit_type.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("value", WitValue::get_type()),
            analysed_type::field("type", WitType::get_type()),
        ])
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for Value {
    fn into_value(self) -> Value {
        let wit_value: WitValue = self.into();
        wit_value.into_value()
    }

    fn get_type() -> AnalysedType {
        WitValue::get_type()
    }
}

#[cfg(feature = "host-bindings")]
impl IntoValue for AnalysedType {
    fn into_value(self) -> Value {
        let wit_type: WitType = self.into();
        wit_type.into_value()
    }

    fn get_type() -> AnalysedType {
        WitType::get_type()
    }
}

impl From<WitType> for AnalysedType {
    fn from(value: WitType) -> Self {
        assert!(!value.nodes.is_empty());
        build_tree(&value.nodes[0], &value.nodes)
    }
}

fn build_tree(node: &NamedWitTypeNode, nodes: &[NamedWitTypeNode]) -> AnalysedType {
    match &node.type_ {
        WitTypeNode::RecordType(fields) => {
            let fields = fields
                .iter()
                .map(|(name, idx)| {
                    let field_type = build_tree(&nodes[*idx as usize], nodes);
                    analysed_type::field(name, field_type)
                })
                .collect();
            analysed_type::record(fields).with_optional_name(node.name.clone())
        }
        WitTypeNode::VariantType(cases) => {
            let cases = cases
                .iter()
                .map(|(name, idx)| match idx {
                    Some(idx) => {
                        let case_type = build_tree(&nodes[*idx as usize], nodes);
                        analysed_type::case(name, case_type)
                    }
                    None => analysed_type::unit_case(name),
                })
                .collect();
            variant(cases).with_optional_name(node.name.clone())
        }
        WitTypeNode::EnumType(names) => AnalysedType::Enum(TypeEnum {
            cases: names.clone(),
            name: node.name.clone(),
            owner: node.owner.clone(),
        }),
        WitTypeNode::FlagsType(names) => AnalysedType::Flags(TypeFlags {
            names: names.clone(),
            name: node.name.clone(),
            owner: node.owner.clone(),
        }),
        WitTypeNode::TupleType(types) => {
            let types = types
                .iter()
                .map(|idx| build_tree(&nodes[*idx as usize], nodes))
                .collect();
            tuple(types).with_optional_name(node.name.clone())
        }
        WitTypeNode::ListType(elem_type) => {
            let elem_type = build_tree(&nodes[*elem_type as usize], nodes);
            list(elem_type).with_optional_name(node.name.clone())
        }
        WitTypeNode::OptionType(inner_type) => {
            let inner_type = build_tree(&nodes[*inner_type as usize], nodes);
            option(inner_type).with_optional_name(node.name.clone())
        }
        WitTypeNode::ResultType((ok_type, err_type)) => match (ok_type, err_type) {
            (Some(ok_type), Some(err_type)) => {
                let ok_type = build_tree(&nodes[*ok_type as usize], nodes);
                let err_type = build_tree(&nodes[*err_type as usize], nodes);
                result(ok_type, err_type).with_optional_name(node.name.clone())
            }
            (None, Some(err_type)) => {
                let err_type = build_tree(&nodes[*err_type as usize], nodes);
                result_err(err_type).with_optional_name(node.name.clone())
            }
            (Some(ok_type), None) => {
                let ok_type = build_tree(&nodes[*ok_type as usize], nodes);
                result_ok(ok_type).with_optional_name(node.name.clone())
            }
            (None, None) => panic!("ResultType with no ok_type or err_type"),
        },
        WitTypeNode::PrimU8Type => analysed_type::u8(),
        WitTypeNode::PrimU16Type => analysed_type::u16(),
        WitTypeNode::PrimU32Type => analysed_type::u32(),
        WitTypeNode::PrimU64Type => analysed_type::u64(),
        WitTypeNode::PrimS8Type => analysed_type::s8(),
        WitTypeNode::PrimS16Type => analysed_type::s16(),
        WitTypeNode::PrimS32Type => analysed_type::s32(),
        WitTypeNode::PrimS64Type => analysed_type::s64(),
        WitTypeNode::PrimF32Type => analysed_type::f32(),
        WitTypeNode::PrimF64Type => analysed_type::f64(),
        WitTypeNode::PrimCharType => analysed_type::chr(),
        WitTypeNode::PrimBoolType => analysed_type::bool(),
        WitTypeNode::PrimStringType => analysed_type::str(),
        WitTypeNode::HandleType((id, mode)) => analysed_type::handle(
            AnalysedResourceId(*id),
            match mode {
                crate::ResourceMode::Owned => AnalysedResourceMode::Owned,
                crate::ResourceMode::Borrowed => AnalysedResourceMode::Borrowed,
            },
        )
        .with_optional_name(node.name.clone()),
    }
}

impl From<AnalysedType> for WitType {
    fn from(value: AnalysedType) -> Self {
        let mut builder = WitTypeBuilder::new();
        builder.add(value);
        builder.build()
    }
}

struct WitTypeBuilder {
    nodes: Vec<NamedWitTypeNode>,
    mapping: HashMap<AnalysedType, usize>,
}

impl WitTypeBuilder {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            mapping: HashMap::new(),
        }
    }

    pub fn add(&mut self, typ: AnalysedType) -> usize {
        if let Some(idx) = self.mapping.get(&typ) {
            *idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(NamedWitTypeNode {
                name: None,
                owner: None,
                type_: WitTypeNode::PrimBoolType,
            }); // placeholder, to be replaced
            let name = typ.name().map(|n| n.to_string());
            let owner = typ.owner().map(|o| o.to_string());
            let node: WitTypeNode = match typ {
                AnalysedType::Variant(variant) => {
                    let mut cases = Vec::new();
                    for pair in variant.cases {
                        let case_idx = pair.typ.map(|case| self.add(case) as i32);
                        cases.push((pair.name, case_idx));
                    }
                    WitTypeNode::VariantType(cases)
                }
                AnalysedType::Result(result) => {
                    let ok_idx = result.ok.map(|ok| self.add(*ok) as i32);
                    let err_idx = result.err.map(|err| self.add(*err) as i32);
                    WitTypeNode::ResultType((ok_idx, err_idx))
                }
                AnalysedType::Option(option) => {
                    let inner_idx = self.add(*option.inner) as i32;
                    WitTypeNode::OptionType(inner_idx)
                }
                AnalysedType::Enum(enm) => WitTypeNode::EnumType(enm.cases),
                AnalysedType::Flags(flags) => WitTypeNode::FlagsType(flags.names),
                AnalysedType::Record(record) => {
                    let mut fields = Vec::new();
                    for field in record.fields {
                        fields.push((field.name, self.add(field.typ) as i32));
                    }
                    WitTypeNode::RecordType(fields)
                }
                AnalysedType::Tuple(tuple) => {
                    let mut indices = Vec::new();
                    for item in tuple.items {
                        indices.push(self.add(item) as i32);
                    }
                    WitTypeNode::TupleType(indices)
                }
                AnalysedType::List(lst) => {
                    let elem_idx = self.add(*lst.inner);
                    WitTypeNode::ListType(elem_idx as i32)
                }
                AnalysedType::Str(_) => WitTypeNode::PrimStringType,
                AnalysedType::Chr(_) => WitTypeNode::PrimCharType,
                AnalysedType::F64(_) => WitTypeNode::PrimF64Type,
                AnalysedType::F32(_) => WitTypeNode::PrimF32Type,
                AnalysedType::U64(_) => WitTypeNode::PrimU64Type,
                AnalysedType::S64(_) => WitTypeNode::PrimS64Type,
                AnalysedType::U32(_) => WitTypeNode::PrimU32Type,
                AnalysedType::S32(_) => WitTypeNode::PrimS32Type,
                AnalysedType::U16(_) => WitTypeNode::PrimU16Type,
                AnalysedType::S16(_) => WitTypeNode::PrimS16Type,
                AnalysedType::U8(_) => WitTypeNode::PrimU8Type,
                AnalysedType::S8(_) => WitTypeNode::PrimS8Type,
                AnalysedType::Bool(_) => WitTypeNode::PrimBoolType,
                AnalysedType::Handle(handle) => WitTypeNode::HandleType((
                    handle.resource_id.0,
                    match handle.mode {
                        AnalysedResourceMode::Owned => ResourceMode::Owned,
                        AnalysedResourceMode::Borrowed => ResourceMode::Borrowed,
                    },
                )),
            };
            self.nodes[idx] = NamedWitTypeNode {
                name,
                owner,
                type_: node,
            };
            idx
        }
    }

    pub fn build(self) -> WitType {
        WitType { nodes: self.nodes }
    }
}

impl From<crate::golem_rpc_0_2_x::types::ValueAndType> for ValueAndType {
    fn from(value: crate::golem_rpc_0_2_x::types::ValueAndType) -> Self {
        Self {
            value: value.value.into(),
            typ: value.typ.into(),
        }
    }
}

impl From<ValueAndType> for crate::golem_rpc_0_2_x::types::ValueAndType {
    fn from(value: ValueAndType) -> Self {
        Self {
            value: value.value.into(),
            typ: value.typ.into(),
        }
    }
}

#[cfg(feature = "extra-bindings")]
mod extra_bindings {
    use crate::{IntoValue, IntoValueAndType, Value};
    use bigdecimal::BigDecimal;
    use bit_vec::BitVec;
    use chrono::{Datelike, Offset, Timelike};
    use golem_wasm_ast::analysis::analysed_type::{bool, list, str};
    use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
    use url::Url;

    impl IntoValue for BigDecimal {
        fn into_value(self) -> Value {
            self.to_string().into_value()
        }

        fn get_type() -> AnalysedType {
            str()
        }
    }

    impl IntoValue for chrono::NaiveDate {
        fn into_value(self) -> Value {
            let year = self.year();
            let month = self.month() as u8;
            let day = self.day() as u8;
            Value::Record(vec![Value::S32(year), Value::U8(month), Value::U8(day)])
        }

        fn get_type() -> AnalysedType {
            analysed_type::record(vec![
                analysed_type::field("year", analysed_type::s32()),
                analysed_type::field("month", analysed_type::u8()),
                analysed_type::field("day", analysed_type::u8()),
            ])
        }
    }

    impl IntoValue for chrono::NaiveTime {
        fn into_value(self) -> Value {
            let hour = self.hour() as u8;
            let minute = self.minute() as u8;
            let second = self.second() as u8;
            let nanosecond = self.nanosecond();

            Value::Record(vec![
                Value::U8(hour),
                Value::U8(minute),
                Value::U8(second),
                Value::U32(nanosecond),
            ])
        }

        fn get_type() -> AnalysedType {
            analysed_type::record(vec![
                analysed_type::field("hours", analysed_type::u8()),
                analysed_type::field("minutes", analysed_type::u8()),
                analysed_type::field("seconds", analysed_type::u8()),
                analysed_type::field("nanoseconds", analysed_type::u32()),
            ])
        }
    }

    impl IntoValue for chrono::NaiveDateTime {
        fn into_value(self) -> Value {
            let date = self.date().into_value_and_type();
            let time = self.time().into_value_and_type();
            Value::Record(vec![date.value, time.value])
        }

        fn get_type() -> AnalysedType {
            analysed_type::record(vec![
                analysed_type::field("date", chrono::NaiveDate::get_type()),
                analysed_type::field("time", chrono::NaiveTime::get_type()),
            ])
        }
    }

    impl IntoValue for chrono::DateTime<chrono::Utc> {
        fn into_value(self) -> Value {
            let timestamp = self.naive_utc().into_value_and_type();
            let offset = self.offset().fix().local_minus_utc();
            Value::Record(vec![timestamp.value, Value::S32(offset)])
        }

        fn get_type() -> AnalysedType {
            analysed_type::record(vec![
                analysed_type::field("timestamp", chrono::NaiveDateTime::get_type()),
                analysed_type::field("offset", analysed_type::s32()),
            ])
        }
    }

    impl IntoValue for BitVec {
        fn into_value(self) -> Value {
            self.into_iter().collect::<Vec<_>>().into_value()
        }

        fn get_type() -> AnalysedType {
            list(bool())
        }
    }

    impl IntoValue for Url {
        fn into_value(self) -> Value {
            Value::String(self.to_string())
        }

        fn get_type() -> AnalysedType {
            str()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{IntoValue, RpcError, WitType, WitValue};
    use golem_wasm_ast::analysis::AnalysedType;
    use test_r::test;

    #[test]
    fn encoding_rpc_error_type() {
        let typ1 = RpcError::get_type();
        let encoded: WitType = typ1.clone().into();
        let typ2: AnalysedType = encoded.into();
        assert_eq!(typ1, typ2);
    }

    #[test]
    fn encoding_wit_value_type() {
        let typ1 = WitValue::get_type();
        let encoded: WitType = typ1.clone().into();
        let typ2: AnalysedType = encoded.into();
        assert_eq!(typ1, typ2);
    }
}
