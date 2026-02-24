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

use crate::{UuidRecord, Value, ValueAndType};
use bigdecimal::BigDecimal;
use std::str::FromStr;
use uuid::Uuid;

pub trait FromValue: Sized {
    fn from_value(value: Value) -> Result<Self, String>;
}

pub trait FromValueAndType: Sized {
    fn from_value_and_type(value_and_type: ValueAndType) -> Result<Self, String>;
}

impl<T: FromValue> FromValueAndType for T {
    fn from_value_and_type(value_and_type: ValueAndType) -> Result<Self, String> {
        Self::from_value(value_and_type.value)
    }
}

impl FromValue for u8 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U8(value) => Ok(value),
            _ => Err(format!("Expected u8 value, got {value:?}")),
        }
    }
}

impl FromValue for u16 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U16(value) => Ok(value),
            _ => Err(format!("Expected u16 value, got {value:?}")),
        }
    }
}

impl FromValue for u32 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U32(value) => Ok(value),
            _ => Err(format!("Expected u32 value, got {value:?}")),
        }
    }
}

impl FromValue for u64 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U64(value) => Ok(value),
            _ => Err(format!("Expected u64 value, got {value:?}")),
        }
    }
}

impl FromValue for usize {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U64(value) => Ok(value as usize),
            _ => Err(format!("Expected usize value, got {value:?}")),
        }
    }
}

impl FromValue for i8 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::S8(value) => Ok(value),
            _ => Err(format!("Expected i8 value, got {value:?}")),
        }
    }
}

impl FromValue for i16 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::S16(value) => Ok(value),
            _ => Err(format!("Expected i16 value, got {value:?}")),
        }
    }
}

impl FromValue for i32 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::S32(value) => Ok(value),
            _ => Err(format!("Expected i32 value, got {value:?}")),
        }
    }
}

impl FromValue for i64 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::S64(value) => Ok(value),
            _ => Err(format!("Expected i64 value, got {value:?}")),
        }
    }
}

impl FromValue for f32 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::F32(value) => Ok(value),
            _ => Err(format!("Expected f32 value, got {value:?}")),
        }
    }
}

impl FromValue for f64 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::F64(value) => Ok(value),
            _ => Err(format!("Expected f64 value, got {value:?}")),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Bool(value) => Ok(value),
            _ => Err(format!("Expected bool value, got {value:?}")),
        }
    }
}

impl FromValue for char {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Char(value) => Ok(value),
            _ => Err(format!("Expected char value, got {value:?}")),
        }
    }
}

impl FromValue for String {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::String(value) => Ok(value),
            _ => Err(format!("Expected String value, got {value:?}")),
        }
    }
}

impl<S: FromValue, E: FromValue> FromValue for Result<S, E> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Result(result) => match result {
                Ok(Some(ok_value)) => Ok(Ok(S::from_value(*ok_value)?)),
                Err(Some(err_value)) => Ok(Err(E::from_value(*err_value)?)),
                _ => Err(format!("Invalid Result value: {result:?}")),
            },
            _ => Err(format!("Expected Result value, got {value:?}")),
        }
    }
}

impl<E: FromValue> FromValue for Result<(), E> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Result(result) => match result {
                Ok(None) => Ok(Ok(())),
                Err(Some(err_value)) => Ok(Err(E::from_value(*err_value)?)),
                _ => Err(format!("Invalid Result<(), E> value: {result:?}")),
            },
            _ => Err(format!("Expected Result<(), E> value, got {value:?}")),
        }
    }
}

impl<S: FromValue> FromValue for Result<S, ()> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Result(result) => match result {
                Ok(Some(ok_value)) => Ok(Ok(S::from_value(*ok_value)?)),
                Err(None) => Ok(Err(())),
                _ => Err(format!("Invalid Result<S, ()> value: {result:?}")),
            },
            _ => Err(format!("Expected Result<S, ()> value, got {value:?}")),
        }
    }
}

impl FromValue for Result<(), ()> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Result(result) => match result {
                Ok(None) => Ok(Ok(())),
                Err(None) => Ok(Err(())),
                _ => Err(format!("Invalid Result<S, ()> value: {result:?}")),
            },
            _ => Err(format!("Expected Result<(), ()> value, got {value:?}")),
        }
    }
}

impl<T: FromValue> FromValue for Box<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        Ok(Box::new(T::from_value(value)?))
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Option(option) => match option {
                Some(inner_value) => Ok(Some(T::from_value(*inner_value)?)),
                None => Ok(None),
            },
            _ => Err(format!("Expected Option value, got {value:?}")),
        }
    }
}

