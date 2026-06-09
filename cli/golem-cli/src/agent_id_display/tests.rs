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

use super::SourceLanguage;
use super::*;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::schema::adapters::{
    analysed_type_to_schema_graph, value_and_type_to_typed_schema_value,
};
use golem_common::schema::agent::{InputSchema, NamedField, ParsedAgentId};
use golem_common::schema::graph::{SchemaGraph, TypedSchemaValue};
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};
use golem_wasm::analysis::proptest_strategies;
use golem_wasm::analysis::{
    AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF64,
    TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult, TypeS32, TypeStr, TypeTuple, TypeU32,
    TypeVariant,
};
use golem_wasm::{Value, ValueAndType};

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Convert a legacy `(Value, AnalysedType)` pair to a `TypedSchemaValue`.
fn to_typed(value: Value, typ: AnalysedType) -> TypedSchemaValue {
    let vat = ValueAndType::new(value, typ);
    value_and_type_to_typed_schema_value(&vat).expect("converting value+type to schema")
}

/// Build an `InputSchema` carrying a single user-supplied parameter `p`
/// of the given schema type.
fn single_param_input_schema(ty: SchemaType) -> InputSchema {
    InputSchema::parameters(vec![NamedField::user_supplied("p", ty)])
}

/// Round-trip a single `(Value, AnalysedType)` pair through the given
/// language: convert to schema layer, render, parse back, and assert
/// the schema-typed parsed result equals the seed.
fn round_trip(value: Value, typ: AnalysedType, lang: SourceLanguage) {
    let typed = to_typed(value.clone(), typ.clone());
    let graph = typed.graph().clone();
    let root_ty = typed.root_type().clone();
    let original_value = typed.value().clone();

    let rendered = render_schema_value(&graph, &root_ty, &original_value, &lang);
    let input_schema = single_param_input_schema(root_ty.clone());
    let parsed = parse_agent_id_params(&rendered, &graph, &input_schema, &lang)
        .unwrap_or_else(|e| panic!("parse failed for rendered='{rendered}': {e}"));
    match parsed {
        SchemaValue::Record { fields } => {
            assert_eq!(fields.len(), 1, "expected single-field record");
            assert_eq!(
                fields[0], original_value,
                "round-trip mismatch via {lang} for rendered='{rendered}'"
            );
        }
        other => panic!("parser did not return a Record: {other:?}"),
    }
}

fn round_trip_all(value: Value, typ: AnalysedType) {
    round_trip(value.clone(), typ.clone(), SourceLanguage::Rust);
    round_trip(value.clone(), typ.clone(), SourceLanguage::TypeScript);
    round_trip(value.clone(), typ.clone(), SourceLanguage::Scala);
    round_trip(value, typ, SourceLanguage::MoonBit);
}

// ── Primitive round-trips ───────────────────────────────────────────────────

#[test]
fn round_trip_bool() {
    round_trip_all(Value::Bool(true), AnalysedType::Bool(TypeBool));
    round_trip_all(Value::Bool(false), AnalysedType::Bool(TypeBool));
}

#[test]
fn round_trip_integers() {
    round_trip_all(Value::U32(42), AnalysedType::U32(TypeU32));
    round_trip_all(Value::S32(-7), AnalysedType::S32(TypeS32));
    round_trip_all(Value::S32(0), AnalysedType::S32(TypeS32));
}

#[test]
fn round_trip_string() {
    round_trip_all(
        Value::String("hello world".into()),
        AnalysedType::Str(TypeStr),
    );
    round_trip_all(
        Value::String("line\nnewline".into()),
        AnalysedType::Str(TypeStr),
    );
}

#[test]
fn round_trip_char_rust_scala_moonbit() {
    // TypeScript renders char as a one-character string; this is covered
    // separately because the parser only accepts what was rendered.
    round_trip(
        Value::Char('a'),
        AnalysedType::Chr(TypeChr),
        SourceLanguage::Rust,
    );
    round_trip(
        Value::Char('\n'),
        AnalysedType::Chr(TypeChr),
        SourceLanguage::Rust,
    );
    round_trip(
        Value::Char('a'),
        AnalysedType::Chr(TypeChr),
        SourceLanguage::Scala,
    );
    round_trip(
        Value::Char('a'),
        AnalysedType::Chr(TypeChr),
        SourceLanguage::MoonBit,
    );
}

