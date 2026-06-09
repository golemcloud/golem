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

use crate::schema::graph::SchemaGraph;
use crate::schema::render::error::RenderError;
use crate::schema::render::json_value::{from_json_value, to_json_value};
use crate::schema::render::tests::paired_strategy::paired_strategy;
use crate::schema::schema_type::{
    DiscriminatorRule, FieldDiscriminator, NamedFieldType, SchemaType, TextRestrictions,
    UnionBranch, UnionSpec,
};
use crate::schema::schema_value::{SchemaValue, TextValuePayload, UnionValuePayload};
use crate::schema::proptest_strategies::schema_values_eq;
use proptest::prelude::*;
use serde_json::json;
use test_r::test;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Round-trip property: a value built from `paired_strategy` survives
    /// `to_json_value` → `from_json_value` unchanged (modulo NaN-equality).
    /// The strategy is constructed so ambiguous option-nesting is never
    /// generated; no `prop_assume!` filter is needed.
    #[test]
    fn paired_value_json_round_trip((ty, value) in paired_strategy()) {
        let graph = SchemaGraph::anonymous(ty.clone());
        let json = to_json_value(&graph, &ty, &value).expect("to_json_value");
        let back = from_json_value(&graph, &ty, &json).expect("from_json_value");
        prop_assert!(
            schema_values_eq(&value, &back),
            "round-trip mismatch:\n  ty: {ty:?}\n  value: {value:?}\n  json: {json}\n  back: {back:?}"
        );
    }

    /// The JSON value produced by `to_json_value` validates against the
    /// JSON Schema produced by `to_json_schema` for the same type.
    #[test]
    fn json_value_validates_against_json_schema((ty, value) in paired_strategy()) {
        let graph = SchemaGraph::anonymous(ty.clone());
        let json = to_json_value(&graph, &ty, &value).expect("to_json_value");
        let schema = crate::schema::render::json_schema::to_json_schema(&graph, &ty);
        let compiled = jsonschema::draft202012::new(&schema).expect("compile schema");
        prop_assert!(
            compiled.is_valid(&json),
            "value did not validate against produced schema:\n  ty: {ty:?}\n  value: {value:?}\n  json: {json}\n  schema: {schema}"
        );
    }

    /// Multi-branch union round-trip with `FieldEquals` discriminators.
    #[test]
    fn multi_branch_field_equals_union_round_trip(
        which in 0u8..3u8,
        rest in 0u32..100u32,
    ) {
        let ty = SchemaType::union(UnionSpec {
            branches: vec![
                make_field_equals_branch("alpha", "a", "kind"),
                make_field_equals_branch("beta", "b", "kind"),
                make_field_equals_branch("gamma", "c", "kind"),
            ],
        });
        let (tag, literal) = match which % 3 {
            0 => ("alpha", "a"),
            1 => ("beta", "b"),
            _ => ("gamma", "c"),
        };
        let value = SchemaValue::Union(UnionValuePayload {
            tag: tag.to_string(),
            body: Box::new(SchemaValue::Record {
                fields: vec![
                    SchemaValue::String(literal.to_string()),
                    SchemaValue::U32(rest),
                ],
            }),
        });
        let graph = SchemaGraph::anonymous(ty.clone());
        let json = to_json_value(&graph, &ty, &value).expect("to_json_value");
        let back = from_json_value(&graph, &ty, &json).expect("from_json_value");
        prop_assert!(schema_values_eq(&value, &back));
    }
}

fn make_field_equals_branch(tag: &str, literal: &str, field_name: &str) -> UnionBranch {
    UnionBranch {
        tag: tag.to_string(),
        body: SchemaType::record(vec![
            NamedFieldType {
                name: field_name.to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "value".to_string(),
                body: SchemaType::u32(),
                metadata: Default::default(),
            },
        ]),
        discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
            field_name: field_name.to_string(),
            literal: Some(literal.to_string()),
        }),
        metadata: Default::default(),
    }
}

