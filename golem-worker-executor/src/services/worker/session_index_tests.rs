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
use crate::services::component::ComponentService;
use crate::services::oplog::{
    CommitLevel, CompressedOplogArchiveService, DurableStreamOplogRecord, MultiLayerOplog,
    MultiLayerOplogService, Oplog, OplogArchiveService, OplogService, PrimaryOplogService,
};
use crate::services::shard::ShardServiceDefault;
use crate::services::stream_session_index::{METADATA_FIELD, Metadata};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use async_trait::async_trait;
use golem_common::model::RetryConfig;
use golem_common::model::Timestamp;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::AgentMode;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::durable_stream::{
    AttachmentId, AttemptId, PersistedStreamInvocationDescriptorV1, ResumeAttemptDescriptorV1,
    StartAttemptDescriptorV1, StreamInvocationIdV1, StreamResumeOperationV1,
    StreamSessionAttachedRecordV1, StreamSessionDetachedRecordV1, StreamSessionFinishedRecordV1,
    StreamSessionInvocationResultRecordV1, StreamSessionPreparedRecordV1, StreamSessionRecordV1,
    StreamSessionResumeAttemptRecordV1,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::OplogPayload;
use golem_common::model::{
    AgentFingerprint, AgentInvocationPayload, AgentMetadata, DurableStreamSessionIndex,
};
use golem_common::read_only_lock;
use golem_service_base::model::component::Component;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use nonempty_collections::nev;
use std::sync::RwLock;
use test_r::test;

pub(crate) struct UnusedComponentService;

#[async_trait]
impl ComponentService for UnusedComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        unreachable!()
    }

    async fn get_metadata(
        &self,
        _component_id: ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        unreachable!()
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        unreachable!()
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        vec![]
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

fn owned_agent(name: &str, component_id: ComponentId) -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id,
            agent_id: name.to_string(),
        },
    )
}

async fn service() -> (DefaultWorkerService, Arc<InMemoryKeyValueStorage>) {
    let kv = Arc::new(InMemoryKeyValueStorage::new());
    let oplog = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    (
        DefaultWorkerService::new(
            kv.clone(),
            Arc::new(ShardServiceDefault::new()),
            oplog,
            Arc::new(UnusedComponentService),
            Arc::new(GolemConfig::default()),
        ),
        kv,
    )
}

async fn service_with_oplog() -> (
    DefaultWorkerService,
    Arc<InMemoryKeyValueStorage>,
    Arc<PrimaryOplogService>,
) {
    let kv = Arc::new(InMemoryKeyValueStorage::new());
    let oplog = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let service = DefaultWorkerService::new(
        kv.clone(),
        Arc::new(ShardServiceDefault::new()),
        oplog.clone(),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    (service, kv, oplog)
}

fn session_key(id: &OwnedAgentId, key: &IdempotencyKey) -> StreamInvocationIdV1 {
    StreamInvocationIdV1 {
        callee_environment_id: id.environment_id,
        callee: id.agent_id.clone(),
        callee_fingerprint: AgentFingerprint(id.agent_id.component_id.0),
        idempotency_key: key.clone(),
    }
}

fn prepared_record(id: &OwnedAgentId, key: &IdempotencyKey) -> StreamSessionRecordV1 {
    let session_key = session_key(id, key);
    StreamSessionRecordV1::Prepared(StreamSessionPreparedRecordV1 {
        format_version: 1,
        attempt: StartAttemptDescriptorV1 {
            format_version: 1,
            session_key: session_key.clone(),
            attachment_id: AttachmentId::primary(id.environment_id, &id.agent_id, key).unwrap(),
            expected_callee_fingerprint: session_key.callee_fingerprint,
            attempt_id: AttemptId(uuid::Uuid::new_v4()),
            invocation: PersistedStreamInvocationDescriptorV1 {
                format_version: 1,
                session_key,
                target_component_revision: ComponentRevision::INITIAL,
                method_name: "test".into(),
                invocation_value: vec![],
                stream_handles: vec![],
                execution_config: vec![],
                effective_identity: vec![],
            },
            effective_identity: vec![],
            live_join_buffer_events: 1,
        },
        stream_mappings: vec![],
    })
}

fn agent_metadata(id: &OwnedAgentId) -> AgentMetadata {
    AgentMetadata {
        agent_id: id.agent_id.clone(),
        env: vec![],
        environment_id: id.environment_id,
        created_by: AccountId::new(),
        created_by_email: AccountEmail::new("session-index@test"),
        config: vec![],
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord::default(),
        original_phantom_id: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
    }
}

fn stale_status() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
        AgentStatusRecord::default(),
    )))
}

fn suspended_status() -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        agent_mode: AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
    })))
}

async fn create_oplog(service: &dyn OplogService, id: &OwnedAgentId) -> Arc<dyn Oplog> {
    service
        .create_fresh(
            id,
            AgentMode::Durable,
            OplogEntry::NoOp {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
            },
            agent_metadata(id),
            stale_status(),
            suspended_status(),
        )
        .await
}

async fn append_session(oplog: &dyn Oplog, record: StreamSessionRecordV1) -> OplogIndex {
    oplog
        .add(DurableStreamOplogRecord::Session(None, Box::new(record)).into_inline_entry())
        .await
}

async fn append_noop(oplog: &dyn Oplog) -> OplogIndex {
    oplog
        .add(OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
        })
        .await
}

async fn append_pending_invocation(oplog: &dyn Oplog, key: &IdempotencyKey) -> OplogIndex {
    oplog
        .add(OplogEntry::pending_agent_invocation(
            key.clone(),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        ))
        .await
}

fn attached_record(
    session_key: StreamInvocationIdV1,
    attachment_id: AttachmentId,
    attempt_id: AttemptId,
    epoch: u64,
    pending_invocation_oplog_index: OplogIndex,
) -> StreamSessionRecordV1 {
    StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
        format_version: 1,
        session_key,
        attachment_id,
        attempt_id,
        epoch,
        pending_invocation_oplog_index,
    })
}

fn detached_record(
    session_key: StreamInvocationIdV1,
    attachment_id: AttachmentId,
    attempt_id: AttemptId,
    epoch: u64,
) -> StreamSessionRecordV1 {
    StreamSessionRecordV1::Detached(StreamSessionDetachedRecordV1 {
        format_version: 1,
        session_key,
        attachment_id,
        owner_attempt_id: attempt_id,
        epoch,
    })
}