#[test]
fn ts_round_trip_char() {
    round_trip(
        Value::Char('a'),
        AnalysedType::Chr(TypeChr),
        SourceLanguage::TypeScript,
    );
}

#[test]
fn round_trip_float() {
    round_trip_all(Value::F64(2.71), AnalysedType::F64(TypeF64));
    round_trip_all(Value::F64(f64::INFINITY), AnalysedType::F64(TypeF64));
    round_trip_all(Value::F64(f64::NEG_INFINITY), AnalysedType::F64(TypeF64));

    // NaN round-trips structurally but cannot be compared via equality.
    for lang in [
        SourceLanguage::Rust,
        SourceLanguage::TypeScript,
        SourceLanguage::Scala,
        SourceLanguage::MoonBit,
    ] {
        let typed = to_typed(Value::F64(f64::NAN), AnalysedType::F64(TypeF64));
        let graph = typed.graph().clone();
        let root_ty = typed.root_type().clone();
        let rendered = render_schema_value(&graph, &root_ty, typed.value(), &lang);
        let input_schema = single_param_input_schema(root_ty.clone());
        let parsed = parse_agent_id_params(&rendered, &graph, &input_schema, &lang).unwrap();
        match parsed {
            SchemaValue::Record { fields } => match fields.into_iter().next().unwrap() {
                SchemaValue::F64(v) => assert!(v.is_nan(), "expected NaN via {lang}"),
                other => panic!("expected F64, got {other:?}"),
            },
            _ => panic!("expected Record"),
        }
    }
}

// ── Composite round-trips ───────────────────────────────────────────────────

fn record_typ() -> AnalysedType {
    AnalysedType::Record(TypeRecord {
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
    })
}

#[test]
fn round_trip_record() {
    let typ = record_typ();
    let val = Value::Record(vec![Value::U32(42), Value::String("hi".into())]);
    round_trip_all(val, typ);
}

fn variant_typ() -> AnalysedType {
    AnalysedType::Variant(TypeVariant {
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
    })
}

#[test]
fn round_trip_variant() {
    round_trip_all(
        Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::U32(99))),
        },
        variant_typ(),
    );
    round_trip_all(
        Value::Variant {
            case_idx: 1,
            case_value: None,
        },
        variant_typ(),
    );
}

#[test]
fn round_trip_enum() {
    let typ = AnalysedType::Enum(TypeEnum {
        name: Some("color".to_string()),
        owner: None,
        cases: vec!["red".to_string(), "green".to_string(), "blue".to_string()],
    });
    round_trip_all(Value::Enum(1), typ);
}

#[test]
fn round_trip_option() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    round_trip_all(Value::Option(Some(Box::new(Value::U32(42)))), typ.clone());
    round_trip_all(Value::Option(None), typ);
}

#[test]
fn round_trip_result() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    round_trip_all(
        Value::Result(Ok(Some(Box::new(Value::U32(42))))),
        typ.clone(),
    );
    round_trip_all(
        Value::Result(Err(Some(Box::new(Value::String("oops".into()))))),
        typ,
    );
}

#[test]
fn round_trip_flags() {
    let typ = AnalysedType::Flags(TypeFlags {
        name: Some("perms".to_string()),
        owner: None,
        names: vec![
            "read".to_string(),
            "write".to_string(),
            "execute".to_string(),
        ],
    });
    round_trip_all(Value::Flags(vec![true, false, true]), typ);
}

#[test]
fn round_trip_tuple() {
    let typ = AnalysedType::Tuple(TypeTuple {
        name: None,
        owner: None,
        items: vec![AnalysedType::U32(TypeU32), AnalysedType::Str(TypeStr)],
    });
    round_trip_all(
        Value::Tuple(vec![Value::U32(1), Value::String("x".into())]),
        typ,
    );
}

