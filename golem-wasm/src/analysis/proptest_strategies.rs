// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Proptest strategies for generating arbitrary `AnalysedType` + `Value` pairs.

use crate::analysis::analysed_type::{
    bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result, result_err,
    result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case, unit_result, variant,
};
use crate::analysis::AnalysedType;
use crate::Value;
use proptest::prelude::*;

/// Generate a finite f32 value (no NaN/Infinity).
pub fn finite_f32() -> BoxedStrategy<f32> {
    prop_oneof![proptest::num::f32::NORMAL, Just(0.0f32), Just(-0.0f32),].boxed()
}

/// Generate a finite f64 value (no NaN/Infinity).
pub fn finite_f64() -> BoxedStrategy<f64> {
    prop_oneof![proptest::num::f64::NORMAL, Just(0.0f64), Just(-0.0f64),].boxed()
}

/// Generate a valid char (proptest's default char strategy).
pub fn arb_char() -> impl Strategy<Value = char> {
    proptest::char::any()
}

/// Generate an arbitrary string (may contain control chars, unicode, etc.).
pub fn arb_string() -> impl Strategy<Value = String> {
    ".*"
}

/// Leaf `AnalysedType` + matching `Value` (no recursion).
pub fn leaf_type_and_value() -> impl Strategy<Value = (AnalysedType, Value)> {
    prop_oneof![
        any::<bool>().prop_map(|b| (bool(), Value::Bool(b))),
        any::<u8>().prop_map(|v| (u8(), Value::U8(v))),
        any::<u16>().prop_map(|v| (u16(), Value::U16(v))),
        any::<u32>().prop_map(|v| (u32(), Value::U32(v))),
        any::<u64>().prop_map(|v| (u64(), Value::U64(v))),
        any::<i8>().prop_map(|v| (s8(), Value::S8(v))),
        any::<i16>().prop_map(|v| (s16(), Value::S16(v))),
        any::<i32>().prop_map(|v| (s32(), Value::S32(v))),
        any::<i64>().prop_map(|v| (s64(), Value::S64(v))),
        finite_f32().prop_map(|v| (f32(), Value::F32(v))),
        finite_f64().prop_map(|v| (f64(), Value::F64(v))),
        arb_char().prop_map(|c| (chr(), Value::Char(c))),
        arb_string().prop_map(|s| (str(), Value::String(s))),
    ]
}

/// Generate a `Value` matching a specific leaf `AnalysedType`.
pub fn arb_leaf_value_for_type(typ: AnalysedType) -> BoxedStrategy<Value> {
    match typ {
        AnalysedType::Bool(_) => any::<bool>().prop_map(Value::Bool).boxed(),
        AnalysedType::U8(_) => any::<u8>().prop_map(Value::U8).boxed(),
        AnalysedType::U16(_) => any::<u16>().prop_map(Value::U16).boxed(),
        AnalysedType::U32(_) => any::<u32>().prop_map(Value::U32).boxed(),
        AnalysedType::U64(_) => any::<u64>().prop_map(Value::U64).boxed(),
        AnalysedType::S8(_) => any::<i8>().prop_map(Value::S8).boxed(),
        AnalysedType::S16(_) => any::<i16>().prop_map(Value::S16).boxed(),
        AnalysedType::S32(_) => any::<i32>().prop_map(Value::S32).boxed(),
        AnalysedType::S64(_) => any::<i64>().prop_map(Value::S64).boxed(),
        AnalysedType::F32(_) => finite_f32().prop_map(Value::F32).boxed(),
        AnalysedType::F64(_) => finite_f64().prop_map(Value::F64).boxed(),
        AnalysedType::Chr(_) => arb_char().prop_map(Value::Char).boxed(),
        AnalysedType::Str(_) => arb_string().prop_map(Value::String).boxed(),
        other => unreachable!("arb_leaf_value_for_type called with non-leaf type: {other:?}"),
    }
}

