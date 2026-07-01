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
use golem_common::base_model::agent::AgentTypeName;
use golem_common::schema::SchemaType;
use golem_common::schema::agent::{InputSchema, NamedField, ParsedAgentId};
use golem_common::schema::graph::{SchemaGraph, TypedSchemaValue};
use golem_common::schema::schema_type::{NamedFieldType, ResultSpec, VariantCaseType};
use golem_common::schema::schema_value::{ResultValuePayload, SchemaValue, VariantValuePayload};

fn single_param_input_schema(ty: SchemaType) -> InputSchema {
    InputSchema::parameters(vec![NamedField::user_supplied("p", ty)])
}

fn round_trip(value: SchemaValue, ty: SchemaType, lang: SourceLanguage) {
    let graph = SchemaGraph::anonymous(ty.clone());
    let rendered = render_schema_value(&graph, &ty, &value, &lang);
    let input_schema = single_param_input_schema(ty);
    let parsed = parse_agent_id_params(&rendered, &graph, &input_schema, &lang)
        .unwrap_or_else(|e| panic!("parse failed for rendered='{rendered}': {e}"));
    match parsed {
        SchemaValue::Record { fields } => {
            assert_eq!(fields.len(), 1, "expected single-field record");
            assert_eq!(
                fields[0], value,
                "round-trip mismatch via {lang} for rendered='{rendered}'"
            );
        }
        other => panic!("parser did not return a Record: {other:?}"),
    }
}

fn round_trip_all(value: SchemaValue, ty: SchemaType) {
    round_trip(value.clone(), ty.clone(), SourceLanguage::Rust);
    round_trip(value.clone(), ty.clone(), SourceLanguage::TypeScript);
    round_trip(value.clone(), ty.clone(), SourceLanguage::Scala);
    round_trip(value, ty, SourceLanguage::MoonBit);
}

#[test]
fn round_trip_primitives() {
    round_trip_all(SchemaValue::Bool(true), SchemaType::bool());
    round_trip_all(SchemaValue::U32(42), SchemaType::u32());
    round_trip_all(SchemaValue::S32(-7), SchemaType::s32());
    round_trip_all(
        SchemaValue::String("hello world".into()),
        SchemaType::string(),
    );
    round_trip_all(SchemaValue::F64(2.71), SchemaType::f64());
    round_trip(
        SchemaValue::Char('a'),
        SchemaType::char(),
        SourceLanguage::Rust,
    );
    round_trip(
        SchemaValue::Char('a'),
        SchemaType::char(),
        SourceLanguage::TypeScript,
    );
    round_trip(
        SchemaValue::Char('a'),
        SchemaType::char(),
        SourceLanguage::Scala,
    );
    round_trip(
        SchemaValue::Char('a'),
        SchemaType::char(),
        SourceLanguage::MoonBit,
    );
}

#[test]
fn round_trip_composites() {
    let record_ty = SchemaType::record(vec![
        NamedFieldType {
            name: "field-one".into(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "field-two".into(),
            body: SchemaType::string(),
            metadata: Default::default(),
        },
    ]);
    round_trip_all(
        SchemaValue::Record {
            fields: vec![SchemaValue::U32(42), SchemaValue::String("hi".into())],
        },
        record_ty,
    );

    let variant_ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "case-a".into(),
            payload: Some(SchemaType::u32()),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "case-b".into(),
            payload: None,
            metadata: Default::default(),
        },
    ]);
    round_trip_all(
        SchemaValue::Variant(VariantValuePayload {
            case: 0,
            payload: Some(Box::new(SchemaValue::U32(99))),
        }),
        variant_ty.clone(),
    );
    round_trip_all(
        SchemaValue::Variant(VariantValuePayload {
            case: 1,
            payload: None,
        }),
        variant_ty,
    );

    round_trip_all(
        SchemaValue::Enum { case: 1 },
        SchemaType::r#enum(vec!["red".into(), "green".into(), "blue".into()]),
    );
    round_trip_all(
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U32(42))),
        },
        SchemaType::option(SchemaType::u32()),
    );
    round_trip_all(
        SchemaValue::Option { inner: None },
        SchemaType::option(SchemaType::u32()),
    );
    round_trip_all(
        SchemaValue::Flags {
            bits: vec![true, false, true],
        },
        SchemaType::flags(vec!["read".into(), "write".into(), "execute".into()]),
    );
    round_trip_all(
        SchemaValue::Tuple {
            elements: vec![SchemaValue::U32(1), SchemaValue::String("x".into())],
        },
        SchemaType::tuple(vec![SchemaType::u32(), SchemaType::string()]),
    );
    round_trip_all(
        SchemaValue::List {
            elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)],
        },
        SchemaType::list(SchemaType::u32()),
    );
}

