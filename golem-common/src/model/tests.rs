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

use crate::model::component::{CanonicalFilePath, ComponentRevision};
use crate::model::durable_stream::{
    AttachmentId, AttemptId, StreamInvocationIdV1, StreamSessionAttachedRecordV1,
    StreamSessionRecordV1,
};
use crate::model::environment::EnvironmentId;
use crate::model::oplog::OplogIndex;
use crate::model::worker::TypedAgentConfigEntry;
use crate::model::{
    AccountEmail, AccountId, AgentFilter, AgentFingerprint, AgentId, AgentMetadata, AgentMode,
    AgentStatus, AgentStatusRecord, ComponentId, DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
    DEFAULT_INVOCATION_RESULT_BLOOM_HASHES, DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY,
    DurableStreamSessionIndex, DurableStreamSessionStatus, FilterComparator, IdempotencyKey,
    InvocationResultBloom, InvocationResultMembership, PendingInvocationRef, PendingUpdateKind,
    PendingUpdateRef, ReceivedCardTransferIndex, ReceivedCardTransferState, StringFilterComparator,
    Timestamp,
};
use desert_rust::BinaryCodec;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::vec;
use test_r::test;
use uuid::{Uuid, uuid};

#[test]
fn durable_stream_session_index_retains_unfinished_and_bounded_recent_finished() {
    let mut index = DurableStreamSessionIndex::default();
    let unfinished = IdempotencyKey::new("unfinished".to_string());
    index.insert(
        unfinished.clone(),
        DurableStreamSessionStatus {
            first_prepared: Some(OplogIndex::from_u64(1)),
            ..Default::default()
        },
    );
    for n in 0..140 {
        index.insert(
            IdempotencyKey::new(format!("done-{n}")),
            DurableStreamSessionStatus {
                first_prepared: Some(OplogIndex::from_u64(n + 2)),
                finished: Some(OplogIndex::from_u64(n + 1000)),
                ..Default::default()
            },
        );
    }
    assert!(index.has_history());
    assert!(index.get(&unfinished).is_some());
    assert_eq!(
        index
            .iter()
            .filter(|(_, value)| value.finished.is_some())
            .count(),
        128
    );
    assert!(
        index
            .get(&IdempotencyKey::new("done-0".to_string()))
            .is_none()
    );
    assert!(
        index
            .get(&IdempotencyKey::new("done-139".to_string()))
            .is_some()
    );
}

#[test]
fn durable_stream_initial_attachment_requires_exact_pending_evidence() {
    let key = IdempotencyKey::new("invocation".to_string());
    let session_key = StreamInvocationIdV1 {
        callee_environment_id: EnvironmentId::new(),
        callee: AgentId {
            component_id: ComponentId::new(),
            agent_id: "agent".to_string(),
        },
        callee_fingerprint: AgentFingerprint(Uuid::new_v4()),
        idempotency_key: key.clone(),
    };
    let attempt = AttemptId::fresh();
    let attachment_id = AttachmentId::primary(
        session_key.callee_environment_id,
        &session_key.callee,
        &session_key.idempotency_key,
    )
    .unwrap();
    let pending = OplogIndex::from_u64(12);
    let mut status = DurableStreamSessionStatus {
        first_prepared: Some(OplogIndex::from_u64(10)),
        prepared: Some(OplogIndex::from_u64(10)),
        session_key: Some(session_key.clone()),
        prepared_attempt_id: Some(attempt),
        ..Default::default()
    };
    status.apply_pending_invocation(pending, &key);
    status.apply_record(
        OplogIndex::from_u64(13),
        &StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
            format_version: 1,
            session_key,
            attachment_id,
            attempt_id: attempt,
            epoch: 1,
            pending_invocation_oplog_index: pending,
        }),
    );
    assert_eq!(status.validated_initial_pending_invocation, Some(pending));
    assert_eq!(status.lifecycle_error, None);
}