fn completed(first: u64, finished: u64) -> DurableStreamSessionStatus {
    DurableStreamSessionStatus {
        first_prepared: Some(OplogIndex::from_u64(first)),
        prepared: Some(OplogIndex::from_u64(first)),
        invocation_result: None,
        finished: Some(OplogIndex::from_u64(finished)),
        ..Default::default()
    }
}

#[test]
async fn persisted_control_projection_reopens_without_history_and_catches_committed_suffix() {
    use crate::services::oplog::tests::ReadCountingIndexedStorage;
    let storage = Arc::new(ReadCountingIndexedStorage::new());
    let kv = Arc::new(InMemoryKeyValueStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            storage.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            10000,
            10000,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let make_service = || {
        DefaultWorkerService::new(
            kv.clone(),
            Arc::new(ShardServiceDefault::new()),
            oplog_service.clone(),
            Arc::new(UnusedComponentService),
            Arc::new(GolemConfig::default()),
        )
    };
    let service = make_service();
    let id = owned_agent("control-projection", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = session_key(&id, &IdempotencyKey::new("result".into()));
    for _ in 0..2050 {
        append_noop(oplog.as_ref()).await;
    }
    let result = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
            format_version: 1,
            session_key: key.clone(),
            result: vec![42; 8192],
            output_streams: vec![],
            stream_mappings: vec![],
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    storage.reset();
    let metadata = service
        .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &key)
        .await
        .unwrap();
    assert_eq!(metadata.invocation_result, Some(result));
    assert!(
        storage.reads() >= 4,
        "initial catchup must read multiple chunks"
    );
    assert!(
        serialize(&metadata).unwrap().len() < 1024,
        "result bytes must not enter the metadata snapshot"
    );

    let reopened = make_service();
    storage.reset();
    let metadata = reopened
        .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &key)
        .await
        .unwrap();
    assert_eq!(metadata.invocation_result, Some(result));
    assert_eq!(
        storage.reads(),
        1,
        "only the committed tip probe is needed after reopening"
    );
    let mut other_key = key.clone();
    other_key.callee_fingerprint = AgentFingerprint::new();
    assert!(
        reopened
            .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &other_key)
            .await
            .unwrap()
            .invocation_result
            .is_none()
    );

    let finished = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
            format_version: 1,
            session_key: key.clone(),
            result: Ok(()),
        }),
    )
    .await;
    // A DB-direct lookup must not expose a buffered local append.
    assert!(
        reopened
            .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &key)
            .await
            .unwrap()
            .finished
            .is_none()
    );
    oplog.commit(CommitLevel::Always).await;
    storage.reset();
    let metadata = reopened
        .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &key)
        .await
        .unwrap();
    assert_eq!(metadata.finished, Some(finished));
    assert_eq!(metadata.covered_through, finished);
    assert_eq!(
        storage.reads(),
        2,
        "the tip and one uncovered suffix read are sufficient"
    );

    let consumer_key = session_key(&id, &IdempotencyKey::new("consumer".into()));
    let stream = StreamId(uuid::Uuid::new_v4());
    let other_stream = StreamId(uuid::Uuid::new_v4());
    let mut expected = Vec::new();
    for ordinal in 0..600 {
        for target in [stream, other_stream] {
            let index = append_session(
                oplog.as_ref(),
                StreamSessionRecordV1::ConsumerItemValue(
                    golem_common::model::durable_stream::StreamConsumerItemValueRecordV1 {
                        format_version: 1,
                        session_key: consumer_key.clone(),
                        stream_id: target,
                        source_offset: golem_common::model::durable_stream::StreamOffsetV1::new(
                            OplogIndex::from_u64(ordinal + 1),
                            0,
                        ),
                        consumer_read_ordinal: ordinal,
                        value: vec![7; 4096],
                        packed_u8: false,
                        recursive_handles: vec![],
                        recursive_mappings: vec![],
                    },
                ),
            )
            .await;
            if target == stream {
                expected.push(index);
            }
        }
    }
    oplog.commit(CommitLevel::Always).await;
    let status = AgentStatusRecord {
        oplog_idx: oplog.current_oplog_index().await,
        has_durable_stream_history: true,
        ..Default::default()
    };
    assert!(!status.durable_stream_sessions.has_history());
    reopened
        .write_cached_status(&id, None, status)
        .await
        .unwrap();
    storage.reset();
    let metadata = reopened
        .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &consumer_key)
        .await
        .unwrap();
    assert_eq!(metadata.consumer_record_counts.get(&stream), Some(&600));
    assert_eq!(
        storage.reads(),
        1,
        "caller-only status flush must cover the journal before cold lookup"
    );
    assert!(
        serialize(&metadata).unwrap().len() < 1024,
        "journal lifetime must not grow the session snapshot"
    );
    let mut actual = Vec::new();
    storage.reset();
    for page in 0..3 {
        actual.extend(
            reopened
                .read_durable_stream_consumer_page(&id, &consumer_key, stream, page)
                .await
                .unwrap(),
        );
    }
    assert_eq!(actual, expected);
    assert_eq!(
        storage.reads(),
        0,
        "journal index pages must not fetch payloads or oplog ranges"
    );

    let mut active = Vec::new();
    for number in 0..320 {
        let record = prepared_record(&id, &IdempotencyKey::new(format!("recover-{number}")));
        let StreamSessionRecordV1::Prepared(prepared) = &record else {
            unreachable!();
        };
        active.push(prepared.attempt.session_key.clone());
        append_session(oplog.as_ref(), record).await;
    }
    oplog.commit(CommitLevel::Always).await;
    let recovery = reopened
        .lookup_durable_stream_recovery_metadata(&id, AgentMode::Durable)
        .await
        .unwrap();
    assert_eq!(recovery.sessions.len(), 320);
    let mut cache = crate::worker::DurableTopologyRecoveryCache::default();
    storage.reset();
    cache
        .refresh(
            oplog.as_ref(),
            &reopened,
            &id,
            AgentMode::Durable,
            key.callee_fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(cache.sessions.len(), 320);
    assert_eq!(
        storage.reads(),
        1,
        "cold recovery uses the catalogue, not agent history"
    );
    cache.dirty.clear();
    storage.reset();
    cache
        .refresh(
            oplog.as_ref(),
            &reopened,
            &id,
            AgentMode::Durable,
            key.callee_fingerprint,
        )
        .await
        .unwrap();
    assert!(cache.dirty.is_empty());
    assert_eq!(
        storage.reads(),
        0,
        "unchanged recovery has no storage reads"
    );

    let record = prepared_record(&id, &IdempotencyKey::new("raw-recovery".into()));
    let StreamSessionRecordV1::Prepared(prepared) = &record else {
        unreachable!();
    };
    let raw_key = prepared.attempt.session_key.clone();
    active.push(raw_key.clone());
    append_session(oplog.as_ref(), record).await;
    cache
        .refresh(
            oplog.as_ref(),
            &reopened,
            &id,
            AgentMode::Durable,
            key.callee_fingerprint,
        )
        .await
        .unwrap();
    assert!(
        cache.sessions.contains_key(&raw_key),
        "raw preparation must be visible before commit"
    );
    assert_eq!(cache.dirty, HashSet::from([raw_key]));
    oplog.commit(CommitLevel::Always).await;
    storage.reset();
    cache
        .refresh(
            oplog.as_ref(),
            &reopened,
            &id,
            AgentMode::Durable,
            key.callee_fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(
        cache.sessions.len(),
        321,
        "commit buffer drain must not discard raw metadata"
    );
    assert_eq!(storage.reads(), 0);

    for key in active.iter().take(260) {
        append_session(
            oplog.as_ref(),
            StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
                format_version: 1,
                session_key: key.clone(),
                result: Ok(()),
            }),
        )
        .await;
    }
    oplog.commit(CommitLevel::Always).await;
    let recovery = reopened
        .lookup_durable_stream_recovery_metadata(&id, AgentMode::Durable)
        .await
        .unwrap();
    assert_eq!(
        recovery
            .sessions
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>(),
        active[260..].iter().cloned().collect()
    );
    let restarted = make_service();
    storage.reset();
    let recovery = restarted
        .lookup_durable_stream_recovery_metadata(&id, AgentMode::Durable)
        .await
        .unwrap();
    assert_eq!(recovery.sessions.len(), 61);
    assert_eq!(
        storage.reads(),
        1,
        "reopened catalogue must not reconstruct completed sessions"
    );
    cache
        .refresh(
            oplog.as_ref(),
            &reopened,
            &id,
            AgentMode::Durable,
            key.callee_fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(cache.sessions.len(), 61);
    for key in active.iter().skip(260) {
        append_session(
            oplog.as_ref(),
            StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
                format_version: 1,
                session_key: key.clone(),
                result: Ok(()),
            }),
        )
        .await;
    }
    oplog.commit(CommitLevel::Always).await;
    assert!(
        restarted
            .lookup_durable_stream_recovery_metadata(&id, AgentMode::Durable)
            .await
            .unwrap()
            .sessions
            .is_empty()
    );

    let mut historical = Vec::new();
    for epoch in 1..=2100 {
        let attempt = AttemptId(uuid::Uuid::new_v4());
        let offset = append_session(
            oplog.as_ref(),
            StreamSessionRecordV1::ResumeAttempt(
                golem_common::model::durable_stream::StreamSessionResumeAttemptRecordV1 {
                    format_version: 1,
                    attempt: golem_common::model::durable_stream::ResumeAttemptDescriptorV1 {
                        format_version: 1,
                        operation:
                            golem_common::model::durable_stream::StreamResumeOperationV1::Takeover,
                        session_key: key.clone(),
                        attachment_id: AttachmentId::primary(
                            id.environment_id,
                            &id.agent_id,
                            &key.idempotency_key,
                        )
                        .unwrap(),
                        expected_callee_fingerprint: key.callee_fingerprint,
                        attempt_id: attempt,
                        expected_epoch: epoch,
                        effective_identity: vec![],
                        cursors: vec![],
                        live_join_buffer_events: 1,
                    },
                    accepted_epoch: epoch + 1,
                },
            ),
        )
        .await;
        historical.push((attempt, offset));
    }
    oplog.commit(CommitLevel::Always).await;

    assert_eq!(
        restarted
            .lookup_durable_stream_resume_offset(&id, AgentMode::Durable, &key, historical[0].0)
            .await
            .unwrap(),
        Some(historical[0].1)
    );
    restarted
        .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &key)
        .await
        .unwrap();
    storage.reset();
    for (attempt, offset) in [historical[0], historical[1025], historical[2099]] {
        assert_eq!(
            restarted
                .lookup_durable_stream_resume_offset(&id, AgentMode::Durable, &key, attempt)
                .await
                .unwrap(),
            Some(offset)
        );
    }
    let mut other_key = key.clone();
    other_key.callee_fingerprint = AgentFingerprint::new();
    assert_eq!(
        restarted
            .lookup_durable_stream_resume_offset(
                &id,
                AgentMode::Durable,
                &other_key,
                historical[0].0
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        storage.reads(),
        4,
        "historical attempts only capture the committed tip, without history or payload reads"
    );
}

