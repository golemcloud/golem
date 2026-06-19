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

//! Tests for the schema adapter layer.
//!
//! Coverage:
//!
//! - Unit tests for the explicit "lossy" cases (rich scalars, unions,
//!   capabilities, maps, fixed lists, recursive cycles, handles).
//! - Property-based round-trip tests over the shared legacy-compatible
//!   subset of types and values: `AnalysedType` → `SchemaType` → `AnalysedType`
//!   and `Value` → `SchemaValue` → `Value` are identities.

use test_r::test;

use golem_wasm::ValueAndType;
use golem_wasm::analysis::analysed_type::{field, record, s32, str, u32};
use golem_wasm::analysis::proptest_strategies::arb_type_and_value;
use proptest::proptest;

use crate::base_model::agent::{
    AgentConstructor, AgentMethod, AgentType, AgentTypeName, BinaryDescriptor, BinaryType,
    ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
    NamedElementSchemas, TextDescriptor, TextType,
};
use crate::schema::adapters::analysed_type::{
    SchemaGraphBuilder, analysed_type_to_schema_graph, analysed_type_to_schema_type_inline,
    schema_graph_to_analysed_type, schema_type_to_analysed_type,
};
use crate::schema::adapters::data_schema::{
    data_schema_to_input_schema, data_schema_to_output_schema, input_schema_to_data_schema,
    output_schema_to_data_schema,
};
use crate::schema::adapters::element_schema::{
    element_schema_to_schema_type, schema_type_to_element_schema,
};
use crate::schema::adapters::error::{SchemaAdapterError, legacy_type_id};
use crate::schema::adapters::value::{
    schema_value_to_value, typed_schema_value_to_value_and_type,
    value_and_type_to_typed_schema_value, value_to_schema_value,
};
use crate::schema::agent::{FieldSource, InputSchema, NamedField, OutputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::{Role, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, NamedFieldType, PathDirection, PathKind, PathSpec, QuantitySpec,
    QuotaTokenSpec, SchemaType, SecretSpec, TextRestrictions, UnionSpec, UrlRestrictions,
};
use crate::schema::schema_value::{
    BinaryValuePayload, QuotaTokenValuePayload, SchemaValue, SecretValuePayload, TextValuePayload,
};

// --------------------------------------------------------------------------
// TypeId normalisation
// --------------------------------------------------------------------------

#[test]
fn legacy_type_id_dots_only() {
    let id = legacy_type_id(Some("a.b"), Some("c")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("a.b.c"));
}

#[test]
fn legacy_type_id_normalises_double_colons() {
    let id = legacy_type_id(Some("a::b"), Some("c")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("a.b.c"));
    let id = legacy_type_id(Some("a"), Some("b::c")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("a.b.c"));
}

#[test]
fn legacy_type_id_bare_name() {
    let id = legacy_type_id(None, Some("x")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("x"));
}

#[test]
fn legacy_type_id_owner_without_name_is_anonymous() {
    // An unanchored owner has nowhere to live in a dotted TypeId, so the
    // adapter drops it and treats the type as anonymous inline. The
    // TypeScript SDK emits this shape for built-in containers (`Result`,
    // `Tuple`, ...) where `owner` is decorative provenance.
    assert!(legacy_type_id(Some("a"), None).unwrap().is_none());
}

#[test]
fn legacy_type_id_none() {
    assert!(legacy_type_id(None, None).unwrap().is_none());
}

#[test]
fn analysed_type_to_schema_graph_disambiguates_same_name_distinct_bodies() {
    // Two `AnalysedType::Variant` values share `name = "Bound"` but carry
    // structurally different payloads. The Rust SDK emits this for every
    // instantiation of `std::ops::Bound<T>` regardless of `T`. The adapter
    // must keep both as distinct `SchemaTypeDef` entries instead of
    // erroring.
    use golem_wasm::analysis::{
        AnalysedType, NameOptionTypePair, TypeS32, TypeS64, TypeTuple, TypeVariant,
    };

    fn bound_variant(inner: AnalysedType) -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            name: Some("Bound".to_string()),
            owner: None,
            cases: vec![
                NameOptionTypePair {
                    name: "Included".into(),
                    typ: Some(inner.clone()),
                },
                NameOptionTypePair {
                    name: "Excluded".into(),
                    typ: Some(inner),
                },
                NameOptionTypePair {
                    name: "Unbounded".into(),
                    typ: None,
                },
            ],
        })
    }

    let ty = AnalysedType::Tuple(TypeTuple {
        name: None,
        owner: None,
        items: vec![
            bound_variant(AnalysedType::S32(TypeS32)),
            bound_variant(AnalysedType::S64(TypeS64)),
        ],
    });

    let graph = analysed_type_to_schema_graph(&ty).expect("conversion must succeed");

    assert_eq!(graph.defs.len(), 2, "expected two distinct defs: {graph:?}");
    // The original `Bound` keeps the bare TypeId; the second registration
    // gets a `__g_<hash>` suffix (URI-safe, JSON-Schema-`$defs`-key-safe).
    assert!(
        graph
            .defs
            .iter()
            .any(|d| d.id == TypeId::new("Bound") && d.name.as_deref() == Some("Bound")),
        "expected bare `Bound` def: {graph:?}",
    );
    assert!(
        graph
            .defs
            .iter()
            .any(|d| d.id.0.starts_with("Bound__g_") && d.name.as_deref() == Some("Bound")),
        "expected disambiguated `Bound__g_…` def: {graph:?}",
    );

    // Same-named legacy types with structurally identical bodies should still
    // dedup to a single def.
    let dedup_ty = AnalysedType::Tuple(TypeTuple {
        name: None,
        owner: None,
        items: vec![
            bound_variant(AnalysedType::S32(TypeS32)),
            bound_variant(AnalysedType::S32(TypeS32)),
        ],
    });
    let dedup_graph =
        analysed_type_to_schema_graph(&dedup_ty).expect("same-body dedup must succeed");
    assert_eq!(dedup_graph.defs.len(), 1);

    // The fingerprint is deterministic across runs: converting the same
    // disambiguating type twice must produce identical TypeIds.
    let second_graph = analysed_type_to_schema_graph(&ty).expect("repeat conversion");
    let mut first_ids: Vec<_> = graph.defs.iter().map(|d| d.id.0.clone()).collect();
    let mut second_ids: Vec<_> = second_graph.defs.iter().map(|d| d.id.0.clone()).collect();
    first_ids.sort();
    second_ids.sort();
    assert_eq!(
        first_ids, second_ids,
        "disambiguation fingerprint must be deterministic"
    );
}

