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

use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
use crate::schema::metadata::TypeId;
use crate::schema::proptest_strategies as strategies;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::{QuotaTokenValuePayload, SchemaValue, SecretValuePayload};
use crate::schema::wit::{
    DecodeError, EncodeError, QuotaTokenHandleRep, QuotaTokenResolver, SecretHandleRep,
    SecretResolver, decode_graph, decode_typed, decode_typed_rejecting_quota_with, decode_value,
    decode_value_rejecting_quota_with, decode_value_with, encode_graph, encode_typed, encode_value,
    encode_value_with, wire,
};
use chrono::{TimeZone, Utc};
use golem_schema::model::EnvironmentId;
use proptest::prelude::*;
use strategies::{
    schema_graph_strategy, transportable_schema_value_strategy,
    transportable_typed_schema_value_strategy,
};
use test_r::test;
use wasmtime::component::{Resource, ResourceTable};

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
    /// floats, see [`crate::schema::proptest_strategies::schema_values_eq`]).
    #[test]
    fn value_round_trip(value in transportable_schema_value_strategy()) {
        let wire = encode_value(&value).expect("encode");
        let back = decode_value(&wire).expect("decode");
        prop_assert!(
            strategies::schema_values_eq(&value, &back),
            "value round-trip mismatch:\n  before: {value:?}\n  after:  {back:?}"
        );
    }

    /// Encoding and then decoding any [`TypedSchemaValue`] preserves both
    /// the graph and the value tree.
    #[test]
    fn typed_round_trip(typed in transportable_typed_schema_value_strategy()) {
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

// ---------- quota-token handle boundary ----------

fn sample_snapshot() -> QuotaTokenValuePayload {
    QuotaTokenValuePayload {
        environment_id: EnvironmentId::new(uuid::Uuid::from_u64_pair(1, 2)),
        resource_name: "gpu-tokens".to_string(),
        expected_use: 1000,
        last_credit: -5,
        last_credit_at: Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
    }
}

fn sample_secret_snapshot() -> SecretValuePayload {
    SecretValuePayload {
        secret_id: uuid::Uuid::from_u64_pair(3, 4),
        config_key: Some(vec!["db".to_string(), "password".to_string()]),
        version: 7,
        resolved_at: Utc.timestamp_opt(1_700_000_010, 0).single().unwrap(),
        category: Some("api-key".to_string()),
    }
}

/// A minimal [`QuotaTokenResolver`] backed by a real [`ResourceTable`], storing
/// the trusted snapshot as the boxed payload of each handle. Mirrors what the
/// executor does, without any of the live-lease machinery.
struct TableResolver {
    table: ResourceTable,
    /// Number of live handles currently held in the table (pushes minus deletes).
    live: i64,
    /// Number of `handle_from_snapshot` calls so far.
    created: usize,
    /// Number of `snapshot_handle` calls so far (handles lifted into a trusted
    /// snapshot, as opposed to merely dropped).
    snapshotted: usize,
    /// If set, `handle_from_snapshot` fails once this many handles have already
    /// been created, simulating a mid-encode resolver failure.
    fail_create_after: Option<usize>,
}

impl TableResolver {
    fn new() -> Self {
        Self {
            table: ResourceTable::new(),
            live: 0,
            created: 0,
            snapshotted: 0,
            fail_create_after: None,
        }
    }

    fn failing_after(n: usize) -> Self {
        Self {
            fail_create_after: Some(n),
            ..Self::new()
        }
    }

    fn secret_handle(&mut self) -> Resource<SecretHandleRep> {
        let handle = self
            .table
            .push(SecretHandleRep::new(sample_secret_snapshot()))
            .unwrap();
        self.live += 1;
        handle
    }
}

impl QuotaTokenResolver for TableResolver {
    type Error = anyhow::Error;

    fn snapshot_handle(
        &mut self,
        handle: Resource<QuotaTokenHandleRep>,
    ) -> Result<QuotaTokenValuePayload, Self::Error> {
        let rep = self.table.delete(handle)?;
        self.live -= 1;
        self.snapshotted += 1;
        rep.downcast_ref::<QuotaTokenValuePayload>()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("handle payload was not a snapshot"))
    }

    fn handle_from_snapshot(
        &mut self,
        snapshot: &QuotaTokenValuePayload,
    ) -> Result<Resource<QuotaTokenHandleRep>, Self::Error> {
        if let Some(limit) = self.fail_create_after
            && self.created >= limit
        {
            return Err(anyhow::anyhow!("resolver refused to create handle"));
        }
        let handle = self
            .table
            .push(QuotaTokenHandleRep::new(snapshot.clone()))?;
        self.created += 1;
        self.live += 1;
        Ok(handle)
    }

    fn drop_handle(&mut self, handle: Resource<QuotaTokenHandleRep>) {
        if self.table.delete(handle).is_ok() {
            self.live -= 1;
        }
    }
}