#[test]
async fn closed_remote_consumer_streams_leave_recovery_across_epochs() {
    use golem_common::model::durable_stream::*;
    use golem_schema::schema::SchemaFingerprintV1;
    let (service, _, oplog_service) = service_with_oplog().await;
    let owner = owned_agent("consumer", ComponentId::new());
    let remote = owned_agent("remote", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &owner).await;
    let key = session_key(&remote, &IdempotencyKey::new("remote-session".into()));
    let consumer = session_key(&owner, &IdempotencyKey::new("consumer-invocation".into()));
    let mut mappings = Vec::new();
    let mut attachments = Vec::new();
    for transport_stream_id in 0..2 {
        let handle = DurableStreamHandleV1 {
            format_version: 1,
            stream_id: StreamId(uuid::Uuid::new_v4()),
            producer_environment_id: remote.environment_id,
            producer: remote.agent_id.clone(),
            expected_producer_fingerprint: key.callee_fingerprint,
            source_invocation: key.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([0; 32]),
        };
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Output,
        };
        let attachment = StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                remote.environment_id,
                &remote.agent_id,
                &key.idempotency_key,
            )
            .unwrap(),
            stream_id: handle.stream_id,
            epoch: 1,
            session_key: key.clone(),
            producer_environment_id: remote.environment_id,
            producer: remote.agent_id.clone(),
            expected_producer_fingerprint: key.callee_fingerprint,
            consumer_environment_id: owner.environment_id,
            consumer: owner.agent_id.clone(),
            expected_consumer_fingerprint: consumer.callee_fingerprint,
            consumer_invocation: consumer.clone(),
        };
        for record in [
            StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                format_version: 1,
                session_key: key.clone(),
                attachment: attachment.clone(),
                mapping: mapping.clone(),
            }),
            StreamSessionRecordV1::TopologyActivated(StreamTopologyActivatedRecordV1 {
                format_version: 1,
                session_key: key.clone(),
                attachment: attachment.clone(),
                mapping: mapping.clone(),
            }),
        ] {
            assert!(record.has_supported_format());
            append_session(oplog.as_ref(), record).await;
        }
        mappings.push(mapping);
        attachments.push(attachment);
    }
    oplog.commit(CommitLevel::Always).await;
    let mut cache = crate::worker::DurableTopologyRecoveryCache::default();
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            consumer.callee_fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(cache.sessions.len(), 1);
    append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::ConsumerTerminal(StreamConsumerTerminalRecordV1 {
            format_version: 1,
            session_key: key.clone(),
            stream_id: attachments[0].stream_id,
            source_offset: StreamOffsetV1::new(OplogIndex::from_u64(100), 0),
            consumer_read_ordinal: 0,
            terminal: StreamConsumerTerminalV1::End(StreamEndResultV1::Ok),
        }),
    )
    .await;
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            consumer.callee_fingerprint,
        )
        .await
        .unwrap();
    let pending = cache.sessions[&key]
        .recovery_topologies(&owner, &key)
        .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "closing one stream must retain the other stream"
    );
    assert_eq!(pending[0].0.stream_id, attachments[1].stream_id);
    append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::SourceUnavailable(StreamSourceUnavailableRecordV1 {
            format_version: 1,
            key: attachments[1].clone(),
            source_offset: StreamOffsetV1::new(OplogIndex::from_u64(101), 0),
            consumer_read_ordinal: 0,
        }),
    )
    .await;
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            consumer.callee_fingerprint,
        )
        .await
        .unwrap();
    assert!(
        cache.sessions.is_empty(),
        "raw closure must retire resident recovery work"
    );
    oplog.commit(CommitLevel::Always).await;
    assert!(
        service
            .lookup_durable_stream_recovery_metadata(&owner, AgentMode::Durable)
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    let metadata = service
        .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &key)
        .await
        .unwrap();
    assert_eq!(
        metadata
            .topology_status(&attachments[0], Some(&mappings[0]))
            .unwrap(),
        crate::durable_host::durable_stream::ConsumerAttachmentStatus::Active,
        "retiring recovery must preserve historical topology evidence"
    );
    for (mut attachment, mapping) in attachments.into_iter().zip(mappings) {
        attachment.epoch = 2;
        append_session(
            oplog.as_ref(),
            StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                format_version: 1,
                session_key: key.clone(),
                attachment,
                mapping,
            }),
        )
        .await;
    }
    oplog.commit(CommitLevel::Always).await;
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            consumer.callee_fingerprint,
        )
        .await
        .unwrap();
    assert!(cache.sessions.is_empty());
    let mut restarted = crate::worker::DurableTopologyRecoveryCache::default();
    restarted
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            consumer.callee_fingerprint,
        )
        .await
        .unwrap();
    assert!(
        restarted.sessions.is_empty(),
        "later epochs must not reopen journal-closed streams"
    );
}

