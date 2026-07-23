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

//! Round-trip tests for the `IntoSchema` / `FromSchema` *meta-encoding* of
//! [`SchemaValue`] itself.
//!
//! Unlike the WIT / protobuf round-trips (which serialize a `SchemaValue` as a
//! value), these exercise the new derives that treat `SchemaValue` as an
//! ordinary Rust type: `to_value()` encodes the enum into a generic
//! `SchemaValue` tree shaped by `SchemaValue`'s *own* structural schema, and
//! `from_value()` decodes it back. This is the mechanism oplog carriers use to
//! persist a bare `SchemaValue` field (see the seam-cutover charter §0.5 N7).

use crate::schema::conversion::{IntoTypedSchemaValue, try_into_schema_graph};
use crate::schema::proptest_strategies as strategies;
use crate::schema::schema_value::SchemaValue;
use crate::schema::validation::validate_graph;
use crate::schema::{FromSchema, IntoSchema};
use proptest::prelude::*;
use strategies::{schema_value_strategy, schema_values_eq};
use test_r::test;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Meta-encoding any `SchemaValue` via the derived `IntoSchema::to_value`
    /// and decoding it back via `FromSchema::from_value` yields the original
    /// value (NaN-tolerant). This is the bare-`SchemaValue`-field persistence
    /// path used by the oplog carriers.
    #[test]
    fn schema_value_meta_round_trip(value in schema_value_strategy()) {
        let encoded: SchemaValue = value.to_value();
        let decoded: SchemaValue = SchemaValue::from_value(&encoded).expect("decode");
        prop_assert!(
            schema_values_eq(&value, &decoded),
            "meta round-trip mismatch:\n original = {value:?}\n decoded  = {decoded:?}"
        );
    }

    /// `into_typed_schema_value` pairs the value with `SchemaValue`'s own
    /// structural graph; that graph must be well-formed and the carried value
    /// must decode back to the original.
    #[test]
    fn schema_value_into_typed_round_trip(value in schema_value_strategy()) {
        let typed = value.into_typed_schema_value().expect("typed encode");
        prop_assert!(validate_graph(typed.graph()).is_ok(), "graph not well-formed");
        let decoded: SchemaValue = SchemaValue::from_value(typed.value()).expect("decode");
        prop_assert!(schema_values_eq(&value, &decoded));
    }
}

/// `SchemaValue`'s structural (meta) schema is a single well-formed graph,
/// independent of any particular value instance.
#[test]
fn schema_value_meta_graph_is_well_formed() {
    let graph = try_into_schema_graph::<SchemaValue>().expect("graph");
    assert!(validate_graph(&graph).is_ok());
}

/// The schema-carrier types themselves (`SchemaType`, `SchemaGraph`,
/// `TypedSchemaValue`, `MetadataEnvelope`) now derive `IntoSchema`/`FromSchema`
/// so they can appear as fields of schema-deriving oplog types (N7). Their
/// meta-schemas must be well-formed graphs.
#[test]
fn schema_carrier_meta_graphs_are_well_formed() {
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
    use crate::schema::metadata::{MetadataEnvelope, TypeId};
    use crate::schema::schema_type::SchemaType;

    assert!(validate_graph(&try_into_schema_graph::<SchemaType>().expect("schema-type")).is_ok());
    assert!(validate_graph(&try_into_schema_graph::<SchemaGraph>().expect("schema-graph")).is_ok());
    assert!(
        validate_graph(&try_into_schema_graph::<SchemaTypeDef>().expect("schema-type-def")).is_ok()
    );
    assert!(
        validate_graph(&try_into_schema_graph::<TypedSchemaValue>().expect("typed-schema-value"))
            .is_ok()
    );
    assert!(
        validate_graph(&try_into_schema_graph::<MetadataEnvelope>().expect("metadata-envelope"))
            .is_ok()
    );
    assert!(validate_graph(&try_into_schema_graph::<TypeId>().expect("type-id")).is_ok());
}

/// A `TypedSchemaValue` carrier round-trips through its own derived
/// `IntoSchema`/`FromSchema` meta-encoding. This is the public-oplog carrier
/// path: a typed value (graph + value) persisted/rendered as a field of a
/// schema-deriving public oplog entry.
#[test]
fn typed_schema_value_meta_round_trip() {
    use crate::schema::graph::TypedSchemaValue;

    let original = SchemaValue::U32(42)
        .into_typed_schema_value()
        .expect("typed");
    let encoded = original.to_value();
    let decoded = TypedSchemaValue::from_value(&encoded).expect("decode");
    assert_eq!(original, decoded);
}

