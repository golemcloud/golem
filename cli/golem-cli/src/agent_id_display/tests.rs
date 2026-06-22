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
        secret_ref: "shhh-do-not-log".to_string(),
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