#[test]
fn bounded_status_evicts_old_completed_sessions_but_retains_unfinished_session() {
    let mut index = DurableStreamSessionIndex::default();
    let old = IdempotencyKey::new("oldest".into());
    index.insert(old.clone(), completed(1, 2));
    for n in 0..140 {
        index.insert(
            IdempotencyKey::new(format!("completed-{n}")),
            completed(3 + n * 2, 4 + n * 2),
        );
    }
    let unfinished = IdempotencyKey::new("unfinished".into());
    index.insert(
        unfinished.clone(),
        DurableStreamSessionStatus {
            first_prepared: Some(OplogIndex::from_u64(500)),
            prepared: Some(OplogIndex::from_u64(501)),
            invocation_result: None,
            finished: None,
            ..Default::default()
        },
    );

    assert!(index.has_history());
    assert!(
        index.get(&old).is_none(),
        "old completion must leave bounded status"
    );
    assert_eq!(index.iter().count(), 129);
    assert!(index.get(&unfinished).is_some());
}

#[test]
async fn covered_physical_index_resolves_evicted_session_and_definitively_misses_absent_key() {
    let (service, kv) = service().await;
    let id = owned_agent("coverage", ComponentId::new());
    let namespace = StreamSessionIndexService::namespace(&id);
    let old = IdempotencyKey::new("evicted".into());
    let old_status = completed(2, 3);
    let metadata = Metadata {
        covered_through: OplogIndex::from_u64(300),
        ..Metadata::default()
    };
    let metadata_bytes = serialize(&metadata).unwrap();
    let status_bytes = serialize(&old_status).unwrap();
    kv.with_entity("test", "seed", "session")
        .set_many_raw(
            namespace,
            &[
                (METADATA_FIELD, metadata_bytes.as_slice()),
                (
                    StreamSessionIndexService::field(&old).as_str(),
                    status_bytes.as_slice(),
                ),
            ],
        )
        .await
        .unwrap();
    let status = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(300),
        durable_stream_sessions: {
            let mut bounded = DurableStreamSessionIndex::default();
            bounded.insert(IdempotencyKey::new("recent".into()), completed(290, 291));
            bounded
        },
        ..AgentStatusRecord::default()
    };

    assert_eq!(
        service
            .lookup_durable_stream_session(&id, AgentMode::Durable, &status, &old)
            .await
            .unwrap(),
        Some(old_status)
    );
    assert_eq!(
        service
            .lookup_durable_stream_session(
                &id,
                AgentMode::Durable,
                &status,
                &IdempotencyKey::new("never-existed".into()),
            )
            .await
            .unwrap(),
        None
    );
}

#[test]
fn stream_session_index_survives_cache_and_checkpoint_binary_roundtrip() {
    let key = IdempotencyKey::new("roundtrip".into());
    let expected = completed(7, 11);
    let mut status = AgentStatusRecord::default();
    status
        .durable_stream_sessions
        .insert(key.clone(), expected.clone());

    let bytes = serialize(&status).unwrap();
    let decoded: AgentStatusRecord = deserialize(&bytes).unwrap();
    assert_eq!(decoded.durable_stream_sessions.get(&key), Some(&expected));

    let mut checkpoint = decoded.clone();
    let parts = split_status(&mut checkpoint);
    let (writes, deletes) = compute_status_field_writes(None, &[], &checkpoint, &parts).unwrap();
    assert!(deletes.is_empty());
    let fields = writes.into_iter().map(|(key, value)| (key, value.into()));
    let restored = reassemble_cached_status(fields).unwrap();
    assert_eq!(restored.durable_stream_sessions.get(&key), Some(&expected));
}