#[test]
fn analysed_type_to_schema_graph_disambiguates_owner_qualified_duplicates() {
    // Owner-qualified collision: two `Bound`s carry the same `owner` and
    // `name` but distinct bodies. Reverse conversion of the disambiguated
    // graph must still recover the original owner (i.e. the `__g_<hash>`
    // suffix must not leak into the legacy `(owner, name)` pair).
    use golem_wasm::analysis::{
        AnalysedType, NameOptionTypePair, TypeS32, TypeS64, TypeTuple, TypeVariant,
    };

    fn bound_variant(inner: AnalysedType) -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            name: Some("Bound".to_string()),
            owner: Some("std::ops".to_string()),
            cases: vec![
                NameOptionTypePair {
                    name: "Included".into(),
                    typ: Some(inner.clone()),
                },
                NameOptionTypePair {
                    name: "Excluded".into(),
                    typ: Some(inner),
                },
                NameOptionTypePair {
                    name: "Unbounded".into(),
                    typ: None,
                },
            ],
        })
    }

    let ty = AnalysedType::Tuple(TypeTuple {
        name: None,
        owner: None,
        items: vec![
            bound_variant(AnalysedType::S32(TypeS32)),
            bound_variant(AnalysedType::S64(TypeS64)),
        ],
    });

    let graph = analysed_type_to_schema_graph(&ty).expect("forward conversion");

    // Both defs survive, both carry the same display `name`, and one keeps
    // the bare base id while the other has the `__g_` marker.
    assert_eq!(graph.defs.len(), 2);
    assert!(
        graph
            .defs
            .iter()
            .all(|d| d.name.as_deref() == Some("Bound")),
    );
    assert!(
        graph
            .defs
            .iter()
            .any(|d| d.id == TypeId::new("std.ops.Bound"))
    );
    assert!(
        graph
            .defs
            .iter()
            .any(|d| d.id.0.starts_with("std.ops.Bound__g_")),
    );

    // Reverse conversion preserves owner/name on every reconstructed
    // variant — the disambiguation suffix must not bleed through into the
    // legacy metadata.
    let reversed = schema_graph_to_analysed_type(&graph).expect("reverse conversion");
    let golem_wasm::analysis::AnalysedType::Tuple(tuple) = reversed else {
        panic!("expected tuple root after reverse: {reversed:?}");
    };
    for item in &tuple.items {
        let golem_wasm::analysis::AnalysedType::Variant(v) = item else {
            panic!("expected variant items in reverse: {tuple:?}");
        };
        assert_eq!(v.name.as_deref(), Some("Bound"));
        assert_eq!(v.owner.as_deref(), Some("std.ops"));
    }
}

#[test]
fn schema_graph_builder_disambiguates_across_multiple_lower_calls() {
    // Two same-name distinct legacy types appear in separate calls to
    // `SchemaGraphBuilder::lower`. The builder's accumulated def table
    // must drive disambiguation across calls — otherwise downstream code
    // that imports each agent constructor / method root through its own
    // call (e.g. CLI bridge generation) silently merges the second root's
    // `Ref` into the first root's body.
    use golem_wasm::analysis::{AnalysedType, NameOptionTypePair, TypeS32, TypeS64, TypeVariant};

    fn bound_variant(inner: AnalysedType) -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            name: Some("Bound".to_string()),
            owner: None,
            cases: vec![
                NameOptionTypePair {
                    name: "Included".into(),
                    typ: Some(inner.clone()),
                },
                NameOptionTypePair {
                    name: "Excluded".into(),
                    typ: Some(inner),
                },
                NameOptionTypePair {
                    name: "Unbounded".into(),
                    typ: None,
                },
            ],
        })
    }

    let mut builder = SchemaGraphBuilder::new();
    let root_s32 = builder
        .lower(&bound_variant(AnalysedType::S32(TypeS32)))
        .unwrap();
    let root_s64 = builder
        .lower(&bound_variant(AnalysedType::S64(TypeS64)))
        .unwrap();

    let SchemaType::Ref { id: id_s32, .. } = &root_s32 else {
        panic!("expected ref root, got {root_s32:?}");
    };
    let SchemaType::Ref { id: id_s64, .. } = &root_s64 else {
        panic!("expected ref root, got {root_s64:?}");
    };
    assert_ne!(
        id_s32, id_s64,
        "two distinct same-name bodies imported across calls must produce distinct TypeIds",
    );

    let snapshot = builder.snapshot_graph(SchemaType::bool());
    assert_eq!(snapshot.defs.len(), 2);
    // Each root resolves to its own body inside the shared graph.
    let s32_def = snapshot.lookup(id_s32).expect("s32 def present");
    let s64_def = snapshot.lookup(id_s64).expect("s64 def present");
    assert_ne!(
        s32_def.body, s64_def.body,
        "shared graph must keep the two distinct bodies",
    );
}

#[test]
fn analysed_type_to_schema_graph_drops_owner_without_name() {
    // The TS SDK emits `owner = "@golemcloud/golem-ts-sdk"` on built-in
    // containers without setting `name`. The adapter must accept this by
    // dropping the unanchored owner and producing an inline (anonymous)
    // schema type.
    use golem_wasm::analysis::{AnalysedType, TypeF64, TypeResult, TypeStr};

    let ty = AnalysedType::Result(TypeResult {
        name: None,
        owner: Some("@golemcloud/golem-ts-sdk".to_string()),
        ok: Some(Box::new(AnalysedType::Str(TypeStr))),
        err: Some(Box::new(AnalysedType::F64(TypeF64))),
    });

    let graph = analysed_type_to_schema_graph(&ty).expect("conversion must succeed");

    assert!(graph.defs.is_empty(), "expected no defs: {graph:?}");
    assert!(
        matches!(graph.root, SchemaType::Result { .. }),
        "expected inline `Result` root: {:?}",
        graph.root,
    );
}