#[test]
fn round_trip_result() {
    let ty = SchemaType::result(ResultSpec {
        ok: Some(Box::new(SchemaType::u32())),
        err: Some(Box::new(SchemaType::string())),
    });
    round_trip_all(
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::U32(42))),
        }),
        ty.clone(),
    );
    round_trip_all(
        SchemaValue::Result(ResultValuePayload::Err {
            value: Some(Box::new(SchemaValue::String("oops".into()))),
        }),
        ty,
    );
}

fn build_parsed_agent_id(
    agent_type: &str,
    params_value: SchemaValue,
    schema_ty: SchemaType,
) -> ParsedAgentId {
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![NamedFieldType {
            name: "p".to_string(),
            body: schema_ty,
            metadata: Default::default(),
        }])),
        SchemaValue::Record {
            fields: vec![params_value],
        },
    );
    ParsedAgentId::new(AgentTypeName(agent_type.to_string()), typed, None)
}

#[test]
fn render_agent_id_format() {
    let parsed = build_parsed_agent_id("my-agent", SchemaValue::U32(42), SchemaType::u32());
    assert_eq!(
        render_agent_id(&parsed, &SourceLanguage::Rust),
        "my-agent(42)"
    );
}

#[test]
fn render_agent_id_with_phantom() {
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![NamedFieldType {
            name: "p".into(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        }])),
        SchemaValue::Record {
            fields: vec![SchemaValue::U32(42)],
        },
    );
    let uuid = uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let parsed = ParsedAgentId::new(AgentTypeName("my-agent".to_string()), typed, Some(uuid));
    assert_eq!(
        render_agent_id(&parsed, &SourceLanguage::Rust),
        "my-agent(42)[12345678-1234-1234-1234-123456789012]"
    );
}

#[test]
fn rust_language_specific_parsed_first() {
    let ty = SchemaType::option(SchemaType::u32());
    let graph = SchemaGraph::anonymous(ty.clone());
    let input_schema = single_param_input_schema(ty);
    let parsed =
        parse_agent_id_params("Some(42)", &graph, &input_schema, &SourceLanguage::Rust).unwrap();
    assert!(matches!(parsed, SchemaValue::Record { .. }));
}

