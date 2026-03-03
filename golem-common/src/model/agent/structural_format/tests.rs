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

use super::*;
use crate::model::agent::{
    BinaryDescriptor, BinaryReference, BinarySource, BinaryType, ComponentModelElementSchema,
    ComponentModelElementValue, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchema, NamedElementSchemas, NamedElementValue, NamedElementValues,
    TextDescriptor, TextReference, TextSource, TextType, UnstructuredBinaryElementValue,
    UnstructuredTextElementValue, Url,
};
use golem_wasm::analysis::analysed_type::{
    bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result, result_err,
    result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case, unit_result, variant,
};
use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode, TypeHandle};
use golem_wasm::{IntoValueAndType, Value, ValueAndType};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use test_r::test;

// ── Helper functions ────────────────────────────────────────────────────────

fn tuple_schema(elements: Vec<(&str, ElementSchema)>) -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas {
        elements: elements
            .into_iter()
            .map(|(name, schema)| NamedElementSchema {
                name: name.to_string(),
                schema,
            })
            .collect(),
    })
}

fn multimodal_schema(elements: Vec<(&str, ElementSchema)>) -> DataSchema {
    DataSchema::Multimodal(NamedElementSchemas {
        elements: elements
            .into_iter()
            .map(|(name, schema)| NamedElementSchema {
                name: name.to_string(),
                schema,
            })
            .collect(),
    })
}

fn cm(typ: AnalysedType) -> ElementSchema {
    ElementSchema::ComponentModel(ComponentModelElementSchema { element_type: typ })
}

fn text_elem_schema() -> ElementSchema {
    ElementSchema::UnstructuredText(TextDescriptor {
        restrictions: None,
    })
}

fn binary_elem_schema() -> ElementSchema {
    ElementSchema::UnstructuredBinary(BinaryDescriptor {
        restrictions: None,
    })
}

fn roundtrip(data: &DataValue, schema: &DataSchema) {
    let formatted = format_structural(data).unwrap();
    let parsed = parse_structural(&formatted, schema).unwrap();
    assert_eq!(&parsed, data, "Roundtrip failed for formatted: {formatted}");
}

fn format_and_check(data: &DataValue, expected: &str) {
    let formatted = format_structural(data).unwrap();
    assert_eq!(formatted, expected);
}

// ── Empty tuple ─────────────────────────────────────────────────────────────

#[test]
fn empty_tuple() {
    let schema = tuple_schema(vec![]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![],
    });
    format_and_check(&data, "");
    roundtrip(&data, &schema);
}

// ── Integer types ───────────────────────────────────────────────────────────

#[test]
fn single_u32() {
    let schema = tuple_schema(vec![("x", cm(u32()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: 42u32.into_value_and_type(),
        })],
    });
    format_and_check(&data, "42");
    roundtrip(&data, &schema);
}

#[test]
fn single_u32_zero() {
    let schema = tuple_schema(vec![("x", cm(u32()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: 0u32.into_value_and_type(),
        })],
    });
    format_and_check(&data, "0");
    roundtrip(&data, &schema);
}

#[test]
fn signed_negative() {
    let schema = tuple_schema(vec![("x", cm(s32()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::S32(-42), s32()),
        })],
    });
    format_and_check(&data, "-42");
    roundtrip(&data, &schema);
}

#[test]
fn all_integer_types() {
    let schema = tuple_schema(vec![
        ("a", cm(u8())),
        ("b", cm(u16())),
        ("c", cm(u64())),
        ("d", cm(s8())),
        ("e", cm(s16())),
        ("f", cm(s64())),
    ]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::U8(255), u8()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::U16(65535), u16()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::U64(u64::MAX), u64()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::S8(-128), s8()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::S16(-32768), s16()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::S64(i64::MIN), s64()),
            }),
        ],
    });
    format_and_check(
        &data,
        &format!("255,65535,{},-128,-32768,{}", u64::MAX, i64::MIN),
    );
    roundtrip(&data, &schema);
}

// ── Float types ─────────────────────────────────────────────────────────────

#[test]
fn float_basic() {
    let schema = tuple_schema(vec![("x", cm(f64()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::F64(3.14), f64()),
        })],
    });
    let formatted = format_structural(&data).unwrap();
    assert!(
        formatted.contains('.'),
        "Float must contain decimal point: {formatted}"
    );
    roundtrip(&data, &schema);
}

#[test]
fn float_zero() {
    let schema = tuple_schema(vec![("x", cm(f64()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::F64(0.0), f64()),
        })],
    });
    format_and_check(&data, "0.0");
    roundtrip(&data, &schema);
}

#[test]
fn float_negative_zero() {
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::F64(-0.0), f64()),
        })],
    });
    // -0.0 maps to 0.0
    format_and_check(&data, "0.0");
}

#[test]
fn float_nan_rejected() {
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::F64(f64::NAN), f64()),
        })],
    });
    assert_eq!(
        format_structural(&data),
        Err(StructuralFormatError::RejectedFloat)
    );
}

#[test]
fn float_inf_rejected() {
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::F64(f64::INFINITY), f64()),
        })],
    });
    assert_eq!(
        format_structural(&data),
        Err(StructuralFormatError::RejectedFloat)
    );
}

#[test]
fn float_f32() {
    let schema = tuple_schema(vec![("x", cm(f32()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::F32(1.5), f32()),
        })],
    });
    format_and_check(&data, "1.5");
    roundtrip(&data, &schema);
}

// ── Bool ────────────────────────────────────────────────────────────────────

#[test]
fn bool_values() {
    let schema = tuple_schema(vec![("a", cm(bool())), ("b", cm(bool()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::Bool(true), bool()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::Bool(false), bool()),
            }),
        ],
    });
    format_and_check(&data, "true,false");
    roundtrip(&data, &schema);
}

// ── String ──────────────────────────────────────────────────────────────────

#[test]
fn string_simple() {
    let schema = tuple_schema(vec![("x", cm(str()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::String("hello".into()), str()),
        })],
    });
    format_and_check(&data, r#""hello""#);
    roundtrip(&data, &schema);
}

#[test]
fn string_with_escapes() {
    let schema = tuple_schema(vec![("x", cm(str()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::String("hello\n\"world\"\t\\".into()),
                str(),
            ),
        })],
    });
    format_and_check(&data, r#""hello\n\"world\"\t\\""#);
    roundtrip(&data, &schema);
}