#[test]
fn durable_stream_initial_attachment_records_malformed_pending_relationship() {
    let key = IdempotencyKey::new("invocation".to_string());
    let session_key = StreamInvocationIdV1 {
        callee_environment_id: EnvironmentId::new(),
        callee: AgentId {
            component_id: ComponentId::new(),
            agent_id: "agent".to_string(),
        },
        callee_fingerprint: AgentFingerprint(Uuid::new_v4()),
        idempotency_key: key,
    };
    let attempt = AttemptId::fresh();
    let attachment_id = AttachmentId::primary(
        session_key.callee_environment_id,
        &session_key.callee,
        &session_key.idempotency_key,
    )
    .unwrap();
    let mut status = DurableStreamSessionStatus {
        first_prepared: Some(OplogIndex::from_u64(10)),
        prepared: Some(OplogIndex::from_u64(10)),
        session_key: Some(session_key.clone()),
        prepared_attempt_id: Some(attempt),
        ..Default::default()
    };
    status.apply_record(
        OplogIndex::from_u64(13),
        &StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
            format_version: 1,
            session_key,
            attachment_id,
            attempt_id: attempt,
            epoch: 1,
            pending_invocation_oplog_index: OplogIndex::from_u64(12),
        }),
    );
    assert!(status.lifecycle_error.is_some());
    assert_eq!(status.attachment_attached, None);

    for pending in [
        OplogIndex::NONE,
        OplogIndex::from_u64(9),
        OplogIndex::from_u64(13),
        OplogIndex::from_u64(14),
    ] {
        let mut invalid = DurableStreamSessionStatus {
            first_prepared: Some(OplogIndex::from_u64(10)),
            prepared: Some(OplogIndex::from_u64(10)),
            session_key: status.session_key.clone(),
            prepared_attempt_id: Some(attempt),
            ..Default::default()
        };
        invalid.apply_pending_invocation(
            pending,
            &invalid
                .session_key
                .as_ref()
                .unwrap()
                .idempotency_key
                .clone(),
        );
        let mut attached = match StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
            format_version: 1,
            session_key: invalid.session_key.clone().unwrap(),
            attachment_id: AttachmentId::primary(
                invalid.session_key.as_ref().unwrap().callee_environment_id,
                &invalid.session_key.as_ref().unwrap().callee,
                &invalid.session_key.as_ref().unwrap().idempotency_key,
            )
            .unwrap(),
            attempt_id: attempt,
            epoch: 1,
            pending_invocation_oplog_index: pending,
        }) {
            StreamSessionRecordV1::Attached(value) => value,
            _ => unreachable!(),
        };
        invalid.apply_record(
            OplogIndex::from_u64(13),
            &StreamSessionRecordV1::Attached(attached.clone()),
        );
        assert!(
            invalid.lifecycle_error.is_some(),
            "pending index {pending:?}"
        );
        assert_eq!(invalid.attachment_attached, None);

        attached.format_version = 2;
        let mut unsupported = DurableStreamSessionStatus {
            first_prepared: Some(OplogIndex::from_u64(10)),
            prepared: Some(OplogIndex::from_u64(10)),
            session_key: Some(attached.session_key.clone()),
            prepared_attempt_id: Some(attempt),
            ..Default::default()
        };
        assert!(
            !unsupported.validate_initial_attachment_reference(OplogIndex::from_u64(13), &attached)
        );
        assert!(unsupported.lifecycle_error.is_some());
    }
}

#[test]
fn invocation_result_membership_bounds_exact_entries_without_false_negatives() {
    let mut membership = InvocationResultMembership::new(2, 64, 3);
    let first = IdempotencyKey::new("first".to_string());
    let second = IdempotencyKey::new("second".to_string());
    let third = IdempotencyKey::new("third".to_string());
    let fourth = IdempotencyKey::new("fourth".to_string());
    let fifth = IdempotencyKey::new("fifth".to_string());

    membership.insert(first.clone(), OplogIndex::from_u64(10));
    membership.insert(second.clone(), OplogIndex::from_u64(20));
    assert!(membership.is_exact_complete());

    membership.insert(third.clone(), OplogIndex::from_u64(30));
    membership.insert(fourth.clone(), OplogIndex::from_u64(40));
    membership.insert(fifth.clone(), OplogIndex::from_u64(50));

    assert_eq!(membership.len(), 2);
    assert!(!membership.is_exact_complete());
    assert_eq!(membership.change_generation(), 5);
    assert_eq!(
        membership.oldest_retained_index(),
        Some(OplogIndex::from_u64(40))
    );
    assert_eq!(membership.get(&first), None);
    assert_eq!(membership.get(&second), None);
    assert_eq!(membership.get(&third), None);
    assert_eq!(membership.get(&fourth), Some(&OplogIndex::from_u64(40)));
    assert_eq!(membership.get(&fifth), Some(&OplogIndex::from_u64(50)));
    assert!(membership.might_contain(&first));
    assert!(membership.might_contain(&second));
    assert!(membership.might_contain(&third));
    assert!(membership.might_contain(&fourth));
    assert!(membership.might_contain(&fifth));
}

