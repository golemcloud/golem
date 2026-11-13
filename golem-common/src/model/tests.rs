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

use crate::model::agent::TypedAgentConfigEntry;
use crate::model::component::{CanonicalFilePath, ComponentRevision};
use crate::model::durable_stream::{
    AttachmentId, AttemptId, SessionStreamRole, StreamExportForkInitializedRecord,
    StreamInvocationId, StreamRegistrationInvocation, StreamSessionAttachedRecord,
    StreamSessionCancelRequestedRecord, StreamSessionExpiredRecord, StreamSessionExpiryPolicy,
    StreamSessionExpiryRefreshedRecord, StreamSessionRecord, StreamSlotTombstonedRecord,
};
use crate::model::environment::EnvironmentId;
use crate::model::oplog::OplogIndex;
use crate::model::{
    AccountEmail, AccountId, AgentFilter, AgentFingerprint, AgentId, AgentMetadata, AgentMode,
    AgentStatus, AgentStatusRecord, ComponentId, DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
    DEFAULT_INVOCATION_RESULT_BLOOM_HASHES, DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY,
    DurableStreamPublicBinding, DurableStreamSessionIndex, DurableStreamSessionStatus,
    ExportForkAdmissions, FilterComparator, IdempotencyKey, InvocationResultBloom,
    InvocationResultMembership, PendingInvocationRef, PendingUpdateKind, PendingUpdateRef,
    ReceivedCardTransferIndex, ReceivedCardTransferState, StringFilterComparator, Timestamp,
};
use desert_rust::BinaryCodec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::vec;
use test_r::test;
use uuid::{Uuid, uuid};

fn durable_stream_test_session_key(key: &str) -> StreamInvocationId {
    StreamInvocationId {
        callee_environment_id: EnvironmentId::new(),
        callee: AgentId {
            component_id: ComponentId::new(),
            agent_id: "agent".to_string(),
        },
        callee_fingerprint: AgentFingerprint(Uuid::new_v4()),
        idempotency_key: IdempotencyKey::new(key.to_string()),
    }
}

fn fold_public_binding(
    current: &mut Option<DurableStreamPublicBinding>,
    record: &StreamSessionRecord,
) {
    *current = DurableStreamPublicBinding::fold(current.as_ref(), record);
}

#[test]
fn durable_stream_public_binding_is_incarnation_and_deadline_fenced() {
    let first = durable_stream_test_session_key("first");
    let second = durable_stream_test_session_key("second");
    let public = "report-42";
    let initialize = |invocation: &StreamInvocationId, deadline| {
        StreamSessionRecord::ExportForkInitialized(StreamExportForkInitializedRecord {
            format_version: 1,
            public_session_id: public.to_string(),
            session_key: invocation.idempotency_key.clone(),
            source_invocation: invocation.clone(),
            request_hash: vec![7; 32],
            expiry_policy: StreamSessionExpiryPolicy::Sliding { ttl_seconds: 10 },
            expiry_deadline_millis: Some(deadline),
        })
    };
    let refresh = |invocation: &StreamInvocationId, expected, deadline| {
        StreamSessionRecord::ExpiryRefreshed(StreamSessionExpiryRefreshedRecord {
            format_version: 1,
            public_session_id: public.to_string(),
            session_key: invocation.idempotency_key.clone(),
            expected_deadline_millis: expected,
            refreshed_at_millis: deadline - 10_000,
            deadline_millis: deadline,
        })
    };
    let expire = |invocation: &StreamInvocationId, expected| {
        StreamSessionRecord::Expired(StreamSessionExpiredRecord {
            format_version: 1,
            public_session_id: public.to_string(),
            session_key: invocation.idempotency_key.clone(),
            expected_deadline_millis: expected,
            expired_at_millis: expected,
        })
    };

    for record in [
        initialize(&first, 10_000),
        refresh(&first, 10_000, 20_000),
        expire(&first, 20_000),
    ] {
        assert!(record.has_supported_format());
        let bytes = crate::serialization::serialize(&record).unwrap();
        let decoded: StreamSessionRecord = crate::serialization::deserialize(&bytes).unwrap();
        assert_eq!(decoded, record);
    }

    let mut binding = None;
    fold_public_binding(&mut binding, &initialize(&first, 10_000));
    fold_public_binding(&mut binding, &initialize(&first, 5_000));
    fold_public_binding(&mut binding, &refresh(&first, 9_999, 20_000));
    assert!(matches!(
        binding.as_ref(),
        Some(DurableStreamPublicBinding::Live {
            session_key,
            expiry_deadline_millis: Some(10_000),
            ..
        }) if session_key == &first.idempotency_key
    ));

    fold_public_binding(&mut binding, &refresh(&first, 10_000, 20_000));
    fold_public_binding(&mut binding, &initialize(&first, 15_000));
    fold_public_binding(&mut binding, &expire(&first, 10_000));
    assert!(matches!(
        binding.as_ref(),
        Some(DurableStreamPublicBinding::Live {
            expiry_deadline_millis: Some(20_000),
            ..
        })
    ));

    fold_public_binding(&mut binding, &expire(&first, 20_000));
    fold_public_binding(&mut binding, &initialize(&first, 30_000));
    assert!(matches!(
        binding.as_ref(),
        Some(DurableStreamPublicBinding::Retired { session_key })
            if session_key == &first.idempotency_key
    ));

    fold_public_binding(&mut binding, &initialize(&second, 30_000));
    fold_public_binding(&mut binding, &expire(&first, 20_000));
    fold_public_binding(&mut binding, &initialize(&first, 40_000));
    assert!(matches!(
        binding.as_ref(),
        Some(DurableStreamPublicBinding::Live {
            session_key,
            expiry_deadline_millis: Some(30_000),
            ..
        }) if session_key == &second.idempotency_key
    ));

    let bytes = crate::serialization::serialize(binding.as_ref().unwrap()).unwrap();
    let decoded: DurableStreamPublicBinding = crate::serialization::deserialize(&bytes).unwrap();
    assert_eq!(Some(decoded), binding);

    let mut status = DurableStreamSessionStatus::default();
    status.apply_record(OplogIndex::from_u64(1), &initialize(&first, 10_000));
    status.apply_record(OplogIndex::from_u64(2), &initialize(&first, 20_000));
    assert_eq!(status.expiry_deadline_millis, Some(10_000));
}

