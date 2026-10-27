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
use crate::model::ExecutionStatus;
use crate::services::oplog::tests::{ReadCountingBlobStorage, ReadCountingIndexedStorage};
use crate::services::oplog::{CommitLevel, PrimaryOplogService};
use arc_swap::ArcSwap;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    AttachmentId, AttemptId, PersistedInvocationTarget, PersistedStreamInvocationDescriptor,
    StartAttemptDescriptor, StreamForkCutRecord, StreamItemsPayload, StreamOffset,
    StreamRegistrationRecordCoordinate, StreamRootKind, StreamSessionExpiryPolicy,
    StreamSessionFinishedRecord, StreamSessionKey, StreamSessionPreparedRecord, StreamSourceKind,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{
    AgentId, AgentMetadata, AgentStatusRecord, IdempotencyKey, RetryConfig, Timestamp,
    agent::OwnerKind,
};
use golem_common::read_only_lock;
use golem_schema::schema::SchemaFingerprintV1;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use test_r::test;

struct ServiceFixture {
    service: Arc<dyn OplogService>,
    oplog: Arc<dyn Oplog>,
    blobs: Arc<ReadCountingBlobStorage>,
}

impl ServiceFixture {
    async fn new(owner: &OwnedAgentId, fingerprint: AgentFingerprint) -> Self {
        let indexed = Arc::new(ReadCountingIndexedStorage::new());
        let blobs = Arc::new(ReadCountingBlobStorage::new());
        let service: Arc<dyn OplogService> = Arc::new(
            PrimaryOplogService::new(indexed, blobs.clone(), 1, 1, 1, 32, RetryConfig::default())
                .await,
        );
        let account = AccountId::new();
        let metadata = AgentMetadata {
            agent_id: owner.agent_id.clone(),
            owner_kind: OwnerKind::ComponentAgent,
            env: vec![],
            environment_id: owner.environment_id,
            created_by: account,
            created_by_email: AccountEmail::new("lineage@test"),
            config: vec![],
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: AgentStatusRecord::default(),
            original_phantom_id: None,
            fingerprint,
            agent_mode: AgentMode::Durable,
        };
        let create = OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: owner.agent_id.clone(),
            owner_kind: OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Durable,
            component_revision: ComponentRevision::INITIAL,
            env: vec![],
            environment_id: owner.environment_id,
            created_by: account,
            parent: None,
            component_size: 100,
            initial_total_linear_memory_size: 100,
            initial_active_plugins: HashSet::new(),
            local_agent_config: vec![],
            original_phantom_id: None,
            instance_id: fingerprint.0,
        }));
        let oplog = service
            .create_fresh(
                &mut service.lock_lifecycle(&owner.agent_id).await,
                owner,
                AgentMode::Durable,
                create,
                metadata,
                read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(ArcSwap::from_pointee(
                    AgentStatusRecord::default(),
                ))),
                read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                    ExecutionStatus::Suspended {
                        agent_mode: AgentMode::Durable,
                        timestamp: Timestamp::now_utc(),
                    },
                ))),
                None,
            )
            .await
            .unwrap();
        Self {
            service,
            oplog,
            blobs,
        }
    }
}

fn owner(name: &str) -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId(uuid::Uuid::from_u128(11)),
        &AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::from_u128(23)),
            agent_id: name.to_string(),
        },
    )
}

fn registration() -> StreamRegisteredRecord {
    let invocation = StreamRegistrationInvocation::Local(IdempotencyKey::new("session-1".into()));
    StreamRegisteredRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        coordinate: StreamRegistrationRecordCoordinate::Root {
            invocation: invocation.clone(),
            root_kind: StreamRootKind::MethodInput,
            recursive_value_path: vec![],
        },
        source_invocation: invocation,
        component_revision: ComponentRevision::new(3).unwrap(),
        element_schema_fingerprint: SchemaFingerprintV1([47; 32]),
        source_kind: StreamSourceKind::ExternalInlineInput,
        session_role: None,
    }
}