#[test]
fn string_with_control_chars() {
    let schema = tuple_schema(vec![("x", cm(str()))]);
    let original = "a\u{0001}b\u{001F}c";
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::String(original.into()), str()),
        })],
    });
    roundtrip(&data, &schema);
}

// ── Char ────────────────────────────────────────────────────────────────────

#[test]
fn char_simple() {
    let schema = tuple_schema(vec![("x", cm(chr()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Char('a'), chr()),
        })],
    });
    format_and_check(&data, r#"c"a""#);
    roundtrip(&data, &schema);
}

#[test]
fn char_special() {
    let schema = tuple_schema(vec![("x", cm(chr()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Char('\n'), chr()),
        })],
    });
    format_and_check(&data, r#"c"\n""#);
    roundtrip(&data, &schema);
}

#[test]
fn char_unicode() {
    let schema = tuple_schema(vec![("x", cm(chr()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Char('🎉'), chr()),
        })],
    });
    roundtrip(&data, &schema);
}

// ── Record ──────────────────────────────────────────────────────────────────

#[test]
fn record_simple() {
    let schema = tuple_schema(vec![(
        "r",
        cm(record(vec![field("x", u32()), field("y", u32())])),
    )]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Record(vec![Value::U32(10), Value::U32(20)]),
                record(vec![field("x", u32()), field("y", u32())]),
            ),
        })],
    });
    format_and_check(&data, "(10,20)");
    roundtrip(&data, &schema);
}

#[test]
fn record_empty() {
    let schema = tuple_schema(vec![("r", cm(record(vec![])))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Record(vec![]), record(vec![])),
        })],
    });
    format_and_check(&data, "()");
    roundtrip(&data, &schema);
}

#[test]
fn record_with_flags() {
    let schema = tuple_schema(vec![(
        "r",
        cm(record(vec![
            field("x", u32()),
            field("y", u32()),
            field("properties", flags(&["a", "b", "c"])),
        ])),
    )]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Record(vec![
                    Value::U32(12),
                    Value::U32(32),
                    Value::Flags(vec![true, false, true]),
                ]),
                record(vec![
                    field("x", u32()),
                    field("y", u32()),
                    field("properties", flags(&["a", "b", "c"])),
                ]),
            ),
        })],
    });
    format_and_check(&data, "(12,32,f(0,2))");
    roundtrip(&data, &schema);
}

// ── Multiple elements ───────────────────────────────────────────────────────

#[test]
fn multiple_cm_elements() {
    let schema = tuple_schema(vec![
        ("a", cm(u32())),
        (
            "b",
            cm(record(vec![
                field("x", u32()),
                field("y", u32()),
                field("properties", flags(&["a", "b", "c"])),
            ])),
        ),
    ]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: 32u32.into_value_and_type(),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(
                    Value::Record(vec![
                        Value::U32(12),
                        Value::U32(32),
                        Value::Flags(vec![true, false, true]),
                    ]),
                    record(vec![
                        field("x", u32()),
                        field("y", u32()),
                        field("properties", flags(&["a", "b", "c"])),
                    ]),
                ),
            }),
        ],
    });
    format_and_check(&data, "32,(12,32,f(0,2))");
    roundtrip(&data, &schema);
}

// ── List ────────────────────────────────────────────────────────────────────

#[test]
fn list_of_u32() {
    let schema = tuple_schema(vec![("args", cm(list(u32())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::List(vec![Value::U32(12), Value::U32(13), Value::U32(14)]),
                list(u32()),
            ),
        })],
    });
    format_and_check(&data, "[12,13,14]");
    roundtrip(&data, &schema);
}

#[test]
fn list_empty() {
    let schema = tuple_schema(vec![("args", cm(list(u32())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::List(vec![]), list(u32())),
        })],
    });
    format_and_check(&data, "[]");
    roundtrip(&data, &schema);
}

// ── Variant ─────────────────────────────────────────────────────────────────

#[test]
fn variant_no_payload() {
    let schema = tuple_schema(vec![(
        "v",
        cm(variant(vec![unit_case("a"), unit_case("b")])),
    )]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Variant {
                    case_idx: 1,
                    case_value: None,
                },
                variant(vec![unit_case("a"), unit_case("b")]),
            ),
        })],
    });
    format_and_check(&data, "v1");
    roundtrip(&data, &schema);
}

#[test]
fn variant_with_payload() {
    let schema = tuple_schema(vec![(
        "v",
        cm(variant(vec![case("x", u32()), case("y", str())])),
    )]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Variant {
                    case_idx: 1,
                    case_value: Some(Box::new(Value::String("hello".into()))),
                },
                variant(vec![case("x", u32()), case("y", str())]),
            ),
        })],
    });
    format_and_check(&data, r#"v1("hello")"#);
    roundtrip(&data, &schema);
}

// ── Enum ────────────────────────────────────────────────────────────────────

#[test]
fn enum_value() {
    let schema = tuple_schema(vec![(
        "e",
        cm(r#enum(&["red", "green", "blue"])),
    )]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Enum(2),
                r#enum(&["red", "green", "blue"]),
            ),
        })],
    });
    format_and_check(&data, "v2");
    roundtrip(&data, &schema);
}

// ── Option ──────────────────────────────────────────────────────────────────

#[test]
fn option_some() {
    let schema = tuple_schema(vec![("x", cm(option(u32())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Option(Some(Box::new(Value::U32(42)))),
                option(u32()),
            ),
        })],
    });
    format_and_check(&data, "s(42)");
    roundtrip(&data, &schema);
}

#[test]
fn option_none() {
    let schema = tuple_schema(vec![("x", cm(option(u32())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Option(None), option(u32())),
        })],
    });
    format_and_check(&data, "n");
    roundtrip(&data, &schema);
}

// ── Result ──────────────────────────────────────────────────────────────────

#[test]
fn result_ok_with_value() {
    let schema = tuple_schema(vec![("x", cm(result(u32(), str())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Result(Ok(Some(Box::new(Value::U32(1))))),
                result(u32(), str()),
            ),
        })],
    });
    format_and_check(&data, "ok(1)");
    roundtrip(&data, &schema);
}