#[test]
fn invocation_result_membership_updates_recency_for_repeated_keys() {
    let mut membership = InvocationResultMembership::new(2, 64, 3);
    let first = IdempotencyKey::new("first".to_string());
    let second = IdempotencyKey::new("second".to_string());
    let third = IdempotencyKey::new("third".to_string());
    let fourth = IdempotencyKey::new("fourth".to_string());
    let fifth = IdempotencyKey::new("fifth".to_string());
    let sixth = IdempotencyKey::new("sixth".to_string());

    membership.insert(first.clone(), OplogIndex::from_u64(10));
    membership.insert(second.clone(), OplogIndex::from_u64(20));
    membership.insert(third, OplogIndex::from_u64(30));
    membership.insert(fourth, OplogIndex::from_u64(40));
    membership.insert(fifth.clone(), OplogIndex::from_u64(50));
    membership.insert(first.clone(), OplogIndex::from_u64(60));
    membership.insert(sixth.clone(), OplogIndex::from_u64(70));

    assert_eq!(membership.get(&first), Some(&OplogIndex::from_u64(60)));
    assert_eq!(membership.get(&second), None);
    assert_eq!(membership.get(&fifth), None);
    assert_eq!(membership.get(&sixth), Some(&OplogIndex::from_u64(70)));
}

#[test]
fn invocation_result_membership_binary_round_trip_preserves_membership() {
    use crate::serialization::{deserialize, serialize};

    let mut membership = InvocationResultMembership::new(2, 64, 3);
    let first = IdempotencyKey::new("first".to_string());
    let second = IdempotencyKey::new("second".to_string());
    let third = IdempotencyKey::new("third".to_string());
    let fourth = IdempotencyKey::new("fourth".to_string());
    let fifth = IdempotencyKey::new("fifth".to_string());
    membership.insert(first.clone(), OplogIndex::from_u64(10));
    membership.insert(second, OplogIndex::from_u64(20));
    membership.insert(third, OplogIndex::from_u64(30));
    membership.insert(fourth, OplogIndex::from_u64(40));
    membership.insert(fifth, OplogIndex::from_u64(50));
    membership.set_revert_generation(4);

    let bytes = serialize(&membership).unwrap();
    let recovered: InvocationResultMembership = deserialize(&bytes).unwrap();

    assert_eq!(recovered, membership);
    assert!(recovered.might_contain(&first));
    assert_eq!(recovered.revert_generation(), 4);
}

#[test]
fn small_invocation_result_membership_serializes_without_a_bloom_filter() {
    use crate::serialization::{deserialize, serialize};

    let mut membership = InvocationResultMembership::default();
    membership.insert(
        IdempotencyKey::new("first".to_string()),
        OplogIndex::from_u64(10),
    );

    assert!(membership.is_exact_complete());
    let bytes = serialize(&membership).unwrap();
    assert!(bytes.len() < 1024);
    let recovered: InvocationResultMembership = deserialize(&bytes).unwrap();
    assert_eq!(recovered, membership);
}

