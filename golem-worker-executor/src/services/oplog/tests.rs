// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::collections::HashSet;
use std::sync::RwLock;
use std::time::Instant;
use test_r::{test, test_dep};

use assert2::check;
use nonempty_collections::nev;
use tracing::{debug, info};
use uuid::Uuid;

use golem_common::config::RedisConfig;
use golem_common::model::oplog::{LogLevel, SpanData, WorkerError};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AccountId, ComponentId, ComponentType, WorkerStatusRecord};
use golem_common::redis::RedisPool;
use golem_common::tracing::{init_tracing, TracingConfig};

use crate::services::oplog::compressed::CompressedOplogArchiveService;
use crate::services::oplog::multilayer::OplogArchiveService;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::IndexedStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;

use super::*;

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing(&TracingConfig::test("op-log-tests"), |_output| {
            golem_common::tracing::filter::boxed::debug_env_with_directives(Vec::new())
        });
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}

fn rounded_ts(ts: Timestamp) -> Timestamp {
    Timestamp::from(ts.to_millis())
}

fn rounded_span_data(invocation_context: Vec<SpanData>) -> Vec<SpanData> {
    invocation_context
        .into_iter()
        .map(|span_data| match span_data {
            SpanData::ExternalSpan { span_id } => SpanData::ExternalSpan { span_id },
            SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                linked_context,
                attributes,
                inherited,
            } => SpanData::LocalSpan {
                span_id,
                start: rounded_ts(start),
                parent_id,
                linked_context: linked_context.map(rounded_span_data),
                attributes,
                inherited,
            },
        })
        .collect()
}

