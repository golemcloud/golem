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

use test_r::test;

use super::*;
use super::SourceLanguage;
use golem_common::model::agent::{
    ComponentModelElementSchema, ComponentModelElementValue, DataSchema, DataValue, ElementSchema,
    ElementValue, ElementValues, NamedElementSchema, NamedElementSchemas,
};
use golem_wasm::analysis::{
    AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF32, TypeF64,
    TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult, TypeS8, TypeS16, TypeS32, TypeS64,
    TypeStr, TypeTuple, TypeU8, TypeU16, TypeU32, TypeU64, TypeVariant,
};
use golem_wasm::{Value, ValueAndType};

fn cm_schema(typ: AnalysedType) -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "p".to_string(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: typ,
            }),
        }],
    })
}

fn cm_value(value: Value, typ: AnalysedType) -> DataValue {
    DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(value, typ),
        })],
    })
}

fn round_trip_rust(value: Value, typ: AnalysedType) {
    let data_value = cm_value(value.clone(), typ.clone());
    let schema = cm_schema(typ.clone());
    let rendered = render_data_value(&data_value, &SourceLanguage::Rust);
    let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::Rust)
        .unwrap_or_else(|e| panic!("parse failed for rendered='{rendered}': {e}"));
    assert_eq!(
        data_value, parsed,
        "round-trip failed for rendered='{rendered}'"
    );
}

fn round_trip_ts(value: Value, typ: AnalysedType) {
    let data_value = cm_value(value.clone(), typ.clone());
    let schema = cm_schema(typ.clone());
    let rendered = render_data_value(&data_value, &SourceLanguage::TypeScript);
    let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::TypeScript)
        .unwrap_or_else(|e| panic!("parse failed for rendered='{rendered}': {e}"));
    assert_eq!(
        data_value, parsed,
        "round-trip failed for rendered='{rendered}'"
    );
}

// Primitive round-trips

#[test]
fn rust_round_trip_bool() {
    round_trip_rust(Value::Bool(true), AnalysedType::Bool(TypeBool));
    round_trip_rust(Value::Bool(false), AnalysedType::Bool(TypeBool));
}

#[test]
fn ts_round_trip_bool() {
    round_trip_ts(Value::Bool(true), AnalysedType::Bool(TypeBool));
    round_trip_ts(Value::Bool(false), AnalysedType::Bool(TypeBool));
}

#[test]
fn rust_round_trip_integers() {
    round_trip_rust(Value::U32(42), AnalysedType::U32(TypeU32));
    round_trip_rust(Value::S32(-7), AnalysedType::S32(TypeS32));
    round_trip_rust(Value::S32(0), AnalysedType::S32(TypeS32));
}

#[test]
fn ts_round_trip_integers() {
    round_trip_ts(Value::U32(42), AnalysedType::U32(TypeU32));
    round_trip_ts(Value::S32(-7), AnalysedType::S32(TypeS32));
}

#[test]
fn rust_round_trip_string() {
    round_trip_rust(
        Value::String("hello world".into()),
        AnalysedType::Str(TypeStr),
    );
    round_trip_rust(
        Value::String("line\nnewline".into()),
        AnalysedType::Str(TypeStr),
    );
    round_trip_rust(
        Value::String("has \"quotes\"".into()),
        AnalysedType::Str(TypeStr),
    );
}

#[test]
fn ts_round_trip_string() {
    round_trip_ts(
        Value::String("hello world".into()),
        AnalysedType::Str(TypeStr),
    );
    round_trip_ts(
        Value::String("line\nnewline".into()),
        AnalysedType::Str(TypeStr),
    );
}

#[test]
fn rust_round_trip_char() {
    round_trip_rust(Value::Char('a'), AnalysedType::Chr(TypeChr));
    round_trip_rust(Value::Char('\n'), AnalysedType::Chr(TypeChr));
}

#[test]
fn ts_round_trip_char() {
    round_trip_ts(Value::Char('a'), AnalysedType::Chr(TypeChr));
}