#[test]
fn result_ok_unit() {
    let schema = tuple_schema(vec![("x", cm(result_err(str())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Result(Ok(None)), result_err(str())),
        })],
    });
    format_and_check(&data, "ok");
    roundtrip(&data, &schema);
}

#[test]
fn result_err_with_value() {
    let schema = tuple_schema(vec![("x", cm(result(u32(), str())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Result(Err(Some(Box::new(Value::String("bad".into()))))),
                result(u32(), str()),
            ),
        })],
    });
    format_and_check(&data, r#"err("bad")"#);
    roundtrip(&data, &schema);
}

#[test]
fn result_err_unit() {
    let schema = tuple_schema(vec![("x", cm(result_ok(u32())))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Result(Err(None)), result_ok(u32())),
        })],
    });
    format_and_check(&data, "err");
    roundtrip(&data, &schema);
}

#[test]
fn result_unit_both() {
    let schema = tuple_schema(vec![("x", cm(unit_result()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Result(Ok(None)), unit_result()),
        })],
    });
    format_and_check(&data, "ok");
    roundtrip(&data, &schema);
}

// ── Flags ───────────────────────────────────────────────────────────────────

#[test]
fn flags_some_set() {
    let schema = tuple_schema(vec![("f", cm(flags(&["a", "b", "c"])))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Flags(vec![true, false, true]),
                flags(&["a", "b", "c"]),
            ),
        })],
    });
    format_and_check(&data, "f(0,2)");
    roundtrip(&data, &schema);
}

#[test]
fn flags_none_set() {
    let schema = tuple_schema(vec![("f", cm(flags(&["a", "b", "c"])))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Flags(vec![false, false, false]),
                flags(&["a", "b", "c"]),
            ),
        })],
    });
    format_and_check(&data, "f()");
    roundtrip(&data, &schema);
}

// ── Tuple ───────────────────────────────────────────────────────────────────

#[test]
fn tuple_type() {
    let schema = tuple_schema(vec![("t", cm(tuple(vec![u32(), str()])))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Tuple(vec![Value::U32(1), Value::String("hi".into())]),
                tuple(vec![u32(), str()]),
            ),
        })],
    });
    format_and_check(&data, r#"(1,"hi")"#);
    roundtrip(&data, &schema);
}

// ── Unstructured text ───────────────────────────────────────────────────────

#[test]
fn text_inline_no_lang() {
    let schema = tuple_schema(vec![("t", text_elem_schema())]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::UnstructuredText(
            UnstructuredTextElementValue {
                value: TextReference::Inline(TextSource {
                    data: "hello, world!".to_string(),
                    text_type: None,
                }),
                descriptor: TextDescriptor {
                    restrictions: None,
                },
            },
        )],
    });
    format_and_check(&data, r#"@t"hello, world!""#);
    roundtrip(&data, &schema);
}

#[test]
fn text_inline_with_lang() {
    let schema = tuple_schema(vec![("t", text_elem_schema())]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::UnstructuredText(
            UnstructuredTextElementValue {
                value: TextReference::Inline(TextSource {
                    data: "hallo".to_string(),
                    text_type: Some(TextType {
                        language_code: "de".to_string(),
                    }),
                }),
                descriptor: TextDescriptor {
                    restrictions: None,
                },
            },
        )],
    });
    format_and_check(&data, r#"@t[de]"hallo""#);
    roundtrip(&data, &schema);
}

#[test]
fn text_url() {
    let schema = tuple_schema(vec![("t", text_elem_schema())]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::UnstructuredText(
            UnstructuredTextElementValue {
                value: TextReference::Url(Url {
                    value: "https://example.com/".to_string(),
                }),
                descriptor: TextDescriptor {
                    restrictions: None,
                },
            },
        )],
    });
    format_and_check(&data, r#"@tu"https://example.com/""#);
    roundtrip(&data, &schema);
}

// ── Unstructured binary ─────────────────────────────────────────────────────

#[test]
fn binary_inline() {
    let schema = tuple_schema(vec![("b", binary_elem_schema())]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::UnstructuredBinary(
            UnstructuredBinaryElementValue {
                value: BinaryReference::Inline(BinarySource {
                    data: b"Hello world!".to_vec(),
                    binary_type: BinaryType {
                        mime_type: "application/json".to_string(),
                    },
                }),
                descriptor: BinaryDescriptor {
                    restrictions: None,
                },
            },
        )],
    });
    let expected_b64 = base64::engine::general_purpose::STANDARD.encode(b"Hello world!");
    format_and_check(
        &data,
        &format!(r#"@b[application/json]"{expected_b64}""#),
    );
    roundtrip(&data, &schema);
}

#[test]
fn binary_url() {
    let schema = tuple_schema(vec![("b", binary_elem_schema())]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::UnstructuredBinary(
            UnstructuredBinaryElementValue {
                value: BinaryReference::Url(Url {
                    value: "https://example.com/image.png".to_string(),
                }),
                descriptor: BinaryDescriptor {
                    restrictions: None,
                },
            },
        )],
    });
    format_and_check(
        &data,
        r#"@bu"https://example.com/image.png""#,
    );
    roundtrip(&data, &schema);
}

// ── Multimodal ──────────────────────────────────────────────────────────────

