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

use golem_common::schema::{
    FromSchema, IntoSchema, SchemaBuilder, SchemaType, SchemaValue, TypeId, try_into_schema_graph,
};
use test_r::test;

test_r::enable!();

#[derive(IntoSchema, FromSchema, Debug, PartialEq)]
struct Node {
    next: Option<Box<Node>>,
}

#[test]
fn recursive_struct_emits_single_def_and_inner_ref() {
    let mut builder = SchemaBuilder::new();
    let root = Node::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert_eq!(graph.defs.len(), 1);
    let id = Node::type_id();
    assert_eq!(graph.defs[0].id, id);

    let body = &graph.defs[0].body;
    let next_field = match body {
        SchemaType::Record { fields } => &fields[0],
        other => panic!("expected record body, got {other:?}"),
    };
    assert_eq!(next_field.name, "next");
    let inner = match &next_field.body {
        SchemaType::Option { inner } => inner,
        other => panic!("expected option, got {other:?}"),
    };
    assert_eq!(inner.as_ref(), &SchemaType::Ref(id.clone()));
}

#[derive(IntoSchema, FromSchema, Debug, PartialEq)]
struct A {
    b: Option<Box<B>>,
}

#[derive(IntoSchema, FromSchema, Debug, PartialEq)]
struct B {
    a: Option<Box<A>>,
}

#[test]
fn mutually_recursive_structs_register_both_definitions() {
    let mut builder = SchemaBuilder::new();
    let root = A::register_in(&mut builder);
    let graph = builder.into_graph(root);

    let ids: Vec<TypeId> = graph.defs.iter().map(|d| d.id.clone()).collect();
    assert!(ids.contains(&A::type_id()));
    assert!(ids.contains(&B::type_id()));
    assert_eq!(graph.defs.len(), 2);

    // Verify A's body has an inner reference to B
    let a_def = graph.defs.iter().find(|d| d.id == A::type_id()).unwrap();
    match &a_def.body {
        SchemaType::Record { fields } => {
            let b_field = fields.iter().find(|f| f.name == "b").unwrap();
            match &b_field.body {
                SchemaType::Option { inner } => {
                    assert_eq!(inner.as_ref(), &SchemaType::Ref(B::type_id()));
                }
                other => panic!("expected option, got {other:?}"),
            }
        }
        other => panic!("expected record, got {other:?}"),
    }
}

// Recursive type with `Vec<Self>` — Vec is anonymous so no separate graph slot
// is created for it.
#[derive(IntoSchema, FromSchema, Debug, PartialEq)]
struct N {
    children: Vec<N>,
}

#[test]
fn vec_self_is_anonymous() {
    let graph = try_into_schema_graph::<N>().expect("graph should be well-formed");
    assert_eq!(graph.defs.len(), 1);
    match &graph.defs[0].body {
        SchemaType::Record { fields } => {
            let children = &fields[0];
            match &children.body {
                SchemaType::List { element } => {
                    assert_eq!(**element, SchemaType::Ref(N::type_id()));
                }
                other => panic!("expected list, got {other:?}"),
            }
        }
        other => panic!("expected record body, got {other:?}"),
    }
}

#[test]
fn mutually_recursive_type_ids_are_dotted() {
    let id_a = A::type_id();
    let id_b = B::type_id();
    assert!(
        !id_a.as_str().contains("::"),
        "A type-id should be dotted, got `{id_a}`"
    );
    assert!(
        !id_b.as_str().contains("::"),
        "B type-id should be dotted, got `{id_b}`"
    );
    assert!(id_a.as_str().ends_with(".A"));
    assert!(id_b.as_str().ends_with(".B"));
}

#[test]
fn n_round_trip_via_to_from_value() {
    let n = N {
        children: vec![
            N { children: vec![] },
            N {
                children: vec![N { children: vec![] }],
            },
        ],
    };
    let v: SchemaValue = n.to_value();
    let n2 = N::from_value(&v).expect("decode succeeds");
    assert_eq!(n, n2);
}