#[test]
fn rust_round_trip_float() {
    round_trip_rust(Value::F64(3.14), AnalysedType::F64(TypeF64));
    round_trip_rust(Value::F64(f64::INFINITY), AnalysedType::F64(TypeF64));
    round_trip_rust(Value::F64(f64::NEG_INFINITY), AnalysedType::F64(TypeF64));
    // NaN needs special handling — can't use equality
    let data = cm_value(Value::F64(f64::NAN), AnalysedType::F64(TypeF64));
    let schema = cm_schema(AnalysedType::F64(TypeF64));
    let rendered = render_data_value(&data, &SourceLanguage::Rust);
    let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::Rust).unwrap();
    // Check the parsed value is NaN
    match &parsed {
        DataValue::Tuple(elems) => match &elems.elements[0] {
            ElementValue::ComponentModel(cm) => match &cm.value.value {
                Value::F64(v) => assert!(v.is_nan(), "expected NaN"),
                _ => panic!("expected F64"),
            },
            _ => panic!("expected CM"),
        },
        _ => panic!("expected Tuple"),
    }
}

#[test]
fn ts_round_trip_float() {
    round_trip_ts(Value::F64(3.14), AnalysedType::F64(TypeF64));
    round_trip_ts(Value::F64(f64::INFINITY), AnalysedType::F64(TypeF64));
    round_trip_ts(Value::F64(f64::NEG_INFINITY), AnalysedType::F64(TypeF64));
}

// Composite types

#[test]
fn rust_round_trip_record() {
    let typ = AnalysedType::Record(TypeRecord {
        name: Some("my-record".to_string()),
        owner: None,
        fields: vec![
            NameTypePair {
                name: "field-one".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "field-two".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    });
    let val = Value::Record(vec![Value::U32(42), Value::String("hi".into())]);
    round_trip_rust(val, typ);
}

#[test]
fn ts_round_trip_record() {
    let typ = AnalysedType::Record(TypeRecord {
        name: Some("my-record".to_string()),
        owner: None,
        fields: vec![
            NameTypePair {
                name: "field-one".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "field-two".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    });
    let val = Value::Record(vec![Value::U32(42), Value::String("hi".into())]);
    round_trip_ts(val, typ);
}

#[test]
fn rust_round_trip_variant() {
    let typ = AnalysedType::Variant(TypeVariant {
        name: Some("my-variant".to_string()),
        owner: None,
        cases: vec![
            NameOptionTypePair {
                name: "case-a".to_string(),
                typ: Some(AnalysedType::U32(TypeU32)),
            },
            NameOptionTypePair {
                name: "case-b".to_string(),
                typ: None,
            },
        ],
    });
    // With payload
    round_trip_rust(
        Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::U32(99))),
        },
        typ.clone(),
    );
    // Without payload
    round_trip_rust(
        Value::Variant {
            case_idx: 1,
            case_value: None,
        },
        typ,
    );
}

#[test]
fn ts_round_trip_variant() {
    let typ = AnalysedType::Variant(TypeVariant {
        name: Some("my-variant".to_string()),
        owner: None,
        cases: vec![
            NameOptionTypePair {
                name: "case-a".to_string(),
                typ: Some(AnalysedType::U32(TypeU32)),
            },
            NameOptionTypePair {
                name: "case-b".to_string(),
                typ: None,
            },
        ],
    });
    round_trip_ts(
        Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::U32(99))),
        },
        typ.clone(),
    );
    round_trip_ts(
        Value::Variant {
            case_idx: 1,
            case_value: None,
        },
        typ,
    );
}

#[test]
fn rust_round_trip_enum() {
    let typ = AnalysedType::Enum(TypeEnum {
        name: Some("color".to_string()),
        owner: None,
        cases: vec![
            "red".to_string(),
            "green".to_string(),
            "blue".to_string(),
        ],
    });
    round_trip_rust(Value::Enum(1), typ);
}

#[test]
fn ts_round_trip_enum() {
    let typ = AnalysedType::Enum(TypeEnum {
        name: None,
        owner: None,
        cases: vec![
            "red".to_string(),
            "green".to_string(),
            "blue".to_string(),
        ],
    });
    round_trip_ts(Value::Enum(0), typ);
}

#[test]
fn rust_round_trip_option() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    round_trip_rust(
        Value::Option(Some(Box::new(Value::U32(42)))),
        typ.clone(),
    );
    round_trip_rust(Value::Option(None), typ);
}