#[test]
fn multimodal_basic() {
    let schema = multimodal_schema(vec![
        ("x", cm(u32())),
        ("y", text_elem_schema()),
        ("z", binary_elem_schema()),
    ]);
    let data = DataValue::Multimodal(NamedElementValues {
        elements: vec![
            NamedElementValue {
                name: "x".to_string(),
                value: ElementValue::ComponentModel(ComponentModelElementValue {
                    value: 42u32.into_value_and_type(),
                }),
                schema_index: 0,
            },
            NamedElementValue {
                name: "y".to_string(),
                value: ElementValue::UnstructuredText(UnstructuredTextElementValue {
                    value: TextReference::Inline(TextSource {
                        data: "hello".to_string(),
                        text_type: None,
                    }),
                    descriptor: TextDescriptor {
                        restrictions: None,
                    },
                }),
                schema_index: 1,
            },
        ],
    });
    format_and_check(&data, r#"0(42),1(@t"hello")"#);
    roundtrip(&data, &schema);
}

#[test]
fn multimodal_repeated_element() {
    let schema = multimodal_schema(vec![("x", cm(u32())), ("y", text_elem_schema())]);
    let data = DataValue::Multimodal(NamedElementValues {
        elements: vec![
            NamedElementValue {
                name: "y".to_string(),
                value: ElementValue::UnstructuredText(UnstructuredTextElementValue {
                    value: TextReference::Inline(TextSource {
                        data: "first".to_string(),
                        text_type: None,
                    }),
                    descriptor: TextDescriptor {
                        restrictions: None,
                    },
                }),
                schema_index: 1,
            },
            NamedElementValue {
                name: "y".to_string(),
                value: ElementValue::UnstructuredText(UnstructuredTextElementValue {
                    value: TextReference::Inline(TextSource {
                        data: "second".to_string(),
                        text_type: None,
                    }),
                    descriptor: TextDescriptor {
                        restrictions: None,
                    },
                }),
                schema_index: 1,
            },
        ],
    });
    format_and_check(&data, r#"1(@t"first"),1(@t"second")"#);
    roundtrip(&data, &schema);
}

#[test]
fn multimodal_empty() {
    let schema = multimodal_schema(vec![("x", cm(u32()))]);
    let data = DataValue::Multimodal(NamedElementValues {
        elements: vec![],
    });
    format_and_check(&data, "");
    roundtrip(&data, &schema);
}

// ── Normalization ───────────────────────────────────────────────────────────

#[test]
fn normalize_strips_whitespace() {
    assert_eq!(normalize_structural("  42 , ( 10 , 20 )  "), "42,(10,20)");
}

#[test]
fn normalize_preserves_string_contents() {
    assert_eq!(
        normalize_structural(r#"  "hello world"  "#),
        r#""hello world""#
    );
}

#[test]
fn normalize_preserves_escapes_in_strings() {
    assert_eq!(
        normalize_structural(r#"  "hello\" world"  "#),
        r#""hello\" world""#
    );
}

#[test]
fn normalize_empty() {
    assert_eq!(normalize_structural(""), "");
}

// ── Parse error cases ───────────────────────────────────────────────────────

#[test]
fn parse_leading_zeros_rejected() {
    let schema = tuple_schema(vec![("x", cm(u32()))]);
    assert!(parse_structural("01", &schema).is_err());
}

#[test]
fn parse_negative_zero_integer_rejected() {
    let schema = tuple_schema(vec![("x", cm(s32()))]);
    assert!(parse_structural("-0", &schema).is_err());
}

#[test]
fn parse_float_missing_decimal_rejected() {
    let schema = tuple_schema(vec![("x", cm(f64()))]);
    assert!(parse_structural("42", &schema).is_err());
}

#[test]
fn parse_flags_non_increasing_rejected() {
    let schema = tuple_schema(vec![("f", cm(flags(&["a", "b", "c"])))]);
    assert!(parse_structural("f(2,1)", &schema).is_err());
}

#[test]
fn parse_flags_out_of_range_rejected() {
    let schema = tuple_schema(vec![("f", cm(flags(&["a", "b", "c"])))]);
    assert!(parse_structural("f(5)", &schema).is_err());
}

#[test]
fn parse_variant_out_of_range_rejected() {
    let schema = tuple_schema(vec![(
        "v",
        cm(variant(vec![unit_case("a"), unit_case("b")])),
    )]);
    assert!(parse_structural("v3", &schema).is_err());
}

#[test]
fn parse_trailing_input_rejected() {
    let schema = tuple_schema(vec![("x", cm(u32()))]);
    assert!(parse_structural("42extra", &schema).is_err());
}

// ── Handle type rejected ────────────────────────────────────────────────────

#[test]
fn handle_type_rejected() {
    use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode, TypeHandle};
    let handle_type = AnalysedType::Handle(TypeHandle {
        name: None,
        owner: None,
        resource_id: AnalysedResourceId(0),
        mode: AnalysedResourceMode::Owned,
    });
    let _schema = tuple_schema(vec![("h", cm(handle_type.clone()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Handle {
                    uri: "test".to_string(),
                    resource_id: 0,
                },
                handle_type,
            ),
        })],
    });
    assert_eq!(
        format_structural(&data),
        Err(StructuralFormatError::HandleType)
    );
}

// ── Nested structures ───────────────────────────────────────────────────────

#[test]
fn deeply_nested_record() {
    let inner = record(vec![field("a", u32())]);
    let middle = record(vec![field("inner", inner.clone())]);
    let outer = record(vec![field("middle", middle.clone())]);
    let schema = tuple_schema(vec![("r", cm(outer.clone()))]);

    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Record(vec![Value::Record(vec![Value::Record(vec![
                    Value::U32(99),
                ])])]),
                outer,
            ),
        })],
    });
    format_and_check(&data, "(((99)))");
    roundtrip(&data, &schema);
}

// ── Complex combined test ───────────────────────────────────────────────────

#[test]
fn complex_agent_id_example() {
    let schema = tuple_schema(vec![
        ("a", cm(u32())),
        (
            "r",
            cm(record(vec![
                field("x", u32()),
                field("y", u32()),
                field("properties", flags(&["a", "b", "c"])),
            ])),
        ),
    ]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: 32u32.into_value_and_type(),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(
                    Value::Record(vec![
                        Value::U32(12),
                        Value::U32(32),
                        Value::Flags(vec![true, false, true]),
                    ]),
                    record(vec![
                        field("x", u32()),
                        field("y", u32()),
                        field("properties", flags(&["a", "b", "c"])),
                    ]),
                ),
            }),
        ],
    });
    format_and_check(&data, "32,(12,32,f(0,2))");
    roundtrip(&data, &schema);
}

// ── Float exponent roundtrip ────────────────────────────────────────────────

#[test]
fn float_large_exponent_roundtrip() {
    let schema = tuple_schema(vec![("x", cm(f64()))]);
    for &v in &[1e20_f64, -1e20, 1e-20, 1.5e15, -3.14e-10] {
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::F64(v), f64()),
            })],
        });
        let formatted = format_structural(&data).unwrap();
        assert!(
            formatted.contains('.'),
            "Float must contain decimal point: {formatted} (from {v})"
        );
        roundtrip(&data, &schema);
    }
}