pub(crate) fn prepared(key: &StreamSessionKey) -> StreamSessionPreparedRecord {
    StreamSessionPreparedRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: key.idempotency_key.clone(),
        public_session_id: key.idempotency_key.value.clone(),
        expiry_policy: StreamSessionExpiryPolicy::None,
        expiry_deadline_millis: None,
        attempt: StartAttemptDescriptor {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: key.clone(),
            attachment_id: AttachmentId::primary(
                key.callee_environment_id,
                &key.callee,
                &key.idempotency_key,
            )
            .unwrap(),
            expected_callee_fingerprint: key.callee_fingerprint,
            attempt_id: AttemptId::fresh(),
            invocation: PersistedStreamInvocationDescriptor {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: key.clone(),
                target_component_revision: ComponentRevision::new(3).unwrap(),
                target: PersistedInvocationTarget::AgentMethod {
                    method_name: "output".into(),
                },
                invocation_value: vec![],
                stream_handles: vec![],
                execution_config: vec![],
                effective_identity: vec![],
            },
            effective_identity: vec![],
            live_join_buffer_events: 32,
        },
        stream_mappings: vec![],
    }
}

pub(crate) fn fork(
    selected_stream_id: Option<LocalStreamId>,
    cut_index: OplogIndex,
) -> StreamForkCutRecord {
    StreamForkCutRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        request_hash: vec![0; 32],
        creation_fingerprint: AgentFingerprint(uuid::Uuid::new_v4()),
        export: None,
        cut_index,
        revert: None,
        epoch_floor: 1,
        selected_stream_id,
        retained_through: None,
    }
}

fn self_revert(end: u64) -> StreamForkCutRecord {
    let mut cut = fork(None, OplogIndex::INITIAL);
    cut.revert = Some(OplogRegion::from_range(2..=end));
    cut.epoch_floor = 2;
    cut
}

fn registration_key(
    registration: &StreamRegisteredRecord,
    owner: &OwnedAgentId,
    fingerprint: AgentFingerprint,
) -> StreamSessionKey {
    registration
        .source_invocation
        .qualify(owner.environment_id, &owner.agent_id, fingerprint)
}

#[test]
async fn revert_marker_requires_adjacent_matching_revert() {
    let identity = owner("source");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
    for matching in [None, Some(false), Some(true)] {
        let fixture = ServiceFixture::new(&identity, fingerprint).await;
        for _ in 2..=3 {
            fixture.oplog.add(OplogEntry::grow_memory(0)).await.unwrap();
        }
        if let Some(matching) = matching {
            fixture
                .oplog
                .add(OplogEntry::revert(OplogRegion::from_range(
                    2..=if matching { 3 } else { 2 },
                )))
                .await
                .unwrap();
        } else {
            fixture.oplog.add(OplogEntry::grow_memory(0)).await.unwrap();
        }
        let marker = StreamSessionRecord::ForkCut(self_revert(3));
        fixture
            .oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture.oplog.upload_payload(&marker).await.unwrap(),
            })
            .await
            .unwrap();
        let result = StreamForkLineage::load(&*fixture.oplog, &identity, fingerprint).await;
        assert_eq!(result.is_ok(), matching == Some(true));
    }
}

#[test]
async fn bare_revert_and_guest_jump_remain_valid_lineage_history() {
    let identity = owner("source");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
    let fixture = ServiceFixture::new(&identity, fingerprint).await;
    fixture
        .oplog
        .add(OplogEntry::jump(None, OplogRegion::from_range(2..=2)))
        .await
        .unwrap();
    fixture
        .oplog
        .add(OplogEntry::revert(OplogRegion::from_range(2..=2)))
        .await
        .unwrap();
    let lineage = StreamForkLineage::load(&*fixture.oplog, &identity, fingerprint)
        .await
        .unwrap();
    assert!(
        lineage
            .deleted_regions()
            .is_in_deleted_region(OplogIndex::from_u64(2))
    );
    assert!(lineage.cuts().is_empty());
}

