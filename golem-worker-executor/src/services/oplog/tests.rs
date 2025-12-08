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

use super::*;
use crate::services::oplog::compressed::CompressedOplogArchiveService;
use crate::services::oplog::multilayer::OplogArchiveService;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::redis::RedisIndexedStorage;
use crate::storage::indexed::IndexedStorage;
use assert2::check;
use golem_common::config::RedisConfig;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::oplog::{LogLevel, WorkerError};
use golem_common::model::regions::OplogRegion;
use golem_common::model::WorkerStatusRecord;
use golem_common::redis::RedisPool;
use golem_common::tracing::{init_tracing, TracingConfig};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_wasm::{FromValue, FromValueAndType, IntoValue, IntoValueAndType};
use nonempty_collections::nev;
use std::collections::HashSet;
use std::sync::RwLock;
use std::time::Instant;
use test_r::{test, test_dep};
use tracing::{debug, info};
use uuid::Uuid;

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

fn default_last_known_status() -> read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord> {
    read_only_lock::tokio::ReadOnlyLock::new(Arc::new(tokio::sync::RwLock::new(
        WorkerStatusRecord::default(),
    )))
}

fn default_execution_status(
    agent_mode: AgentMode,
) -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        agent_mode,
        timestamp: Timestamp::now_utc(),
    })))
}

#[test]
async fn open_add_and_read_back(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 1, 100).await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();

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
async fn open_add_and_read_back_many(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 1, 100).await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();
    let entry4 = OplogEntry::interrupted().rounded();

    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;
    oplog.add(entry4.clone()).await; // uncommitted entry

    let entries = oplog
        .read_many(OplogIndex::INITIAL, 4)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2, entry3, entry4]);
}

#[test]
async fn open_add_and_read_back_ephemeral(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();

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
async fn open_add_and_read_back_many_ephemeral(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let primary_oplog_service = Arc::new(
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId::new(),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Ephemeral),
        )
        .await;

    let entry1 = OplogEntry::jump(OplogRegion {
        start: OplogIndex::from_u64(5),
        end: OplogIndex::from_u64(12),
    })
    .rounded();
    let entry2 = OplogEntry::suspend().rounded();
    let entry3 = OplogEntry::exited().rounded();
    let entry4 = OplogEntry::interrupted().rounded();

    oplog.add(entry1.clone()).await;
    oplog.add(entry2.clone()).await;
    oplog.add(entry3.clone()).await;
    oplog.commit(CommitLevel::Always).await;
    oplog.add(entry4.clone()).await; // uncommitted

    let entries = oplog
        .read_many(OplogIndex::INITIAL, 4)
        .await
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(entries, vec![entry1, entry2, entry3, entry4]);
}

