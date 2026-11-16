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

use super::*;
use crate::base_model::ShardId;
use crate::base_model::agent_id::H_BITS;
use crate::model::agent::AgentTypeName;
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;
use test_r::test;
use uuid::{Uuid, Variant, Version, uuid};

const ENV: EnvironmentId = EnvironmentId(uuid!("6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b"));
const OTHER_ENV: EnvironmentId = EnvironmentId(uuid!("0e0d0c0b-0a09-4807-8605-040302010000"));

fn type_id(id: u32) -> TypeId {
    TypeId::try_from(id).unwrap()
}

fn key(name: &str, number: u32) -> AgentKey {
    let field = |name: &str, body| NamedFieldType {
        name: name.to_string(),
        body,
        metadata: MetadataEnvelope::default(),
    };
    AgentKey::new(
        AgentTypeName("Counter".to_string()),
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(vec![
                field("name", SchemaType::string()),
                field("number", SchemaType::u32()),
            ])),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String(name.to_string()),
                    SchemaValue::U32(number),
                ],
            },
        ),
    )
    .unwrap()
}

fn idempotency_key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value.to_string())
}

const CALLER_FINGERPRINT: AgentFingerprint =
    AgentFingerprint(uuid!("00000000-0000-0000-0000-000000000005"));
const COMPONENT: ComponentId = ComponentId(uuid!("11111111-2222-4333-8444-555555555555"));

const PHANTOM_NAME: &str = r#"Counter("x")[0b6f2c4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f]"#;

fn all_derivations() -> Vec<(&'static str, AgentId)> {
    let root = AgentId::keyed(ENV, type_id(7), &key("alice", 42));
    vec![
        ("keyed", root),
        (
            "idempotent_create",
            AgentId::idempotent_create(ENV, type_id(7), "user:acct-1", &idempotency_key("key-1")),
        ),
        (
            "ephemeral_invocation",
            AgentId::ephemeral_invocation(ENV, type_id(7), &idempotency_key("owner-key-1")),
        ),
        (
            "durable_stream_fork",
            AgentId::durable_stream_fork(ENV, root, "fork-a"),
        ),
        (
            "durable_stream_session",
            AgentId::durable_stream_session(ENV, type_id(7), "route:api/stream", "session-1"),
        ),
        (
            "imported_phantom",
            AgentId::imported_phantom(ENV, type_id(7), PHANTOM_NAME),
        ),
        (
            "allocated",
            AgentId::allocated(ENV, 7, root, CALLER_FINGERPRINT, &idempotency_key("call-1")),
        ),
        (
            "external_tool_owner",
            AgentId::external_tool_owner(ENV, COMPONENT, &idempotency_key("owner-key-1")),
        ),
    ]
}

/// The expected IDs were computed independently of this module, from the layout and
/// domain tags in its documentation. Any change to them moves existing agents.
#[test]
fn golden_vectors() {
    let actual: Vec<(&str, String)> = all_derivations()
        .into_iter()
        .map(|(name, id)| (name, id.to_string()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("keyed", "00000007-0cfd-8d08-825f-b29b4e7457ad".to_string()),
            (
                "idempotent_create",
                "00000007-fb86-88c4-bd22-e49a08a43417".to_string()
            ),
            (
                "ephemeral_invocation",
                "00000007-2814-87e4-be24-1eca3768b3a6".to_string()
            ),
            (
                "durable_stream_fork",
                "00000007-8bbd-8649-ab48-43b063c528dd".to_string()
            ),
            (
                "durable_stream_session",
                "00000007-8b41-8849-a71b-f474822c84f1".to_string()
            ),
            (
                "imported_phantom",
                "00000007-863e-802f-8498-562366a23c83".to_string()
            ),
            (
                "allocated",
                "00000007-5161-8feb-b559-215f1122f82b".to_string()
            ),
            (
                "external_tool_owner",
                "00000000-564d-8e3b-a1f0-d53af5172140".to_string()
            ),
        ]
    );
}

#[test]
fn the_same_inputs_to_different_derivations_give_different_ids() {
    let ids: BTreeSet<AgentId> = all_derivations().into_iter().map(|(_, id)| id).collect();
    assert_eq!(ids.len(), all_derivations().len());
}

#[test]
fn derivations_are_deterministic() {
    assert_eq!(all_derivations(), all_derivations());
}

#[test]
fn variable_length_inputs_are_length_prefixed() {
    let key = |value: &str| idempotency_key(value);
    assert_ne!(
        AgentId::idempotent_create(ENV, type_id(7), "ab", &key("c")),
        AgentId::idempotent_create(ENV, type_id(7), "a", &key("bc")),
    );
    assert_ne!(
        AgentId::durable_stream_session(ENV, type_id(7), "ab", "c"),
        AgentId::durable_stream_session(ENV, type_id(7), "a", "bc"),
    );
}