#[test]
fn rust_variant_pascal_case() {
    let ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "MyCase".into(),
            payload: Some(SchemaType::u32()),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "OtherCase".into(),
            payload: None,
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let parsed = parse_agent_id_params(
        "MyCase(5)",
        &graph,
        &single_param_input_schema(ty),
        &SourceLanguage::Rust,
    )
    .unwrap();
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
    let ty = SchemaType::result(ResultSpec {
        ok: Some(Box::new(SchemaType::u32())),
        err: Some(Box::new(SchemaType::string())),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let parsed = parse_agent_id_params(
        "Ok(42)",
        &graph,
        &single_param_input_schema(ty),
        &SourceLanguage::Rust,
    )
    .unwrap();
    match parsed {
        SchemaValue::Record { fields } => match fields.into_iter().next().unwrap() {
            SchemaValue::Result(ResultValuePayload::Ok { value }) => {
                assert!(matches!(value.as_deref(), Some(SchemaValue::U32(42))))
            }
            other => panic!("expected Result::Ok, got {other:?}"),
        },
        _ => panic!("expected Record"),
    }
}

#[test]
fn ts_record_camel_case_fields() {
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "myField".into(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "anotherField".into(),
            body: SchemaType::string(),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let parsed = parse_agent_id_params(
        r#"{ myField: 10, anotherField: "hi" }"#,
        &graph,
        &single_param_input_schema(ty),
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
    assert_eq!(SourceLanguage::from("ts"), SourceLanguage::TypeScript);
    assert_eq!(
        SourceLanguage::from("go"),
        SourceLanguage::Other("go".to_string())
    );
}

#[test]
fn round_trip_rich_constructors() {
    use chrono::DateTime;
    use golem_common::schema::schema_type::{
        PathDirection, PathKind, PathSpec, QuantityValue, UrlRestrictions,
    };
    use golem_common::schema::schema_value::DurationValuePayload;

    round_trip_all(
        SchemaValue::Path {
            path: "/tmp/report.txt".into(),
        },
        SchemaType::path(PathSpec {
            direction: PathDirection::Input,
            kind: PathKind::File,
            allowed_mime_types: None,
            allowed_extensions: None,
        }),
    );
    round_trip_all(
        SchemaValue::Url {
            url: "https://example.com/a".into(),
        },
        SchemaType::url(UrlRestrictions::default()),
    );
    round_trip_all(
        SchemaValue::Datetime {
            value: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        },
        SchemaType::datetime(),
    );
    round_trip_all(
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: 30_000_000_000,
        }),
        SchemaType::duration(),
    );
    // Canonical quantity (1.5kg) — no trailing zeros so the value round-trips
    // exactly through the canonical encoder.
    round_trip_all(
        SchemaValue::Quantity(QuantityValue {
            mantissa: 15,
            scale: 1,
            unit: "kg".into(),
        }),
        quantity_kg(),
    );
}

fn quantity_kg() -> SchemaType {
    use golem_common::schema::schema_type::QuantitySpec;
    SchemaType::quantity(QuantitySpec {
        base_unit: "kg".into(),
        allowed_suffixes: vec![],
        min: None,
        max: None,
    })
}

fn parse_native(input: &str, ty: SchemaType, lang: SourceLanguage) -> SchemaValue {
    let graph = SchemaGraph::anonymous(ty.clone());
    parse_value_for_language(input, &graph, &ty, &lang)
        .unwrap_or_else(|e| panic!("parse failed for '{input}' ({lang}): {e}"))
}

#[test]
fn rust_native_quantity_literal() {
    use golem_common::schema::schema_type::QuantityValue;
    assert_eq!(
        parse_native("5.kg()", quantity_kg(), SourceLanguage::Rust),
        SchemaValue::Quantity(QuantityValue {
            mantissa: 5,
            scale: 0,
            unit: "kg".into(),
        }),
    );
    assert_eq!(
        parse_native("1.5.kg()", quantity_kg(), SourceLanguage::Rust),
        SchemaValue::Quantity(QuantityValue {
            mantissa: 15,
            scale: 1,
            unit: "kg".into(),
        }),
    );
}

#[test]
fn rust_native_duration_literal() {
    use golem_common::schema::schema_value::DurationValuePayload;
    assert_eq!(
        parse_native(
            "Duration::from_secs(30)",
            SchemaType::duration(),
            SourceLanguage::Rust
        ),
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: 30_000_000_000,
        }),
    );
    assert_eq!(
        parse_native(
            "Duration::from_millis(5)",
            SchemaType::duration(),
            SourceLanguage::Rust
        ),
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: 5_000_000,
        }),
    );
}

#[test]
fn ts_native_quantity_and_duration_literals() {
    use golem_common::schema::schema_type::QuantityValue;
    use golem_common::schema::schema_value::DurationValuePayload;
    assert_eq!(
        parse_native("5n * kg", quantity_kg(), SourceLanguage::TypeScript),
        SchemaValue::Quantity(QuantityValue {
            mantissa: 5,
            scale: 0,
            unit: "kg".into(),
        }),
    );
    assert_eq!(
        parse_native(
            "Duration.seconds(30)",
            SchemaType::duration(),
            SourceLanguage::TypeScript
        ),
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: 30_000_000_000,
        }),
    );
    assert_eq!(
        parse_native(
            "Duration.milliseconds(5n)",
            SchemaType::duration(),
            SourceLanguage::TypeScript
        ),
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: 5_000_000,
        }),
    );
}