/// Round-trip the foreign value types (A2 schema-native shapes) used by the
/// oplog payloads through `IntoSchema::to_value` / `FromSchema::from_value`.
#[test]
fn foreign_value_types_round_trip() {
    fn round_trip<T>(value: T)
    where
        T: IntoSchema + FromSchema + PartialEq + std::fmt::Debug,
    {
        let encoded = value.to_value();
        let decoded = T::from_value(&encoded).expect("decode");
        assert_eq!(value, decoded);
    }

    round_trip("123.456789".parse::<bigdecimal::BigDecimal>().unwrap());
    round_trip("-0.0001".parse::<bigdecimal::BigDecimal>().unwrap());

    round_trip(bit_vec::BitVec::from_bytes(&[0b1010_0110, 0b1111_0000]));
    round_trip(bit_vec::BitVec::from_elem(13, true));
    round_trip(bit_vec::BitVec::new());

    round_trip(chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap());
    round_trip(
        chrono::NaiveDate::from_ymd_opt(1999, 12, 31)
            .unwrap()
            .and_hms_nano_opt(23, 59, 59, 123_456_789)
            .unwrap(),
    );
    round_trip(chrono::NaiveTime::from_hms_nano_opt(13, 45, 30, 987_654_321).unwrap());
}

/// Round-trip the foreign / wrapper "leaf" types (A2 schema-native shapes) used
/// by the oplog payloads: `usize`, `Bound<T>`, the proto `UpdateMode` enum, the
/// hand-written `Serializable*` wrappers, and the derived `ValuesRange<T>`.
#[test]
fn foreign_leaf_types_round_trip() {
    use crate::model::oplog::payload::types::{
        SerializableFsErrorCode, SerializableIpAddress, SerializableMacAddress,
        SerializableSocketErrorCode, ValuesRange,
    };
    use golem_api_grpc::proto::golem::worker::UpdateMode;
    use std::ops::Bound;

    // `value -> to_value() -> from_value() -> value` for constructible types.
    fn round_trip<T>(value: T)
    where
        T: IntoSchema + FromSchema + PartialEq + std::fmt::Debug,
    {
        let encoded = value.to_value();
        let decoded = T::from_value(&encoded).expect("decode");
        assert_eq!(value, decoded);
    }

    // `SchemaValue -> from_value() -> to_value() -> SchemaValue` for types with
    // private fields (the enum wrappers cannot be constructed from this module).
    fn value_round_trip<T>(start: SchemaValue)
    where
        T: IntoSchema + FromSchema,
    {
        let decoded = T::from_value(&start).expect("decode");
        assert_eq!(decoded.to_value(), start);
    }

    // usize
    round_trip(0usize);
    round_trip(42usize);
    round_trip(usize::MAX);

    // Bound<i64>: all three cases.
    round_trip(Bound::Included(7i64));
    round_trip(Bound::Excluded(-3i64));
    round_trip::<Bound<i64>>(Bound::Unbounded);

    // UpdateMode (proto enum): case 0 and the non-zero case 1.
    round_trip(UpdateMode::Automatic);
    round_trip(UpdateMode::Manual);

    // SerializableFsErrorCode (37 cases): zero, mid, and last index.
    for case in [0u32, 13, 36] {
        value_round_trip::<SerializableFsErrorCode>(SchemaValue::Enum { case });
    }

    // SerializableSocketErrorCode (21 cases): zero, mid, and last index.
    for case in [0u32, 8, 20] {
        value_round_trip::<SerializableSocketErrorCode>(SchemaValue::Enum { case });
    }

    // SerializableIpAddress: IPv4 and IPv6.
    round_trip(SerializableIpAddress::IPv4 {
        address: [127, 0, 0, 1],
    });
    round_trip(SerializableIpAddress::IPv6 {
        address: [0, 0, 0, 0, 0, 0, 0, 1],
    });

    // SerializableMacAddress: decode a known string, then round-trip the value.
    let mac =
        SerializableMacAddress::from_value(&SchemaValue::String("01:23:45:67:89:AB".to_string()))
            .expect("decode mac");
    round_trip(mac);

    // ValuesRange<i64> (derived): Included/Excluded/Unbounded combinations.
    round_trip(ValuesRange::new(
        Bound::Included(1i64),
        Bound::Excluded(10i64),
    ));
    round_trip(ValuesRange::new(Bound::Unbounded, Bound::Included(5i64)));
    round_trip(ValuesRange::<i64>::new(Bound::Unbounded, Bound::Unbounded));
}