#[test]
fn default_invocation_result_bloom_keeps_new_invocations_local_at_100x_capacity() {
    let history_size = 100 * DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY;
    let mut bloom = InvocationResultBloom::new(
        DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
        DEFAULT_INVOCATION_RESULT_BLOOM_HASHES,
    );
    for index in 0..history_size {
        bloom.insert(&IdempotencyKey::new(format!("existing-{index}")));
    }

    let sample_size = 100_000;
    let false_positives = (0..sample_size)
        .filter(|index| bloom.might_contain(&IdempotencyKey::new(format!("new-{index}"))))
        .count();

    assert!(
        false_positives * 100 < sample_size * 2,
        "default Bloom filter sent {false_positives}/{sample_size} new invocations to physical lookup"
    );
}

#[test]
fn timestamp_conversion() {
    let ts: Timestamp = Timestamp::now_utc();

    let prost_ts: prost_types::Timestamp = ts.into();

    let ts2: Timestamp = prost_ts.into();

    assert_eq!(ts2, ts);
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
struct ExampleWithAccountId {
    account_id: AccountId,
}

#[test]
fn account_id_from_json_apigateway_version() {
    let json = "{ \"account_id\": \"f935056f-e2f0-4183-a40f-d8ef3011f0bc\" }";
    let example: ExampleWithAccountId = serde_json::from_str(json).unwrap();
    assert_eq!(
        example.account_id,
        AccountId(uuid!("f935056f-e2f0-4183-a40f-d8ef3011f0bc"))
    );
}

#[test]
fn account_id_json_serialization() {
    // We want to use this variant for serialization because it is used on the public API gateway API
    let example: ExampleWithAccountId = ExampleWithAccountId {
        account_id: AccountId(uuid!("f935056f-e2f0-4183-a40f-d8ef3011f0bc")),
    };
    let json = serde_json::to_string(&example).unwrap();
    assert_eq!(
        json,
        "{\"account_id\":\"f935056f-e2f0-4183-a40f-d8ef3011f0bc\"}"
    );
}

#[test]
fn worker_filter_parse() {
    assert_eq!(
        AgentFilter::from_str(" name =  worker-1").unwrap(),
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
    );

    assert_eq!(
        AgentFilter::from_str("status == Running").unwrap(),
        AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running)
    );

    assert_eq!(
        AgentFilter::from_str("revision >= 10").unwrap(),
        AgentFilter::new_revision(
            FilterComparator::GreaterEqual,
            ComponentRevision::new(10).unwrap()
        )
    );

    assert_eq!(
        AgentFilter::from_str("env.tag1 == abc ").unwrap(),
        AgentFilter::new_env(
            "tag1".to_string(),
            StringFilterComparator::Equal,
            "abc".to_string(),
        )
    );
}

#[test]
fn worker_filter_combination() {
    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).not(),
        AgentFilter::new_not(AgentFilter::new_name(
            StringFilterComparator::Equal,
            "worker-1".to_string(),
        ))
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).and(
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running)
        ),
        AgentFilter::new_and(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_status(
                FilterComparator::Equal,
                AgentStatus::Running,
            ))
            .and(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_and(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()).or(
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running)
        ),
        AgentFilter::new_or(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .or(AgentFilter::new_status(
                FilterComparator::NotEqual,
                AgentStatus::Running,
            ))
            .or(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_or(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
            AgentFilter::new_status(FilterComparator::NotEqual, AgentStatus::Running),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_status(
                FilterComparator::NotEqual,
                AgentStatus::Running,
            ))
            .or(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_or(vec![
            AgentFilter::new_and(vec![
                AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                AgentFilter::new_status(FilterComparator::NotEqual, AgentStatus::Running),
            ]),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );

    assert_eq!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .or(AgentFilter::new_status(
                FilterComparator::NotEqual,
                AgentStatus::Running,
            ))
            .and(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            )),
        AgentFilter::new_and(vec![
            AgentFilter::new_or(vec![
                AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string()),
                AgentFilter::new_status(FilterComparator::NotEqual, AgentStatus::Running),
            ]),
            AgentFilter::new_revision(FilterComparator::Equal, ComponentRevision::new(1).unwrap()),
        ])
    );
}

