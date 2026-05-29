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
    BinaryRestrictions, DiscriminatorRule, FieldDiscriminator, NamedFieldType, QuantitySpec,
    QuantityValue, SchemaType, TextRestrictions, UnionBranch, UnionSpec, VariantCaseType,
};
use crate::schema::validation::well_formedness::{SchemaError, validate_graph};
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
                metadata: Default::default(),
                body: SchemaType::Bool,
            },
            SchemaTypeDef {
                id: TypeId::new("dup"),
                name: None,
                metadata: Default::default(),
                body: SchemaType::S32,
            },
        ],
        root: SchemaType::Ref(TypeId::new("dup")),
    };
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateTypeId(TypeId::new("dup"))));
}

#[test]
fn dangling_ref_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Ref(TypeId::new("missing")));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DanglingRef(TypeId::new("missing"))));
}

#[test]
fn empty_variant_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Variant { cases: vec![] });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyVariant));
}

#[test]
fn empty_enum_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Enum { cases: vec![] });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyEnum));
}

#[test]
fn empty_union_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec { branches: vec![] }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::EmptyUnion));
}

#[test]
fn duplicate_record_field_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::Bool,
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::S32,
                metadata: Default::default(),
            },
        ],
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateFieldName("a".to_string())));
}

#[test]
fn map_key_not_primitive_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Map {
        key: Box::new(SchemaType::Record { fields: vec![] }),
        value: Box::new(SchemaType::Bool),
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::MapKeyNotPrimitive));
}

#[test]
fn fixed_list_zero_length_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::FixedList {
        element: Box::new(SchemaType::Bool),
        length: 0,
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::FixedListZeroLength));
}

#[test]
fn quantity_min_gt_max_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Quantity(QuantitySpec {
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
    let graph = SchemaGraph::anonymous(SchemaType::Quantity(QuantitySpec {
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
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record { fields: vec![] },
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
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record {
                fields: vec![NamedFieldType {
                    name: "n".to_string(),
                    body: SchemaType::S32,
                    metadata: Default::default(),
                }],
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
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::S32,
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
            metadata: Default::default(),
            body: SchemaType::String,
        }],
        root: SchemaType::Union(UnionSpec {
            branches: vec![UnionBranch {
                tag: "t".to_string(),
                body: SchemaType::Ref(TypeId::new("S")),
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
    let graph = SchemaGraph::anonymous(SchemaType::Flags { flags: vec![] });
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
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateVariantCase("a".to_string())));
}

#[test]
fn duplicate_enum_case_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec!["x".to_string(), "x".to_string()],
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateEnumCase("x".to_string())));
}

#[test]
fn duplicate_flag_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Flags {
        flags: vec!["f".to_string(), "f".to_string()],
    });
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::DuplicateFlagName("f".to_string())));
}

#[test]
fn duplicate_union_tag_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "x".to_string(),
                body: SchemaType::String,
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "alpha".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "x".to_string(),
                body: SchemaType::String,
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
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record { fields: vec![] },
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
            metadata: Default::default(),
            body: SchemaType::String,
        }],
        root: SchemaType::Map {
            key: Box::new(SchemaType::Ref(TypeId::new("StringDef"))),
            value: Box::new(SchemaType::Bool),
        },
    };
    assert!(validate_graph(&graph).is_ok());
}

#[test]
fn one_sided_quantity_min_unit_mismatch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Quantity(QuantitySpec {
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
    let graph = SchemaGraph::anonymous(SchemaType::Quantity(QuantitySpec {
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
    let graph = SchemaGraph::anonymous(SchemaType::Quantity(QuantitySpec {
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
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "a".to_string(),
                body: SchemaType::String,
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "a".to_string(),
                },
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "b".to_string(),
                body: SchemaType::String,
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

#[test]
fn invalid_regex_on_union_branch_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::String,
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
    let graph = SchemaGraph::anonymous(SchemaType::Union(UnionSpec {
        branches: vec![UnionBranch {
            tag: "t".to_string(),
            body: SchemaType::Record {
                fields: vec![NamedFieldType {
                    name: "kind".to_string(),
                    body: SchemaType::String,
                    metadata: Default::default(),
                }],
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
    let graph = SchemaGraph::anonymous(SchemaType::Text(TextRestrictions {
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
    let graph = SchemaGraph::anonymous(SchemaType::Binary(BinaryRestrictions {
        mime_types: None,
        min_bytes: Some(100),
        max_bytes: Some(50),
    }));
    let errors = validate_graph(&graph).expect_err("should fail");
    assert!(errors.contains(&SchemaError::BinaryByteRangeInverted));
}

#[test]
fn invalid_text_regex_is_reported() {
    let graph = SchemaGraph::anonymous(SchemaType::Text(TextRestrictions {
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
            inner: Box::new(SchemaType::U32),
        }),
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
        inner: Box::new(SchemaType::Union(UnionSpec {
            branches: vec![UnionBranch {
                tag: "t".to_string(),
                body: SchemaType::Option {
                    inner: Box::new(SchemaType::U32),
                },
                discriminator: DiscriminatorRule::Prefix {
                    prefix: "x".to_string(),
                },
                metadata: Default::default(),
            }],
        })),
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
            metadata: Default::default(),
            body: SchemaType::Option {
                inner: Box::new(SchemaType::U32),
            },
        }],
        root: SchemaType::Option {
            inner: Box::new(SchemaType::Ref(inner_id)),
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
            metadata: Default::default(),
            body: SchemaType::Option {
                inner: Box::new(SchemaType::Ref(id.clone())),
            },
        }],
        root: SchemaType::Ref(id),
    };
    let _ = validate_graph(&graph);
}