#[test]
fn float_f32_exponent_roundtrip() {
    let schema = tuple_schema(vec![("x", cm(f32()))]);
    for &v in &[1e20_f32, -1e10, 1e-20] {
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::F32(v), f32()),
            })],
        });
        let formatted = format_structural(&data).unwrap();
        assert!(
            formatted.contains('.'),
            "Float must contain decimal point: {formatted} (from {v})"
        );
        roundtrip(&data, &schema);
    }
}

// ── Variant payload enforcement ─────────────────────────────────────────────

#[test]
fn parse_variant_missing_required_payload() {
    let schema = tuple_schema(vec![(
        "v",
        cm(variant(vec![case("x", u32())])),
    )]);
    // v0 without payload but case "x" requires u32
    assert!(parse_structural("v0", &schema).is_err());
}

#[test]
fn parse_variant_unexpected_payload() {
    let schema = tuple_schema(vec![(
        "v",
        cm(variant(vec![unit_case("x")])),
    )]);
    // v0(42) but case "x" has no payload
    assert!(parse_structural("v0(42)", &schema).is_err());
}

// ── Result payload enforcement ──────────────────────────────────────────────

#[test]
fn parse_result_ok_missing_required_payload() {
    let schema = tuple_schema(vec![("x", cm(result(u32(), str())))]);
    // bare "ok" but ok type is u32
    assert!(parse_structural("ok", &schema).is_err());
}

#[test]
fn parse_result_err_missing_required_payload() {
    let schema = tuple_schema(vec![("x", cm(result(u32(), str())))]);
    // bare "err" but err type is str
    assert!(parse_structural("err", &schema).is_err());
}

#[test]
fn parse_result_ok_unit_with_unexpected_payload() {
    let schema = tuple_schema(vec![("x", cm(result_err(str())))]);
    // ok(42) but ok type is unit
    assert!(parse_structural("ok(42)", &schema).is_err());
}

// ── Float leading zeros rejected ────────────────────────────────────────────

#[test]
fn parse_float_leading_zeros_rejected() {
    let schema = tuple_schema(vec![("x", cm(f64()))]);
    assert!(parse_structural("01.0", &schema).is_err());
}

// ── Proptest strategies ─────────────────────────────────────────────────────

/// Generate a finite f32 value (no NaN/Infinity).
fn finite_f32() -> BoxedStrategy<f32> {
    prop_oneof![
        proptest::num::f32::NORMAL,
        Just(0.0f32),
        Just(-0.0f32),
    ]
    .boxed()
}

/// Generate a finite f64 value (no NaN/Infinity).
fn finite_f64() -> BoxedStrategy<f64> {
    prop_oneof![
        proptest::num::f64::NORMAL,
        Just(0.0f64),
        Just(-0.0f64),
    ]
    .boxed()
}

/// Generate a valid char (proptest's default char strategy).
fn arb_char() -> impl Strategy<Value = char> {
    proptest::char::any()
}

/// Generate an arbitrary string (may contain control chars, unicode, etc.).
fn arb_string() -> impl Strategy<Value = String> {
    ".*"
}

/// Leaf AnalysedType + matching Value (no recursion).
fn leaf_type_and_value() -> impl Strategy<Value = (AnalysedType, Value)> {
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

/// Generate a Value matching a specific leaf AnalysedType.
fn arb_leaf_value_for_type(typ: AnalysedType) -> BoxedStrategy<Value> {
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
        _ => any::<u32>().prop_map(Value::U32).boxed(),
    }
}

/// Recursive AnalysedType + matching Value, up to a bounded depth/size.
fn arb_type_and_value() -> impl Strategy<Value = (AnalysedType, Value)> {
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
                (0..5usize, leaf_type_and_value()).prop_flat_map(|(len, (item_type, _))| {
                    let gen = arb_leaf_value_for_type(item_type.clone());
                    (Just(item_type), prop::collection::vec(gen, len..=len))
                }).prop_map(|(item_type, values)| {
                    (list(item_type), Value::List(values))
                }),
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
                (1..4usize, inner.clone()).prop_flat_map(|(num_cases, (payload_type, payload_val))| {
                    (0..num_cases, Just(num_cases), Just(payload_type), Just(payload_val)).prop_map(
                        |(chosen, num_cases, pt, pv)| {
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
                        },
                    )
                }),
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
                    (
                        result(ok_t, err_t),
                        Value::Result(Ok(Some(Box::new(ok_v)))),
                    )
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

/// Generate a TextReference for unstructured text elements.
fn arb_text_reference() -> impl Strategy<Value = TextReference> {
    prop_oneof![
        arb_string().prop_map(|s| TextReference::Url(Url { value: s })),
        (arb_string(), prop_oneof![
            Just(None),
            Just(Some(TextType { language_code: "en".to_string() })),
            Just(Some(TextType { language_code: "de".to_string() })),
        ]).prop_map(|(data, text_type)| TextReference::Inline(TextSource { data, text_type })),
    ]
}

/// Generate a BinaryReference for unstructured binary elements.
fn arb_binary_reference() -> impl Strategy<Value = BinaryReference> {
    prop_oneof![
        arb_string().prop_map(|s| BinaryReference::Url(Url { value: s })),
        (prop::collection::vec(any::<u8>(), 0..64), prop_oneof![
            Just("application/json".to_string()),
            Just("image/png".to_string()),
            Just("text/plain".to_string()),
        ]).prop_map(|(data, mime)| BinaryReference::Inline(BinarySource {
            data,
            binary_type: BinaryType { mime_type: mime },
        })),
    ]
}

/// Generate an ElementSchema + matching ElementValue pair.
fn arb_element_schema_and_value() -> impl Strategy<Value = (ElementSchema, ElementValue)> {
    prop_oneof![
        5 => arb_type_and_value().prop_map(|(typ, val)| {
            let schema = ElementSchema::ComponentModel(ComponentModelElementSchema { element_type: typ.clone() });
            let value = ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(val, typ),
            });
            (schema, value)
        }),
        1 => arb_text_reference().prop_map(|text_ref| {
            let schema = ElementSchema::UnstructuredText(TextDescriptor { restrictions: None });
            let value = ElementValue::UnstructuredText(UnstructuredTextElementValue {
                value: text_ref,
                descriptor: TextDescriptor { restrictions: None },
            });
            (schema, value)
        }),
        1 => arb_binary_reference().prop_map(|bin_ref| {
            let schema = ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None });
            let value = ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                value: bin_ref,
                descriptor: BinaryDescriptor { restrictions: None },
            });
            (schema, value)
        }),
    ]
}