#[test]
fn scala_native_datetime_literal() {
    use chrono::DateTime;
    let expected_millis = DateTime::from_timestamp_millis(1_700_000_000_000).unwrap();
    assert_eq!(
        parse_native(
            "Datetime.fromEpochMillis(1700000000000)",
            SchemaType::datetime(),
            SourceLanguage::Scala
        ),
        SchemaValue::Datetime {
            value: expected_millis,
        },
    );
    let expected_secs = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    assert_eq!(
        parse_native(
            "Datetime.fromEpochSeconds(1700000000)",
            SchemaType::datetime(),
            SourceLanguage::Scala
        ),
        SchemaValue::Datetime {
            value: expected_secs,
        },
    );
}

#[test]
fn moonbit_native_datetime_literal() {
    use chrono::DateTime;
    let expected = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    assert_eq!(
        parse_native(
            "Datetime::{ seconds: 1700000000, nanoseconds: 0 }",
            SchemaType::datetime(),
            SourceLanguage::MoonBit
        ),
        SchemaValue::Datetime { value: expected },
    );
}

#[test]
fn native_literal_rejections() {
    use golem_common::schema::schema_type::UrlRestrictions;

    // TypeScript has no native datetime literal — `new Date(...)` must not parse.
    let dt = SchemaType::datetime();
    let dt_graph = SchemaGraph::anonymous(dt.clone());
    assert!(
        parse_value_for_language(
            "new Date(\"2023-01-01T00:00:00Z\")",
            &dt_graph,
            &dt,
            &SourceLanguage::TypeScript
        )
        .is_err()
    );

    // `Url` is constructor-only in every language — `Url::parse(...)` must not parse.
    let url = SchemaType::url(UrlRestrictions::default());
    let url_graph = SchemaGraph::anonymous(url.clone());
    assert!(
        parse_value_for_language(
            "Url::parse(\"https://example.com\")",
            &url_graph,
            &url,
            &SourceLanguage::Rust
        )
        .is_err()
    );
}

#[test]
fn float_literals_still_lex_after_quantity_dot_change() {
    // Making `5.kg()` lex as `5 . kg ( )` must not regress ordinary float
    // literals (the renderers emit trailing-`.0` floats, but exponents and
    // hand-typed forms must still parse).
    for (input, expected) in [
        ("5.0", 5.0_f64),
        ("-0.0", -0.0_f64),
        ("1e3", 1000.0_f64),
        ("1.5e3", 1500.0_f64),
        ("2.71", 2.71_f64),
    ] {
        assert_eq!(
            parse_native(input, SchemaType::f64(), SourceLanguage::Rust),
            SchemaValue::F64(expected),
            "float literal '{input}' regressed",
        );
    }
}

#[test]
fn rust_native_duration_rejects_overflow() {
    // 10e9 seconds * 1e9 ns/s overflows i64 nanoseconds.
    let dur = SchemaType::duration();
    let graph = SchemaGraph::anonymous(dur.clone());
    assert!(
        parse_value_for_language(
            "Duration::from_secs(10000000000)",
            &graph,
            &dur,
            &SourceLanguage::Rust
        )
        .is_err()
    );
}

#[test]
fn scala_native_datetime_fractional_epochs() {
    use chrono::DateTime;
    // `fromEpochSeconds`/`fromEpochMillis` take a Scala `Double`: fractional
    // arguments keep sub-unit precision instead of being truncated.
    assert_eq!(
        parse_native(
            "Datetime.fromEpochSeconds(1.5)",
            SchemaType::datetime(),
            SourceLanguage::Scala
        ),
        SchemaValue::Datetime {
            value: DateTime::from_timestamp(1, 500_000_000).unwrap(),
        },
    );
    assert_eq!(
        parse_native(
            "Datetime.fromEpochMillis(1234.5)",
            SchemaType::datetime(),
            SourceLanguage::Scala
        ),
        SchemaValue::Datetime {
            value: DateTime::from_timestamp(1, 234_500_000).unwrap(),
        },
    );
    // Far-future fractional epoch (year 9999): the whole-value epoch
    // nanoseconds would overflow i64, but seconds-first splitting keeps it
    // inside chrono's range.
    assert_eq!(
        parse_native(
            "Datetime.fromEpochMillis(253402300799999.5)",
            SchemaType::datetime(),
            SourceLanguage::Scala
        ),
        SchemaValue::Datetime {
            value: DateTime::from_timestamp(253_402_300_799, 999_500_000).unwrap(),
        },
    );
}

