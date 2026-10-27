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
use crate::durable_host::durable_stream::registration::registered_stream;
use crate::durable_host::durable_stream::tests::{TestIdentity, identity, registration};
use crate::model::ExecutionStatus;
use crate::services::golem_config::GolemConfig;
use crate::services::oplog::tests::{ReadCountingBlobStorage, ReadCountingIndexedStorage};
use crate::services::oplog::{
    CompressedOplogArchiveService, MultiLayerOplog, MultiLayerOplogService, OplogService,
    PrimaryOplogService,
};
use crate::services::shard::ShardServiceDefault;
use crate::services::worker::DefaultWorkerService;
use crate::services::worker::session_index_tests::UnusedComponentService;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::durable_stream::{
    StreamRegistrationInvocation, StreamRootKind, StreamSessionMappingUpdateRecord,
};
use golem_common::model::{AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp};
use golem_common::read_only_lock;
use test_r::{test, timeout};

fn local_reader(introducing_oplog_index: u64, binding_slot: u32) -> LocalStreamReaderId {
    LocalStreamReaderId {
        introducing_oplog_index: OplogIndex::from_u64(introducing_oplog_index),
        binding_slot,
    }
}

struct Fixture {
    identity: TestIdentity,
    service: Arc<DefaultWorkerService>,
    oplog: Arc<dyn Oplog>,
    indexed: Arc<ReadCountingIndexedStorage>,
    blobs: Arc<ReadCountingBlobStorage>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_archive(false).await
    }

    async fn with_archive(archive: bool) -> Self {
        Self::with_storage(archive, Arc::new(InMemoryKeyValueStorage::new())).await
    }

    async fn with_storage(archive: bool, storage: Arc<dyn KeyValueStorage + Send + Sync>) -> Self {
        Self::with_buffer(archive, storage, 1).await
    }

    async fn with_buffer(
        archive: bool,
        storage: Arc<dyn KeyValueStorage + Send + Sync>,
        max_operations_before_commit: u64,
    ) -> Self {
        let identity = identity();
        let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
        let indexed = Arc::new(ReadCountingIndexedStorage::new());
        let blobs = Arc::new(ReadCountingBlobStorage::new());
        let primary = Arc::new(
            PrimaryOplogService::new(
                indexed.clone(),
                blobs.clone(),
                max_operations_before_commit,
                1,
                100,
                32,
                RetryConfig::default(),
            )
            .await,
        );
        let primary: Arc<dyn OplogService> = if archive {
            Arc::new(MultiLayerOplogService::new(
                primary,
                nonempty_collections::nev![Arc::new(CompressedOplogArchiveService::new(
                    indexed.clone(),
                    1,
                    RetryConfig::default()
                ))
                    as Arc<dyn crate::services::oplog::OplogArchiveService>],
                10_000,
                10_000,
            ))
        } else {
            primary
        };
        let account = AccountId::new();
        let metadata = AgentMetadata {
            owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
            agent_id: identity.agent_id.clone(),
            env: vec![],
            environment_id: identity.environment_id,
            created_by: account,
            created_by_email: AccountEmail::new("producer-index@test"),
            config: vec![],
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: AgentStatusRecord::default(),
            original_phantom_id: None,
            fingerprint: identity.fingerprint,
            agent_mode: AgentMode::Durable,
        };
        let create = OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
            agent_id: identity.agent_id.clone(),
            owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Durable,
            component_revision: golem_common::model::component::ComponentRevision::INITIAL,
            env: vec![],
            environment_id: identity.environment_id,
            created_by: account,
            parent: None,
            component_size: 100,
            initial_total_linear_memory_size: 100,
            initial_active_plugins: HashSet::new(),
            local_agent_config: vec![],
            original_phantom_id: None,
            instance_id: identity.fingerprint.0,
        }));
        let service = Arc::new(DefaultWorkerService::new(
            storage,
            Arc::new(ShardServiceDefault::new()),
            primary.clone(),
            Arc::new(UnusedComponentService),
            Arc::new(GolemConfig::default()),
        ));
        let oplog = primary
            .create_fresh(
                &mut primary.lock_lifecycle(&owner.agent_id).await,
                &owner,
                AgentMode::Durable,
                create,
                metadata,
                read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(
                    arc_swap::ArcSwap::from_pointee(AgentStatusRecord::default()),
                )),
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
            identity,
            service,
            oplog,
            indexed,
            blobs,
        }
    }

    async fn producer(&self) -> Arc<DurableStreamStore> {
        let oplog = self.oplog.clone();
        let commit: DurableStreamCommit = Arc::new(move |published| {
            let oplog = oplog.clone();
            Box::pin(async move {
                oplog
                    .commit(CommitLevel::Always)
                    .await
                    .expect("oplog write");
                if let Some(published) = published {
                    let _ = published.send(Ok(()));
                }
            })
        });
        DurableStreamStore::load_indexed_with_commit(
            self.oplog.clone(),
            OwnedAgentId::new(self.identity.environment_id, &self.identity.agent_id),
            self.identity.fingerprint,
            None,
            commit,
            self.service.clone(),
            AgentMode::Durable,
        )
        .await
        .unwrap()
    }

    fn registration(&self, ordinal: u32) -> ProducerRegistrationRequest {
        registration(
            &self.identity,
            StreamRegistrationCoordinate::Root {
                invocation_id: self.identity.invocation.clone(),
                root_kind: StreamRootKind::MethodResult,
                recursive_value_path: vec![
                    golem_common::model::durable_stream::StreamValuePathStep::TupleElement(ordinal),
                ],
            },
            StreamSourceKind::InvocationOutput,
        )
    }

    fn input_registration(&self, ordinal: u32) -> ProducerRegistrationRequest {
        registration(
            &self.identity,
            StreamRegistrationCoordinate::Root {
                invocation_id: self.identity.invocation.clone(),
                root_kind: StreamRootKind::MethodInput,
                recursive_value_path: vec![
                    golem_common::model::durable_stream::StreamValuePathStep::TupleElement(ordinal),
                ],
            },
            StreamSourceKind::ExternalInlineInput,
        )
    }

    async fn persist(&self) {
        let owner = OwnedAgentId::new(self.identity.environment_id, &self.identity.agent_id);
        self.oplog
            .commit(CommitLevel::Always)
            .await
            .expect("oplog write");
        self.service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                self.identity.fingerprint,
                vec![ProducerMetadataKey::Global],
            )
            .await
            .unwrap();
        self.indexed.reset();
        self.blobs.reset();
    }
}

#[test]
#[timeout("30s")]
async fn active_attachment_count_is_available_after_cold_metadata_load() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let mut key = crate::durable_host::durable_stream::tests::attachment_key(
        &fixture.identity,
        handle.stream_id,
    );
    key.session_key = fixture.identity.invocation.clone();
    key.attachment_id = AttachmentId::primary(
        key.session_key.callee_environment_id,
        &key.session_key.callee,
        &key.session_key.idempotency_key,
    )
    .unwrap();
    producer.prepare_attachment(key.clone(), 100).await.unwrap();
    producer
        .activate_attachment(key.clone(), 110)
        .await
        .unwrap();
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    assert!(cold.has_reconcilable_attachments().await);
    assert!(
        cold.has_active_attachment(&key.session_key, &handle)
            .await
            .unwrap()
    );
    assert!(cold.index.lock().await.attachments.is_empty());
    key.epoch += 1;
    cold.prepare_attachment(key.clone(), 120).await.unwrap();
    assert!(cold.has_reconcilable_attachments().await);
    assert!(
        !cold
            .has_active_attachment(&key.session_key, &handle)
            .await
            .unwrap()
    );
    cold.activate_attachment(key.clone(), 130).await.unwrap();
    assert!(
        cold.activate_attachment(key.clone(), 130)
            .await
            .unwrap()
            .replayed
    );
    fixture.persist().await;
    drop(cold);

    let cold = fixture.producer().await;
    cold.finalize_attachment(
        key.clone(),
        StreamAttachmentFinalizationReason::ConsumerFinalized,
        140,
    )
    .await
    .unwrap();
    assert!(!cold.has_reconcilable_attachments().await);
    assert!(
        !cold
            .has_active_attachment(&key.session_key, &handle)
            .await
            .unwrap()
    );
    fixture.persist().await;
    drop(cold);
    assert!(
        !fixture
            .producer()
            .await
            .has_reconcilable_attachments()
            .await
    );
    assert!(
        !fixture
            .producer()
            .await
            .has_active_attachment(&key.session_key, &handle)
            .await
            .unwrap()
    );
}

#[test]
#[timeout("60s")]
async fn cold_session_mapping_loads_its_local_registration() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let binding = producer
        .local_binding(17, &handle, SessionStreamRole::Output)
        .await
        .unwrap();
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    cold.append_session_record(
        None,
        StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: StreamRegistrationInvocation::Local(
                fixture.identity.invocation.idempotency_key.clone(),
            ),
            mapping: binding.clone(),
        }),
    )
    .await
    .unwrap();
    fixture.persist().await;
    drop(cold);

    let cold = fixture.producer().await;
    assert_eq!(
        cold.materialize_bindings(&[binding]).await.unwrap()[0].handle,
        handle
    );
}

