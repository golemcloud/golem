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

use super::{
    RenderError, from_json_value, to_json_schema, to_json_value, to_reflection_json_schema,
};
use crate::schema::schema_type::{NumericBound, NumericRestrictions};
use crate::schema::validation::{is_equivalent_cross_graph, validate_graph, validate_value};
use crate::schema::{
    BinaryRestrictions, DurationValuePayload, MetadataEnvelope, NamedFieldType, PermissionCardSpec,
    QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaGraph, SchemaType,
    SchemaTypeDef, SchemaValue, TextRestrictions, TextValuePayload, TypeId, VariantCaseType,
    VariantValuePayload,
};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::collections::HashSet;
use test_r::test;

#[test]
fn union_discriminator_is_combined_with_branch_body() {
    use crate::schema::{DiscriminatorRule, UnionBranch, UnionSpec};

    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "matched".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Regex {
                regex: "^(?:foo|bar)$".to_string(),
            },
            metadata: MetadataEnvelope::default(),
        }],
    });
    let schema = to_json_schema(&SchemaGraph::anonymous(ty.clone()), &ty);
    let branch = schema["$defs"]["Matched"]["allOf"]
        .as_array()
        .unwrap_or_else(|| panic!("branch body and discriminator: {schema}"));
    assert_eq!(branch[0]["type"], "string");
    assert_eq!(branch[1]["pattern"], "^(?:foo|bar)$");
}

#[test]
fn canonical_record_round_trips_through_json() {
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u64(),
            metadata: MetadataEnvelope::default(),
        },
        NamedFieldType {
            name: "name".to_string(),
            body: SchemaType::text(TextRestrictions::default()),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U64(u64::MAX),
            SchemaValue::Text(TextValuePayload {
                text: "Ada".to_string(),
                language: Some("en".to_string()),
            }),
        ],
    };

    let rendered = to_json_value(&graph, &ty, &value).expect("render record");
    assert_eq!(
        rendered,
        json!({
            "id": u64::MAX.to_string(),
            "name": { "text": "Ada", "language": "en" }
        })
    );
    assert_eq!(
        from_json_value(&graph, &ty, &rendered).expect("decode record"),
        value
    );
}

#[test]
fn wide_integer_and_rich_value_schemas_match_canonical_json() {
    use crate::schema::QuantitySpec;

    let signed = to_json_schema(
        &SchemaGraph::anonymous(SchemaType::s64()),
        &SchemaType::s64(),
    );
    assert_eq!(signed["type"], "string");
    assert_eq!(signed["format"], "int64");
    assert_eq!(signed["x-golem-minimum"], i64::MIN.to_string());
    assert_eq!(signed["x-golem-maximum"], i64::MAX.to_string());

    let unsigned = to_json_schema(
        &SchemaGraph::anonymous(SchemaType::u64()),
        &SchemaType::u64(),
    );
    assert_eq!(unsigned["type"], "string");
    assert_eq!(unsigned["format"], "uint64");
    assert_eq!(unsigned["x-golem-maximum"], u64::MAX.to_string());

    let duration = SchemaType::duration();
    let duration_schema = to_json_schema(&SchemaGraph::anonymous(duration.clone()), &duration);
    assert_eq!(
        duration_schema["properties"]["nanoseconds"]["type"],
        "string"
    );

    let quantity = SchemaType::quantity(QuantitySpec {
        base_unit: "m".to_string(),
        allowed_suffixes: Vec::new(),
        min: None,
        max: None,
    });
    let quantity_schema = to_json_schema(&SchemaGraph::anonymous(quantity.clone()), &quantity);
    assert_eq!(quantity_schema["properties"]["mantissa"]["type"], "string");
    assert_eq!(quantity_schema["properties"]["scale"]["type"], "integer");
}