#[test]
fn ts_round_trip_option() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    round_trip_ts(
        Value::Option(Some(Box::new(Value::U32(42)))),
        typ.clone(),
    );
    round_trip_ts(Value::Option(None), typ);
}

#[test]
fn rust_round_trip_result() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    round_trip_rust(
        Value::Result(Ok(Some(Box::new(Value::U32(42))))),
        typ.clone(),
    );
    round_trip_rust(
        Value::Result(Err(Some(Box::new(Value::String("oops".into()))))),
        typ,
    );
}

#[test]
fn ts_round_trip_result() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    round_trip_ts(
        Value::Result(Ok(Some(Box::new(Value::U32(42))))),
        typ.clone(),
    );
    round_trip_ts(
        Value::Result(Err(Some(Box::new(Value::String("oops".into()))))),
        typ,
    );
}

#[test]
fn rust_round_trip_result_unit_ok() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: None,
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    round_trip_rust(Value::Result(Ok(None)), typ);
}

#[test]
fn ts_round_trip_result_unit_ok() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: None,
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    round_trip_ts(Value::Result(Ok(None)), typ);
}

#[test]
fn rust_round_trip_flags() {
    let typ = AnalysedType::Flags(TypeFlags {
        name: Some("perms".to_string()),
        owner: None,
        names: vec![
            "read".to_string(),
            "write".to_string(),
            "execute".to_string(),
        ],
    });
    round_trip_rust(Value::Flags(vec![true, false, true]), typ);
}

#[test]
fn ts_round_trip_flags() {
    let typ = AnalysedType::Flags(TypeFlags {
        name: None,
        owner: None,
        names: vec![
            "read".to_string(),
            "write".to_string(),
            "execute".to_string(),
        ],
    });
    round_trip_ts(Value::Flags(vec![true, false, true]), typ);
}

#[test]
fn rust_round_trip_tuple() {
    let typ = AnalysedType::Tuple(TypeTuple {
        name: None,
        owner: None,
        items: vec![AnalysedType::U32(TypeU32), AnalysedType::Str(TypeStr)],
    });
    round_trip_rust(
        Value::Tuple(vec![Value::U32(1), Value::String("x".into())]),
        typ,
    );
}

#[test]
fn ts_round_trip_tuple() {
    let typ = AnalysedType::Tuple(TypeTuple {
        name: None,
        owner: None,
        items: vec![AnalysedType::U32(TypeU32), AnalysedType::Str(TypeStr)],
    });
    round_trip_ts(
        Value::Tuple(vec![Value::U32(1), Value::String("x".into())]),
        typ,
    );
}