#[test]
#[timeout("60s")]
async fn persisted_metadata_restores_entity_attribution() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let entity_parent_start_index = Some(OplogIndex::from_u64(42));
    let mut request = fixture.registration(0);
    request.entity_parent_start_index = entity_parent_start_index;
    let handle = producer.register(None, request).await.unwrap().value;
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    let index = cold
        .index_for([
            ProducerMetadataKey::Stream(handle.stream_id),
            ProducerMetadataKey::Session(fixture.identity.invocation.clone()),
        ])
        .await
        .unwrap();
    assert_eq!(
        index.entity_parent_start_index(handle.stream_id).unwrap(),
        entity_parent_start_index
    );
    assert_eq!(
        index.session_entity_parent_start_index(&fixture.identity.invocation),
        entity_parent_start_index
    );
    drop(index);
    cold.register_result_streams(
        None,
        fixture.identity.invocation.clone(),
        vec![17],
        vec![ProducerOutputRegistration {
            transport_stream_id: 0,
            source: ProducerOutputSource::Existing(handle),
            cancellation_epoch: None,
            role: SessionStreamRole::Output,
        }],
        entity_parent_start_index,
    )
    .await
    .unwrap();
    fixture.persist().await;
    drop(cold);

    fixture
        .producer()
        .await
        .finish_session(
            None,
            fixture.identity.invocation.clone(),
            entity_parent_start_index,
            Ok(()),
            StreamCancelReason::Protocol,
        )
        .await
        .unwrap();
    let tip = fixture.oplog.current_oplog_index().await;
    for position in 2..=tip.as_u64() {
        let entry = fixture
            .oplog
            .read(OplogIndex::from_u64(position))
            .await
            .unwrap();
        assert_eq!(entry.entity_parent_start_index(), entity_parent_start_index);
    }
    assert!(matches!(
        fixture.oplog.read(tip).await.unwrap(),
        OplogEntry::StreamSession { .. }
    ));
}

#[test]
#[timeout("60s")]
async fn result_without_owned_outputs_retains_entity_attribution_after_reload() {
    let fixture = Fixture::new().await;
    let attribution = Some(OplogIndex::from_u64(73));
    fixture
        .producer()
        .await
        .register_result_streams(
            None,
            fixture.identity.invocation.clone(),
            vec![29],
            Vec::new(),
            attribution,
        )
        .await
        .unwrap();
    let result_index = fixture.oplog.current_oplog_index().await;
    assert_eq!(
        fixture
            .oplog
            .read(result_index)
            .await
            .unwrap()
            .entity_parent_start_index(),
        attribution
    );
    fixture.persist().await;
    fixture
        .producer()
        .await
        .finish_session(
            None,
            fixture.identity.invocation.clone(),
            attribution,
            Ok(()),
            StreamCancelReason::Protocol,
        )
        .await
        .unwrap();
    let finished_index = fixture.oplog.current_oplog_index().await;
    assert_eq!(finished_index, result_index.next());
    let entry = fixture.oplog.read(finished_index).await.unwrap();
    assert_eq!(entry.entity_parent_start_index(), attribution);
    let OplogEntry::StreamSession { record, .. } = entry else {
        panic!("expected session completion")
    };
    assert!(matches!(
        fixture.oplog.download_payload(record).await.unwrap(),
        StreamSessionRecord::Finished(_)
    ));
}

#[test]
#[timeout("60s")]
async fn external_producer_offsets_survive_indexed_cold_load() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.input_registration(0))
        .await
        .unwrap()
        .value;
    let request = ExternalProducer {
        id: ExternalProducerId::Client("indexed".into()),
        epoch: 3,
        sequence: 0,
    };
    let accepted = producer
        .append_external_input(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![7])),
            false,
            Some(request.clone()),
        )
        .await
        .unwrap();
    let ExternalAppendOutcome::Accepted(offset) = accepted else {
        panic!()
    };
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    assert_eq!(
        cold.append_external_input(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayload::PackedU8(vec![7])),
            false,
            Some(request),
        )
        .await
        .unwrap(),
        ExternalAppendOutcome::Duplicate {
            offset,
            highest_sequence: Some(0),
        }
    );
}

#[test]
#[timeout("60s")]
async fn attached_transport_sequences_survive_indexed_cold_load_with_http_interleaving() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.input_registration(97))
        .await
        .unwrap()
        .value;
    let first = producer
        .write_attached_items_with_nested(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![10, 11]),
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(first.value.len(), 2);
    assert!(matches!(
        producer
            .append_external_input(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayload::Values(vec![
                    vec![20],
                    vec![30],
                    vec![31],
                ])),
                false,
                None,
            )
            .await
            .unwrap(),
        ExternalAppendOutcome::Accepted(_)
    ));
    let nested = registration(
        &fixture.identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: handle.stream_id,
            parent_producer_sequence: 2,
            recursive_value_path: vec![
                golem_common::model::durable_stream::StreamValuePathStep::TupleElement(0),
            ],
        },
        StreamSourceKind::Nested,
    );
    let payload = StreamItemsPayload::Values(vec![vec![40]]);
    let reads = fixture.indexed.reads();
    assert_eq!(
        producer
            .attached_global_sequence(&fixture.identity.invocation, handle.stream_id, 2)
            .await
            .unwrap(),
        5
    );
    assert_eq!(
        fixture.indexed.reads(),
        reads,
        "a hot attached sequence lookup must not project an absent sequence from storage"
    );
    let fresh = producer
        .write_attached_items_with_nested(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            2,
            payload.clone(),
            vec![nested.clone()],
        )
        .await
        .unwrap();
    assert!(!fresh.replayed);
    assert_eq!(
        producer
            .attached_global_sequence(&fixture.identity.invocation, handle.stream_id, 2)
            .await
            .unwrap(),
        5
    );
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    let retry = cold
        .write_attached_items_with_nested(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            2,
            payload.clone(),
            vec![nested.clone()],
        )
        .await
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.value, fresh.value);
    assert_eq!(
        cold.attached_global_sequence(&fixture.identity.invocation, handle.stream_id, 2)
            .await
            .unwrap(),
        5
    );
    let high_water = cold
        .attached_input_high_water(&fixture.identity.invocation, handle.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(high_water.highest_contiguous_sequence, 2);
    assert!(!high_water.terminal);

    *cold.index.lock().await = ProducerStreamIndex::default();
    fixture.indexed.reset();
    let rehydrated = cold
        .write_attached_items_with_nested(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            2,
            payload,
            vec![nested],
        )
        .await
        .unwrap();
    assert!(rehydrated.replayed);
    assert_eq!(rehydrated.value, fresh.value);
    cold.append_external_input(
        None,
        &fixture.identity.invocation,
        handle.stream_id,
        None,
        true,
        Some(ExternalProducer {
            id: ExternalProducerId::Attached,
            epoch: 0,
            sequence: 3,
        }),
    )
    .await
    .unwrap();
    let high_water = cold
        .attached_input_high_water(&fixture.identity.invocation, handle.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(high_water.highest_contiguous_sequence, 3);
    assert!(high_water.terminal);
}

#[test]
#[timeout("60s")]
async fn persisted_finished_session_rejects_new_events_after_cold_load_and_eviction() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let session = fixture.identity.invocation.clone();
    producer
        .ensure_session_accepts_new_events(&session)
        .await
        .unwrap();
    producer
        .finish_session(
            None,
            session.clone(),
            None,
            Ok(()),
            StreamCancelReason::Protocol,
        )
        .await
        .unwrap();
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    assert!(cold.index.lock().await.finished_sessions.is_empty());
    let tip = fixture.oplog.current_oplog_index().await;
    assert_eq!(
        cold.ensure_session_accepts_new_events(&session).await,
        Err(StreamStoreError::SessionFinished(session.clone()))
    );
    let reads = fixture.indexed.reads();
    for _ in 0..3 {
        assert_eq!(
            cold.ensure_session_accepts_new_events(&session).await,
            Err(StreamStoreError::SessionFinished(session.clone()))
        );
    }
    assert_eq!(fixture.indexed.reads(), reads, "warm checks perform no IO");

    for ordinal in 0..130 {
        let mut other = session.clone();
        other.idempotency_key =
            golem_common::model::IdempotencyKey::new(format!("open-session-{ordinal}"));
        cold.ensure_session_accepts_new_events(&other)
            .await
            .unwrap();
    }
    assert!(!cold.index.lock().await.finished_sessions.contains(&session));
    assert_eq!(
        cold.ensure_session_accepts_new_events(&session).await,
        Err(StreamStoreError::SessionFinished(session))
    );
    assert_eq!(fixture.oplog.current_oplog_index().await, tip);
    assert_eq!(
        fixture.blobs.reads(),
        0,
        "checks do not load payload history"
    );
}