#[test]
fn round_trip_list() {
    let typ = AnalysedType::List(TypeList {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    round_trip_all(
        Value::List(vec![Value::U32(1), Value::U32(2), Value::U32(3)]),
        typ,
    );
}

// ── render_agent_id tests ────────────────────────────────────────────────────

fn build_parsed_agent_id(
    agent_type: &str,
    params_value: SchemaValue,
    schema_ty: SchemaType,
) -> ParsedAgentId {
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::schema_type::NamedFieldType {
                name: "p".to_string(),
                body: schema_ty,
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![params_value],
        },
    );
    ParsedAgentId::new(AgentTypeName(agent_type.to_string()), typed, None)
}

#[test]
fn render_agent_id_format() {
    let parsed = build_parsed_agent_id("my-agent", SchemaValue::U32(42), SchemaType::u32());
    let result = render_agent_id(&parsed, &SourceLanguage::Rust);
    assert_eq!(result, "my-agent(42)");
}

#[test]
fn render_agent_id_with_phantom() {
    use uuid::Uuid;
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::schema_type::NamedFieldType {
                name: "p".to_string(),
                body: SchemaType::u32(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::U32(42)],
        },
    );
    let uuid = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let parsed = ParsedAgentId::new(AgentTypeName("my-agent".to_string()), typed, Some(uuid));
    let result = render_agent_id(&parsed, &SourceLanguage::Rust);
    assert_eq!(result, "my-agent(42)[12345678-1234-1234-1234-123456789012]");
}

// ── parse_agent_id_params specifics ─────────────────────────────────────────