#[test]
fn durable_stream_absolute_expiry_cannot_be_refreshed_past_its_fixed_deadline() {
    let invocation = durable_stream_test_session_key("absolute");
    let public = "report-absolute";
    let mut index = DurableStreamSessionIndex::default();
    let initialized =
        StreamSessionRecord::ExportForkInitialized(StreamExportForkInitializedRecord {
            format_version: 1,
            public_session_id: public.to_string(),
            session_key: invocation.idempotency_key.clone(),
            source_invocation: invocation.clone(),
            request_hash: vec![7; 32],
            expiry_policy: StreamSessionExpiryPolicy::Absolute {
                expires_at_millis: 10_000,
            },
            expiry_deadline_millis: Some(10_000),
        });
    let refreshed = StreamSessionRecord::ExpiryRefreshed(StreamSessionExpiryRefreshedRecord {
        format_version: 1,
        session_key: invocation.idempotency_key.clone(),
        public_session_id: public.to_string(),
        expected_deadline_millis: 10_000,
        refreshed_at_millis: 9_000,
        deadline_millis: 20_000,
    });
    index.apply_record(OplogIndex::from_u64(1), &initialized);
    index.apply_record(OplogIndex::from_u64(2), &refreshed);
    let mut binding = None;
    fold_public_binding(&mut binding, &initialized);
    fold_public_binding(&mut binding, &refreshed);

    assert!(matches!(
        binding,
        Some(DurableStreamPublicBinding::Live {
            expiry_policy: StreamSessionExpiryPolicy::Absolute {
                expires_at_millis: 10_000
            },
            expiry_deadline_millis: Some(10_000),
            ..
        })
    ));
    assert_eq!(
        index
            .get(&invocation.idempotency_key)
            .unwrap()
            .expiry_deadline_millis,
        Some(10_000)
    );
}

#[test]
fn durable_stream_fork_status_resets_retained_sessions_and_preserves_results() {
    use crate::model::durable_stream::StreamForkCutRecord;
    let source = durable_stream_test_session_key("fork");
    let cut = StreamSessionRecord::ForkCut(StreamForkCutRecord {
        format_version: 1,
        request_hash: vec![0; 32],
        creation_fingerprint: source.callee_fingerprint,
        export: None,
        cut_index: OplogIndex::from_u64(20),
        revert: None,
        epoch_floor: 1,
        selected_stream_id: None,
        retained_through: None,
    });
    let prepared_attempt = AttemptId::fresh();
    let attachment_attempt = AttemptId::fresh();
    let original = DurableStreamSessionStatus {
        session_key: Some(source.idempotency_key.clone()),
        first_prepared: Some(OplogIndex::from_u64(2)),
        prepared: Some(OplogIndex::from_u64(2)),
        prepared_attempt_id: Some(prepared_attempt),
        initial_attachment_attempt_id: Some(prepared_attempt),
        initial_attachment_epoch: Some(1),
        validated_initial_pending_invocation: Some(OplogIndex::from_u64(3)),
        initial_pending_invocation_oplog_index: Some(OplogIndex::from_u64(3)),
        attachment_epoch: Some(7),
        attachment_attempt_id: Some(attachment_attempt),
        attachment_attached: Some(true),
        invocation_result: Some(OplogIndex::from_u64(10)),
        finished: Some(OplogIndex::from_u64(12)),
        tombstoned_slots: ["input".to_string()].into_iter().collect(),
        cancellation_requested: true,
        ..Default::default()
    };
    let mut index = DurableStreamSessionIndex::default();
    index.insert(source.idempotency_key.clone(), original.clone());
    let sibling_key = IdempotencyKey::new("sibling".to_string());
    let sibling = DurableStreamSessionStatus {
        session_key: Some(sibling_key.clone()),
        attachment_epoch: Some(9),
        attachment_attempt_id: Some(AttemptId::fresh()),
        attachment_attached: Some(true),
        invocation_result: Some(OplogIndex::from_u64(11)),
        tombstoned_slots: ["output".to_string()].into_iter().collect(),
        ..Default::default()
    };
    index.insert(sibling_key.clone(), sibling.clone());
    index.apply_record(OplogIndex::from_u64(21), &cut);
    let expected = DurableStreamSessionStatus {
        attachment_epoch: Some(1),
        attachment_attempt_id: Some(attachment_attempt),
        attachment_attached: Some(false),
        ..original
    };
    assert_eq!(index.get(&source.idempotency_key), Some(&expected));
    let expected_sibling = DurableStreamSessionStatus {
        attachment_epoch: Some(1),
        attachment_attempt_id: sibling.attachment_attempt_id,
        attachment_attached: Some(false),
        ..sibling
    };
    assert_eq!(index.get(&sibling_key), Some(&expected_sibling));
}