#[test]
async fn physical_indexes_are_isolated_per_agent_and_clear_deletes_only_target_agent() {
    let (service, kv) = service().await;
    let component_id = ComponentId::new();
    let first = owned_agent("first", component_id);
    let second = owned_agent("second", component_id);
    let key = IdempotencyKey::new("same-key".into());
    for (id, finished) in [(&first, 10), (&second, 20)] {
        let namespace = StreamSessionIndexService::namespace(id);
        kv.with_entity("test", "seed", "session")
            .set_raw(
                namespace,
                &StreamSessionIndexService::field(&key),
                &serialize(&completed(finished - 1, finished)).unwrap(),
            )
            .await
            .unwrap();
    }

    service.stream_session_index.clear(&first).await.unwrap();
    let first_keys = kv
        .with("test", "verify")
        .keys(StreamSessionIndexService::namespace(&first))
        .await
        .unwrap();
    let second_keys = kv
        .with("test", "verify")
        .keys(StreamSessionIndexService::namespace(&second))
        .await
        .unwrap();
    assert!(first_keys.is_empty());
    assert_eq!(second_keys, vec![StreamSessionIndexService::field(&key)]);
}

#[test]
async fn catchup_scans_multiple_chunks_and_recovers_evicted_completed_session() {
    let (service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("multi-chunk", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let old = IdempotencyKey::new("old".into());
    let old_first = append_session(oplog.as_ref(), prepared_record(&id, &old)).await;
    let old_result = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &old),
            result: vec![1],
            output_streams: vec![],
            stream_mappings: vec![],
        }),
    )
    .await;
    let old_finished = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &old),
            result: Ok(()),
        }),
    )
    .await;
    for _ in 0..1050 {
        append_noop(oplog.as_ref()).await;
    }
    let mut bounded = DurableStreamSessionIndex::default();
    for n in 0..129 {
        let key = IdempotencyKey::new(format!("recent-{n}"));
        let prepared = append_session(oplog.as_ref(), prepared_record(&id, &key)).await;
        let finished = append_session(
            oplog.as_ref(),
            StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
                format_version: 1,
                session_key: session_key(&id, &key),
                result: Ok(()),
            }),
        )
        .await;
        bounded.insert(key, completed(prepared.as_u64(), finished.as_u64()));
    }
    oplog.commit(CommitLevel::Always).await;
    let horizon = oplog.current_oplog_index().await;
    assert!(horizon.as_u64() > 1024);
    assert!(bounded.iter().count() <= 128);
    let status = AgentStatusRecord {
        oplog_idx: horizon,
        durable_stream_sessions: bounded,
        ..AgentStatusRecord::default()
    };

    let actual = service
        .lookup_durable_stream_session(&id, AgentMode::Durable, &status, &old)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.first_prepared, Some(old_first));
    assert_eq!(actual.prepared, Some(old_first));
    assert_eq!(actual.invocation_result, Some(old_result));
    assert_eq!(actual.finished, Some(old_finished));
    assert_eq!(actual.session_key, Some(session_key(&id, &old)));
}

#[test]
async fn incremental_catchup_merges_later_fields_into_old_unfinished_session() {
    let (service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("incremental", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("unfinished".into());
    let first = append_session(oplog.as_ref(), prepared_record(&id, &key)).await;
    for _ in 0..130 {
        let other = IdempotencyKey::new(uuid::Uuid::new_v4().to_string());
        append_session(oplog.as_ref(), prepared_record(&id, &other)).await;
        append_session(
            oplog.as_ref(),
            StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
                format_version: 1,
                session_key: session_key(&id, &other),
                result: Ok(()),
            }),
        )
        .await;
    }
    oplog.commit(CommitLevel::Always).await;
    let first_horizon = oplog.current_oplog_index().await;
    let status_at_first_horizon = AgentStatusRecord {
        oplog_idx: first_horizon,
        durable_stream_sessions: {
            let mut index = DurableStreamSessionIndex::default();
            for n in 0..129 {
                index.insert(
                    IdempotencyKey::new(format!("bounded-{n}")),
                    completed(n * 2 + 2, n * 2 + 3),
                );
            }
            index
        },
        ..AgentStatusRecord::default()
    };
    let initial = service
        .lookup_durable_stream_session(&id, AgentMode::Durable, &status_at_first_horizon, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.first_prepared, Some(first));
    assert_eq!(initial.finished, None);

    let result = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &key),
            result: vec![2],
            output_streams: vec![],
            stream_mappings: vec![],
        }),
    )
    .await;
    let finished = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &key),
            result: Ok(()),
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    let mut later_status = status_at_first_horizon;
    later_status.oplog_idx = finished;

    let actual = service
        .lookup_durable_stream_session(&id, AgentMode::Durable, &later_status, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.first_prepared, Some(first));
    assert_eq!(actual.prepared, Some(first));
    assert_eq!(actual.invocation_result, Some(result));
    assert_eq!(actual.finished, Some(finished));
    assert_eq!(actual.session_key, initial.session_key);
    assert_eq!(actual.prepared_attempt_id, initial.prepared_attempt_id);
}

#[test]
fn status_fold_tracks_local_lifecycle_without_retaining_caller_results() {
    use crate::worker::status::update_status_with_new_entries;
    use golem_common::model::oplog::OplogPayload;
    use std::collections::BTreeMap;

    let id = owned_agent("fold", ComponentId::new());
    let key = IdempotencyKey::new("local".into());
    let mut status = AgentStatusRecord::default();
    let mut outgoing_session = session_key(&id, &key);
    outgoing_session.callee.agent_id = "other-callee".into();
    assert!(!status.has_durable_stream_history);
    status = update_status_with_new_entries(
        AgentMode::Durable,
        status,
        BTreeMap::from([(
            OplogIndex::INITIAL,
            OplogEntry::NoOp {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
            },
        )]),
        &RetryConfig::default(),
    )
    .unwrap()
    .unwrap();
    assert!(!status.has_durable_stream_history);
    status = AgentStatusRecord::default();
    let records = [
        StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
            format_version: 1,
            session_key: outgoing_session,
            result: vec![],
            output_streams: vec![],
            stream_mappings: vec![],
        }),
        prepared_record(&id, &key),
        StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &key),
            result: vec![],
            output_streams: vec![],
            stream_mappings: vec![],
        }),
        StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &key),
            result: Ok(()),
        }),
    ];
    for (offset, record) in records.into_iter().enumerate() {
        let entry = OplogEntry::stream_session(
            None,
            OplogPayload::SerializedInline {
                bytes: serialize(&record).unwrap(),
                cached: None,
            },
        );
        status = update_status_with_new_entries(
            AgentMode::Durable,
            status,
            BTreeMap::from([(OplogIndex::from_u64(offset as u64 + 1), entry)]),
            &RetryConfig::default(),
        )
        .unwrap()
        .unwrap();
        if offset == 0 {
            assert!(!status.durable_stream_sessions.has_history());
        }
        assert!(
            status.has_durable_stream_history,
            "caller-side stream records must also disable the streamless fast path"
        );
    }
    assert_eq!(status.durable_stream_sessions.iter().count(), 1);
    let actual = status.durable_stream_sessions.get(&key).unwrap();
    assert_eq!(actual.first_prepared, Some(OplogIndex::from_u64(2)));
    assert_eq!(actual.prepared, Some(OplogIndex::from_u64(2)));
    assert_eq!(actual.invocation_result, Some(OplogIndex::from_u64(3)));
    assert_eq!(actual.finished, Some(OplogIndex::from_u64(4)));
    assert_eq!(actual.session_key, Some(session_key(&id, &key)));
}

