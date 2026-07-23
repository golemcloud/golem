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

use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NamedFieldType, PathDirection, PathKind, PathSpec,
    QuantitySpec, QuantityValue, QuotaTokenSpec, ResultSpec, SchemaType, SecretSpec,
    TextRestrictions, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{
    BinaryValuePayload, DurationValuePayload, QuotaTokenValuePayload, ResultValuePayload,
    SchemaValue, SecretValuePayload, TextValuePayload, UnionValuePayload, VariantValuePayload,
};
use crate::schema::validation::subtyping::is_assignable;
use crate::schema::validation::value::{ValueError, ValuePathSegment, validate_value};
use chrono::Utc;
use proptest::prelude::*;
use test_r::test;

// --- Paired type + value strategy ---

/// Produce a `(SchemaType, SchemaValue)` pair where the value matches the
/// type by construction. Built bottom-up so the type and value trees stay
/// in lockstep.
fn paired_strategy() -> impl Strategy<Value = (SchemaType, SchemaValue)> {
    leaf_paired().prop_recursive(3, 32, 4, |inner| composite_paired(inner.clone()))
}

fn leaf_paired() -> BoxedStrategy<(SchemaType, SchemaValue)> {
    prop_oneof![
        // Primitives
        Just((SchemaType::bool(), SchemaValue::Bool(false))),
        any::<i8>().prop_map(|i| (SchemaType::s8(), SchemaValue::S8(i))),
        any::<i16>().prop_map(|i| (SchemaType::s16(), SchemaValue::S16(i))),
        any::<i32>().prop_map(|i| (SchemaType::s32(), SchemaValue::S32(i))),
        any::<i64>().prop_map(|i| (SchemaType::s64(), SchemaValue::S64(i))),
        any::<u8>().prop_map(|i| (SchemaType::u8(), SchemaValue::U8(i))),
        any::<u16>().prop_map(|i| (SchemaType::u16(), SchemaValue::U16(i))),
        any::<u32>().prop_map(|i| (SchemaType::u32(), SchemaValue::U32(i))),
        any::<u64>().prop_map(|u| (SchemaType::u64(), SchemaValue::U64(u))),
        // Non-NaN finite floats to keep equality predictable.
        (-1.0e6_f32..1.0e6_f32).prop_map(|f| (SchemaType::f32(), SchemaValue::F32(f))),
        (-1.0e6_f64..1.0e6_f64).prop_map(|f| (SchemaType::f64(), SchemaValue::F64(f))),
        any::<char>().prop_map(|c| (SchemaType::char(), SchemaValue::Char(c))),
        "[ -~]{0,8}".prop_map(|s: String| (SchemaType::string(), SchemaValue::String(s))),
        // Enum / flags
        Just((
            SchemaType::Enum {
                cases: vec!["a".to_string(), "b".to_string()],
                metadata: Default::default(),
            },
            SchemaValue::Enum { case: 0 }
        )),
        Just((
            SchemaType::Enum {
                cases: vec!["a".to_string(), "b".to_string()],
                metadata: Default::default(),
            },
            SchemaValue::Enum { case: 1 }
        )),
        Just((
            SchemaType::Flags {
                flags: vec!["x".to_string(), "y".to_string()],
                metadata: Default::default(),
            },
            SchemaValue::Flags {
                bits: vec![true, false]
            }
        )),
        // Semantic scalars
        "[ -~]{0,8}".prop_map(|s: String| (
            SchemaType::text(TextRestrictions::default()),
            SchemaValue::Text(TextValuePayload {
                text: s,
                language: None,
            })
        )),
        proptest::collection::vec(any::<u8>(), 0..8).prop_map(|bytes| (
            SchemaType::binary(BinaryRestrictions::default()),
            SchemaValue::Binary(BinaryValuePayload {
                bytes,
                mime_type: None,
            })
        )),
        "[a-zA-Z][a-zA-Z0-9/._-]{0,8}".prop_map(|p: String| (
            SchemaType::path(PathSpec {
                direction: PathDirection::Input,
                kind: PathKind::Any,
                allowed_mime_types: None,
                allowed_extensions: None,
            }),
            SchemaValue::Path { path: p }
        )),
        Just((
            SchemaType::url(UrlRestrictions::default()),
            SchemaValue::Url {
                url: "https://example.com/".to_string()
            }
        )),
        Just((
            SchemaType::datetime(),
            SchemaValue::Datetime { value: Utc::now() }
        )),
        any::<i64>().prop_map(|n| (
            SchemaType::duration(),
            SchemaValue::Duration(DurationValuePayload { nanoseconds: n })
        )),
        (-1000i64..1000i64).prop_map(|m| (
            SchemaType::quantity(QuantitySpec {
                base_unit: "kg".to_string(),
                allowed_suffixes: vec![],
                min: None,
                max: None,
            }),
            SchemaValue::Quantity(QuantityValue {
                mantissa: m,
                scale: 0,
                unit: "kg".to_string(),
            })
        )),
        Just((
            SchemaType::secret(SecretSpec::default()),
            SchemaValue::Secret(SecretValuePayload {
                secret_id: uuid::Uuid::nil(),
                config_key: None,
                version: 0,
                resolved_at: Utc::now(),
                category: None,
            })
        )),
        "[a-z][a-z0-9-]{0,4}".prop_map(|r: String| {
            let resource = if r.is_empty() { "r".to_string() } else { r };
            (
                SchemaType::quota_token(QuotaTokenSpec {
                    resource_name: Some(resource.clone()),
                }),
                SchemaValue::QuotaToken(QuotaTokenValuePayload {
                    environment_id: golem_schema::model::EnvironmentId::new(uuid::Uuid::nil()),
                    resource_name: resource,
                    expected_use: 1,
                    last_credit: 0,
                    last_credit_at: Utc::now(),
                }),
            )
        }),
    ]
    .boxed()
}