#[test]
fn durable_stream_self_revert_preserves_retained_session() {
    use crate::model::durable_stream::StreamForkCutRecord;
    use crate::model::regions::OplogRegion;

    let source = durable_stream_test_session_key("self-revert");
    let public_session_id = "retained-public";
    let mut index = DurableStreamSessionIndex::default();
    index.apply_record(
        OplogIndex::from_u64(2),
        &StreamSessionRecord::ExportForkInitialized(StreamExportForkInitializedRecord {
            format_version: 1,
            public_session_id: public_session_id.into(),
            session_key: source.idempotency_key.clone(),
            source_invocation: source.clone(),
            request_hash: vec![0; 32],
            expiry_policy: StreamSessionExpiryPolicy::Sliding { ttl_seconds: 30 },
            expiry_deadline_millis: Some(40_000),
        }),
    );
    index.apply_record(
        OplogIndex::from_u64(5),
        &StreamSessionRecord::ForkCut(StreamForkCutRecord {
            format_version: 1,
            request_hash: vec![0; 32],
            creation_fingerprint: source.callee_fingerprint,
            export: None,
            cut_index: OplogIndex::from_u64(2),
            revert: Some(OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(3),
            }),
            epoch_floor: 2,
            selected_stream_id: None,
            retained_through: None,
        }),
    );

    let status = index.get(&source.idempotency_key).unwrap();
    assert_eq!(status.public_session_id.as_deref(), Some(public_session_id));
    assert_eq!(status.expiry_deadline_millis, Some(40_000));
    assert_eq!(status.attachment_epoch, Some(2));
    assert_eq!(status.attachment_attached, Some(false));
}

#[test]
fn durable_stream_control_records_binary_roundtrip_and_validate() {
    let session_key = durable_stream_test_session_key("control-roundtrip");
    let records = [
        StreamSessionRecord::Tombstoned(StreamSlotTombstonedRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(session_key.idempotency_key.clone()),
            slot: "input".to_string(),
            role: SessionStreamRole::Input,
        }),
        StreamSessionRecord::CancelRequested(StreamSessionCancelRequestedRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(session_key.idempotency_key),
        }),
    ];

    for record in records {
        assert!(record.has_supported_format());
        let bytes = crate::serialization::serialize(&record).unwrap();
        let decoded: StreamSessionRecord = crate::serialization::deserialize(&bytes).unwrap();
        assert_eq!(decoded, record);
    }

    let invalid = StreamSessionRecord::Tombstoned(StreamSlotTombstonedRecord {
        format_version: 1,
        session_key: StreamRegistrationInvocation::Local(
            durable_stream_test_session_key("empty-slot").idempotency_key,
        ),
        slot: String::new(),
        role: SessionStreamRole::Input,
    });
    assert!(!invalid.has_supported_format());
}

#[test]
fn durable_stream_forwarding_records_roundtrip_and_validate() {
    use crate::model::StreamId;
    use crate::model::durable_stream::{
        DurableStreamHandle, LocalStreamId, LocalStreamReaderId, StreamBindingRecord,
        StreamReaderForwardDestination, StreamReaderForwardIntentRecord,
        StreamReaderForwardPublication, StreamRecordReference, StreamSessionMappingRecord,
    };
    use golem_schema::schema::SchemaFingerprintV1;

    let invocation = durable_stream_test_session_key("forward-target");
    let source = StreamRegistrationInvocation::Local(IdempotencyKey::new("source".into()));
    let mapping = StreamSessionMappingRecord {
        transport_stream_id: 29,
        role: SessionStreamRole::Input,
        handle: DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(Uuid::new_v4()),
            producer_environment_id: invocation.callee_environment_id,
            producer: invocation.callee.clone(),
            expected_producer_fingerprint: invocation.callee_fingerprint,
            producer_generation: OplogIndex::from_u64(4),
            source_invocation: invocation.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([17; 32]),
        },
    };
    let intent = StreamReaderForwardIntentRecord {
        format_version: 1,
        session_key: source.clone(),
        reader_id: LocalStreamReaderId {
            introducing_oplog_index: OplogIndex::from_u64(7),
            binding_slot: 2,
        },
        destination: StreamReaderForwardDestination::InvocationInput {
            invocation: invocation.clone(),
            mapping: mapping.clone(),
        },
    };
    let local = StreamReaderForwardIntentRecord {
        destination: StreamReaderForwardDestination::SessionBinding {
            session_key: StreamRegistrationInvocation::Remote(invocation),
            binding: StreamBindingRecord::foreign(&mapping),
            publication: StreamReaderForwardPublication::InvocationInput,
        },
        ..intent.clone()
    };
    for record in [
        StreamSessionRecord::ReaderForwardIntent(intent.clone()),
        StreamSessionRecord::ReaderForwardIntent(local.clone()),
    ] {
        assert!(record.has_supported_format());
        let bytes = crate::serialization::serialize(&record).unwrap();
        assert_eq!(
            crate::serialization::deserialize::<StreamSessionRecord>(&bytes).unwrap(),
            record
        );
    }
    for publication in [
        StreamReaderForwardPublication::InvocationResult { handle_index: 3 },
        StreamReaderForwardPublication::ProducerItem {
            parent_stream: LocalStreamId(OplogIndex::from_u64(17)),
            sequence: 41,
            handle_index: 2,
        },
    ] {
        let mut changed = local.clone();
        let StreamReaderForwardDestination::SessionBinding {
            binding,
            publication: target,
            ..
        } = &mut changed.destination
        else {
            unreachable!()
        };
        binding.role = SessionStreamRole::Output;
        *target = publication;
        let record = StreamSessionRecord::ReaderForwardIntent(changed);
        assert!(record.has_supported_format());
        let bytes = crate::serialization::serialize(&record).unwrap();
        assert_eq!(
            crate::serialization::deserialize::<StreamSessionRecord>(&bytes).unwrap(),
            record
        );
    }
    let mut invalid = Vec::new();
    for publication in [
        StreamReaderForwardPublication::InvocationResult { handle_index: 3 },
        StreamReaderForwardPublication::ProducerItem {
            parent_stream: LocalStreamId(OplogIndex::NONE),
            sequence: 41,
            handle_index: 2,
        },
    ] {
        let mut changed = local.clone();
        let StreamReaderForwardDestination::SessionBinding {
            publication: target,
            ..
        } = &mut changed.destination
        else {
            unreachable!()
        };
        *target = publication;
        invalid.push(changed);
    }
    let mut changed = intent.clone();
    changed.format_version = 0;
    invalid.push(changed);
    let mut changed = intent.clone();
    changed.reader_id.introducing_oplog_index = OplogIndex::NONE;
    invalid.push(changed);
    for mutation in 0..3 {
        let mut changed = intent.clone();
        let StreamReaderForwardDestination::InvocationInput {
            invocation,
            mapping,
        } = &mut changed.destination
        else {
            unreachable!()
        };
        match mutation {
            0 => invocation.callee_fingerprint = AgentFingerprint(Uuid::nil()),
            1 => mapping.role = SessionStreamRole::Output,
            _ => mapping.handle.format_version = 0,
        }
        invalid.push(changed);
    }
    for source in [
        StreamRecordReference::Local(LocalStreamId(OplogIndex::NONE)),
        StreamRecordReference::Foreign(DurableStreamHandle {
            format_version: 0,
            ..mapping.handle
        }),
    ] {
        let mut changed = local.clone();
        let StreamReaderForwardDestination::SessionBinding { binding, .. } =
            &mut changed.destination
        else {
            unreachable!()
        };
        binding.source = source;
        invalid.push(changed);
    }
    for record in invalid {
        assert!(!StreamSessionRecord::ReaderForwardIntent(record).has_supported_format());
    }
}