// --------------------------------------------------------------------------
// AnalysedType → SchemaType: unsupported / lossy
// --------------------------------------------------------------------------

#[test]
fn schema_type_to_analysed_type_rejects_rich_scalars() {
    let cases = vec![
        SchemaType::text(TextRestrictions::default()),
        SchemaType::binary(BinaryRestrictions::default()),
        SchemaType::path(PathSpec {
            direction: PathDirection::InOut,
            kind: PathKind::Any,
            allowed_mime_types: None,
            allowed_extensions: None,
        }),
        SchemaType::url(UrlRestrictions::default()),
        SchemaType::datetime(),
        SchemaType::duration(),
        SchemaType::quantity(QuantitySpec {
            base_unit: "B".into(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        }),
        SchemaType::union(UnionSpec { branches: vec![] }),
        SchemaType::secret(SecretSpec::default()),
        SchemaType::quota_token(QuotaTokenSpec::default()),
        SchemaType::map(SchemaType::string(), SchemaType::s32()),
        SchemaType::fixed_list(SchemaType::s32(), 3),
    ];
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    for ty in cases {
        let err = schema_type_to_analysed_type(&graph, &ty).unwrap_err();
        assert!(
            matches!(err, SchemaAdapterError::LossySchemaType(_)),
            "expected LossySchemaType for {ty:?}, got {err:?}"
        );
    }
}

#[test]
fn schema_type_to_analysed_type_rejects_recursive_cycles() {
    let id = TypeId::new("rec");
    let graph = SchemaGraph {
        defs: vec![crate::schema::graph::SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::record(vec![NamedFieldType {
                name: "self".into(),
                body: SchemaType::ref_to(id.clone()),
                metadata: Default::default(),
            }]),
        }],
        root: SchemaType::ref_to(id.clone()),
    };
    let err = schema_graph_to_analysed_type(&graph).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::RecursiveRef(got) if got == id));
}

#[test]
fn schema_type_to_analysed_type_dangling_ref() {
    let id = TypeId::new("missing");
    let graph = SchemaGraph::anonymous(SchemaType::ref_to(id.clone()));
    let err = schema_graph_to_analysed_type(&graph).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::DanglingRef(got) if got == id));
}

// --------------------------------------------------------------------------
// Value → SchemaValue: lossy cases
// --------------------------------------------------------------------------

#[test]
fn schema_value_to_value_rejects_rich_payloads() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let lossy = vec![
        (
            SchemaType::text(TextRestrictions::default()),
            SchemaValue::Text(TextValuePayload {
                text: "hi".into(),
                language: None,
            }),
        ),
        (
            SchemaType::binary(BinaryRestrictions::default()),
            SchemaValue::Binary(BinaryValuePayload {
                bytes: vec![1],
                mime_type: None,
            }),
        ),
        (
            SchemaType::secret(SecretSpec::default()),
            SchemaValue::Secret(SecretValuePayload {
                secret_ref: "x".into(),
            }),
        ),
        (
            SchemaType::quota_token(QuotaTokenSpec::default()),
            SchemaValue::QuotaToken(QuotaTokenValuePayload {
                environment_id: golem_schema::model::EnvironmentId::new(uuid::Uuid::nil()),
                resource_name: "rss".into(),
                expected_use: 0,
                last_credit: 0,
                last_credit_at: chrono::Utc::now(),
            }),
        ),
    ];
    for (ty, val) in lossy {
        let err = schema_value_to_value(&graph, &ty, &val).unwrap_err();
        assert!(
            matches!(err, SchemaAdapterError::LossySchemaType(_)),
            "expected LossySchemaType for {ty:?}/{val:?}, got {err:?}"
        );
    }
}

// --------------------------------------------------------------------------
// ElementSchema round-trip
// --------------------------------------------------------------------------

#[test]
fn element_schema_unstructured_text_round_trip() {
    let leg = ElementSchema::UnstructuredText(TextDescriptor {
        restrictions: Some(vec![TextType {
            language_code: "en-US".into(),
        }]),
    });
    let schema = element_schema_to_schema_type(&leg).unwrap();
    let graph = SchemaGraph::anonymous(schema.clone());
    let back = schema_type_to_element_schema(&graph, &schema).unwrap();
    assert_eq!(leg, back);
}

#[test]
fn element_schema_unstructured_binary_round_trip() {
    let leg = ElementSchema::UnstructuredBinary(BinaryDescriptor {
        restrictions: Some(vec![BinaryType {
            mime_type: "application/json".into(),
        }]),
    });
    let schema = element_schema_to_schema_type(&leg).unwrap();
    let graph = SchemaGraph::anonymous(schema.clone());
    let back = schema_type_to_element_schema(&graph, &schema).unwrap();
    assert_eq!(leg, back);
}

#[test]
fn unstructured_text_role_with_wrong_case_names_is_rejected() {
    use crate::schema::adapters::unstructured::unstructured_text_restrictions;
    use crate::schema::schema_type::VariantCaseType;

    // Role marker says unstructured-text, but the cases are mis-named: the
    // role is authoritative, so this must be a hard error rather than being
    // silently treated as a plain variant.
    let mut ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "body".into(),
            payload: Some(SchemaType::text(TextRestrictions::default())),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "link".into(),
            payload: Some(SchemaType::url(UrlRestrictions::default())),
            metadata: Default::default(),
        },
    ]);
    ty.metadata_mut().role = Some(Role::UnstructuredText);
    let graph = SchemaGraph::anonymous(ty.clone());
    let err = unstructured_text_restrictions(&graph, &ty).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::LossySchemaType(_)));
}

#[test]
fn unstructured_text_role_on_non_variant_is_rejected() {
    use crate::schema::adapters::unstructured::unstructured_text_restrictions;

    // A node tagged with the unstructured-text role that is not a variant at
    // all is malformed and must error (role is authoritative).
    let mut ty = SchemaType::text(TextRestrictions::default());
    ty.metadata_mut().role = Some(Role::UnstructuredText);
    let graph = SchemaGraph::anonymous(ty.clone());
    let err = unstructured_text_restrictions(&graph, &ty).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::LossySchemaType(_)));
}