pub fn rounded(entry: OplogEntry) -> OplogEntry {
    match entry {
        OplogEntry::Create {
            timestamp,
            worker_id,
            component_version,
            args,
            env,
            wasi_config_vars,
            project_id,
            created_by,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins,
        } => OplogEntry::Create {
            timestamp: rounded_ts(timestamp),
            worker_id,
            component_version,
            args,
            env,
            wasi_config_vars,
            project_id,
            created_by,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins,
        },
        OplogEntry::ImportedFunctionInvoked {
            timestamp,
            function_name,
            request,
            response,
            durable_function_type,
        } => OplogEntry::ImportedFunctionInvoked {
            timestamp: rounded_ts(timestamp),
            function_name,
            request,
            response,
            durable_function_type,
        },
        OplogEntry::ExportedFunctionInvoked {
            timestamp,
            function_name,
            request,
            idempotency_key,
            trace_id,
            trace_states,
            invocation_context,
        } => OplogEntry::ExportedFunctionInvoked {
            timestamp: rounded_ts(timestamp),
            function_name,
            request,
            idempotency_key,
            trace_id,
            trace_states,
            invocation_context: rounded_span_data(invocation_context),
        },
        OplogEntry::ExportedFunctionCompleted {
            timestamp,
            response,
            consumed_fuel,
        } => OplogEntry::ExportedFunctionCompleted {
            timestamp: rounded_ts(timestamp),
            response,
            consumed_fuel,
        },
        OplogEntry::Suspend { timestamp } => OplogEntry::Suspend {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::NoOp { timestamp } => OplogEntry::NoOp {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::Jump { timestamp, jump } => OplogEntry::Jump {
            timestamp: rounded_ts(timestamp),
            jump,
        },
        OplogEntry::Interrupted { timestamp } => OplogEntry::Interrupted {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::Exited { timestamp } => OplogEntry::Exited {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::ChangeRetryPolicy {
            timestamp,
            new_policy,
        } => OplogEntry::ChangeRetryPolicy {
            timestamp: rounded_ts(timestamp),
            new_policy,
        },
        OplogEntry::BeginAtomicRegion { timestamp } => OplogEntry::BeginAtomicRegion {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::EndAtomicRegion {
            timestamp,
            begin_index,
        } => OplogEntry::EndAtomicRegion {
            timestamp: rounded_ts(timestamp),
            begin_index,
        },
        OplogEntry::BeginRemoteWrite { timestamp } => OplogEntry::BeginRemoteWrite {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::EndRemoteWrite {
            timestamp,
            begin_index,
        } => OplogEntry::EndRemoteWrite {
            timestamp: rounded_ts(timestamp),
            begin_index,
        },
        OplogEntry::PendingUpdate {
            timestamp,
            description,
        } => OplogEntry::PendingUpdate {
            timestamp: rounded_ts(timestamp),
            description,
        },
        OplogEntry::SuccessfulUpdate {
            timestamp,
            target_version,
            new_component_size,
            new_active_plugins,
        } => OplogEntry::SuccessfulUpdate {
            timestamp: rounded_ts(timestamp),
            target_version,
            new_component_size,
            new_active_plugins,
        },
        OplogEntry::FailedUpdate {
            timestamp,
            target_version,
            details,
        } => OplogEntry::FailedUpdate {
            timestamp: rounded_ts(timestamp),
            target_version,
            details,
        },
        OplogEntry::Error {
            timestamp,
            error,
            retry_from,
        } => OplogEntry::Error {
            timestamp: rounded_ts(timestamp),
            error,
            retry_from,
        },
        OplogEntry::PendingWorkerInvocation {
            timestamp,
            invocation,
        } => OplogEntry::PendingWorkerInvocation {
            timestamp: rounded_ts(timestamp),
            invocation,
        },
        OplogEntry::GrowMemory { timestamp, delta } => OplogEntry::GrowMemory {
            timestamp: rounded_ts(timestamp),
            delta,
        },
        OplogEntry::CreateResource {
            timestamp,
            id,
            resource_type_id,
        } => OplogEntry::CreateResource {
            timestamp: rounded_ts(timestamp),
            id,
            resource_type_id,
        },
        OplogEntry::DropResource {
            timestamp,
            id,
            resource_type_id,
        } => OplogEntry::DropResource {
            timestamp: rounded_ts(timestamp),
            id,
            resource_type_id,
        },
        OplogEntry::Log {
            timestamp,
            level,
            context,
            message,
        } => OplogEntry::Log {
            timestamp: rounded_ts(timestamp),
            level,
            context,
            message,
        },
        OplogEntry::Restart { timestamp } => OplogEntry::Restart {
            timestamp: rounded_ts(timestamp),
        },
        OplogEntry::ActivatePlugin { timestamp, plugin } => OplogEntry::ActivatePlugin {
            timestamp: rounded_ts(timestamp),
            plugin,
        },
        OplogEntry::DeactivatePlugin { timestamp, plugin } => OplogEntry::DeactivatePlugin {
            timestamp: rounded_ts(timestamp),
            plugin,
        },
        OplogEntry::Revert {
            timestamp,
            dropped_region,
        } => OplogEntry::Revert {
            timestamp: rounded_ts(timestamp),
            dropped_region,
        },
        OplogEntry::CancelPendingInvocation {
            timestamp,
            idempotency_key,
        } => OplogEntry::CancelPendingInvocation {
            timestamp: rounded_ts(timestamp),
            idempotency_key,
        },
        OplogEntry::StartSpan {
            timestamp,
            span_id,
            parent_id,
            linked_context_id,
            attributes,
        } => OplogEntry::StartSpan {
            timestamp: rounded_ts(timestamp),
            span_id,
            parent_id,
            linked_context_id,
            attributes,
        },
        OplogEntry::FinishSpan { timestamp, span_id } => OplogEntry::FinishSpan {
            timestamp: rounded_ts(timestamp),
            span_id,
        },
        OplogEntry::SetSpanAttribute {
            timestamp,
            span_id,
            key,
            value,
        } => OplogEntry::SetSpanAttribute {
            timestamp: rounded_ts(timestamp),
            span_id,
            key,
            value,
        },
        OplogEntry::ChangePersistenceLevel { timestamp, level } => {
            OplogEntry::ChangePersistenceLevel {
                timestamp: rounded_ts(timestamp),
                level,
            }
        }
        OplogEntry::BeginRemoteTransaction {
            timestamp,
            transaction_id,
            original_begin_index,
        } => OplogEntry::BeginRemoteTransaction {
            timestamp: rounded_ts(timestamp),
            transaction_id,
            original_begin_index,
        },
        OplogEntry::PreCommitRemoteTransaction {
            timestamp,
            begin_index,
        } => OplogEntry::PreCommitRemoteTransaction {
            timestamp: rounded_ts(timestamp),
            begin_index,
        },
        OplogEntry::PreRollbackRemoteTransaction {
            timestamp,
            begin_index,
        } => OplogEntry::PreRollbackRemoteTransaction {
            timestamp: rounded_ts(timestamp),
            begin_index,
        },
        OplogEntry::CommittedRemoteTransaction {
            timestamp,
            begin_index,
        } => OplogEntry::CommittedRemoteTransaction {
            timestamp: rounded_ts(timestamp),
            begin_index,
        },
        OplogEntry::RolledBackRemoteTransaction {
            timestamp,
            begin_index,
        } => OplogEntry::RolledBackRemoteTransaction {
            timestamp: rounded_ts(timestamp),
            begin_index,
        },
    }
}

fn default_last_known_status() -> read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord> {
    read_only_lock::tokio::ReadOnlyLock::new(Arc::new(tokio::sync::RwLock::new(
        WorkerStatusRecord::default(),
    )))
}

fn default_execution_status(
    component_type: ComponentType,
) -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        component_type,
        timestamp: Timestamp::now_utc(),
    })))
}

#[test]
async fn open_add_and_read_back(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 100).await;
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;

    let entry1 = rounded(OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    }));
    let entry2 = rounded(OplogEntry::suspend());
    let entry3 = rounded(OplogEntry::exited());

    let last_oplog_idx = oplog.current_oplog_index().await;
    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);

    let entries = oplog_service
        .read(&owned_worker_id, last_oplog_idx.next(), 3)
        .await;
    assert_eq!(
        entries.into_values().collect::<Vec<_>>(),
        vec![entry1, entry2, entry3]
    );
}