/// Round-trip the Wave-3 slice-2 pure-data model / base_model target types
/// through `IntoSchema::to_value` / `FromSchema::from_value`, and assert each
/// builds a well-formed schema graph. These are the additive targets enumerated
/// in the seam-cutover charter §9(b).
#[test]
fn model_target_types_round_trip() {
    use crate::base_model::agent::AgentTypeName;
    use crate::base_model::component::{ComponentId, ComponentRevision};
    use crate::base_model::environment::EnvironmentId;
    use crate::base_model::worker::{RevertToOplogIndex, RevertWorkerTarget};
    use crate::base_model::{
        AgentFingerprint, AgentId, AgentStatus, IdempotencyKey, OplogIndex, PromiseId,
        TransactionId,
    };
    use crate::model::quota::{Reservation, ReserveResult};
    use crate::model::retry_policy::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy};
    use crate::model::{ForkResult, RdbmsPoolKey};
    use std::time::Duration;

    // `value -> to_value() -> from_value() -> value` plus a well-formed-graph
    // assertion for the value's type.
    fn round_trip<T>(value: T)
    where
        T: IntoSchema + FromSchema + PartialEq + std::fmt::Debug,
    {
        let encoded = value.to_value();
        let decoded = T::from_value(&encoded).expect("decode");
        assert_eq!(value, decoded);
        assert!(
            try_into_schema_graph::<T>().is_ok(),
            "graph for {} is not well-formed",
            std::any::type_name::<T>()
        );
    }

    let component_id = ComponentId::from(uuid::Uuid::from_u64_pair(1, 2));
    let agent_id = AgentId {
        component_id,
        agent_id: "weather-agent".to_string(),
    };

    // base_model leaves / records
    round_trip(agent_id.clone());
    round_trip(AgentFingerprint::from(uuid::Uuid::from_u64_pair(3, 4)));
    round_trip(IdempotencyKey::new("idem-1".to_string()));
    round_trip(OplogIndex::from_u64(42));
    round_trip(PromiseId {
        agent_id: agent_id.clone(),
        oplog_idx: OplogIndex::from_u64(7),
    });
    round_trip(TransactionId::new("tx-1"));
    round_trip(AgentStatus::Running);
    round_trip(AgentStatus::Failed);
    round_trip(AgentTypeName("weather-agent".to_string()));
    round_trip(ComponentId::from(uuid::Uuid::from_u64_pair(5, 6)));
    round_trip(ComponentRevision::INITIAL);
    round_trip(ComponentRevision::new(9).unwrap());
    round_trip(EnvironmentId::from(uuid::Uuid::from_u64_pair(7, 8)));

    // worker revert target (union of two record structs)
    round_trip(RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
        last_oplog_index: OplogIndex::from_u64(5),
    }));

    // retry policy: recursive enum + predicate + predicate value
    round_trip(PredicateValue::Text("hello".to_string()));
    round_trip(PredicateValue::Integer(7));
    round_trip(PredicateValue::Boolean(true));
    let policy = RetryPolicy::CountBox {
        max_retries: 3,
        inner: Box::new(RetryPolicy::FilteredOn {
            predicate: Predicate::PropEq {
                property: "kind".to_string(),
                value: PredicateValue::Integer(5),
            },
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(250))),
        }),
    };
    round_trip(policy.clone());
    round_trip(NamedRetryPolicy {
        name: "fast".to_string(),
        priority: 10,
        predicate: Predicate::PropExists("kind".to_string()),
        policy,
    });

    // model leaves
    round_trip(ForkResult::Original);
    round_trip(ForkResult::Forked);
    round_trip(RdbmsPoolKey::from("postgres://user@localhost:5432/db").unwrap());

    // quota: reserve result + reservation
    round_trip(ReserveResult::Ok(Reservation::Unlimited));
    round_trip(ReserveResult::InsufficientAllocation {
        enforcement_action: crate::base_model::quota::EnforcementAction::Throttle,
        estimated_wait_nanos: Some(1_000),
    });

    // Purely-transitive helper graphs also build cleanly.
    assert!(try_into_schema_graph::<Predicate>().is_ok());
    assert!(try_into_schema_graph::<Reservation>().is_ok());
    assert!(try_into_schema_graph::<RevertToOplogIndex>().is_ok());
}

/// The foreign / wrapper leaf types that build a graph (or are used as fields of
/// schema-deriving oplog types) must produce well-formed schema graphs.
#[test]
fn foreign_leaf_type_graphs_are_well_formed() {
    use crate::model::oplog::payload::types::{
        SerializableFsErrorCode, SerializableIpAddress, SerializableMacAddress,
        SerializableSocketErrorCode, ValuesRange,
    };
    use golem_api_grpc::proto::golem::worker::UpdateMode;
    use std::ops::Bound;

    assert!(try_into_schema_graph::<usize>().is_ok());
    assert!(try_into_schema_graph::<Bound<i64>>().is_ok());
    assert!(try_into_schema_graph::<UpdateMode>().is_ok());
    assert!(try_into_schema_graph::<SerializableFsErrorCode>().is_ok());
    assert!(try_into_schema_graph::<SerializableSocketErrorCode>().is_ok());
    assert!(try_into_schema_graph::<SerializableIpAddress>().is_ok());
    assert!(try_into_schema_graph::<SerializableMacAddress>().is_ok());
    assert!(try_into_schema_graph::<ValuesRange<i64>>().is_ok());
}