#[test]
fn unstructured_text_restrictions_ignores_unmarked_variant() {
    use crate::schema::adapters::unstructured::unstructured_text_restrictions;
    use crate::schema::schema_type::VariantCaseType;

    // The same structural shape WITHOUT the role marker is just an ordinary
    // user variant, not an unstructured wrapper: detection returns None.
    let ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "inline".into(),
            payload: Some(SchemaType::text(TextRestrictions::default())),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "url".into(),
            payload: Some(SchemaType::url(UrlRestrictions::default())),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(ty.clone());
    assert!(unstructured_text_restrictions(&graph, &ty).unwrap().is_none());
}

#[test]
fn decode_unstructured_output_matrix() {
    use crate::schema::adapters::unstructured::{
        UnstructuredOutput, decode_unstructured_output, unstructured_binary_schema_type,
        unstructured_inline_value, unstructured_text_schema_type, unstructured_url_value,
    };
    use crate::schema::schema_value::{BinaryValuePayload, TextValuePayload};

    let text_wrapper = unstructured_text_schema_type(TextRestrictions::default());
    let binary_wrapper = unstructured_binary_schema_type(BinaryRestrictions::default());
    let bare_text = SchemaType::text(TextRestrictions::default());
    let bare_binary = SchemaType::binary(BinaryRestrictions::default());

    let text_val = SchemaValue::Text(TextValuePayload {
        text: "hi".into(),
        language: None,
    });
    let binary_val = SchemaValue::Binary(BinaryValuePayload {
        bytes: vec![1, 2, 3],
        mime_type: None,
    });

    let graph = SchemaGraph::empty();

    // Wrapper schema accepts the wrapper inline value.
    let v = unstructured_inline_value(text_val.clone());
    let out = decode_unstructured_output(&graph, &text_wrapper, &v).unwrap();
    assert!(matches!(out, Some(UnstructuredOutput::Inline(_))));

    // Wrapper schema accepts a *raw* matching value (DE).
    let out = decode_unstructured_output(&graph, &text_wrapper, &text_val).unwrap();
    assert!(matches!(out, Some(UnstructuredOutput::Inline(_))));
    let out = decode_unstructured_output(&graph, &binary_wrapper, &binary_val).unwrap();
    assert!(matches!(out, Some(UnstructuredOutput::Inline(_))));

    // Wrapper schema yields a url only via the wrapper `url` value.
    let v = unstructured_url_value("https://example.com/x".into());
    let out = decode_unstructured_output(&graph, &text_wrapper, &v).unwrap();
    assert!(matches!(out, Some(UnstructuredOutput::Url(u)) if u == "https://example.com/x"));

    // Wrapper schema + kind-mismatched raw value errors.
    assert!(decode_unstructured_output(&graph, &text_wrapper, &binary_val).is_err());
    // Wrapper schema + wrapper inline value of the wrong kind errors.
    let v = unstructured_inline_value(binary_val.clone());
    assert!(decode_unstructured_output(&graph, &text_wrapper, &v).is_err());

    // Bare schema accepts a raw matching value.
    let out = decode_unstructured_output(&graph, &bare_text, &text_val).unwrap();
    assert!(matches!(out, Some(UnstructuredOutput::Inline(_))));
    let out = decode_unstructured_output(&graph, &bare_binary, &binary_val).unwrap();
    assert!(matches!(out, Some(UnstructuredOutput::Inline(_))));

    // Bare schema + kind-mismatched value errors.
    assert!(decode_unstructured_output(&graph, &bare_text, &binary_val).is_err());

    // A bare scalar schema never yields a `url` (a wrapper-only case): a url
    // value paired with a bare schema is a kind mismatch and must error.
    let v = unstructured_url_value("https://example.com/x".into());
    assert!(decode_unstructured_output(&graph, &bare_text, &v).is_err());

    // A non-unstructured schema returns None (caller falls back).
    let out = decode_unstructured_output(&graph, &SchemaType::s32(), &SchemaValue::S32(1)).unwrap();
    assert!(out.is_none());
}

#[test]
fn element_schema_component_model_round_trip() {
    let leg = ElementSchema::ComponentModel(ComponentModelElementSchema {
        element_type: record(vec![field("x", s32()), field("y", str())]),
    });
    let schema = element_schema_to_schema_type(&leg).unwrap();
    let graph = SchemaGraph::anonymous(schema.clone());
    let back = schema_type_to_element_schema(&graph, &schema).unwrap();
    assert_eq!(leg, back);
}

#[test]
fn element_schema_with_constraints_is_lossy() {
    let restricted = SchemaType::text(TextRestrictions {
        languages: None,
        min_length: Some(1),
        max_length: None,
        regex: None,
    });
    let graph = SchemaGraph::anonymous(restricted.clone());
    let err = schema_type_to_element_schema(&graph, &restricted).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::LossySchemaType(_)));
}

// --------------------------------------------------------------------------
// DataSchema round-trip
// --------------------------------------------------------------------------