#[test]
async fn open_add_and_read_back_ephemeral(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Ephemeral),
        )
        .await;

    let entry1 = rounded(OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    }));
    let entry2 = rounded(OplogEntry::suspend());
    let entry3 = rounded(OplogEntry::exited());

    let last_oplog_idx = oplog.current_oplog_index().await;
    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);

    let entries = oplog_service
        .read(&owned_worker_id, last_oplog_idx.next(), 3)
        .await;
    assert_eq!(
        entries.into_values().collect::<Vec<_>>(),
        vec![entry1, entry2, entry3]
    );
}

#[test]
async fn entries_with_small_payload(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 100).await;
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;

    let last_oplog_idx = oplog.current_oplog_index().await;
    let entry1 = rounded(
        oplog
            .add_imported_function_invoked(
                "f1".to_string(),
                &"request".to_string(),
                &"response".to_string(),
                DurableFunctionType::ReadRemote,
            )
            .await
            .unwrap(),
    );
    let entry2 = rounded(
        oplog
            .add_exported_function_invoked(
                "f2".to_string(),
                &"request".to_string(),
                IdempotencyKey::fresh(),
                InvocationContextStack::fresh(),
            )
            .await
            .unwrap(),
    );
    let entry3 = rounded(
        oplog
            .add_exported_function_completed(&"response".to_string(), 42)
            .await
            .unwrap(),
    );

    let desc = oplog
        .create_snapshot_based_update_description(11, &[1, 2, 3])
        .await
        .unwrap();
    let entry4 = rounded(OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc(),
        description: desc.clone(),
    });
    oplog.add(entry4.clone()).await;

    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;
    let r4 = oplog.read(last_oplog_idx.next().next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);
    assert_eq!(r4, entry4);

    let entries = oplog_service
        .read(&owned_worker_id, last_oplog_idx.next(), 4)
        .await;
    assert_eq!(
        entries.into_values().collect::<Vec<_>>(),
        vec![
            entry1.clone(),
            entry2.clone(),
            entry3.clone(),
            entry4.clone(),
        ]
    );

    let p1 = oplog
        .get_payload_of_entry::<String>(&entry1)
        .await
        .unwrap()
        .unwrap();
    let p2 = oplog
        .get_payload_of_entry::<String>(&entry2)
        .await
        .unwrap()
        .unwrap();
    let p3 = oplog
        .get_payload_of_entry::<String>(&entry3)
        .await
        .unwrap()
        .unwrap();
    let p4 = oplog
        .get_upload_description_payload(&desc)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(p1, "response");
    assert_eq!(p2, "request");
    assert_eq!(p3, "response");
    assert_eq!(p4, vec![1, 2, 3]);
}

