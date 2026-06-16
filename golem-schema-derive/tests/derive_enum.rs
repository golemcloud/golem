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
#![allow(dead_code)]

use golem_common::schema::SchemaBuilder;
use golem_common::schema::{FromSchema, IntoSchema};
use golem_common::schema::{NamedFieldType, SchemaType, SchemaValue, VariantCaseType};
use test_r::test;

test_r::enable!();

#[derive(IntoSchema)]
enum Status {
    Active,
    Pending(u32),
    Failed { reason: String },
}

#[test]
fn enum_into_schema_emits_variant() {
    let mut builder = SchemaBuilder::new();
    let root = Status::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert_eq!(graph.defs.len(), 1);
    match &graph.defs[0].body {
        SchemaType::Variant { cases, .. } => {
            let expected = vec![
                VariantCaseType {
                    name: "active".to_string(),
                    payload: None,
                    metadata: Default::default(),
                },
                VariantCaseType {
                    name: "pending".to_string(),
                    payload: Some(SchemaType::u32()),
                    metadata: Default::default(),
                },
                VariantCaseType {
                    name: "failed".to_string(),
                    payload: Some(SchemaType::record(vec![NamedFieldType {
                        name: "reason".to_string(),
                        body: SchemaType::string(),
                        metadata: Default::default(),
                    }])),
                    metadata: Default::default(),
                },
            ];
            assert_eq!(cases.as_slice(), expected.as_slice());
        }
        other => panic!("expected variant body, got {other:?}"),
    }
}

#[derive(Debug, PartialEq, IntoSchema, FromSchema)]
enum Color {
    Red,
    Green,
    Blue,
}

#[test]
fn all_unit_enum_into_schema_emits_enum() {
    let mut builder = SchemaBuilder::new();
    let root = Color::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert_eq!(graph.defs.len(), 1);
    match &graph.defs[0].body {
        SchemaType::Enum { cases, .. } => {
            assert_eq!(
                cases.as_slice(),
                &["red".to_string(), "green".to_string(), "blue".to_string(),]
            );
        }
        other => panic!("expected enum body, got {other:?}"),
    }
}

#[test]
fn all_unit_enum_to_value_emits_enum() {
    assert_eq!(Color::Red.to_value(), SchemaValue::Enum { case: 0 });
    assert_eq!(Color::Green.to_value(), SchemaValue::Enum { case: 1 });
    assert_eq!(Color::Blue.to_value(), SchemaValue::Enum { case: 2 });
}

#[test]
fn all_unit_enum_round_trip() {
    for color in [Color::Red, Color::Green, Color::Blue] {
        let value = color.to_value();
        let decoded = Color::from_value(&value).expect("decode succeeds");
        assert_eq!(decoded, color);
    }
}

#[test]
fn all_unit_enum_from_value_rejects_variant() {
    let result = Color::from_value(&SchemaValue::Variant(
        golem_common::schema::schema_value::VariantValuePayload {
            case: 0,
            payload: None,
        },
    ));
    assert!(result.is_err());
}

#[derive(IntoSchema)]
enum MultiTuple {
    A(u32, String),
    B,
}

#[test]
fn enum_multi_tuple_variant_payload_is_tuple() {
    let mut builder = SchemaBuilder::new();
    let root = MultiTuple::register_in(&mut builder);
    let graph = builder.into_graph(root);

    match &graph.defs[0].body {
        SchemaType::Variant { cases, .. } => {
            let payload = cases[0].payload.as_ref().expect("first case has a payload");
            match payload {
                SchemaType::Tuple { elements, .. } => {
                    assert_eq!(
                        elements.as_slice(),
                        &[SchemaType::u32(), SchemaType::string()]
                    );
                }
                other => panic!("expected tuple payload, got {other:?}"),
            }
            assert!(cases[1].payload.is_none());
        }
        other => panic!("expected variant body, got {other:?}"),
    }
}