#[test]
#[timeout("60s")]
async fn persisted_producer_cold_load_pages_only_requested_payloads() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let mut offsets = Vec::new();
    for sequence in 0..140 {
        let outcome = producer
            .write_items(
                None,
                handle.stream_id,
                sequence,
                StreamItemsPayload::Values(vec![vec![sequence as u8; 128]]),
            )
            .await
            .unwrap();
        offsets.push(outcome.value[0]);
    }
    for _ in 0..2100 {
        fixture
            .oplog
            .add(OplogEntry::interrupted())
            .await
            .expect("oplog write");
    }
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;
    assert_eq!(
        fixture.indexed.reads(),
        1,
        "cold load reads the owner's Create entry without scanning stream history"
    );
    assert_eq!(
        fixture.blobs.reads(),
        0,
        "cold load must not download payload history"
    );
    assert!(cold.index.lock().await.streams.is_empty());
    let high_water = cold
        .input_high_water(handle.stream_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(high_water.highest_contiguous_sequence, 139);
    let metadata_reads = fixture.indexed.reads();
    assert_eq!(fixture.blobs.reads(), 0);
    for _ in 0..20 {
        cold.input_high_water(handle.stream_id).await.unwrap();
    }
    assert_eq!(
        fixture.indexed.reads(),
        metadata_reads,
        "warm metadata performs no storage IO"
    );
    let first = cold
        .read_segment(&handle, None, Some(offsets[0]))
        .await
        .unwrap();
    assert_eq!(first.len(), 1, "catch-up stops at the requested horizon");
    assert_eq!(fixture.blobs.reads(), 1);
    let second = cold
        .read_segment(&handle, Some(first[0].offset), Some(offsets[1]))
        .await
        .unwrap();
    assert_eq!(second[0].producer_sequence, 1);
    assert_eq!(fixture.blobs.reads(), 2);
    assert!(
        cold.index.lock().await.streams[&handle.stream_id]
            .batches
            .len()
            <= 128
    );
    let mut after = Some(second[0].offset);
    for sequence in 2..140 {
        let page = cold
            .read_segment(&handle, after, Some(offsets[sequence as usize]))
            .await
            .unwrap();
        assert_eq!(page[0].producer_sequence, sequence);
        after = Some(page[0].offset);
        assert!(cold.index.lock().await.batch_positions.len() <= 129);
    }
    assert_eq!(
        fixture.blobs.reads(),
        140,
        "each requested payload is read once"
    );
    let replay = cold
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![0; 128]]),
        )
        .await
        .unwrap();
    assert_eq!(replay.value, vec![first[0].offset]);
}

#[test]
#[timeout("60s")]
async fn cold_packed_cursor_reads_the_enclosing_batch_and_traverses_pages() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let offsets = producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8((0..5000).map(|index| index as u8).collect()),
        )
        .await
        .unwrap()
        .value;
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;

    let mut after = Some(offsets[9]);
    let mut sequences = Vec::new();
    loop {
        let page = cold.read_segment(&handle, after, None).await.unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 256);
        if sequences.is_empty() {
            assert_eq!(
                page.iter()
                    .map(|event| event.producer_sequence)
                    .collect::<Vec<_>>(),
                (10..266).collect::<Vec<_>>()
            );
        }
        sequences.extend(page.iter().map(|event| event.producer_sequence));
        after = page.last().map(|event| event.offset);
    }
    assert_eq!(sequences, (10..5000).collect::<Vec<_>>());
    assert_eq!(
        fixture.blobs.reads(),
        20,
        "one packed payload read per page"
    );
}

#[test]
#[timeout("60s")]
async fn cold_packed_batches_remain_contiguous_and_respect_exact_through() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let first = producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![1; 64]),
        )
        .await
        .unwrap()
        .value;
    let second = producer
        .write_items(
            None,
            handle.stream_id,
            64,
            StreamItemsPayload::PackedU8(vec![2; 64]),
        )
        .await
        .unwrap()
        .value;
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;

    let page = cold
        .read_segment(&handle, Some(first[9]), None)
        .await
        .unwrap();
    assert_eq!(
        page.iter()
            .map(|event| event.producer_sequence)
            .collect::<Vec<_>>(),
        (10..128).collect::<Vec<_>>()
    );
    assert_eq!(
        page[54].offset, second[0],
        "the next batch starts at sequence 64"
    );

    fixture.blobs.reset();
    let exact = cold
        .read_segment(&handle, Some(first[9]), Some(first[63]))
        .await
        .unwrap();
    assert_eq!(exact.len(), 54);
    assert_eq!(exact.last().unwrap().offset, first[63]);
    assert_eq!(
        fixture.blobs.reads(),
        1,
        "through does not read the next payload"
    );
}

#[test]
#[timeout("60s")]
async fn cold_ordinary_segment_loads_batch_locators_in_linear_windows() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    for sequence in 0..256 {
        producer
            .write_items(
                None,
                handle.stream_id,
                sequence,
                StreamItemsPayload::Values(vec![vec![sequence as u8; 64]]),
            )
            .await
            .unwrap();
    }
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;
    let indexed_reads = fixture.indexed.reads();
    let page = cold.read_segment(&handle, None, None).await.unwrap();
    assert_eq!(page.len(), 256);
    assert_eq!(
        page.iter()
            .map(|event| event.producer_sequence)
            .collect::<Vec<_>>(),
        (0..256).collect::<Vec<_>>()
    );
    assert_eq!(fixture.blobs.reads(), 256);
    // Each external payload requires one indexed oplog entry read, in addition
    // to the metadata reads that resolve the three locator windows.
    let locator_reads = fixture.indexed.reads() - indexed_reads - 256;
    assert!(
        locator_reads <= 6,
        "batch locator fields are loaded in fixed windows, got {locator_reads} reads"
    );
}

#[test]
#[timeout("60s")]
async fn deletion_complete_cold_fallback_preserves_full_locator_prefix() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let mut offsets = Vec::new();
    for sequence in 0..140 {
        offsets.push(
            producer
                .write_items(
                    None,
                    handle.stream_id,
                    sequence,
                    StreamItemsPayload::Values(vec![vec![sequence as u8]]),
                )
                .await
                .unwrap()
                .value[0],
        );
    }
    let terminal_offset = producer
        .end(None, handle.stream_id, 140, StreamEndResult::Ok)
        .await
        .unwrap()
        .value;
    fixture.persist().await;
    drop(producer);

    let cold = fixture.producer().await;
    cold.commit_deletion_barrier(1_000, true).await.unwrap();
    assert!(cold.index.lock().await.complete_for_deletion);

    let page = cold.read_segment(&handle, None, None).await.unwrap();
    assert_eq!(page.len(), 141);
    assert_eq!(
        page.iter().map(|event| event.offset).collect::<Vec<_>>(),
        offsets
            .iter()
            .copied()
            .chain(std::iter::once(terminal_offset))
            .collect::<Vec<_>>()
    );
    assert!(page[..140].iter().enumerate().all(|(sequence, event)| {
        event.producer_sequence == sequence as u64
            && event.payload == CommittedProducerStreamEventPayload::Value(vec![sequence as u8])
    }));
    assert!(matches!(
        &page.last().unwrap().payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    ));

    let index = cold.index.lock().await;
    assert!(index.complete_for_deletion);
    let stream = &index.streams[&handle.stream_id];
    assert_eq!(
        stream.batches,
        offsets
            .iter()
            .enumerate()
            .map(|(sequence, offset)| { (sequence as u64, offset.producer_oplog_index()) })
            .collect()
    );
    assert_eq!(
        stream.offsets(),
        offsets
            .iter()
            .copied()
            .chain(std::iter::once(terminal_offset))
            .collect::<Vec<_>>(),
        "deletion cascade must retain the exact producer offset prefix"
    );
    drop(index);
    cold.validate_cursor(handle.stream_id, Some(offsets[139]))
        .await
        .unwrap();
}

#[test]
#[timeout("60s")]
async fn archived_projection_extends_nested_chunk_and_replays_lazily() {
    let fixture = Fixture::with_archive(true).await;
    let producer = DurableStreamStore::load(
        fixture.oplog.clone(),
        fixture.identity.environment_id,
        fixture.identity.agent_id.clone(),
        fixture.identity.fingerprint,
        None,
    )
    .await
    .unwrap();
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    for _ in 0..1021 {
        fixture
            .oplog
            .add(OplogEntry::interrupted())
            .await
            .expect("oplog write");
    }
    assert_eq!(fixture.oplog.current_oplog_index().await.as_u64(), 1023);
    let nested = registration(
        &fixture.identity,
        StreamRegistrationCoordinate::Nested {
            parent_stream_id: handle.stream_id,
            parent_producer_sequence: 0,
            recursive_value_path: vec![
                golem_common::model::durable_stream::StreamValuePathStep::OptionSome,
            ],
        },
        StreamSourceKind::Nested,
    );
    producer
        .write_items_with_nested(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1; 128]]),
            vec![nested],
        )
        .await
        .unwrap();
    let nested_handles = producer.nested_handles(handle.stream_id, 0).await.unwrap();
    fixture
        .oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");
    MultiLayerOplog::try_archive_blocking(&fixture.oplog)
        .await
        .expect("archive layer");
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;
    assert_eq!(fixture.blobs.reads(), 0);
    let page = cold.read_segment(&handle, None, None).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].nested_handles, nested_handles);
    assert_eq!(fixture.blobs.reads(), 1);
    assert_eq!(cold.index.lock().await.streams.len(), 2);
}