fn composite_paired(
    inner: BoxedStrategy<(SchemaType, SchemaValue)>,
) -> BoxedStrategy<(SchemaType, SchemaValue)> {
    prop_oneof![
        // record
        proptest::collection::vec(inner.clone(), 0..3).prop_map(|pairs| {
            let mut fields: Vec<NamedFieldType> = Vec::with_capacity(pairs.len());
            let mut values: Vec<SchemaValue> = Vec::with_capacity(pairs.len());
            for (i, (t, v)) in pairs.into_iter().enumerate() {
                fields.push(NamedFieldType {
                    name: format!("f{i}"),
                    body: t,
                    metadata: Default::default(),
                });
                values.push(v);
            }
            (
                SchemaType::Record {
                    fields,
                    metadata: Default::default(),
                },
                SchemaValue::Record { fields: values },
            )
        }),
        // tuple
        proptest::collection::vec(inner.clone(), 0..3).prop_map(|pairs| {
            let (elements, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
            (
                SchemaType::Tuple {
                    elements,
                    metadata: Default::default(),
                },
                SchemaValue::Tuple { elements: values },
            )
        }),
        // list — all elements share the same type, so replicate the head
        // value to keep the value tree consistent with the type tree.
        (inner.clone(), 0u8..3u8).prop_map(|((t, v), n)| {
            let elements: Vec<SchemaValue> = (0..n).map(|_| v.clone()).collect();
            (
                SchemaType::List {
                    element: Box::new(t),
                    metadata: Default::default(),
                },
                SchemaValue::List { elements },
            )
        }),
        // fixed list of length 2 to keep things small
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::FixedList {
                    element: Box::new(t),
                    length: 2,
                    metadata: Default::default(),
                },
                SchemaValue::FixedList {
                    elements: vec![v.clone(), v],
                },
            )
        }),
        // option (some)
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::Option {
                    inner: Box::new(t),
                    metadata: Default::default(),
                },
                SchemaValue::Option {
                    inner: Some(Box::new(v)),
                },
            )
        }),
        // option (none)
        inner.clone().prop_map(|(t, _v)| {
            (
                SchemaType::Option {
                    inner: Box::new(t),
                    metadata: Default::default(),
                },
                SchemaValue::Option { inner: None },
            )
        }),
        // result (ok)
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(t)),
                    err: None,
                }),
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(v)),
                }),
            )
        }),
        // result (err)
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::result(ResultSpec {
                    ok: None,
                    err: Some(Box::new(t)),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(v)),
                }),
            )
        }),
        // variant without payload
        Just((
            SchemaType::Variant {
                cases: vec![VariantCaseType {
                    name: "only".to_string(),
                    payload: None,
                    metadata: Default::default(),
                }],
                metadata: Default::default(),
            },
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: None,
            }),
        )),
        // union with prefix discriminator (string body)
        Just((
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "u".to_string(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix {
                        prefix: "k1:".to_string(),
                    },
                    metadata: Default::default(),
                }],
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "u".to_string(),
                body: Box::new(SchemaValue::String("k1:hello".to_string())),
            }),
        )),
        // map<string, _> — values share a single declared type.
        (inner.clone(), 0u8..3u8).prop_map(|((vt, vv), n)| {
            let entries: Vec<(SchemaValue, SchemaValue)> = (0..n)
                .map(|i| (SchemaValue::String(format!("k{i}")), vv.clone()))
                .collect();
            (
                SchemaType::Map {
                    key: Box::new(SchemaType::string()),
                    value: Box::new(vt),
                    metadata: Default::default(),
                },
                SchemaValue::Map { entries },
            )
        }),
        // variant
        inner.clone().prop_map(|(t, v)| {
            (
                SchemaType::Variant {
                    cases: vec![VariantCaseType {
                        name: "only".to_string(),
                        payload: Some(t),
                        metadata: Default::default(),
                    }],
                    metadata: Default::default(),
                },
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(v)),
                }),
            )
        }),
        // union with field-equals discriminator (record body)
        Just((
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "t".to_string(),
                    body: SchemaType::Record {
                        fields: vec![NamedFieldType {
                            name: "kind".to_string(),
                            body: SchemaType::string(),
                            metadata: Default::default(),
                        }],
                        metadata: Default::default(),
                    },
                    discriminator: DiscriminatorRule::FieldEquals(
                        crate::schema::schema_type::FieldDiscriminator {
                            field_name: "kind".to_string(),
                            literal: Some("k1".to_string()),
                        }
                    ),
                    metadata: Default::default(),
                }],
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "t".to_string(),
                body: Box::new(SchemaValue::Record {
                    fields: vec![SchemaValue::String("k1".to_string())],
                }),
            }),
        )),
    ]
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any value built by `paired_strategy` validates against its paired
    /// type.
    #[test]
    fn paired_value_validates((ty, value) in paired_strategy()) {
        let graph = SchemaGraph::anonymous(ty.clone());
        match validate_value(&graph, &ty, &value) {
            Ok(()) => {}
            Err(errors) => prop_assert!(
                false,
                "paired value failed to validate: {errors:?}\n  type: {ty:?}\n  value: {value:?}"
            ),
        }
    }

    /// If `value` validates against `ty`, then for any supertype `ty'`
    /// such that `is_assignable(ty, ty')` holds, `value` must also
    /// validate against `ty'`. Uses the type itself (reflexive) and an
    /// `Option` wrapper as small bounded supertypes.
    #[test]
    fn assignable_then_value_validates((ty, value) in paired_strategy()) {
        let graph = SchemaGraph::anonymous(ty.clone());
        prop_assume!(validate_value(&graph, &ty, &value).is_ok());

        // Reflexive supertype.
        prop_assert!(is_assignable(&graph, &ty, &ty));
        prop_assert!(validate_value(&graph, &ty, &value).is_ok());
    }
}