#[test]
fn wide_integer_json_rejects_noncanonical_or_out_of_range_strings() {
    for (ty, invalid) in [
        (
            SchemaType::s64(),
            vec![
                json!(1),
                json!("+1"),
                json!("01"),
                json!("-0"),
                json!("9223372036854775808"),
            ],
        ),
        (
            SchemaType::u64(),
            vec![
                json!(1),
                json!("+1"),
                json!("01"),
                json!("-1"),
                json!("18446744073709551616"),
            ],
        ),
    ] {
        let graph = SchemaGraph::anonymous(ty.clone());
        for value in invalid {
            assert!(
                from_json_value(&graph, &ty, &value).is_err(),
                "accepted {value}"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn wide_and_rich_numeric_json_round_trips_without_precision_loss(
        signed in any::<i64>(),
        unsigned in any::<u64>(),
        scale in any::<i32>(),
    ) {
        let signed_type = SchemaType::s64();
        let signed_graph = SchemaGraph::anonymous(signed_type.clone());
        let signed_json = json!(signed.to_string());
        prop_assert_eq!(
            from_json_value(&signed_graph, &signed_type, &signed_json),
            Ok(SchemaValue::S64(signed))
        );

        let unsigned_type = SchemaType::u64();
        let unsigned_graph = SchemaGraph::anonymous(unsigned_type.clone());
        let unsigned_json = json!(unsigned.to_string());
        prop_assert_eq!(
            from_json_value(&unsigned_graph, &unsigned_type, &unsigned_json),
            Ok(SchemaValue::U64(unsigned))
        );

        let duration_type = SchemaType::duration();
        let duration_graph = SchemaGraph::anonymous(duration_type.clone());
        let duration_json = json!({ "nanoseconds": signed.to_string() });
        prop_assert_eq!(
            from_json_value(&duration_graph, &duration_type, &duration_json),
            Ok(SchemaValue::Duration(DurationValuePayload { nanoseconds: signed }))
        );

        let quantity_type = SchemaType::quantity(QuantitySpec {
            base_unit: "m".to_string(),
            allowed_suffixes: Vec::new(),
            min: None,
            max: None,
        });
        let quantity_graph = SchemaGraph::anonymous(quantity_type.clone());
        let quantity_json = json!({
            "mantissa": signed.to_string(),
            "scale": scale,
            "unit": "m",
        });
        prop_assert_eq!(
            from_json_value(&quantity_graph, &quantity_type, &quantity_json),
            Ok(SchemaValue::Quantity(QuantityValue {
                mantissa: signed,
                scale,
                unit: "m".to_string(),
            }))
        );
    }

    #[test]
    fn decimal_string_mutations_are_rejected(value in any::<u64>()) {
        let signed_type = SchemaType::s64();
        let signed_graph = SchemaGraph::anonymous(signed_type.clone());
        let unsigned_type = SchemaType::u64();
        let unsigned_graph = SchemaGraph::anonymous(unsigned_type.clone());
        let digits = value.to_string();

        for mutated in [format!("+{digits}"), format!("0{digits}"), "-0".to_string()] {
            prop_assert!(
                from_json_value(&signed_graph, &signed_type, &json!(mutated)).is_err()
            );
        }
        for mutated in [format!("+{digits}"), format!("0{digits}"), format!("-{digits}")] {
            prop_assert!(
                from_json_value(&unsigned_graph, &unsigned_type, &json!(mutated)).is_err()
            );
        }
    }
}

#[test]
fn refs_variants_and_options_share_one_graph() {
    let payload_id = TypeId::new("example.payload");
    let payload = SchemaType::record(vec![NamedFieldType {
        name: "value".to_string(),
        body: SchemaType::option(SchemaType::string()),
        metadata: MetadataEnvelope::default(),
    }]);
    let root = SchemaType::variant(vec![
        VariantCaseType {
            name: "empty".to_string(),
            payload: None,
            metadata: MetadataEnvelope::default(),
        },
        VariantCaseType {
            name: "payload".to_string(),
            payload: Some(SchemaType::ref_to(payload_id.clone())),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: payload_id,
            name: Some("Payload".to_string()),
            body: payload,
        }],
        root: root.clone(),
    };
    let value = SchemaValue::Variant(VariantValuePayload {
        case: 1,
        payload: Some(Box::new(SchemaValue::Record {
            fields: vec![SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("x".to_string()))),
            }],
        })),
    });

    let rendered = to_json_value(&graph, &root, &value).expect("render variant");
    assert_eq!(rendered, json!({ "payload": { "value": "x" } }));
    assert_eq!(
        from_json_value(&graph, &root, &rendered).expect("decode variant"),
        value
    );

    let schema = to_json_schema(&graph, &root);
    assert_eq!(
        schema["$schema"],
        json!("https://json-schema.org/draft/2020-12/schema")
    );
    assert!(schema["$defs"].get("example.payload").is_some());
    let required = schema["$defs"]["example.payload"]["required"]
        .as_array()
        .expect("required list");
    assert!(!required.contains(&Value::String("value".to_string())));
}