#[test]
fn through_does_not_expose_future_deletions() {
    let lineage = StreamForkLineage {
        revert_regions: vec![(OplogIndex::from_u64(8), OplogRegion::from_range(2..=5))],
        deleted_regions: DeletedRegions::from_regions([OplogRegion::from_range(2..=5)]),
        ..Default::default()
    };
    assert!(
        !lineage
            .through(OplogIndex::INITIAL)
            .unwrap()
            .deleted_regions()
            .is_in_deleted_region(OplogIndex::from_u64(3))
    );
    assert!(lineage.through(OplogIndex::from_u64(7)).is_err());
    assert!(
        lineage
            .through(OplogIndex::from_u64(8))
            .unwrap()
            .deleted_regions()
            .is_in_deleted_region(OplogIndex::from_u64(3))
    );
}

#[test]
fn through_merges_overlapping_revert_regions() {
    let lineage = StreamForkLineage {
        revert_regions: vec![
            (OplogIndex::from_u64(21), OplogRegion::from_range(3..=20)),
            (OplogIndex::from_u64(22), OplogRegion::from_range(4..=9)),
        ],
        ..Default::default()
    };
    assert_eq!(
        lineage
            .through(OplogIndex::from_u64(22))
            .unwrap()
            .deleted_regions()
            .regions()
            .cloned()
            .collect::<Vec<_>>(),
        vec![OplogRegion::from_range(3..=20)]
    );
}

#[test]
async fn lineage_skips_deleted_stream_payloads_without_downloading_them() {
    let identity = owner("source");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
    let fixture = ServiceFixture::new(&identity, fingerprint).await;
    let registration = registration();
    let key = registration_key(&registration, &identity, fingerprint);
    for index in 2..=9 {
        let entry = match index {
            5 => OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture.oplog.upload_payload(&registration).await.unwrap(),
            },
            6 => OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture
                    .oplog
                    .upload_payload(&StreamSessionRecord::Prepared(prepared(&key)))
                    .await
                    .unwrap(),
            },
            7 => {
                let mut discarded = registration.clone();
                discarded.format_version = 0;
                OplogEntry::StreamRegistered {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    summary: None,
                    record: fixture.oplog.upload_payload(&discarded).await.unwrap(),
                }
            }
            8 => {
                let mut discarded = prepared(&key);
                discarded.format_version = 0;
                OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    summary: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::Prepared(discarded))
                        .await
                        .unwrap(),
                }
            }
            9 => {
                let mut discarded = self_revert(7);
                discarded.format_version = 0;
                OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    summary: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::ForkCut(discarded))
                        .await
                        .unwrap(),
                }
            }
            _ => OplogEntry::grow_memory(0),
        };
        fixture.oplog.add(entry).await.unwrap();
    }
    let region = OplogRegion::from_range(7..=9);
    fixture
        .oplog
        .add(OplogEntry::revert(region.clone()))
        .await
        .unwrap();
    let mut marker = fork(None, OplogIndex::from_u64(6));
    marker.revert = Some(region);
    marker.epoch_floor = 2;
    fixture
        .oplog
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            summary: None,
            record: fixture
                .oplog
                .upload_payload(&StreamSessionRecord::ForkCut(marker))
                .await
                .unwrap(),
        })
        .await
        .unwrap();
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    fixture.blobs.reset();

    let lineage = StreamForkLineage::load_from_service(
        &*fixture.service,
        &identity,
        AgentMode::Durable,
        fingerprint,
        OplogIndex::from_u64(11),
    )
    .await
    .unwrap();
    assert_eq!(lineage.cuts().len(), 1);
    assert_eq!(fixture.blobs.reads(), 3);
    assert!(
        StreamForkLineage::load_at_horizon(
            &*fixture.oplog,
            &identity,
            fingerprint,
            OplogIndex::from_u64(10),
        )
        .await
        .is_err()
    );
}