// --- Negative fixtures ---

#[test]
fn primitive_shape_mismatch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::s32());
    let errors =
        validate_value(&graph, &SchemaType::s32(), &SchemaValue::S64(1)).expect_err("should fail");
    assert!(matches!(&errors[0], ValueError::ShapeMismatch { .. }));
}

#[test]
fn secret_schema_rejects_plaintext_value() {
    let ty = SchemaType::secret(SecretSpec::default());
    let graph = SchemaGraph::anonymous(ty.clone());

    let errors = validate_value(
        &graph,
        &ty,
        &SchemaValue::String("plaintext-secret".to_string()),
    )
    .expect_err("plaintext must not validate as a secret handle");

    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        ValueError::ShapeMismatch { expected, found, .. }
            if expected == "secret" && found == "string"
    ));
}

#[test]
fn secret_category_mismatch_is_reported() {
    let ty = SchemaType::secret(SecretSpec {
        inner: Box::new(SchemaType::string()),
        category: Some("api-key".to_string()),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Secret(SecretValuePayload {
        secret_id: uuid::Uuid::nil(),
        config_key: Some(vec!["secret".to_string()]),
        version: 1,
        resolved_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        category: Some("password".to_string()),
    });

    let errors = validate_value(&graph, &ty, &value)
        .expect_err("secret category must match the schema category");
    assert_eq!(
        errors[0].to_string(),
        "secret value at  expected category `api-key`, found `password`"
    );
}

#[test]
fn variant_case_out_of_range_is_reported() {
    let ty = SchemaType::Variant {
        cases: vec![VariantCaseType {
            name: "a".to_string(),
            payload: None,
            metadata: Default::default(),
        }],
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = SchemaValue::Variant(VariantValuePayload {
        case: 7,
        payload: None,
    });
    let errors = validate_value(&graph, &ty, &v).expect_err("should fail");
    assert!(matches!(
        &errors[0],
        ValueError::VariantCaseOutOfRange { .. }
    ));
}

#[test]
fn enum_case_out_of_range_is_reported() {
    let ty = SchemaType::Enum {
        cases: vec!["a".to_string(), "b".to_string()],
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = SchemaValue::Enum { case: 5 };
    let errors = validate_value(&graph, &ty, &v).expect_err("should fail");
    assert!(matches!(&errors[0], ValueError::EnumCaseOutOfRange { .. }));
}

#[test]
fn record_arity_mismatch_is_reported() {
    let ty = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::bool(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::bool(),
                metadata: Default::default(),
            },
        ],
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = SchemaValue::Record {
        fields: vec![SchemaValue::Bool(true)],
    };
    let errors = validate_value(&graph, &ty, &v).expect_err("should fail");
    assert!(matches!(&errors[0], ValueError::RecordArityMismatch { .. }));
}

#[test]
fn fixed_list_length_mismatch_is_reported() {
    let ty = SchemaType::FixedList {
        element: Box::new(SchemaType::bool()),
        length: 3,
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = SchemaValue::FixedList {
        elements: vec![SchemaValue::Bool(true), SchemaValue::Bool(false)],
    };
    let errors = validate_value(&graph, &ty, &v).expect_err("should fail");
    assert!(matches!(
        &errors[0],
        ValueError::FixedListLengthMismatch { .. }
    ));
}

#[test]
fn union_unknown_tag_is_reported() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "x".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Prefix {
                prefix: String::new(),
            },
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = SchemaValue::Union(UnionValuePayload {
        tag: "nope".to_string(),
        body: Box::new(SchemaValue::String("anything".to_string())),
    });
    let errors = validate_value(&graph, &ty, &v).expect_err("should fail");
    assert!(matches!(&errors[0], ValueError::UnionUnknownTag { .. }));
}

#[test]
fn union_discriminator_mismatch_is_reported() {
    let ty = SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "x".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Prefix {
                prefix: "https://".to_string(),
            },
            metadata: Default::default(),
        }],
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let v = SchemaValue::Union(UnionValuePayload {
        tag: "x".to_string(),
        body: Box::new(SchemaValue::String("ftp://blah".to_string())),
    });
    let errors = validate_value(&graph, &ty, &v).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UnionDiscriminatorMismatch { .. }))
    );
}