#[test]
async fn entries_with_small_payload(_tracing: &Tracing) {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 1, 100).await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let last_oplog_idx = oplog.current_oplog_index().await;
    let entry1 = oplog
        .add_imported_function_invoked(
            HostFunctionName::Custom("f1".to_string()),
            &HostRequest::Custom("request".into_value_and_type()),
            &HostResponse::Custom("response".into_value_and_type()),
            DurableFunctionType::ReadRemote,
        )
        .await
        .unwrap()
        .rounded();
    let entry2 = oplog
        .add_exported_function_invoked(
            "f2".to_string(),
            &vec!["request".into_value()],
            IdempotencyKey::fresh(),
            InvocationContextStack::fresh_rounded(),
        )
        .await
        .unwrap()
        .rounded();
    let entry3 = oplog
        .add_exported_function_completed(&Some("response".into_value_and_type()), 42)
        .await
        .unwrap()
        .rounded();

    let desc = oplog
        .create_snapshot_based_update_description(ComponentRevision(11), vec![1, 2, 3])
        .await
        .unwrap();
    let entry4 = OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc(),
        description: desc.clone(),
    }
    .rounded();
    oplog.add(entry4.clone()).await;

    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await.rounded();
    let r2 = oplog.read(last_oplog_idx.next().next()).await.rounded();
    let r3 = oplog
        .read(last_oplog_idx.next().next().next())
        .await
        .rounded();
    let r4 = oplog
        .read(last_oplog_idx.next().next().next().next())
        .await
        .rounded();

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);
    assert_eq!(r4, entry4);

    let entries = oplog_service
        .read(&owned_worker_id, last_oplog_idx.next(), 4)
        .await;
    assert_eq!(
        entries
            .into_values()
            .map(|entry| entry.rounded())
            .collect::<Vec<_>>(),
        vec![
            entry1.clone(),
            entry2.clone(),
            entry3.clone(),
            entry4.clone(),
        ]
    );

    let p1 = match entry1 {
        OplogEntry::ImportedFunctionInvoked { response, .. } => {
            let response = oplog_service
                .download_payload(&owned_worker_id, response)
                .await
                .unwrap();
            match response {
                HostResponse::Custom(vnt) => String::from_value_and_type(vnt).unwrap(),
                _ => panic!("unexpected entry"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p2 = match entry2 {
        OplogEntry::ExportedFunctionInvoked { request, .. } => {
            let request = oplog_service
                .download_payload(&owned_worker_id, request)
                .await
                .unwrap();
            match request.first() {
                Some(value) => String::from_value(value.clone()).unwrap(),
                _ => panic!("unexpected entry"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p3 = match entry3 {
        OplogEntry::ExportedFunctionCompleted { response, .. } => {
            let response = oplog_service
                .download_payload(&owned_worker_id, response)
                .await
                .unwrap();
            match response {
                Some(vnt) => String::from_value_and_type(vnt).unwrap(),
                _ => panic!("unexpected entry"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p4 = oplog
        .get_upload_description_payload(desc)
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
    let oplog_service = PrimaryOplogService::new(indexed_storage, blob_storage, 1, 1, 100).await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let large_payload1 = vec![0u8; 1024 * 1024];
    let large_payload2 = vec![1u8; 1024 * 1024];
    let large_payload3 = vec![2u8; 1024 * 1024];
    let large_payload4 = vec![3u8; 1024 * 1024];

    let last_oplog_idx = oplog.current_oplog_index().await;
    let entry1 = oplog
        .add_imported_function_invoked(
            HostFunctionName::Custom("f1".to_string()),
            &HostRequest::Custom("request".into_value_and_type()),
            &HostResponse::Custom(large_payload1.clone().into_value_and_type()),
            DurableFunctionType::ReadRemote,
        )
        .await
        .unwrap()
        .rounded();
    let entry2 = oplog
        .add_exported_function_invoked(
            "f2".to_string(),
            &vec![large_payload2.clone().into_value()],
            IdempotencyKey::fresh(),
            InvocationContextStack::fresh_rounded(),
        )
        .await
        .unwrap()
        .rounded();
    let entry3 = oplog
        .add_exported_function_completed(&Some(large_payload3.clone().into_value_and_type()), 42)
        .await
        .unwrap()
        .rounded();

    let desc = oplog
        .create_snapshot_based_update_description(ComponentRevision(11), large_payload4.clone())
        .await
        .unwrap();
    let entry4 = OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc(),
        description: desc.clone(),
    }
    .rounded();
    oplog.add(entry4.clone()).await;

    oplog.commit(CommitLevel::Always).await;

    let r1 = oplog.read(last_oplog_idx.next()).await.rounded();
    let r2 = oplog.read(last_oplog_idx.next().next()).await.rounded();
    let r3 = oplog
        .read(last_oplog_idx.next().next().next())
        .await
        .rounded();
    let r4 = oplog
        .read(last_oplog_idx.next().next().next().next())
        .await
        .rounded();

    assert_eq!(r1, entry1);
    assert_eq!(r2, entry2);
    assert_eq!(r3, entry3);
    assert_eq!(r4, entry4);

    let entries = oplog_service
        .read(&owned_worker_id, last_oplog_idx.next(), 4)
        .await;
    assert_eq!(
        entries
            .into_values()
            .map(|entry| entry.rounded())
            .collect::<Vec<_>>(),
        vec![
            entry1.clone(),
            entry2.clone(),
            entry3.clone(),
            entry4.clone(),
        ]
    );

    let p1 = match entry1 {
        OplogEntry::ImportedFunctionInvoked { response, .. } => {
            let response = oplog_service
                .download_payload(&owned_worker_id, response)
                .await
                .unwrap();
            match response {
                HostResponse::Custom(vnt) => Vec::<u8>::from_value_and_type(vnt).unwrap(),
                _ => panic!("unexpected entry"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p2 = match entry2 {
        OplogEntry::ExportedFunctionInvoked { request, .. } => {
            let request = oplog_service
                .download_payload(&owned_worker_id, request)
                .await
                .unwrap();
            match request.first() {
                Some(value) => Vec::<u8>::from_value(value.clone()).unwrap(),
                _ => panic!("unexpected entry"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p3 = match entry3 {
        OplogEntry::ExportedFunctionCompleted { response, .. } => {
            let response = oplog_service
                .download_payload(&owned_worker_id, response)
                .await
                .unwrap();
            match response {
                Some(vnt) => Vec::<u8>::from_value_and_type(vnt).unwrap(),
                _ => panic!("unexpected entry"),
            }
        }
        _ => panic!("unexpected entry"),
    };
    let p4 = oplog
        .get_upload_description_payload(desc)
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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

    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    let mut entries = Vec::new();

    for i in 0..n {
        let entry = oplog
            .add_imported_function_invoked(
                HostFunctionName::Custom("test-function".to_string()),
                &HostRequest::Custom(i.into_value_and_type()),
                &HostResponse::Custom("response".into_value_and_type()),
                DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap()
            .rounded();
        oplog.commit(CommitLevel::Always).await;
        entries.push(entry);
    }

    let start = Instant::now();
    loop {
        let primary_length = primary_oplog_service
            .open(
                &owned_worker_id,
                primary_oplog_service.get_last_index(&owned_worker_id).await,
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
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
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    let timestamp = Timestamp::now_utc();
    let mut entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            }
            .rounded()
        })
        .collect();

    let initial_oplog_idx = oplog.current_oplog_index().await;

    for entry in &entries {
        oplog.add(entry.clone()).await;
    }
    oplog.commit(CommitLevel::Always).await;
    let uncommitted1 = OplogEntry::interrupted().rounded();
    let uncommitted2 = OplogEntry::suspend().rounded();
    oplog.add(uncommitted1.clone()).await;
    oplog.add(uncommitted2.clone()).await;

    entries.push(uncommitted1);
    entries.push(uncommitted2);

    tokio::time::sleep(Duration::from_secs(2)).await;

    let primary_length = primary_oplog_service
        .open(
            &owned_worker_id,
            primary_oplog_service.get_last_index(&owned_worker_id).await,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
    let original_first10 = entries.iter().take(10).cloned().collect::<Vec<_>>();

    assert_eq!(first10.into_values().collect::<Vec<_>>(), original_first10);

    let last10 = oplog
        .read_many(oplog.current_oplog_index().await.subtract(10).next(), 10)
        .await
        .into_values()
        .collect::<Vec<_>>();

    let original_last10 = entries.into_iter().rev().take(10).rev().collect::<Vec<_>>();
    assert_eq!(last10, original_last10);
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    let timestamp = Timestamp::now_utc();
    let create_entry = OplogEntry::Create {
        timestamp,
        worker_id: WorkerId {
            component_id: ComponentId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        },
        component_revision: ComponentRevision(1),
        env: vec![],
        wasi_config_vars: BTreeMap::new(),
        environment_id,
        created_by: account_id,
        parent: None,
        component_size: 0,
        initial_total_linear_memory_size: 0,
        initial_active_plugins: HashSet::new(),
        original_phantom_id: None,
    }
    .rounded();

    let oplog = oplog_service
        .create(
            &owned_worker_id,
            create_entry.clone(),
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    info!("FIRST OPEN");
    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;
    info!("FIRST OPEN DONE");

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            }
            .rounded()
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
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else if reopen == Reopen::Full {
        drop(oplog);
        primary_oplog_service = Arc::new(
            PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100)
                .await,
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
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else {
        oplog
    };

    let entries: Vec<OplogEntry> = (100..1000)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            }
            .rounded()
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
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else if reopen == Reopen::Full {
        drop(oplog);
        primary_oplog_service = Arc::new(
            PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100)
                .await,
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
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
            )
            .await
    } else {
        oplog
    };

    oplog
        .add(
            OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown("last".to_string()),
                retry_from: OplogIndex::NONE,
            }
            .rounded(),
        )
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
        OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("0".to_string()),
            retry_from: OplogIndex::NONE,
        }
        .rounded()
    );
    assert_eq!(
        entry2.get(&OplogIndex::from_u64(100)).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("99".to_string()),
            retry_from: OplogIndex::NONE,
        }
        .rounded()
    );
    assert_eq!(
        entry3.get(&OplogIndex::from_u64(1000)).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("999".to_string()),
            retry_from: OplogIndex::NONE,
        }
        .rounded()
    );
    assert_eq!(
        entry4.get(&OplogIndex::from_u64(1001)).unwrap().clone(),
        OplogEntry::Error {
            timestamp,
            error: WorkerError::Unknown("last".to_string()),
            retry_from: OplogIndex::NONE,
        }
        .rounded()
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
    let oplog = oplog_service
        .open(
            &owned_worker_id,
            last_oplog_index,
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
        )
        .await;

    // As we add 100 entries at once, and that exceeds the limit, we expect that all entries have
    // been moved to the secondary layer. By doing this 10 more times, we end up having all entries
    // in the tertiary layer.

    for _ in 0..10 {
        let timestamp = Timestamp::now_utc();
        let entries: Vec<OplogEntry> = (0..100)
            .map(|i| {
                OplogEntry::Error {
                    timestamp,
                    error: WorkerError::Unknown(i.to_string()),
                    retry_from: OplogIndex::NONE,
                }
                .rounded()
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
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let worker_id = WorkerId {
        component_id: ComponentId(Uuid::new_v4()),
        worker_name: "test".to_string(),
    };
    let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);

    let timestamp = Timestamp::now_utc();
    let entries: Vec<OplogEntry> = (0..100)
        .map(|i| {
            OplogEntry::Error {
                timestamp,
                error: WorkerError::Unknown(i.to_string()),
                retry_from: OplogIndex::NONE,
            }
            .rounded()
        })
        .collect();

    // Adding 100 entries to the primary oplog, schedule archive and immediately drop the oplog
    let archive_result = {
        let last_oplog_index = oplog_service.get_last_index(&owned_worker_id).await;
        let oplog = oplog_service
            .open(
                &owned_worker_id,
                last_oplog_index,
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
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
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
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
            WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
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
        PrimaryOplogService::new(indexed_storage.clone(), blob_storage.clone(), 1, 1, 100).await,
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
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    // Adding some workers
    let mut primary_workers = Vec::new();
    let mut secondary_workers = Vec::new();
    let mut tertiary_workers = Vec::new();
    for i in 0..100 {
        let worker_id = WorkerId {
            component_id,
            worker_name: format!("worker-{i}"),
        };
        let create_entry = OplogEntry::create(
            worker_id.clone(),
            ComponentRevision(1),
            Vec::new(),
            environment_id,
            account_id,
            None,
            100,
            100,
            HashSet::new(),
            BTreeMap::new(),
            None,
        );

        let owned_worker_id = OwnedWorkerId::new(&environment_id, &worker_id);
        let oplog = oplog_service
            .create(
                &owned_worker_id,
                create_entry,
                WorkerMetadata::default(worker_id.clone(), account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
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
            .scan_for_component(&environment_id, &component_id, cursor, page_size)
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