/// Generate a complete DataSchema::Tuple + matching DataValue::Tuple.
fn arb_tuple_data() -> impl Strategy<Value = (DataSchema, DataValue)> {
    prop::collection::vec(arb_element_schema_and_value(), 0..6).prop_map(|elems| {
        let schemas = NamedElementSchemas {
            elements: elems
                .iter()
                .enumerate()
                .map(|(i, (s, _))| NamedElementSchema {
                    name: format!("p{i}"),
                    schema: s.clone(),
                })
                .collect(),
        };
        let values = ElementValues {
            elements: elems.into_iter().map(|(_, v)| v).collect(),
        };
        (DataSchema::Tuple(schemas), DataValue::Tuple(values))
    })
}

/// Generate a complete DataSchema::Multimodal + matching DataValue::Multimodal.
fn arb_multimodal_data() -> impl Strategy<Value = (DataSchema, DataValue)> {
    // Generate 1-4 schema elements, then 0-6 named value elements referencing them.
    prop::collection::vec(arb_element_schema_and_value(), 1..5).prop_flat_map(|schema_elems| {
        let schemas = NamedElementSchemas {
            elements: schema_elems
                .iter()
                .enumerate()
                .map(|(i, (s, _))| NamedElementSchema {
                    name: format!("m{i}"),
                    schema: s.clone(),
                })
                .collect(),
        };
        let num_schemas = schema_elems.len();
        // Generate 0-6 instances, each picking a random schema index
        let schema_and_generators: Vec<(ElementSchema, ElementValue)> = schema_elems;
        (
            Just(schemas),
            Just(schema_and_generators),
            prop::collection::vec(0..num_schemas, 0..7),
        )
    }).prop_flat_map(|(schemas, schema_elems, indices)| {
        // For each chosen index, regenerate a value matching that schema
        let schema_types: Vec<ElementSchema> = schema_elems.iter().map(|(s, _)| s.clone()).collect();
        let strats: Vec<_> = indices.iter().map(|&idx| {
            let s = schema_types[idx].clone();
            let name = format!("m{idx}");
            let schema_index = idx as u32;
            match s {
                ElementSchema::ComponentModel(ref cms) => {
                    let typ = cms.element_type.clone();
                    arb_vat_for_type(typ).prop_map(move |val| {
                        NamedElementValue {
                            name: name.clone(),
                            value: ElementValue::ComponentModel(ComponentModelElementValue {
                                value: val,
                            }),
                            schema_index,
                        }
                    }).boxed()
                }
                ElementSchema::UnstructuredText(_) => {
                    arb_text_reference().prop_map(move |text_ref| {
                        NamedElementValue {
                            name: name.clone(),
                            value: ElementValue::UnstructuredText(UnstructuredTextElementValue {
                                value: text_ref,
                                descriptor: TextDescriptor { restrictions: None },
                            }),
                            schema_index,
                        }
                    }).boxed()
                }
                ElementSchema::UnstructuredBinary(_) => {
                    arb_binary_reference().prop_map(move |bin_ref| {
                        NamedElementValue {
                            name: name.clone(),
                            value: ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                                value: bin_ref,
                                descriptor: BinaryDescriptor { restrictions: None },
                            }),
                            schema_index,
                        }
                    }).boxed()
                }
            }
        }).collect();

        (Just(schemas), strats)
    }).prop_map(|(schemas, named_values)| {
        (
            DataSchema::Multimodal(schemas),
            DataValue::Multimodal(NamedElementValues { elements: named_values }),
        )
    })
}

/// Generate a Value matching a specific AnalysedType (recursive).
fn arb_value_for_type(typ: AnalysedType) -> BoxedStrategy<Value> {
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
        AnalysedType::Option(type_opt) => {
            let inner = arb_value_for_type(*type_opt.inner);
            prop_oneof![
                inner.prop_map(|v| Value::Option(Some(Box::new(v)))),
                Just(Value::Option(None)),
            ]
            .boxed()
        }
        AnalysedType::List(type_list) => {
            let inner = arb_value_for_type(*type_list.inner);
            prop::collection::vec(inner, 0..4)
                .prop_map(Value::List)
                .boxed()
        }
        AnalysedType::Record(type_record) => {
            let field_strats: Vec<_> = type_record
                .fields
                .iter()
                .map(|f| arb_value_for_type(f.typ.clone()))
                .collect();
            field_strats
                .prop_map(Value::Record)
                .boxed()
        }
        AnalysedType::Tuple(type_tuple) => {
            let item_strats: Vec<_> = type_tuple
                .items
                .iter()
                .map(|t| arb_value_for_type(t.clone()))
                .collect();
            item_strats
                .prop_map(Value::Tuple)
                .boxed()
        }
        AnalysedType::Variant(type_variant) => {
            let num_cases = type_variant.cases.len();
            (0..num_cases)
                .prop_flat_map(move |chosen| {
                    let case_type = type_variant.cases[chosen].typ.clone();
                    match case_type {
                        Some(payload_type) => arb_value_for_type(payload_type)
                            .prop_map(move |v| Value::Variant {
                                case_idx: chosen as u32,
                                case_value: Some(Box::new(v)),
                            })
                            .boxed(),
                        None => Just(Value::Variant {
                            case_idx: chosen as u32,
                            case_value: None,
                        })
                        .boxed(),
                    }
                })
                .boxed()
        }
        AnalysedType::Enum(type_enum) => {
            let n = type_enum.cases.len() as u32;
            (0..n).prop_map(Value::Enum).boxed()
        }
        AnalysedType::Flags(type_flags) => {
            let n = type_flags.names.len();
            prop::collection::vec(any::<bool>(), n..=n)
                .prop_map(Value::Flags)
                .boxed()
        }
        AnalysedType::Result(type_res) => {
            let ok_strat: BoxedStrategy<Value> = match type_res.ok {
                Some(ref ok_type) => arb_value_for_type(*ok_type.clone())
                    .prop_map(|v| Value::Result(Ok(Some(Box::new(v)))))
                    .boxed(),
                None => Just(Value::Result(Ok(None))).boxed(),
            };
            let err_strat: BoxedStrategy<Value> = match type_res.err {
                Some(ref err_type) => arb_value_for_type(*err_type.clone())
                    .prop_map(|v| Value::Result(Err(Some(Box::new(v)))))
                    .boxed(),
                None => Just(Value::Result(Err(None))).boxed(),
            };
            prop_oneof![ok_strat, err_strat].boxed()
        }
        _ => any::<u32>().prop_map(Value::U32).boxed(),
    }
}