#[test]
fn worker_filter_matches() {
    let component_id = ComponentId::new();
    let worker_metadata = AgentMetadata {
        agent_id: AgentId {
            agent_id: "worker-1".to_string(),
            component_id,
        },
        env: vec![
            ("env1".to_string(), "value1".to_string()),
            ("env2".to_string(), "value2".to_string()),
        ],
        environment_id: EnvironmentId::new(),
        created_by: AccountId(uuid!("f935056f-e2f0-4183-a40f-d8ef3011f0bc")),
        created_by_email: AccountEmail::new("test@golem"),
        config: vec![TypedAgentConfigEntry {
            path: vec!["var1".to_string()],
            value: crate::schema::IntoTypedSchemaValue::into_typed_schema_value(
                &"value1".to_string(),
            )
            .unwrap(),
        }],
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord {
            component_revision: ComponentRevision::new(1).unwrap(),
            ..AgentStatusRecord::default()
        },
        original_phantom_id: None,
        fingerprint: AgentFingerprint(Uuid::now_v7()),
        agent_mode: AgentMode::Durable,
    };

    assert!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_status(
                FilterComparator::Equal,
                AgentStatus::Idle,
            ))
            .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_env(
            "env1".to_string(),
            StringFilterComparator::Equal,
            "value1".to_string(),
        )
        .and(AgentFilter::new_status(
            FilterComparator::Equal,
            AgentStatus::Idle,
        ))
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_env(
            "env1".to_string(),
            StringFilterComparator::Equal,
            "value2".to_string(),
        )
        .not()
        .and(
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Running).or(
                AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Idle)
            )
        )
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-1".to_string())
            .and(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            ))
            .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_name(StringFilterComparator::Equal, "worker-2".to_string())
            .or(AgentFilter::new_revision(
                FilterComparator::Equal,
                ComponentRevision::new(1).unwrap()
            ))
            .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_revision(
            FilterComparator::GreaterEqual,
            ComponentRevision::new(1).unwrap()
        )
        .and(AgentFilter::new_revision(
            FilterComparator::Less,
            ComponentRevision::new(2).unwrap()
        ))
        .or(AgentFilter::new_name(
            StringFilterComparator::Equal,
            "worker-2".to_string(),
        ))
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_config(
            "var1".to_string(),
            StringFilterComparator::Equal,
            "value1".to_string(),
        )
        .matches(&worker_metadata)
    );

    assert!(
        !AgentFilter::new_config(
            "var1".to_string(),
            StringFilterComparator::Equal,
            "value2".to_string(),
        )
        .matches(&worker_metadata)
    );

    assert!(
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable)
            .matches(&worker_metadata)
    );

    assert!(
        !AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral)
            .matches(&worker_metadata)
    );

    let ephemeral_metadata = AgentMetadata {
        agent_mode: AgentMode::Ephemeral,
        ..worker_metadata.clone()
    };

    assert!(
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral)
            .matches(&ephemeral_metadata)
    );

    assert!(
        AgentFilter::new_mode(FilterComparator::NotEqual, AgentMode::Durable)
            .matches(&ephemeral_metadata)
    );

    let parsed = AgentFilter::from_str("mode == ephemeral").unwrap();
    assert_eq!(
        parsed,
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral)
    );
    let parsed = AgentFilter::from_str("mode == Durable").unwrap();
    assert_eq!(
        parsed,
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable)
    );
}

#[test]
fn agent_status_record_has_pending_work_for_pending_invocations() {
    let mut status = AgentStatusRecord::default();
    status.pending_invocations.push(PendingInvocationRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        idempotency_key: None,
        manual_update_target_revision: Some(ComponentRevision::INITIAL),
    });

    assert!(status.has_pending_work());
}

#[test]
fn agent_status_record_has_pending_work_for_pending_updates() {
    let mut status = AgentStatusRecord::default();
    status.pending_updates.push_back(PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        target_revision: ComponentRevision::INITIAL,
        kind: PendingUpdateKind::Automatic,
    });

    assert!(status.has_pending_work());
}

#[test]
fn agent_status_record_has_pending_work_is_false_without_pending_queues() {
    assert!(!AgentStatusRecord::default().has_pending_work());
}

