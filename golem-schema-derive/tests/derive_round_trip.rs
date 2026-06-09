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

use golem_common::schema::render::{from_json_value, to_json_value};
use golem_common::schema::{FromSchema, IntoSchema, try_into_schema_graph};
use proptest::prelude::*;
use test_r::test;

test_r::enable!();

// ----------------------------------------------------------------------
// Synthetic non-property test (kept for easy debugging).
// ----------------------------------------------------------------------

#[derive(Debug, PartialEq, IntoSchema, FromSchema)]
struct Point {
    x: u32,
    y: u32,
}

#[test]
fn point_to_from_value_round_trip() {
    let original = Point { x: 10, y: 20 };
    let v = original.to_value();
    let decoded = Point::from_value(&v).expect("decode succeeds");
    assert_eq!(decoded, original);

    // also via JSON renderer
    let graph = try_into_schema_graph::<Point>().expect("graph should be well-formed");
    let json = to_json_value(&graph, &graph.root, &v).expect("to json");
    let value2 = from_json_value(&graph, &graph.root, &json).expect("from json");
    let decoded2 = Point::from_value(&value2).expect("decode via JSON");
    assert_eq!(decoded2, original);
}

// ----------------------------------------------------------------------
// Property-based round trips: struct
// ----------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone, IntoSchema, FromSchema)]
struct PointS {
    x: i32,
    y: i32,
    label: String,
}

fn point_strategy() -> impl Strategy<Value = PointS> {
    (any::<i32>(), any::<i32>(), "[a-z]{0,8}").prop_map(|(x, y, s)| PointS { x, y, label: s })
}

proptest! {
    #[test]
    fn struct_round_trip_json(p in point_strategy()) {
        let graph = try_into_schema_graph::<PointS>().expect("graph should be well-formed");
        let v = p.to_value();
        let json = to_json_value(&graph, &graph.root, &v).expect("to json");
        let v2 = from_json_value(&graph, &graph.root, &json).expect("from json");
        let p2 = PointS::from_value(&v2).expect("decode");
        prop_assert_eq!(p, p2);
    }
}

// ----------------------------------------------------------------------
// Property-based round trips: enum (carried-tag variant)
// ----------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone, IntoSchema, FromSchema)]
enum Status {
    Active,
    Pending(u32),
    Failed { reason: String },
}

fn status_strategy() -> impl Strategy<Value = Status> {
    prop_oneof![
        Just(Status::Active),
        any::<u32>().prop_map(Status::Pending),
        "[a-z]{0,12}".prop_map(|reason| Status::Failed { reason }),
    ]
}

proptest! {
    #[test]
    fn enum_round_trip_json(s in status_strategy()) {
        let graph = try_into_schema_graph::<Status>().expect("graph should be well-formed");
        let v = s.to_value();
        let json = to_json_value(&graph, &graph.root, &v).expect("to json");
        let v2 = from_json_value(&graph, &graph.root, &json).expect("from json");
        let s2 = Status::from_value(&v2).expect("decode");
        prop_assert_eq!(s, s2);
    }
}

// ----------------------------------------------------------------------
// Property-based round trips: union (inferred-tag)
// ----------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone, IntoSchema, FromSchema)]
#[schema(union)]
enum Resource {
    #[schema(prefix = "ssh://")]
    Ssh(String),
    #[schema(prefix = "https://")]
    Web(String),
}

fn resource_strategy() -> impl Strategy<Value = Resource> {
    prop_oneof![
        "[a-z]{1,8}".prop_map(|s| Resource::Ssh(format!("ssh://{s}"))),
        "[a-z]{1,8}".prop_map(|s| Resource::Web(format!("https://{s}"))),
    ]
}

proptest! {
    #[test]
    fn union_round_trip_json(r in resource_strategy()) {
        let graph = try_into_schema_graph::<Resource>().expect("graph should be well-formed");
        let v = r.to_value();
        let json = to_json_value(&graph, &graph.root, &v).expect("to json");
        let v2 = from_json_value(&graph, &graph.root, &json).expect("from json");
        let r2 = Resource::from_value(&v2).expect("decode");
        prop_assert_eq!(r, r2);
    }
}

// ----------------------------------------------------------------------
// Property-based round trips: recursive
// ----------------------------------------------------------------------

#[derive(Debug, PartialEq, IntoSchema, FromSchema, Clone)]
struct LinkedNode {
    label: String,
    next: Option<Box<LinkedNode>>,
}

fn linked_node_strategy() -> impl Strategy<Value = LinkedNode> {
    let leaf = "[a-z]{1,4}".prop_map(|s| LinkedNode {
        label: s,
        next: None,
    });
    leaf.prop_recursive(4, 8, 2, |inner| {
        ("[a-z]{1,4}", inner).prop_map(|(s, inner)| LinkedNode {
            label: s,
            next: Some(Box::new(inner)),
        })
    })
}

proptest! {
    #[test]
    fn recursive_round_trip_json(n in linked_node_strategy()) {
        let graph = try_into_schema_graph::<LinkedNode>().expect("graph should be well-formed");
        let v = n.to_value();
        let json = to_json_value(&graph, &graph.root, &v).expect("to json");
        let v2 = from_json_value(&graph, &graph.root, &json).expect("from json");
        let n2 = LinkedNode::from_value(&v2).expect("decode");
        prop_assert_eq!(n, n2);
    }
}

// ----------------------------------------------------------------------
// Property-based round trips: generic + newtype
// ----------------------------------------------------------------------

#[derive(Debug, PartialEq, IntoSchema, FromSchema, Clone)]
struct Container<Inner> {
    items: Vec<Inner>,
}

#[derive(Debug, PartialEq, IntoSchema, FromSchema, Clone)]
#[schema(transparent)]
struct UserId(String);

fn container_strategy() -> impl Strategy<Value = Container<UserId>> {
    proptest::collection::vec("[a-z]{1,4}", 0..6).prop_map(|ids| Container {
        items: ids.into_iter().map(UserId).collect(),
    })
}

proptest! {
    #[test]
    fn generic_newtype_round_trip_json(c in container_strategy()) {
        let graph = try_into_schema_graph::<Container<UserId>>()
            .expect("graph should be well-formed");
        let v = c.to_value();
        let json = to_json_value(&graph, &graph.root, &v).expect("to json");
        let v2 = from_json_value(&graph, &graph.root, &json).expect("from json");
        let c2 = Container::<UserId>::from_value(&v2).expect("decode");
        prop_assert_eq!(c, c2);
    }
}