/// Recursive `AnalysedType` + matching `Value`, up to a bounded depth/size.
pub fn arb_type_and_value() -> impl Strategy<Value = (AnalysedType, Value)> {
    leaf_type_and_value().prop_recursive(
        4,  // depth
        64, // desired size
        8,  // items per collection
        |inner| {
            prop_oneof![
                // Option: Some
                inner
                    .clone()
                    .prop_map(|(t, v)| (option(t), Value::Option(Some(Box::new(v))))),
                // Option: None — pick a random inner type but produce None
                inner
                    .clone()
                    .prop_map(|(t, _)| (option(t), Value::Option(None))),
                // List (all items same type — use leaf for uniformity)
                (0..5usize, leaf_type_and_value())
                    .prop_flat_map(|(len, (item_type, _))| {
                        let gen = arb_leaf_value_for_type(item_type.clone());
                        (Just(item_type), prop::collection::vec(gen, len..=len))
                    })
                    .prop_map(|(item_type, values)| { (list(item_type), Value::List(values)) }),
                // Record (1-4 fields)
                prop::collection::vec(inner.clone(), 1..5).prop_map(|fields| {
                    let field_types: Vec<_> = fields
                        .iter()
                        .enumerate()
                        .map(|(i, (t, _))| field(&format!("f{i}"), t.clone()))
                        .collect();
                    let field_values: Vec<_> = fields.into_iter().map(|(_, v)| v).collect();
                    (record(field_types), Value::Record(field_values))
                }),
                // Tuple (1-4 items)
                prop::collection::vec(inner.clone(), 1..5).prop_map(|items| {
                    let item_types: Vec<_> = items.iter().map(|(t, _)| t.clone()).collect();
                    let item_values: Vec<_> = items.into_iter().map(|(_, v)| v).collect();
                    (tuple(item_types), Value::Tuple(item_values))
                }),
                // Variant with payload (2-4 cases, pick one)
                (1..4usize, inner.clone()).prop_flat_map(
                    |(num_cases, (payload_type, payload_val))| {
                        (
                            0..num_cases,
                            Just(num_cases),
                            Just(payload_type),
                            Just(payload_val),
                        )
                            .prop_map(|(chosen, num_cases, pt, pv)| {
                                let cases: Vec<_> = (0..num_cases)
                                    .map(|i| {
                                        if i == chosen {
                                            case(&format!("c{i}"), pt.clone())
                                        } else {
                                            unit_case(&format!("c{i}"))
                                        }
                                    })
                                    .collect();
                                (
                                    variant(cases),
                                    Value::Variant {
                                        case_idx: chosen as u32,
                                        case_value: Some(Box::new(pv.clone())),
                                    },
                                )
                            })
                    }
                ),
                // Variant without payload
                (2..5usize).prop_flat_map(|num_cases| {
                    (0..num_cases, Just(num_cases)).prop_map(|(chosen, num_cases)| {
                        let cases: Vec<_> = (0..num_cases)
                            .map(|i| unit_case(&format!("c{i}")))
                            .collect();
                        (
                            variant(cases),
                            Value::Variant {
                                case_idx: chosen as u32,
                                case_value: None,
                            },
                        )
                    })
                }),
                // Enum (2-5 cases)
                (2..6usize).prop_flat_map(|num_cases| {
                    (0..num_cases, Just(num_cases)).prop_map(|(chosen, num_cases)| {
                        let case_names: Vec<String> =
                            (0..num_cases).map(|i| format!("e{i}")).collect();
                        let case_refs: Vec<&str> = case_names.iter().map(|s| s.as_str()).collect();
                        (r#enum(&case_refs), Value::Enum(chosen as u32))
                    })
                }),
                // Flags (1-8 flags)
                (1..9usize).prop_flat_map(|num_flags| {
                    prop::collection::vec(any::<bool>(), num_flags..=num_flags).prop_map(
                        move |bits| {
                            let flag_names: Vec<String> =
                                (0..num_flags).map(|i| format!("fl{i}")).collect();
                            let flag_refs: Vec<&str> =
                                flag_names.iter().map(|s| s.as_str()).collect();
                            (flags(&flag_refs), Value::Flags(bits))
                        },
                    )
                }),
                // Result ok with payload
                (inner.clone(), inner.clone()).prop_map(|((ok_t, ok_v), (err_t, _))| {
                    (result(ok_t, err_t), Value::Result(Ok(Some(Box::new(ok_v)))))
                }),
                // Result err with payload
                (inner.clone(), inner.clone()).prop_map(|((ok_t, _), (err_t, err_v))| {
                    (
                        result(ok_t, err_t),
                        Value::Result(Err(Some(Box::new(err_v)))),
                    )
                }),
                // Result ok unit
                inner
                    .clone()
                    .prop_map(|(err_t, _)| (result_err(err_t), Value::Result(Ok(None)))),
                // Result err unit
                inner
                    .clone()
                    .prop_map(|(ok_t, _)| (result_ok(ok_t), Value::Result(Err(None)))),
                // Result both unit
                Just((unit_result(), Value::Result(Ok(None)))),
            ]
        },
    )
}