/// Generate a ValueAndType matching a specific AnalysedType.
fn arb_vat_for_type(typ: AnalysedType) -> BoxedStrategy<ValueAndType> {
    let t = typ.clone();
    arb_value_for_type(typ)
        .prop_map(move |v| ValueAndType::new(v, t.clone()))
        .boxed()
}

/// Generate either a Tuple or Multimodal DataSchema+DataValue pair.
fn arb_data() -> impl Strategy<Value = (DataSchema, DataValue)> {
    prop_oneof![
        arb_tuple_data(),
        arb_multimodal_data(),
    ]
}

// ── Proptest roundtrips ─────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 500, .. ProptestConfig::default()
    })]

    // ── Primitive roundtrips ────────────────────────────────────────────

    #[test]
    fn roundtrip_u32(x in 0u32..u32::MAX) {
        let schema = tuple_schema(vec![("x", cm(u32()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: x.into_value_and_type(),
            })],
        });
        roundtrip(&data, &schema);
    }

    #[test]
    fn roundtrip_s64(x in i64::MIN..i64::MAX) {
        let schema = tuple_schema(vec![("x", cm(s64()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::S64(x), s64()),
            })],
        });
        roundtrip(&data, &schema);
    }

    #[test]
    fn roundtrip_f64_normal(x in proptest::num::f64::NORMAL) {
        let schema = tuple_schema(vec![("x", cm(f64()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::F64(x), f64()),
            })],
        });
        roundtrip(&data, &schema);
    }

    #[test]
    fn roundtrip_f32_finite(x in finite_f32()) {
        let schema = tuple_schema(vec![("x", cm(f32()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::F32(x), f32()),
            })],
        });
        roundtrip(&data, &schema);
    }

    #[test]
    fn roundtrip_string(s in ".*") {
        let schema = tuple_schema(vec![("x", cm(str()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::String(s), str()),
            })],
        });
        roundtrip(&data, &schema);
    }

    #[test]
    fn roundtrip_char(c in arb_char()) {
        let schema = tuple_schema(vec![("x", cm(chr()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::Char(c), chr()),
            })],
        });
        roundtrip(&data, &schema);
    }

    #[test]
    fn roundtrip_bool(b in any::<bool>()) {
        let schema = tuple_schema(vec![("x", cm(bool()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::Bool(b), bool()),
            })],
        });
        roundtrip(&data, &schema);
    }

    // ── Leaf CM type roundtrip ──────────────────────────────────────────

    #[test]
    fn roundtrip_leaf_cm_value((typ, val) in leaf_type_and_value()) {
        let schema = tuple_schema(vec![("x", cm(typ.clone()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(val, typ),
            })],
        });
        roundtrip(&data, &schema);
    }

    // ── Recursive CM type roundtrip ─────────────────────────────────────

    #[test]
    fn roundtrip_complex_cm_value((typ, val) in arb_type_and_value()) {
        let schema = tuple_schema(vec![("x", cm(typ.clone()))]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(val, typ),
            })],
        });
        roundtrip(&data, &schema);
    }

    // ── Multi-element tuple roundtrip ───────────────────────────────────

    #[test]
    fn roundtrip_multi_element_tuple(
        (typ1, val1) in leaf_type_and_value(),
        (typ2, val2) in leaf_type_and_value(),
        (typ3, val3) in leaf_type_and_value(),
    ) {
        let schema = tuple_schema(vec![
            ("a", cm(typ1.clone())),
            ("b", cm(typ2.clone())),
            ("c", cm(typ3.clone())),
        ]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType::new(val1, typ1),
                }),
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType::new(val2, typ2),
                }),
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType::new(val3, typ3),
                }),
            ],
        });
        roundtrip(&data, &schema);
    }

    // ── Full arbitrary tuple roundtrip ──────────────────────────────────

    #[test]
    fn roundtrip_arbitrary_tuple((schema, data) in arb_tuple_data()) {
        roundtrip(&data, &schema);
    }

    // ── Full arbitrary multimodal roundtrip ─────────────────────────────

    #[test]
    fn roundtrip_arbitrary_multimodal((schema, data) in arb_multimodal_data()) {
        roundtrip(&data, &schema);
    }

    // ── Full arbitrary data (tuple or multimodal) roundtrip ─────────────

    #[test]
    fn roundtrip_arbitrary_data((schema, data) in arb_data()) {
        roundtrip(&data, &schema);
    }

    // ── Unstructured text roundtrip ─────────────────────────────────────

    #[test]
    fn roundtrip_arbitrary_text(text_ref in arb_text_reference()) {
        let schema = tuple_schema(vec![("t", text_elem_schema())]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::UnstructuredText(
                UnstructuredTextElementValue {
                    value: text_ref,
                    descriptor: TextDescriptor { restrictions: None },
                },
            )],
        });
        roundtrip(&data, &schema);
    }

    // ── Unstructured binary roundtrip ───────────────────────────────────

    #[test]
    fn roundtrip_arbitrary_binary(bin_ref in arb_binary_reference()) {
        let schema = tuple_schema(vec![("b", binary_elem_schema())]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![ElementValue::UnstructuredBinary(
                UnstructuredBinaryElementValue {
                    value: bin_ref,
                    descriptor: BinaryDescriptor { restrictions: None },
                },
            )],
        });
        roundtrip(&data, &schema);
    }

    // ── Normalization properties ────────────────────────────────────────

    #[test]
    fn normalize_is_idempotent(s in "[a-z0-9(),\\[\\]\"a-z ]{0,50}") {
        let n1 = normalize_structural(&s);
        let n2 = normalize_structural(&n1);
        prop_assert_eq!(n1, n2);
    }

    #[test]
    fn format_then_normalize_is_identity((_schema, data) in arb_data()) {
        if let Ok(formatted) = format_structural(&data) {
            let normalized = normalize_structural(&formatted);
            prop_assert_eq!(&formatted, &normalized);
        }
    }

    // ── Mixed tuple with all element kinds ──────────────────────────────

    #[test]
    fn roundtrip_mixed_tuple(
        (typ, val) in arb_type_and_value(),
        text_ref in arb_text_reference(),
        bin_ref in arb_binary_reference(),
    ) {
        let schema = tuple_schema(vec![
            ("cm", cm(typ.clone())),
            ("txt", text_elem_schema()),
            ("bin", binary_elem_schema()),
        ]);
        let data = DataValue::Tuple(ElementValues {
            elements: vec![
                ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType::new(val, typ),
                }),
                ElementValue::UnstructuredText(UnstructuredTextElementValue {
                    value: text_ref,
                    descriptor: TextDescriptor { restrictions: None },
                }),
                ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                    value: bin_ref,
                    descriptor: BinaryDescriptor { restrictions: None },
                }),
            ],
        });
        roundtrip(&data, &schema);
    }
}