#[test]
fn scala_native_datetime_rejects_non_finite() {
    let dt = SchemaType::datetime();
    let graph = SchemaGraph::anonymous(dt.clone());
    for input in [
        "Datetime.fromEpochMillis(NaN)",
        "Datetime.fromEpochSeconds(Infinity)",
    ] {
        assert!(
            parse_value_for_language(input, &graph, &dt, &SourceLanguage::Scala).is_err(),
            "'{input}' should be rejected",
        );
    }
}

#[test]
fn moonbit_native_datetime_qualified_and_defaults() {
    use chrono::DateTime;
    let expected = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    // The idiomatic re-exported `@wallClock.Datetime` qualifier is accepted.
    assert_eq!(
        parse_native(
            "@wallClock.Datetime::{ seconds: 1700000000, nanoseconds: 0 }",
            SchemaType::datetime(),
            SourceLanguage::MoonBit
        ),
        SchemaValue::Datetime { value: expected },
    );
    // `nanoseconds` defaults to 0 when omitted.
    assert_eq!(
        parse_native(
            "@wallClock.Datetime::{ seconds: 1700000000 }",
            SchemaType::datetime(),
            SourceLanguage::MoonBit
        ),
        SchemaValue::Datetime { value: expected },
    );
}

#[test]
fn moonbit_native_datetime_nested_in_list() {
    use chrono::DateTime;
    // A qualified literal must also lex inside a nested position where the
    // dispatcher peeks the leading `@` before choosing a parser.
    assert_eq!(
        parse_native(
            "[@wallClock.Datetime::{ seconds: 1, nanoseconds: 0 }, Datetime::{ seconds: 2 }]",
            SchemaType::list(SchemaType::datetime()),
            SourceLanguage::MoonBit
        ),
        SchemaValue::List {
            elements: vec![
                SchemaValue::Datetime {
                    value: DateTime::from_timestamp(1, 0).unwrap(),
                },
                SchemaValue::Datetime {
                    value: DateTime::from_timestamp(2, 0).unwrap(),
                },
            ],
        },
    );
}

#[test]
fn moonbit_native_datetime_rejects_out_of_range_nanos() {
    let dt = SchemaType::datetime();
    let graph = SchemaGraph::anonymous(dt.clone());
    assert!(
        parse_value_for_language(
            "Datetime::{ seconds: 1, nanoseconds: 1000000000 }",
            &graph,
            &dt,
            &SourceLanguage::MoonBit
        )
        .is_err()
    );
}

#[test]
fn capability_values_render_as_redacted_in_every_language() {
    use golem_common::schema::schema_type::{QuotaTokenSpec, SecretSpec};
    use golem_common::schema::schema_value::{QuotaTokenValuePayload, SecretValuePayload};

    let langs = [
        SourceLanguage::Rust,
        SourceLanguage::TypeScript,
        SourceLanguage::Scala,
        SourceLanguage::MoonBit,
    ];

    let secret_ty = SchemaType::secret(SecretSpec::default());
    let secret_val = SchemaValue::Secret(SecretValuePayload {
        secret_id: uuid::Uuid::nil(),
        config_key: Some(vec!["shhh-do-not-log".to_string()]),
        version: 0,
        resolved_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        category: None,
    });
    let secret_graph = SchemaGraph::anonymous(secret_ty.clone());

    let quota_ty = SchemaType::quota_token(QuotaTokenSpec {
        resource_name: Some("gpu-quota".to_string()),
    });
    let quota_val = SchemaValue::QuotaToken(QuotaTokenValuePayload {
        environment_id: uuid::Uuid::nil().into(),
        resource_name: "gpu-quota".to_string(),
        expected_use: 1,
        last_credit: 0,
        last_credit_at: chrono::Utc::now(),
    });
    let quota_graph = SchemaGraph::anonymous(quota_ty.clone());

    for lang in &langs {
        let secret = render_schema_value(&secret_graph, &secret_ty, &secret_val, lang);
        assert_eq!(secret, "<redacted: secret>", "lang={lang}");
        assert!(!secret.contains("shhh-do-not-log"), "lang={lang}");

        let quota = render_schema_value(&quota_graph, &quota_ty, &quota_val, lang);
        assert_eq!(quota, "<redacted: quota-token>", "lang={lang}");
        assert!(!quota.contains("gpu-quota"), "lang={lang}");
    }
}