#[test]
async fn entries_with_large_payload(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 100).await;
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;

    let large_payload1 = vec![0u8; 1024 * 1024];
    let large_payload2 = vec![1u8; 1024 * 1024];
    let large_payload3 = vec![2u8; 1024 * 1024];
    let large_payload4 = vec![3u8; 1024 * 1024];

    let last_oplog_idx = oplog.current_oplog_index().await;
    let entry1 = rounded(
        oplog
            .add_imported_function_invoked(
                "f1".to_string(),
                &"request".to_string(),
                &large_payload1,
                DurableFunctionType::ReadRemote,
            )
            .await
            .unwrap(),
    );
    let entry2 = rounded(
        oplog
            .add_exported_function_invoked(
                "f2".to_string(),
                &large_payload2,
                IdempotencyKey::fresh(),
                InvocationContextStack::fresh(),
            )
            .await
            .unwrap(),
    );
    let entry3 = rounded(
        oplog
            .add_exported_function_completed(&large_payload3, 42)
            .await
            .unwrap(),
    );

    let desc = oplog
        .create_snapshot_based_update_description(11, &large_payload4)
        .await
        .unwrap();
    let entry4 = rounded(OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc(),
        description: desc.clone(),
    });
    oplog.add(entry4.clone()).await;

    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await;
    let r2 = oplog.read(last_oplog_idx.next().next()).await;
    let r3 = oplog.read(last_oplog_idx.next().next().next()).await;
    let r4 = oplog.read(last_oplog_idx.next().next().next().next()).await;

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);
    assert_eq!(r4, entry4);

    let entries = oplog_service
        .read(&owned_worker_id, last_oplog_idx.next(), 4)
        .await;
    assert_eq!(
        entries.into_values().collect::<Vec<_>>(),
        vec![
            entry1.clone(),
            entry2.clone(),
            entry3.clone(),
            entry4.clone(),
        ]
    );

    let p1 = oplog
        .get_payload_of_entry::<Vec<u8>>(&entry1)
        .await
        .unwrap()
        .unwrap();
    let p2 = oplog
        .get_payload_of_entry::<Vec<u8>>(&entry2)
        .await
        .unwrap()
        .unwrap();
    let p3 = oplog
        .get_payload_of_entry::<Vec<u8>>(&entry3)
        .await
        .unwrap()
        .unwrap();
    let p4 = oplog
        .get_upload_description_payload(&desc)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(p1, large_payload1);
    assert_eq!(p2, large_payload2);
    assert_eq!(p3, large_payload3);
    assert_eq!(p4, large_payload4);
}

#[test]
async fn multilayer_transfers_entries_after_limit_reached_1(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 315, 5, 1, 3, false).await;
}

#[test]
async fn multilayer_transfers_entries_after_limit_reached_2(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 12, 2, 1, 0, false).await;
}

#[test]
async fn multilayer_transfers_entries_after_limit_reached_3(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 10000, 0, 0, 100, false).await;
}

#[test]
async fn blob_multilayer_transfers_entries_after_limit_reached_1(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 315, 5, 1, 3, true).await;
}

#[test]
async fn blob_multilayer_transfers_entries_after_limit_reached_2(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 12, 2, 1, 0, true).await;
}

#[test]
async fn blob_multilayer_transfers_entries_after_limit_reached_3(_tracing: &Tracing) {
    multilayer_transfers_entries_after_limit_reached(false, 10000, 0, 0, 100, true).await;
}