#[test]
fn rust_round_trip_list() {
    let typ = AnalysedType::List(TypeList {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    round_trip_rust(
        Value::List(vec![Value::U32(1), Value::U32(2), Value::U32(3)]),
        typ,
    );
}

#[test]
fn ts_round_trip_list() {
    let typ = AnalysedType::List(TypeList {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    round_trip_ts(
        Value::List(vec![Value::U32(1), Value::U32(2), Value::U32(3)]),
        typ,
    );
}

// Canonical structural fallback

#[test]
fn canonical_fallback_always_accepted() {
    // Language-aware parse should accept canonical structural form
    let typ = AnalysedType::Record(TypeRecord {
        name: Some("test-record".to_string()),
        owner: None,
        fields: vec![NameTypePair {
            name: "field-a".to_string(),
            typ: AnalysedType::U32(TypeU32),
        }],
    });
    let val = Value::Record(vec![Value::U32(42)]);
    let data_value = cm_value(val, typ.clone());
    let schema = cm_schema(typ);
    // Render in canonical structural form
    let structural =
        golem_common::model::agent::structural_format::format_structural(&data_value).unwrap();
    // Parse with Rust language — should still accept canonical form
    let parsed = parse_agent_id_params(&structural, &schema, &SourceLanguage::Rust).unwrap();
    assert_eq!(data_value, parsed);
}

// render_agent_id tests

#[test]
fn render_agent_id_format() {
    use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
    let data = cm_value(Value::U32(42), AnalysedType::U32(TypeU32));
    let parsed = ParsedAgentId::new(AgentTypeName("my-agent".to_string()), data, None).unwrap();
    let result = render_agent_id(&parsed, &SourceLanguage::Rust);
    assert_eq!(result, "my-agent(42)");
}

#[test]
fn render_agent_id_with_phantom() {
    use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
    use uuid::Uuid;
    let data = cm_value(Value::U32(42), AnalysedType::U32(TypeU32));
    let uuid = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let parsed =
        ParsedAgentId::new(AgentTypeName("my-agent".to_string()), data, Some(uuid)).unwrap();
    let result = render_agent_id(&parsed, &SourceLanguage::Rust);
    assert_eq!(
        result,
        "my-agent(42)[12345678-1234-1234-1234-123456789012]"
    );
}

// ── Property-based roundtrip tests ──────────────────────────────────────────

use proptest::prelude::*;

    // ── Strategies ──────────────────────────────────────────────────────

    /// Generate a finite f32 (no NaN/Infinity for roundtrip equality).
    fn finite_f32() -> BoxedStrategy<f32> {
        prop_oneof![proptest::num::f32::NORMAL, Just(0.0f32), Just(-0.0f32),].boxed()
    }

    /// Generate a finite f64.
    fn finite_f64() -> BoxedStrategy<f64> {
        prop_oneof![proptest::num::f64::NORMAL, Just(0.0f64), Just(-0.0f64),].boxed()
    }

    /// Valid identifier name (used for field/case/flag names).
    fn arb_ident_name() -> BoxedStrategy<String> {
        "[a-z][a-z0-9_]{0,7}".prop_map(|s| s.to_string()).boxed()
    }

    /// Generate a string safe for round-tripping through both Rust and TS escaping.
    fn arb_safe_string() -> BoxedStrategy<String> {
        prop_oneof![
            Just(String::new()),
            "[a-zA-Z0-9 !@#%^&*()_+=<>?]{0,20}",
            "[ -~]{0,30}", // printable ASCII
        ]
        .boxed()
    }

    /// Generate a char safe for round-tripping (avoid surrogates, use printable + common control).
    fn arb_safe_char() -> BoxedStrategy<char> {
        prop_oneof![
            prop::char::range('a', 'z'),
            prop::char::range('A', 'Z'),
            prop::char::range('0', '9'),
            Just('\n'),
            Just('\t'),
            Just('\\'),
            Just('"'),
            Just('\''),
        ]
        .boxed()
    }

    /// Leaf type + matching value (no recursion).
    fn leaf_type_and_value() -> BoxedStrategy<(AnalysedType, Value)> {
        prop_oneof![
            any::<bool>().prop_map(|b| (AnalysedType::Bool(TypeBool), Value::Bool(b))),
            any::<u8>().prop_map(|v| (AnalysedType::U8(TypeU8), Value::U8(v))),
            any::<u16>().prop_map(|v| (AnalysedType::U16(TypeU16), Value::U16(v))),
            any::<u32>().prop_map(|v| (AnalysedType::U32(TypeU32), Value::U32(v))),
            any::<u64>().prop_map(|v| (AnalysedType::U64(TypeU64), Value::U64(v))),
            any::<i8>().prop_map(|v| (AnalysedType::S8(TypeS8), Value::S8(v))),
            any::<i16>().prop_map(|v| (AnalysedType::S16(TypeS16), Value::S16(v))),
            any::<i32>().prop_map(|v| (AnalysedType::S32(TypeS32), Value::S32(v))),
            any::<i64>().prop_map(|v| (AnalysedType::S64(TypeS64), Value::S64(v))),
            finite_f32().prop_map(|v| (AnalysedType::F32(TypeF32), Value::F32(v))),
            finite_f64().prop_map(|v| (AnalysedType::F64(TypeF64), Value::F64(v))),
            arb_safe_char().prop_map(|c| (AnalysedType::Chr(TypeChr), Value::Char(c))),
            arb_safe_string().prop_map(|s| (AnalysedType::Str(TypeStr), Value::String(s))),
        ]
        .boxed()
    }

    /// Generate a value matching a specific leaf AnalysedType.
    fn arb_leaf_value_for_type(typ: &AnalysedType) -> BoxedStrategy<Value> {
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
            AnalysedType::Chr(_) => arb_safe_char().prop_map(Value::Char).boxed(),
            AnalysedType::Str(_) => arb_safe_string().prop_map(Value::String).boxed(),
            _ => any::<u32>().prop_map(Value::U32).boxed(),
        }
    }

    /// Recursive AnalysedType + matching Value.
    fn arb_type_and_value() -> BoxedStrategy<(AnalysedType, Value)> {
        leaf_type_and_value()
            .prop_recursive(
                3,  // depth
                32, // desired size
                6,  // items per collection
                |inner| {
                    prop_oneof![
                        // Option Some
                        inner.clone().prop_map(|(t, v)| {
                            (
                                AnalysedType::Option(TypeOption {
                                    name: None,
                                    owner: None,
                                    inner: Box::new(t),
                                }),
                                Value::Option(Some(Box::new(v))),
                            )
                        }),
                        // Option None
                        inner.clone().prop_map(|(t, _)| {
                            (
                                AnalysedType::Option(TypeOption {
                                    name: None,
                                    owner: None,
                                    inner: Box::new(t),
                                }),
                                Value::Option(None),
                            )
                        }),
                        // List
                        (0..4usize, leaf_type_and_value())
                            .prop_flat_map(|(len, (item_type, _))| {
                                let gen = arb_leaf_value_for_type(&item_type);
                                (Just(item_type), prop::collection::vec(gen, len..=len))
                            })
                            .prop_map(|(item_type, values)| {
                                (
                                    AnalysedType::List(TypeList {
                                        name: None,
                                        owner: None,
                                        inner: Box::new(item_type),
                                    }),
                                    Value::List(values),
                                )
                            }),
                        // Record (1-3 fields)
                        prop::collection::vec(inner.clone(), 1..4)
                            .prop_flat_map(|fields| {
                                let n = fields.len();
                                (
                                    Just(fields),
                                    prop::collection::vec(arb_ident_name(), n..=n),
                                )
                            })
                            .prop_map(|(fields, names)| {
                                let unique_names: Vec<String> = names
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, n)| format!("{n}{i}"))
                                    .collect();
                                let field_types: Vec<NameTypePair> = fields
                                    .iter()
                                    .enumerate()
                                    .map(|(i, (t, _))| NameTypePair {
                                        name: unique_names[i].clone(),
                                        typ: t.clone(),
                                    })
                                    .collect();
                                let field_values: Vec<Value> =
                                    fields.into_iter().map(|(_, v)| v).collect();
                                (
                                    AnalysedType::Record(TypeRecord {
                                        name: None,
                                        owner: None,
                                        fields: field_types,
                                    }),
                                    Value::Record(field_values),
                                )
                            }),
                        // Tuple (1-3 items)
                        prop::collection::vec(inner.clone(), 1..4).prop_map(|items| {
                            let item_types: Vec<AnalysedType> =
                                items.iter().map(|(t, _)| t.clone()).collect();
                            let item_values: Vec<Value> =
                                items.into_iter().map(|(_, v)| v).collect();
                            (
                                AnalysedType::Tuple(TypeTuple {
                                    name: None,
                                    owner: None,
                                    items: item_types,
                                }),
                                Value::Tuple(item_values),
                            )
                        }),
                        // Variant with payload
                        (1..3usize, inner.clone()).prop_flat_map(
                            |(num_cases, (payload_type, payload_val))| {
                                let names: Vec<String> = (0..num_cases + 1)
                                    .map(|i| format!("case{i}"))
                                    .collect();
                                (
                                    0..=num_cases,
                                    Just(names),
                                    Just(payload_type),
                                    Just(payload_val),
                                )
                                    .prop_map(|(chosen, names, pt, pv)| {
                                        let cases: Vec<NameOptionTypePair> = names
                                            .iter()
                                            .enumerate()
                                            .map(|(i, name)| {
                                                if i == chosen {
                                                    NameOptionTypePair {
                                                        name: name.clone(),
                                                        typ: Some(pt.clone()),
                                                    }
                                                } else {
                                                    NameOptionTypePair {
                                                        name: name.clone(),
                                                        typ: None,
                                                    }
                                                }
                                            })
                                            .collect();
                                        (
                                            AnalysedType::Variant(TypeVariant {
                                                name: None,
                                                owner: None,
                                                cases,
                                            }),
                                            Value::Variant {
                                                case_idx: chosen as u32,
                                                case_value: Some(Box::new(pv)),
                                            },
                                        )
                                    })
                            }
                        ),
                        // Enum
                        (2..5usize).prop_flat_map(|num_cases| {
                            let names: Vec<String> =
                                (0..num_cases).map(|i| format!("opt{i}")).collect();
                            (0..num_cases, Just(names)).prop_map(|(chosen, names)| {
                                (
                                    AnalysedType::Enum(TypeEnum {
                                        name: None,
                                        owner: None,
                                        cases: names,
                                    }),
                                    Value::Enum(chosen as u32),
                                )
                            })
                        }),
                        // Flags
                        (1..6usize).prop_flat_map(|num_flags| {
                            let names: Vec<String> =
                                (0..num_flags).map(|i| format!("flag{i}")).collect();
                            prop::collection::vec(any::<bool>(), num_flags..=num_flags)
                                .prop_map(move |bits| {
                                    (
                                        AnalysedType::Flags(TypeFlags {
                                            name: None,
                                            owner: None,
                                            names: names.clone(),
                                        }),
                                        Value::Flags(bits),
                                    )
                                })
                        }),
                        // Result Ok with payload
                        (inner.clone(), inner.clone()).prop_map(
                            |((ok_t, ok_v), (err_t, _))| {
                                (
                                    AnalysedType::Result(TypeResult {
                                        name: None,
                                        owner: None,
                                        ok: Some(Box::new(ok_t)),
                                        err: Some(Box::new(err_t)),
                                    }),
                                    Value::Result(Ok(Some(Box::new(ok_v)))),
                                )
                            }
                        ),
                        // Result Err with payload
                        (inner.clone(), inner.clone()).prop_map(
                            |((ok_t, _), (err_t, err_v))| {
                                (
                                    AnalysedType::Result(TypeResult {
                                        name: None,
                                        owner: None,
                                        ok: Some(Box::new(ok_t)),
                                        err: Some(Box::new(err_t)),
                                    }),
                                    Value::Result(Err(Some(Box::new(err_v)))),
                                )
                            }
                        ),
                        // Result Ok unit
                        inner.clone().prop_map(|(err_t, _)| {
                            (
                                AnalysedType::Result(TypeResult {
                                    name: None,
                                    owner: None,
                                    ok: None,
                                    err: Some(Box::new(err_t)),
                                }),
                                Value::Result(Ok(None)),
                            )
                        }),
                        // Result Err unit
                        inner.clone().prop_map(|(ok_t, _)| {
                            (
                                AnalysedType::Result(TypeResult {
                                    name: None,
                                    owner: None,
                                    ok: Some(Box::new(ok_t)),
                                    err: None,
                                }),
                                Value::Result(Err(None)),
                            )
                        }),
                    ]
                },
            )
            .boxed()
    }

    /// Wrap a CM type+value into a DataSchema/DataValue pair.
    fn cm_schema_for(typ: AnalysedType) -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "p".to_string(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: typ,
                }),
            }],
        })
    }

    fn cm_data_for(value: Value, typ: AnalysedType) -> DataValue {
        DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(value, typ),
            })],
        })
    }

    // ── Roundtrip tests ─────────────────────────────────────────────────

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 200, .. ProptestConfig::default()
        })]

        #[test]
        fn proptest_rust_leaf_roundtrip((typ, val) in leaf_type_and_value()) {
            let data = cm_data_for(val, typ.clone());
            let schema = cm_schema_for(typ);
            let rendered = render_data_value(&data, &SourceLanguage::Rust);
            let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::Rust)
                .unwrap_or_else(|e| panic!("Rust parse failed for '{rendered}': {e}"));
            prop_assert_eq!(data, parsed);
        }

        #[test]
        fn proptest_ts_leaf_roundtrip((typ, val) in leaf_type_and_value()) {
            let data = cm_data_for(val, typ.clone());
            let schema = cm_schema_for(typ);
            let rendered = render_data_value(&data, &SourceLanguage::TypeScript);
            let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::TypeScript)
                .unwrap_or_else(|e| panic!("TS parse failed for '{rendered}': {e}"));
            prop_assert_eq!(data, parsed);
        }

        #[test]
        fn proptest_rust_complex_roundtrip((typ, val) in arb_type_and_value()) {
            let data = cm_data_for(val, typ.clone());
            let schema = cm_schema_for(typ);
            let rendered = render_data_value(&data, &SourceLanguage::Rust);
            let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::Rust)
                .unwrap_or_else(|e| panic!("Rust parse failed for '{rendered}': {e}"));
            prop_assert_eq!(data, parsed);
        }

        #[test]
        fn proptest_ts_complex_roundtrip((typ, val) in arb_type_and_value()) {
            let data = cm_data_for(val, typ.clone());
            let schema = cm_schema_for(typ);
            let rendered = render_data_value(&data, &SourceLanguage::TypeScript);
            let parsed = parse_agent_id_params(&rendered, &schema, &SourceLanguage::TypeScript)
                .unwrap_or_else(|e| panic!("TS parse failed for '{rendered}': {e}"));
            prop_assert_eq!(data, parsed);
        }
    }

    #[test]
    fn source_language_from_str() {
        // Rust variants
        assert_eq!(SourceLanguage::from("rust"), SourceLanguage::Rust);
        assert_eq!(SourceLanguage::from("Rust"), SourceLanguage::Rust);
        assert_eq!(SourceLanguage::from("RUST"), SourceLanguage::Rust);
        assert_eq!(SourceLanguage::from("  rust  "), SourceLanguage::Rust);

        // TypeScript variants
        assert_eq!(SourceLanguage::from("typescript"), SourceLanguage::TypeScript);
        assert_eq!(SourceLanguage::from("TypeScript"), SourceLanguage::TypeScript);
        assert_eq!(SourceLanguage::from("ts"), SourceLanguage::TypeScript);
        assert_eq!(SourceLanguage::from("TS"), SourceLanguage::TypeScript);
        assert_eq!(SourceLanguage::from("  typescript  "), SourceLanguage::TypeScript);

        // Other
        assert_eq!(SourceLanguage::from("go"), SourceLanguage::Other("go".to_string()));
        assert_eq!(SourceLanguage::from(""), SourceLanguage::Other("".to_string()));
        assert_eq!(SourceLanguage::from("python"), SourceLanguage::Other("python".to_string()));
    }

    #[test]
    fn unknown_language_falls_back_to_canonical() {
        use golem_common::model::agent::structural_format::format_structural;

        let data = cm_value(Value::U32(42), AnalysedType::U32(TypeU32));
        let rendered_other = render_data_value(&data, &SourceLanguage::Other("go".to_string()));
        let canonical = format_structural(&data).unwrap();
        assert_eq!(
            rendered_other, canonical,
            "Unknown language should produce canonical structural format"
        );
    }

