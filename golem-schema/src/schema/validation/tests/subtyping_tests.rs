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
    BinaryRestrictions, NamedFieldType, QuantitySpec, QuantityValue, SchemaType, TextRestrictions,
    UrlRestrictions, VariantCaseType,
};
use crate::schema::validation::subtyping::{is_assignable, is_equivalent_cross_graph};
use proptest::prelude::*;
use test_r::test;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Subtyping is reflexive on well-formed graphs.
    #[test]
    fn reflexivity(graph in wellformed_schema_graph_strategy()) {
        prop_assert!(is_assignable(&graph, &graph.root, &graph.root));
        for def in &graph.defs {
            prop_assert!(is_assignable(&graph, &def.body, &def.body));
        }
    }
}

// Generators for narrowing chains used by the transitivity test.
fn text_chain() -> impl Strategy<Value = (SchemaType, SchemaType, SchemaType)> {
    // C = unrestricted, B = narrower, A = narrowest.
    (10u32..20, 50u32..100).prop_map(|(min, max)| {
        let c = SchemaType::text(TextRestrictions::default());
        let b = SchemaType::text(TextRestrictions {
            languages: None,
            min_length: Some(min),
            max_length: Some(max),
            regex: None,
        });
        let a = SchemaType::text(TextRestrictions {
            languages: None,
            min_length: Some(min + 1),
            max_length: Some(max - 1),
            regex: None,
        });
        (a, b, c)
    })
}

fn binary_chain() -> impl Strategy<Value = (SchemaType, SchemaType, SchemaType)> {
    (10u32..20, 50u32..100).prop_map(|(min, max)| {
        let c = SchemaType::binary(BinaryRestrictions::default());
        let b = SchemaType::binary(BinaryRestrictions {
            mime_types: None,
            min_bytes: Some(min),
            max_bytes: Some(max),
        });
        let a = SchemaType::binary(BinaryRestrictions {
            mime_types: None,
            min_bytes: Some(min + 1),
            max_bytes: Some(max - 1),
        });
        (a, b, c)
    })
}

fn quantity_chain() -> impl Strategy<Value = (SchemaType, SchemaType, SchemaType)> {
    (1i64..10, 50i64..100).prop_map(|(small, large)| {
        let base = "kg".to_string();
        let qv = |m: i64| QuantityValue {
            mantissa: m,
            scale: 0,
            unit: base.clone(),
        };
        let c = SchemaType::quantity(QuantitySpec {
            base_unit: base.clone(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        });
        let b = SchemaType::quantity(QuantitySpec {
            base_unit: base.clone(),
            allowed_suffixes: vec![],
            min: Some(qv(small)),
            max: Some(qv(large)),
        });
        let a = SchemaType::quantity(QuantitySpec {
            base_unit: base.clone(),
            allowed_suffixes: vec![],
            min: Some(qv(small + 1)),
            max: Some(qv(large - 1)),
        });
        (a, b, c)
    })
}

fn url_chain() -> impl Strategy<Value = (SchemaType, SchemaType, SchemaType)> {
    Just((
        SchemaType::url(UrlRestrictions {
            allowed_schemes: Some(vec!["https".to_string()]),
            allowed_hosts: Some(vec!["a.example".to_string()]),
        }),
        SchemaType::url(UrlRestrictions {
            allowed_schemes: Some(vec!["https".to_string(), "http".to_string()]),
            allowed_hosts: Some(vec!["a.example".to_string(), "b.example".to_string()]),
        }),
        SchemaType::url(UrlRestrictions {
            allowed_schemes: None,
            allowed_hosts: None,
        }),
    ))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Bounded transitivity: A is a narrowing of B is a narrowing of C, so A ⊑ C.
    #[test]
    fn transitivity_text((a, b, c) in text_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::bool());
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }

    #[test]
    fn transitivity_binary((a, b, c) in binary_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::bool());
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }

    #[test]
    fn transitivity_quantity((a, b, c) in quantity_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::bool());
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }

    #[test]
    fn transitivity_url((a, b, c) in url_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::bool());
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }
}