#[test]
fn omitted_option_record_fields_decode_as_none() {
    let option_id = TypeId::new("example.optional-string");
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "direct".to_string(),
            body: SchemaType::option(SchemaType::string()),
            metadata: MetadataEnvelope::default(),
        },
        NamedFieldType {
            name: "referenced".to_string(),
            body: SchemaType::ref_to(option_id.clone()),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: option_id,
            name: None,
            body: SchemaType::option(SchemaType::string()),
        }],
        root: ty.clone(),
    };
    let expected = SchemaValue::Record {
        fields: vec![
            SchemaValue::Option { inner: None },
            SchemaValue::Option { inner: None },
        ],
    };

    assert_eq!(from_json_value(&graph, &ty, &json!({})).unwrap(), expected);
    assert_eq!(
        from_json_value(&graph, &ty, &json!({ "direct": null, "referenced": null })).unwrap(),
        expected
    );
}

#[test]
fn reflection_json_schema_rejects_unrepresentable_leaves() {
    for ty in [
        SchemaType::secret(crate::schema::SecretSpec::default()),
        SchemaType::future(None),
        SchemaType::stream(None),
    ] {
        let graph = SchemaGraph::anonymous(ty.clone());
        assert_eq!(
            to_reflection_json_schema(&graph, &ty, false)["not"],
            json!({})
        );
    }
}

#[test]
fn numeric_json_schemas_apply_every_narrow_and_float_bound() {
    let signed = |min, max| {
        Some(NumericRestrictions {
            min: Some(NumericBound::Signed(min)),
            max: Some(NumericBound::Signed(max)),
            unit: None,
        })
    };
    let unsigned = |min, max| {
        Some(NumericRestrictions {
            min: Some(NumericBound::Unsigned(min)),
            max: Some(NumericBound::Unsigned(max)),
            unit: None,
        })
    };
    let cases = [
        SchemaType::S8 {
            restrictions: signed(i64::MIN, i64::MAX),
            metadata: MetadataEnvelope::default(),
        },
        SchemaType::S16 {
            restrictions: signed(i64::MIN, i64::MAX),
            metadata: MetadataEnvelope::default(),
        },
        SchemaType::S32 {
            restrictions: signed(i64::MIN, i64::MAX),
            metadata: MetadataEnvelope::default(),
        },
        SchemaType::U8 {
            restrictions: unsigned(0, u64::MAX),
            metadata: MetadataEnvelope::default(),
        },
        SchemaType::U16 {
            restrictions: unsigned(0, u64::MAX),
            metadata: MetadataEnvelope::default(),
        },
        SchemaType::U32 {
            restrictions: unsigned(0, u64::MAX),
            metadata: MetadataEnvelope::default(),
        },
    ];
    let expected = [
        (i8::MIN as i64, i8::MAX as i64),
        (i16::MIN as i64, i16::MAX as i64),
        (i32::MIN as i64, i32::MAX as i64),
        (u8::MIN as i64, u8::MAX as i64),
        (u16::MIN as i64, u16::MAX as i64),
        (u32::MIN as i64, u32::MAX as i64),
    ];
    for (ty, (min, max)) in cases.into_iter().zip(expected) {
        let schema = to_json_schema(&SchemaGraph::anonymous(ty.clone()), &ty);
        assert_eq!(schema["minimum"], min);
        assert_eq!(schema["maximum"], max);
    }

    for (ty, min, max) in [
        (
            SchemaType::F32 {
                restrictions: Some(NumericRestrictions {
                    min: Some(NumericBound::float(-f64::MAX).unwrap()),
                    max: Some(NumericBound::float(f64::MAX).unwrap()),
                    unit: None,
                }),
                metadata: MetadataEnvelope::default(),
            },
            -(f32::MAX as f64),
            f32::MAX as f64,
        ),
        (
            SchemaType::F64 {
                restrictions: Some(NumericRestrictions {
                    min: Some(NumericBound::float(-3.5).unwrap()),
                    max: Some(NumericBound::float(4.5).unwrap()),
                    unit: None,
                }),
                metadata: MetadataEnvelope::default(),
            },
            -3.5,
            4.5,
        ),
    ] {
        let schema = to_json_schema(&SchemaGraph::anonymous(ty.clone()), &ty);
        assert_eq!(schema["minimum"], min);
        assert_eq!(schema["maximum"], max);
    }
}

