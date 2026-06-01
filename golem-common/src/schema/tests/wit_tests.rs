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

use super::strategies;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::wit::{
    DecodeError, EncodeError, decode_graph, decode_typed, decode_value, encode_graph, encode_typed,
    encode_value, wire,
};
use proptest::prelude::*;
use strategies::{schema_graph_strategy, schema_value_strategy, typed_schema_value_strategy};
use test_r::test;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Encoding and then decoding any well-formed schema graph yields the
    /// original graph (recursive equality).
    #[test]
    fn graph_round_trip(graph in schema_graph_strategy()) {
        let wire = encode_graph(&graph).expect("encode");
        let back = decode_graph(&wire).expect("decode");
        prop_assert_eq!(graph, back);
    }

    /// Encoding and then decoding any schema value yields a value
    /// bitwise-equal to the original (with `NaN`-tolerant comparison for
    /// floats, see [`crate::schema::tests::strategies::schema_values_eq`]).
    #[test]
    fn value_round_trip(value in schema_value_strategy()) {
        let wire = encode_value(&value);
        let back = decode_value(&wire).expect("decode");
        prop_assert!(
            strategies::schema_values_eq(&value, &back),
            "value round-trip mismatch:\n  before: {value:?}\n  after:  {back:?}"
        );
    }

    /// Encoding and then decoding any [`TypedSchemaValue`] preserves both
    /// the graph and the value tree.
    #[test]
    fn typed_round_trip(typed in typed_schema_value_strategy()) {
        let wire = encode_typed(&typed).expect("encode");
        let back = decode_typed(&wire).expect("decode");
        prop_assert_eq!(typed.graph(), back.graph());
        prop_assert!(
            strategies::schema_values_eq(typed.value(), back.value()),
            "typed value round-trip mismatch:\n  before: {:?}\n  after:  {:?}",
            typed.value(),
            back.value()
        );
    }
}

// ---------- negative-case fixtures (not properties) ----------

#[test]
fn duplicate_def_id_is_rejected_on_encode() {
    let graph = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: TypeId::new("dup"),
                name: None,
                body: SchemaType::s32(),
            },
            SchemaTypeDef {
                id: TypeId::new("dup"),
                name: None,
                body: SchemaType::s64(),
            },
        ],
        root: SchemaType::ref_to(TypeId::new("dup")),
    };
    let err = encode_graph(&graph).expect_err("should fail");
    assert!(matches!(err, EncodeError::DuplicateTypeId(_)));
}

#[test]
fn unknown_ref_is_rejected_on_encode() {
    let graph = SchemaGraph::anonymous(SchemaType::ref_to(TypeId::new("missing")));
    let err = encode_graph(&graph).expect_err("should fail");
    assert!(matches!(err, EncodeError::UnknownTypeId(_)));
}

#[test]
fn duplicate_def_id_is_rejected_on_decode() {
    let graph = wire::SchemaGraph {
        type_nodes: vec![wire::SchemaTypeNode {
            body: wire::SchemaTypeBody::S32Type,
            metadata: empty_metadata(),
        }],
        defs: vec![
            wire::SchemaTypeDef {
                id: "dup".to_string(),
                name: None,
                body: 0,
            },
            wire::SchemaTypeDef {
                id: "dup".to_string(),
                name: None,
                body: 0,
            },
        ],
        root: 0,
    };
    let err = decode_graph(&graph).expect_err("should fail");
    assert!(matches!(err, DecodeError::DuplicateTypeId(_)));
}

#[test]
fn type_node_out_of_range_is_rejected_on_decode() {
    let graph = wire::SchemaGraph {
        type_nodes: vec![],
        defs: vec![],
        root: 0,
    };
    let err = decode_graph(&graph).expect_err("should fail");
    assert!(matches!(err, DecodeError::TypeNodeIndexOutOfRange(0)));
}

#[test]
fn value_node_out_of_range_is_rejected_on_decode() {
    let tree = wire::SchemaValueTree {
        value_nodes: vec![],
        root: 0,
    };
    let err = decode_value(&tree).expect_err("should fail");
    assert!(matches!(err, DecodeError::ValueNodeIndexOutOfRange(0)));
}

#[test]
fn def_index_out_of_range_is_rejected_on_decode() {
    let graph = wire::SchemaGraph {
        type_nodes: vec![wire::SchemaTypeNode {
            body: wire::SchemaTypeBody::RefType(99),
            metadata: empty_metadata(),
        }],
        defs: vec![],
        root: 0,
    };
    let err = decode_graph(&graph).expect_err("should fail");
    assert!(matches!(err, DecodeError::DefIndexOutOfRange(99)));
}

#[test]
fn invalid_datetime_is_rejected_on_decode() {
    let tree = wire::SchemaValueTree {
        value_nodes: vec![wire::SchemaValueNode::DatetimeValue(wire::Datetime {
            seconds: i64::MAX,
            nanoseconds: 999_999_999,
        })],
        root: 0,
    };
    let err = decode_value(&tree).expect_err("should fail");
    assert!(matches!(err, DecodeError::InvalidDatetime { .. }));
}

#[test]
fn empty_typed_schema_value_round_trips() {
    // A minimal smoke check: an anonymous `Bool` paired with `Bool(false)`.
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::bool()),
        crate::schema::schema_value::SchemaValue::Bool(false),
    );
    let wire = encode_typed(&typed).expect("encode");
    let back = decode_typed(&wire).expect("decode");
    assert_eq!(typed.graph(), back.graph());
    assert_eq!(typed.value(), back.value());
}

fn empty_metadata() -> wire::MetadataEnvelope {
    wire::MetadataEnvelope {
        doc: None,
        aliases: Vec::new(),
        examples: Vec::new(),
        deprecated: None,
        role: None,
    }
}