async fn multilayer_transfers_entries_after_limit_reached(
    use_redis: bool,
    n: u64,
    expected_1: u64,
    expected_2: u64,
    expected_3: u64,
    use_blob: bool,
) {
    let indexed_storage: Arc<dyn IndexedStorage + Send + Sync> = if use_redis {
        let pool = RedisPool::configured(&RedisConfig::default())
            .await
            .unwrap();
        Arc::new(RedisIndexedStorage::new(pool))
    } else {
        Arc::new(InMemoryIndexedStorage::new())
    };

    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));

    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;
    let mut entries = Vec::new();

    for i in 0..n {
        let entry = rounded(
            oplog
                .add_imported_function_invoked(
                    "test-function".to_string(),
                    &"request".to_string(),
                    &i,
                    DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap(),
        );
        oplog.commit(CommitLevel::Always).await;
        entries.push(entry);
    }

    let start = Instant::now();
    loop {
        let primary_length = primary_oplog_service
            .open(
                &owned_worker_id,
                primary_oplog_service.get_last_index(&owned_worker_id).await,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await
            .length()
            .await;

        let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
        if primary_length == expected_1 && secondary_length == expected_2 {
            break;
        }
        let elapsed = start.elapsed();
        if elapsed.as_secs() > 120 {
            panic!("Timeout");
        } else {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    debug!("Fetching information to evaluate the test");

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    let all_entries = oplog_service
        .read(&owned_worker_id, OplogIndex::NONE, n + 100)
        .await;

    assert_eq!(all_entries.len(), entries.len());
    assert_eq!(primary_length, expected_1);
    assert_eq!(secondary_length, expected_2);
    assert_eq!(tertiary_length, expected_3);
    assert_eq!(
        all_entries.keys().cloned().collect::<Vec<_>>(),
        (1..=n).map(OplogIndex::from_u64).collect::<Vec<_>>()
    );
    check!(all_entries.values().cloned().collect::<Vec<_>>() == entries);
}

#[test]
async fn read_from_archive(_tracing: &Tracing) {
    read_from_archive_impl(false).await;
}

#[test]
async fn blob_read_from_archive(_tracing: &Tracing) {
    read_from_archive_impl(true).await;
}

async fn read_from_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            rounded(OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            })
        })
        .collect();

    let initial_oplog_idx = oplog.current_oplog_index().await;

    for entry in &entries {
        oplog.add(entry.clone()).await;
    }
    oplog.commit(CommitLevel::Always).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    let first10 = oplog_service
        .read(&owned_worker_id, initial_oplog_idx.next(), 10)
        .await;
    let original_first10 = entries.into_iter().take(10).collect::<Vec<_>>();

    assert_eq!(first10.into_values().collect::<Vec<_>>(), original_first10);
}

#[test]
async fn read_initial_from_archive(_tracing: &Tracing) {
    crate::services::oplog::tests::read_initial_from_archive_impl(false).await;
}

#[test]
async fn blob_read_initial_from_archive(_tracing: &Tracing) {
    crate::services::oplog::tests::read_initial_from_archive_impl(true).await;
}