#[test]
async fn lineage_loader_finds_marker_after_chunk_boundary_without_a_cache() {
    use crate::durable_host::durable_stream::tests::TestOplog;
    let registration = registration();
    let target = owner("target");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
    let key = registration_key(&registration, &target, fingerprint);
    let cut = fork(
        Some(LocalStreamId(OplogIndex::from_u64(5))),
        OplogIndex::from_u64(1024),
    );
    let oplog = TestOplog::default();
    for index in 1..=1025 {
        let entry = match index {
            5 => OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: oplog.upload_payload(&registration).await.unwrap(),
            },
            6 => OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: oplog
                    .upload_payload(&StreamSessionRecord::Prepared(prepared(&key)))
                    .await
                    .unwrap(),
            },
            1025 => OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: oplog
                    .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                    .await
                    .unwrap(),
            },
            _ => OplogEntry::grow_memory(0),
        };
        oplog.add(entry).await.unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            StreamForkLineage::load(&oplog, &target, fingerprint)
                .await
                .unwrap()
                .cuts(),
            &[(OplogIndex::from_u64(1025), cut.clone())]
        );
        assert_eq!(
            oplog.take_read_ranges(),
            vec![
                (OplogIndex::INITIAL, 1024),
                (OplogIndex::from_u64(1025), 1),
                (OplogIndex::INITIAL, 1024),
                (OplogIndex::from_u64(1025), 1),
            ]
        );
    }
}

#[test]
async fn lineage_completion_validation_observes_the_marker_boundary() {
    use crate::durable_host::durable_stream::tests::TestOplog;
    for (selected, finished_before) in [(true, true), (false, true), (true, false)] {
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
        let key = registration_key(&registration, &target, fingerprint);
        let mut cut = fork(
            Some(LocalStreamId(OplogIndex::from_u64(5))),
            OplogIndex::from_u64(7),
        );
        if !selected {
            cut.selected_stream_id = None;
        }
        let oplog = TestOplog::default();
        for index in 1..=9 {
            let session = match index {
                6 => Some(StreamSessionRecord::Prepared(prepared(&key))),
                7 if finished_before => {
                    Some(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                        format_version: 1,
                        session_key: registration.source_invocation.clone(),
                        result: Ok(()),
                    }))
                }
                8 => Some(StreamSessionRecord::ForkCut(cut.clone())),
                9 if !finished_before => {
                    Some(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                        format_version: 1,
                        session_key: registration.source_invocation.clone(),
                        result: Ok(()),
                    }))
                }
                _ => None,
            };
            let entry = if index == 5 {
                OplogEntry::StreamRegistered {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    summary: None,
                    record: oplog.upload_payload(&registration).await.unwrap(),
                }
            } else if let Some(session) = session {
                OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    summary: None,
                    record: oplog.upload_payload(&session).await.unwrap(),
                }
            } else {
                OplogEntry::grow_memory(0)
            };
            oplog.add(entry).await.unwrap();
        }
        assert_eq!(
            StreamForkLineage::load(&oplog, &target, fingerprint)
                .await
                .is_ok(),
            !selected || !finished_before
        );
    }
}

#[test]
async fn service_lineage_loader_matches_open_oplog_across_chunk_boundary() {
    let registration = registration();
    let target = owner("target");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
    let key = registration_key(&registration, &target, fingerprint);
    let cut = fork(
        Some(LocalStreamId(OplogIndex::from_u64(5))),
        OplogIndex::from_u64(1024),
    );
    let fixture = ServiceFixture::new(&target, fingerprint).await;
    for index in 2..=1025 {
        let entry = match index {
            5 => OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture.oplog.upload_payload(&registration).await.unwrap(),
            },
            6 => OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture
                    .oplog
                    .upload_payload(&StreamSessionRecord::Prepared(prepared(&key)))
                    .await
                    .unwrap(),
            },
            1025 => OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture
                    .oplog
                    .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                    .await
                    .unwrap(),
            },
            _ => OplogEntry::grow_memory(0),
        };
        assert_eq!(
            fixture.oplog.add(entry).await.unwrap(),
            OplogIndex::from_u64(index)
        );
    }
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    let from_oplog = StreamForkLineage::load(&*fixture.oplog, &target, fingerprint)
        .await
        .unwrap();
    fixture.blobs.reset();
    let from_service = StreamForkLineage::load_from_service(
        &*fixture.service,
        &target,
        AgentMode::Durable,
        fingerprint,
        OplogIndex::from_u64(1025),
    )
    .await
    .unwrap();
    assert_eq!(from_service, from_oplog);
    assert!(fixture.blobs.reads() >= 3);
    assert!(
        StreamForkLineage::load_from_service(
            &*fixture.service,
            &target,
            AgentMode::Durable,
            fingerprint,
            OplogIndex::from_u64(1024),
        )
        .await
        .unwrap()
        .cuts()
        .is_empty()
    );
}