#[test]
#[timeout("30s")]
async fn cold_replay_allows_nested_dependencies_larger_than_cache_budget() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let nested = (0..64)
        .map(|ordinal| {
            registration(
                &fixture.identity,
                StreamRegistrationCoordinate::Nested {
                    parent_stream_id: handle.stream_id,
                    parent_producer_sequence: 0,
                    recursive_value_path: vec![
                        golem_common::model::durable_stream::StreamValuePathStep::TupleElement(
                            ordinal,
                        ),
                    ],
                },
                StreamSourceKind::Nested,
            )
        })
        .collect();
    producer
        .write_items_with_nested(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::Values(vec![vec![1; 128]]),
            nested,
        )
        .await
        .unwrap();
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;
    let page = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cold.read_segment(&handle, None, None),
    )
    .await
    .expect("one operation may exceed the resident cache budget")
    .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].nested_handles.len(), 64);
    let record = cold
        .read_item_batch(page[0].offset.producer_oplog_index())
        .await
        .unwrap();
    let reads = fixture.indexed.reads();
    assert_eq!(
        cold.resolve_nested_handles(&record.nested_stream_ids)
            .await
            .unwrap(),
        page[0].nested_handles
    );
    assert_eq!(
        fixture.indexed.reads(),
        reads,
        "the complete working set stays warm"
    );
    cold.input_high_water(handle.stream_id).await.unwrap();
    assert!(cold.index.lock().await.loaded_metadata.len() <= 128);
}

#[test]
#[timeout("60s")]
async fn persisted_reconciliation_pages_active_catalogue_without_payloads() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7]),
        )
        .await
        .unwrap();
    let terminal_offset = producer
        .end(None, handle.stream_id, 1, StreamEndResult::Ok)
        .await
        .unwrap()
        .value;
    let mut attachments = Vec::new();
    for ordinal in 0..140 {
        let mut key = crate::durable_host::durable_stream::tests::attachment_key(
            &fixture.identity,
            handle.stream_id,
        );
        key.session_key.idempotency_key =
            golem_common::model::IdempotencyKey::new(format!("consumer-{ordinal}"));
        key.consumer_invocation = key.session_key.clone();
        key.attachment_id = AttachmentId::primary(
            key.consumer_environment_id,
            &key.consumer,
            &key.session_key.idempotency_key,
        )
        .unwrap();
        producer.prepare_attachment(key.clone(), 100).await.unwrap();
        attachments.push(key);
    }
    producer
        .finalize_attachment(
            attachments[0].clone(),
            StreamAttachmentFinalizationReason::ConsumerFinalized,
            200,
        )
        .await
        .unwrap();
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;
    let mut found = HashSet::new();
    for _ in 0..5 {
        let page = cold
            .indexed_attachment_candidates(32)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.candidates.len(), 32);
        for candidate in page.candidates {
            let attachment = candidate.attachment;
            let summary = candidate.journal_summary;
            assert_eq!(
                summary,
                ProducerJournalSummary {
                    event_count: 2,
                    last_offset: Some(terminal_offset),
                    terminal: true,
                }
            );
            assert_ne!(attachment.key, attachments[0]);
            found.insert(attachment.key);
        }
    }
    assert_eq!(found, attachments.into_iter().skip(1).collect());
    assert_eq!(fixture.blobs.reads(), 0);
    assert!(
        cold.index.lock().await.streams.is_empty(),
        "reconciler must not hydrate runtime streams"
    );
}

#[test]
#[timeout("60s")]
async fn persisted_fork_projection_keeps_prefix_locators_and_post_marker_appends() {
    let fixture = Fixture::new().await;
    let mut source = identity();
    source.agent_id.agent_id = "source".into();
    source.fingerprint = AgentFingerprint(uuid::Uuid::from_u128(90));
    source.invocation.callee = source.agent_id.clone();
    source.invocation.callee_fingerprint = source.fingerprint;
    let registration = registered_stream(
        OplogIndex::from_u64(3),
        source.environment_id,
        source.agent_id.clone(),
        source.fingerprint,
        registration(
            &source,
            StreamRegistrationCoordinate::Root {
                invocation_id: source.invocation.clone(),
                root_kind: StreamRootKind::MethodResult,
                recursive_value_path: vec![],
            },
            StreamSourceKind::InvocationOutput,
        ),
    );
    let old = registration.handle.clone();
    let attachment =
        crate::durable_host::durable_stream::tests::attachment_key(&source, old.stream_id);
    let cut = StreamForkCutRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        request_hash: vec![0; 32],
        creation_fingerprint: fixture.identity.fingerprint,
        export: None,
        cut_index: OplogIndex::from_u64(5),
        revert: None,
        epoch_floor: 1,
        selected_stream_id: Some(LocalStreamId(OplogIndex::from_u64(3))),
        retained_through: Some(StreamOffset::new(OplogIndex::from_u64(4), 1)),
    };
    let mut next = old.clone();
    next.stream_id = StreamId::derive(
        fixture.identity.environment_id,
        &fixture.identity.agent_id,
        fixture.identity.fingerprint,
        OplogIndex::from_u64(3),
    )
    .unwrap();
    next.producer = fixture.identity.agent_id.clone();
    next.expected_producer_fingerprint = fixture.identity.fingerprint;
    next.source_invocation = fixture.identity.invocation.clone();
    let items = |position, _stream_id, _producer_fingerprint, first_sequence, bytes: Vec<u8>| {
        OplogEntry::StreamItems {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamItemsRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: golem_common::base_model::durable_stream::LocalStreamId(
                    OplogIndex::from_u64(3),
                ),
                first_sequence,
                offsets: (0..bytes.len())
                    .map(|sub| StreamOffset::new(OplogIndex::from_u64(position), sub as u32))
                    .collect(),
                nested_stream_ids: vec![],
                newly_registered_stream_ids: vec![],
                payload: StreamItemsPayload::PackedU8(bytes),
            })),
            summary: None,
        }
    };
    for entry in [
        OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::Prepared(
                crate::services::worker_fork::lineage::tests::prepared(&source.invocation),
            ))),
            summary: None,
        },
        OplogEntry::StreamRegistered {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(registration.record)),
            summary: None,
        },
        items(
            4,
            old.stream_id,
            source.fingerprint,
            0,
            vec![10, 11, 12, 13, 14],
        ),
        OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::AttachmentPrepared(
                golem_common::model::durable_stream::StreamAttachmentPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: attachment.clone(),
                    prepared_at_millis: 100,
                    lease_expires_at_millis: 1000,
                },
            ))),
            summary: None,
        },
        OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            summary: None,
        },
        items(
            7,
            next.stream_id,
            fixture.identity.fingerprint,
            2,
            vec![40, 41],
        ),
    ] {
        fixture.oplog.add(entry).await.unwrap();
    }
    fixture.persist().await;
    let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
    let (_, rows) = fixture
        .service
        .lookup_durable_stream_producer_metadata(
            &owner,
            AgentMode::Durable,
            fixture.identity.fingerprint,
            vec![
                ProducerMetadataKey::Stream(old.stream_id),
                ProducerMetadataKey::Stream(next.stream_id),
                ProducerMetadataKey::Batch(next.stream_id, 0),
                ProducerMetadataKey::Position(next.stream_id, OplogIndex::from_u64(4)),
                ProducerMetadataKey::Batch(next.stream_id, 2),
                ProducerMetadataKey::SessionPage(0),
                ProducerMetadataKey::Session(source.invocation),
                ProducerMetadataKey::Attachment(
                    attachment.attachment_id,
                    old.stream_id,
                    attachment.consumer_environment_id,
                    attachment.consumer.clone(),
                ),
                ProducerMetadataKey::AttachmentPage(0),
                ProducerMetadataKey::Global,
            ],
        )
        .await
        .unwrap();
    assert!(rows[0].is_none());
    assert!(
        matches!(&rows[1], Some(ProducerMetadataRow::Stream(state)) if state.next_sequence == 4 && !state.terminal)
    );
    assert!(
        matches!(rows[2], Some(ProducerMetadataRow::Batch(position)) if position == OplogIndex::from_u64(4))
    );
    assert!(matches!(rows[3], Some(ProducerMetadataRow::Position(0, 2))));
    assert!(
        matches!(rows[4], Some(ProducerMetadataRow::Batch(position)) if position == OplogIndex::from_u64(7))
    );
    assert!(
        matches!(&rows[5], Some(ProducerMetadataRow::SessionPage(sessions)) if sessions == std::slice::from_ref(&fixture.identity.invocation))
    );
    assert!(rows[6].is_none());
    assert!(rows[7].is_none());
    assert!(rows[8].is_none());
    assert!(matches!(
        &rows[9],
        Some(ProducerMetadataRow::Global {
            active_attachments: 0,
            ..
        })
    ));
    let mut fresh = attachment.clone();
    fresh.stream_id = next.stream_id;
    fresh.producer = next.producer.clone();
    fresh.expected_producer_fingerprint = next.expected_producer_fingerprint;
    let producer = fixture.producer().await;
    let prefix = producer.read_segment(&next, None, None).await.unwrap();
    assert_eq!(
        prefix
            .iter()
            .map(|event| event.payload.clone())
            .collect::<Vec<_>>(),
        [10, 11, 40, 41]
            .into_iter()
            .map(CommittedProducerStreamEventPayload::PackedU8)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        prefix.iter().map(|event| event.offset).collect::<Vec<_>>(),
        vec![
            StreamOffset::new(OplogIndex::from_u64(4), 0),
            StreamOffset::new(OplogIndex::from_u64(4), 1),
            StreamOffset::new(OplogIndex::from_u64(7), 0),
            StreamOffset::new(OplogIndex::from_u64(7), 1),
        ]
    );
    assert!(prefix.iter().all(|event| event.stream_id == next.stream_id));
    assert_eq!(prefix[0].packed_u8_batch_end, Some(prefix[1].offset));
    assert_eq!(prefix[2].packed_u8_batch_end, Some(prefix[3].offset));
    producer
        .prepare_attachment(fresh.clone(), 200)
        .await
        .unwrap();
    producer
        .activate_attachment(fresh.clone(), 210)
        .await
        .unwrap();
    fixture.persist().await;
    let (_, rows) = fixture
        .service
        .lookup_durable_stream_producer_metadata(
            &owner,
            AgentMode::Durable,
            fixture.identity.fingerprint,
            vec![
                ProducerMetadataKey::AttachmentPage(0),
                ProducerMetadataKey::AttachmentPosition(
                    fresh.attachment_id,
                    fresh.stream_id,
                    fresh.consumer_environment_id,
                    fresh.consumer.clone(),
                ),
                ProducerMetadataKey::Global,
            ],
        )
        .await
        .unwrap();
    assert!(
        matches!(&rows[0], Some(ProducerMetadataRow::AttachmentPage(page)) if page == &vec![(fresh.attachment_id, fresh.stream_id, fresh.consumer_environment_id, fresh.consumer.clone())])
    );
    assert!(matches!(
        &rows[1],
        Some(ProducerMetadataRow::AttachmentPosition(Some(0)))
    ));
    assert!(matches!(
        &rows[2],
        Some(ProducerMetadataRow::Global {
            active_attachments: 1,
            ..
        })
    ));
    producer
        .finalize_attachment(
            fresh.clone(),
            StreamAttachmentFinalizationReason::ConsumerFinalized,
            220,
        )
        .await
        .unwrap();
    fixture.persist().await;
    let (_, rows) = fixture
        .service
        .lookup_durable_stream_producer_metadata(
            &owner,
            AgentMode::Durable,
            fixture.identity.fingerprint,
            vec![
                ProducerMetadataKey::AttachmentPage(0),
                ProducerMetadataKey::Global,
            ],
        )
        .await
        .unwrap();
    assert!(matches!(&rows[0], Some(ProducerMetadataRow::AttachmentPage(page)) if page.is_empty()));
    assert!(matches!(
        &rows[1],
        Some(ProducerMetadataRow::Global {
            active_attachments: 0,
            ..
        })
    ));
}