async fn read_initial_from_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    let timestamp = Timestamp::now_utc();
    let create_entry = rounded(OplogEntry::Create {
        timestamp,
        worker_id: WorkerId {
            component_id: ComponentId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        },
        component_version: 1,
        args: vec![],
        env: vec![],
        wasi_config_vars: BTreeMap::new(),
        project_id: project_id.clone(),
        created_by: AccountId {
            value: "user1".to_string(),
        },
        parent: None,
        component_size: 0,
        initial_total_linear_memory_size: 0,
        initial_active_plugins: HashSet::new(),
    });

    let oplog = oplog_service
        .create(
            &owned_worker_id,
            create_entry.clone(),
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;

    // The create entry is in the primary oplog now
    let read1 = oplog_service
        .read(&owned_worker_id, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_1 = oplog_service.get_last_index(&owned_worker_id).await;

    // Archiving it to the secondary
    let more = MultiLayerOplog::try_archive_blocking(&oplog).await;

    // Reading it again, now it needs to be fetched from the secondary layer
    let read2 = oplog_service
        .read(&owned_worker_id, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_2 = oplog_service.get_last_index(&owned_worker_id).await;

    // Archiving it to the tertiary
    MultiLayerOplog::try_archive_blocking(&oplog).await;

    // Reading it again, now it needs to be fetched from the tertiary layer
    let read3 = oplog_service
        .read(&owned_worker_id, OplogIndex::INITIAL, 1)
        .await
        .into_iter()
        .next();
    let last_index_3 = oplog_service.get_last_index(&owned_worker_id).await;

    assert_eq!(more, Some(true));
    assert_eq!(read1, Some((OplogIndex::INITIAL, create_entry.clone())));
    assert_eq!(read2, Some((OplogIndex::INITIAL, create_entry.clone())));
    assert_eq!(read3, Some((OplogIndex::INITIAL, create_entry)));

    assert_eq!(last_index_1, OplogIndex::INITIAL);
    assert_eq!(last_index_2, OplogIndex::INITIAL);
    assert_eq!(last_index_3, OplogIndex::INITIAL);
}

#[test]
async fn write_after_archive(_tracing: &Tracing) {
    write_after_archive_impl(false, Reopen::No).await;
}

#[test]
async fn blob_write_after_archive(_tracing: &Tracing) {
    write_after_archive_impl(true, Reopen::No).await;
}

#[test]
async fn write_after_archive_reopen(_tracing: &Tracing) {
    write_after_archive_impl(false, Reopen::Yes).await;
}

#[test]
async fn blob_write_after_archive_reopen(_tracing: &Tracing) {
    write_after_archive_impl(true, Reopen::Yes).await;
}

#[test]
async fn write_after_archive_reopen_full(_tracing: &Tracing) {
    write_after_archive_impl(false, Reopen::Full).await;
}

#[test]
async fn blob_write_after_archive_reopen_full(_tracing: &Tracing) {
    write_after_archive_impl(true, Reopen::Full).await;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Reopen {
    No,
    Yes,
    Full,
}

async fn write_after_archive_impl(use_blob: bool, reopen: Reopen) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let mut primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
        ))
    };
    let mut oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    info!("FIRST OPEN");
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;
    info!("FIRST OPEN DONE");

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            rounded(OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            })
        })
        .collect();

    let initial_oplog_idx = oplog.current_oplog_index().await;

    for entry in &entries {
        oplog.add(entry.clone()).await;
    }
    oplog.commit(CommitLevel::Always).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    info!("initial oplog index: {}", initial_oplog_idx);
    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    let oplog = if reopen == Reopen::Yes {
        drop(oplog);
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await
    } else if reopen == Reopen::Full {
        drop(oplog);
        primary_oplog_service = Arc::new(
            PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
        );
        oplog_service = Arc::new(MultiLayerOplogService::new(
            primary_oplog_service.clone(),
            nev![secondary_layer.clone(), tertiary_layer.clone()],
            10,
            10,
        ));
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await
    } else {
        oplog
    };

    let entries: Vec<OplogEntry> = (100..1000)
        .map(|i| {
            rounded(OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            })
        })
        .collect();

    for (n, entry) in entries.iter().enumerate() {
        oplog.add(entry.clone()).await;
        if n % 100 == 0 {
            oplog.commit(CommitLevel::Always).await;
        }
    }
    oplog.commit(CommitLevel::Always).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    info!("initial oplog index: {}", initial_oplog_idx);
    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    let oplog = if reopen == Reopen::Yes {
        drop(oplog);
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await
    } else if reopen == Reopen::Full {
        drop(oplog);
        primary_oplog_service = Arc::new(
            PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
        );
        oplog_service = Arc::new(MultiLayerOplogService::new(
            primary_oplog_service.clone(),
            nev![secondary_layer.clone(), tertiary_layer.clone()],
            10,
            10,
        ));
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await
    } else {
        oplog
    };

    oplog
        .add(rounded(OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("last".to_string()),
            retry_from: OplogIndex::NONE,
        }))
        .await;
    oplog.commit(CommitLevel::Always).await;
    drop(oplog);

    let entry1 = oplog_service
        .read(&owned_worker_id, OplogIndex::INITIAL, 1)
        .await;
    let entry2 = oplog_service
        .read(&owned_worker_id, OplogIndex::from_u64(100), 1)
        .await;
    let entry3 = oplog_service
        .read(&owned_worker_id, OplogIndex::from_u64(1000), 1)
        .await;
    let entry4 = oplog_service
        .read(&owned_worker_id, OplogIndex::from_u64(1001), 1)
        .await;

    assert_eq!(entry1.len(), 1);
    assert_eq!(entry2.len(), 1);
    assert_eq!(entry3.len(), 1);
    assert_eq!(entry4.len(), 1);

    assert_eq!(
        entry1.get(&OplogIndex::INITIAL).unwrap().clone(),
        rounded(OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("0".to_string()),
            retry_from: OplogIndex::NONE,
        })
    );
    assert_eq!(
        entry2.get(&OplogIndex::from_u64(100)).unwrap().clone(),
        rounded(OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("99".to_string()),
            retry_from: OplogIndex::NONE,
        })
    );
    assert_eq!(
        entry3.get(&OplogIndex::from_u64(1000)).unwrap().clone(),
        rounded(OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("999".to_string()),
            retry_from: OplogIndex::NONE,
        })
    );
    assert_eq!(
        entry4.get(&OplogIndex::from_u64(1001)).unwrap().clone(),
        rounded(OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("last".to_string()),
            retry_from: OplogIndex::NONE,
        })
    );
}