impl SecretResolver for TableResolver {
    type Error = anyhow::Error;

    fn snapshot_secret_handle(
        &mut self,
        handle: Resource<SecretHandleRep>,
    ) -> Result<SecretValuePayload, Self::Error> {
        let rep = self.table.delete(handle)?;
        self.live -= 1;
        rep.downcast_ref::<SecretValuePayload>()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("secret handle payload was not a snapshot"))
    }

    fn secret_handle_from_snapshot(
        &mut self,
        snapshot: &SecretValuePayload,
    ) -> Result<Resource<SecretHandleRep>, Self::Error> {
        let handle = self.table.push(SecretHandleRep::new(snapshot.clone()))?;
        self.live += 1;
        Ok(handle)
    }

    fn drop_secret_handle(&mut self, handle: Resource<SecretHandleRep>) {
        if self.table.delete(handle).is_ok() {
            self.live -= 1;
        }
    }
}

#[test]
fn quota_token_round_trips_through_resolver() {
    let value = SchemaValue::QuotaToken(sample_snapshot());
    let mut resolver = TableResolver::new();
    let wire = encode_value_with(&value, &mut resolver).expect("encode_with");
    let back = decode_value_with(wire, &mut resolver).expect("decode_with");
    assert_eq!(value, back);
    // The lowered handle was consumed by decoding; nothing leaks.
    assert_eq!(resolver.live, 0);
}

#[test]
fn secret_round_trips_through_resolver() {
    let value = SchemaValue::Secret(sample_secret_snapshot());
    let mut resolver = TableResolver::new();
    let wire = encode_value_with(&value, &mut resolver).expect("encode_with");
    let back = decode_value_with(wire, &mut resolver).expect("decode_with");
    assert_eq!(value, back);
    assert_eq!(resolver.live, 0);
}

#[test]
fn nested_quota_token_round_trips_through_resolver() {
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(7),
            SchemaValue::List {
                elements: vec![
                    SchemaValue::QuotaToken(sample_snapshot()),
                    SchemaValue::QuotaToken(QuotaTokenValuePayload {
                        resource_name: "other".to_string(),
                        ..sample_snapshot()
                    }),
                ],
            },
        ],
    };
    let mut resolver = TableResolver::new();
    let wire = encode_value_with(&value, &mut resolver).expect("encode_with");
    let back = decode_value_with(wire, &mut resolver).expect("decode_with");
    assert_eq!(value, back);
    assert_eq!(resolver.live, 0);
}

#[test]
fn pure_encode_rejects_quota_token() {
    let value = SchemaValue::QuotaToken(sample_snapshot());
    assert!(matches!(
        encode_value(&value),
        Err(EncodeError::QuotaTokenNotTransportable)
    ));
}

#[test]
fn pure_decode_rejects_quota_handle() {
    let mut resolver = TableResolver::new();
    let wire =
        encode_value_with(&SchemaValue::QuotaToken(sample_snapshot()), &mut resolver).unwrap();
    let err = decode_value(&wire).expect_err("pure decode must reject handles");
    assert!(matches!(err, DecodeError::QuotaTokenRequiresResolver));
}

#[test]
fn pure_decode_rejects_unreferenced_secret_handle() {
    let mut resolver = TableResolver::new();
    let secret = resolver.secret_handle();
    let tree = wire::SchemaValueTree {
        value_nodes: vec![
            wire::SchemaValueNode::SecretValue(secret),
            wire::SchemaValueNode::BoolValue(false),
        ],
        root: 1,
    };

    let err = decode_value(&tree).expect_err("pure decode must reject secret transport");
    assert!(matches!(err, DecodeError::SecretRequiresResolver));
}

