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

use super::wellformed_strategy::wellformed_schema_graph_strategy;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, NumericBound,
    NumericRestrictionError, NumericRestrictions, QuantitySpec, QuantityValue, SchemaType,
    SecretSpec, TextRestrictions, UnionBranch, UnionSpec, VariantCaseType,
};
use crate::schema::validation::well_formedness::{SchemaError, validate_graph, validate_root_type};
use proptest::prelude::*;
use test_r::test;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Well-formed graphs produced by `wellformed_schema_graph_strategy`
    /// always pass validation. The validator is idempotent on accepted input.
    #[test]
    fn wellformed_strategy_always_validates(graph in wellformed_schema_graph_strategy()) {
        match validate_graph(&graph) {
            Ok(()) => {}
            Err(errors) => prop_assert!(
                false,
                "expected wellformed graph to validate, got errors: {errors:?}\n  graph: {graph:?}"
            ),
        }
    }
}

#[test]
fn duplicate_type_id_is_reported() {
    let graph = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: TypeId::new("dup"),
                name: None,
                body: SchemaType::bool(),
            },
            SchemaTypeDef {
                id: TypeId::new("dup"),
                name: None,
                body: SchemaType::s32(),
            },
        ],
        root: SchemaType::ref_to(TypeId::new("dup")),
    };
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateTypeId(TypeId::new("dup"))));
}

#[test]
fn dangling_ref_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::ref_to(TypeId::new("missing")));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DanglingRef(TypeId::new("missing"))));
}

#[test]
fn validate_root_type_reports_dangling_ref_through_alias_chain() {
    let alias = TypeId::new("alias");
    let missing = TypeId::new("missing");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: alias.clone(),
            name: None,
            body: SchemaType::ref_to(missing.clone()),
        }],
        root: SchemaType::bool(),
    };

    let errors = validate_root_type(&graph, &SchemaType::ref_to(alias))
        .expect_err("a root alias chain ending in a missing definition is ill-formed");
    assert!(
        errors.contains(&SchemaError::DanglingRef(missing)),
        "expected the dangling target to be reported, got {errors:?}"
    );
}

#[test]
fn pure_recursive_alias_root_is_rejected() {
    let id = TypeId::new("Cycle");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::ref_to(id.clone()),
        }],
        root: SchemaType::ref_to(id),
    };

    assert!(
        validate_graph(&graph).is_err(),
        "a pure recursive alias never resolves to a concrete schema type and must be rejected"
    );
}

#[test]
fn mutual_pure_alias_cycle_is_rejected() {
    // A -> ref B, B -> ref A: a two-step pure alias cycle that never bottoms
    // out in a concrete type.
    let a = TypeId::new("A");
    let b = TypeId::new("B");
    let graph = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: a.clone(),
                name: None,
                body: SchemaType::ref_to(b.clone()),
            },
            SchemaTypeDef {
                id: b,
                name: None,
                body: SchemaType::ref_to(a.clone()),
            },
        ],
        root: SchemaType::ref_to(a),
    };
    let errors = validate_graph(&graph).expect_err("mutual pure alias cycle must be rejected");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::RecursiveAlias(_))),
        "expected a RecursiveAlias error, got {errors:?}"
    );
}

#[test]
fn legitimate_recursive_type_through_constructor_is_accepted() {
    // tree -> record { children: list<ref tree> }: the cycle passes through
    // value-shrinking constructors (record/list), so it resolves to a concrete
    // type and is valid.
    let id = TypeId::new("tree");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::record(vec![NamedFieldType {
                name: "children".to_string(),
                body: SchemaType::list(SchemaType::ref_to(id.clone())),
                metadata: Default::default(),
            }]),
        }],
        root: SchemaType::ref_to(id),
    };
    assert!(
        validate_graph(&graph).is_ok(),
        "a recursive type whose cycle passes through a constructor is well-formed"
    );
}

#[test]
fn dangling_ref_in_secret_inner_is_reported() {
    let missing = TypeId::new("missing-secret-inner");
    let graph = SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
        inner: Box::new(SchemaType::ref_to(missing.clone())),
        category: None,
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DanglingRef(missing)));
}

#[test]
fn empty_variant_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Variant {
        cases: vec![],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyVariant));
}

#[test]
fn empty_enum_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec![],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyEnum));
}

#[test]
fn empty_union_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec { branches: vec![] }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyUnion));
}

#[test]
fn duplicate_record_field_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::bool(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::s32(),
                metadata: Default::default(),
            },
        ],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateFieldName("a".to_string())));
}