#[test]
fn derived_idempotency_key() {
    let base1 = IdempotencyKey::fresh();
    let base2 = IdempotencyKey::fresh();
    let base3 = IdempotencyKey {
        value: "base3".to_string(),
    };

    assert_ne!(base1, base2);

    let idx1 = OplogIndex::from_u64(2);
    let idx2 = OplogIndex::from_u64(11);

    let derived11a = IdempotencyKey::derived(&base1, idx1);
    let derived12a = IdempotencyKey::derived(&base1, idx2);
    let derived21a = IdempotencyKey::derived(&base2, idx1);
    let derived22a = IdempotencyKey::derived(&base2, idx2);

    let derived11b = IdempotencyKey::derived(&base1, idx1);
    let derived12b = IdempotencyKey::derived(&base1, idx2);
    let derived21b = IdempotencyKey::derived(&base2, idx1);
    let derived22b = IdempotencyKey::derived(&base2, idx2);

    let derived31 = IdempotencyKey::derived(&base3, idx1);
    let derived32 = IdempotencyKey::derived(&base3, idx2);

    assert_eq!(derived11a, derived11b);
    assert_eq!(derived12a, derived12b);
    assert_eq!(derived21a, derived21b);
    assert_eq!(derived22a, derived22b);

    assert_ne!(derived11a, derived12a);
    assert_ne!(derived11a, derived21a);
    assert_ne!(derived11a, derived22a);
    assert_ne!(derived12a, derived21a);
    assert_ne!(derived12a, derived22a);
    assert_ne!(derived21a, derived22a);

    assert_ne!(derived11a, derived31);
    assert_ne!(derived21a, derived31);
    assert_ne!(derived12a, derived32);
    assert_ne!(derived22a, derived32);
    assert_ne!(derived31, derived32);
}

#[test]
fn canonical_file_path_from_absolute() {
    let path = CanonicalFilePath::from_abs_str("/a/b/c").unwrap();
    assert_eq!(path.to_string(), "/a/b/c");
}

#[test]
fn canonical_file_path_from_relative_is_error() {
    let path = CanonicalFilePath::from_abs_str("a/b/c");
    assert!(path.is_err());
}

#[test]
fn agent_filter_mode_protobuf_round_trip() {
    use golem_api_grpc::proto::golem::worker::AgentFilter as ProtoAgentFilter;

    let original_durable = AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable);
    let proto: ProtoAgentFilter = original_durable.clone().into();
    let recovered: AgentFilter = proto.try_into().unwrap();
    assert_eq!(original_durable, recovered);

    let original_ephemeral =
        AgentFilter::new_mode(FilterComparator::NotEqual, AgentMode::Ephemeral);
    let proto: ProtoAgentFilter = original_ephemeral.clone().into();
    let recovered: AgentFilter = proto.try_into().unwrap();
    assert_eq!(original_ephemeral, recovered);

    let composite = AgentFilter::new_name(StringFilterComparator::Equal, "w1".to_string()).and(
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
    );
    let proto: ProtoAgentFilter = composite.clone().into();
    let recovered: AgentFilter = proto.try_into().unwrap();
    assert_eq!(composite, recovered);
}

#[test]
fn agent_status_record_agent_mode_is_not_serialized() {
    use crate::serialization::{deserialize, serialize};

    let mut received_card_transfers = ReceivedCardTransferIndex::default();
    received_card_transfers.insert(Uuid::new_v4(), ReceivedCardTransferState::Conflict);

    // `agent_mode` is `#[transient]`: it is excluded from the serialized status blob and stored
    // under a separate KV key instead. A record serialized with a non-default mode must therefore
    // deserialize back with the `Durable` default, while all other fields round-trip unchanged.
    let original = AgentStatusRecord {
        component_revision: ComponentRevision::new(7).unwrap(),
        component_size: 1234,
        received_card_transfers,
        agent_mode: AgentMode::Ephemeral,
        ..AgentStatusRecord::default()
    };

    let bytes = serialize(&original).unwrap();
    let recovered: AgentStatusRecord = deserialize(&bytes).unwrap();

    assert_eq!(
        recovered.agent_mode,
        AgentMode::Durable,
        "agent_mode must not be carried by the serialized blob (defaults on read)"
    );

    // Everything except the transient agent_mode must survive the round-trip.
    let expected = AgentStatusRecord {
        agent_mode: AgentMode::Durable,
        ..original
    };
    assert_eq!(recovered, expected);
}

