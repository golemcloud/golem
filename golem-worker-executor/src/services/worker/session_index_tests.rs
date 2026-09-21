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
    AttachmentId, AttemptId, LocalStreamReaderId, PersistedStreamInvocationDescriptor,
    ResumeAttemptDescriptor, StartAttemptDescriptor, StreamInvocationId,
    StreamRegistrationInvocation, StreamResumeOperation, StreamSessionAttachedRecord,
    StreamSessionDetachedRecord, StreamSessionFinishedRecord, StreamSessionInvocationResultRecord,
    StreamSessionPreparedRecord, StreamSessionRecord, StreamSessionResumeAttemptRecord,
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

fn local_reader(introducing_oplog_index: OplogIndex, binding_slot: u32) -> LocalStreamReaderId {
    LocalStreamReaderId {
        introducing_oplog_index,
        binding_slot,
    }
}

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

fn local_registration(key: &StreamInvocationId) -> StreamRegistrationInvocation {
    StreamRegistrationInvocation::Local(key.idempotency_key.clone())
}

fn remote_registration(key: &StreamInvocationId) -> StreamRegistrationInvocation {
    StreamRegistrationInvocation::Remote(key.clone())
}

#[test]
async fn cancellation_receipts_prune_recovery_catalogue_after_cold_reopen() {
    use golem_common::model::durable_stream::*;

    let (service, kv, oplog_service) = service_with_oplog().await;
    let owner = owned_agent("cancellation-consumer", ComponentId::new());
    let remote = owned_agent("cancellation-producer", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &owner).await;
    let key = session_key(&remote, &IdempotencyKey::new("cancel-session".into()));
    let handle = DurableStreamHandle {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        stream_id: StreamId(uuid::Uuid::new_v4()),
        producer_environment_id: remote.environment_id,
        producer: remote.agent_id.clone(),
        expected_producer_fingerprint: key.callee_fingerprint,
        producer_generation: OplogIndex::NONE,
        source_invocation: key.clone(),
        component_revision: ComponentRevision::INITIAL,
        element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([0; 32]),
    };
    let intent = StreamConsumerCancelIntentRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        session_key: remote_registration(&key),
        consumer_invocation: key.idempotency_key.clone(),
        source: StreamRecordReference::Foreign(handle),
        epoch: 3,
        role: StreamCancelRole::OutputConsumer,
        reason: StreamCancelReason::Cancelled,
        details: Some("external cancellation".into()),
    };
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::ConsumerCancelIntent(intent.clone()),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        service
            .lookup_durable_stream_recovery_metadata(&owner, AgentMode::Durable)
            .await
            .unwrap()
            .sessions
            .len(),
        1,
        "caller-side cancellation without Prepared must remain discoverable"
    );

    let mut wrong_intent = intent.clone();
    wrong_intent.epoch = 4;
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            intent: wrong_intent,
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        service
            .lookup_durable_stream_recovery_metadata(&owner, AgentMode::Durable)
            .await
            .unwrap()
            .sessions
            .len(),
        1,
        "a receipt from another epoch must not retire the original obligation"
    );

    append_session(
        oplog.as_ref(),
        StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            intent: intent.clone(),
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    assert!(
        service
            .lookup_durable_stream_recovery_metadata(&owner, AgentMode::Durable)
            .await
            .unwrap()
            .sessions
            .is_empty()
    );
    drop(service);
    let reopened = DefaultWorkerService::new(
        kv,
        Arc::new(ShardServiceDefault::new()),
        oplog_service,
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    assert!(
        reopened
            .lookup_durable_stream_recovery_metadata(&owner, AgentMode::Durable)
            .await
            .unwrap()
            .sessions
            .is_empty(),
        "cold recovery must not resurrect an acknowledged cancellation"
    );
    let history = reopened
        .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &key)
        .await
        .unwrap();
    assert!(!history.has_cancellation_intents());
    assert!(!history.needs_recovery(&owner, &key));
}