#[test]
fn record_width_subtyping_is_rejected() {
    // Record subtyping is exact-match; width-subtyping must be rejected
    // because `SchemaValue::Record` is positional and does not carry field
    // names.
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let sub = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::bool(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::s32(),
                metadata: Default::default(),
            },
        ],
        metadata: Default::default(),
    };
    let sup = SchemaType::Record {
        fields: vec![NamedFieldType {
            name: "a".to_string(),
            body: SchemaType::bool(),
            metadata: Default::default(),
        }],
        metadata: Default::default(),
    };
    assert!(!is_assignable(&graph, &sub, &sup));
    assert!(!is_assignable(&graph, &sup, &sub));
}

#[test]
fn record_field_reordering_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let sub = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::bool(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::s32(),
                metadata: Default::default(),
            },
        ],
        metadata: Default::default(),
    };
    let sup = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::s32(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::bool(),
                metadata: Default::default(),
            },
        ],
        metadata: Default::default(),
    };
    assert!(!is_assignable(&graph, &sub, &sup));
}

#[test]
fn list_depth_subtyping() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let sub = SchemaType::List {
        element: Box::new(SchemaType::text(TextRestrictions {
            languages: None,
            min_length: Some(5),
            max_length: Some(10),
            regex: None,
        })),
        metadata: Default::default(),
    };
    let sup = SchemaType::List {
        element: Box::new(SchemaType::text(TextRestrictions::default())),
        metadata: Default::default(),
    };
    assert!(is_assignable(&graph, &sub, &sup));
}

#[test]
fn cycle_does_not_loop() {
    // Mutually recursive defs: A points to B, B points to A.
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
    // Should terminate (and accept under coinductive assumption).
    assert!(is_assignable(
        &graph,
        &SchemaType::ref_to(TypeId::new("A")),
        &SchemaType::ref_to(TypeId::new("A"))
    ));
}

#[test]
fn primitive_kind_mismatch_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    assert!(!is_assignable(
        &graph,
        &SchemaType::s32(),
        &SchemaType::s64()
    ));
}

#[test]
fn quantity_suffix_subset_is_enforced() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let sup = SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec!["kg".to_string(), "g".to_string()],
        min: None,
        max: None,
    });
    let sub_ok = SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec!["kg".to_string()],
        min: None,
        max: None,
    });
    let sub_not = SchemaType::quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec!["kg".to_string(), "lb".to_string()],
        min: None,
        max: None,
    });
    assert!(is_assignable(&graph, &sub_ok, &sup));
    assert!(!is_assignable(&graph, &sub_not, &sup));
}

// --- Cross-graph structural equivalence (`is_equivalent_cross_graph`) ---
//
// This is the strict compatibility gate used when an agent type's config
// declarations change: a stored, positional config value may only be
// reinterpreted under the new declaration when the two types are
// structurally identical across the (possibly different) graphs.

fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: Default::default(),
    }
}

/// `Node = record { <value_field>: s32, next: option<ref Node> }` rooted at
/// `ref Node`, with the def id and the first field name parameterized so we
/// can build structurally-identical-but-differently-named graphs.
fn recursive_list_graph(def_id: &str, value_field: &str) -> SchemaGraph {
    SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::new(def_id),
            name: None,
            body: SchemaType::record(vec![
                field(value_field, SchemaType::s32()),
                field(
                    "next",
                    SchemaType::option(SchemaType::ref_to(TypeId::new(def_id))),
                ),
            ]),
        }],
        root: SchemaType::ref_to(TypeId::new(def_id)),
    }
}

fn equiv(a: &SchemaGraph, b: &SchemaGraph) -> bool {
    is_equivalent_cross_graph(a, &a.root, b, &b.root)
}

#[test]
fn cross_graph_identical_scalar_accepts() {
    let a = SchemaGraph::anonymous(SchemaType::bool());
    let b = SchemaGraph::anonymous(SchemaType::bool());
    assert!(equiv(&a, &b));
}

#[test]
fn cross_graph_primitive_kind_mismatch_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::s32());
    let b = SchemaGraph::anonymous(SchemaType::s64());
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_record_identical_accepts() {
    let rec = || {
        SchemaType::record(vec![
            field("a", SchemaType::bool()),
            field("b", SchemaType::s32()),
        ])
    };
    let a = SchemaGraph::anonymous(rec());
    let b = SchemaGraph::anonymous(rec());
    assert!(equiv(&a, &b));
}