impl<T: FromValue> FromValue for std::collections::Bound<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match case_idx {
                0 => Ok(std::collections::Bound::Included(T::from_value(
                    *case_value.unwrap(),
                )?)),
                1 => Ok(std::collections::Bound::Excluded(T::from_value(
                    *case_value.unwrap(),
                )?)),
                2 => Ok(std::collections::Bound::Unbounded),
                _ => Err(format!("Invalid Bound variant index: {case_idx}")),
            },
            _ => Err(format!("Expected Variant value for Bound, got {value:?}")),
        }
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::List(values) => {
                let mut result = Vec::with_capacity(values.len());
                for v in values {
                    result.push(T::from_value(v)?);
                }
                Ok(result)
            }
            _ => Err(format!("Expected List value, got {value:?}")),
        }
    }
}

macro_rules! impl_from_value_for_tuples {
    ($($ty:ident),*) => {
        impl<$($ty: FromValue),*> FromValue for ($($ty,)*) {
            fn from_value(value: Value) -> Result<Self, String> {
                const EXPECTED_LEN: usize = [$(stringify!($ty)),*].len();
                match value {
                    Value::Tuple(values) if values.len() == EXPECTED_LEN => {
                        let mut iter = values.into_iter();
                        Ok((
                            $(
                                $ty::from_value(iter.next().unwrap())?,
                            )*
                        ))
                    }
                    _ => Err(format!("Expected Tuple of {EXPECTED_LEN} elements, got {value:?}")),
                }
            }
        }
    };
}

impl_from_value_for_tuples!(A, B);
impl_from_value_for_tuples!(A, B, C);
impl_from_value_for_tuples!(A, B, C, D);
impl_from_value_for_tuples!(A, B, C, D, E);
impl_from_value_for_tuples!(A, B, C, D, E, F);
impl_from_value_for_tuples!(A, B, C, D, E, F, G);
impl_from_value_for_tuples!(A, B, C, D, E, F, G, H);
impl_from_value_for_tuples!(A, B, C, D, E, F, G, H, I);
impl_from_value_for_tuples!(A, B, C, D, E, F, G, H, I, J);
impl_from_value_for_tuples!(A, B, C, D, E, F, G, H, I, J, K);
impl_from_value_for_tuples!(A, B, C, D, E, F, G, H, I, J, K, L);

impl<K: FromValue + Eq + std::hash::Hash, V: FromValue> FromValue
    for std::collections::HashMap<K, V>
{
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::List(pairs) => {
                let mut map = std::collections::HashMap::new();
                for pair in pairs {
                    match pair {
                        Value::Tuple(mut values) if values.len() == 2 => {
                            let key = K::from_value(values.remove(0))?;
                            let val = V::from_value(values.remove(0))?;
                            map.insert(key, val);
                        }
                        _ => {
                            return Err(format!(
                                "Expected Tuple of 2 in HashMap list, got {pair:?}"
                            ))
                        }
                    }
                }
                Ok(map)
            }
            _ => Err(format!(
                "Expected List of tuples for HashMap, got {value:?}"
            )),
        }
    }
}

impl<K: FromValue + Ord, V: FromValue> FromValue for std::collections::BTreeMap<K, V> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::List(pairs) => {
                let mut map = std::collections::BTreeMap::new();
                for pair in pairs {
                    match pair {
                        Value::Tuple(mut values) if values.len() == 2 => {
                            let key = K::from_value(values.remove(0))?;
                            let val = V::from_value(values.remove(0))?;
                            map.insert(key, val);
                        }
                        _ => {
                            return Err(format!(
                                "Expected Tuple of 2 in BTreeMap list, got {pair:?}"
                            ))
                        }
                    }
                }
                Ok(map)
            }
            _ => Err(format!(
                "Expected List of tuples for BTreeMap, got {value:?}"
            )),
        }
    }
}

impl<T: FromValue + Ord> FromValue for std::collections::BTreeSet<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::List(values) => {
                let mut set = std::collections::BTreeSet::new();
                for v in values {
                    set.insert(T::from_value(v)?);
                }
                Ok(set)
            }
            _ => Err(format!("Expected List value for BTreeSet, got {value:?}")),
        }
    }
}