#[test]
#[timeout("60s")]
async fn cold_fork_terminal_reads_use_owner_relative_identity() {
    let fixture = Fixture::new().await;
    fixture
        .oplog
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::Prepared(
                crate::services::worker_fork::lineage::tests::prepared(
                    &fixture.identity.invocation,
                ),
            ))),
            summary: None,
        })
        .await
        .unwrap();
    let producer = fixture.producer().await;
    let original = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let terminal = producer
        .end(None, original.stream_id, 0, StreamEndResult::Ok)
        .await
        .unwrap()
        .value;
    fixture.persist().await;
    drop(producer);
    let cut = StreamForkCutRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        request_hash: vec![0; 32],
        creation_fingerprint: fixture.identity.fingerprint,
        export: None,
        cut_index: fixture.oplog.current_oplog_index().await,
        revert: None,
        epoch_floor: 1,
        selected_stream_id: None,
        retained_through: None,
    };
    let continuation = original;
    fixture
        .oplog
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            summary: None,
        })
        .await
        .unwrap();
    fixture.persist().await;
    let cold = fixture.producer().await;
    let events = cold.read_segment(&continuation, None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].stream_id, continuation.stream_id);
    assert_eq!(events[0].offset, terminal);
    assert_eq!(
        events[0].payload,
        CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
    );
}

#[test]
#[timeout("60s")]
async fn raw_and_indexed_controls_discard_nested_descendants_at_a_fork() {
    use crate::durable_host::durable_session::StreamSession;
    use crate::durable_host::durable_stream::tests::TestOplog;
    use crate::services::worker_fork::lineage::tests::{fork, prepared};
    use golem_common::model::durable_stream::{
        StreamConsumerItemValueRecord, StreamSessionMappingUpdateRecord,
    };

    let fixture = Fixture::with_buffer(false, Arc::new(InMemoryKeyValueStorage::new()), 100).await;
    let target = &fixture.identity;
    let owner = OwnedAgentId::new(target.environment_id, &target.agent_id);
    let mut source = identity();
    source.agent_id.agent_id = "source".into();
    source.invocation.callee = source.agent_id.clone();
    let oplog = Arc::new(TestOplog::default());
    oplog
        .add(fixture.oplog.read(OplogIndex::INITIAL).await.unwrap())
        .await
        .unwrap();
    let producer = DurableStreamStore::load(
        oplog.clone(),
        source.environment_id,
        source.agent_id.clone(),
        source.fingerprint,
        None,
    )
    .await
    .unwrap();
    producer
        .append_session_record(
            None,
            StreamSessionRecord::Prepared(prepared(&source.invocation)),
        )
        .await
        .unwrap();
    let root = producer
        .register(
            None,
            registration(
                &source,
                StreamRegistrationCoordinate::Root {
                    invocation_id: source.invocation.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: vec![],
                },
                StreamSourceKind::InvocationOutput,
            ),
        )
        .await
        .unwrap()
        .value;
    let root_local_id = LocalStreamId(oplog.current_oplog_index().await);
    let retained = producer
        .write_items(
            None,
            root.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![7, 9]),
        )
        .await
        .unwrap()
        .value[0];
    let mut handles = vec![root.clone()];
    for (parent, sequence) in [(0, 2), (1, 0)] {
        let parent = handles[parent].stream_id;
        producer
            .write_items_with_nested(
                None,
                parent,
                sequence,
                StreamItemsPayload::Values(vec![vec![1]]),
                vec![registration(
                    &source,
                    StreamRegistrationCoordinate::Nested {
                        parent_stream_id: parent,
                        parent_producer_sequence: sequence,
                        recursive_value_path: vec![],
                    },
                    StreamSourceKind::Nested,
                )],
            )
            .await
            .unwrap();
        handles.push(producer.nested_handles(parent, sequence).await.unwrap()[0].clone());
    }
    for (number, handle) in handles.iter().enumerate() {
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                    format_version: 1,
                    session_key: StreamRegistrationInvocation::Remote(source.invocation.clone()),
                    mapping: StreamBindingRecord {
                        transport_stream_id: number as u64,
                        source: StreamRecordReference::Foreign(handle.clone()),
                        role: SessionStreamRole::Output,
                    },
                }),
            )
            .await
            .unwrap();
        let reader_id = LocalStreamReaderId {
            introducing_oplog_index: oplog.current_oplog_index().await,
            binding_slot: 0,
        };
        producer
                .append_session_record(
                    None,
                    StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                        format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                        observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                        reobserves: None,
                        session_key: StreamRegistrationInvocation::Remote(
                            source.invocation.clone(),
                        ),
                        reader_id,
                        source_offset: retained,
                        consumer_read_ordinal: 0,
                        value: vec![7],
                        packed_u8: true,
                        recursive_mappings: vec![],
                    }),
                )
                .await
                .unwrap();
    }
    let copied = Arc::new(TestOplog::default());
    for (_, entry) in oplog
        .read_exact(
            OplogIndex::INITIAL,
            retained.producer_oplog_index().as_u64(),
        )
        .await
        .unwrap()
    {
        copied.add(entry).await.unwrap();
    }
    let mut cut = fork(Some(root_local_id), copied.current_oplog_index().await);
    cut.creation_fingerprint = target.fingerprint;
    cut.retained_through = Some(retained);
    copied
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            summary: None,
        })
        .await
        .unwrap();
    let oplog = copied;
    for (_, entry) in oplog
        .read_exact(
            OplogIndex::INITIAL.next(),
            oplog.current_oplog_index().await.as_u64() - 1,
        )
        .await
        .unwrap()
    {
        fixture.oplog.add(entry).await.unwrap();
    }
    fixture.persist().await;
    let raw_producer = DurableStreamStore::load(
        oplog.clone(),
        owner.environment_id,
        owner.agent_id.clone(),
        target.fingerprint,
        None,
    )
    .await
    .unwrap();
    let raw = StreamSession::new(
        raw_producer.clone(),
        oplog,
        StreamRegistrationInvocation::Local(target.invocation.idempotency_key.clone()),
        [],
    );
    let raw = raw.current_control_metadata().await.unwrap();
    let mut indexed = fixture
        .service
        .lookup_durable_stream_control_metadata(
            &owner,
            AgentMode::Durable,
            target.fingerprint,
            &target.invocation,
        )
        .await
        .unwrap();
    indexed.assign_recovery_slot(None);
    assert_eq!(*raw, indexed);
    assert!(raw.acceptance_mappings().unwrap().is_empty());
    assert!(raw.consumer_record_counts().is_empty());
    let root = raw_producer
        .materialize_binding(&StreamBindingRecord {
            transport_stream_id: 0,
            source: StreamRecordReference::Local(root_local_id),
            role: SessionStreamRole::Output,
        })
        .await
        .unwrap()
        .handle;
    let events = raw_producer.read_segment(&root, None, None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload,
        CommittedProducerStreamEventPayload::PackedU8(7)
    );
    assert!(matches!(
        raw_producer.nested_handles(root.stream_id, 2).await,
        Err(StreamStoreError::EventConflict)
    ));
}