#[test]
fn record_renders_as_json_object() {
    let ty = SchemaType::record(vec![
        NamedFieldType {
            name: "id".to_string(),
            body: SchemaType::u32(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "name".to_string(),
            body: SchemaType::text(TextRestrictions::default()),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(7),
            SchemaValue::Text(TextValuePayload {
                text: "Ada".to_string(),
                language: None,
            }),
        ],
    };
    let json = to_json_value(&graph, &ty, &value).expect("to_json_value");
    assert_eq!(json, json!({ "id": 7, "name": { "text": "Ada" } }));
    let back = from_json_value(&graph, &ty, &json).expect("from_json_value");
    assert_eq!(back, value);
}

#[test]
fn variant_unit_case_renders_as_bare_string() {
    let ty = SchemaType::variant(vec![crate::schema::schema_type::VariantCaseType {
        name: "ready".to_string(),
        payload: None,
        metadata: Default::default(),
    }]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Variant(crate::schema::schema_value::VariantValuePayload {
        case: 0,
        payload: None,
    });
    let json = to_json_value(&graph, &ty, &value).expect("to_json_value");
    assert_eq!(json, json!("ready"));
    let back = from_json_value(&graph, &ty, &json).expect("from_json_value");
    assert_eq!(back, value);
}

#[test]
fn shape_mismatch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::s32());
    let err = to_json_value(&graph, &SchemaType::s32(), &SchemaValue::Bool(true))
        .expect_err("should fail");
    assert!(matches!(err, RenderError::ValueMismatch { .. }));
}

// ----- Decoder strict-mode tests -----

#[test]
fn record_decoder_rejects_extra_fields() {
    let ty = SchemaType::record(vec![NamedFieldType {
        name: "id".to_string(),
        body: SchemaType::u32(),
        metadata: Default::default(),
    }]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!({ "id": 1, "extra": "nope" });
    let err = from_json_value(&graph, &ty, &json).expect_err("should fail");
    assert!(matches!(err, RenderError::UnexpectedField { .. }));
}

#[test]
fn f32_decoder_rejects_out_of_range() {
    let ty = SchemaType::f32();
    let graph = SchemaGraph::anonymous(ty.clone());
    // 1e39 exceeds f32::MAX (~3.4e38); `as f32` would saturate to inf.
    let err = from_json_value(&graph, &ty, &json!(1e39)).expect_err("should fail");
    assert!(
        matches!(err, RenderError::ValueMismatch { ref reason, .. } if reason.contains("f32 out of range")),
        "unexpected error: {err:?}"
    );
    // Negative overflow.
    let err = from_json_value(&graph, &ty, &json!(-1e39)).expect_err("should fail");
    assert!(
        matches!(err, RenderError::ValueMismatch { ref reason, .. } if reason.contains("f32 out of range")),
        "unexpected error: {err:?}"
    );
}

#[test]
fn f32_decoder_accepts_in_range_values() {
    let ty = SchemaType::f32();
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = from_json_value(&graph, &ty, &json!(1.5)).expect("from_json_value");
    assert!(matches!(v, SchemaValue::F32(f) if (f - 1.5_f32).abs() < f32::EPSILON));
    // f32::MAX itself is in range.
    let v = from_json_value(&graph, &ty, &json!(f32::MAX as f64)).expect("from_json_value");
    assert!(matches!(v, SchemaValue::F32(f) if f == f32::MAX));
}

#[test]
fn flags_decoder_rejects_duplicates() {
    let ty = SchemaType::flags(vec!["a".to_string(), "b".to_string()]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!(["a", "a"]);
    let err = from_json_value(&graph, &ty, &json).expect_err("should fail");
    assert!(matches!(err, RenderError::DuplicateFlag { .. }));
}

#[test]
fn tuple_arity_mismatch_is_rejected() {
    let ty = SchemaType::tuple(vec![SchemaType::u32(), SchemaType::string()]);
    let graph = SchemaGraph::anonymous(ty.clone());
    let too_few = json!([1]);
    let too_many = json!([1, "x", true]);
    assert!(from_json_value(&graph, &ty, &too_few).is_err());
    assert!(from_json_value(&graph, &ty, &too_many).is_err());
}

#[test]
fn union_no_match_is_reported() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Prefix {
                prefix: "x:".to_string(),
            },
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!("y:hello");
    let err = from_json_value(&graph, &ty, &json).expect_err("should fail");
    assert!(matches!(err, RenderError::UnionNoMatch));
}

#[test]
fn union_ambiguous_match_is_reported() {
    // Two branches with the same `FieldEquals` rule overlap: validation
    // should normally catch this, but the runtime safety net activates here
    // because we build the graph directly.
    let body_ty = SchemaType::record(vec![NamedFieldType {
        name: "k".to_string(),
        body: SchemaType::string(),
        metadata: Default::default(),
    }]);
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "a".to_string(),
                body: body_ty.clone(),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "k".to_string(),
                    literal: None,
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "b".to_string(),
                body: body_ty,
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "k".to_string(),
                    literal: None,
                }),
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!({ "k": "v" });
    let err = from_json_value(&graph, &ty, &json).expect_err("should fail");
    assert!(matches!(err, RenderError::UnionAmbiguous { .. }));
}