#[test]
fn environment_and_type_are_part_of_every_typed_derivation() {
    let alice = key("alice", 42);
    assert_ne!(
        AgentId::keyed(ENV, type_id(7), &alice),
        AgentId::keyed(OTHER_ENV, type_id(7), &alice)
    );
    assert_ne!(
        AgentId::keyed(ENV, type_id(7), &alice).as_uuid().as_bytes()[4..],
        AgentId::keyed(ENV, type_id(8), &alice).as_uuid().as_bytes()[4..],
        "the type ID must be hashed, not only used as the tag"
    );
    assert_ne!(
        AgentId::idempotent_create(ENV, type_id(7), "s", &idempotency_key("k")),
        AgentId::idempotent_create(OTHER_ENV, type_id(7), "s", &idempotency_key("k"))
    );
    assert_ne!(
        AgentId::durable_stream_session(ENV, type_id(7), "r", "s"),
        AgentId::durable_stream_session(OTHER_ENV, type_id(7), "r", "s")
    );
    assert_ne!(
        AgentId::durable_stream_session(ENV, type_id(7), "r", "s"),
        AgentId::durable_stream_session(ENV, type_id(8), "r", "s")
    );
    assert_ne!(
        AgentId::imported_phantom(ENV, type_id(7), PHANTOM_NAME),
        AgentId::imported_phantom(OTHER_ENV, type_id(7), PHANTOM_NAME)
    );
}

