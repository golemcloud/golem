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
use golem_common::schema::{IntoSchema, NamedFieldType, SchemaType, SchemaValue};
use test_r::test;

test_r::enable!();

#[derive(IntoSchema)]
struct Foo {
    a: u32,
    b: String,
}

#[test]
fn struct_into_schema_emits_named_record() {
    let mut builder = SchemaBuilder::new();
    let root = Foo::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert_eq!(graph.defs.len(), 1);
    let def = &graph.defs[0];
    assert_eq!(def.id, Foo::type_id());
    assert!(def.name.is_some());

    match &def.body {
        SchemaType::Record { fields } => {
            assert_eq!(
                fields.as_slice(),
                &[
                    NamedFieldType {
                        name: "a".to_string(),
                        body: SchemaType::U32,
                        metadata: Default::default(),
                    },
                    NamedFieldType {
                        name: "b".to_string(),
                        body: SchemaType::String,
                        metadata: Default::default(),
                    },
                ]
            );
        }
        other => panic!("expected record body, got {other:?}"),
    }

    match &graph.root {
        SchemaType::Ref(id) => assert_eq!(id, &Foo::type_id()),
        other => panic!("expected ref root, got {other:?}"),
    }
}

#[derive(IntoSchema)]
struct Bag(u32, String);

#[test]
fn tuple_struct_into_schema_emits_tuple_body() {
    let mut builder = SchemaBuilder::new();
    let root = Bag::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert_eq!(graph.defs.len(), 1);
    match &graph.defs[0].body {
        SchemaType::Tuple { elements } => {
            assert_eq!(elements.as_slice(), &[SchemaType::U32, SchemaType::String]);
        }
        other => panic!("expected tuple body, got {other:?}"),
    }
}

#[derive(IntoSchema)]
struct Marker;

#[test]
fn unit_struct_into_schema_emits_empty_record() {
    let mut builder = SchemaBuilder::new();
    let root = Marker::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert!(matches!(graph.root, SchemaType::Ref(_)));
    assert_eq!(graph.defs.len(), 1);
    assert!(matches!(&graph.defs[0].body, SchemaType::Record { fields } if fields.is_empty()));
}

#[test]
fn struct_to_value_emits_positional_record() {
    let value = Foo {
        a: 7,
        b: "hi".to_string(),
    };
    let v = value.to_value();
    match v {
        SchemaValue::Record { fields } => {
            assert_eq!(
                fields,
                vec![SchemaValue::U32(7), SchemaValue::String("hi".to_string())]
            );
        }
        other => panic!("expected record, got {other:?}"),
    }
}
