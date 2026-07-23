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

mod schema_native_tests {
    use super::*;
    use crate::model::oplog::payload::types::{SecretRevealAudit, SerializableDateTime};
    use crate::model::oplog::payload::{HostRequestSecretReveal, HostResponseSecretRevealed};
    use crate::model::{AgentId, ComponentId};
    use crate::schema::IntoTypedSchemaValue;
    use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
    use crate::schema::metadata::MetadataEnvelope;
    use crate::schema::schema_type::{
        BinaryRestrictions, DiscriminatorRule, NamedFieldType, PathDirection, PathKind, PathSpec,
        QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType, SecretSpec,
        TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
    };
    use crate::schema::schema_value::{
        BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
        SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
    };
    use chrono::{TimeZone, Utc};
    use golem_schema::EnvironmentId;
    use pretty_assertions::assert_eq;
    use test_r::test;
    use uuid::Uuid;

    fn field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: MetadataEnvelope::default(),
        }
    }

    fn case(name: &str, payload: Option<SchemaType>) -> VariantCaseType {
        VariantCaseType {
            name: name.to_string(),
            payload,
            metadata: MetadataEnvelope::default(),
        }
    }

    fn typed(fields: Vec<(&str, SchemaType)>, values: Vec<SchemaValue>) -> TypedSchemaValue {
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(
                fields.into_iter().map(|(n, t)| field(n, t)).collect(),
            )),
            SchemaValue::Record { fields: values },
        )
    }

    fn roundtrip(value: &TypedSchemaValue) -> String {
        let formatted = format_structural_typed(value).unwrap();
        let parsed = parse_structural_typed(&formatted, value.graph(), value.root_type()).unwrap();
        assert_eq!(parsed, value.value().clone(), "formatted: {formatted}");
        formatted
    }

    #[test]
    fn primitives_roundtrip_and_format() {
        let v = typed(
            vec![
                ("b", SchemaType::bool()),
                ("s8", SchemaType::s8()),
                ("s64", SchemaType::s64()),
                ("u8", SchemaType::u8()),
                ("u64", SchemaType::u64()),
                ("f32", SchemaType::f32()),
                ("f64", SchemaType::f64()),
                ("c", SchemaType::char()),
                ("s", SchemaType::string()),
            ],
            vec![
                SchemaValue::Bool(true),
                SchemaValue::S8(i8::MIN),
                SchemaValue::S64(i64::MAX),
                SchemaValue::U8(u8::MAX),
                SchemaValue::U64(u64::MAX),
                SchemaValue::F32(0.0),
                SchemaValue::F64(-2.5),
                SchemaValue::Char('\n'),
                SchemaValue::String("a\"b\\c".to_string()),
            ],
        );
        assert_eq!(
            roundtrip(&v),
            format!(
                "true,-128,{},255,{},0.0,-2.5,c\"\\n\",\"a\\\"b\\\\c\"",
                i64::MAX,
                u64::MAX
            )
        );
    }

    #[test]
    fn structural_composites_roundtrip() {
        let empty_record = SchemaType::record(vec![]);
        let nested_record = SchemaType::record(vec![
            field("x", SchemaType::u32()),
            field(
                "y",
                SchemaType::tuple(vec![SchemaType::string(), empty_record.clone()]),
            ),
        ]);
        let v = typed(
            vec![
                ("empty", empty_record),
                (
                    "tuple",
                    SchemaType::tuple(vec![SchemaType::bool(), SchemaType::s32()]),
                ),
                ("nested", nested_record),
            ],
            vec![
                SchemaValue::Record { fields: vec![] },
                SchemaValue::Tuple {
                    elements: vec![SchemaValue::Bool(false), SchemaValue::S32(-7)],
                },
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::U32(9),
                        SchemaValue::Tuple {
                            elements: vec![
                                SchemaValue::String("x".into()),
                                SchemaValue::Record { fields: vec![] },
                            ],
                        },
                    ],
                },
            ],
        );
        assert_eq!(roundtrip(&v), "(),(false,-7),(9,(\"x\",()))");
    }

    #[test]
    fn lists_options_results_roundtrip() {
        let v = typed(
            vec![
                ("list", SchemaType::list(SchemaType::u16())),
                ("fixed", SchemaType::fixed_list(SchemaType::bool(), 2)),
                ("empty", SchemaType::list(SchemaType::string())),
                ("some", SchemaType::option(SchemaType::string())),
                ("none", SchemaType::option(SchemaType::u32())),
                (
                    "ok",
                    SchemaType::result(ResultSpec {
                        ok: Some(Box::new(SchemaType::u8())),
                        err: None,
                    }),
                ),
                (
                    "err",
                    SchemaType::result(ResultSpec {
                        ok: None,
                        err: Some(Box::new(SchemaType::string())),
                    }),
                ),
                (
                    "ok_unit",
                    SchemaType::result(ResultSpec {
                        ok: None,
                        err: None,
                    }),
                ),
            ],
            vec![
                SchemaValue::List {
                    elements: vec![SchemaValue::U16(1), SchemaValue::U16(2)],
                },
                SchemaValue::FixedList {
                    elements: vec![SchemaValue::Bool(true), SchemaValue::Bool(false)],
                },
                SchemaValue::List { elements: vec![] },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("x".into()))),
                },
                SchemaValue::Option { inner: None },
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(SchemaValue::U8(3))),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(SchemaValue::String("bad".into()))),
                }),
                SchemaValue::Result(ResultValuePayload::Ok { value: None }),
            ],
        );
        assert_eq!(
            roundtrip(&v),
            "[1,2],[true,false],[],s(\"x\"),n,ok(3),err(\"bad\"),ok"
        );
    }

    #[test]
    fn variants_enums_flags_and_union_roundtrip() {
        let union = SchemaType::union(UnionSpec {
            branches: vec![
                UnionBranch {
                    tag: "s".into(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix { prefix: "".into() },
                    metadata: MetadataEnvelope::default(),
                },
                UnionBranch {
                    tag: "empty".into(),
                    body: SchemaType::record(vec![]),
                    discriminator: DiscriminatorRule::FieldAbsent {
                        field_name: "x".into(),
                    },
                    metadata: MetadataEnvelope::default(),
                },
            ],
        });
        let v = typed(
            vec![
                (
                    "variant0",
                    SchemaType::variant(vec![
                        case("none", None),
                        case("some", Some(SchemaType::string())),
                    ]),
                ),
                (
                    "variant1",
                    SchemaType::variant(vec![
                        case("none", None),
                        case("some", Some(SchemaType::u32())),
                    ]),
                ),
                ("enum", SchemaType::r#enum(vec!["a".into(), "b".into()])),
                (
                    "flags0",
                    SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
                ),
                (
                    "flags1",
                    SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
                ),
                (
                    "flags2",
                    SchemaType::flags(vec!["a".into(), "b".into(), "c".into()]),
                ),
                ("union", union),
            ],
            vec![
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: None,
                }),
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(SchemaValue::U32(7))),
                }),
                SchemaValue::Enum { case: 1 },
                SchemaValue::Flags {
                    bits: vec![false, false, false],
                },
                SchemaValue::Flags {
                    bits: vec![true, false, true],
                },
                SchemaValue::Flags {
                    bits: vec![true, true, true],
                },
                SchemaValue::Union(UnionValuePayload {
                    tag: "s".into(),
                    body: Box::new(SchemaValue::String("body".into())),
                }),
            ],
        );
        assert_eq!(
            roundtrip(&v),
            "v0,v1(7),v1,f(),f(0,2),f(0,1,2),u0(\"body\")"
        );
    }

    #[test]
    fn rich_values_and_map_roundtrip() {
        let dt = Utc.with_ymd_and_hms(2025, 4, 12, 13, 14, 15).unwrap();
        let quota = QuotaTokenValuePayload {
            environment_id: EnvironmentId { uuid: Uuid::nil() },
            resource_name: "res".into(),
            expected_use: 5,
            last_credit: -2,
            last_credit_at: dt,
        };
        let v = typed(
            vec![
                ("text", SchemaType::text(TextRestrictions::default())),
                ("text_lang", SchemaType::text(TextRestrictions::default())),
                ("bin", SchemaType::binary(BinaryRestrictions::default())),
                (
                    "bin_mime",
                    SchemaType::binary(BinaryRestrictions::default()),
                ),
                (
                    "path",
                    SchemaType::path(PathSpec {
                        direction: PathDirection::Input,
                        kind: PathKind::File,
                        allowed_mime_types: None,
                        allowed_extensions: None,
                    }),
                ),
                ("url", SchemaType::url(UrlRestrictions::default())),
                ("dt", SchemaType::datetime()),
                ("dur", SchemaType::duration()),
                (
                    "qty",
                    SchemaType::quantity(QuantitySpec {
                        base_unit: "kg".into(),
                        allowed_suffixes: vec![],
                        min: None,
                        max: None,
                    }),
                ),
                ("secret", SchemaType::secret(SecretSpec::default())),
                ("quota", SchemaType::quota_token(QuotaTokenSpec::default())),
                (
                    "map",
                    SchemaType::map(SchemaType::string(), SchemaType::u32()),
                ),
            ],
            vec![
                SchemaValue::Text(TextValuePayload {
                    text: "hello".into(),
                    language: None,
                }),
                SchemaValue::Text(TextValuePayload {
                    text: "szia".into(),
                    language: Some("hu".into()),
                }),
                SchemaValue::Binary(BinaryValuePayload {
                    bytes: vec![1, 2, 3],
                    mime_type: None,
                }),
                SchemaValue::Binary(BinaryValuePayload {
                    bytes: b"abc".to_vec(),
                    mime_type: Some("text/plain".into()),
                }),
                SchemaValue::Path {
                    path: "/tmp/a b".into(),
                },
                SchemaValue::Url {
                    url: "https://example.com/a?b=c".into(),
                },
                SchemaValue::Datetime { value: dt },
                SchemaValue::Duration(DurationValuePayload {
                    nanoseconds: 1_500_000_000,
                }),
                SchemaValue::Quantity(QuantityValue {
                    mantissa: 123,
                    scale: 1,
                    unit: "kg".into(),
                }),
                SchemaValue::Secret(SecretValuePayload {
                    secret_id: Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap(),
                    config_key: Some(vec!["secret".to_string(), "ref".to_string()]),
                    version: 7,
                    resolved_at: dt,
                    category: Some("api-key".to_string()),
                }),
                SchemaValue::QuotaToken(quota),
                SchemaValue::Map {
                    entries: vec![
                        (SchemaValue::String("a".into()), SchemaValue::U32(1)),
                        (SchemaValue::String("b".into()), SchemaValue::U32(2)),
                    ],
                },
            ],
        );
        let formatted = roundtrip(&v);
        assert!(formatted.starts_with("@t\"hello\",@t[hu]\"szia\",@b[]\"AQID\",@b[text/plain]\"YWJj\",@p\"/tmp/a b\",@u\"https://example.com/a?b=c\",@dt\"2025-04-12T13:14:15.000000000Z\",@dur\"PT1.5S\",@qty\"12.3kg\",@secret\"secret:{"));
        assert!(
            formatted.contains("\\\"secretId\\\":\\\"00000000-0000-0000-0000-000000000123\\\"")
        );
        assert!(formatted.contains("\\\"configKey\\\":[\\\"secret\\\",\\\"ref\\\"]"));
        assert!(formatted.contains("\\\"version\\\":7"));
        assert!(formatted.ends_with(",m[(\"a\",1),(\"b\",2)]"));
    }

    #[test]
    fn secret_reveal_payloads_roundtrip_and_format() {
        let secret_id = Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();

        let request = HostRequestSecretReveal {
            secret_id,
            expected_type: SchemaGraph::anonymous(SchemaType::string()),
        }
        .into_typed_schema_value()
        .expect("secret reveal request must be schema-encodable");
        let formatted_request = roundtrip(&request);
        assert!(!formatted_request.is_empty());

        let response = HostResponseSecretRevealed {
            secret_id,
            pinned_revision: 7,
            resolved_at: SerializableDateTime {
                seconds: 1_700_000_000,
                nanoseconds: 0,
            },
            result: Ok(()),
            audit: SecretRevealAudit {
                calling_agent: AgentId {
                    component_id: ComponentId(Uuid::nil()),
                    agent_id: "agent-1".to_string(),
                },
                config_key: Some(vec!["db".to_string(), "password".to_string()]),
                timestamp: SerializableDateTime {
                    seconds: 1_700_000_001,
                    nanoseconds: 0,
                },
            },
        }
        .into_typed_schema_value()
        .expect("secret revealed response must be schema-encodable");
        let formatted_response = roundtrip(&response);
        assert!(!formatted_response.is_empty());
        assert!(formatted_response.contains("password"));
    }

    #[test]
    fn deep_nesting_error() {
        let mut ty = SchemaType::u32();
        let mut value = SchemaValue::U32(1);
        for _ in 0..MAX_DEPTH {
            ty = SchemaType::tuple(vec![ty]);
            value = SchemaValue::Tuple {
                elements: vec![value],
            };
        }
        let v = typed(vec![("deep", ty)], vec![value]);
        assert_eq!(
            format_structural_typed(&v),
            Err(StructuralFormatError::MaxDepthExceeded(MAX_DEPTH))
        );
    }

    #[test]
    fn float_nan_and_inf_rejected() {
        let cases = [
            (SchemaType::f64(), SchemaValue::F64(f64::NAN)),
            (SchemaType::f64(), SchemaValue::F64(f64::INFINITY)),
            (SchemaType::f32(), SchemaValue::F32(f32::NAN)),
            (SchemaType::f32(), SchemaValue::F32(f32::NEG_INFINITY)),
        ];
        for (ty, bad) in cases {
            let v = typed(vec![("x", ty)], vec![bad]);
            assert_eq!(
                format_structural_typed(&v),
                Err(StructuralFormatError::RejectedFloat)
            );
        }
    }
}
