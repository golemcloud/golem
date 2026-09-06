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
    CommitLevel, DurableStreamOplogRecord, Oplog, OplogService, PrimaryOplogService,
};
use crate::services::shard::ShardServiceDefault;
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
    AttachmentId, AttemptId, PersistedStreamInvocationDescriptorV1, StartAttemptDescriptorV1,
    StreamInvocationIdV1, StreamSessionFinishedRecordV1, StreamSessionInvocationResultRecordV1,
    StreamSessionPreparedRecordV1, StreamSessionRecordV1,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentMetadata};
use golem_common::read_only_lock;
use golem_service_base::model::component::Component;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::sync::RwLock;
use test_r::test;

struct UnusedComponentService;

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
        callee_fingerprint: AgentFingerprint::new(),
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
            attachment_id: AttachmentId(uuid::Uuid::new_v4()),
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

async fn create_oplog(service: &PrimaryOplogService, id: &OwnedAgentId) -> Arc<dyn Oplog> {
    let metadata = AgentMetadata {
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
    };
    service
        .create_fresh(
            id,
            AgentMode::Durable,
            OplogEntry::NoOp {
                timestamp: Timestamp::now_utc(),
            },
            metadata,
            read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
                AgentStatusRecord::default(),
            ))),
            read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                ExecutionStatus::Suspended {
                    agent_mode: AgentMode::Durable,
                    timestamp: Timestamp::now_utc(),
                },
            ))),
        )
        .await
}

async fn append_session(oplog: &dyn Oplog, record: StreamSessionRecordV1) -> OplogIndex {
    oplog
        .add(DurableStreamOplogRecord::Session(Box::new(record)).into_inline_entry())
        .await
}

async fn append_noop(oplog: &dyn Oplog) -> OplogIndex {
    oplog
        .add(OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
        })
        .await
}

fn completed(first: u64, finished: u64) -> DurableStreamSessionStatus {
    DurableStreamSessionStatus {
        first_prepared: Some(OplogIndex::from_u64(first)),
        prepared: Some(OplogIndex::from_u64(first)),
        invocation_result: None,
        finished: Some(OplogIndex::from_u64(finished)),
    }
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
    let namespace = DefaultWorkerService::stream_session_index_namespace(&id.agent_id);
    let old = IdempotencyKey::new("evicted".into());
    let old_status = completed(2, 3);
    let metadata = DurableStreamSessionIndexMetadata {
        covered_through: OplogIndex::from_u64(300),
    };
    let metadata_bytes = serialize(&metadata).unwrap();
    let status_bytes = serialize(&old_status).unwrap();
    kv.with_entity("test", "seed", "session")
        .set_many_raw(
            namespace,
            &[
                (
                    STREAM_SESSION_INDEX_METADATA_FIELD,
                    metadata_bytes.as_slice(),
                ),
                (
                    stream_session_index_field(&old).as_str(),
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
        let namespace = DefaultWorkerService::stream_session_index_namespace(&id.agent_id);
        kv.with_entity("test", "seed", "session")
            .set_raw(
                namespace,
                &stream_session_index_field(&key),
                &serialize(&completed(finished - 1, finished)).unwrap(),
            )
            .await
            .unwrap();
    }

    service.clear_stream_session_index(&first).await.unwrap();
    let first_keys = kv
        .with("test", "verify")
        .keys(DefaultWorkerService::stream_session_index_namespace(
            &first.agent_id,
        ))
        .await
        .unwrap();
    let second_keys = kv
        .with("test", "verify")
        .keys(DefaultWorkerService::stream_session_index_namespace(
            &second.agent_id,
        ))
        .await
        .unwrap();
    assert!(first_keys.is_empty());
    assert_eq!(second_keys, vec![stream_session_index_field(&key)]);
}

#[test]
async fn catchup_scans_multiple_chunks_and_recovers_evicted_completed_session() {
    let (service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("multi-chunk", ComponentId::new());
    let oplog = create_oplog(&oplog_service, &id).await;
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

    assert_eq!(
        service
            .lookup_durable_stream_session(&id, AgentMode::Durable, &status, &old)
            .await
            .unwrap(),
        Some(DurableStreamSessionStatus {
            first_prepared: Some(old_first),
            prepared: Some(old_first),
            invocation_result: Some(old_result),
            finished: Some(old_finished),
        })
    );
}

#[test]
async fn incremental_catchup_merges_later_fields_into_old_unfinished_session() {
    let (service, _kv, oplog_service) = service_with_oplog().await;
    let id = owned_agent("incremental", ComponentId::new());
    let oplog = create_oplog(&oplog_service, &id).await;
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

    assert_eq!(
        service
            .lookup_durable_stream_session(&id, AgentMode::Durable, &later_status, &key)
            .await
            .unwrap(),
        Some(DurableStreamSessionStatus {
            first_prepared: Some(first),
            prepared: Some(first),
            invocation_result: Some(result),
            finished: Some(finished),
        })
    );
}

#[test]
fn status_fold_tracks_local_lifecycle_without_retaining_caller_results() {
    use crate::worker::status::update_status_with_new_entries;
    use golem_common::model::oplog::OplogPayload;
    use std::collections::BTreeMap;

    let id = owned_agent("fold", ComponentId::new());
    let key = IdempotencyKey::new("local".into());
    let mut status = AgentStatusRecord::default();
    let records = [
        StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
            format_version: 1,
            session_key: session_key(&id, &IdempotencyKey::new("outgoing".into())),
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
        let entry = OplogEntry::stream_session(OplogPayload::SerializedInline {
            bytes: serialize(&record).unwrap(),
            cached: None,
        });
        status = update_status_with_new_entries(
            AgentMode::Durable,
            status,
            BTreeMap::from([(OplogIndex::from_u64(offset as u64 + 1), entry)]),
            &RetryConfig::default(),
        )
        .unwrap();
        if offset == 0 {
            assert!(!status.durable_stream_sessions.has_history());
        }
    }
    assert_eq!(status.durable_stream_sessions.iter().count(), 1);
    assert_eq!(
        status.durable_stream_sessions.get(&key),
        Some(&DurableStreamSessionStatus {
            first_prepared: Some(OplogIndex::from_u64(2)),
            prepared: Some(OplogIndex::from_u64(2)),
            invocation_result: Some(OplogIndex::from_u64(3)),
            finished: Some(OplogIndex::from_u64(4)),
        })
    );
}