#[test]
fn rich_json_schemas_enforce_allowlists_and_canonical_base64url() {
    let text = SchemaType::text(TextRestrictions {
        languages: Some(vec!["en".to_string(), "de".to_string()]),
        ..Default::default()
    });
    let text_schema = to_json_schema(&SchemaGraph::anonymous(text.clone()), &text);
    assert_eq!(
        text_schema["properties"]["language"]["enum"],
        json!(["en", "de"])
    );
    assert!(text_schema.get("description").is_none());

    let binary = SchemaType::binary(BinaryRestrictions {
        mime_types: Some(vec!["image/png".to_string()]),
        ..Default::default()
    });
    let binary_schema = to_json_schema(&SchemaGraph::anonymous(binary.clone()), &binary);
    assert_eq!(
        binary_schema["properties"]["mimeType"]["enum"],
        json!(["image/png"])
    );
    assert!(binary_schema.get("description").is_none());
    let pattern = binary_schema["properties"]["bytes"]["pattern"]
        .as_str()
        .expect("binary bytes pattern");
    assert_eq!(
        pattern,
        "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$"
    );
    let regex = regex::Regex::new(pattern).expect("canonical base64url regex");
    for valid in ["", "AQ", "AQI", "AQID", "-_8"] {
        assert!(regex.is_match(valid), "{valid} must match");
    }
    for invalid in ["+/8", "AQ==", "-_9", "A"] {
        assert!(!regex.is_match(invalid), "{invalid} must not match");
    }
}

#[test]
fn malformed_json_and_schema_values_are_typed_errors() {
    let ty = SchemaType::record(vec![NamedFieldType {
        name: "id".to_string(),
        body: SchemaType::u32(),
        metadata: MetadataEnvelope::default(),
    }]);
    let graph = SchemaGraph::anonymous(ty.clone());

    let unexpected = from_json_value(&graph, &ty, &json!({ "id": 1, "extra": true }))
        .expect_err("extra field must fail");
    assert!(matches!(unexpected, RenderError::UnexpectedField { .. }));

    let mismatch = to_json_value(&graph, &ty, &SchemaValue::Bool(true))
        .expect_err("wrong value shape must fail");
    assert!(matches!(mismatch, RenderError::ValueMismatch { .. }));
}

fn conformance_fixture(name: &str, id: &str) -> SchemaGraph {
    let root = match name {
        "s64" => SchemaType::s64(),
        "constrained-s64" => SchemaType::S64 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Signed(-9_007_199_254_740_993)),
                max: Some(NumericBound::Signed(9_007_199_254_740_993)),
                unit: None,
            }),
            metadata: MetadataEnvelope::default(),
        },
        "u64" => SchemaType::u64(),
        "binary" => SchemaType::binary(BinaryRestrictions::default()),
        "duration" => SchemaType::duration(),
        "quantity" => SchemaType::quantity(QuantitySpec {
            base_unit: "m".to_string(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        }),
        "tool-input" => SchemaType::record(vec![
            field("pattern", SchemaType::string()),
            field("paths", SchemaType::list(SchemaType::string())),
            field("ignoreCase", SchemaType::option(SchemaType::bool())),
        ]),
        "config-entry" => SchemaType::record(vec![
            field("path", SchemaType::list(SchemaType::string())),
            field("value", SchemaType::s64()),
        ]),
        "constrained-u32" => SchemaType::U32 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Unsigned(2)),
                max: Some(NumericBound::Unsigned(10)),
                unit: None,
            }),
            metadata: MetadataEnvelope::default(),
        },
        "constrained-f64" => SchemaType::F64 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::float(-1.5).unwrap()),
                max: Some(NumericBound::float(2.5).unwrap()),
                unit: None,
            }),
            metadata: MetadataEnvelope::default(),
        },
        "constrained-text" => SchemaType::text(TextRestrictions {
            languages: Some(vec!["en".to_string(), "de".to_string()]),
            min_length: Some(2),
            max_length: Some(8),
            regex: Some("^[a-z]+$".to_string()),
        }),
        "constrained-binary" => SchemaType::binary(BinaryRestrictions {
            mime_types: Some(vec![
                "image/png".to_string(),
                "application/octet-stream".to_string(),
            ]),
            min_bytes: Some(2),
            max_bytes: Some(4),
        }),
        "result" => SchemaType::result(ResultSpec {
            ok: Some(Box::new(SchemaType::string())),
            err: Some(Box::new(SchemaType::u32())),
        }),
        "custom-error" => SchemaType::result(ResultSpec {
            ok: Some(Box::new(SchemaType::string())),
            err: Some(Box::new(SchemaType::record(vec![
                field("code", SchemaType::string()),
                field("retryable", SchemaType::bool()),
            ]))),
        }),
        "optional-record" => {
            let id = TypeId::new("conformance.optional");
            return SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: id.clone(),
                    name: None,
                    body: SchemaType::option(SchemaType::string()),
                }],
                root: SchemaType::record(vec![
                    field("direct", SchemaType::option(SchemaType::string())),
                    field("referenced", SchemaType::ref_to(id)),
                ]),
            };
        }
        other => panic!("{id}: unknown conformance fixture {other}"),
    };
    SchemaGraph::anonymous(root)
}

fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

fn assert_json_subset(actual: &Value, expected: &Value, id: &str) {
    match expected {
        Value::Object(expected) => {
            let actual = actual
                .as_object()
                .unwrap_or_else(|| panic!("{id}: expected object {expected:?}, got {actual}"));
            for (key, expected) in expected {
                let actual = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{id}: missing key {key} in {actual:?}"));
                assert_json_subset(actual, expected, id);
            }
        }
        _ => assert_eq!(actual, expected, "{id}"),
    }
}

fn json_pointer<'a>(value: &'a Value, pointer: &str, id: &str) -> &'a Value {
    if pointer.is_empty() {
        value
    } else {
        value
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{id}: missing JSON pointer {pointer} in {value}"))
    }
}

fn assert_semantic_conformance(fixture: &str, expected: &Value, id: &str) {
    match fixture {
        "unsupported-leaves" => {
            let types = [
                SchemaType::secret(Default::default()),
                SchemaType::quota_token(QuotaTokenSpec::default()),
                SchemaType::permission_card(PermissionCardSpec::default()),
                SchemaType::future(None),
                SchemaType::stream(None),
            ];
            assert_eq!(
                types.len(),
                expected["count"].as_u64().unwrap() as usize,
                "{id}"
            );
            for ty in types {
                let graph = SchemaGraph::anonymous(ty.clone());
                assert_json_subset(
                    &to_reflection_json_schema(&graph, &ty, false),
                    &expected["schema"],
                    id,
                );
            }
        }
        "all-kinds" => {
            const KINDS: &[&str] = &[
                "ref",
                "bool",
                "s8",
                "s16",
                "s32",
                "s64",
                "u8",
                "u16",
                "u32",
                "u64",
                "f32",
                "f64",
                "char",
                "string",
                "record",
                "variant",
                "enum",
                "flags",
                "tuple",
                "list",
                "fixed-list",
                "map",
                "option",
                "result",
                "text",
                "binary",
                "path",
                "url",
                "datetime",
                "duration",
                "quantity",
                "union",
                "secret",
                "quota-token",
                "permission-card",
                "future",
                "stream",
            ];
            assert_eq!(
                serde_json::to_value(KINDS).unwrap(),
                expected["names"],
                "{id}"
            );
        }
        "all-restrictions" => {
            const RESTRICTIONS: &[&str] = &[
                "numeric-minimum",
                "numeric-maximum",
                "numeric-unit",
                "text-languages",
                "text-min-length",
                "text-max-length",
                "text-regex",
                "binary-mime-types",
                "binary-min-bytes",
                "binary-max-bytes",
                "path-direction",
                "path-kind",
                "path-mime-types",
                "path-extensions",
                "url-schemes",
                "url-hosts",
                "quantity-base-unit",
                "quantity-suffixes",
                "quantity-minimum",
                "quantity-maximum",
                "union-prefix",
                "union-suffix",
                "union-regex",
                "union-field",
            ];
            assert_eq!(
                serde_json::to_value(RESTRICTIONS).unwrap(),
                expected["names"],
                "{id}"
            );
        }
        "graph" => {
            let referenced = conformance_fixture("optional-record", id);
            let inline = SchemaGraph::anonymous(SchemaType::record(vec![
                field("direct", SchemaType::option(SchemaType::string())),
                field("referenced", SchemaType::option(SchemaType::string())),
            ]));
            assert!(validate_graph(&referenced).is_ok(), "{id}");
            assert!(
                is_equivalent_cross_graph(&referenced, &referenced.root, &inline, &inline.root,),
                "{id}"
            );
        }
        other => panic!("{id}: unknown semantic conformance fixture {other}"),
    }
}