#[test]
fn aliased_quota_handle_is_rejected() {
    let mut resolver = TableResolver::new();
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    // A malformed tree where a tuple references the same handle node twice.
    let tree = wire::SchemaValueTree {
        value_nodes: vec![
            wire::SchemaValueNode::QuotaTokenHandle(handle),
            wire::SchemaValueNode::TupleValue(vec![0, 0]),
        ],
        root: 1,
    };
    let err = decode_value_with(tree, &mut resolver).expect_err("aliasing must be rejected");
    assert!(matches!(err, DecodeError::AliasedValueNode(0)));
    // The single handle was consumed exactly once; nothing leaks.
    assert_eq!(resolver.live, 0);
}

#[test]
fn unreferenced_quota_handle_is_dropped_and_rejected() {
    let mut resolver = TableResolver::new();
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    // A tree whose root is a plain value, with an extra owned handle node that
    // is never reachable from the root.
    let tree = wire::SchemaValueTree {
        value_nodes: vec![
            wire::SchemaValueNode::QuotaTokenHandle(handle),
            wire::SchemaValueNode::U32Value(1),
        ],
        root: 1,
    };
    let err =
        decode_value_with(tree, &mut resolver).expect_err("unreferenced handle must be rejected");
    assert!(matches!(err, DecodeError::UnconsumedQuotaTokenHandle(0)));
    // The unreachable handle was dropped, not leaked.
    assert_eq!(resolver.live, 0);
}

#[test]
fn decode_does_not_snapshot_quota_handle_when_a_later_node_is_invalid() {
    // Atomicity regression: a tree `tuple([quota-token-handle, invalid-datetime])`
    // must be rejected for the invalid datetime *without* snapshotting (and thus
    // consuming) the quota token first. The handle is instead released cleanly
    // via `drop_handle`, so it neither leaks nor is silently turned into a
    // discarded snapshot.
    let mut resolver = TableResolver::new();
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    let tree = wire::SchemaValueTree {
        value_nodes: vec![
            wire::SchemaValueNode::QuotaTokenHandle(handle),
            wire::SchemaValueNode::DatetimeValue(wire::Datetime {
                seconds: i64::MAX,
                nanoseconds: 999_999_999,
            }),
            wire::SchemaValueNode::TupleValue(vec![0, 1]),
        ],
        root: 2,
    };
    let err = decode_value_with(tree, &mut resolver)
        .expect_err("an invalid sibling must reject the whole tree");
    assert!(matches!(err, DecodeError::InvalidDatetime { .. }));
    // The token was never lifted into a snapshot, only dropped, and nothing
    // leaks from the table.
    assert_eq!(resolver.snapshotted, 0);
    assert_eq!(resolver.live, 0);
}

#[test]
fn encode_cleans_up_handles_when_resolver_fails_midway() {
    // A record with two quota tokens; the resolver succeeds on the first and
    // fails on the second.
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::QuotaToken(sample_snapshot()),
            SchemaValue::QuotaToken(QuotaTokenValuePayload {
                resource_name: "other".to_string(),
                ..sample_snapshot()
            }),
        ],
    };
    let mut resolver = TableResolver::failing_after(1);
    let err = encode_value_with(&value, &mut resolver).expect_err("encode must fail");
    assert!(matches!(err, EncodeError::QuotaResolver(_)));
    // The first handle was already minted; it must be dropped, not leaked.
    assert_eq!(resolver.live, 0);
}

#[test]
fn reject_decoder_drops_root_handle_and_rejects() {
    let mut resolver = TableResolver::new();
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    let tree = wire::SchemaValueTree {
        value_nodes: vec![wire::SchemaValueNode::QuotaTokenHandle(handle)],
        root: 0,
    };
    let err = decode_value_rejecting_quota_with(tree, &mut resolver)
        .expect_err("quota handle must be rejected at a reject-only boundary");
    assert!(matches!(err, DecodeError::QuotaTokenNotPermitted(0)));
    // The handle was deleted from the table, not leaked.
    assert_eq!(resolver.live, 0);
}

#[test]
fn reject_decoder_drops_unreferenced_handle() {
    let mut resolver = TableResolver::new();
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    // Root is a plain value; an extra owned handle node is never referenced.
    let tree = wire::SchemaValueTree {
        value_nodes: vec![
            wire::SchemaValueNode::QuotaTokenHandle(handle),
            wire::SchemaValueNode::U32Value(1),
        ],
        root: 1,
    };
    let err = decode_value_rejecting_quota_with(tree, &mut resolver)
        .expect_err("an unreferenced handle must still be rejected");
    assert!(matches!(err, DecodeError::QuotaTokenNotPermitted(0)));
    assert_eq!(resolver.live, 0);
}