#[test]
#[timeout("60s")]
async fn persisted_fork_chain_preserves_consumer_only_heads() {
    use golem_common::model::durable_stream::{
        StreamConsumerItemValueRecord, StreamConsumerTerminal, StreamConsumerTerminalRecord,
    };
    for terminal_record in [false, true] {
        let fixture =
            Fixture::with_buffer(false, Arc::new(InMemoryKeyValueStorage::new()), 100).await;
        let target = fixture.identity.invocation.clone();
        let mut source = target.clone();
        source.callee.agent_id = "source".into();
        source.callee_fingerprint = AgentFingerprint(uuid::Uuid::from_u128(91));
        let mut middle = target.clone();
        middle.callee.agent_id = "middle".into();
        middle.callee_fingerprint = AgentFingerprint(uuid::Uuid::from_u128(92));
        let handle = super::super::registration::registered_stream(
            OplogIndex::from_u64(123),
            fixture.identity.environment_id,
            fixture.identity.agent_id.clone(),
            fixture.identity.fingerprint,
            fixture.registration(0),
        )
        .handle;
        let mut prepared = crate::services::worker_fork::lineage::tests::prepared(&source);
        prepared
            .attempt
            .invocation
            .stream_handles
            .push(handle.clone());
        prepared.stream_mappings.push(StreamBindingRecord {
            transport_stream_id: 0,
            source: StreamRecordReference::Foreign(handle.clone()),
            role: SessionStreamRole::Input,
        });
        let reader = local_reader(2, 0);
        let terminal = StreamOffset::new(OplogIndex::from_u64(91), 0);
        let cut = |_from: &StreamSessionKey, to: &StreamSessionKey, index| {
            StreamSessionRecord::ForkCut(StreamForkCutRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                request_hash: vec![0; 32],
                creation_fingerprint: to.callee_fingerprint,
                export: None,
                cut_index: OplogIndex::from_u64(index),
                revert: None,
                epoch_floor: 1,
                selected_stream_id: None,
                retained_through: None,
            })
        };
        for record in [
            StreamSessionRecord::Prepared(prepared),
            StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                format_version:
                    golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                observation:
                    golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                reobserves: None,
                session_key: StreamRegistrationInvocation::Local(source.idempotency_key.clone()),
                reader_id: reader,
                source_offset: StreamOffset::new(OplogIndex::from_u64(90), 3),
                consumer_read_ordinal: 0,
                value: vec![3, 5],
                packed_u8: true,
                recursive_mappings: vec![],
            }),
            if terminal_record {
                StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                        format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                        observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                        reobserves: None,
                        session_key: StreamRegistrationInvocation::Local(
                            source.idempotency_key.clone(),
                        ),
                        reader_id: reader,
                        source_offset: terminal,
                        consumer_read_ordinal: 2,
                        terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
                    })
            } else {
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                        reobserves: None,
                        format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                        observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                        session_key: StreamRegistrationInvocation::Local(
                            source.idempotency_key.clone(),
                        ),
                        reader_id: reader,
                        source_offset: terminal,
                        consumer_read_ordinal: 2,
                        value: vec![7],
                        packed_u8: true,
                        recursive_mappings: vec![],
                    })
            },
            cut(&source, &middle, 4),
            cut(&middle, &target, 5),
        ] {
            let prepared = matches!(&record, StreamSessionRecord::Prepared(_));
            fixture
                .oplog
                .add(OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: OplogPayload::Inline(Box::new(record)),
                    summary: None,
                })
                .await
                .unwrap();
            if prepared {
                assert!(
                    fixture
                        .oplog
                        .raw_durable_stream_session_status(&target)
                        .await
                        .status
                        .unwrap()
                        .is_some()
                );
                assert!(
                    fixture
                        .oplog
                        .raw_durable_stream_session_status(&source)
                        .await
                        .status
                        .unwrap()
                        .is_none()
                );
            }
        }
        let buffered = fixture
            .oplog
            .raw_durable_stream_session_status(&target)
            .await
            .status
            .unwrap()
            .unwrap();
        assert_eq!(buffered.session_key.as_ref(), Some(&target.idempotency_key));
        assert_eq!(buffered.attachment_epoch, Some(1));
        assert_eq!(buffered.attachment_attached, Some(false));
        assert!(
            fixture
                .oplog
                .raw_durable_stream_session_status(&source)
                .await
                .status
                .unwrap()
                .is_none()
        );
        fixture.persist().await;
        use crate::services::HasOplogService;
        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let persisted = fixture
            .service
            .oplog_service()
            .stream_session_index()
            .unwrap()
            .lookup_latest(
                &owner,
                AgentMode::Durable,
                fixture.identity.fingerprint,
                &target.idempotency_key,
            )
            .await
            .unwrap();
        assert_eq!(persisted, Some(buffered.clone()));
        assert_eq!(
            fixture
                .oplog
                .raw_durable_stream_session_status(&target)
                .await
                .status
                .unwrap(),
            Some(buffered)
        );
        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let (_, rows) = fixture
            .service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                fixture.identity.fingerprint,
                vec![
                    ProducerMetadataKey::ConsumerHead(source.clone(), reader),
                    ProducerMetadataKey::ConsumerHead(middle.clone(), reader),
                    ProducerMetadataKey::ConsumerHead(target.clone(), reader),
                ],
            )
            .await
            .unwrap();
        assert!(rows[0].is_none());
        assert!(rows[1].is_none());
        assert!(
            matches!(&rows[2], Some(ProducerMetadataRow::ConsumerHead(head)) if head.terminal == terminal_record && head.next_read_ordinal == 3 && head.last_source_offset == Some(terminal))
        );
        let control = fixture
            .service
            .lookup_durable_stream_control_metadata(
                &owner,
                AgentMode::Durable,
                fixture.identity.fingerprint,
                &target,
            )
            .await
            .unwrap();
        assert_eq!(control.prepared_position(), Some(OplogIndex::from_u64(2)));
        assert_eq!(
            control.consumer_record_counts(),
            &HashMap::from([(reader, 2)])
        );
        let raw_producer = DurableStreamStore::load(
            fixture.oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            fixture.identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let raw = crate::durable_host::durable_session::StreamSession::new(
            raw_producer.clone(),
            fixture.oplog.clone(),
            StreamRegistrationInvocation::Local(target.idempotency_key.clone()),
            [],
        );
        let raw_control = raw.current_control_metadata().await.unwrap();
        let mut expected = control.clone();
        expected.assign_recovery_slot(None);
        assert_eq!(*raw_control, expected);
        assert_eq!(
            fixture
                .service
                .read_durable_stream_consumer_page(
                    &owner,
                    fixture.identity.fingerprint,
                    &target,
                    reader,
                    0,
                )
                .await
                .unwrap(),
            vec![OplogIndex::from_u64(3), OplogIndex::from_u64(4)]
        );
        for retired in [&source, &middle] {
            let raw = crate::durable_host::durable_session::StreamSession::new(
                raw_producer.clone(),
                fixture.oplog.clone(),
                StreamRegistrationInvocation::Remote(retired.clone()),
                [],
            );
            let raw_control = raw.current_control_metadata().await.unwrap();
            assert!(raw_control.prepared_position().is_none());
            assert!(raw_control.consumer_record_counts().is_empty());
            let control = fixture
                .service
                .lookup_durable_stream_control_metadata(
                    &owner,
                    AgentMode::Durable,
                    fixture.identity.fingerprint,
                    retired,
                )
                .await
                .unwrap();
            assert!(control.prepared_position().is_none());
            assert!(control.consumer_record_counts().is_empty());
        }
        let recovery = fixture
            .service
            .lookup_durable_stream_recovery_metadata(
                &owner,
                AgentMode::Durable,
                fixture.identity.fingerprint,
            )
            .await
            .unwrap();
        assert_eq!(
            recovery
                .sessions
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
            vec![target.clone()]
        );
        if !terminal_record {
            fixture
                    .oplog
                    .add(OplogEntry::StreamSession {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: OplogPayload::Inline(Box::new(
                            StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                                format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                                observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                                reobserves: None,
                                session_key: StreamRegistrationInvocation::Local(
                                    target.idempotency_key.clone(),
                                ),
                                reader_id: reader,
                                source_offset: StreamOffset::new(OplogIndex::from_u64(92), 4),
                                consumer_read_ordinal: 3,
                                value: vec![11, 13],
                                packed_u8: true,
                                recursive_mappings: vec![],
                            }),
                        )),
                        summary: None,
                    })
                    .await
                    .unwrap();
            fixture.persist().await;
            assert_eq!(
                fixture
                    .service
                    .read_durable_stream_consumer_page(
                        &owner,
                        fixture.identity.fingerprint,
                        &target,
                        reader,
                        0,
                    )
                    .await
                    .unwrap(),
                vec![
                    OplogIndex::from_u64(3),
                    OplogIndex::from_u64(4),
                    OplogIndex::from_u64(7)
                ]
            );
            let control = fixture
                .service
                .lookup_durable_stream_control_metadata(
                    &owner,
                    AgentMode::Durable,
                    fixture.identity.fingerprint,
                    &target,
                )
                .await
                .unwrap();
            assert_eq!(
                control.consumer_record_counts(),
                &HashMap::from([(reader, 3)])
            );
        }
    }
}