#[test]
fn rust_language_specific_parsed_first() {
    let typ = AnalysedType::Option(TypeOption {
        name: None,
        owner: None,
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    let (graph, schema_ty) = (
        analysed_type_to_schema_graph(&typ).unwrap(),
        analysed_type_to_schema_graph(&typ).unwrap().root,
    );
    let input_schema = single_param_input_schema(schema_ty);
    let parsed =
        parse_agent_id_params("Some(42)", &graph, &input_schema, &SourceLanguage::Rust).unwrap();
    assert!(matches!(parsed, SchemaValue::Record { .. }));
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
    let graph = analysed_type_to_schema_graph(&typ).unwrap();
    let input_schema = single_param_input_schema(graph.root.clone());
    let parsed =
        parse_agent_id_params("MyCase(5)", &graph, &input_schema, &SourceLanguage::Rust).unwrap();
    match parsed {
        SchemaValue::Record { fields } => match fields.into_iter().next().unwrap() {
            SchemaValue::Variant(VariantValuePayload { case, payload }) => {
                assert_eq!(case, 0);
                assert!(matches!(payload.as_deref(), Some(SchemaValue::U32(5))));
            }
            other => panic!("expected Variant, got {other:?}"),
        },
        _ => panic!("expected Record"),
    }
}

#[test]
fn rust_result_ok_parsed() {
    let typ = AnalysedType::Result(TypeResult {
        name: None,
        owner: None,
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });
    let graph = analysed_type_to_schema_graph(&typ).unwrap();
    let input_schema = single_param_input_schema(graph.root.clone());
    let parsed =
        parse_agent_id_params("Ok(42)", &graph, &input_schema, &SourceLanguage::Rust).unwrap();
    match parsed {
        SchemaValue::Record { fields } => match fields.into_iter().next().unwrap() {
            SchemaValue::Result(ResultValuePayload::Ok { value }) => {
                assert!(matches!(value.as_deref(), Some(SchemaValue::U32(42))));
            }
            other => panic!("expected Result::Ok, got {other:?}"),
        },
        _ => panic!("expected Record"),
    }
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
    let graph = analysed_type_to_schema_graph(&typ).unwrap();
    let input_schema = single_param_input_schema(graph.root.clone());
    let parsed = parse_agent_id_params(
        r#"{ myField: 10, anotherField: "hi" }"#,
        &graph,
        &input_schema,
        &SourceLanguage::TypeScript,
    )
    .unwrap();
    match parsed {
        SchemaValue::Record { fields } => match fields.into_iter().next().unwrap() {
            SchemaValue::Record { fields: inner } => {
                assert_eq!(inner.len(), 2);
                assert!(matches!(inner[0], SchemaValue::U32(10)));
                assert!(matches!(&inner[1], SchemaValue::String(s) if s == "hi"));
            }
            other => panic!("expected inner Record, got {other:?}"),
        },
        _ => panic!("expected Record"),
    }
}

#[test]
fn source_language_from_str() {
    assert_eq!(SourceLanguage::from("rust"), SourceLanguage::Rust);
    assert_eq!(SourceLanguage::from("Rust"), SourceLanguage::Rust);
    assert_eq!(SourceLanguage::from("RUST"), SourceLanguage::Rust);
    assert_eq!(SourceLanguage::from("  rust  "), SourceLanguage::Rust);
    assert_eq!(
        SourceLanguage::from("typescript"),
        SourceLanguage::TypeScript
    );
    assert_eq!(
        SourceLanguage::from("TypeScript"),
        SourceLanguage::TypeScript
    );
    assert_eq!(SourceLanguage::from("ts"), SourceLanguage::TypeScript);
    assert_eq!(SourceLanguage::from("TS"), SourceLanguage::TypeScript);
    assert_eq!(
        SourceLanguage::from("go"),
        SourceLanguage::Other("go".to_string())
    );
    assert_eq!(
        SourceLanguage::from(""),
        SourceLanguage::Other("".to_string())
    );
}

// ── Property-based round-trips ──────────────────────────────────────────────

use proptest::prelude::*;

fn leaf_type_and_value() -> impl Strategy<Value = (AnalysedType, Value)> {
    proptest_strategies::leaf_type_and_value()
}

fn arb_type_and_value() -> impl Strategy<Value = (AnalysedType, Value)> {
    proptest_strategies::arb_type_and_value()
}

fn proptest_round_trip(typ: AnalysedType, val: Value, lang: SourceLanguage) {
    let typed = to_typed(val, typ);
    let graph = typed.graph().clone();
    let root_ty = typed.root_type().clone();
    let original_value = typed.value().clone();

    let rendered = render_schema_value(&graph, &root_ty, &original_value, &lang);
    let input_schema = single_param_input_schema(root_ty.clone());
    let parsed = parse_agent_id_params(&rendered, &graph, &input_schema, &lang)
        .unwrap_or_else(|e| panic!("{lang} parse failed for '{rendered}': {e}"));
    match parsed {
        SchemaValue::Record { fields } => {
            assert_eq!(fields.len(), 1);
            assert_eq!(
                fields[0], original_value,
                "round-trip mismatch via {lang} for rendered='{rendered}'"
            );
        }
        _ => panic!("expected Record from parser"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 200, .. ProptestConfig::default()
    })]

    #[test]
    fn proptest_rust_leaf_roundtrip((typ, val) in leaf_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::Rust);
    }

    #[test]
    fn proptest_ts_leaf_roundtrip((typ, val) in leaf_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::TypeScript);
    }

    #[test]
    fn proptest_scala_leaf_roundtrip((typ, val) in leaf_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::Scala);
    }

    #[test]
    fn proptest_moonbit_leaf_roundtrip((typ, val) in leaf_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::MoonBit);
    }

    #[test]
    fn proptest_rust_complex_roundtrip((typ, val) in arb_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::Rust);
    }

    #[test]
    fn proptest_ts_complex_roundtrip((typ, val) in arb_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::TypeScript);
    }

    #[test]
    fn proptest_scala_complex_roundtrip((typ, val) in arb_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::Scala);
    }

    #[test]
    fn proptest_moonbit_complex_roundtrip((typ, val) in arb_type_and_value()) {
        proptest_round_trip(typ, val, SourceLanguage::MoonBit);
    }
}