#[test]
async fn raw_attachment_authority_fences_before_commit_and_survives_buffer_drain() {
    use crate::durable_host::durable_session::DurableSessionStreams;
    use crate::durable_host::durable_stream::DurableStreamProducer;
    use golem_common::model::durable_stream::{
        ResumeAttemptDescriptorV1, StreamResumeOperationV1, StreamSessionAttachedRecordV1,
        StreamSessionDetachedRecordV1, StreamSessionResumeAttemptRecordV1,
    };

    let (_, _, oplog_service) = service_with_oplog().await;
    let id = owned_agent("raw-authority", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("stream".into());
    let prepared = prepared_record(&id, &key);
    assert!(prepared.has_supported_format());
    let StreamSessionRecordV1::Prepared(prepared_record) = &prepared else {
        unreachable!()
    };
    let first_attempt = prepared_record.attempt.attempt_id;
    let attachment_id = prepared_record.attempt.attachment_id;
    let session = prepared_record.attempt.session_key.clone();
    append_session(oplog.as_ref(), prepared).await;
    let pending_invocation_oplog_index = append_pending_invocation(oplog.as_ref(), &key).await;
    let attached = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
            format_version: 1,
            session_key: session.clone(),
            attachment_id,
            attempt_id: first_attempt,
            epoch: 1,
            pending_invocation_oplog_index,
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;

    let producer = DurableStreamProducer::load(
        oplog.clone(),
        id.environment_id,
        id.agent_id.clone(),
        session.callee_fingerprint,
        None,
    )
    .await
    .unwrap();
    let old = DurableSessionStreams::new(producer.clone(), oplog.clone(), session.clone(), [])
        .with_attachment(1, first_attempt);
    old.ensure_current_attachment().await.unwrap();

    let takeover_attempt = AttemptId::fresh();
    let resume = StreamSessionRecordV1::ResumeAttempt(StreamSessionResumeAttemptRecordV1 {
        format_version: 1,
        attempt: ResumeAttemptDescriptorV1 {
            format_version: 1,
            operation: StreamResumeOperationV1::Takeover,
            session_key: session.clone(),
            attachment_id,
            expected_callee_fingerprint: session.callee_fingerprint,
            attempt_id: takeover_attempt,
            expected_epoch: 1,
            effective_identity: vec![],
            cursors: vec![],
            live_join_buffer_events: 1,
        },
        accepted_epoch: 2,
    });
    assert!(resume.has_supported_format());
    let resumed = append_session(oplog.as_ref(), resume).await;
    assert_eq!(
        oplog_service.get_last_index(&id, AgentMode::Durable).await,
        attached
    );
    let new = DurableSessionStreams::new(producer, oplog.clone(), session.clone(), [])
        .with_attachment(2, takeover_attempt);
    assert!(old.ensure_current_attachment().await.is_err());
    new.ensure_current_attachment().await.unwrap();

    // No Worker/status actor participates: draining the buffer must not reset raw authority
    // back to the older published status used when this oplog was constructed.
    oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        oplog_service.get_last_index(&id, AgentMode::Durable).await,
        resumed
    );
    assert!(old.ensure_current_attachment().await.is_err());
    new.ensure_current_attachment().await.unwrap();
    let raw = oplog.raw_durable_stream_session_status(&session).await;
    assert_eq!(raw.watermark, resumed);
    assert_eq!(raw.status.unwrap().unwrap().attachment_epoch, Some(2));

    append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Detached(StreamSessionDetachedRecordV1 {
            format_version: 1,
            session_key: session.clone(),
            attachment_id,
            owner_attempt_id: takeover_attempt,
            epoch: 2,
        }),
    )
    .await;
    assert!(new.ensure_current_attachment().await.is_err());
    let raw = oplog.raw_durable_stream_session_status(&session).await;
    assert_eq!(
        raw.status.unwrap().unwrap().attachment_attached,
        Some(false)
    );
}