#[test]
fn durable_stream_control_status_folds_after_finished_idempotently() {
    let session_key = durable_stream_test_session_key("control-fold");
    let key = session_key.idempotency_key.clone();
    let mut index = DurableStreamSessionIndex::default();
    index.insert(
        key.clone(),
        DurableStreamSessionStatus {
            session_key: Some(session_key.idempotency_key.clone()),
            finished: Some(OplogIndex::from_u64(10)),
            ..Default::default()
        },
    );
    let tombstone = StreamSessionRecord::Tombstoned(StreamSlotTombstonedRecord {
        format_version: 1,
        session_key: StreamRegistrationInvocation::Local(session_key.idempotency_key.clone()),
        slot: "output".to_string(),
        role: SessionStreamRole::Output,
    });
    let cancel = StreamSessionRecord::CancelRequested(StreamSessionCancelRequestedRecord {
        format_version: 1,
        session_key: StreamRegistrationInvocation::Local(session_key.idempotency_key),
    });

    for (index_value, record) in [
        (11, &tombstone),
        (12, &cancel),
        (13, &tombstone),
        (14, &cancel),
    ] {
        index.apply_record(OplogIndex::from_u64(index_value), record);
    }

    let status = index.get(&key).unwrap();
    assert_eq!(status.finished, Some(OplogIndex::from_u64(10)));
    assert_eq!(status.tombstoned_slots.len(), 1);
    assert!(status.tombstoned_slots.contains("output"));
    assert!(status.cancellation_requested);
}

#[test]
fn durable_stream_remote_control_cannot_cancel_same_owner_local_session() {
    let session = durable_stream_test_session_key("same-owner");
    let mut index = DurableStreamSessionIndex::default();
    index.insert(
        session.idempotency_key.clone(),
        DurableStreamSessionStatus {
            session_key: Some(session.idempotency_key.clone()),
            prepared: Some(OplogIndex::from_u64(2)),
            ..Default::default()
        },
    );
    let cancel = |session_key| {
        StreamSessionRecord::CancelRequested(StreamSessionCancelRequestedRecord {
            format_version: 1,
            session_key,
        })
    };
    index.apply_record(
        OplogIndex::from_u64(3),
        &cancel(StreamRegistrationInvocation::Remote(session.clone())),
    );
    assert!(
        !index
            .get(&session.idempotency_key)
            .unwrap()
            .cancellation_requested
    );
    index.apply_record(
        OplogIndex::from_u64(4),
        &cancel(StreamRegistrationInvocation::Local(
            session.idempotency_key.clone(),
        )),
    );
    assert!(
        index
            .get(&session.idempotency_key)
            .unwrap()
            .cancellation_requested
    );
}

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
fn durable_stream_session_index_serialization_stays_bounded() {
    let mut index = DurableStreamSessionIndex::default();
    for n in 0..128 {
        index.insert(
            IdempotencyKey::new(format!("done-{n:04}")),
            DurableStreamSessionStatus {
                first_prepared: Some(OplogIndex::from_u64(1)),
                finished: Some(OplogIndex::from_u64(2)),
                ..Default::default()
            },
        );
    }
    let bounded_size = crate::serialization::serialize(&index).unwrap().len();

    for n in 128..1_128 {
        index.insert(
            IdempotencyKey::new(format!("done-{n:04}")),
            DurableStreamSessionStatus {
                first_prepared: Some(OplogIndex::from_u64(1)),
                finished: Some(OplogIndex::from_u64(2)),
                ..Default::default()
            },
        );
    }

    assert_eq!(
        crate::serialization::serialize(&index).unwrap().len(),
        bounded_size
    );
}