#[test]
#[timeout("60s")]
async fn persisted_session_catalogue_includes_mapping_free_foreign_sessions_once() {
    let fixture = Fixture::new().await;
    let mut expected = HashSet::new();
    for range in [0..127, 127..130, 0..130] {
        let producer = fixture.producer().await;
        for ordinal in range {
            let mut session = fixture.identity.invocation.clone();
            session.idempotency_key =
                golem_common::model::IdempotencyKey::new(format!("outbound-{ordinal}"));
            session.callee.agent_id = "foreign()".into();
            let repeated = !expected.insert(session.clone());
            let reader_id = local_reader(1, 0);
            producer
                    .append_session_record(None, StreamSessionRecord::ConsumerItemValue(
                        golem_common::base_model::durable_stream::StreamConsumerItemValueRecord {
                            format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                            observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                            reobserves: None,
                            session_key: StreamRegistrationInvocation::Remote(session),
                            reader_id,
                            source_offset: StreamOffset::new(
                                OplogIndex::from_u64(if repeated { 2 } else { 1 }),
                                0,
                            ),
                            consumer_read_ordinal: u64::from(repeated),
                            value: vec![7],
                            packed_u8: false,
                            recursive_mappings: Vec::new(),
                        },
                    ))
                    .await
                    .unwrap();
        }
        fixture.persist().await;
        drop(producer);
        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let (_, rows) = fixture
            .service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                fixture.identity.fingerprint,
                vec![
                    ProducerMetadataKey::Global,
                    ProducerMetadataKey::SessionPage(0),
                    ProducerMetadataKey::SessionPage(1),
                ],
            )
            .await
            .unwrap();
        assert!(
            matches!(rows[0], Some(ProducerMetadataRow::Global { session_count, .. }) if session_count == expected.len() as u64)
        );
        let mut actual = Vec::new();
        for (page, row) in rows.into_iter().skip(1).enumerate() {
            if let Some(ProducerMetadataRow::SessionPage(sessions)) = row {
                assert_eq!(
                    sessions.len(),
                    (expected.len().saturating_sub(page * 128)).min(128)
                );
                actual.extend(sessions);
            }
        }
        assert_eq!(actual.len(), expected.len());
        assert_eq!(actual.into_iter().collect::<HashSet<_>>(), expected);
    }
}

#[test]
#[timeout("60s")]
async fn persisted_consumer_head_summarizes_long_journal_without_history_reads() {
    let fixture = Fixture::new().await;
    let consumer = fixture.producer().await;
    let reader_id = local_reader(2, 0);
    let session_key = fixture.identity.invocation.clone();
    for ordinal in 0..512 {
        consumer
                .append_session_record(
                    None,
                    StreamSessionRecord::ConsumerItemValue(
                        golem_common::base_model::durable_stream::StreamConsumerItemValueRecord {
                            format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                            observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                            reobserves: None,
                            session_key: StreamRegistrationInvocation::Local(
                                session_key.idempotency_key.clone(),
                            ),
                            reader_id,
                            source_offset: StreamOffset::new(OplogIndex::from_u64(ordinal + 1), 0),
                            consumer_read_ordinal: ordinal,
                            value: vec![ordinal as u8],
                            packed_u8: false,
                            recursive_mappings: Vec::new(),
                        },
                    ),
                )
                .await
                .unwrap();
    }
    let terminal_offset = StreamOffset::new(OplogIndex::from_u64(513), 0);
    consumer
            .append_session_record(
                None,
                StreamSessionRecord::ConsumerTerminal(
                    golem_common::model::durable_stream::StreamConsumerTerminalRecord {
                        format_version: golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                        observation: golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                        reobserves: None,
                        session_key: StreamRegistrationInvocation::Local(
                            session_key.idempotency_key.clone(),
                        ),
                        reader_id,
                        source_offset: terminal_offset,
                        consumer_read_ordinal: 512,
                        terminal: golem_common::model::durable_stream::StreamConsumerTerminal::End(
                            StreamEndResult::Ok,
                        ),
                    },
                ),
            )
            .await
            .unwrap();
    fixture.persist().await;
    drop(consumer);

    let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
    let reads_before = fixture.indexed.reads();
    let (_, rows) = fixture
        .service
        .lookup_durable_stream_producer_metadata(
            &owner,
            AgentMode::Durable,
            fixture.identity.fingerprint,
            vec![ProducerMetadataKey::ConsumerHead(session_key, reader_id)],
        )
        .await
        .unwrap();
    let Some(Some(ProducerMetadataRow::ConsumerHead(head))) = rows.into_iter().next() else {
        panic!("persisted consumer head is missing");
    };
    assert_eq!(head.next_read_ordinal, 513);
    assert_eq!(head.last_source_offset, Some(terminal_offset));
    assert!(head.terminal);
    assert!(head.source_unavailable.is_none());
    assert_eq!(fixture.indexed.reads(), reads_before + 1);
    assert_eq!(fixture.blobs.reads(), 0);
}

#[test]
#[timeout("30s")]
async fn cancelled_control_query_drains_detached_index_catch_up() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let session = fixture.identity.invocation.clone();
    let reader = local_reader(2, 0);
    let record = fixture
        .oplog
        .upload_payload(&StreamSessionRecord::ConsumerItemValue(
            golem_common::base_model::durable_stream::StreamConsumerItemValueRecord {
                format_version:
                    golem_common::base_model::durable_stream::STREAM_CONSUMER_RECORD_FORMAT_VERSION,
                observation:
                    golem_common::base_model::durable_stream::StreamConsumerObservation::Host,
                reobserves: None,
                session_key: StreamRegistrationInvocation::Local(session.idempotency_key.clone()),
                reader_id: reader,
                source_offset: StreamOffset::new(OplogIndex::from_u64(17), 0),
                consumer_read_ordinal: 0,
                value: vec![9; 256],
                packed_u8: false,
                recursive_mappings: Vec::new(),
            },
        ))
        .await
        .unwrap();
    assert!(matches!(record, OplogPayload::External { .. }));
    fixture
        .oplog
        .add(OplogEntry::stream_session(None, record, None))
        .await
        .expect("oplog write");
    fixture
        .oplog
        .commit(CommitLevel::Always)
        .await
        .expect("oplog write");
    let horizon = fixture.oplog.current_oplog_index().await;
    let (started, release) = fixture.blobs.pause_next_read();
    let mut query = Box::pin(producer.persisted_control_metadata(&session));
    assert!(futures::poll!(query.as_mut()).is_pending());
    started.await.unwrap();
    drop(query);
    producer.durable_activity.close();
    let drained = producer.durable_activity.wait_drained();
    tokio::pin!(drained);
    assert!(futures::poll!(&mut drained).is_pending());
    release.send(()).unwrap();
    drained.await;

    let reads = fixture.indexed.reads();
    let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
    let metadata = fixture
        .service
        .lookup_durable_stream_control_metadata(
            &owner,
            AgentMode::Durable,
            fixture.identity.fingerprint,
            &session,
        )
        .await
        .unwrap();
    assert_eq!(metadata.covered_through(), horizon);
    assert_eq!(metadata.consumer_record_count(reader), 1);
    assert_eq!(
        fixture.indexed.reads(),
        reads + 1,
        "only the horizon lookup may read the oplog after drain, not another catch-up"
    );
    assert!(producer.persisted_control_metadata(&session).await.is_err());
}