// ── Rich scalar round-trips ─────────────────────────────────────────────────
//
// Rich schema variants (Text/Binary/Path/Url/Datetime/Duration/Quantity)
// have no legacy `AnalysedType` counterpart, so we construct
// `SchemaType + SchemaValue` directly and round-trip via
// `render_schema_value` + `parse_value_for_language` per language.

fn rich_round_trip(ty: SchemaType, value: SchemaValue, lang: SourceLanguage) {
    let graph = SchemaGraph::anonymous(ty.clone());
    let rendered = render_schema_value(&graph, &ty, &value, &lang);
    let parsed = parse_value_for_language(&rendered, &graph, &ty, &lang)
        .unwrap_or_else(|e| panic!("rich-scalar parse failed for {lang:?} '{rendered}': {e}"));
    assert_eq!(
        parsed, value,
        "rich-scalar round-trip mismatch via {lang:?} for rendered='{rendered}'"
    );
}

fn rich_round_trip_all(ty: SchemaType, value: SchemaValue) {
    for lang in [
        SourceLanguage::Rust,
        SourceLanguage::TypeScript,
        SourceLanguage::Scala,
        SourceLanguage::MoonBit,
    ] {
        rich_round_trip(ty.clone(), value.clone(), lang);
    }
}

#[test]
fn round_trip_text_no_language() {
    use golem_common::schema::schema_type::TextRestrictions;
    use golem_common::schema::schema_value::TextValuePayload;
    rich_round_trip_all(
        SchemaType::text(TextRestrictions::default()),
        SchemaValue::Text(TextValuePayload {
            text: "hello world".into(),
            language: None,
        }),
    );
}

#[test]
fn round_trip_text_with_language() {
    use golem_common::schema::schema_type::TextRestrictions;
    use golem_common::schema::schema_value::TextValuePayload;
    rich_round_trip_all(
        SchemaType::text(TextRestrictions::default()),
        SchemaValue::Text(TextValuePayload {
            text: "bonjour".into(),
            language: Some("fr".into()),
        }),
    );
}

#[test]
fn round_trip_text_with_special_chars() {
    use golem_common::schema::schema_type::TextRestrictions;
    use golem_common::schema::schema_value::TextValuePayload;
    rich_round_trip_all(
        SchemaType::text(TextRestrictions::default()),
        SchemaValue::Text(TextValuePayload {
            text: "line\nwith \"quotes\" and \\ backslash".into(),
            language: None,
        }),
    );
}

#[test]
fn round_trip_binary() {
    use golem_common::schema::schema_type::BinaryRestrictions;
    use golem_common::schema::schema_value::BinaryValuePayload;
    rich_round_trip_all(
        SchemaType::binary(BinaryRestrictions::default()),
        SchemaValue::Binary(BinaryValuePayload {
            bytes: b"\x00\x01hello\xff".to_vec(),
            mime_type: Some("application/octet-stream".into()),
        }),
    );
}

#[test]
fn round_trip_path() {
    use golem_common::schema::schema_type::{PathDirection, PathKind, PathSpec};
    rich_round_trip_all(
        SchemaType::path(PathSpec {
            direction: PathDirection::Input,
            kind: PathKind::Any,
            allowed_mime_types: None,
            allowed_extensions: None,
        }),
        SchemaValue::Path {
            path: "/tmp/some file.txt".into(),
        },
    );
}

#[test]
fn round_trip_url() {
    use golem_common::schema::schema_type::UrlRestrictions;
    rich_round_trip_all(
        SchemaType::url(UrlRestrictions::default()),
        SchemaValue::Url {
            url: "https://example.com/a?b=c&d=e".into(),
        },
    );
}

#[test]
fn round_trip_datetime() {
    use chrono::{TimeZone, Utc};
    rich_round_trip_all(
        SchemaType::datetime(),
        SchemaValue::Datetime {
            value: Utc.with_ymd_and_hms(2025, 4, 12, 13, 14, 15).unwrap(),
        },
    );
}

#[test]
fn round_trip_duration() {
    use golem_common::schema::schema_value::DurationValuePayload;
    rich_round_trip_all(
        SchemaType::duration(),
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: 3_600_000_000_000,
        }),
    );
}