#[test]
fn map_key_not_primitive_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Map {
        key: Box::new(SchemaType::Record {
            fields: vec![],
            metadata: Default::default(),
        }),
        value: Box::new(SchemaType::bool()),
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::MapKeyNotPrimitive));
}

#[test]
fn recursive_alias_map_key_reports_only_recursive_alias() {
    let key = TypeId::new("key");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: key.clone(),
            name: None,
            body: SchemaType::ref_to(key.clone()),
        }],
        root: SchemaType::bool(),
    };

    let errors = validate_root_type(
        &graph,
        &SchemaType::map(SchemaType::ref_to(key.clone()), SchemaType::string()),
    )
    .expect_err("recursive alias map key must be rejected");

    assert_eq!(
        errors,
        vec![SchemaError::RecursiveAlias(key)],
        "a recursive alias map key should not also cascade into MapKeyNotPrimitive"
    );
}

#[test]
fn fixed_list_zero_length_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::FixedList {
        element: Box::new(SchemaType::bool()),
        length: 0,
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::FixedListZeroLength));
}

#[test]
fn quantity_min_gt_max_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec![],
        min: Some(QuantityValue {
            mantissa: 10,
            scale: 0,
            unit: "kg".to_string(),
        }),
        max: Some(QuantityValue {
            mantissa: 1,
            scale: 0,
            unit: "kg".to_string(),
        }),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::QuantityMinGreaterThanMax));
}

#[test]
fn quantity_unit_mismatch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec![],
        min: Some(QuantityValue {
            mantissa: 1,
            scale: 0,
            unit: "g".to_string(),
        }),
        max: Some(QuantityValue {
            mantissa: 10,
            scale: 0,
            unit: "kg".to_string(),
        }),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::QuantityMinUnitMismatch { .. }))
    );
}

#[test]
fn string_pattern_rule_on_record_body_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
            discriminator: DiscriminatorRule::Prefix {
                prefix: "x".to_string(),
            },
            metadata: Default::default(),
        }],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors.contains(&SchemaError::UnionStringRuleOnNonStringBody {
            tag: "t".to_string(),
        })
    );
}

#[test]
fn field_equals_literal_on_non_string_field_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record {
                fields: vec![NamedFieldType {
                    name: "n".to_string(),
                    body: SchemaType::s32(),
                    metadata: Default::default(),
                }],
                metadata: Default::default(),
            },
            discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: "n".to_string(),
                literal: Some("x".to_string()),
            }),
            metadata: Default::default(),
        }],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors.contains(&SchemaError::UnionFieldEqualsLiteralOnNonStringField {
            tag: "t".to_string(),
            field_name: "n".to_string(),
        })
    );
}

#[test]
fn field_rule_on_non_record_body_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::s32(),
            discriminator: DiscriminatorRule::FieldAbsent {
                field_name: "x".to_string(),
            },
            metadata: Default::default(),
        }],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors.contains(&SchemaError::UnionFieldRuleOnNonRecordBody {
            tag: "t".to_string(),
        })
    );
}

#[test]
fn ref_resolution_in_union_branch_body() {
    // Branch body is a Ref to a String def — should resolve and accept a
    // string-pattern rule.
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new("S"),
            name: None,
            body: SchemaType::string(),
        }],
        root: SchemaType::union(UnionSpec {
            branches: vec![UnionBranch {
                tag: "t".to_string(),
                body: SchemaType::ref_to(TypeId::new("S")),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "x".to_string(),
                },
                metadata: Default::default(),
            }],
        }),
    };
    assert!(validate_graph(&graph).is_ok());
}

#[test]
fn empty_flags_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Flags {
        flags: vec![],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyFlags));
}

#[test]
fn duplicate_variant_case_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Variant {
        cases: vec![
            VariantCaseType {
                name: "a".to_string(),
                payload: None,
                metadata: Default::default(),
            },
            VariantCaseType {
                name: "a".to_string(),
                payload: None,
                metadata: Default::default(),
            },
        ],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateVariantCase("a".to_string())));
}

#[test]
fn duplicate_enum_case_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec!["x".to_string(), "x".to_string()],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateEnumCase("x".to_string())));
}

#[test]
fn duplicate_flag_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Flags {
        flags: vec!["f".to_string(), "f".to_string()],
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateFlagName("f".to_string())));
}

#[test]
fn duplicate_union_tag_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "x".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "alpha".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "x".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "beta".to_string(),
                },
                metadata: Default::default(),
            },
        ],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateUnionTag("x".to_string())));
}