#[test]
#[timeout("30s")]
async fn suspended_host_does_not_retain_producer_hydration_lock() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    producer
        .end(
            None,
            handle.stream_id,
            0,
            StreamEndResult::ErrorContext(vec![7; 256]),
        )
        .await
        .unwrap();
    fixture.persist().await;
    drop(producer);
    let cold = fixture.producer().await;
    let (started, release) = fixture.blobs.pause_next_read();
    let mut suspended = Box::pin(cold.end(
        None,
        handle.stream_id,
        0,
        StreamEndResult::ErrorContext(vec![7; 256]),
    ));
    assert!(futures::poll!(suspended.as_mut()).is_pending());
    tokio::time::timeout(std::time::Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cold.input_high_water(handle.stream_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fixture.blobs.reads(), 1);
    suspended.await.unwrap();
}

#[test]
#[timeout("30s")]
async fn retirement_joins_unary_hydration_before_and_after_parent_cancellation() {
    for cancel_parent in [false, true] {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        producer
            .end(
                None,
                handle.stream_id,
                0,
                StreamEndResult::ErrorContext(vec![7; 256]),
            )
            .await
            .unwrap();
        fixture.persist().await;
        let cold = fixture.producer().await;
        let late = fixture.producer().await;
        let (started, release) = fixture.blobs.pause_next_read();
        let mut unary = Some(Box::pin(cold.index_for_terminal([], handle.stream_id)));
        assert!(futures::poll!(unary.as_mut().unwrap().as_mut()).is_pending());
        started.await.unwrap();
        let mut stop = Box::pin(fixture.oplog.stop_and_wait());
        assert!(
            futures::poll!(stop.as_mut()).is_pending(),
            "retirement must observe the task before its parent is cancelled"
        );
        assert!(late.index_for_terminal([], handle.stream_id).await.is_err());
        assert!(late.indexed_attachment_candidates(32).await.is_err());
        if cancel_parent {
            drop(unary.take());
        }
        assert!(futures::poll!(stop.as_mut()).is_pending());
        release.send(()).unwrap();
        stop.await.unwrap();
        if let Some(unary) = unary {
            let index = unary.await.unwrap();
            assert!(index.streams[&handle.stream_id].terminal_event.is_some());
        }
        assert!(
            cold.index.try_lock().is_ok(),
            "hydration must release its actual lock"
        );
        assert_eq!(fixture.blobs.reads(), 1);
    }
}

#[test]
#[timeout("30s")]
async fn cached_queries_need_no_task_admission_after_root_stop() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let expected = producer.input_high_water(handle.stream_id).await.unwrap();
    let reads = fixture.indexed.reads();
    producer.tasks().stop_roots_and_wait().await;
    assert!(
        producer.indexed_attachment_candidates(1).await.is_ok(),
        "owner cleanup still needs metadata queries after roots stop"
    );
    producer.tasks().stop_and_wait().await.unwrap();
    let reads_after_cleanup = fixture.indexed.reads();
    assert!(reads_after_cleanup >= reads);
    for _ in 0..32 {
        assert_eq!(
            producer.input_high_water(handle.stream_id).await.unwrap(),
            expected
        );
    }
    assert_eq!(fixture.indexed.reads(), reads_after_cleanup);
    assert!(producer.indexed_attachment_candidates(1).await.is_err());
}

#[test]
#[ignore]
async fn metadata_task_admission_workloads() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let worker = Arc::new(());
    producer.set_worker_tasks(
        fixture
            .oplog
            .task_owner()
            .unwrap()
            .for_worker(worker.clone()),
    );
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    producer
        .write_items(
            None,
            handle.stream_id,
            0,
            StreamItemsPayload::PackedU8(vec![17, 3, 29]),
        )
        .await
        .unwrap();
    producer
        .end(
            None,
            handle.stream_id,
            3,
            StreamEndResult::ErrorContext(vec![7; 256]),
        )
        .await
        .unwrap();
    let attachment = crate::durable_host::durable_stream::tests::attachment_key(
        &fixture.identity,
        handle.stream_id,
    );
    producer.prepare_attachment(attachment, 100).await.unwrap();
    let attempt_id = golem_common::model::durable_stream::AttemptId::fresh();
    producer
        .append_session_record(
            None,
            StreamSessionRecord::CallerAttempt(
                golem_common::model::durable_stream::StreamCallerAttemptRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: StreamRegistrationInvocation::Local(
                        fixture.identity.invocation.idempotency_key.clone(),
                    ),
                    attempt_id,
                },
            ),
        )
        .await
        .unwrap();
    fixture.persist().await;

    for workload in [
        "cached-query",
        "cold-hydration",
        "attachment-query",
        "session-resume",
    ] {
        for sample in 0..=10 {
            for tracked in if sample % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let iterations = if sample == 0 {
                    100
                } else if workload == "cached-query" {
                    100_000
                } else {
                    1_000
                };
                let reads = fixture.indexed.reads();
                let blobs = fixture.blobs.reads();
                let start = std::time::Instant::now();
                for _ in 0..iterations {
                    match workload {
                        "cached-query" => {
                            assert!(
                                producer
                                    .input_high_water(handle.stream_id)
                                    .await
                                    .unwrap()
                                    .is_some()
                            );
                        }
                        "cold-hydration" => {
                            *producer.index.lock().await = ProducerStreamIndex::default();
                            let query = producer.clone().hydrate_query(
                                vec![ProducerMetadataKey::Stream(handle.stream_id)],
                                Some(handle.stream_id),
                                None,
                            );
                            let task = if tracked {
                                producer.tasks().spawn_metadata(query).unwrap()
                            } else {
                                tokio::spawn(query)
                            };
                            task.await.unwrap().unwrap();
                            assert!(
                                producer.index.lock().await.streams[&handle.stream_id]
                                    .terminal_event
                                    .is_some()
                            );
                        }
                        "attachment-query" => {
                            let producer_for_query = producer.clone();
                            let query = async move {
                                producer_for_query
                                    .indexed_attachment_candidates_inner(32)
                                    .await
                            };
                            let task = if tracked {
                                producer.tasks().spawn_metadata(query).unwrap()
                            } else {
                                tokio::spawn(query)
                            };
                            let IndexedAttachmentCandidateBatch { candidates, .. } =
                                task.await.unwrap().unwrap().unwrap();
                            assert_eq!(candidates.len(), 1);
                        }
                        "session-resume" => {
                            let streams = crate::durable_host::durable_session::StreamSession::new(
                                producer.clone(),
                                fixture.oplog.clone(),
                                StreamRegistrationInvocation::Local(
                                    fixture.identity.invocation.idempotency_key.clone(),
                                ),
                                [],
                            );
                            let scope = crate::worker::tasks::TaskScope::default();
                            let resumed = if tracked {
                                scope.bind(producer.tasks()).unwrap();
                                scope.run(streams.caller_attempt_id()).await.unwrap()
                            } else {
                                streams.caller_attempt_id().await
                            };
                            assert_eq!(resumed.unwrap(), attempt_id);
                        }
                        _ => unreachable!(),
                    }
                }
                let elapsed = start.elapsed();
                println!(
                    "workload={workload}, tracked={tracked}, sample={sample}, iterations={iterations}, ns/op={}, indexed_reads={}, blob_reads={}",
                    elapsed.as_nanos() / iterations,
                    fixture.indexed.reads() - reads,
                    fixture.blobs.reads() - blobs
                );
            }
        }
    }
}

#[test]
async fn suspended_contended_query_does_not_reserve_producer_index() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let held = producer.index.lock().await;
    let mut suspended = Box::pin(producer.input_high_water(handle.stream_id));
    assert!(futures::poll!(suspended.as_mut()).is_pending());
    drop(held);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        producer.input_high_water(handle.stream_id),
    )
    .await
    .unwrap()
    .unwrap();
    suspended.await.unwrap();
}

#[test]
async fn contended_warm_producer_queries_make_progress() {
    let fixture = Fixture::new().await;
    let producer = fixture.producer().await;
    let handle = producer
        .register(None, fixture.registration(0))
        .await
        .unwrap()
        .value;
    let held = producer.index.lock().await;
    let mut queries = (0..16)
        .map(|_| Box::pin(producer.input_high_water(handle.stream_id)))
        .collect::<Vec<_>>();
    for query in &mut queries {
        assert!(futures::poll!(query.as_mut()).is_pending());
    }
    drop(held);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        futures::future::try_join_all(queries),
    )
    .await
    .unwrap()
    .unwrap();
}