#[test]
fn round_trip_quantity() {
    use golem_common::schema::schema_type::{QuantitySpec, QuantityValue};
    rich_round_trip_all(
        SchemaType::quantity(QuantitySpec {
            base_unit: "kg".into(),
            allowed_suffixes: Vec::new(),
            min: None,
            max: None,
        }),
        SchemaValue::Quantity(QuantityValue {
            // `15` with scale `1` = 1.5; this is the canonical normalised
            // form so the parser produces the same struct verbatim.
            mantissa: 15,
            scale: 1,
            unit: "kg".into(),
        }),
    );
}

// ── parse_map round-trip ────────────────────────────────────────────────────
//
// The previous lexer used to swallow the `=>` arrow before reaching the
// raw `>` skip, so any rendered map was unparseable. Cover the round-trip
// directly here.

#[test]
fn round_trip_map() {
    let ty = SchemaType::map(SchemaType::string(), SchemaType::u32());
    let value = SchemaValue::Map {
        entries: vec![
            (SchemaValue::String("alpha".into()), SchemaValue::U32(1)),
            (SchemaValue::String("beta".into()), SchemaValue::U32(2)),
        ],
    };
    rich_round_trip_all(ty, value);
}

// ── Narrow integer overflow rejection ──────────────────────────────────────

#[test]
fn parse_rejects_u8_overflow() {
    let ty = SchemaType::u8();
    let graph = SchemaGraph::anonymous(ty.clone());
    let err = parse_value_for_language("256", &graph, &ty, &SourceLanguage::Rust)
        .expect_err("256 should not fit in u8");
    assert!(
        err.message.contains("u8"),
        "expected overflow message mentioning u8, got: {err}"
    );
}

#[test]
fn parse_rejects_s8_underflow() {
    let ty = SchemaType::s8();
    let graph = SchemaGraph::anonymous(ty.clone());
    let err = parse_value_for_language("-129", &graph, &ty, &SourceLanguage::Rust)
        .expect_err("-129 should not fit in s8");
    assert!(
        err.message.contains("s8"),
        "expected overflow message mentioning s8, got: {err}"
    );
}

#[test]
fn parse_rejects_f32_overflow() {
    let ty = SchemaType::f32();
    let graph = SchemaGraph::anonymous(ty.clone());
    let err = parse_value_for_language("1e100", &graph, &ty, &SourceLanguage::Rust)
        .expect_err("1e100 should not fit in f32");
    assert!(
        err.message.contains("f32"),
        "expected overflow message mentioning f32, got: {err}"
    );
}

// ── TypeScript type-parser round-trip ───────────────────────────────────────
//
// The TS renderer emits forms that the type parser needs to accept:
// `(a | b)[]` (parenthesised list element) and `Uint8Array` (for
// `list<u8>`).

fn ts_type_round_trip(ty: SchemaType) {
    let graph = SchemaGraph::anonymous(ty.clone());
    let rendered = render_type_for_language(&SourceLanguage::TypeScript, &graph, &ty, false);
    let (_, parsed) = parse_type_for_language(&rendered, &SourceLanguage::TypeScript)
        .unwrap_or_else(|e| panic!("TS type parse failed for '{rendered}': {e}"));
    assert_eq!(
        parsed, ty,
        "TS type round-trip mismatch for rendered='{rendered}'"
    );
}

#[test]
fn ts_type_round_trip_uint8array() {
    ts_type_round_trip(SchemaType::list(SchemaType::u8()));
}

#[test]
fn ts_type_round_trip_paren_grouped_list_element() {
    // `list<option<string>>` renders as `(string | undefined)[]`. (We
    // can't easily round-trip `option<u32>` because the TS renderer
    // collapses all numerics to `number` while the parser still uses
    // the WIT names — that's a pre-existing limitation, orthogonal to
    // the `(...)` grouping fix.)
    ts_type_round_trip(SchemaType::list(SchemaType::option(SchemaType::string())));
}