#[test]
fn agent_invocation_result_redacted_debug_hides_capability_material() {
    use crate::model::AgentInvocationResult;
    use crate::schema::{SchemaValue, SecretValuePayload};

    let result = AgentInvocationResult::AgentMethod {
        output: SchemaValue::Record {
            fields: vec![
                SchemaValue::String("svc".to_string()),
                SchemaValue::Secret(SecretValuePayload {
                    secret_id: uuid::Uuid::nil(),
                    config_key: None,
                    version: 0,
                    resolved_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                    category: None,
                }),
            ],
        },
    };

    let rendered = format!("{:?}", result.redacted_debug());
    assert!(
        !rendered.contains("shhh-do-not-log"),
        "secret ref leaked into diagnostic debug: {rendered}"
    );
    assert!(
        rendered.contains("<redacted: secret>"),
        "expected redacted placeholder, got: {rendered}"
    );
    // Non-capability variants render normally.
    assert_eq!(
        format!("{:?}", AgentInvocationResult::ManualUpdate.redacted_debug()),
        format!("{:?}", AgentInvocationResult::ManualUpdate)
    );
}

#[test]
fn agent_invocation_payload_round_trip_preserves_scope_card() {
    use crate::model::card::{CardId, ScopeCard};
    use crate::model::invocation_context::InvocationContextStack;
    use crate::model::{AgentInvocation, AgentInvocationPayload, Principal};
    use crate::schema::SchemaValue;
    use crate::serialization::{deserialize, serialize};

    let scope_card = ScopeCard {
        scope_card_id: CardId(Uuid::from_u128(1)),
        root_card_ids: vec![CardId(Uuid::from_u128(2))],
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
    };
    let invocation = AgentInvocation::AgentMethod {
        idempotency_key: IdempotencyKey::fresh(),
        method_name: "run".to_string(),
        input: SchemaValue::Tuple {
            elements: Vec::new(),
        },
        invocation_context: InvocationContextStack::fresh(),
        principal: Principal::anonymous(),
        scope_card: Some(scope_card.clone()),
    };
    let (_, payload, _) = invocation.into_parts();
    let encoded = serialize(&payload).unwrap();
    let decoded: AgentInvocationPayload = deserialize(&encoded).unwrap();

    assert!(matches!(
        decoded,
        AgentInvocationPayload::AgentMethod {
            scope_card: Some(decoded_scope_card),
            ..
        } if decoded_scope_card == scope_card
    ));
}

#[test]
fn durable_stream_session_index_rejects_unloaded_external_payload() {
    use crate::model::oplog::{OplogEntry, OplogPayload, PayloadId};

    let entry = OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::External {
            payload_id: PayloadId::new(),
            md5_hash: vec![0; 16],
            cached: None,
        },
    };

    let error = DurableStreamSessionIndex::default()
        .apply_oplog_entry(OplogIndex::from_u64(1), &entry)
        .unwrap_err();
    assert!(error.contains("has not been loaded"));
}

#[test]
fn durable_stream_session_index_rejects_unsupported_inline_payload_version() {
    use crate::model::oplog::{OplogEntry, OplogPayload};

    let entry = OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::SerializedInline {
            bytes: vec![0xff],
            cached: None,
        },
    };

    let error = DurableStreamSessionIndex::default()
        .apply_oplog_entry(OplogIndex::from_u64(1), &entry)
        .unwrap_err();
    assert!(error.contains("unsupported serialization version"));
}

#[test]
fn durable_stream_session_index_rejects_malformed_inline_payload() {
    use crate::model::oplog::{OplogEntry, OplogPayload};

    let entry = OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::SerializedInline {
            bytes: vec![crate::serialization::SERIALIZATION_VERSION_V3, 0xff],
            cached: None,
        },
    };

    let error = DurableStreamSessionIndex::default()
        .apply_oplog_entry(OplogIndex::from_u64(1), &entry)
        .unwrap_err();
    assert!(error.contains("failed to decode"));
}