impl<T: FromValue + Eq + std::hash::Hash> FromValue for std::collections::HashSet<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::List(values) => {
                let mut set = std::collections::HashSet::new();
                for v in values {
                    set.insert(T::from_value(v)?);
                }
                Ok(set)
            }
            _ => Err(format!("Expected List value for HashSet, got {value:?}")),
        }
    }
}

impl FromValue for uuid::Uuid {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(fields) if fields.len() == 2 => {
                let mut iter = fields.into_iter();
                let hi = u64::from_value(iter.next().unwrap())?;
                let lo = u64::from_value(iter.next().unwrap())?;
                Ok(uuid::Uuid::from_u64_pair(hi, lo))
            }
            Value::String(s) => uuid::Uuid::parse_str(&s).map_err(|e| format!("Invalid UUID: {e}")),
            _ => Err(format!(
                "Expected Record with 2 fields for UUID, got {value:?}"
            )),
        }
    }
}

impl FromValue for UuidRecord {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 1 => match fields.remove(0) {
                Value::String(s) => {
                    let value = Uuid::parse_str(&s)
                        .map_err(|e| format!("Invalid UUID string in UuidRecord: {e}"))?;
                    Ok(UuidRecord { value })
                }
                other => Err(format!(
                    "Expected String value in UuidRecord, got {other:?}"
                )),
            },
            _ => Err(format!(
                "Expected Record with value for UuidRecord, got {value:?}"
            )),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::WitValue {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 1 => {
                let nodes = Vec::<crate::WitNode>::from_value(fields.remove(0))?;
                Ok(crate::WitValue { nodes })
            }
            _ => Err(format!(
                "Expected Record with nodes for WitValue, got {value:?}"
            )),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::WitNode {
    fn from_value(value: Value) -> Result<Self, String> {
        use crate::WitNode;
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => {
                let inner = *case_value.unwrap();

                match case_idx {
                    0 => Ok(WitNode::RecordValue(Vec::<crate::NodeIndex>::from_value(
                        inner,
                    )?)),
                    1 => match inner {
                        Value::Tuple(mut values) if values.len() == 2 => {
                            let idx = u32::from_value(values.remove(0))?;
                            let val = Option::<crate::NodeIndex>::from_value(values.remove(0))?;
                            Ok(WitNode::VariantValue((idx, val)))
                        }
                        _ => Err(format!("Expected Tuple for VariantValue, got {inner:?}")),
                    },
                    2 => Ok(WitNode::EnumValue(u32::from_value(inner)?)),
                    3 => Ok(WitNode::FlagsValue(Vec::<bool>::from_value(inner)?)),
                    4 => Ok(WitNode::TupleValue(Vec::<crate::NodeIndex>::from_value(
                        inner,
                    )?)),
                    5 => Ok(WitNode::ListValue(Vec::<crate::NodeIndex>::from_value(
                        inner,
                    )?)),
                    6 => Ok(WitNode::OptionValue(
                        Option::<crate::NodeIndex>::from_value(inner)?,
                    )),
                    7 => Ok(WitNode::ResultValue(
                        <Result<Option<i32>, Option<i32>> as FromValue>::from_value(inner)?,
                    )),
                    8 => Ok(WitNode::PrimU8(u8::from_value(inner)?)),
                    9 => Ok(WitNode::PrimU16(u16::from_value(inner)?)),
                    10 => Ok(WitNode::PrimU32(u32::from_value(inner)?)),
                    11 => Ok(WitNode::PrimU64(u64::from_value(inner)?)),
                    12 => Ok(WitNode::PrimS8(i8::from_value(inner)?)),
                    13 => Ok(WitNode::PrimS16(i16::from_value(inner)?)),
                    14 => Ok(WitNode::PrimS32(i32::from_value(inner)?)),
                    15 => Ok(WitNode::PrimS64(i64::from_value(inner)?)),
                    16 => Ok(WitNode::PrimFloat32(f32::from_value(inner)?)),
                    17 => Ok(WitNode::PrimFloat64(f64::from_value(inner)?)),
                    18 => Ok(WitNode::PrimChar(char::from_value(inner)?)),
                    19 => Ok(WitNode::PrimBool(bool::from_value(inner)?)),
                    20 => Ok(WitNode::PrimString(String::from_value(inner)?)),
                    21 => match inner {
                        Value::Tuple(mut values) if values.len() == 2 => {
                            let uri = crate::Uri::from_value(values.remove(0))?;
                            let resource_id = u64::from_value(values.remove(0))?;
                            Ok(WitNode::Handle((uri, resource_id)))
                        }
                        _ => Err(format!("Expected Tuple for Handle, got {inner:?}")),
                    },
                    _ => Err(format!("Invalid WitNode variant index: {case_idx}")),
                }
            }
            _ => Err(format!("Expected Variant for WitNode, got {value:?}")),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::Uri {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 1 => {
                let value = String::from_value(fields.remove(0))?;
                Ok(crate::Uri { value })
            }
            _ => Err(format!("Expected Record with value for Uri, got {value:?}")),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::WitTypeNode {
    fn from_value(value: Value) -> Result<Self, String> {
        use crate::WitTypeNode;
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match case_idx {
                0 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::RecordType(
                        Vec::<(String, crate::NodeIndex)>::from_value(inner)?,
                    ))
                }
                1 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::VariantType(Vec::<(
                        String,
                        Option<crate::NodeIndex>,
                    )>::from_value(
                        inner
                    )?))
                }
                2 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::EnumType(Vec::<String>::from_value(inner)?))
                }
                3 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::FlagsType(Vec::<String>::from_value(inner)?))
                }
                4 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::TupleType(Vec::<crate::NodeIndex>::from_value(
                        inner,
                    )?))
                }
                5 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::ListType(crate::NodeIndex::from_value(inner)?))
                }
                6 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    Ok(WitTypeNode::OptionType(crate::NodeIndex::from_value(
                        inner,
                    )?))
                }
                7 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    match inner {
                        Value::Tuple(mut values) if values.len() == 2 => {
                            let ok = Option::<crate::NodeIndex>::from_value(values.remove(0))?;
                            let err = Option::<crate::NodeIndex>::from_value(values.remove(0))?;
                            Ok(WitTypeNode::ResultType((ok, err)))
                        }
                        _ => Err(format!("Expected Tuple for ResultType, got {inner:?}")),
                    }
                }
                8 => Ok(WitTypeNode::PrimU8Type),
                9 => Ok(WitTypeNode::PrimU16Type),
                10 => Ok(WitTypeNode::PrimU32Type),
                11 => Ok(WitTypeNode::PrimU64Type),
                12 => Ok(WitTypeNode::PrimS8Type),
                13 => Ok(WitTypeNode::PrimS16Type),
                14 => Ok(WitTypeNode::PrimS32Type),
                15 => Ok(WitTypeNode::PrimS64Type),
                16 => Ok(WitTypeNode::PrimF32Type),
                17 => Ok(WitTypeNode::PrimF64Type),
                18 => Ok(WitTypeNode::PrimCharType),
                19 => Ok(WitTypeNode::PrimBoolType),
                20 => Ok(WitTypeNode::PrimStringType),
                21 => {
                    let inner = match case_value {
                        Some(inner) => *inner,
                        None => {
                            return Err(format!("Expected case_value for case_idx {}", case_idx))
                        }
                    };
                    match inner {
                        Value::Tuple(mut values) if values.len() == 2 => {
                            let id = u64::from_value(values.remove(0))?;
                            let mode = crate::ResourceMode::from_value(values.remove(0))?;
                            Ok(WitTypeNode::HandleType((id, mode)))
                        }
                        _ => Err(format!("Expected Tuple for HandleType, got {inner:?}")),
                    }
                }
                _ => Err(format!("Invalid WitTypeNode variant index: {case_idx}")),
            },
            _ => Err(format!("Expected Variant for WitTypeNode, got {value:?}")),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::golem_core_1_5_x::types::NamedWitTypeNode {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 3 => {
                let name = Option::<String>::from_value(fields.remove(0))?;
                let owner = Option::<String>::from_value(fields.remove(0))?;
                let type_ = crate::WitTypeNode::from_value(fields.remove(0))?;
                Ok(crate::golem_core_1_5_x::types::NamedWitTypeNode { name, owner, type_ })
            }
            _ => Err(format!(
                "Expected Record for NamedWitTypeNode, got {value:?}"
            )),
        }
    }
}