#[test]
fn union_encode_validates_branch_body() {
    // The encoded body of the chosen branch must satisfy the branch's
    // discriminator rule; constructing a value where it doesn't is an
    // encode error.
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "p".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Prefix {
                prefix: "expected:".to_string(),
            },
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Union(UnionValuePayload {
        tag: "p".to_string(),
        body: Box::new(SchemaValue::String("wrong".to_string())),
    });
    let err = to_json_value(&graph, &ty, &value).expect_err("should fail");
    assert!(matches!(err, RenderError::UnionTagMismatch { .. }));
}

#[test]
fn union_decode_picks_prefix_branch() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "ssh".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "ssh://".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "https".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "https://".to_string(),
                },
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!("ssh://example.com");
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "ssh");
    } else {
        panic!("expected union");
    }
}

#[test]
fn union_decode_picks_suffix_branch() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "tar".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Suffix {
                    suffix: ".tar.gz".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "zip".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Suffix {
                    suffix: ".zip".to_string(),
                },
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!("file.tar.gz");
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "tar");
    } else {
        panic!("expected union");
    }
}

#[test]
fn union_decode_picks_contains_branch() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "marker".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Contains {
                substring: "@@".to_string(),
            },
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!("hello@@world");
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "marker");
    } else {
        panic!("expected union");
    }
}

#[test]
fn union_decode_picks_regex_branch() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "num".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Regex {
                regex: "^[0-9]+$".to_string(),
            },
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!("12345");
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "num");
    } else {
        panic!("expected union");
    }
}

#[test]
fn union_decode_picks_field_equals_with_literal() {
    let body = SchemaType::record(vec![NamedFieldType {
        name: "kind".to_string(),
        body: SchemaType::string(),
        metadata: Default::default(),
    }]);
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "left".to_string(),
                body: body.clone(),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("L".to_string()),
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "right".to_string(),
                body: body.clone(),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("R".to_string()),
                }),
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!({ "kind": "R" });
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "right");
    } else {
        panic!("expected union");
    }
}

#[test]
fn union_decode_picks_field_equals_without_literal() {
    // Two branches discriminated by presence of distinct fields (no
    // literals).
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "left".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "left_only".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "left_only".to_string(),
                    literal: None,
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "right".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "right_only".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "right_only".to_string(),
                    literal: None,
                }),
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!({ "right_only": "ok" });
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "right");
    } else {
        panic!("expected union");
    }
}

#[test]
fn union_decode_picks_field_absent_branch() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "with_kind".to_string(),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                }]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: None,
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "without_kind".to_string(),
                body: SchemaType::record(vec![]),
                discriminator: DiscriminatorRule::FieldAbsent {
                    field_name: "kind".to_string(),
                },
                metadata: Default::default(),
            },
        ],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!({});
    let decoded = from_json_value(&graph, &ty, &json).expect("from_json_value");
    if let SchemaValue::Union(payload) = decoded {
        assert_eq!(payload.tag, "without_kind");
    } else {
        panic!("expected union");
    }
}

// --- Multimodal unions ---
//
// Multimodal unions (`Role::Multimodal`) are positionally tagged in the
// outer envelope and carry placeholder `FieldAbsent { field_name: "" }`
// discriminators on each branch. The generic encode/decode pipeline must
// not apply those placeholder rules to the inner body, otherwise a scalar
// branch body would fail to encode (`UnionTagMismatch`) or to decode
// (`UnionNoMatch`).

fn multimodal_caption_image_union() -> SchemaType {
    let mut union = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "caption".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::FieldAbsent {
                    field_name: String::new(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "image_url".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::FieldAbsent {
                    field_name: String::new(),
                },
                metadata: Default::default(),
            },
        ],
    });
    union.metadata_mut().role = Some(crate::schema::metadata::Role::Multimodal);
    union
}

#[test]
fn multimodal_union_encode_scalar_body_does_not_apply_placeholder_rule() {
    // A multimodal `caption: string` body should encode to its bare JSON
    // string without tripping the safety net for the placeholder
    // `FieldAbsent { field_name: "" }` discriminator.
    let ty = multimodal_caption_image_union();
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Union(UnionValuePayload {
        tag: "caption".to_string(),
        body: Box::new(SchemaValue::String("hello world".to_string())),
    });
    let json = to_json_value(&graph, &ty, &value).expect("encode must succeed");
    assert_eq!(json, json!("hello world"));
}

#[test]
fn multimodal_union_decode_is_explicitly_unsupported() {
    // Generic discriminator-based decoding cannot recover the positional
    // tag from a bare union body, so the decoder must reject multimodal
    // unions explicitly rather than silently mis-tag values.
    let ty = multimodal_caption_image_union();
    let graph = SchemaGraph::anonymous(ty.clone());
    let json = json!("hello world");
    let err = from_json_value(&graph, &ty, &json).expect_err("multimodal decode must error");
    assert!(matches!(err, RenderError::Unsupported(_)), "got {err:?}");
}