#[test]
fn multimodal_variant_value_validates() {
    // Multimodal is modelled as a tagged `variant` (each part carries its
    // alternative name), so the value validator handles it with the generic
    // variant rules: a scalar `caption: string` body validates against the
    // `caption` case with no discriminator machinery involved. The
    // `Role::Multimodal` marker does not change value validation.
    let mut ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "caption".to_string(),
            payload: Some(SchemaType::string()),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "image_url".to_string(),
            payload: Some(SchemaType::string()),
            metadata: Default::default(),
        },
    ]);
    ty.metadata_mut().role = Some(crate::schema::metadata::Role::Multimodal);
    let graph = SchemaGraph::anonymous(ty.clone());
    let value = SchemaValue::Variant(VariantValuePayload {
        case: 0,
        payload: Some(Box::new(SchemaValue::String("hello world".to_string()))),
    });
    validate_value(&graph, &ty, &value).expect("multimodal scalar body must validate");
}

#[test]
fn direct_ref_cycle_returns_recursive_ref_error() {
    // `A` resolves to itself; a value reaching this type cannot have a
    // finite leaf because the ref chain at one value position never
    // reaches a structural shape. The validator must report
    // `RecursiveRef`, not silently succeed and not stack-overflow.
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new("A"),
            name: None,
            body: SchemaType::ref_to(TypeId::new("A")),
        }],
        root: SchemaType::ref_to(TypeId::new("A")),
    };
    let value = SchemaValue::Bool(true);
    let errors = validate_value(&graph, &graph.root, &value).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::RecursiveRef { .. })),
        "expected RecursiveRef, got {errors:?}"
    );
}