#[test]
fn keyed_ids_depend_on_the_key() {
    let ids: BTreeSet<AgentId> = [key("alice", 42), key("alice", 43), key("bob", 42)]
        .iter()
        .map(|k| AgentId::keyed(ENV, type_id(7), k))
        .collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn ids_are_version_8_and_carry_their_tag() {
    for tag in [0, 1, u32::MAX] {
        for h in [0, H_MASK, 0x2AA_AAAA_AAAA_AAAA_AAAA_AAAA] {
            let id = AgentId::from_parts(tag, h);
            assert_eq!(id.as_uuid().get_version(), Some(Version::Custom));
            assert_eq!(id.as_uuid().get_variant(), Variant::RFC4122);
            assert_eq!(id.type_tag(), tag);
            assert_eq!(id.h(), h);
            assert_eq!(id.to_string().parse::<AgentId>(), Ok(id));
        }
    }
    assert_eq!(
        AgentId::from_parts(u32::MAX, 0).to_string(),
        "ffffffff-0000-8000-8000-000000000000"
    );
    assert_eq!(
        AgentId::from_parts(1, H_MASK).to_string(),
        "00000001-ffff-8fff-bfff-ffffffffffff"
    );
}

#[test]
fn every_bit_of_h_has_its_own_uuid_bit() {
    let zero = AgentId::from_parts(0, 0).as_uuid().as_u128();
    let mut seen = 0u128;
    for bit in 0..H_BITS {
        let id = AgentId::from_parts(0, 1 << bit);
        assert_eq!(id.h(), 1 << bit);
        let changed = id.as_uuid().as_u128() ^ zero;
        assert_eq!(changed.count_ones(), 1, "bit {bit}");
        assert_eq!(changed & seen, 0, "bit {bit}");
        seen |= changed;
    }
    assert_eq!(seen.count_ones(), H_BITS);
    assert_eq!(seen >> 96, 0, "H never reaches the tag");
}

#[test]
fn the_reserved_tag_has_no_type_id() {
    assert_eq!(AgentId::from_parts(RESERVED_TYPE_TAG, 5).type_id(), None);
    assert_eq!(AgentId::random_reserved().type_tag(), RESERVED_TYPE_TAG);
    assert_eq!(AgentId::from_parts(7, 5).type_id(), Some(type_id(7)));
    assert_eq!(
        AgentId::external_tool_owner(ENV, COMPONENT, &idempotency_key("k")).type_id(),
        None
    );
}

#[test]
fn allocated_ids_depend_on_every_input() {
    let root = AgentId::keyed(ENV, type_id(7), &key("alice", 42));
    let other = AgentId::keyed(ENV, type_id(7), &key("bob", 42));
    let call = idempotency_key("call-1");
    let id = AgentId::allocated(ENV, 9, root, CALLER_FINGERPRINT, &call);
    assert_eq!(id.type_tag(), 9);
    assert_eq!(
        id,
        AgentId::allocated(ENV, 9, root, CALLER_FINGERPRINT, &call)
    );
    let others = [
        AgentId::allocated(OTHER_ENV, 9, root, CALLER_FINGERPRINT, &call),
        AgentId::allocated(ENV, 8, root, CALLER_FINGERPRINT, &call),
        AgentId::allocated(ENV, 9, other, CALLER_FINGERPRINT, &call),
        AgentId::allocated(ENV, 9, root, AgentFingerprint(Uuid::from_u128(6)), &call),
        AgentId::allocated(ENV, 9, root, CALLER_FINGERPRINT, &idempotency_key("call-2")),
    ];
    for other in others {
        assert_ne!(
            other.as_uuid().as_bytes()[4..],
            id.as_uuid().as_bytes()[4..]
        );
    }
}

#[test]
fn external_tool_owners_depend_on_every_input() {
    let key = idempotency_key("key-1");
    let id = AgentId::external_tool_owner(ENV, COMPONENT, &key);
    let others = [
        AgentId::external_tool_owner(OTHER_ENV, COMPONENT, &key),
        AgentId::external_tool_owner(ENV, ComponentId(Uuid::from_u128(1)), &key),
        AgentId::external_tool_owner(ENV, COMPONENT, &idempotency_key("key-2")),
    ];
    for other in others {
        assert_ne!(other, id);
    }
}

#[test]
fn shards_come_from_the_hash_bits() {
    const SHARDS: usize = 16;
    const KEYS: usize = 10_000;
    let mut counts = [0usize; SHARDS];
    for i in 0..KEYS {
        let id = AgentId::keyed(ENV, type_id(7), &key(&format!("key-{i}"), 0));
        counts[ShardId::from_routing_hash(id.routing_hash(), SHARDS).value() as usize] += 1;
    }
    let mean = KEYS / SHARDS;
    for (shard, count) in counts.iter().enumerate() {
        assert!(
            count.abs_diff(mean) <= mean / 5,
            "shard {shard} got {count} of {KEYS} keys: {counts:?}"
        );
    }
}

#[test]
fn routing_hash_ignores_the_tag() {
    assert_eq!(
        AgentId::from_parts(1, 0x1234_5678).routing_hash(),
        AgentId::from_parts(2, 0x1234_5678).routing_hash()
    );
    assert_eq!(AgentId::from_parts(0, H_MASK).routing_hash(), i64::MAX);
    assert_eq!(
        AgentId::from_parts(0, 1 << (H_BITS - 1)).routing_hash(),
        1 << 62
    );
    assert_eq!(AgentId::from_parts(0, 1 << (H_BITS - 63)).routing_hash(), 1);
    assert_eq!(
        AgentId::from_parts(u32::MAX, (1 << (H_BITS - 63)) - 1).routing_hash(),
        0
    );
}

#[test]
fn random_ids_carry_the_tag_and_differ() {
    let a = AgentId::random(type_id(9));
    let b = AgentId::random(type_id(9));
    assert_eq!(a.type_tag(), 9);
    assert_eq!(b.type_tag(), 9);
    assert_ne!(a, b);
}

#[test]
fn durable_stream_forks() {
    let root = AgentId::keyed(ENV, type_id(7), &key("alice", 42));
    let other_root = AgentId::keyed(ENV, type_id(7), &key("bob", 42));
    let fork = AgentId::durable_stream_fork(ENV, root, "fork-a");
    assert_eq!(fork.type_tag(), root.type_tag());
    assert_eq!(fork, AgentId::durable_stream_fork(ENV, root, "fork-a"));
    assert_ne!(fork, AgentId::durable_stream_fork(ENV, root, "fork-b"));
    assert_ne!(
        fork,
        AgentId::durable_stream_fork(ENV, other_root, "fork-a")
    );
    assert_ne!(
        fork,
        AgentId::durable_stream_fork(OTHER_ENV, root, "fork-a")
    );
    assert_ne!(fork, root);
    let tagged_root = AgentId::from_parts(u32::MAX, 5);
    assert_eq!(
        AgentId::durable_stream_fork(ENV, tagged_root, "fork-a").type_tag(),
        u32::MAX
    );
}

#[test]
fn durable_stream_sessions() {
    let session = AgentId::durable_stream_session(ENV, type_id(7), "route-a", "s-1");
    assert_eq!(session.type_tag(), 7);
    assert_eq!(
        session,
        AgentId::durable_stream_session(ENV, type_id(7), "route-a", "s-1")
    );
    assert_ne!(
        session,
        AgentId::durable_stream_session(ENV, type_id(7), "route-a", "s-2")
    );
    assert_ne!(
        session,
        AgentId::durable_stream_session(ENV, type_id(7), "route-b", "s-1")
    );
    assert_ne!(
        session,
        AgentId::ephemeral_invocation(ENV, type_id(7), &idempotency_key("s-1"))
    );
}

#[test]
fn imported_phantoms_hash_the_whole_stored_name() {
    let names = [
        PHANTOM_NAME,
        r#"Counter("x")[0b6f2c4e-1a2b-4c3d-8e9f-0a1b2c3d4e60]"#,
        r#"Counter("y")[0b6f2c4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f]"#,
        r#"Other("x")[0b6f2c4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f]"#,
        r#"Counter(m[("a",1),("b",2)])[0b6f2c4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f]"#,
        r#"Counter(m[("b",2),("a",1)])[0b6f2c4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f]"#,
    ];
    let ids: BTreeSet<AgentId> = names
        .iter()
        .map(|name| AgentId::imported_phantom(ENV, type_id(7), name))
        .collect();
    assert_eq!(ids.len(), names.len());
    assert_eq!(
        AgentId::imported_phantom(ENV, type_id(7), PHANTOM_NAME),
        AgentId::imported_phantom(ENV, type_id(7), PHANTOM_NAME)
    );
}