// ── Trailing option defaults ────────────────────────────────────────────────

#[test]
fn record_trailing_options_all_missing() {
    let typ = record(vec![
        field("x", u32()),
        field("y", option(str())),
        field("z", option(bool())),
    ]);
    let schema = tuple_schema(vec![("r", cm(typ.clone()))]);
    let parsed = parse_structural("(42)", &schema).unwrap();
    let expected = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Record(vec![
                    Value::U32(42),
                    Value::Option(None),
                    Value::Option(None),
                ]),
                typ,
            ),
        })],
    });
    assert_eq!(parsed, expected);
}

#[test]
fn record_trailing_options_partial() {
    let typ = record(vec![
        field("x", u32()),
        field("y", option(str())),
        field("z", option(bool())),
    ]);
    let schema = tuple_schema(vec![("r", cm(typ.clone()))]);
    let parsed = parse_structural("(42,s(\"hi\"))", &schema).unwrap();
    let expected = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Record(vec![
                    Value::U32(42),
                    Value::Option(Some(Box::new(Value::String("hi".to_string())))),
                    Value::Option(None),
                ]),
                typ,
            ),
        })],
    });
    assert_eq!(parsed, expected);
}

#[test]
fn tuple_trailing_options_all_missing() {
    let typ = tuple(vec![u32(), option(str()), option(bool())]);
    let schema = tuple_schema(vec![("t", cm(typ.clone()))]);
    let parsed = parse_structural("(42)", &schema).unwrap();
    let expected = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Tuple(vec![
                    Value::U32(42),
                    Value::Option(None),
                    Value::Option(None),
                ]),
                typ,
            ),
        })],
    });
    assert_eq!(parsed, expected);
}

#[test]
fn tuple_elems_trailing_options_missing() {
    let schema = tuple_schema(vec![
        ("x", cm(u32())),
        ("y", cm(option(str()))),
        ("z", cm(option(bool()))),
    ]);
    let parsed = parse_structural("42", &schema).unwrap();
    let expected = DataValue::Tuple(ElementValues {
        elements: vec![
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::U32(42), u32()),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::Option(None), option(str())),
            }),
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(Value::Option(None), option(bool())),
            }),
        ],
    });
    assert_eq!(parsed, expected);
}

#[test]
fn record_non_option_trailing_field_fails() {
    let typ = record(vec![field("x", u32()), field("y", str())]);
    let schema = tuple_schema(vec![("r", cm(typ))]);
    assert!(parse_structural("(42)", &schema).is_err());
}

#[test]
fn record_mixed_non_option_after_option_fails() {
    let typ = record(vec![
        field("x", u32()),
        field("y", option(str())),
        field("z", bool()),
    ]);
    let schema = tuple_schema(vec![("r", cm(typ))]);
    // Cannot default because bool (non-option) is trailing
    assert!(parse_structural("(42)", &schema).is_err());
}

#[test]
fn format_rejects_depth_exceeding_max() {
    let mut typ = u32();
    let mut val: Value = Value::U32(0);
    for _ in 0..32 {
        typ = option(typ);
        val = Value::Option(Some(Box::new(val)));
    }
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(val, typ.clone()),
        })],
    });
    assert_eq!(
        format_structural(&data),
        Err(StructuralFormatError::MaxDepthExceeded(32))
    );
}

#[test]
fn parse_rejects_depth_exceeding_max() {
    let mut typ = u32();
    for _ in 0..32 {
        typ = option(typ);
    }
    let schema = tuple_schema(vec![("x", cm(typ))]);
    let input = format!("{}0{}", "s(".repeat(32), ")".repeat(32));
    let result = parse_structural(&input, &schema);
    assert!(
        matches!(result, Err(StructuralFormatError::MaxDepthExceeded(32))),
        "Expected MaxDepthExceeded(32), got: {result:?}"
    );
}

#[test]
fn depth_at_boundary_succeeds() {
    let mut typ = u32();
    let mut val: Value = Value::U32(0);
    for _ in 0..31 {
        typ = option(typ);
        val = Value::Option(Some(Box::new(val)));
    }
    let schema = tuple_schema(vec![("x", cm(typ.clone()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(val, typ),
        })],
    });
    let formatted = format_structural(&data).expect("format should succeed at boundary");
    let parsed = parse_structural(&formatted, &schema).expect("parse should succeed at boundary");
    assert_eq!(
        parsed,
        data,
        "Roundtrip should succeed at depth boundary"
    );
}

#[test]
fn format_rejects_handle_type() {
    let handle_type = AnalysedType::Handle(TypeHandle {
        name: None,
        owner: None,
        resource_id: AnalysedResourceId(0),
        mode: AnalysedResourceMode::Owned,
    });
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Handle {
                    uri: "test".to_string(),
                    resource_id: 0,
                },
                handle_type,
            ),
        })],
    });
    assert_eq!(
        format_structural(&data),
        Err(StructuralFormatError::HandleType)
    );
}

#[test]
fn parse_rejects_handle_type() {
    let handle_type = AnalysedType::Handle(TypeHandle {
        name: None,
        owner: None,
        resource_id: AnalysedResourceId(0),
        mode: AnalysedResourceMode::Owned,
    });
    let schema = tuple_schema(vec![("h", cm(handle_type))]);
    let result = parse_structural("anything", &schema);
    assert_eq!(result, Err(StructuralFormatError::HandleType));
}