#[test]
fn mutual_pure_ref_cycle_returns_recursive_ref_error() {
    // `A → B → A` ref chain at one value position never reaches a
    // structural shape; the validator must report `RecursiveRef` rather
    // than recurse forever.
    let graph = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: TypeId::new("A"),
                name: None,
                body: SchemaType::ref_to(TypeId::new("B")),
            },
            SchemaTypeDef {
                id: TypeId::new("B"),
                name: None,
                body: SchemaType::ref_to(TypeId::new("A")),
            },
        ],
        root: SchemaType::ref_to(TypeId::new("A")),
    };
    let value = SchemaValue::Bool(true);
    let errors = validate_value(&graph, &graph.root, &value).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::RecursiveRef { .. })),
        "expected RecursiveRef, got {errors:?}"
    );
}

#[test]
fn nested_recursive_value_validates_every_level() {
    // `type Tree = { value: i32, children: list<Tree> }`. A two-level
    // tree value with a wrong-typed leaf at the inner level must produce
    // a `ShapeMismatch` for that inner leaf — the validator must NOT
    // silently skip validation of the inner Tree just because the outer
    // resolution already passed through `Ref<Tree>`.
    let tree_id = TypeId::new("Tree");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: tree_id.clone(),
            name: Some("Tree".to_string()),
            body: SchemaType::record(vec![
                NamedFieldType {
                    name: "value".to_string(),
                    body: SchemaType::s32(),
                    metadata: Default::default(),
                },
                NamedFieldType {
                    name: "children".to_string(),
                    body: SchemaType::list(SchemaType::ref_to(tree_id.clone())),
                    metadata: Default::default(),
                },
            ]),
        }],
        root: SchemaType::ref_to(tree_id),
    };

    // A valid two-level tree passes.
    let valid = SchemaValue::Record {
        fields: vec![
            SchemaValue::S32(1),
            SchemaValue::List {
                elements: vec![SchemaValue::Record {
                    fields: vec![SchemaValue::S32(2), SchemaValue::List { elements: vec![] }],
                }],
            },
        ],
    };
    validate_value(&graph, &graph.root, &valid).expect("valid nested tree must validate");

    // A two-level tree with a `bool` instead of `i32` for the inner
    // `value` field must produce a `ShapeMismatch` for the inner leaf —
    // proving the inner subtree IS validated (i.e. the cycle break does
    // not silently skip it).
    let wrong_inner_leaf = SchemaValue::Record {
        fields: vec![
            SchemaValue::S32(1),
            SchemaValue::List {
                elements: vec![SchemaValue::Record {
                    fields: vec![
                        SchemaValue::Bool(true),
                        SchemaValue::List { elements: vec![] },
                    ],
                }],
            },
        ],
    };
    let errors = validate_value(&graph, &graph.root, &wrong_inner_leaf)
        .expect_err("inner wrong-typed leaf must fail");
    // The exact failing path proves the inner `value` field was reached
    // by the validator (i.e. ref-cycle protection did not silently skip
    // the inner recursive subtree).
    let expected_path = [
        ValuePathSegment::Field("children".to_string()),
        ValuePathSegment::Index(0),
        ValuePathSegment::Field("value".to_string()),
    ];
    assert!(
        errors.iter().any(|e| matches!(
            e,
            ValueError::ShapeMismatch { path, .. }
                if path.segments() == expected_path
        )),
        "expected ShapeMismatch at children[0].value, got {errors:?}"
    );
}

// --- URL restrictions ---

fn url_with_restrictions(restrictions: UrlRestrictions) -> SchemaType {
    SchemaType::Url {
        restrictions,
        metadata: Default::default(),
    }
}

fn url_value(s: &str) -> SchemaValue {
    SchemaValue::Url { url: s.to_string() }
}