#[test]
fn cross_graph_record_field_rename_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::record(vec![
        field("a", SchemaType::bool()),
        field("b", SchemaType::s32()),
    ]));
    let b = SchemaGraph::anonymous(SchemaType::record(vec![
        field("a", SchemaType::bool()),
        field("renamed", SchemaType::s32()),
    ]));
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_record_field_reorder_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::record(vec![
        field("a", SchemaType::bool()),
        field("b", SchemaType::s32()),
    ]));
    let b = SchemaGraph::anonymous(SchemaType::record(vec![
        field("b", SchemaType::s32()),
        field("a", SchemaType::bool()),
    ]));
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_record_width_change_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::record(vec![
        field("a", SchemaType::bool()),
        field("b", SchemaType::s32()),
    ]));
    let b = SchemaGraph::anonymous(SchemaType::record(vec![field("a", SchemaType::bool())]));
    assert!(!equiv(&a, &b));
    assert!(!equiv(&b, &a));
}

#[test]
fn cross_graph_enum_case_rename_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec!["red".to_string(), "green".to_string()],
        metadata: Default::default(),
    });
    let b = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec!["red".to_string(), "blue".to_string()],
        metadata: Default::default(),
    });
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_enum_reorder_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec!["red".to_string(), "green".to_string()],
        metadata: Default::default(),
    });
    let b = SchemaGraph::anonymous(SchemaType::Enum {
        cases: vec!["green".to_string(), "red".to_string()],
        metadata: Default::default(),
    });
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_variant_case_rename_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::variant(vec![
        VariantCaseType {
            name: "ok".to_string(),
            payload: Some(SchemaType::s32()),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "err".to_string(),
            payload: None,
            metadata: Default::default(),
        },
    ]));
    let b = SchemaGraph::anonymous(SchemaType::variant(vec![
        VariantCaseType {
            name: "ok".to_string(),
            payload: Some(SchemaType::s32()),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: "failure".to_string(),
            payload: None,
            metadata: Default::default(),
        },
    ]));
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_variant_payload_type_change_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::variant(vec![VariantCaseType {
        name: "ok".to_string(),
        payload: Some(SchemaType::s32()),
        metadata: Default::default(),
    }]));
    let b = SchemaGraph::anonymous(SchemaType::variant(vec![VariantCaseType {
        name: "ok".to_string(),
        payload: Some(SchemaType::s64()),
        metadata: Default::default(),
    }]));
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_scalar_restriction_difference_rejected() {
    let a = SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
        languages: None,
        min_length: Some(1),
        max_length: Some(10),
        regex: None,
    }));
    let b = SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
        languages: None,
        min_length: Some(2),
        max_length: Some(10),
        regex: None,
    }));
    // Even though `a` would be *assignable* to a looser text type, exact
    // equivalence rejects any restriction difference.
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_recursive_identical_accepts() {
    let a = recursive_list_graph("Node", "value");
    let b = recursive_list_graph("Node", "value");
    assert!(equiv(&a, &b));
}

#[test]
fn cross_graph_recursive_identical_with_different_def_ids_accepts() {
    // The realistic migration case: the updated agent graph may use a
    // different internal def id for the same structural type. Equivalence
    // must look through the def id and compare structure.
    let a = recursive_list_graph("NodeOld", "value");
    let b = recursive_list_graph("NodeNew", "value");
    assert!(equiv(&a, &b));
}

#[test]
fn cross_graph_recursive_rename_in_cycle_rejected() {
    // A field rename inside a recursive cycle must still be detected; the
    // coinductive cycle break only short-circuits re-entry, not the first
    // (acyclic) comparison of the recursive body.
    let a = recursive_list_graph("Node", "value");
    let b = recursive_list_graph("Node", "renamed");
    assert!(!equiv(&a, &b));
}

#[test]
fn cross_graph_mutually_recursive_identical_accepts() {
    let mutual = || SchemaGraph {
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
    let a = mutual();
    let b = mutual();
    assert!(equiv(&a, &b));
}