#[test]
fn data_schema_tuple_input_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "a".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: s32(),
                }),
            },
            NamedElementSchema {
                name: "b".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
        ],
    });
    let input = data_schema_to_input_schema(&ds).unwrap();
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = input_schema_to_data_schema(&graph, &input).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_multimodal_input_round_trip() {
    // Multimodal input is supported generically: it maps to a single
    // user-supplied `parts` field of type `list<variant<… Role::Multimodal>>`
    // and round-trips back to the original multimodal `DataSchema`.
    let ds = DataSchema::Multimodal(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "text".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
            NamedElementSchema {
                name: "binary".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
            },
        ],
    });
    let input = data_schema_to_input_schema(&ds).unwrap();
    let InputSchema::Parameters(fields) = &input;
    assert_eq!(
        fields.len(),
        1,
        "multimodal input is a single `parts` field"
    );
    assert_eq!(fields[0].name, "parts");
    assert!(matches!(fields[0].source, FieldSource::UserSupplied));
    match &fields[0].schema {
        SchemaType::List { element, metadata } => {
            assert_eq!(metadata.role, Some(Role::Multimodal));
            match element.as_ref() {
                SchemaType::Variant { cases, .. } => {
                    let names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
                    assert_eq!(names, vec!["text", "binary"]);
                    assert!(
                        cases.iter().all(|c| c.payload.is_some()),
                        "every multimodal variant case carries an element payload"
                    );
                }
                other => panic!("expected list element to be Variant, got {other:?}"),
            }
        }
        other => panic!("expected `parts` to be a List, got {other:?}"),
    }
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = input_schema_to_data_schema(&graph, &input).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_drops_auto_injected_fields() {
    // Auto-injected fields (e.g. the host-provided principal) are out-of-band:
    // they are omitted from the legacy `DataSchema`, leaving only the
    // user-supplied fields.
    let input = InputSchema::Parameters(vec![
        NamedField::user_supplied("query", SchemaType::string()),
        NamedField {
            name: "principal".into(),
            source: FieldSource::AutoInjected(crate::schema::agent::AutoInjectedKind::Principal),
            schema: SchemaType::string(),
            metadata: Default::default(),
        },
    ]);
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let ds = input_schema_to_data_schema(&graph, &input).unwrap();
    match ds {
        DataSchema::Tuple(NamedElementSchemas { elements }) => {
            let names: Vec<&str> = elements.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, vec!["query"]);
        }
        other => panic!("expected Tuple DataSchema, got {other:?}"),
    }
}

#[test]
fn data_schema_only_auto_injected_fields_becomes_empty_tuple() {
    let input = InputSchema::Parameters(vec![NamedField {
        name: "principal".into(),
        source: FieldSource::AutoInjected(crate::schema::agent::AutoInjectedKind::Principal),
        schema: SchemaType::string(),
        metadata: Default::default(),
    }]);
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let ds = input_schema_to_data_schema(&graph, &input).unwrap();
    assert_eq!(
        ds,
        DataSchema::Tuple(NamedElementSchemas { elements: vec![] })
    );
}

#[test]
fn data_schema_tuple_output_empty_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas { elements: vec![] });
    let output = data_schema_to_output_schema(&ds).unwrap();
    assert!(matches!(output, OutputSchema::Unit));
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_tuple_output_single_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "value".into(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: s32(),
            }),
        }],
    });
    let output = data_schema_to_output_schema(&ds).unwrap();
    // Single element collapses into the inline body (anonymous), so the
    // round-trip uses the fallback name `value`.
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_tuple_output_single_record_round_trip() {
    // A method returning a single value of type Record (a real record),
    // which must NOT be confused with the synthetic multi-output wrapper.
    // Reproduces PR #3605 failure where the reverse mapping flattens the
    // record into multiple legacy `DataSchema::Tuple` elements.
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "value".into(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: record(vec![field("x", s32()), field("y", str())]),
            }),
        }],
    });
    let output = data_schema_to_output_schema(&ds).unwrap();
    // The single-element case must NOT mark the inner record as the
    // synthetic wrapper.
    if let OutputSchema::Single(boxed) = &output {
        if let SchemaType::Record { metadata, .. } = boxed.as_ref() {
            assert_eq!(
                metadata.role, None,
                "single-element output record must not be marked as a synthetic wrapper"
            );
        } else {
            panic!("expected Single(Record(...)), got {output:?}");
        }
    } else {
        panic!("expected OutputSchema::Single, got {output:?}");
    }
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_tuple_output_multi_rejected() {
    // Golem agent methods only ever return 0 or 1 output element. The
    // schema-layer adapter must reject multi-element output tuples rather
    // than silently round-tripping them (the reverse cannot distinguish a
    // synthetic multi-output wrapper from a real user-defined record).
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "a".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: s32(),
                }),
            },
            NamedElementSchema {
                name: "b".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
        ],
    });
    let err = data_schema_to_output_schema(&ds).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::ValueShapeMismatch(_)));
}

#[test]
fn data_schema_multimodal_output_round_trip() {
    let ds = DataSchema::Multimodal(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "text".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
            NamedElementSchema {
                name: "binary".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
            },
        ],
    });
    let output = data_schema_to_output_schema(&ds).unwrap();
    // Forward should produce `Single(list<variant<...>>)` whose list node
    // carries `Role::Multimodal`.
    match &output {
        OutputSchema::Single(boxed) => match boxed.as_ref() {
            SchemaType::List { element, metadata } => {
                assert_eq!(metadata.role, Some(Role::Multimodal));
                match element.as_ref() {
                    SchemaType::Variant { cases, .. } => {
                        let names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
                        assert_eq!(names, vec!["text", "binary"]);
                    }
                    other => panic!("expected list element to be Variant, got {other:?}"),
                }
            }
            other => panic!("expected Single(List(...)), got {other:?}"),
        },
        other => panic!("expected Single(List(...)), got {other:?}"),
    }
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

// --------------------------------------------------------------------------
// Agent-level adapters
// --------------------------------------------------------------------------

#[test]
fn agent_type_round_trip() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::{agent_type_to_schema, schema_agent_type_to_legacy};

    let ty = AgentType {
        type_name: AgentTypeName("weather-agent".into()),
        description: "Reports weather".into(),
        source_language: "rust".into(),
        constructor: AgentConstructor {
            name: Some("ctor".into()),
            description: "ctor".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "city".into(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "forecast".into(),
            description: "forecast".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "days".into(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: u32(),
                    }),
                }],
            }),
            // Single-element output forms collapse into an anonymous body
            // and rehydrate with the fallback `value` name; multi-element
            // outputs are not supported (Golem agent methods only ever
            // return 0 or 1 element).
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "value".into(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                }],
            }),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Ephemeral,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let schema = agent_type_to_schema(&ty).unwrap();
    let back = schema_agent_type_to_legacy(&schema).unwrap();
    assert_eq!(ty, back);
}