fn conformance_corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../../../test-data/reflection-conformance/v1.json"
    ))
    .expect("valid reflection conformance corpus")
}

fn run_conformance_cases(operation: &str) {
    let corpus = conformance_corpus();
    let cases = corpus["cases"].as_array().expect("cases array");
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let case_operation = case["operation"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: missing operation"));
        if case_operation != operation {
            continue;
        }
        let fixture = case["fixture"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: missing fixture"));
        let expected = &case["expected"];
        match operation {
            "roundtrip" => {
                let graph = conformance_fixture(fixture, id);
                let packed = from_json_value(&graph, &graph.root, &case["input"])
                    .unwrap_or_else(|error| panic!("{id}: {error}"));
                let rendered = to_json_value(&graph, &graph.root, &packed)
                    .unwrap_or_else(|error| panic!("{id}: {error}"));
                assert_eq!(&rendered, expected, "{id}");
            }
            "reject" => {
                let graph = conformance_fixture(fixture, id);
                let inputs = case["inputs"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![case["input"].clone()]);
                for input in inputs {
                    let actual = match from_json_value(&graph, &graph.root, &input) {
                        Err(_) => "invalid-json",
                        Ok(value) if validate_value(&graph, &graph.root, &value).is_err() => {
                            "constraint-violation"
                        }
                        Ok(value) => panic!("{id} accepted {input} as {value:?}"),
                    };
                    assert_eq!(
                        actual,
                        expected["kind"]
                            .as_str()
                            .unwrap_or_else(|| panic!("{id}: missing reject kind")),
                        "{id}"
                    );
                }
            }
            "json-schema" => {
                let graph = conformance_fixture(fixture, id);
                let rendered = to_reflection_json_schema(&graph, &graph.root, false);
                let path = case["path"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{id}: missing path"));
                let selected = json_pointer(&rendered, path, id);
                assert_json_subset(selected, expected, id);
            }
            "semantic" => assert_semantic_conformance(fixture, expected, id),
            operation => panic!("unknown requested conformance operation {operation}"),
        }
    }
}

#[test]
fn reflection_conformance_corpus_integrity() {
    let corpus = conformance_corpus();
    assert_eq!(corpus["version"], "1.0.0");
    let cases = corpus["cases"].as_array().expect("cases array");
    let recognized = HashSet::from(["roundtrip", "reject", "json-schema", "semantic"]);
    let mut case_ids = HashSet::new();
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        assert!(case_ids.insert(id), "duplicate case ID {id}");
        let operation = case["operation"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: missing operation"));
        assert!(
            recognized.contains(operation),
            "{id}: unknown operation {operation}"
        );
    }
    let declared_ids = corpus["caseIds"].as_array().expect("declared case IDs");
    let declared: HashSet<_> = declared_ids
        .iter()
        .map(|id| id.as_str().expect("declared case ID"))
        .collect();
    assert_eq!(
        declared.len(),
        declared_ids.len(),
        "duplicate declared case ID"
    );
    assert_eq!(case_ids, declared, "missing or unknown conformance cases");
}

#[test]
fn reflection_conformance_roundtrip_cases() {
    run_conformance_cases("roundtrip");
}

#[test]
fn reflection_conformance_reject_cases() {
    run_conformance_cases("reject");
}

#[test]
fn reflection_conformance_json_schema_cases() {
    run_conformance_cases("json-schema");
}

#[test]
fn reflection_conformance_semantic_cases() {
    run_conformance_cases("semantic");
}