#[test]
async fn empty_layer_gets_deleted(_tracing: &Tracing) {
    empty_layer_gets_deleted_impl(false).await;
}

#[test]
async fn blob_empty_layer_gets_deleted(_tracing: &Tracing) {
    empty_layer_gets_deleted_impl(true).await;
}

async fn empty_layer_gets_deleted_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        10,
        10,
    ));
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await;

    // As we add 100 entries at once, and that exceeds the limit, we expect that all entries have
    // been moved to the secondary layer. By doing this 10 more times, we end up having all entries
    // in the tertiary layer.

    for _ in 0..10 {
        let timestamp = Timestamp::now_utc();
        let entries: Vec<OplogEntry> = (0..100)
            .map(|i| {
                rounded(OplogEntry::Error {
                    timestamp,
                    error: WorkerError::Unknown(i.to_string()),
                    retry_from: OplogIndex::NONE,
                })
            })
            .collect();

        for entry in &entries {
            oplog.add(entry.clone()).await;
        }
        oplog.commit(CommitLevel::Always).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    let primary_exists = primary_oplog_service.exists(&owned_worker_id).await;
    let secondary_exists = secondary_layer.exists(&owned_worker_id).await;
    let tertiary_exists = tertiary_layer.exists(&owned_worker_id).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    assert_eq!(primary_length, 0);
    assert_eq!(secondary_length, 0);
    assert_eq!(tertiary_length, 1);

    assert!(!primary_exists);
    assert!(!secondary_exists);
    assert!(tertiary_exists);
}

#[test]
async fn scheduled_archive(_tracing: &Tracing) {
    scheduled_archive_impl(false).await;
}

#[test]
async fn blob_scheduled_archive(_tracing: &Tracing) {
    scheduled_archive_impl(true).await;
}

async fn scheduled_archive_impl(use_blob: bool) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 1))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            1,
        ))
    };
    let tertiary_layer: Arc<dyn OplogArchiveService> = if use_blob {
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2))
    } else {
        Arc::new(CompressedOplogArchiveService::new(
            indexed_storage.clone(),
            2,
        ))
    };
    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        1000, // no transfer will occur by reaching limit in this test
        10,
    ));
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            rounded(OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            })
        })
        .collect();

    // Adding 100 entries to the primary oplog, schedule archive and immediately drop the oplog
    let archive_result = {
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        let oplog = oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await;
        for entry in &entries {
            oplog.add(entry.clone()).await;
        }
        oplog.commit(CommitLevel::Always).await;

        let result = MultiLayerOplog::try_archive(&oplog).await;
        drop(oplog);
        result
    };

    let last_oplog_index_1 = oplog_service.get_last_index(&owned_worker_id).await;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    info!("primary_length: {}", primary_length);
    info!("secondary_length: {}", secondary_length);
    info!("tertiary_length: {}", tertiary_length);

    assert_eq!(primary_length, 0);
    assert_eq!(secondary_length, 1);
    assert_eq!(tertiary_length, 0);
    assert_eq!(archive_result, Some(true));

    let last_oplog_index_2 = oplog_service.get_last_index(&owned_worker_id).await;

    assert_eq!(last_oplog_index_1, last_oplog_index_2);

    // Calling archive again
    let archive_result2 = {
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        let oplog = oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await;
        let result = MultiLayerOplog::try_archive(&oplog).await;
        drop(oplog);
        result
    };

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
            default_last_known_status(),
            default_execution_status(ComponentType::Durable),
        )
        .await
        .length()
        .await;
    let secondary_length = secondary_layer.open(&owned_worker_id).await.length().await;
    let tertiary_length = tertiary_layer.open(&owned_worker_id).await.length().await;

    info!("primary_length 2: {}", primary_length);
    info!("secondary_length 2: {}", secondary_length);
    info!("tertiary_length 2: {}", tertiary_length);

    assert_eq!(primary_length, 0);
    assert_eq!(secondary_length, 0);
    assert_eq!(tertiary_length, 1);
    assert_eq!(archive_result2, Some(false));

    let last_oplog_index_3 = oplog_service.get_last_index(&owned_worker_id).await;

    assert_eq!(last_oplog_index_2, last_oplog_index_3);
}

