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

//! Round-trip tests for the `desert_rust::BinaryCodec` derives on the
//! schema-graph types reachable from [`TypedSchemaValue`].

use crate::schema::proptest_strategies::{
    schema_graph_strategy, schema_value_strategy, schema_values_eq, typed_schema_value_strategy,
};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::schema_value::SchemaValue;
use proptest::prelude::*;
use test_r::test;

fn roundtrip<T>(value: &T) -> T
where
    T: desert_rust::BinarySerializer + desert_rust::BinaryDeserializer,
{
    let bytes = desert_rust::serialize_to_byte_vec(value).expect("serialize");
    desert_rust::deserialize::<T>(&bytes).expect("deserialize")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn schema_graph_binary_codec_round_trip(graph in schema_graph_strategy()) {
        let back: SchemaGraph = roundtrip(&graph);
        prop_assert_eq!(graph, back);
    }

    #[test]
    fn schema_value_binary_codec_round_trip(value in schema_value_strategy()) {
        let back: SchemaValue = roundtrip(&value);
        prop_assert!(
            schema_values_eq(&value, &back),
            "value round-trip mismatch:\n  before: {value:?}\n  after:  {back:?}"
        );
    }

    #[test]
    fn typed_schema_value_binary_codec_round_trip(typed in typed_schema_value_strategy()) {
        let back: TypedSchemaValue = roundtrip(&typed);
        prop_assert_eq!(typed.graph(), back.graph());
        prop_assert!(
            schema_values_eq(typed.value(), back.value()),
            "typed value round-trip mismatch:\n  before: {typed:?}\n  after:  {back:?}"
        );
    }
}