#[test]
async fn raw_cold_reopen_ignores_stale_supplied_status_and_recovers_committed_resume_epoch() {
    let (_worker_service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("raw-cold-reopen", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("stream".into());
    let prepared = prepared_record(&id, &key);
    let StreamSessionRecordV1::Prepared(value) = &prepared else {
        unreachable!()
    };
    let session_key = value.attempt.session_key.clone();
    let attachment_id = value.attempt.attachment_id;
    let first_attempt = value.attempt.attempt_id;
    append_session(oplog.as_ref(), prepared).await;
    let pending = append_pending_invocation(oplog.as_ref(), &key).await;
    append_session(
        oplog.as_ref(),
        attached_record(
            session_key.clone(),
            attachment_id,
            first_attempt,
            1,
            pending,
        ),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;

    let resumed_attempt = AttemptId::fresh();
    let resumed = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::ResumeAttempt(StreamSessionResumeAttemptRecordV1 {
            format_version: 1,
            attempt: ResumeAttemptDescriptorV1 {
                format_version: 1,
                operation: StreamResumeOperationV1::Takeover,
                session_key: session_key.clone(),
                attachment_id,
                expected_callee_fingerprint: session_key.callee_fingerprint,
                attempt_id: resumed_attempt,
                expected_epoch: 1,
                effective_identity: vec![],
                cursors: vec![],
                live_join_buffer_events: 1,
            },
            accepted_epoch: 2,
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    drop(oplog);

    let reopened = oplog_service
        .open(
            &id,
            AgentMode::Durable,
            None,
            agent_metadata(&id),
            stale_status(),
            suspended_status(),
        )
        .await;
    let raw = reopened
        .raw_durable_stream_session_status(&session_key)
        .await;
    assert_eq!(raw.watermark, resumed);
    let status = raw.status.unwrap().unwrap();
    assert_eq!(status.attachment_epoch, Some(2));
    assert_eq!(status.attachment_attempt_id, Some(resumed_attempt));
    assert_eq!(status.attachment_attached, Some(true));
}

#[test]
async fn raw_cached_lookup_observes_takeover_committed_by_another_oplog_actor() {
    let (_worker_service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("raw-shared-authority", ComponentId::new());
    let first = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("stream".into());
    let prepared = prepared_record(&id, &key);
    let StreamSessionRecordV1::Prepared(value) = &prepared else {
        unreachable!()
    };
    let session_key = value.attempt.session_key.clone();
    let attachment_id = value.attempt.attachment_id;
    let first_attempt = value.attempt.attempt_id;
    append_session(first.as_ref(), prepared).await;
    let pending = append_pending_invocation(first.as_ref(), &key).await;
    append_session(
        first.as_ref(),
        attached_record(
            session_key.clone(),
            attachment_id,
            first_attempt,
            1,
            pending,
        ),
    )
    .await;
    first.commit(CommitLevel::Always).await;
    assert_eq!(
        first
            .raw_durable_stream_session_status(&session_key)
            .await
            .status
            .unwrap()
            .unwrap()
            .attachment_epoch,
        Some(1)
    );

    let second = oplog_service
        .open(
            &id,
            AgentMode::Durable,
            None,
            agent_metadata(&id),
            stale_status(),
            suspended_status(),
        )
        .await;
    let takeover_attempt = AttemptId::fresh();
    append_session(
        second.as_ref(),
        StreamSessionRecordV1::ResumeAttempt(StreamSessionResumeAttemptRecordV1 {
            format_version: 1,
            attempt: ResumeAttemptDescriptorV1 {
                format_version: 1,
                operation: StreamResumeOperationV1::Takeover,
                session_key: session_key.clone(),
                attachment_id,
                expected_callee_fingerprint: session_key.callee_fingerprint,
                attempt_id: takeover_attempt,
                expected_epoch: 1,
                effective_identity: vec![],
                cursors: vec![],
                live_join_buffer_events: 1,
            },
            accepted_epoch: 2,
        }),
    )
    .await;
    second.commit(CommitLevel::Always).await;

    let observed = first
        .raw_durable_stream_session_status(&session_key)
        .await
        .status
        .unwrap()
        .unwrap();
    assert_eq!(observed.attachment_epoch, Some(2));
    assert_eq!(observed.attachment_attempt_id, Some(takeover_attempt));
}

#[test]
async fn raw_cache_eviction_recovers_finished_session_and_folds_buffered_then_committed_detach() {
    let (_worker_service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("raw-cache-eviction", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let old_key = IdempotencyKey::new("old".into());
    let prepared = prepared_record(&id, &old_key);
    let StreamSessionRecordV1::Prepared(value) = &prepared else {
        unreachable!()
    };
    let session_key = value.attempt.session_key.clone();
    let attachment_id = value.attempt.attachment_id;
    let attempt_id = value.attempt.attempt_id;
    append_session(oplog.as_ref(), prepared).await;
    let pending = append_pending_invocation(oplog.as_ref(), &old_key).await;
    append_session(
        oplog.as_ref(),
        attached_record(session_key.clone(), attachment_id, attempt_id, 1, pending),
    )
    .await;
    append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
            format_version: 1,
            session_key: session_key.clone(),
            result: Ok(()),
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    assert!(
        oplog
            .raw_durable_stream_session_status(&session_key)
            .await
            .status
            .unwrap()
            .is_some()
    );

    for n in 0..129 {
        let other = self::session_key(&id, &IdempotencyKey::new(format!("evict-{n}")));
        assert!(
            oplog
                .raw_durable_stream_session_status(&other)
                .await
                .status
                .unwrap()
                .is_none()
        );
    }

    let detached = append_session(
        oplog.as_ref(),
        detached_record(session_key.clone(), attachment_id, attempt_id, 1),
    )
    .await;
    let buffered = oplog.raw_durable_stream_session_status(&session_key).await;
    assert_eq!(buffered.watermark, detached);
    let buffered = buffered.status.unwrap().unwrap();
    assert!(
        buffered.finished.is_some(),
        "evicted committed history was lost"
    );
    assert_eq!(buffered.attachment_attached, Some(false));

    oplog.commit(CommitLevel::Always).await;
    let committed = oplog
        .raw_durable_stream_session_status(&session_key)
        .await
        .status
        .unwrap()
        .unwrap();
    assert!(committed.finished.is_some());
    assert_eq!(committed.attachment_attached, Some(false));
}

#[test]
async fn persisted_exact_horizon_rejects_newer_index_and_offsets_hide_newer_attachment() {
    let (service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("persisted-horizon", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("stream".into());
    let prepared = prepared_record(&id, &key);
    let StreamSessionRecordV1::Prepared(value) = &prepared else {
        unreachable!()
    };
    let session_key = value.attempt.session_key.clone();
    let attachment_id = value.attempt.attachment_id;
    let attempt_id = value.attempt.attempt_id;
    let prepared_idx = append_session(oplog.as_ref(), prepared).await;
    let attached_idx = append_session(
        oplog.as_ref(),
        attached_record(
            session_key,
            attachment_id,
            attempt_id,
            1,
            OplogIndex::INITIAL,
        ),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    service
        .stream_session_index
        .catch_up(&id, AgentMode::Durable, attached_idx)
        .await
        .unwrap();

    let error = service
        .stream_session_index
        .lookup_persisted(&id, AgentMode::Durable, prepared_idx, &key)
        .await
        .unwrap_err();
    assert!(error.contains("newer than the requested horizon"));
    let offsets = service
        .stream_session_index
        .lookup_persisted_offsets(&id, AgentMode::Durable, prepared_idx, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(offsets.prepared, Some(prepared_idx));
    assert_eq!(offsets.session_key, None);
    assert_eq!(offsets.prepared_attempt_id, None);
    assert_eq!(offsets.initial_attachment_epoch, None);
    assert_eq!(offsets.initial_attachment_attempt_id, None);
    assert_eq!(offsets.initial_pending_invocation_oplog_index, None);
    assert_eq!(offsets.attachment_epoch, None);
    assert_eq!(offsets.attachment_attempt_id, None);
    assert_eq!(offsets.attachment_attached, None);
    assert_eq!(offsets.lifecycle_error, None);
}

#[test]
async fn raw_lookup_catches_up_archived_history_after_full_multilayer_reopen() {
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let blobs = Arc::new(InMemoryBlobStorage::new());
    let kv = Arc::new(InMemoryKeyValueStorage::new());
    let id = owned_agent("raw-archived-reopen", ComponentId::new());

    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            blobs.clone(),
            1,
            1,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let secondary: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed.clone(),
        1,
        RetryConfig::default(),
    ));
    let tertiary: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed.clone(),
        2,
        RetryConfig::default(),
    ));
    let multilayer = Arc::new(MultiLayerOplogService::new(
        primary,
        nev![secondary, tertiary],
        100,
        100,
    ));
    let worker_service = DefaultWorkerService::new(
        kv.clone(),
        Arc::new(ShardServiceDefault::new()),
        multilayer.clone(),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    let oplog = create_oplog(multilayer.as_ref(), &id).await;
    let key = IdempotencyKey::new("archived".into());
    let prepared = prepared_record(&id, &key);
    let session_key = session_key(&id, &key);
    let prepared_idx = append_session(oplog.as_ref(), prepared).await;
    let finished_idx = append_session(
        oplog.as_ref(),
        StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
            format_version: 1,
            session_key: session_key.clone(),
            result: Ok(()),
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        MultiLayerOplog::try_archive_blocking(&oplog).await,
        Some(true)
    );
    assert_eq!(
        MultiLayerOplog::try_archive_blocking(&oplog).await,
        Some(false)
    );
    drop(oplog);
    drop(worker_service);
    drop(multilayer);

    let primary = Arc::new(
        PrimaryOplogService::new(indexed.clone(), blobs, 1, 1, 100, RetryConfig::default()).await,
    );
    let secondary: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed.clone(),
        1,
        RetryConfig::default(),
    ));
    let tertiary: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed,
        2,
        RetryConfig::default(),
    ));
    let reopened_service = Arc::new(MultiLayerOplogService::new(
        primary,
        nev![secondary, tertiary],
        100,
        100,
    ));
    let _worker_service = DefaultWorkerService::new(
        kv,
        Arc::new(ShardServiceDefault::new()),
        reopened_service.clone(),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    let reopened = reopened_service
        .open(
            &id,
            AgentMode::Durable,
            None,
            agent_metadata(&id),
            stale_status(),
            suspended_status(),
        )
        .await;
    let raw = reopened
        .raw_durable_stream_session_status(&session_key)
        .await;
    assert_eq!(raw.watermark, finished_idx);
    let status = raw.status.unwrap().unwrap();
    assert_eq!(status.first_prepared, Some(prepared_idx));
    assert_eq!(status.finished, Some(finished_idx));
    assert_eq!(status.session_key, Some(session_key));
}

#[test]
async fn indexed_raw_authority_cold_and_warm_lookups_do_not_read_oplog_history() {
    use crate::services::oplog::tests::ReadCountingIndexedStorage;

    let storage = Arc::new(ReadCountingIndexedStorage::new());
    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            storage.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            10000,
            10000,
            100,
            RetryConfig::default(),
        )
        .await,
    );
    let service = DefaultWorkerService::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        Arc::new(ShardServiceDefault::new()),
        oplog_service.clone(),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    let id = owned_agent("raw-indexed-reads", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("session".into());
    let prepared = prepared_record(&id, &key);
    let StreamSessionRecordV1::Prepared(value) = &prepared else {
        unreachable!()
    };
    let session = value.attempt.session_key.clone();
    let attachment = value.attempt.attachment_id;
    let attempt = value.attempt.attempt_id;
    append_session(oplog.as_ref(), prepared).await;
    let pending = append_pending_invocation(oplog.as_ref(), &key).await;
    append_session(
        oplog.as_ref(),
        attached_record(session.clone(), attachment, attempt, 1, pending),
    )
    .await;
    for _ in 0..2048 {
        append_noop(oplog.as_ref()).await;
    }
    oplog.commit(CommitLevel::Always).await;
    let horizon = oplog.current_oplog_index().await;
    service
        .stream_session_index
        .catch_up(&id, AgentMode::Durable, horizon)
        .await
        .unwrap();
    drop(oplog);
    let reopened = oplog_service
        .open(
            &id,
            AgentMode::Durable,
            None,
            agent_metadata(&id),
            stale_status(),
            suspended_status(),
        )
        .await;
    storage.reset();
    assert_eq!(
        reopened
            .raw_durable_stream_session_status(&session)
            .await
            .status
            .unwrap()
            .unwrap()
            .attachment_epoch,
        Some(1)
    );
    assert_eq!(
        storage.reads(),
        0,
        "cold authority must use the persisted projection"
    );
    append_session(
        reopened.as_ref(),
        detached_record(session.clone(), attachment, attempt, 1),
    )
    .await;
    for _ in 0..20 {
        assert_eq!(
            reopened
                .raw_durable_stream_session_status(&session)
                .await
                .status
                .unwrap()
                .unwrap()
                .attachment_attached,
            Some(false)
        );
    }
    assert_eq!(
        storage.reads(),
        0,
        "warm authority must fold raw appends without storage reads"
    );
}

#[test]
async fn raw_authority_ignores_foreign_results_sharing_an_idempotency_key() {
    let (_worker_service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("raw-shared-idempotency-key", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let idempotency_key = IdempotencyKey::new("shared".into());
    let first = prepared_record(&id, &idempotency_key);
    let StreamSessionRecordV1::Prepared(first_value) = &first else {
        unreachable!()
    };
    let first_key = first_value.attempt.session_key.clone();
    let remote = owned_agent("remote-callee", ComponentId::new());
    let second_key = session_key(&remote, &idempotency_key);
    let second = StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
        format_version: 1,
        session_key: second_key.clone(),
        result: vec![],
        output_streams: vec![],
        stream_mappings: vec![],
    });
    assert!(first.has_supported_format());
    assert!(second.has_supported_format());
    assert_ne!(first_key, second_key);

    append_session(oplog.as_ref(), second.clone()).await;
    append_session(oplog.as_ref(), first).await;
    append_session(oplog.as_ref(), second).await;
    oplog.commit(CommitLevel::Always).await;

    assert!(
        oplog
            .raw_durable_stream_session_status(&first_key)
            .await
            .status
            .unwrap()
            .is_some()
    );
    assert!(
        oplog
            .raw_durable_stream_session_status(&second_key)
            .await
            .status
            .unwrap()
            .is_none(),
        "a caller-side result must not establish local session authority"
    );
}