impl FromValue for std::time::Instant {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U64(nanos) => {
                Ok(std::time::Instant::now() - std::time::Duration::from_nanos(nanos))
            }
            _ => Err(format!("Expected U64 for Instant, got {value:?}")),
        }
    }
}

impl FromValue for std::time::Duration {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U64(nanos) => Ok(std::time::Duration::from_nanos(nanos)),
            _ => Err(format!("Expected U64 for Duration, got {value:?}")),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::ResourceMode {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Enum(idx) => match idx {
                0 => Ok(crate::ResourceMode::Owned),
                1 => Ok(crate::ResourceMode::Borrowed),
                _ => Err(format!("Invalid ResourceMode enum index: {idx}")),
            },
            _ => Err(format!("Expected Enum for ResourceMode, got {value:?}")),
        }
    }
}

#[cfg(feature = "host")]
impl FromValue for Value {
    fn from_value(value: Value) -> Result<Self, String> {
        let wit_value = crate::WitValue::from_value(value)?;
        Ok(wit_value.into())
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::analysis::AnalysedType {
    fn from_value(value: Value) -> Result<Self, String> {
        let wit_type: crate::WitType = crate::WitType::from_value(value)?;
        Ok(wit_type.into())
    }
}

#[cfg(feature = "host")]
impl FromValue for ValueAndType {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 2 => {
                let value = Value::from_value(fields.remove(0))?;
                let typ = crate::analysis::AnalysedType::from_value(fields.remove(0))?;
                Ok(ValueAndType { value, typ })
            }
            _ => Err(format!(
                "Expected Record with value and type for ValueAndType, got {value:?}"
            )),
        }
    }
}