/// Refs in a parent constructor / method body resolve against
/// [`AgentTypeSchema::schema`] during the schema → legacy conversion.
#[test]
fn agent_type_reverse_resolves_parent_refs() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
        OutputSchema,
    };
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let user_id = TypeId::new("a.b.UserId");
    let agent = AgentTypeSchema {
        type_name: AgentTypeName("dir".into()),
        description: "x".into(),
        source_language: String::new(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: user_id.clone(),
                name: Some("UserId".into()),
                body: SchemaType::string(),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "id",
                SchemaType::ref_to(user_id.clone()),
            )]),
        },
        methods: vec![AgentMethodSchema {
            name: "lookup".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "id",
                SchemaType::ref_to(user_id.clone()),
            )]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::ref_to(user_id.clone()))),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    // Should succeed — refs resolve via the agent's graph.
    let legacy = schema_agent_type_to_legacy(&agent).expect("ref resolution should succeed");

    // Constructor `id` parameter resolves to whatever the data-schema
    // adapter chooses for a `SchemaType::String` (the structural details
    // are covered by element-schema tests; here we only need to confirm
    // that the ref was resolved at all).
    let ctor_elements = match &legacy.constructor.input_schema {
        DataSchema::Tuple(elements) => &elements.elements,
        _ => panic!("expected tuple"),
    };
    assert_eq!(ctor_elements.len(), 1);
    assert_eq!(ctor_elements[0].name, "id");

    // Method output single ref → wrapped in a synthetic tuple element named "value".
    let out_elements = match &legacy.methods[0].output_schema {
        DataSchema::Tuple(elements) => &elements.elements,
        _ => panic!("expected tuple"),
    };
    assert_eq!(out_elements.len(), 1);
    assert_eq!(out_elements[0].name, "value");
}

/// A dependency's refs resolve against the dependency's own graph, not
/// the parent agent's.
#[test]
fn agent_dependency_reverse_resolves_dep_refs() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentDependencySchema, AgentTypeSchema, InputSchema, NamedField,
    };
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let dep_local = TypeId::new("dep.Local");
    let parent_only = TypeId::new("parent.Only");

    let agent = AgentTypeSchema {
        type_name: AgentTypeName("parent".into()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: parent_only.clone(),
                name: None,
                body: SchemaType::string(),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![AgentDependencySchema {
            type_name: "dep".into(),
            description: None,
            schema: SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: dep_local.clone(),
                    name: None,
                    body: SchemaType::u32(),
                }],
                root: SchemaType::Record {
                    fields: vec![],
                    metadata: Default::default(),
                },
            },
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "n",
                    SchemaType::ref_to(dep_local.clone()),
                )]),
            },
            methods: vec![],
        }],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let legacy = schema_agent_type_to_legacy(&agent).expect("dep ref resolution should succeed");
    let dep_ctor_elements = match &legacy.dependencies[0].constructor.input_schema {
        DataSchema::Tuple(elements) => &elements.elements,
        _ => panic!("expected tuple"),
    };
    assert_eq!(dep_ctor_elements[0].name, "n");
}

/// A ref to a type missing from the agent graph is rejected by the
/// reverse adapter with a clear error.
#[test]
fn agent_type_reverse_rejects_unresolvable_ref() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{AgentConstructorSchema, AgentTypeSchema, InputSchema, NamedField};
    use crate::schema::graph::SchemaGraph;
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let agent = AgentTypeSchema {
        type_name: AgentTypeName("dir".into()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "id",
                SchemaType::ref_to(TypeId::new("missing")),
            )]),
        },
        methods: vec![],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let err = schema_agent_type_to_legacy(&agent).unwrap_err();
    assert!(
        matches!(err, SchemaAdapterError::DanglingRef(ref id) if id == &TypeId::new("missing")),
        "expected DanglingRef(missing), got: {err:?}"
    );
}

/// A dependency cannot resolve a ref that lives only in the parent
/// agent's graph: it should fail with `DanglingRef`, enforcing the
/// "no cross-agent registry" rule.
#[test]
fn agent_dependency_cannot_see_parent_refs() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentDependencySchema, AgentTypeSchema, InputSchema, NamedField,
    };
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let parent_only = TypeId::new("parent.Only");

    let agent = AgentTypeSchema {
        type_name: AgentTypeName("parent".into()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: parent_only.clone(),
                name: None,
                body: SchemaType::string(),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![AgentDependencySchema {
            type_name: "dep".into(),
            description: None,
            schema: SchemaGraph::empty(), // dep has no defs of its own
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "x",
                    SchemaType::ref_to(parent_only.clone()),
                )]),
            },
            methods: vec![],
        }],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let err = schema_agent_type_to_legacy(&agent).unwrap_err();
    assert!(
        matches!(err, SchemaAdapterError::DanglingRef(ref id) if id == &parent_only),
        "expected DanglingRef({parent_only}) — dependencies must not see parent defs; got: {err:?}"
    );
}

// --------------------------------------------------------------------------
// Legacy resource handles
// --------------------------------------------------------------------------

#[test]
fn lower_errors_on_handle() {
    use golem_wasm::analysis::analysed_type::handle;
    use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode};

    let ty = handle(AnalysedResourceId(0), AnalysedResourceMode::Owned);
    let mut builder = SchemaGraphBuilder::new();
    let err = builder.lower(&ty).unwrap_err();
    assert!(
        matches!(err, SchemaAdapterError::LegacyHandle),
        "builder must reject handles; got: {err:?}"
    );
}

#[test]
fn lower_errors_on_nested_handle() {
    use golem_wasm::analysis::analysed_type::{handle, list};
    use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode};

    // Handles must be rejected recursively, e.g. inside a `list<handle>`.
    let ty = list(handle(AnalysedResourceId(7), AnalysedResourceMode::Owned));
    let mut builder = SchemaGraphBuilder::new();
    let err = builder.lower(&ty).unwrap_err();
    assert!(
        matches!(err, SchemaAdapterError::LegacyHandle),
        "builder must reject nested handles; got: {err:?}"
    );
}

// --------------------------------------------------------------------------
// Property-based round-trip tests over the shared legacy subset
// --------------------------------------------------------------------------