#[test]
fn field_equals_missing_field_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
            discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                field_name: "missing".to_string(),
                literal: None,
            }),
            metadata: Default::default(),
        }],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::UnionFieldRuleMissingField {
        tag: "t".to_string(),
        field_name: "missing".to_string(),
    }));
}

#[test]
fn map_key_ref_to_string_is_accepted() {
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new("StringDef"),
            name: None,
            body: SchemaType::string(),
        }],
        root: SchemaType::Map {
            key: Box::new(SchemaType::ref_to(TypeId::new("StringDef"))),
            value: Box::new(SchemaType::bool()),
            metadata: Default::default(),
        },
    };
    assert!(validate_graph(&graph).is_ok());
}

#[test]
fn one_sided_quantity_min_unit_mismatch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec![],
        min: Some(QuantityValue {
            mantissa: 1,
            scale: 0,
            unit: "g".to_string(),
        }),
        max: None,
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::QuantityMinUnitMismatch { .. }))
    );
}

#[test]
fn one_sided_quantity_max_unit_mismatch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec![],
        min: None,
        max: Some(QuantityValue {
            mantissa: 1,
            scale: 0,
            unit: "g".to_string(),
        }),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::QuantityMaxUnitMismatch { .. }))
    );
}

#[test]
fn quantity_comparison_overflow_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec![],
        min: Some(QuantityValue {
            mantissa: i64::MAX,
            scale: -38,
            unit: "kg".to_string(),
        }),
        max: Some(QuantityValue {
            mantissa: 1,
            scale: 0,
            unit: "kg".to_string(),
        }),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::QuantityComparisonOverflow { .. }))
    );
}

#[test]
fn union_discriminator_overlap_prefix_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "a".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "a".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "b".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "ab".to_string(),
                },
                metadata: Default::default(),
            },
        ],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::UnionAmbiguousDiscriminators { .. }))
    );
}

// Deferred: `discriminators_overlap` is intentionally conservative and only
// detects same-kind nesting/empties (regex overlap is undecidable). Detecting
// cross-kind overlaps such as prefix-vs-suffix is a separate union-ambiguity
// completeness effort that also requires redesigning the well-formed property
// generator (which deliberately relies on the conservative checker), so it is
// out of scope for the tool dangling/duplicate-detection work and tracked
// separately.
#[test]
#[ignore = "deferred: cross-kind discriminator overlap detection is a separate effort"]
fn union_discriminator_overlap_prefix_suffix_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "prefix".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "a".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "suffix".to_string(),
                body: SchemaType::string(),
                discriminator: DiscriminatorRule::Suffix {
                    suffix: "b".to_string(),
                },
                metadata: Default::default(),
            },
        ],
    }));

    let errors = validate_graph(&graph).expect_err("value `ab` matches both branches");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::UnionAmbiguousDiscriminators { .. })),
        "expected an ambiguous-discriminator error, got {errors:?}"
    );
}

#[test]
fn invalid_regex_on_union_branch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::string(),
            discriminator: DiscriminatorRule::Regex {
                regex: "(".to_string(),
            },
            metadata: Default::default(),
        }],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::InvalidRegex { .. }))
    );
}

#[test]
fn unsatisfiable_field_absent_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::union(UnionSpec {
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
            discriminator: DiscriminatorRule::FieldAbsent {
                field_name: "kind".to_string(),
            },
            metadata: Default::default(),
        }],
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::UnionUnsatisfiableFieldAbsent { .. }))
    );
}

#[test]
fn inverted_text_length_range_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
        languages: None,
        min_length: Some(20),
        max_length: Some(10),
        regex: None,
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::TextLengthRangeInverted));
}

#[test]
fn inverted_binary_byte_range_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions {
        mime_types: None,
        min_bytes: Some(100),
        max_bytes: Some(50),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::BinaryByteRangeInverted));
}

#[test]
fn invalid_text_regex_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
        languages: None,
        min_length: None,
        max_length: None,
        regex: Some("(".to_string()),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::InvalidTextRegex { .. }))
    );
}

#[test]
fn nested_option_of_option_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::Option {
        inner: Box::new(SchemaType::Option {
            inner: Box::new(SchemaType::u32()),
            metadata: Default::default(),
        }),
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::NullableNesting { .. }))
    );
}