#[test]
fn durable_stream_initial_attachment_requires_exact_pending_evidence() {
    let key = IdempotencyKey::new("invocation".to_string());
    let session_key = StreamInvocationId {
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
        session_key: Some(session_key.idempotency_key.clone()),
        prepared_attempt_id: Some(attempt),
        ..Default::default()
    };
    status.apply_pending_invocation(pending, &key);
    status.apply_record(
        OplogIndex::from_u64(13),
        &StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
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
    let session_key = StreamInvocationId {
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
        session_key: Some(session_key.idempotency_key.clone()),
        prepared_attempt_id: Some(attempt),
        ..Default::default()
    };
    status.apply_record(
        OplogIndex::from_u64(13),
        &StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
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
        invalid.apply_pending_invocation(pending, &invalid.session_key.as_ref().unwrap().clone());
        let mut attached = match StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: invalid.session_key.clone().unwrap(),
            attachment_id: AttachmentId::primary(
                session_key.callee_environment_id,
                &session_key.callee,
                &session_key.idempotency_key,
            )
            .unwrap(),
            attempt_id: attempt,
            epoch: 1,
            pending_invocation_oplog_index: pending,
        }) {
            StreamSessionRecord::Attached(value) => value,
            _ => unreachable!(),
        };
        invalid.apply_record(
            OplogIndex::from_u64(13),
            &StreamSessionRecord::Attached(attached.clone()),
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
fn worker_filter_parse_agent_ids_without_quotes() {
    assert_eq!(
        AgentFilter::from_str(r#"name == CounterAgent("a b", 2)"#).unwrap(),
        AgentFilter::new_name(
            StringFilterComparator::Equal,
            r#"CounterAgent("a b", 2)"#.to_string()
        )
    );
    assert_eq!(
        AgentFilter::from_str(r#"(name == Chat("x") or status == Failed)"#).unwrap(),
        AgentFilter::new_or(vec![
            AgentFilter::new_name(StringFilterComparator::Equal, r#"Chat("x")"#.to_string()),
            AgentFilter::new_status(FilterComparator::Equal, AgentStatus::Failed),
        ])
    );
}

#[test]
fn worker_filter_parse_precedence_and_keywords() {
    let name = |v: &str| AgentFilter::new_name(StringFilterComparator::Equal, v.to_string());
    assert_eq!(
        AgentFilter::from_str("name == a or name == b AND not name == c").unwrap(),
        AgentFilter::new_or(vec![
            name("a"),
            AgentFilter::new_and(vec![name("b"), name("c").not()]),
        ])
    );
    assert_eq!(
        AgentFilter::from_str("(name == a or name == b) and name == c").unwrap(),
        AgentFilter::new_and(vec![
            AgentFilter::new_or(vec![name("a"), name("b")]),
            name("c"),
        ])
    );
    assert_eq!(AgentFilter::from_str("name == and").unwrap(), name("and"));
    assert_eq!(
        AgentFilter::from_str("TRUE or false").unwrap(),
        AgentFilter::new_or(vec![
            AgentFilter::new_and(vec![]),
            AgentFilter::new_or(vec![])
        ])
    );
}

#[test]
fn worker_filter_parse_quoted_values_and_keys() {
    assert_eq!(
        AgentFilter::from_str(r#"name startswith "Type(""#).unwrap(),
        AgentFilter::new_name(StringFilterComparator::StartsWith, "Type(".to_string())
    );
    assert_eq!(
        AgentFilter::from_str(r#"env."my key" == "a \"b\" \\ c""#).unwrap(),
        AgentFilter::new_env(
            "my key".to_string(),
            StringFilterComparator::Equal,
            r#"a "b" \ c"#.to_string()
        )
    );
    assert_eq!(
        AgentFilter::from_str("config.db.pool == 10").unwrap(),
        AgentFilter::new_config(
            "db.pool".to_string(),
            StringFilterComparator::Equal,
            "10".to_string()
        )
    );
}

#[test]
fn worker_filter_parse_brackets_in_values() {
    let name = |v: &str| AgentFilter::new_name(StringFilterComparator::Equal, v.to_string());
    for (input, value) in [
        ("name == a(())()", "a(())()"),
        (r#"name == a(")")"#, r#"a(")")"#),
        (r#"name == ")()(""#, ")()("),
        (r#"name == "(())()""#, "(())()"),
        ("((name == a))", "a"),
    ] {
        assert_eq!(
            AgentFilter::from_str(input).unwrap(),
            name(value),
            "input: {input}"
        );
    }
    for value in [")()(", "(())()", "a)(", "a(()", "(", ")", r#"a(")"#] {
        let displayed = name(value).to_string();
        assert_eq!(
            AgentFilter::from_str(&displayed).unwrap(),
            name(value),
            "displayed: {displayed}"
        );
    }
}

#[test]
fn worker_filter_parse_brackets_in_env_and_config_keys() {
    let env = |k: &str| {
        AgentFilter::new_env(
            k.to_string(),
            StringFilterComparator::Equal,
            "x".to_string(),
        )
    };
    let config = |k: &str| {
        AgentFilter::new_config(
            k.to_string(),
            StringFilterComparator::Equal,
            "x".to_string(),
        )
    };
    for (input, filter) in [
        ("env.a(b) == x", env("a(b)")),
        ("config.a(())() == x", config("a(())()")),
        (r#"env.a(")") == x"#, env(r#"a(")")"#)),
        (r#"env.")()(" == x"#, env(")()(")),
        (r#"config."(())()" == x"#, config("(())()")),
        ("env.(())() == x", env("(())()")),
        ("(env.a(b) == x)", env("a(b)")),
    ] {
        assert_eq!(
            AgentFilter::from_str(input).unwrap(),
            filter,
            "input: {input}"
        );
    }
    for key in [")()(", "(())()", "a)(", "a(()", "(", ")", r#"a(")"#, "a(b)"] {
        for filter in [env(key), config(key)] {
            let displayed = filter.to_string();
            assert_eq!(
                AgentFilter::from_str(&displayed).unwrap(),
                filter,
                "displayed: {displayed}"
            );
        }
    }
    for invalid in [
        "env.)()( == x",
        "config.a)( == x",
        "env.a(() == x",
        "config.a(b == x",
        "env.a)b == x",
        r#"env.a(") == x"#,
        r#"config.")()( == x"#,
        r#"env.")"b == x"#,
    ] {
        assert!(
            AgentFilter::from_str(invalid).is_err(),
            "expected `{invalid}` to be refused"
        );
    }
}

#[test]
fn worker_filter_parse_limits_nesting() {
    use crate::model::MAX_AGENT_FILTER_DEPTH as MAX;

    let name = AgentFilter::new_name(StringFilterComparator::Equal, "a".to_string());
    let nots = |n: usize| format!("{}name == a", "not ".repeat(n));
    let parens = |n: usize| format!("{}name == a{}", "(".repeat(n), ")".repeat(n));
    let and_chain = |depth: usize| {
        (1..depth).fold(name.clone(), |filter, _| {
            AgentFilter::new_and(vec![filter, name.clone()])
        })
    };
    let depth_error = format!("nested deeper than {MAX} levels");

    assert_eq!(
        AgentFilter::from_str(&nots(MAX - 1)).unwrap(),
        not_chain(name.clone(), MAX)
    );
    assert_eq!(AgentFilter::from_str(&parens(MAX - 1)).unwrap(), name);
    assert_eq!(
        AgentFilter::from_str(&and_chain(MAX).to_string()).unwrap(),
        and_chain(MAX)
    );
    assert!(AgentFilter::from(vec![nots(MAX - 1)]).is_ok());

    for input in [
        nots(MAX),
        parens(MAX),
        and_chain(MAX + 1).to_string(),
        nots(1_000_000),
        parens(1_000_000),
    ] {
        let error = AgentFilter::from_str(&input).unwrap_err();
        assert!(
            error.contains(&depth_error),
            "{}",
            &error[error.len() - 80..]
        );
    }
    // Several `--filter` values join under one `And`, which is a level too.
    assert!(
        AgentFilter::from(vec![nots(MAX - 1), "name == b".to_string()])
            .unwrap_err()
            .contains(&depth_error)
    );
}

#[test]
fn worker_filter_canonical_form() {
    let name = |v: &str| AgentFilter::new_name(StringFilterComparator::Equal, v.to_string());

    assert_eq!(
        AgentFilter::new_or(vec![AgentFilter::new_and(vec![AgentFilter::new_not(
            AgentFilter::new_or(vec![name("a")])
        ),])])
        .canonical(),
        name("a").not()
    );
    assert_eq!(
        AgentFilter::new_and(vec![name("a"), AgentFilter::new_or(vec![name("b")])]).canonical(),
        AgentFilter::new_and(vec![name("a"), name("b")])
    );
    assert_eq!(
        AgentFilter::new_and(vec![]).canonical(),
        AgentFilter::new_and(vec![])
    );

    // The canonical form is what counts against the depth limit, and the text of a filter has
    // no parentheses for a one-child `And` or `Or`, so such a filter parses back from its text.
    use crate::model::MAX_AGENT_FILTER_DEPTH as MAX;
    let wrapped = AgentFilter::new_or(vec![AgentFilter::new_and(vec![not_chain(name("a"), MAX)])]);
    assert_eq!(wrapped.depth(), MAX + 2);
    assert_eq!(
        wrapped.clone().validated().unwrap(),
        not_chain(name("a"), MAX)
    );
    assert_eq!(
        AgentFilter::from_str(&wrapped.to_string()).unwrap(),
        wrapped.canonical()
    );
    assert!(
        AgentFilter::new_and(vec![not_chain(name("a"), MAX + 1)])
            .validated()
            .is_err()
    );

    // One `--filter` is its filter, not an `And` around it; none is the empty `And`.
    assert_eq!(
        AgentFilter::from(vec!["name == a".to_string()]).unwrap(),
        name("a")
    );
    assert_eq!(
        AgentFilter::from(vec![]).unwrap(),
        AgentFilter::new_and(vec![])
    );
}

#[test]
fn worker_filter_parse_rejects_malformed_filters() {
    for invalid in [
        "name startswith Type(",
        r#"name == "abc"#,
        "(name == a",
        "name == a)",
        ")()(",
        "(())()",
        "()",
        "((name == a)",
        "(name == a))",
        ")name == a(",
        "name == )()(",
        "name == (())()",
        "name == a)(",
        "name == a(()",
        "name == a())",
        r#"name == a(")"#,
        "name ==",
        "name == a name == b",
        "env. == a",
        "unknown == a",
        "not",
        "",
    ] {
        assert!(
            AgentFilter::from_str(invalid).is_err(),
            "expected `{invalid}` to be refused"
        );
    }
}

fn agent_filter_strategy() -> impl proptest::strategy::Strategy<Value = AgentFilter> {
    use proptest::prelude::*;

    // Text that hits every quoting rule: whitespace, parentheses, quotes, backslashes,
    // keywords, empty text and agent-ID shapes.
    let text = prop_oneof![
        r#"[a-zA-Z0-9 ()"\\.,_\-]{0,10}"#,
        Just("and".to_string()),
        Just("NOT".to_string()),
        Just(String::new()),
        "[a-z]{1,6}".prop_map(|t| format!(r#"Agent("{t} x", 2)"#)),
    ];
    let comparator = prop_oneof![
        Just(FilterComparator::Equal),
        Just(FilterComparator::NotEqual),
        Just(FilterComparator::GreaterEqual),
        Just(FilterComparator::Greater),
        Just(FilterComparator::LessEqual),
        Just(FilterComparator::Less),
    ];
    let string_comparator = prop_oneof![
        Just(StringFilterComparator::Equal),
        Just(StringFilterComparator::NotEqual),
        Just(StringFilterComparator::Like),
        Just(StringFilterComparator::NotLike),
        Just(StringFilterComparator::StartsWith),
    ];
    let status = prop_oneof![
        Just(AgentStatus::Running),
        Just(AgentStatus::Idle),
        Just(AgentStatus::Suspended),
        Just(AgentStatus::Interrupted),
        Just(AgentStatus::Retrying),
        Just(AgentStatus::Failed),
        Just(AgentStatus::Exited),
    ];
    let mode = prop_oneof![Just(AgentMode::Durable), Just(AgentMode::Ephemeral)];

    let leaf = prop_oneof![
        (string_comparator.clone(), text.clone()).prop_map(|(c, v)| AgentFilter::new_name(c, v)),
        (comparator.clone(), status).prop_map(|(c, v)| AgentFilter::new_status(c, v)),
        (comparator.clone(), 0u64..1_000_000).prop_map(|(c, v)| {
            AgentFilter::new_revision(c, ComponentRevision::new(v).unwrap())
        }),
        (comparator.clone(), mode).prop_map(|(c, v)| AgentFilter::new_mode(c, v)),
        (comparator, 0u64..4_000_000_000_000)
            .prop_map(|(c, millis)| AgentFilter::new_created_at(c, Timestamp::from(millis))),
        (text.clone(), string_comparator.clone(), text.clone())
            .prop_map(|(k, c, v)| AgentFilter::new_env(k, c, v)),
        (text.clone(), string_comparator, text)
            .prop_map(|(k, c, v)| AgentFilter::new_config(k, c, v)),
    ];

    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(AgentFilter::new_and),
            proptest::collection::vec(inner.clone(), 0..4).prop_map(AgentFilter::new_or),
            inner.prop_map(|f| f.not()),
        ]
    })
}

mod worker_filter_syntax {
    use super::*;
    use proptest::prelude::*;
    use test_r::test;

    proptest! {
        #[test]
        fn display_parses_back_to_the_same_filter(filter in agent_filter_strategy()) {
            let displayed = filter.to_string();
            let parsed = AgentFilter::from_str(&displayed)
                .map_err(|e| TestCaseError::fail(format!("{displayed}: {e}")))?;
            prop_assert_eq!(parsed, filter.canonical(), "displayed: {}", displayed);
        }
    }
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
        owner_kind: crate::model::agent::OwnerKind::ComponentAgent,
        component_id,
        agent_type_name: None,
        canonical_key: None,
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
        create_request: crate::model::oplog::CreateRequest::new(
            crate::model::oplog::CreateRequestKind::GetOrCreate,
            None,
            crate::model::agent::Principal::anonymous(),
            &crate::model::invocation_context::InvocationContextStack::fresh_rounded(),
        ),
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

    // `true` is the empty `and` and `false` the empty `or`, and both mean what they say.
    for (text, expected) in [
        ("true", true),
        ("false", false),
        ("not true", false),
        ("not false", true),
        ("name == worker-1 and true", true),
        ("name == worker-1 and false", false),
        ("name == worker-2 or true", true),
        ("name == worker-2 or false", false),
        ("name == worker-1 or false", true),
        ("status == Idle and not false", true),
    ] {
        assert_eq!(
            AgentFilter::from_str(text)
                .unwrap()
                .matches(&worker_metadata),
            expected,
            "{text}"
        );
    }
    assert!(!AgentFilter::new_or(vec![]).matches(&worker_metadata));
}

#[test]
fn agent_status_record_has_pending_work_for_pending_invocations() {
    let mut status = AgentStatusRecord::default();
    status.pending_invocations.push(PendingInvocationRef {
        timestamp: Timestamp::now_utc(),
        not_before: None,
        oplog_index: OplogIndex::INITIAL,
        idempotency_key: None,
        manual_update_target_revision: Some(ComponentRevision::INITIAL),
    });

    assert!(status.has_pending_work(Timestamp::now_utc()));
}

#[test]
fn scheduled_invocations_become_pending_work_only_when_due() {
    let deadline = Timestamp::from(2000);
    let status = AgentStatusRecord {
        pending_invocations: vec![PendingInvocationRef {
            timestamp: Timestamp::from(1000),
            not_before: Some(deadline),
            oplog_index: OplogIndex::INITIAL,
            idempotency_key: Some(IdempotencyKey::fresh()),
            manual_update_target_revision: None,
        }],
        ..Default::default()
    };

    assert!(!status.has_pending_work(Timestamp::from(1999)));
    assert!(status.has_pending_work(deadline));
    assert!(status.has_pending_work(Timestamp::from(2001)));
}

#[test]
fn agent_status_record_has_pending_work_for_pending_updates() {
    let mut status = AgentStatusRecord::default();
    status.pending_updates.push_back(PendingUpdateRef {
        timestamp: Timestamp::now_utc(),
        oplog_index: OplogIndex::INITIAL,
        admission_index: OplogIndex::INITIAL,
        target_revision: ComponentRevision::INITIAL,
        kind: PendingUpdateKind::Automatic,
    });

    assert!(status.has_pending_work(Timestamp::now_utc()));
}

#[test]
fn agent_status_record_has_pending_work_is_false_without_pending_queues() {
    assert!(!AgentStatusRecord::default().has_pending_work(Timestamp::now_utc()));
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
        last_automatic_snapshot: Some(crate::model::AutomaticSnapshot {
            index: crate::model::oplog::OplogIndex::from_u64(9),
            timestamp: crate::model::Timestamp::from(1_000),
            component_revision: ComponentRevision::new(7).unwrap(),
            files: crate::model::SnapshotFiles::Confirmed(
                crate::model::oplog::FilesystemSnapshotName::periodic(),
            ),
        }),
        previous_usable_automatic_snapshot: Some(crate::model::UsableAutomaticSnapshot {
            index: crate::model::oplog::OplogIndex::from_u64(5),
            component_revision: ComponentRevision::new(7).unwrap(),
            filesystem_snapshot: Some(crate::model::oplog::FilesystemSnapshotName::periodic()),
        }),
        export_fork_admissions: ExportForkAdmissions {
            owner_fingerprint: Some(AgentFingerprint(Uuid::new_v4())),
            reservations: HashMap::new(),
            session_counts: HashMap::from([("session".to_string(), 3)]),
            updated_millis: 1200,
            credit_millis: Some(900),
        },
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
        expires_at: None,
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
fn external_tool_results_round_trip_and_treat_nan_as_replay_equivalent() {
    use crate::model::AgentInvocationResult;
    use crate::model::oplog::payload::types::{
        SerializableToolError, SerializableToolInvocationResult, SerializableToolRpcError,
    };
    use crate::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
    use crate::serialization::{deserialize, serialize};

    let success = AgentInvocationResult::ExternalTool {
        result: Ok(SerializableToolInvocationResult {
            result: Some(Box::new(TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::f64()),
                SchemaValue::F64(f64::NAN),
            ))),
        }),
    };
    let decoded_success: AgentInvocationResult =
        deserialize(&serialize(&success).unwrap()).unwrap();
    assert!(success.replay_equivalent(&decoded_success));

    let failure = AgentInvocationResult::ExternalTool {
        result: Err(SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::InvalidCommandPath(vec![
                "admin".to_string(),
                "rotate".to_string(),
            ]),
        ))),
    };
    let decoded_failure: AgentInvocationResult =
        deserialize(&serialize(&failure).unwrap()).unwrap();
    assert_eq!(failure, decoded_failure);
}

#[test]
fn external_tool_custom_error_nan_is_replay_equivalent_after_round_trip() {
    use crate::base_model::tool::{
        SerializableCustomToolError, SerializableToolError, SerializableToolRpcError,
    };
    use crate::model::AgentInvocationResult;
    use crate::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
    use crate::serialization::{deserialize, serialize};

    let result = AgentInvocationResult::ExternalTool {
        result: Err(SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "nan".to_string(),
                payload: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::f64()),
                    SchemaValue::F64(f64::NAN),
                ),
            })),
        ))),
    };
    let decoded: AgentInvocationResult = deserialize(&serialize(&result).unwrap()).unwrap();

    assert!(result.replay_equivalent(&decoded));
}

#[test]
fn durable_stream_session_index_rejects_unloaded_external_payload() {
    use crate::model::oplog::{OplogEntry, OplogPayload, PayloadId};

    let entry = OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        summary: None,
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
        summary: None,
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
        summary: None,
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

#[test]
fn an_automatic_snapshot_is_usable_when_confirmed_or_without_a_name() {
    use crate::model::SnapshotFiles;
    let name = crate::model::oplog::FilesystemSnapshotName::periodic();
    let usable = |files: SnapshotFiles| {
        crate::model::AutomaticSnapshot {
            index: crate::model::oplog::OplogIndex::from_u64(10),
            timestamp: crate::model::Timestamp::from(1_000),
            component_revision: ComponentRevision::new(2).unwrap(),
            files,
        }
        .into_usable()
        .map(|usable| usable.filesystem_snapshot)
    };

    assert_eq!(
        [
            usable(SnapshotFiles::Unnamed),
            usable(SnapshotFiles::Confirmed(name.clone())),
            usable(SnapshotFiles::Unconfirmed(name.clone())),
        ],
        [Some(None), Some(Some(name)), None]
    );
}

#[test]
fn the_files_of_an_automatic_snapshot_give_their_name() {
    use crate::model::SnapshotFiles;
    let name = crate::model::oplog::FilesystemSnapshotName::periodic();
    let files = [
        SnapshotFiles::Unnamed,
        SnapshotFiles::Unconfirmed(name.clone()),
        SnapshotFiles::Confirmed(name.clone()),
    ];

    assert_eq!(
        files.each_ref().map(|files| files.name().cloned()),
        [None, Some(name.clone()), Some(name)]
    );
}

#[test]
fn only_the_named_filesystem_snapshot_of_an_automatic_snapshot_is_confirmed() {
    use crate::model::SnapshotFiles;
    let name = crate::model::oplog::FilesystemSnapshotName::periodic();
    let other = crate::model::oplog::FilesystemSnapshotName::periodic();
    let confirmed = |files: SnapshotFiles, by: &crate::model::oplog::FilesystemSnapshotName| {
        files.confirmed(by)
    };

    assert_eq!(
        [
            confirmed(SnapshotFiles::named(Some(name.clone())), &name),
            confirmed(SnapshotFiles::named(Some(name.clone())), &other),
            confirmed(SnapshotFiles::named(None), &name),
        ],
        [
            SnapshotFiles::Confirmed(name.clone()),
            SnapshotFiles::Unconfirmed(name),
            SnapshotFiles::Unnamed,
        ]
    );
}

#[test]
fn a_pending_update_kind_keeps_the_assisted_selection_behind_one_box() {
    assert_eq!(std::mem::size_of::<crate::model::PendingUpdateKind>(), 24);
}

#[test]
fn pod_uri_and_display_bracket_ipv6() {
    use crate::model::Pod;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let v4 = Pod {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        port: 9000,
    };
    assert_eq!(v4.to_string(), "10.0.0.1:9000");
    assert_eq!(v4.uri(false).to_string(), "http://10.0.0.1:9000/");

    let v6 = Pod {
        ip: IpAddr::V6("fdaa:0:1:a7b::2".parse::<Ipv6Addr>().unwrap()),
        port: 9000,
    };
    assert_eq!(v6.to_string(), "[fdaa:0:1:a7b::2]:9000");
    assert_eq!(v6.uri(true).to_string(), "https://[fdaa:0:1:a7b::2]:9000/");
}

/// A chain of `not` above `leaf`, `depth` levels deep in total.
fn not_chain(leaf: AgentFilter, depth: usize) -> AgentFilter {
    (1..depth).fold(leaf, |filter, _| filter.not())
}

#[test]
fn agent_filter_depth_limit_matches_grpc() {
    use crate::model::MAX_AGENT_FILTER_DEPTH;
    use golem_api_grpc::proto::golem::workerexecutor::v1::GetWorkersMetadataRequest;
    use prost::Message;

    // A `created_at` leaf takes the most protobuf messages of any leaf.
    let leaf = AgentFilter::new_created_at(FilterComparator::Equal, Timestamp::from(1u64));
    let decodes = |filter: &AgentFilter| {
        let request = GetWorkersMetadataRequest {
            filter: Some(filter.clone().into()),
            ..Default::default()
        };
        GetWorkersMetadataRequest::decode(request.encode_to_vec().as_slice()).is_ok()
    };

    let deepest = not_chain(leaf.clone(), MAX_AGENT_FILTER_DEPTH);
    assert_eq!(deepest.depth(), MAX_AGENT_FILTER_DEPTH);
    assert!(decodes(&deepest));
    assert!(deepest.validate_depth().is_ok());

    let too_deep = not_chain(leaf, MAX_AGENT_FILTER_DEPTH + 1);
    assert!(!decodes(&too_deep));
    assert!(too_deep.validate_depth().is_err());
}