proptest! {
    /// `AnalysedType` → inline `SchemaType` → `AnalysedType` is an identity
    /// for the legacy-compatible subset (no `name`/`owner` on composites,
    /// no `Handle`).
    #[test]
    fn proptest_analysed_type_inline_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, _value) = pair;
        let schema = analysed_type_to_schema_type_inline(&ty).expect("forward type conv");
        let graph = SchemaGraph::anonymous(schema.clone());
        let back = schema_type_to_analysed_type(&graph, &schema).expect("reverse type conv");
        proptest::prop_assert_eq!(strip_names(ty), back);
    }

    /// `Value` + `AnalysedType` → `SchemaValue` → `Value` is an identity
    /// for the legacy-compatible subset.
    #[test]
    fn proptest_value_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, value) = pair;
        let schema = analysed_type_to_schema_type_inline(&ty).expect("forward type conv");
        let graph = SchemaGraph::anonymous(schema.clone());
        let sv = value_to_schema_value(&value, &ty).expect("forward value conv");
        let back = schema_value_to_value(&graph, &schema, &sv).expect("reverse value conv");
        proptest::prop_assert_eq!(value, back);
    }

    /// `ValueAndType` → `TypedSchemaValue` → `ValueAndType` is an identity
    /// for the legacy-compatible subset.
    #[test]
    fn proptest_value_and_type_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, value) = pair;
        let vat = ValueAndType { value, typ: ty };
        let tsv = value_and_type_to_typed_schema_value(&vat).expect("forward conv");
        let back = typed_schema_value_to_value_and_type(&tsv).expect("reverse conv");
        // Strip `name`/`owner` from the type side of the original because
        // the inline strategy never sets them, but the graph reverse may
        // attach defaults.
        proptest::prop_assert_eq!(strip_value_and_type_names(vat), back);
    }

    /// Named composites with `owner` / `name` survive the graph-aware
    /// round-trip: the schema graph re-attaches the legacy display name on
    /// reverse.
    #[test]
    fn proptest_analysed_type_graph_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, _value) = pair;
        let graph = analysed_type_to_schema_graph(&ty).expect("forward graph conv");
        let back = schema_graph_to_analysed_type(&graph).expect("reverse graph conv");
        proptest::prop_assert_eq!(strip_names(ty), back);
    }
}

// --------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------

/// Strip `name` / `owner` from every composite layer of an `AnalysedType`
/// (used because the inline adapter drops these and the strategy never
/// produces them anyway, but to be defensive).
fn strip_names(ty: golem_wasm::analysis::AnalysedType) -> golem_wasm::analysis::AnalysedType {
    use golem_wasm::analysis::AnalysedType::*;
    match ty {
        Bool(_) | S8(_) | S16(_) | S32(_) | S64(_) | U8(_) | U16(_) | U32(_) | U64(_) | F32(_)
        | F64(_) | Chr(_) | Str(_) => ty,
        List(mut t) => {
            t.name = None;
            t.owner = None;
            t.inner = Box::new(strip_names(*t.inner));
            List(t)
        }
        Tuple(mut t) => {
            t.name = None;
            t.owner = None;
            t.items = t.items.into_iter().map(strip_names).collect();
            Tuple(t)
        }
        Record(mut t) => {
            t.name = None;
            t.owner = None;
            t.fields = t
                .fields
                .into_iter()
                .map(|p| golem_wasm::analysis::NameTypePair {
                    name: p.name,
                    typ: strip_names(p.typ),
                })
                .collect();
            Record(t)
        }
        Variant(mut t) => {
            t.name = None;
            t.owner = None;
            t.cases = t
                .cases
                .into_iter()
                .map(|c| golem_wasm::analysis::NameOptionTypePair {
                    name: c.name,
                    typ: c.typ.map(strip_names),
                })
                .collect();
            Variant(t)
        }
        Enum(mut t) => {
            t.name = None;
            t.owner = None;
            Enum(t)
        }
        Flags(mut t) => {
            t.name = None;
            t.owner = None;
            Flags(t)
        }
        Option(mut t) => {
            t.name = None;
            t.owner = None;
            t.inner = Box::new(strip_names(*t.inner));
            Option(t)
        }
        Result(mut t) => {
            t.name = None;
            t.owner = None;
            t.ok = t.ok.map(|b| Box::new(strip_names(*b)));
            t.err = t.err.map(|b| Box::new(strip_names(*b)));
            Result(t)
        }
        Handle(h) => Handle(h),
    }
}

fn strip_value_and_type_names(vat: ValueAndType) -> ValueAndType {
    ValueAndType {
        value: vat.value,
        typ: strip_names(vat.typ),
    }
}

// --------------------------------------------------------------------------
// UntypedDataValue ↔ TypedSchemaValue
// --------------------------------------------------------------------------

mod untyped_round_trip {
    use super::*;
    use crate::base_model::agent::{
        BinaryReference, BinaryReferenceValue, BinarySource, BinaryType, TextReference,
        TextReferenceValue, TextSource, TextType, UntypedDataValue, UntypedElementValue,
        UntypedNamedElementValue,
    };
    use crate::schema::adapters::untyped::{
        typed_input_to_untyped_data_value, typed_schema_value_to_untyped_data_value,
        untyped_data_value_to_typed_input, untyped_data_value_to_typed_schema_output,
    };
    use golem_wasm::Value;
    use test_r::test;

