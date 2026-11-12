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
        QuantitySpec, QuantityValue, ResultSpec, SchemaType, TextRestrictions, UnionBranch,
        UnionSpec, UrlRestrictions, VariantCaseType,
    };
    use crate::schema::schema_value::{
        BinaryValuePayload, DurationValuePayload, ResultValuePayload, SchemaValue,
        SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
    };
    use chrono::{TimeZone, Utc};
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
                ("uuid", SchemaType::uuid()),
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
                SchemaValue::Uuid(
                    uuid::Uuid::parse_str("dd00721b-3329-4621-a01d-c71f02cd78c6").unwrap(),
                ),
                SchemaValue::Datetime { value: dt },
                SchemaValue::Duration(DurationValuePayload {
                    nanoseconds: 1_500_000_000,
                }),
                SchemaValue::Quantity(QuantityValue {
                    mantissa: 123,
                    scale: 1,
                    unit: "kg".into(),
                }),
                SchemaValue::Map {
                    entries: vec![
                        (SchemaValue::String("a".into()), SchemaValue::U32(1)),
                        (SchemaValue::String("b".into()), SchemaValue::U32(2)),
                    ],
                },
            ],
        );
        let formatted = roundtrip(&v);
        assert!(formatted.starts_with("@t\"hello\",@t[hu]\"szia\",@b[]\"AQID\",@b[text/plain]\"YWJj\",@p\"/tmp/a b\",@u\"https://example.com/a?b=c\",@uuid\"dd00721b-3329-4621-a01d-c71f02cd78c6\",@dt\"2025-04-12T13:14:15.000000000Z\",@dur\"PT1.5S\",@qty\"12.3kg\""));
        assert!(formatted.ends_with(",m[(\"a\",1),(\"b\",2)]"));
    }

    #[test]
    fn host_managed_capabilities_are_rejected_in_agent_ids() {
        let value = typed(
            vec![("secret", SchemaType::secret(Default::default()))],
            vec![SchemaValue::Secret(SecretValuePayload {
                secret_id: Uuid::nil(),
                config_key: None,
                version: 1,
                resolved_at: Utc::now(),
                category: None,
            })],
        );

        assert_eq!(
            format_structural_typed(&value),
            Err(StructuralFormatError::HostManagedCapability)
        );
        assert_eq!(
            parse_structural_typed("@secret\"forged\"", value.graph(), value.root_type()),
            Err(StructuralFormatError::HostManagedCapability)
        );
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

mod canonical_key_tests {
    use super::*;
    use crate::schema::metadata::MetadataEnvelope;
    use crate::schema::render::tests::paired_strategy::paired_strategy;
    use crate::schema::schema_type::{
        BinaryRestrictions, DiscriminatorRule, NamedFieldType, ResultSpec, SchemaType,
        TextRestrictions, UnionBranch, UnionSpec, VariantCaseType,
    };
    use crate::schema::schema_value::{
        BinaryValuePayload, ResultValuePayload, SchemaValue, TextValuePayload, UnionValuePayload,
        VariantValuePayload,
    };
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use test_r::test;

    fn field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: MetadataEnvelope::default(),
        }
    }

    fn record_type(types: Vec<SchemaType>) -> SchemaType {
        SchemaType::record(
            types
                .into_iter()
                .enumerate()
                .map(|(i, t)| field(&format!("f{i}"), t))
                .collect(),
        )
    }

    fn params(types: Vec<SchemaType>, values: Vec<SchemaValue>) -> TypedSchemaValue {
        TypedSchemaValue::new(
            SchemaGraph::anonymous(record_type(types)),
            SchemaValue::Record { fields: values },
        )
    }

    fn format(types: Vec<SchemaType>, values: Vec<SchemaValue>) -> String {
        format_structural_typed(&params(types, values)).unwrap()
    }

    fn parse(
        text: &str,
        types: Vec<SchemaType>,
    ) -> Result<Vec<SchemaValue>, StructuralFormatError> {
        let graph = SchemaGraph::anonymous(record_type(types));
        match parse_structural_typed(text, &graph, &graph.root)? {
            SchemaValue::Record { fields } => Ok(fields),
            other => panic!("expected a record, got {other:?}"),
        }
    }

    fn none() -> SchemaValue {
        SchemaValue::Option { inner: None }
    }

    fn some(v: SchemaValue) -> SchemaValue {
        SchemaValue::Option {
            inner: Some(Box::new(v)),
        }
    }

    fn text(text: &str, language: Option<&str>) -> SchemaValue {
        SchemaValue::Text(TextValuePayload {
            text: text.to_string(),
            language: language.map(str::to_string),
        })
    }

    fn binary(bytes: &[u8], mime_type: Option<&str>) -> SchemaValue {
        SchemaValue::Binary(BinaryValuePayload {
            bytes: bytes.to_vec(),
            mime_type: mime_type.map(str::to_string),
        })
    }

    fn map_type() -> SchemaType {
        SchemaType::map(SchemaType::u32(), SchemaType::string())
    }

    fn map(entries: &[(u32, &str)]) -> SchemaValue {
        SchemaValue::Map {
            entries: entries
                .iter()
                .map(|(k, v)| (SchemaValue::U32(*k), SchemaValue::String(v.to_string())))
                .collect(),
        }
    }

    fn text_type() -> SchemaType {
        SchemaType::text(TextRestrictions::default())
    }

    fn binary_type() -> SchemaType {
        SchemaType::binary(BinaryRestrictions::default())
    }

    fn branch(tag: &str, body: SchemaType) -> UnionBranch {
        UnionBranch {
            tag: tag.into(),
            body,
            discriminator: DiscriminatorRule::FieldAbsent {
                field_name: tag.into(),
            },
            metadata: MetadataEnvelope::default(),
        }
    }

    /// Pins the canonical text of every value kind that this module's canonical rules
    /// touch. Any change to this string changes the address of keyed agents.
    #[test]
    fn golden_canonical_text() {
        let union = SchemaType::union(UnionSpec {
            branches: vec![
                branch(
                    "opt",
                    record_type(vec![SchemaType::option(SchemaType::u8())]),
                ),
                branch("num", SchemaType::u8()),
            ],
        });
        let nested = record_type(vec![
            SchemaType::option(SchemaType::u32()),
            SchemaType::u32(),
            SchemaType::option(SchemaType::u32()),
        ]);
        let types = vec![
            map_type(),
            text_type(),
            text_type(),
            text_type(),
            binary_type(),
            binary_type(),
            binary_type(),
            union.clone(),
            union,
            nested,
            SchemaType::tuple(vec![SchemaType::option(SchemaType::u8())]),
            SchemaType::string(),
            SchemaType::option(SchemaType::u32()),
            SchemaType::option(SchemaType::u32()),
        ];
        let values = vec![
            map(&[(9, "nine"), (10, "ten"), (1, "one")]),
            text("hello", Some("EN-us")),
            text("szia", Some("x]y\\z")),
            text("plain", Some("")),
            binary(b"abc", Some("Text/Plain; Charset=UTF-8")),
            binary(b"", Some("a]b")),
            binary(&[1, 2, 3], None),
            SchemaValue::Union(UnionValuePayload {
                tag: "opt".into(),
                body: Box::new(SchemaValue::Record {
                    fields: vec![none()],
                }),
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "num".into(),
                body: Box::new(SchemaValue::U8(7)),
            }),
            SchemaValue::Record {
                fields: vec![none(), SchemaValue::U32(5), none()],
            },
            SchemaValue::Tuple {
                elements: vec![none()],
            },
            SchemaValue::String("é".into()),
            some(SchemaValue::U32(3)),
            none(),
        ];
        let formatted = format(types.clone(), values);
        assert_eq!(
            formatted,
            r#"m[(1,"one"),(10,"ten"),(9,"nine")],@t[en-us]"hello",@t[x\]y\\z]"szia",@t"plain",@b[text/plain; Charset=UTF-8]"YWJj",@b[a\]b]"",@b[]"AQID",u0,u1(7),(n,5),(n),"é",s(3)"#
        );
        let parsed = parse(&formatted, types.clone()).unwrap();
        assert_eq!(format(types, parsed), formatted);
    }

    #[test]
    fn map_keys_sort_by_their_text_not_their_value() {
        assert_eq!(
            format(vec![map_type()], vec![map(&[(9, "a"), (10, "b")])]),
            r#"m[(10,"b"),(9,"a")]"#
        );
    }

    #[test]
    fn duplicate_map_keys_are_rejected() {
        assert_eq!(
            format_structural_typed(&params(vec![map_type()], vec![map(&[(1, "a"), (1, "b")])])),
            Err(StructuralFormatError::DuplicateMapKey("1".to_string()))
        );
        assert_eq!(
            parse(r#"m[(1,"a"),(2,"c"),(1,"b")]"#, vec![map_type()]),
            Err(StructuralFormatError::DuplicateMapKey("1".to_string()))
        );
    }

    #[test]
    fn agent_key_with_duplicate_map_keys_is_an_error_not_a_panic() {
        let result = crate::schema::agent::ParsedAgentKey::try_new(
            crate::model::agent::AgentTypeName("T".to_string()),
            params(vec![map_type()], vec![map(&[(1, "a"), (1, "b")])]),
            None,
        );
        assert_eq!(result, Err("Duplicate map key: 1".to_string()));
    }

    #[test]
    fn unsorted_map_input_parses_to_the_canonical_value() {
        let canonical = parse(r#"m[(10,"b"),(9,"a")]"#, vec![map_type()]).unwrap();
        let unsorted = parse(r#"m[(9,"a"),(10,"b")]"#, vec![map_type()]).unwrap();
        assert_eq!(unsorted, canonical);
        assert_eq!(canonical, vec![map(&[(10, "b"), (9, "a")])]);
    }

    #[test]
    fn payload_mismatches_are_rejected() {
        let variant = SchemaType::variant(vec![VariantCaseType {
            name: "unit".into(),
            payload: None,
            metadata: MetadataEnvelope::default(),
        }]);
        let unit_ok = SchemaType::result(ResultSpec {
            ok: None,
            err: None,
        });
        let cases = [
            (
                variant,
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(SchemaValue::U8(1))),
                }),
            ),
            (
                unit_ok,
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(SchemaValue::U8(1))),
                }),
            ),
        ];
        for (typ, value) in cases {
            assert!(matches!(
                format_structural_typed(&params(vec![typ], vec![value])),
                Err(StructuralFormatError::PayloadMismatch(_))
            ));
        }
    }

    #[test]
    fn only_trailing_none_fields_are_omitted() {
        let types = vec![
            SchemaType::option(SchemaType::u32()),
            SchemaType::u32(),
            SchemaType::option(SchemaType::u32()),
        ];
        let values = vec![none(), SchemaValue::U32(5), none()];
        assert_eq!(format(types.clone(), values.clone()), "n,5");
        assert_eq!(parse("n,5", types.clone()).unwrap(), values);
        assert_eq!(parse("n,5,n", types).unwrap(), values);
    }

    #[test]
    fn missing_trailing_fields_must_be_options() {
        let types = vec![SchemaType::u32(), SchemaType::u32()];
        assert!(matches!(
            parse("1", types),
            Err(StructuralFormatError::ParseError { .. })
        ));
    }

    #[test]
    fn tag_whitespace_survives_normalization() {
        assert_eq!(
            normalize_structural(r#"@b[text/plain; charset=utf-8] "YQ==" , @t[a\] b]"x""#),
            r#"@b[text/plain; charset=utf-8]"YQ==",@t[a\] b]"x""#
        );
        assert_eq!(normalize_structural("[ 1, 2 ]"), "[1,2]");
    }

    #[test]
    fn tag_escapes_other_than_bracket_and_backslash_are_rejected() {
        assert!(matches!(
            parse(r#"@t[a\x]"y""#, vec![text_type()]),
            Err(StructuralFormatError::ParseError { .. })
        ));
    }

    #[test]
    fn non_canonical_tags_parse_to_the_canonical_value() {
        assert_eq!(
            parse(
                r#"@t[EN]"x",@t[]"y",@b[IMAGE/PNG]"""#,
                vec![text_type(), text_type(), binary_type()]
            )
            .unwrap(),
            vec![
                text("x", Some("en")),
                text("y", None),
                binary(b"", Some("image/png"))
            ]
        );
    }

    #[test]
    fn unicode_is_not_normalized() {
        let precomposed = format(
            vec![SchemaType::string()],
            vec![SchemaValue::String("\u{e9}".into())],
        );
        let decomposed = format(
            vec![SchemaType::string()],
            vec![SchemaValue::String("e\u{301}".into())],
        );
        assert_ne!(precomposed, decomposed);
        assert_eq!(
            parse(&decomposed, vec![SchemaType::string()]).unwrap(),
            vec![SchemaValue::String("e\u{301}".into())]
        );
    }

    /// Paired values whose type the canonical text supports (no host-managed capabilities).
    fn key_value_strategy() -> impl Strategy<Value = (SchemaType, SchemaValue)> {
        paired_strategy().prop_filter("formattable as an agent key", |(t, v)| {
            format_structural_typed(&params(vec![t.clone()], vec![v.clone()])).is_ok()
        })
    }

    proptest! {
        #[test]
        fn map_insertion_order_does_not_change_the_text(
            entries in proptest::collection::btree_map(any::<u32>(), "[ -~]{0,4}", 0..8)
                .prop_map(|m| m.into_iter().collect::<Vec<_>>())
                .prop_shuffle()
        ) {
            let mut sorted = entries.clone();
            sorted.sort_by_key(|(k, _)| k.to_string());
            let to_map = |entries: &[(u32, String)]| SchemaValue::Map {
                entries: entries
                    .iter()
                    .map(|(k, v)| (SchemaValue::U32(*k), SchemaValue::String(v.clone())))
                    .collect(),
            };
            let shuffled_text = format(vec![map_type()], vec![to_map(&entries)]);
            prop_assert_eq!(&shuffled_text, &format(vec![map_type()], vec![to_map(&sorted)]));
            prop_assert_eq!(parse(&shuffled_text, vec![map_type()]).unwrap(), vec![to_map(&sorted)]);
        }

        #[test]
        fn parse_of_format_is_the_identity(pairs in proptest::collection::vec(key_value_strategy(), 0..4)) {
            let (types, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
            let formatted = format(types.clone(), values.clone());
            prop_assert_eq!(parse(&formatted, types).unwrap(), values);
        }

        #[test]
        fn appending_an_option_field_keeps_existing_text(
            pairs in proptest::collection::vec(key_value_strategy(), 0..4),
            (appended, _) in key_value_strategy(),
        ) {
            let (types, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
            let old_text = format(types.clone(), values.clone());
            let mut new_types = types;
            new_types.push(SchemaType::option(appended));
            let mut new_values = values;
            new_values.push(none());
            prop_assert_eq!(&format(new_types.clone(), new_values.clone()), &old_text);
            prop_assert_eq!(parse(&old_text, new_types).unwrap(), new_values);
        }
    }
}