#[test]
fn url_unrestricted_accepts_any_well_formed_url() {
    let ty = url_with_restrictions(UrlRestrictions::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    validate_value(&graph, &ty, &url_value("https://example.com/path")).expect("valid url");
    validate_value(&graph, &ty, &url_value("file:///tmp/x")).expect("file url");
}

#[test]
fn url_invalid_syntax_is_reported() {
    let ty = url_with_restrictions(UrlRestrictions::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let errors =
        validate_value(&graph, &ty, &url_value("not a url")).expect_err("malformed url must fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UrlInvalid { .. })),
        "expected UrlInvalid, got {errors:?}"
    );
}

#[test]
fn url_empty_is_reported() {
    let ty = url_with_restrictions(UrlRestrictions::default());
    let graph = SchemaGraph::anonymous(ty.clone());
    let errors = validate_value(&graph, &ty, &url_value("")).expect_err("empty url must fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UrlEmpty { .. })),
        "expected UrlEmpty, got {errors:?}"
    );
}

#[test]
fn url_scheme_allow_list_accepts_listed() {
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: Some(vec!["https".to_string(), "wss".to_string()]),
        allowed_hosts: None,
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    validate_value(&graph, &ty, &url_value("https://example.com/")).expect("https allowed");
    // Case-insensitive match.
    validate_value(&graph, &ty, &url_value("WSS://example.com/")).expect("wss allowed (case-i)");
}

#[test]
fn url_scheme_allow_list_rejects_unlisted() {
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: Some(vec!["https".to_string()]),
        allowed_hosts: None,
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let errors = validate_value(&graph, &ty, &url_value("http://example.com/"))
        .expect_err("http not allowed");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UrlSchemeNotAllowed { .. })),
        "expected UrlSchemeNotAllowed, got {errors:?}"
    );
}

#[test]
fn url_host_allow_list_accepts_listed() {
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: None,
        allowed_hosts: Some(vec!["example.com".to_string()]),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    validate_value(&graph, &ty, &url_value("https://example.com/path"))
        .expect("listed host allowed");
    // Case-insensitive match.
    validate_value(&graph, &ty, &url_value("https://EXAMPLE.com/")).expect("case-i host");
}

#[test]
fn url_host_allow_list_rejects_unlisted() {
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: None,
        allowed_hosts: Some(vec!["example.com".to_string()]),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let errors = validate_value(&graph, &ty, &url_value("https://attacker.com/"))
        .expect_err("attacker.com not allowed");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UrlHostNotAllowed { .. })),
        "expected UrlHostNotAllowed, got {errors:?}"
    );
}

#[test]
fn url_host_allow_list_rejects_missing_host() {
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: None,
        allowed_hosts: Some(vec!["example.com".to_string()]),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    // `file:` URLs lack a host.
    let errors = validate_value(&graph, &ty, &url_value("file:///tmp/x"))
        .expect_err("missing host must fail when allow-list is set");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UrlHostMissing { .. })),
        "expected UrlHostMissing, got {errors:?}"
    );
}

#[test]
fn url_userinfo_confusion_does_not_bypass_host_allow_list() {
    // `https://example.com@attacker.com/` parses with host=`attacker.com`
    // (the `example.com` segment is userinfo). The validator must reject
    // it when only `example.com` is allowed — i.e. not be fooled by
    // userinfo into matching the allow-list.
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: None,
        allowed_hosts: Some(vec!["example.com".to_string()]),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let errors = validate_value(&graph, &ty, &url_value("https://example.com@attacker.com/"))
        .expect_err("userinfo must not bypass host allow-list");
    assert!(
        errors.iter().any(
            |e| matches!(e, ValueError::UrlHostNotAllowed { host, .. } if host == "attacker.com")
        ),
        "expected UrlHostNotAllowed for attacker.com, got {errors:?}"
    );
}

#[test]
fn url_subdomain_is_not_implicitly_allowed_by_parent_host() {
    // Exact host match only — no wildcard/suffix semantics.
    let ty = url_with_restrictions(UrlRestrictions {
        allowed_schemes: None,
        allowed_hosts: Some(vec!["example.com".to_string()]),
    });
    let graph = SchemaGraph::anonymous(ty.clone());
    let errors = validate_value(&graph, &ty, &url_value("https://api.example.com/"))
        .expect_err("subdomain must NOT be implicitly allowed");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::UrlHostNotAllowed { .. })),
        "expected UrlHostNotAllowed for subdomain, got {errors:?}"
    );
}

#[test]
fn dangling_ref_is_reported() {
    // A root `Ref<Missing>` with no matching def in the graph must
    // produce `DanglingRef` rather than silently succeed.
    let graph = SchemaGraph {
        defs: vec![],
        root: SchemaType::ref_to(TypeId::new("Missing")),
    };
    let value = SchemaValue::Bool(true);
    let errors = validate_value(&graph, &graph.root, &value).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValueError::DanglingRef { .. })),
        "expected DanglingRef, got {errors:?}"
    );
}