#[test]
fn reject_decoder_passes_through_handle_free_value() {
    let mut resolver = TableResolver::new();
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::U32(7), SchemaValue::String("ok".to_string())],
    };
    let wire = encode_value(&value).expect("encode");
    let back = decode_value_rejecting_quota_with(wire, &mut resolver).expect("decode");
    assert_eq!(value, back);
    assert_eq!(resolver.live, 0);
}

#[test]
fn typed_reject_decoder_drains_handle_and_rejects() {
    let mut resolver = TableResolver::new();
    // A valid graph (anonymous bool) paired with a value tree that smuggles in
    // an owned quota handle.
    let valid = encode_typed(&TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::bool()),
        SchemaValue::Bool(false),
    ))
    .expect("encode valid typed");
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    let typed = wire::TypedSchemaValue {
        graph: valid.graph,
        value: wire::SchemaValueTree {
            value_nodes: vec![wire::SchemaValueNode::QuotaTokenHandle(handle)],
            root: 0,
        },
    };
    let err = decode_typed_rejecting_quota_with(typed, &mut resolver)
        .expect_err("typed reject decoder must reject quota handles");
    assert!(matches!(err, DecodeError::QuotaTokenNotPermitted(0)));
    assert_eq!(resolver.live, 0);
}

#[test]
fn typed_reject_decoder_drains_handle_before_decoding_invalid_graph() {
    let mut resolver = TableResolver::new();
    let handle = resolver.handle_from_snapshot(&sample_snapshot()).unwrap();
    // An invalid graph (root references a missing type node) combined with a
    // quota handle in the value. The value must be drained first, so the handle
    // is deleted from the table even though the graph would otherwise error.
    let typed = wire::TypedSchemaValue {
        graph: wire::SchemaGraph {
            type_nodes: vec![],
            defs: vec![],
            root: 0,
        },
        value: wire::SchemaValueTree {
            value_nodes: vec![wire::SchemaValueNode::QuotaTokenHandle(handle)],
            root: 0,
        },
    };
    let err = decode_typed_rejecting_quota_with(typed, &mut resolver)
        .expect_err("must reject before reaching the invalid graph");
    assert!(matches!(err, DecodeError::QuotaTokenNotPermitted(0)));
    assert_eq!(resolver.live, 0);
}

#[test]
fn secrets_create_interface_is_imported_by_host_and_sdk_worlds() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("golem-common is a workspace member");
    let world_files = [
        "wit/host.wit",
        "sdks/rust/golem-rust/wit/golem-rust.wit",
        "sdks/ts/wit/main.wit",
        "sdks/scala/wit/main.wit",
        "sdks/moonbit/golem_sdk/wit/main.wit",
    ];
    let missing = world_files
        .iter()
        .filter(|path| {
            let contents = std::fs::read_to_string(workspace_root.join(path))
                .unwrap_or_else(|err| panic!("failed to read {path}: {err}"));
            !contents.contains("import golem:secrets/create@0.1.0;")
        })
        .copied()
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "golem:secrets/create@0.1.0 is missing from these host/SDK worlds: {missing:?}"
    );
}

#[test]
fn agent_error_reject_decoder_drains_secret_handle() {
    let mut resolver = TableResolver::new();
    let valid = encode_typed(&TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::bool()),
        SchemaValue::Bool(false),
    ))
    .expect("encode valid typed");
    let secret = resolver.secret_handle();
    let wire_err =
        crate::schema::agent::wit::wire::AgentError::CustomError(wire::TypedSchemaValue {
            graph: valid.graph,
            value: wire::SchemaValueTree {
                value_nodes: vec![wire::SchemaValueNode::SecretValue(secret)],
                root: 0,
            },
        });

    let err =
        crate::schema::agent::wit::decode_agent_error_rejecting_quota_with(wire_err, &mut resolver)
            .expect_err("custom errors must not carry secret handles");
    assert!(err.to_string().contains("secret"));
    assert_eq!(
        resolver.live, 0,
        "secret handle was rejected but not drained"
    );
}