#[test]
fn option_of_union_with_nullable_branch_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::Option {
        inner: Box::new(SchemaType::union(UnionSpec {
            branches: vec![UnionBranch {
                tag: "t".to_string(),
                body: SchemaType::Option {
                    inner: Box::new(SchemaType::u32()),
                    metadata: Default::default(),
                },
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "x".to_string(),
                },
                metadata: Default::default(),
            }],
        })),
        metadata: Default::default(),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::NullableNesting { .. }))
    );
}

#[test]
fn option_of_ref_resolving_to_option_is_rejected() {
    let inner_id = TypeId::new("Nullable");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: inner_id.clone(),
            name: None,
            body: SchemaType::Option {
                inner: Box::new(SchemaType::u32()),
                metadata: Default::default(),
            },
        }],
        root: SchemaType::Option {
            inner: Box::new(SchemaType::ref_to(inner_id)),
            metadata: Default::default(),
        },
    };
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SchemaError::NullableNesting { .. }))
    );
}

#[test]
fn option_of_self_recursive_ref_terminates() {
    // a -> Option<a> — pathological but valid in the sense that the
    // nullable-nesting check must terminate via cycle detection without
    // crashing. The graph itself is still rejected because the ref body is
    // an Option that wraps a nullable.
    let id = TypeId::new("Cycle");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::Option {
                inner: Box::new(SchemaType::ref_to(id.clone())),
                metadata: Default::default(),
            },
        }],
        root: SchemaType::ref_to(id),
    };
    let _ = validate_graph(&graph);
}

// --- Numeric restrictions ---

#[test]
fn numeric_valid_restrictions_pass() {
    let ty = SchemaType::U32 {
        restrictions: NumericRestrictions {
            min: Some(NumericBound::Unsigned(0)),
            max: Some(NumericBound::Unsigned(100)),
            unit: Some("items".to_string()),
        }
        .normalize(),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);
    validate_graph(&graph).expect("valid numeric restrictions must pass");
}

#[test]
fn numeric_min_greater_than_max_is_reported() {
    let ty = SchemaType::U32 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::Unsigned(10)),
            max: Some(NumericBound::Unsigned(1)),
            unit: None,
        }),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::InvalidNumericRestriction {
        error: NumericRestrictionError::MinGreaterThanMax,
    }));
}

#[test]
fn numeric_bound_family_mismatch_is_reported() {
    // A signed bound on an unsigned repr.
    let ty = SchemaType::U32 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::Signed(5)),
            max: None,
            unit: None,
        }),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::InvalidNumericRestriction {
        error: NumericRestrictionError::FamilyMismatch,
    }));
}

#[test]
fn numeric_bound_out_of_range_is_reported() {
    // 300 does not fit in u8.
    let ty = SchemaType::U8 {
        restrictions: Some(NumericRestrictions {
            min: None,
            max: Some(NumericBound::Unsigned(300)),
            unit: None,
        }),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::InvalidNumericRestriction {
        error: NumericRestrictionError::BoundOutOfRange,
    }));
}

#[test]
fn numeric_f32_bound_not_round_trippable_is_reported() {
    // 0.1 cannot be represented exactly in f32, so it does not round-trip.
    let ty = SchemaType::F32 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::float(0.1).unwrap()),
            max: None,
            unit: None,
        }),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::InvalidNumericRestriction {
        error: NumericRestrictionError::FloatNotRoundTrippable,
    }));
}

#[test]
fn numeric_stored_empty_restriction_is_reported() {
    // `Some(empty)` must never be stored; well-formedness rejects it even
    // though smart constructors/decoders normalize it away.
    let ty = SchemaType::U32 {
        restrictions: Some(NumericRestrictions::default()),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::InvalidNumericRestriction {
        error: NumericRestrictionError::EmptyStored,
    }));
}

#[test]
fn numeric_empty_unit_with_bound_is_accepted() {
    // An empty `unit` is *non-canonical* (the codecs normalize `Some("")` to
    // `None` on decode, see `serde_empty_unit_with_bound_drops_only_the_unit`),
    // but it is not *structurally invalid*: the restriction still carries a
    // meaningful bound. Well-formedness validates structural validity, not
    // canonical spelling — enforcing empty-unit canonicality through
    // `validate_for_repr` would make value validation skip numeric range checks
    // for such a type, which is worse than accepting the non-canonical unit.
    let ty = SchemaType::U32 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::Unsigned(1)),
            max: None,
            unit: Some(String::new()),
        }),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty);

    assert!(
        validate_graph(&graph).is_ok(),
        "a non-canonical empty unit alongside a real bound is structurally valid"
    );
}