#[test]
async fn multilayer_scan_for_component(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 100).await,
    );
    let secondary_layer: Arc<dyn OplogArchiveService> = Arc::new(
        CompressedOplogArchiveService::new(indexed_storage.clone(), 1),
    );
    let tertiary_layer: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage.clone(), 2));

    let oplog_service = Arc::new(MultiLayerOplogService::new(
        primary_oplog_service.clone(),
        nev![secondary_layer.clone(), tertiary_layer.clone()],
        1000, // no transfer will occur by reaching limit in this test
        10,
    ));
    let account_id = AccountId {
        value: "user1".to_string(),
    };
    let project_id = ProjectId::new_v4();
    let component_id = ComponentId::new_v4();

    // Adding some workers
    let mut primary_workers = Vec::new();
    let mut secondary_workers = Vec::new();
    let mut tertiary_workers = Vec::new();
    for i in 0..100 {
        let worker_id = WorkerId {
            component_id: component_id.clone(),
            worker_name: format!("worker-{i}"),
        };
        let create_entry = OplogEntry::create(
            worker_id.clone(),
            1,
            Vec::new(),
            Vec::new(),
            BTreeMap::new(),
            project_id.clone(),
            account_id.clone(),
            None,
            100,
            100,
            HashSet::new(),
        );

        let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);
        let oplog = oplog_service
            .create(
                &owned_worker_id,
                create_entry,
                WorkerMetadata::default(worker_id.clone(), account_id.clone(), project_id.clone()),
                default_last_known_status(),
                default_execution_status(ComponentType::Durable),
            )
            .await;

        debug!("Created {worker_id}");
        match i % 3 {
            0 => primary_workers.push(worker_id),
            1 => {
                secondary_workers.push(worker_id.clone());
                debug!("Archiving {worker_id} to secondary layer");
                MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!("Adding more oplog entries to primary");
                    oplog
                        .add_and_commit(OplogEntry::log(
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await;
                }
            }
            2 => {
                tertiary_workers.push(worker_id.clone());
                debug!("Archiving {worker_id} to secondary layer");
                let r = MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!("Adding more oplog entries to primary going to be moved to the secondary layer");
                    oplog
                        .add_and_commit(OplogEntry::log(
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await;
                }

                debug!("[{r:?}] => archiving {worker_id} to tertiary layer");
                MultiLayerOplog::try_archive_blocking(&oplog).await;

                if i % 2 == 1 {
                    debug!("Adding more oplog entries to primary");
                    oplog
                        .add_and_commit(OplogEntry::log(
                            LogLevel::Debug,
                            "test".to_string(),
                            "test".to_string(),
                        ))
                        .await;
                }
            }
            _ => unreachable!(),
        }
    }

    debug!(
        "Created {}/{}/{} workers, waiting for background processes",
        primary_workers.len(),
        secondary_workers.len(),
        tertiary_workers.len()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut cursor = ScanCursor::default();
    let mut result = Vec::new();
    let page_size = 10;
    loop {
        let (new_cursor, ids) = oplog_service
            .scan_for_component(&project_id, &component_id, cursor, page_size)
            .await
            .unwrap();
        debug!("Got {} elements, new cursor is {}", ids.len(), new_cursor);
        result.extend(ids);
        if new_cursor.is_finished() {
            break;
        } else {
            cursor = new_cursor;
        }
    }

    assert_eq!(result.len(), 100);
}