    fn input_schema_two_fields() -> DataSchema {
        DataSchema::Tuple(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "n".into(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: s32(),
                    }),
                },
                NamedElementSchema {
                    name: "note".into(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                },
            ],
        })
    }

    fn input_value_two_fields() -> UntypedDataValue {
        UntypedDataValue::Tuple(vec![
            UntypedElementValue::ComponentModel(Value::S32(7)),
            UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource {
                    data: "hi".into(),
                    text_type: Some(TextType {
                        language_code: "en".into(),
                    }),
                }),
            }),
        ])
    }

    #[test]
    fn input_two_fields_round_trip() {
        let value = input_value_two_fields();
        let schema = input_schema_two_fields();
        let (input_schema, values) =
            untyped_data_value_to_typed_input(value.clone(), &schema).unwrap();
        let back = typed_input_to_untyped_data_value(&input_schema, &values).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn input_multimodal_round_trip() {
        // Multimodal input is supported: a multimodal schema + value lowers
        // to a single `parts` parameter carrying a `list<union<…>>` value and
        // round-trips back to the original multimodal `UntypedDataValue`.
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "summary".into(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                },
                NamedElementSchema {
                    name: "image".into(),
                    schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                        restrictions: None,
                    }),
                },
            ],
        });
        let value = UntypedDataValue::Multimodal(vec![
            UntypedNamedElementValue {
                name: "summary".into(),
                value: UntypedElementValue::UnstructuredText(TextReferenceValue {
                    value: TextReference::Inline(TextSource {
                        data: "ok".into(),
                        text_type: None,
                    }),
                }),
            },
            UntypedNamedElementValue {
                name: "image".into(),
                value: UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data: vec![1, 2, 3],
                        binary_type: BinaryType {
                            mime_type: "image/png".into(),
                        },
                    }),
                }),
            },
        ]);
        let (input_schema, values) =
            untyped_data_value_to_typed_input(value.clone(), &schema).unwrap();
        let InputSchema::Parameters(fields) = &input_schema;
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "parts");
        assert_eq!(values.len(), 1);
        let back = typed_input_to_untyped_data_value(&input_schema, &values).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn input_multimodal_value_rejected() {
        let err = untyped_data_value_to_typed_input(
            UntypedDataValue::Multimodal(vec![]),
            &DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
        )
        .unwrap_err();
        assert!(matches!(err, SchemaAdapterError::ValueShapeMismatch(_)));
    }

    #[test]
    fn output_empty_round_trip() {
        let value = UntypedDataValue::Tuple(vec![]);
        let schema = DataSchema::Tuple(NamedElementSchemas { elements: vec![] });
        let typed = untyped_data_value_to_typed_schema_output(value.clone(), &schema).unwrap();
        let back = typed_schema_value_to_untyped_data_value(&typed).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn output_single_component_model_round_trip() {
        let value =
            UntypedDataValue::Tuple(vec![UntypedElementValue::ComponentModel(Value::S32(42))]);
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "value".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: s32(),
                }),
            }],
        });
        let typed = untyped_data_value_to_typed_schema_output(value.clone(), &schema).unwrap();
        let back = typed_schema_value_to_untyped_data_value(&typed).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn output_multi_record_rejected() {
        // Multi-element output tuples are not supported (Golem agent methods
        // only ever return 0 or 1 output element); the adapter must reject
        // them rather than silently flattening into / out of a synthetic
        // wrapper record.
        let value = input_value_two_fields();
        let schema = input_schema_two_fields();
        let err = untyped_data_value_to_typed_schema_output(value, &schema).unwrap_err();
        assert!(matches!(err, SchemaAdapterError::ValueShapeMismatch(_)));
    }

    #[test]
    fn output_multimodal_round_trip() {
        let schema = DataSchema::Multimodal(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "summary".into(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                },
                NamedElementSchema {
                    name: "image".into(),
                    schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                        restrictions: None,
                    }),
                },
            ],
        });
        let value = UntypedDataValue::Multimodal(vec![
            UntypedNamedElementValue {
                name: "summary".into(),
                value: UntypedElementValue::UnstructuredText(TextReferenceValue {
                    value: TextReference::Inline(TextSource {
                        data: "ok".into(),
                        text_type: None,
                    }),
                }),
            },
            UntypedNamedElementValue {
                name: "image".into(),
                value: UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data: vec![1, 2, 3],
                        binary_type: BinaryType {
                            mime_type: "image/png".into(),
                        },
                    }),
                }),
            },
        ]);
        let typed = untyped_data_value_to_typed_schema_output(value.clone(), &schema).unwrap();
        let back = typed_schema_value_to_untyped_data_value(&typed).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn output_single_record_round_trip() {
        // A method returning a single value of type Record (a real record,
        // not a synthetic wrapper for multi-element output).
        // Reproduces PR #3605 failure where the reverse mapping flattens the
        // record into multiple tuple elements, breaking the SDK contract
        // (`Tuple([single_record])`).
        let inner_record_type = record(vec![field("u8v", u32()), field("s", str())]);
        let inner_record_value =
            Value::Record(vec![Value::U32(42), Value::String("sample".into())]);
        let value = UntypedDataValue::Tuple(vec![UntypedElementValue::ComponentModel(
            inner_record_value.clone(),
        )]);
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "value".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: inner_record_type,
                }),
            }],
        });
        let typed = untyped_data_value_to_typed_schema_output(value.clone(), &schema).unwrap();
        let back = typed_schema_value_to_untyped_data_value(&typed).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn url_text_reference_round_trips() {
        // With the canonical role-marked unstructured variant form
        // (`variant { inline: text, url: url }`), URL references are now
        // representable via the `url` case, so they round-trip rather than
        // failing as lossy (the previous bare `SchemaType::Text` form could
        // only carry inline text).
        let value = UntypedDataValue::Tuple(vec![UntypedElementValue::UnstructuredText(
            TextReferenceValue {
                value: TextReference::Url(crate::base_model::agent::Url {
                    value: "https://example.com/notes.txt".into(),
                }),
            },
        )]);
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "note".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            }],
        });
        let (input_schema, values) =
            untyped_data_value_to_typed_input(value.clone(), &schema).unwrap();
        let back = typed_input_to_untyped_data_value(&input_schema, &values).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn url_binary_reference_round_trips() {
        let value = UntypedDataValue::Tuple(vec![UntypedElementValue::UnstructuredBinary(
            BinaryReferenceValue {
                value: BinaryReference::Url(crate::base_model::agent::Url {
                    value: "https://example.com/image.png".into(),
                }),
            },
        )]);
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "image".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
            }],
        });
        let (input_schema, values) =
            untyped_data_value_to_typed_input(value.clone(), &schema).unwrap();
        let back = typed_input_to_untyped_data_value(&input_schema, &values).unwrap();
        assert_eq!(value, back);
    }
}