#[test]
async fn suffix_discovery_is_fixed_to_its_horizon_and_finds_a_later_marker() {
    let target = owner("target");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
    let fixture = ServiceFixture::new(&target, fingerprint).await;
    for index in 2..=1024 {
        assert_eq!(
            fixture.oplog.add(OplogEntry::grow_memory(0)).await.unwrap(),
            OplogIndex::from_u64(index)
        );
    }
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    assert!(
        !StreamForkLineage::suffix_contains_fork_cut(
            &*fixture.service,
            &target,
            AgentMode::Durable,
            OplogIndex::from_u64(2),
            OplogIndex::from_u64(1024),
        )
        .await
        .unwrap()
    );
    let marker = StreamSessionRecord::ForkCut(fork(None, OplogIndex::from_u64(1024)));
    assert_eq!(
        fixture
            .oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                summary: None,
                record: fixture.oplog.upload_payload(&marker).await.unwrap(),
            })
            .await
            .unwrap(),
        OplogIndex::from_u64(1025)
    );
    fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    assert!(
        StreamForkLineage::suffix_contains_fork_cut(
            &*fixture.service,
            &target,
            AgentMode::Durable,
            OplogIndex::from_u64(1025),
            OplogIndex::from_u64(1025),
        )
        .await
        .unwrap()
    );
    assert!(
        !StreamForkLineage::suffix_contains_fork_cut(
            &*fixture.service,
            &target,
            AgentMode::Durable,
            OplogIndex::from_u64(2),
            OplogIndex::from_u64(1024),
        )
        .await
        .unwrap()
    );
}

#[test]
#[should_panic(expected = "missing oplog entries in range [2..=2]")]
async fn service_lineage_loader_rejects_a_missing_requested_range() {
    let source = owner("source");
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
    let fixture = ServiceFixture::new(&source, fingerprint).await;
    StreamForkLineage::load_from_service(
        &*fixture.service,
        &source,
        AgentMode::Durable,
        fingerprint,
        OplogIndex::from_u64(2),
    )
    .await
    .unwrap();
}

#[test]
fn project_item_batch_clips_only_the_selected_local_stream() {
    let local = LocalStreamId(OplogIndex::from_u64(5));
    let mut cut = fork(Some(local), OplogIndex::from_u64(9));
    cut.retained_through = Some(StreamOffset::new(OplogIndex::from_u64(7), 1));
    let lineage = StreamForkLineage {
        cuts: vec![(OplogIndex::from_u64(10), cut)],
        ..Default::default()
    };
    let mut batch = StreamItemsRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        stream_id: local,
        first_sequence: 3,
        nested_stream_ids: vec![StreamRecordReference::Local(local)],
        newly_registered_stream_ids: vec![],
        payload: StreamItemsPayload::PackedU8(vec![1, 2, 3]),
        offsets: (0..3)
            .map(|sub| StreamOffset::new(OplogIndex::from_u64(7), sub))
            .collect(),
    };
    lineage
        .project_item_batch(OplogIndex::from_u64(7), &mut batch)
        .unwrap();
    assert_eq!(batch.payload, StreamItemsPayload::PackedU8(vec![1, 2]));
    assert_eq!(
        batch.nested_stream_ids,
        vec![StreamRecordReference::Local(local)]
    );
}

#[test]
fn through_preserves_marker_and_deletion_boundaries() {
    let deleted = OplogRegion::from_range(3..=5);
    let lineage = StreamForkLineage {
        cuts: vec![(
            OplogIndex::from_u64(10),
            fork(None, OplogIndex::from_u64(9)),
        )],
        revert_regions: vec![(OplogIndex::from_u64(6), deleted.clone())],
        deleted_regions: DeletedRegions::from_regions([deleted]),
    };
    assert!(lineage.through(OplogIndex::from_u64(4)).is_err());
    assert!(
        lineage
            .through(OplogIndex::from_u64(9))
            .unwrap()
            .cuts()
            .is_empty()
    );
    assert_eq!(
        lineage
            .through(OplogIndex::from_u64(10))
            .unwrap()
            .cuts()
            .len(),
        1
    );
}