#[test]
fn rust_language_specific_parsed_first() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    let schema = cm_schema(typ.clone());
    let parsed = parse_agent_id_params("Some(42)", &schema, &SourceLanguage::Rust).unwrap();
    let expected = cm_value(
        Value::Option(Some(Box::new(Value::U32(42)))),
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn ts_language_specific_parsed_first() {
    let typ = AnalysedType::Record(TypeRecord {
        name: None,
        owner: None,
        fields: vec![NameTypePair {
            name: "fieldOne".to_string(),
            typ: AnalysedType::U32(TypeU32),
        }],
    });
    let schema = cm_schema(typ.clone());
    let parsed =
        parse_agent_id_params("{ fieldOne: 42 }", &schema, &SourceLanguage::TypeScript).unwrap();
    let expected = cm_value(Value::Record(vec![Value::U32(42)]), typ);
    assert_eq!(parsed, expected);
}

#[test]
fn canonical_fallback_for_rust_language() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    let schema = cm_schema(typ.clone());
    let parsed = parse_agent_id_params("s(42)", &schema, &SourceLanguage::Rust).unwrap();
    let expected = cm_value(
        Value::Option(Some(Box::new(Value::U32(42)))),
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn canonical_fallback_for_ts_language() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    let schema = cm_schema(typ.clone());
    let parsed =
        parse_agent_id_params("s(42)", &schema, &SourceLanguage::TypeScript).unwrap();
    let expected = cm_value(
        Value::Option(Some(Box::new(Value::U32(42)))),
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn combined_error_on_both_failures() {
    let schema = cm_schema(AnalysedType::U32(TypeU32));
    let result =
        parse_agent_id_params("not_a_number_at_all!!!", &schema, &SourceLanguage::Rust);
    let err = result.unwrap_err();
    assert!(
        err.message.contains("Rust parser"),
        "error should mention Rust parser: {}",
        err.message
    );
    assert!(
        err.message.contains("Structural parser"),
        "error should mention Structural parser: {}",
        err.message
    );
}

#[test]
fn combined_error_on_both_failures_ts() {
    let schema = cm_schema(AnalysedType::U32(TypeU32));
    let result = parse_agent_id_params(
        "not_a_number_at_all!!!",
        &schema,
        &SourceLanguage::TypeScript,
    );
    let err = result.unwrap_err();
    assert!(
        err.message.contains("TypeScript parser"),
        "error should mention TypeScript parser: {}",
        err.message
    );
    assert!(
        err.message.contains("Structural parser"),
        "error should mention Structural parser: {}",
        err.message
    );
}

#[test]
fn unknown_language_uses_canonical_only() {
    let schema = cm_schema(AnalysedType::U32(TypeU32));
    let parsed = parse_agent_id_params(
        "42",
        &schema,
        &SourceLanguage::Other("go".into()),
    )
    .unwrap();
    let expected = cm_value(Value::U32(42), AnalysedType::U32(TypeU32));
    assert_eq!(parsed, expected);
}

#[test]
fn rust_option_none_parsed() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    let schema = cm_schema(typ.clone());
    let parsed = parse_agent_id_params("None", &schema, &SourceLanguage::Rust).unwrap();
    let expected = cm_value(Value::Option(None), typ);
    assert_eq!(parsed, expected);
}

#[test]
fn rust_result_ok_parsed() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    let schema = cm_schema(typ.clone());
    let parsed = parse_agent_id_params("Ok(42)", &schema, &SourceLanguage::Rust).unwrap();
    let expected = cm_value(
        Value::Result(Ok(Some(Box::new(Value::U32(42))))),
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn rust_result_err_parsed() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    let schema = cm_schema(typ.clone());
    let parsed =
        parse_agent_id_params(r#"Err("fail")"#, &schema, &SourceLanguage::Rust).unwrap();
    let expected = cm_value(
        Value::Result(Err(Some(Box::new(Value::String("fail".into()))))),
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn ts_record_camel_case_fields() {
    let typ = AnalysedType::Record(TypeRecord {
        name: None,
        owner: None,
        fields: vec![
            NameTypePair {
                name: "myField".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "anotherField".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    });
    let schema = cm_schema(typ.clone());
    let parsed = parse_agent_id_params(
        r#"{ myField: 10, anotherField: "hi" }"#,
        &schema,
        &SourceLanguage::TypeScript,
    )
    .unwrap();
    let expected = cm_value(
        Value::Record(vec![Value::U32(10), Value::String("hi".into())]),
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn rust_variant_pascal_case() {
    let typ = AnalysedType::Variant(TypeVariant {
        name: None,
        owner: None,
        cases: vec![
            NameOptionTypePair {
                name: "MyCase".to_string(),
                typ: Some(AnalysedType::U32(TypeU32)),
            },
            NameOptionTypePair {
                name: "OtherCase".to_string(),
                typ: None,
            },
        ],
    });
    let schema = cm_schema(typ.clone());
    let parsed = parse_agent_id_params("MyCase(5)", &schema, &SourceLanguage::Rust).unwrap();
    let expected = cm_value(
        Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::U32(5))),
        },
        typ,
    );
    assert_eq!(parsed, expected);
}

#[test]
fn multi_param_rust_syntax() {
    let schema = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "p1".to_string(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: AnalysedType::U32(TypeU32),
                }),
            },
            NamedElementSchema {
                name: "p2".to_string(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: AnalysedType::Option(TypeOption {
                        name: None,
                        owner: None,
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    }),
                }),
            },
        ],
    });
    let parsed = parse_agent_id_params(
        r#"42, Some("hello")"#,
        &schema,
        &SourceLanguage::Rust,
    )
    .unwrap();
    let expected = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::U32(42), AnalysedType::U32(TypeU32)),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(
                    Value::Option(Some(Box::new(Value::String("hello".into())))),
                    AnalysedType::Option(TypeOption {
                        name: None,
                        owner: None,
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    }),
                ),
            }),
        ],
    });
    assert_eq!(parsed, expected);
}
