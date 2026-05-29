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
    UrlRestrictions,
};
use crate::schema::validation::subtyping::is_assignable;
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
        let c = SchemaType::Text(TextRestrictions::default());
        let b = SchemaType::Text(TextRestrictions {
            languages: None,
            min_length: Some(min),
            max_length: Some(max),
            regex: None,
        });
        let a = SchemaType::Text(TextRestrictions {
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
        let c = SchemaType::Binary(BinaryRestrictions::default());
        let b = SchemaType::Binary(BinaryRestrictions {
            mime_types: None,
            min_bytes: Some(min),
            max_bytes: Some(max),
        });
        let a = SchemaType::Binary(BinaryRestrictions {
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
        let c = SchemaType::Quantity(QuantitySpec {
            base_unit: base.clone(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        });
        let b = SchemaType::Quantity(QuantitySpec {
            base_unit: base.clone(),
            allowed_suffixes: vec![],
            min: Some(qv(small)),
            max: Some(qv(large)),
        });
        let a = SchemaType::Quantity(QuantitySpec {
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
        SchemaType::Url(UrlRestrictions {
            allowed_schemes: Some(vec!["https".to_string()]),
            allowed_hosts: Some(vec!["a.example".to_string()]),
        }),
        SchemaType::Url(UrlRestrictions {
            allowed_schemes: Some(vec!["https".to_string(), "http".to_string()]),
            allowed_hosts: Some(vec!["a.example".to_string(), "b.example".to_string()]),
        }),
        SchemaType::Url(UrlRestrictions {
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
        let graph = SchemaGraph::anonymous(SchemaType::Bool);
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }

    #[test]
    fn transitivity_binary((a, b, c) in binary_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::Bool);
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }

    #[test]
    fn transitivity_quantity((a, b, c) in quantity_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::Bool);
        prop_assert!(is_assignable(&graph, &a, &b));
        prop_assert!(is_assignable(&graph, &b, &c));
        prop_assert!(is_assignable(&graph, &a, &c));
    }

    #[test]
    fn transitivity_url((a, b, c) in url_chain()) {
        let graph = SchemaGraph::anonymous(SchemaType::Bool);
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
    let graph = SchemaGraph::anonymous(SchemaType::Bool);
    let sub = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::Bool,
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::S32,
                metadata: Default::default(),
            },
        ],
    };
    let sup = SchemaType::Record {
        fields: vec![NamedFieldType {
            name: "a".to_string(),
            body: SchemaType::Bool,
            metadata: Default::default(),
        }],
    };
    assert!(!is_assignable(&graph, &sub, &sup));
    assert!(!is_assignable(&graph, &sup, &sub));
}

#[test]
fn record_field_reordering_is_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::Bool);
    let sub = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::Bool,
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::S32,
                metadata: Default::default(),
            },
        ],
    };
    let sup = SchemaType::Record {
        fields: vec![
            NamedFieldType {
                name: "b".to_string(),
                body: SchemaType::S32,
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "a".to_string(),
                body: SchemaType::Bool,
                metadata: Default::default(),
            },
        ],
    };
    assert!(!is_assignable(&graph, &sub, &sup));
}

#[test]
fn list_depth_subtyping() {
    let graph = SchemaGraph::anonymous(SchemaType::Bool);
    let sub = SchemaType::List {
        element: Box::new(SchemaType::Text(TextRestrictions {
            languages: None,
            min_length: Some(5),
            max_length: Some(10),
            regex: None,
        })),
    };
    let sup = SchemaType::List {
        element: Box::new(SchemaType::Text(TextRestrictions::default())),
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
                metadata: Default::default(),
                body: SchemaType::Ref(TypeId::new("B")),
            },
            SchemaTypeDef {
                id: TypeId::new("B"),
                name: None,
                metadata: Default::default(),
                body: SchemaType::Ref(TypeId::new("A")),
            },
        ],
        root: SchemaType::Ref(TypeId::new("A")),
    };
    // Should terminate (and accept under coinductive assumption).
    assert!(is_assignable(
        &graph,
        &SchemaType::Ref(TypeId::new("A")),
        &SchemaType::Ref(TypeId::new("A"))
    ));
}

#[test]
fn primitive_kind_mismatch_rejected() {
    let graph = SchemaGraph::anonymous(SchemaType::Bool);
    assert!(!is_assignable(&graph, &SchemaType::S32, &SchemaType::S64));
}

#[test]
fn quantity_suffix_subset_is_enforced() {
    let graph = SchemaGraph::anonymous(SchemaType::Bool);
    let sup = SchemaType::Quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec!["kg".to_string(), "g".to_string()],
        min: None,
        max: None,
    });
    let sub_ok = SchemaType::Quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec!["kg".to_string()],
        min: None,
        max: None,
    });
    let sub_not = SchemaType::Quantity(QuantitySpec {
        base_unit: "kg".to_string(),
        allowed_suffixes: vec!["kg".to_string(), "lb".to_string()],
        min: None,
        max: None,
    });
    assert!(is_assignable(&graph, &sub_ok, &sup));
    assert!(!is_assignable(&graph, &sub_not, &sup));
}