impl FromValue for bigdecimal::BigDecimal {
    fn from_value(value: Value) -> Result<Self, String> {
        let s = String::from_value(value)?;
        BigDecimal::from_str(&s).map_err(|e| format!("Invalid BigDecimal: {e}"))
    }
}

impl FromValue for chrono::NaiveDate {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 3 => {
                let year = i32::from_value(fields.remove(0))?;
                let month = u8::from_value(fields.remove(0))?;
                let day = u8::from_value(fields.remove(0))?;
                chrono::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
                    .ok_or_else(|| format!("Invalid date: {year}-{month}-{day}"))
            }
            _ => Err(format!("Expected Record for NaiveDate, got {value:?}")),
        }
    }
}

impl FromValue for chrono::NaiveTime {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 4 => {
                let hour = u8::from_value(fields.remove(0))?;
                let minute = u8::from_value(fields.remove(0))?;
                let second = u8::from_value(fields.remove(0))?;
                let nanosecond = u32::from_value(fields.remove(0))?;
                chrono::NaiveTime::from_hms_nano_opt(
                    hour as u32,
                    minute as u32,
                    second as u32,
                    nanosecond,
                )
                .ok_or_else(|| format!("Invalid time: {hour}:{minute}:{second}.{nanosecond}"))
            }
            _ => Err(format!("Expected Record for NaiveTime, got {value:?}")),
        }
    }
}

impl FromValue for chrono::NaiveDateTime {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 2 => {
                let date = chrono::NaiveDate::from_value(fields.remove(0))?;
                let time = chrono::NaiveTime::from_value(fields.remove(0))?;
                Ok(chrono::NaiveDateTime::new(date, time))
            }
            _ => Err(format!("Expected Record for NaiveDateTime, got {value:?}")),
        }
    }
}

impl FromValue for chrono::DateTime<chrono::Utc> {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 2 => {
                let timestamp = chrono::NaiveDateTime::from_value(fields.remove(0))?;
                let _offset_seconds = i32::from_value(fields.remove(0))?; // Ignored for Utc
                Ok(chrono::DateTime::from_naive_utc_and_offset(
                    timestamp,
                    chrono::Utc,
                ))
            }
            _ => Err(format!("Expected Record for DateTime<Utc>, got {value:?}")),
        }
    }
}

impl FromValue for bit_vec::BitVec {
    fn from_value(value: Value) -> Result<Self, String> {
        let bits = Vec::<bool>::from_value(value)?;
        Ok(bit_vec::BitVec::from_iter(bits))
    }
}

impl FromValue for url::Url {
    fn from_value(value: Value) -> Result<Self, String> {
        let s = String::from_value(value)?;
        url::Url::parse(&s).map_err(|e| format!("Invalid URL: {e}"))
    }
}

#[cfg(feature = "host")]
impl FromValue for crate::WitType {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Record(mut fields) if fields.len() == 1 => {
                let nodes = Vec::<crate::golem_core_1_5_x::types::NamedWitTypeNode>::from_value(
                    fields.remove(0),
                )?;
                Ok(crate::WitType { nodes })
            }
            _ => Err(format!(
                "Expected Record with nodes for WitType, got {value:?}"
            )),
        }
    }
}
