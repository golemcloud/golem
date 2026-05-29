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

use golem_common::schema::IntoSchema;
use golem_common::schema::SchemaBuilder;
use golem_common::schema::{NamedFieldType, SchemaType, VariantCaseType};
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
        SchemaType::Variant { cases } => {
            let expected = vec![
                VariantCaseType {
                    name: "active".to_string(),
                    payload: None,
                    metadata: Default::default(),
                },
                VariantCaseType {
                    name: "pending".to_string(),
                    payload: Some(SchemaType::U32),
                    metadata: Default::default(),
                },
                VariantCaseType {
                    name: "failed".to_string(),
                    payload: Some(SchemaType::Record {
                        fields: vec![NamedFieldType {
                            name: "reason".to_string(),
                            body: SchemaType::String,
                            metadata: Default::default(),
                        }],
                    }),
                    metadata: Default::default(),
                },
            ];
            assert_eq!(cases.as_slice(), expected.as_slice());
        }
        other => panic!("expected variant body, got {other:?}"),
    }
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
        SchemaType::Variant { cases } => {
            let payload = cases[0].payload.as_ref().expect("first case has a payload");
            match payload {
                SchemaType::Tuple { elements } => {
                    assert_eq!(elements.as_slice(), &[SchemaType::U32, SchemaType::String]);
                }
                other => panic!("expected tuple payload, got {other:?}"),
            }
            assert!(cases[1].payload.is_none());
        }
        other => panic!("expected variant body, got {other:?}"),
    }
}