#[test]
async fn committed_cancellation_probe_preserves_exact_authority_after_takeover() {
    use crate::durable_host::durable_stream::{
        ConsumerAttachmentStatus, DbDirectStreamAttachmentConsumerProbe,
        StreamAttachmentConsumerProbe,
    };
    use golem_common::model::durable_stream::*;
    use golem_schema::schema::SchemaFingerprintV1;

    let (service, _, oplog_service) = service_with_oplog().await;
    let owner = owned_agent("cancel-authority", ComponentId::new());
    let remote = owned_agent("cancel-source", ComponentId::new());
    let key = session_key(&owner, &IdempotencyKey::new("session".into()));
    let mut metadata = agent_metadata(&owner);
    metadata.fingerprint = key.callee_fingerprint;
    let oplog = oplog_service
        .create_fresh(
            &mut oplog_service.lock_lifecycle(&owner.agent_id).await,
            &owner,
            AgentMode::Durable,
            OplogEntry::create(
                owner.agent_id.clone(),
                golem_common::model::agent::OwnerKind::ComponentAgent,
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                owner.environment_id,
                metadata.created_by,
                None,
                100,
                100,
                Default::default(),
                vec![],
                None,
                key.callee_fingerprint.0,
            ),
            metadata,
            stale_status(),
            suspended_status(),
        )
        .await;
    let mapping = StreamSessionMappingRecord {
        transport_stream_id: 7,
        role: SessionStreamRole::Output,
        handle: DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(uuid::Uuid::new_v4()),
            producer_environment_id: remote.environment_id,
            producer: remote.agent_id.clone(),
            expected_producer_fingerprint: AgentFingerprint(remote.agent_id.component_id.0),
            producer_generation: OplogIndex::NONE,
            source_invocation: session_key(&remote, &IdempotencyKey::new("source".into())),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([0; 32]),
        },
    };
    let StreamSessionRecord::Prepared(prepared) = prepared_record(&owner, &key.idempotency_key)
    else {
        unreachable!()
    };
    let attachment_id = prepared.attempt.attachment_id;
    let attempt_id = prepared.attempt.attempt_id;
    append_session(oplog.as_ref(), StreamSessionRecord::Prepared(prepared)).await;
    let pending = append_pending_invocation(oplog.as_ref(), &key.idempotency_key).await;
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: key.idempotency_key.clone(),
            attachment_id,
            attempt_id,
            epoch: 1,
            pending_invocation_oplog_index: pending,
        }),
    )
    .await;
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
            format_version: 1,
            session_key: local_registration(&key),
            mapping: StreamBindingRecord::foreign(&mapping),
        }),
    )
    .await;
    let attachment = StreamAttachmentKey {
        attachment_id,
        stream_id: mapping.handle.stream_id,
        epoch: 1,
        session_key: key.clone(),
        producer_environment_id: remote.environment_id,
        producer: remote.agent_id.clone(),
        expected_producer_fingerprint: mapping.handle.expected_producer_fingerprint,
        consumer_environment_id: owner.environment_id,
        consumer: owner.agent_id.clone(),
        expected_consumer_fingerprint: key.callee_fingerprint,
        consumer_invocation: key.clone(),
    };
    let intent = StreamConsumerCancelIntentRecord {
        format_version: 1,
        session_key: remote_registration(&key),
        consumer_invocation: key.idempotency_key.clone(),
        source: StreamRecordReference::Foreign(mapping.handle.clone()),
        epoch: 1,
        role: StreamCancelRole::OutputConsumer,
        reason: StreamCancelReason::Cancelled,
        details: Some("committed external cancellation".into()),
    };
    oplog.commit(CommitLevel::Always).await;
    let probe =
        DbDirectStreamAttachmentConsumerProbe::new(Arc::new(service), oplog_service.clone());
    assert_eq!(
        probe
            .committed_cancellation_status(&attachment, &mapping, &intent)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Missing
    );
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::ConsumerCancelIntent(StreamConsumerCancelIntentRecord {
            session_key: local_registration(&key),
            ..intent.clone()
        }),
    )
    .await;
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::ResumeAttempt(StreamSessionResumeAttemptRecord {
            format_version: 1,
            session_key: key.idempotency_key.clone(),
            attempt: ResumeAttemptDescriptor {
                format_version: 1,
                operation: StreamResumeOperation::Takeover,
                session_key: key.clone(),
                attachment_id,
                expected_callee_fingerprint: key.callee_fingerprint,
                attempt_id: AttemptId::fresh(),
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
    assert_eq!(
        probe
            .status_exact(&attachment, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::EpochMismatch
    );
    let foreign = owned_agent("independent-consumer", ComponentId::new());
    let consumer_invocation = session_key(&foreign, &IdempotencyKey::new("consumer".into()));
    let mut foreign_metadata = agent_metadata(&foreign);
    foreign_metadata.fingerprint = consumer_invocation.callee_fingerprint;
    let foreign_oplog = oplog_service
        .create_fresh(
            &mut oplog_service.lock_lifecycle(&foreign.agent_id).await,
            &foreign,
            AgentMode::Durable,
            OplogEntry::create(
                foreign.agent_id.clone(),
                golem_common::model::agent::OwnerKind::ComponentAgent,
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                foreign.environment_id,
                foreign_metadata.created_by,
                None,
                100,
                100,
                Default::default(),
                vec![],
                None,
                consumer_invocation.callee_fingerprint.0,
            ),
            foreign_metadata,
            stale_status(),
            suspended_status(),
        )
        .await;
    let foreign_attachment = StreamAttachmentKey {
        consumer_environment_id: foreign.environment_id,
        consumer: foreign.agent_id.clone(),
        expected_consumer_fingerprint: consumer_invocation.callee_fingerprint,
        consumer_invocation,
        ..attachment.clone()
    };
    for record in [
        StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
            format_version: 1,
            session_key: key.clone(),
            attachment: foreign_attachment.clone(),
            mapping: mapping.clone(),
        }),
        StreamSessionRecord::TopologyActivated(StreamTopologyActivatedRecord {
            format_version: 1,
            session_key: key.clone(),
            attachment: foreign_attachment.clone(),
            mapping: mapping.clone(),
        }),
    ] {
        append_session(foreign_oplog.as_ref(), record).await;
    }
    foreign_oplog.commit(CommitLevel::Always).await;
    assert_eq!(
        probe
            .status_exact(&foreign_attachment, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    let mut wrong_foreign_epoch = foreign_attachment;
    wrong_foreign_epoch.epoch = 2;
    assert_eq!(
        probe
            .status_exact(&wrong_foreign_epoch, Some(&mapping))
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Missing
    );
    assert_eq!(
        probe
            .committed_cancellation_status(&attachment, &mapping, &intent)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Active
    );
    let mut changed = intent.clone();
    changed.details = Some("unrecorded cancellation".into());
    assert_eq!(
        probe
            .committed_cancellation_status(&attachment, &mapping, &changed)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Missing
    );
    let mut changed_mapping = mapping.clone();
    changed_mapping.transport_stream_id = 8;
    assert_eq!(
        probe
            .committed_cancellation_status(&attachment, &changed_mapping, &intent)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::Missing
    );
    let mut changed_consumer = attachment.clone();
    changed_consumer.expected_consumer_fingerprint = AgentFingerprint::new();
    changed_consumer.consumer_invocation.callee_fingerprint =
        changed_consumer.expected_consumer_fingerprint;
    assert_eq!(
        probe
            .committed_cancellation_status(&changed_consumer, &mapping, &intent)
            .await
            .unwrap(),
        ConsumerAttachmentStatus::IncarnationMismatch
    );
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

fn session_key(id: &OwnedAgentId, key: &IdempotencyKey) -> StreamInvocationId {
    StreamInvocationId {
        callee_environment_id: id.environment_id,
        callee: id.agent_id.clone(),
        callee_fingerprint: AgentFingerprint(id.agent_id.component_id.0),
        idempotency_key: key.clone(),
    }
}

fn prepared_record(id: &OwnedAgentId, key: &IdempotencyKey) -> StreamSessionRecord {
    let session_key = session_key(id, key);
    StreamSessionRecord::Prepared(StreamSessionPreparedRecord {
        format_version: 1,
        session_key: key.clone(),
        attempt: StartAttemptDescriptor {
            format_version: 1,
            session_key: session_key.clone(),
            attachment_id: AttachmentId::primary(id.environment_id, &id.agent_id, key).unwrap(),
            expected_callee_fingerprint: session_key.callee_fingerprint,
            attempt_id: AttemptId(uuid::Uuid::new_v4()),
            invocation: PersistedStreamInvocationDescriptor {
                format_version: 1,
                session_key,
                target_component_revision: ComponentRevision::INITIAL,
                target: golem_common::base_model::durable_stream::PersistedInvocationTarget::AgentMethod {
                    method_name: "test".into(),
                },
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

fn prepared_with_reader(id: &OwnedAgentId, key: &IdempotencyKey) -> StreamSessionRecord {
    let StreamSessionRecord::Prepared(mut record) = prepared_record(id, key) else {
        unreachable!()
    };
    let mut source = id.clone();
    source.agent_id.agent_id = "remote-reader-source".into();
    let source_session = session_key(&source, key);
    let handle = golem_common::model::durable_stream::DurableStreamHandle {
        format_version: 1,
        stream_id: golem_common::model::durable_stream::StreamId(uuid::Uuid::from_u128(1234)),
        producer_environment_id: source.environment_id,
        producer: source.agent_id,
        expected_producer_fingerprint: source_session.callee_fingerprint,
        producer_generation: OplogIndex::NONE,
        source_invocation: source_session,
        component_revision: ComponentRevision::INITIAL,
        element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([0; 32]),
    };
    record
        .attempt
        .invocation
        .stream_handles
        .push(handle.clone());
    record.stream_mappings.push(
        golem_common::model::durable_stream::StreamBindingRecord::foreign(
            &golem_common::model::durable_stream::StreamSessionMappingRecord {
                transport_stream_id: 0,
                handle,
                role: golem_common::model::durable_stream::SessionStreamRole::Input,
            },
        ),
    );
    StreamSessionRecord::Prepared(record)
}

fn agent_metadata(id: &OwnedAgentId) -> AgentMetadata {
    AgentMetadata {
        agent_id: id.agent_id.clone(),
        owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
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
    let mut metadata = agent_metadata(id);
    metadata.fingerprint = AgentFingerprint(id.agent_id.component_id.0);
    service
        .create_fresh(
            &mut service.lock_lifecycle(&id.agent_id).await,
            id,
            AgentMode::Durable,
            OplogEntry::create(
                id.agent_id.clone(),
                golem_common::model::agent::OwnerKind::ComponentAgent,
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                id.environment_id,
                metadata.created_by,
                None,
                100,
                100,
                Default::default(),
                vec![],
                None,
                metadata.fingerprint.0,
            ),
            metadata,
            stale_status(),
            suspended_status(),
        )
        .await
}

async fn append_session(oplog: &dyn Oplog, record: StreamSessionRecord) -> OplogIndex {
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

#[test]
#[test_r::timeout("30s")]
async fn recovery_cache_refolds_a_cut_committed_after_its_snapshot() {
    use golem_common::model::durable_stream::StreamForkCutRecord;
    use golem_common::model::regions::OplogRegion;

    let oplog_service = Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1000,
            1000,
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
    let owner = owned_agent("cached-cut", ComponentId::new());
    let key = IdempotencyKey::new("discarded".into());
    let fingerprint = session_key(&owner, &key).callee_fingerprint;
    let oplog = create_oplog(oplog_service.as_ref(), &owner).await;
    append_session(oplog.as_ref(), prepared_record(&owner, &key)).await;
    oplog.commit(CommitLevel::Always).await;
    let mut cache = crate::worker::DurableTopologyRecoveryCache::default();
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(cache.sessions.len(), 1);
    let region = OplogRegion {
        start: OplogIndex::INITIAL.next(),
        end: oplog.current_oplog_index().await,
    };
    let marker = DurableStreamOplogRecord::Session(
        None,
        Box::new(StreamSessionRecord::ForkCut(StreamForkCutRecord {
            format_version: 1,
            request_hash: vec![0; 32],
            creation_fingerprint: fingerprint,
            export: None,
            cut_index: OplogIndex::INITIAL,
            revert: Some(region.clone()),
            epoch_floor: 2,
            selected_stream_id: None,
            retained_through: None,
        })),
    )
    .into_inline_entry();
    oplog
        .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
        .await;
    // An uncommitted cut must fail closed, rather than repeatedly reloading an old index.
    assert!(
        cache
            .refresh(
                oplog.as_ref(),
                &service,
                &owner,
                AgentMode::Durable,
                fingerprint
            )
            .await
            .is_err()
    );
    oplog.commit(CommitLevel::Always).await;
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            fingerprint,
        )
        .await
        .unwrap();
    assert!(cache.sessions.is_empty());
    assert!(cache.dirty.is_empty());
    append_session(oplog.as_ref(), prepared_record(&owner, &key)).await;
    cache
        .refresh(
            oplog.as_ref(),
            &service,
            &owner,
            AgentMode::Durable,
            fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(cache.sessions.len(), 1);
    assert_eq!(cache.dirty.len(), 1);
}

#[test]
#[test_r::timeout("30s")]
async fn reverted_session_is_absent_from_warm_and_cold_indexes_across_empty_chunks() {
    use golem_common::model::durable_stream::StreamForkCutRecord;
    use golem_common::model::regions::OplogRegion;

    for warm in [false, true] {
        let (service, _, oplog_service) = service_with_oplog().await;
        let id = owned_agent("reverted-session", ComponentId::new());
        let key = IdempotencyKey::new("discarded".into());
        let fingerprint = session_key(&id, &key).callee_fingerprint;
        let mut metadata = agent_metadata(&id);
        metadata.fingerprint = fingerprint;
        let oplog = oplog_service
            .create_fresh(
                &mut oplog_service.lock_lifecycle(&id.agent_id).await,
                &id,
                AgentMode::Durable,
                OplogEntry::create(
                    id.agent_id.clone(),
                    golem_common::model::agent::OwnerKind::ComponentAgent,
                    AgentMode::Durable,
                    ComponentRevision::INITIAL,
                    vec![],
                    id.environment_id,
                    metadata.created_by,
                    None,
                    100,
                    100,
                    Default::default(),
                    vec![],
                    None,
                    fingerprint.0,
                ),
                metadata,
                stale_status(),
                suspended_status(),
            )
            .await;
        append_session(oplog.as_ref(), prepared_record(&id, &key)).await;
        for _ in 0..2050 {
            append_noop(oplog.as_ref()).await;
        }
        oplog.commit(CommitLevel::Always).await;
        let end = oplog.current_oplog_index().await;
        if warm {
            assert!(
                service
                    .stream_session_index
                    .lookup_persisted(&id, AgentMode::Durable, end, &key,)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        let region = OplogRegion {
            start: OplogIndex::INITIAL.next(),
            end,
        };
        let cut = StreamSessionRecord::ForkCut(StreamForkCutRecord {
            format_version: 1,
            request_hash: vec![0; 32],
            creation_fingerprint: fingerprint,
            export: None,
            cut_index: OplogIndex::INITIAL,
            revert: Some(region.clone()),
            epoch_floor: 2,
            selected_stream_id: None,
            retained_through: None,
        });
        let marker_entry =
            DurableStreamOplogRecord::Session(None, Box::new(cut)).into_inline_entry();
        oplog
            .add_pair(OplogEntry::revert(region), Box::new(move |_| marker_entry))
            .await;
        oplog.commit(CommitLevel::Always).await;
        let marker = oplog.current_oplog_index().await;
        assert!(
            service
                .stream_session_index
                .lookup_persisted(&id, AgentMode::Durable, marker, &key,)
                .await
                .unwrap()
                .is_none(),
            "deleted session survived (warm={warm})"
        );
        assert!(
            service
                .lookup_durable_stream_recovery_metadata(&id, AgentMode::Durable)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );

        // The same invocation key can be accepted again after its old preparation is deleted.
        let prepared = append_session(oplog.as_ref(), prepared_record(&id, &key)).await;
        oplog.commit(CommitLevel::Always).await;
        let state = service
            .stream_session_index
            .lookup_persisted(&id, AgentMode::Durable, prepared, &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.first_prepared, Some(prepared));
        assert_eq!(
            service
                .lookup_durable_stream_recovery_metadata(&id, AgentMode::Durable)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
    }
}

#[test]
#[test_r::timeout("30s")]
async fn self_revert_retains_foreign_consumer_prefix_across_partial_page_and_replaces_tail() {
    use golem_common::model::durable_stream::{
        StreamConsumerItemValueRecord, StreamForkCutRecord, StreamOffset,
    };
    use golem_common::model::regions::OplogRegion;

    let (service, kv, oplog_service) = service_with_oplog().await;
    let owner = owned_agent("consumer-journal-owner", ComponentId::new());
    let mut foreign = owner.clone();
    foreign.agent_id.agent_id = "source-journal-owner".into();
    let local_key = IdempotencyKey::new("original-rpc".into());
    let local_session = session_key(&owner, &local_key);
    let foreign_session = session_key(&foreign, &local_key);
    let fingerprint = local_session.callee_fingerprint;
    let mut metadata = agent_metadata(&owner);
    metadata.fingerprint = fingerprint;
    let oplog = oplog_service
        .create_fresh(
            &mut oplog_service.lock_lifecycle(&owner.agent_id).await,
            &owner,
            AgentMode::Durable,
            OplogEntry::create(
                owner.agent_id.clone(),
                golem_common::model::agent::OwnerKind::ComponentAgent,
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                owner.environment_id,
                metadata.created_by,
                None,
                100,
                100,
                Default::default(),
                vec![],
                None,
                fingerprint.0,
            ),
            metadata,
            stale_status(),
            suspended_status(),
        )
        .await;
    append_session(
        oplog.as_ref(),
        prepared_with_reader(&foreign, &foreign_session.idempotency_key),
    )
    .await;
    let reader = local_reader(OplogIndex::from_u64(2), 0);
    let mut retained = Vec::new();
    for ordinal in 0..259 {
        retained.push(
            append_session(
                oplog.as_ref(),
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                    format_version: 1,
                    session_key: local_registration(&foreign_session),
                    reader_id: reader,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(ordinal + 1), 0),
                    consumer_read_ordinal: ordinal,
                    value: vec![ordinal as u8],
                    packed_u8: true,
                    recursive_mappings: vec![],
                }),
            )
            .await,
        );
    }
    let fork = append_session(
        oplog.as_ref(),
        StreamSessionRecord::ForkCut(StreamForkCutRecord {
            format_version: 1,
            request_hash: vec![0; 32],
            creation_fingerprint: fingerprint,
            export: None,
            cut_index: oplog.current_oplog_index().await,
            revert: None,
            epoch_floor: 1,
            selected_stream_id: None,
            retained_through: None,
        }),
    )
    .await;
    let mut removed = Vec::new();
    for ordinal in 259..262 {
        removed.push(
            append_session(
                oplog.as_ref(),
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                    format_version: 1,
                    session_key: local_registration(&local_session),
                    reader_id: reader,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(ordinal + 1), 0),
                    consumer_read_ordinal: ordinal,
                    value: vec![ordinal as u8],
                    packed_u8: true,
                    recursive_mappings: vec![],
                }),
            )
            .await,
        );
    }
    oplog.commit(CommitLevel::Always).await;

    let region = OplogRegion {
        start: removed[0],
        end: *removed.last().unwrap(),
    };
    service
        .stream_session_index
        .catch_up(&owner, AgentMode::Durable, region.end)
        .await
        .unwrap();
    assert_eq!(
        service
            .read_durable_stream_consumer_page(&owner, &local_session, reader, 1)
            .await
            .unwrap(),
        retained[256..]
            .iter()
            .chain(&removed)
            .copied()
            .collect::<Vec<_>>()
    );
    let cut = StreamSessionRecord::ForkCut(StreamForkCutRecord {
        format_version: 1,
        request_hash: vec![1; 32],
        creation_fingerprint: fingerprint,
        export: None,
        cut_index: fork,
        revert: Some(region.clone()),
        epoch_floor: 2,
        selected_stream_id: None,
        retained_through: None,
    });
    let marker = DurableStreamOplogRecord::Session(None, Box::new(cut)).into_inline_entry();
    oplog
        .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
        .await;
    oplog.commit(CommitLevel::Always).await;

    let expected_second_page = retained[256..].to_vec();
    service
        .stream_session_index
        .catch_up(
            &owner,
            AgentMode::Durable,
            oplog.current_oplog_index().await,
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .read_durable_stream_consumer_page(&owner, &local_session, reader, 1)
            .await
            .unwrap(),
        expected_second_page
    );
    let cold = DefaultWorkerService::new(
        kv,
        Arc::new(ShardServiceDefault::new()),
        oplog_service,
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    assert_eq!(
        cold.read_durable_stream_consumer_page(&owner, &local_session, reader, 1)
            .await
            .unwrap(),
        expected_second_page
    );
    let replacement = append_session(
        oplog.as_ref(),
        StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
            format_version: 1,
            session_key: local_registration(&local_session),
            reader_id: reader,
            source_offset: StreamOffset::new(OplogIndex::from_u64(260), 0),
            consumer_read_ordinal: 259,
            value: vec![99],
            packed_u8: true,
            recursive_mappings: vec![],
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;
    cold.stream_session_index
        .catch_up(
            &owner,
            AgentMode::Durable,
            oplog.current_oplog_index().await,
        )
        .await
        .unwrap();
    assert_eq!(
        cold.read_durable_stream_consumer_page(&owner, &local_session, reader, 1)
            .await
            .unwrap(),
        retained[256..]
            .iter()
            .copied()
            .chain(std::iter::once(replacement))
            .collect::<Vec<_>>()
    );
}

#[test]
#[test_r::timeout("30s")]
async fn concurrent_index_services_refold_same_paired_revert_without_stale_rows() {
    use golem_common::model::durable_stream::{
        StreamConsumerItemValueRecord, StreamForkCutRecord, StreamOffset,
    };
    use golem_common::model::regions::OplogRegion;

    let (first, kv, oplog_service) = service_with_oplog().await;
    let second = DefaultWorkerService::new(
        kv.clone(),
        Arc::new(ShardServiceDefault::new()),
        oplog_service.clone(),
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    let owner = owned_agent("concurrent-refold", ComponentId::new());
    let key = IdempotencyKey::new("session".into());
    let session = session_key(&owner, &key);
    let fingerprint = session.callee_fingerprint;
    let mut metadata = agent_metadata(&owner);
    metadata.fingerprint = fingerprint;
    let oplog = oplog_service
        .create_fresh(
            &mut oplog_service.lock_lifecycle(&owner.agent_id).await,
            &owner,
            AgentMode::Durable,
            OplogEntry::create(
                owner.agent_id.clone(),
                golem_common::model::agent::OwnerKind::ComponentAgent,
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                owner.environment_id,
                metadata.created_by,
                None,
                100,
                100,
                Default::default(),
                vec![],
                None,
                fingerprint.0,
            ),
            metadata,
            stale_status(),
            suspended_status(),
        )
        .await;
    append_session(oplog.as_ref(), prepared_with_reader(&owner, &key)).await;
    let reader = local_reader(OplogIndex::from_u64(2), 0);
    let mut retained = Vec::new();
    for ordinal in 0..259 {
        retained.push(
            append_session(
                oplog.as_ref(),
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                    format_version: 1,
                    session_key: local_registration(&session),
                    reader_id: reader,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(ordinal + 1), 0),
                    consumer_read_ordinal: ordinal,
                    value: vec![ordinal as u8],
                    packed_u8: true,
                    recursive_mappings: vec![],
                }),
            )
            .await,
        );
    }
    oplog.commit(CommitLevel::Always).await;
    let horizon = oplog.current_oplog_index().await;
    let (warm_first, warm_second) = tokio::join!(
        first
            .stream_session_index
            .lookup_persisted(&owner, AgentMode::Durable, horizon, &key),
        second
            .stream_session_index
            .lookup_persisted(&owner, AgentMode::Durable, horizon, &key),
    );
    assert!(warm_first.unwrap().is_some());
    assert!(warm_second.unwrap().is_some());
    let mut removed = Vec::new();
    for ordinal in 259..262 {
        removed.push(
            append_session(
                oplog.as_ref(),
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                    format_version: 1,
                    session_key: local_registration(&session),
                    reader_id: reader,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(ordinal + 1), 0),
                    consumer_read_ordinal: ordinal,
                    value: vec![ordinal as u8],
                    packed_u8: true,
                    recursive_mappings: vec![],
                }),
            )
            .await,
        );
    }
    let region = OplogRegion {
        start: removed[0],
        end: *removed.last().unwrap(),
    };
    oplog.commit(CommitLevel::Always).await;
    first
        .stream_session_index
        .catch_up(&owner, AgentMode::Durable, region.end)
        .await
        .unwrap();
    assert_eq!(
        first
            .read_durable_stream_consumer_page(&owner, &session, reader, 1)
            .await
            .unwrap(),
        retained[256..]
            .iter()
            .chain(&removed)
            .copied()
            .collect::<Vec<_>>()
    );
    let cut = StreamSessionRecord::ForkCut(StreamForkCutRecord {
        format_version: 1,
        request_hash: vec![2; 32],
        creation_fingerprint: fingerprint,
        export: None,
        cut_index: horizon,
        revert: Some(region.clone()),
        epoch_floor: 2,
        selected_stream_id: None,
        retained_through: None,
    });
    let marker = DurableStreamOplogRecord::Session(None, Box::new(cut)).into_inline_entry();
    oplog
        .add_pair(OplogEntry::revert(region), Box::new(move |_| marker))
        .await;
    oplog.commit(CommitLevel::Always).await;
    let expected = retained[256..].to_vec();
    let (left_control, right_control) = tokio::join!(
        first.lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &session),
        second.lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &session),
    );
    assert_eq!(
        left_control.unwrap().consumer_record_counts().get(&reader),
        Some(&259)
    );
    assert_eq!(
        right_control.unwrap().consumer_record_counts().get(&reader),
        Some(&259)
    );
    let (left, right) = tokio::join!(
        first.read_durable_stream_consumer_page(&owner, &session, reader, 1),
        second.read_durable_stream_consumer_page(&owner, &session, reader, 1),
    );
    assert_eq!(left.unwrap(), expected);
    assert_eq!(right.unwrap(), expected);
    let cold = DefaultWorkerService::new(
        kv,
        Arc::new(ShardServiceDefault::new()),
        oplog_service,
        Arc::new(UnusedComponentService),
        Arc::new(GolemConfig::default()),
    );
    assert_eq!(
        cold.read_durable_stream_consumer_page(&owner, &session, reader, 1)
            .await
            .unwrap(),
        expected
    );
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
    session_key: StreamInvocationId,
    attachment_id: AttachmentId,
    attempt_id: AttemptId,
    epoch: u64,
    pending_invocation_oplog_index: OplogIndex,
) -> StreamSessionRecord {
    StreamSessionRecord::Attached(StreamSessionAttachedRecord {
        format_version: 1,
        session_key: session_key.idempotency_key,
        attachment_id,
        attempt_id,
        epoch,
        pending_invocation_oplog_index,
    })
}

fn detached_record(
    session_key: StreamInvocationId,
    attachment_id: AttachmentId,
    attempt_id: AttemptId,
    epoch: u64,
) -> StreamSessionRecord {
    StreamSessionRecord::Detached(StreamSessionDetachedRecord {
        format_version: 1,
        session_key: session_key.idempotency_key,
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
        StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
            format_version: 1,
            session_key: local_registration(&key),
            result: vec![42; 8192],
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
    assert_eq!(metadata.result_position(), Some(result));
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
    assert_eq!(metadata.result_position(), Some(result));
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
            .result_position()
            .is_none()
    );

    let finished = append_session(
        oplog.as_ref(),
        StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: 1,
            session_key: local_registration(&key),
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
            .finished_position()
            .is_none()
    );
    oplog.commit(CommitLevel::Always).await;
    storage.reset();
    let metadata = reopened
        .lookup_durable_stream_control_metadata(&id, AgentMode::Durable, &key)
        .await
        .unwrap();
    assert_eq!(metadata.finished_position(), Some(finished));
    assert_eq!(metadata.covered_through(), finished);
    assert_eq!(
        storage.reads(),
        2,
        "the tip and one uncovered suffix read are sufficient"
    );

    let consumer_key = session_key(&id, &IdempotencyKey::new("consumer".into()));
    let stream = local_reader(OplogIndex::from_u64(2), 0);
    let other_stream = local_reader(OplogIndex::from_u64(2), 1);
    let mut expected = Vec::new();
    for ordinal in 0..600 {
        for target in [stream, other_stream] {
            let index = append_session(
                oplog.as_ref(),
                StreamSessionRecord::ConsumerItemValue(
                    golem_common::model::durable_stream::StreamConsumerItemValueRecord {
                        format_version: 1,
                        session_key: remote_registration(&consumer_key),
                        reader_id: target,
                        source_offset: golem_common::model::durable_stream::StreamOffset::new(
                            OplogIndex::from_u64(ordinal + 1),
                            0,
                        ),
                        consumer_read_ordinal: ordinal,
                        value: vec![7; 4096],
                        packed_u8: false,
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
    assert_eq!(metadata.consumer_record_count(stream), 600);
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
        let StreamSessionRecord::Prepared(prepared) = &record else {
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
    let StreamSessionRecord::Prepared(prepared) = &record else {
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
            StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                format_version: 1,
                session_key: local_registration(key),
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
            StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                format_version: 1,
                session_key: local_registration(key),
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
            StreamSessionRecord::ResumeAttempt(
                golem_common::model::durable_stream::StreamSessionResumeAttemptRecord {
                    format_version: 1,
                    session_key: key.idempotency_key.clone(),
                    attempt: golem_common::model::durable_stream::ResumeAttemptDescriptor {
                        format_version: 1,
                        operation:
                            golem_common::model::durable_stream::StreamResumeOperation::Takeover,
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
        let handle = DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(uuid::Uuid::new_v4()),
            producer_environment_id: remote.environment_id,
            producer: remote.agent_id.clone(),
            expected_producer_fingerprint: key.callee_fingerprint,
            producer_generation: OplogIndex::NONE,
            source_invocation: key.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([0; 32]),
        };
        let mapping = StreamSessionMappingRecord {
            transport_stream_id,
            handle: handle.clone(),
            role: SessionStreamRole::Output,
        };
        let attachment = StreamAttachmentKey {
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
        append_session(
            oplog.as_ref(),
            StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: 1,
                session_key: StreamRegistrationInvocation::Remote(key.clone()),
                mapping: StreamBindingRecord::foreign(&mapping),
            }),
        )
        .await;
        for record in [
            StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
                format_version: 1,
                session_key: key.clone(),
                attachment: attachment.clone(),
                mapping: mapping.clone(),
            }),
            StreamSessionRecord::TopologyActivated(StreamTopologyActivatedRecord {
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
    let readers = [
        local_reader(OplogIndex::from_u64(2), 0),
        local_reader(OplogIndex::from_u64(5), 0),
    ];
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Remote(key.clone()),
            reader_id: readers[0],
            source_offset: StreamOffset::new(OplogIndex::from_u64(100), 0),
            consumer_read_ordinal: 0,
            terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
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
    assert_eq!(
        pending[0].attachment_key.stream_id,
        attachments[1].stream_id
    );
    append_session(
        oplog.as_ref(),
        StreamSessionRecord::SourceUnavailable(StreamSourceUnavailableRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Remote(attachments[1].session_key.clone()),
            reader_id: readers[1],
            source_offset: StreamOffset::new(OplogIndex::from_u64(101), 0),
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
            StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
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
        StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(old.clone()),
            result: vec![1],
            stream_mappings: vec![],
        }),
    )
    .await;
    let old_finished = append_session(
        oplog.as_ref(),
        StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(old.clone()),
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
            StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                format_version: 1,
                session_key: StreamRegistrationInvocation::Local(key.clone()),
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
    assert_eq!(actual.session_key, Some(old));
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
            StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                format_version: 1,
                session_key: StreamRegistrationInvocation::Local(other.clone()),
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
        StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(key.clone()),
            result: vec![2],
            stream_mappings: vec![],
        }),
    )
    .await;
    let finished = append_session(
        oplog.as_ref(),
        StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(key.clone()),
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
        StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
            format_version: 1,
            session_key: remote_registration(&outgoing_session),
            result: vec![],
            stream_mappings: vec![],
        }),
        prepared_record(&id, &key),
        StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(key.clone()),
            result: vec![],
            stream_mappings: vec![],
        }),
        StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: 1,
            session_key: StreamRegistrationInvocation::Local(key.clone()),
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
    assert_eq!(actual.session_key, Some(key));
}

#[test]
async fn raw_attachment_authority_fences_before_commit_and_survives_buffer_drain() {
    use crate::durable_host::durable_session::StreamSession;
    use crate::durable_host::durable_stream::DurableStreamStore;
    use golem_common::model::durable_stream::{
        ResumeAttemptDescriptor, StreamResumeOperation, StreamSessionAttachedRecord,
        StreamSessionDetachedRecord, StreamSessionResumeAttemptRecord,
    };

    let (_, _, oplog_service) = service_with_oplog().await;
    let id = owned_agent("raw-authority", ComponentId::new());
    let oplog = create_oplog(oplog_service.as_ref(), &id).await;
    let key = IdempotencyKey::new("stream".into());
    let prepared = prepared_record(&id, &key);
    assert!(prepared.has_supported_format());
    let StreamSessionRecord::Prepared(prepared_record) = &prepared else {
        unreachable!()
    };
    let first_attempt = prepared_record.attempt.attempt_id;
    let attachment_id = prepared_record.attempt.attachment_id;
    let session = prepared_record.attempt.session_key.clone();
    append_session(oplog.as_ref(), prepared).await;
    let pending_invocation_oplog_index = append_pending_invocation(oplog.as_ref(), &key).await;
    let attached = append_session(
        oplog.as_ref(),
        StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: session.idempotency_key.clone(),
            attachment_id,
            attempt_id: first_attempt,
            epoch: 1,
            pending_invocation_oplog_index,
        }),
    )
    .await;
    oplog.commit(CommitLevel::Always).await;

    let producer = DurableStreamStore::load(
        oplog.clone(),
        id.environment_id,
        id.agent_id.clone(),
        session.callee_fingerprint,
        None,
    )
    .await
    .unwrap();
    let old = StreamSession::new(
        producer.clone(),
        oplog.clone(),
        StreamRegistrationInvocation::Local(session.idempotency_key.clone()),
        [],
    )
    .with_attachment(1, first_attempt);
    old.ensure_current_attachment().await.unwrap();

    let takeover_attempt = AttemptId::fresh();
    let resume = StreamSessionRecord::ResumeAttempt(StreamSessionResumeAttemptRecord {
        format_version: 1,
        session_key: session.idempotency_key.clone(),
        attempt: ResumeAttemptDescriptor {
            format_version: 1,
            operation: StreamResumeOperation::Takeover,
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
    let new = StreamSession::new(
        producer,
        oplog.clone(),
        StreamRegistrationInvocation::Local(session.idempotency_key.clone()),
        [],
    )
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
        StreamSessionRecord::Detached(StreamSessionDetachedRecord {
            format_version: 1,
            session_key: session.idempotency_key.clone(),
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
    let StreamSessionRecord::Prepared(value) = &prepared else {
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
        StreamSessionRecord::ResumeAttempt(StreamSessionResumeAttemptRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
            attempt: ResumeAttemptDescriptor {
                format_version: 1,
                operation: StreamResumeOperation::Takeover,
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
            &mut oplog_service.lock_lifecycle(&id.agent_id).await,
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
    let StreamSessionRecord::Prepared(value) = &prepared else {
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
            &mut oplog_service.lock_lifecycle(&id.agent_id).await,
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
        StreamSessionRecord::ResumeAttempt(StreamSessionResumeAttemptRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
            attempt: ResumeAttemptDescriptor {
                format_version: 1,
                operation: StreamResumeOperation::Takeover,
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
    let StreamSessionRecord::Prepared(value) = &prepared else {
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
        StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: 1,
            session_key: local_registration(&session_key),
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
    let StreamSessionRecord::Prepared(value) = &prepared else {
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
        StreamSessionRecord::Finished(StreamSessionFinishedRecord {
            format_version: 1,
            session_key: local_registration(&session_key),
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
            &mut reopened_service.lock_lifecycle(&id.agent_id).await,
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
    assert_eq!(status.session_key, Some(session_key.idempotency_key));
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
    let StreamSessionRecord::Prepared(value) = &prepared else {
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
            &mut oplog_service.lock_lifecycle(&id.agent_id).await,
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
        1,
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
        1,
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
    let StreamSessionRecord::Prepared(first_value) = &first else {
        unreachable!()
    };
    let first_key = first_value.attempt.session_key.clone();
    let remote = owned_agent("remote-callee", ComponentId::new());
    let second_key = session_key(&remote, &idempotency_key);
    let second = StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
        format_version: 1,
        session_key: remote_registration(&second_key),
        result: vec![],
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
    for (reference, expected) in [
        (
            StreamRegistrationInvocation::Remote(first_key.clone()),
            false,
        ),
        (StreamRegistrationInvocation::Local(idempotency_key), true),
    ] {
        append_session(
            oplog.as_ref(),
            StreamSessionRecord::CancelRequested(
                golem_common::model::durable_stream::StreamSessionCancelRequestedRecord {
                    format_version: 1,
                    session_key: reference,
                },
            ),
        )
        .await;
        for committed in [false, true] {
            if committed {
                oplog.commit(CommitLevel::Always).await;
            }
            let status = oplog
                .raw_durable_stream_session_status(&first_key)
                .await
                .status
                .unwrap()
                .unwrap();
            assert_eq!(status.cancellation_requested, expected);
            assert!(
                oplog
                    .raw_durable_stream_session_status(&second_key)
                    .await
                    .status
                    .unwrap()
                    .is_none()
            );
        }
    }
}
