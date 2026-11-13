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
use crate::services::oplog::compressed::CompressedOplogArchiveService;
use crate::services::oplog::primary::PrimaryOplogService;
use crate::services::oplog::tests::{
    default_execution_status, default_last_known_status, make_agent_metadata,
};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use golem_common::model::oplog::{AgentError, OplogErrorKind};
use golem_common::model::{RetryConfig, Timestamp};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use nonempty_collections::nev;
use proptest::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;

test_r::enable!();

#[test]
fn archive_failure_backoff_grows_and_is_capped() {
    assert_eq!(archive_retry_delay(1), Duration::from_secs(1));
    assert_eq!(archive_retry_delay(2), Duration::from_secs(2));
    assert_eq!(archive_retry_delay(3), Duration::from_secs(4));
    assert_eq!(archive_retry_delay(32), ARCHIVE_RETRY_MAX_DELAY);
}

#[test]
async fn archive_failure_backoff_is_per_agent_and_resets_on_success() {
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let primary: Arc<dyn OplogService> = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            100,
            100,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed,
        1,
        RetryConfig::default(),
    ));
    let service = MultiLayerOplogService::new(primary, nev![archive], 100, 100);
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();
    let failed = OwnedAgentId::new(
        environment_id,
        &AgentId {
            component_id,
            agent_id: "failed".to_string(),
        },
    );
    let healthy = OwnedAgentId::new(
        environment_id,
        &AgentId {
            component_id,
            agent_id: "healthy".to_string(),
        },
    );

    service.record_archive_failure(&failed, ArchiveSource::Lower(0));
    assert!(!service.begin_archive_attempt(&failed, ArchiveSource::Lower(0)));
    assert!(service.begin_archive_attempt(&failed, ArchiveSource::Primary));
    assert!(service.begin_archive_attempt(&healthy, ArchiveSource::Lower(0)));

    service
        .archive_failures
        .lock()
        .unwrap()
        .get_mut(&(failed.clone(), ArchiveSource::Lower(0)))
        .unwrap()
        .retry_at = Instant::now();
    assert!(service.begin_archive_attempt(&failed, ArchiveSource::Lower(0)));
    service.record_archive_success(&failed, ArchiveSource::Lower(0));
    assert!(service.begin_archive_attempt(&failed, ArchiveSource::Lower(0)));
}

async fn open_idle_multilayer(
    agent_name: &str,
) -> (Arc<dyn Oplog>, MultiLayerOplogService, AgentId) {
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let primary_service = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            100,
            100,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let archive: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed,
        1,
        RetryConfig::default(),
    ));
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: agent_name.to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let primary = primary_service
        .open(
            &mut primary_service.lock_lifecycle(&agent_id).await,
            &owned_agent_id,
            AgentMode::Durable,
            None,
            make_agent_metadata(agent_id.clone(), account_id, environment_id),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();
    let service = MultiLayerOplogService::new(primary_service, nev![archive], 100, 100);
    let oplog = MultiLayerOplog::new(
        owned_agent_id,
        AgentMode::Durable,
        account_id,
        primary,
        service.clone(),
        Box::new(|| {}),
        None,
    )
    .await
    .unwrap();
    (oplog, service, agent_id)
}

#[test]
async fn executor_shutdown_fence_stops_transfer_while_handle_survives() {
    let (oplog, _, _) = open_idle_multilayer("shutdown-transfer-surviving-handle").await;
    let transfer_closed = oplog.closed();
    let shutdown = oplog.executor_shutdown_handle();

    shutdown.fence();
    shutdown.fence();

    tokio::time::timeout(Duration::from_secs(1), transfer_closed)
        .await
        .expect("multi-layer transfer remained live after executor shutdown preparation")
        .unwrap();
    assert!(Arc::strong_count(&oplog) > 0);
}

#[test]
async fn saved_executor_shutdown_handle_settles_after_outer_handle_drops() {
    let (oplog, _, _) = open_idle_multilayer("saved-shutdown-completion").await;
    let shutdown = oplog.executor_shutdown_handle();

    shutdown.fence();
    drop(oplog);

    tokio::time::timeout(Duration::from_secs(1), shutdown.close_and_wait())
        .await
        .expect("saved multi-layer shutdown handle did not settle")
        .unwrap();
}

#[test]
async fn old_shutdown_fence_does_not_unregister_newer_transfer() {
    let (old, service, agent_id) = open_idle_multilayer("replacement-transfer").await;
    let shutdown = old.executor_shutdown_handle();
    let newer = new_transfer_fiber();
    service.register_transfer(agent_id.clone(), &newer);
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let transfer = tokio::spawn(async move {
        if start_rx.await.is_ok() {
            let _ = release_rx.await;
        }
    });
    MultiLayerOplogService::start_transfer(&newer, start_tx, transfer).await;

    shutdown.fence();
    drop(old);

    assert!(
        service
            .transfer_fibers
            .lock()
            .unwrap()
            .get(&agent_id)
            .is_some_and(|registered| registered.ptr_eq(&Arc::downgrade(&newer)))
    );
    release_tx.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        MultiLayerOplogService::transfer_closed(&newer),
    )
    .await
    .expect("newer transfer was stopped by old-generation shutdown fence")
    .unwrap();
}

#[test]
async fn observer_open_preserves_cursor_when_primary_grows() {
    observer_open_after_primary_growth(0).await;
}

#[test]
async fn observer_open_uses_deep_archive_watermark_when_primary_grows() {
    observer_open_after_primary_growth(3).await;
}

async fn observer_open_after_primary_growth(archived: u64) {
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let blob = Arc::new(InMemoryBlobStorage::new());
    let writer_service = PrimaryOplogService::new(
        indexed.clone(),
        blob.clone(),
        100,
        100,
        100,
        32,
        RetryConfig::default(),
    )
    .await;
    let observer_service = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            blob,
            100,
            100,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let first: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed.clone(),
        1,
        RetryConfig::default(),
    ));
    let deepest: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed,
        2,
        RetryConfig::default(),
    ));
    let account = AccountId::new();
    let environment = EnvironmentId::new();
    let agent = AgentId {
        component_id: ComponentId::new(),
        agent_id: "observer".into(),
    };
    let owned = OwnedAgentId::new(environment, &agent);
    let metadata = make_agent_metadata(agent.clone(), account, environment);
    let writer = writer_service
        .open(
            &mut writer_service.lock_lifecycle(&agent).await,
            &owned,
            AgentMode::Durable,
            None,
            metadata.clone(),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();
    let entries = (1..=archived + 10)
        .map(|index| {
            OplogEntry::Error {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                kind: OplogErrorKind::Invocation,
                error: AgentError::Unknown(index.to_string()),
                retry_from: OplogIndex::NONE,
                retry_policy_state: None,
            }
            .rounded()
        })
        .collect::<Vec<_>>();
    let captured = archived + 2;
    for entry in &entries[..captured as usize] {
        writer.add(entry.clone()).await.unwrap();
    }
    writer.commit(CommitLevel::Always).await.unwrap();
    let deep_archive = deepest
        .open(&owned, AgentMode::Durable, None)
        .await
        .unwrap();
    if archived > 0 {
        let prefix = entries[..archived as usize]
            .iter()
            .enumerate()
            .map(|(index, entry)| (OplogIndex::from_u64(index as u64 + 1), entry.clone()))
            .collect::<Vec<_>>();
        deep_archive.append(&prefix).await.unwrap();
        writer.drop_prefix(OplogIndex::from_u64(archived)).await;
    }
    let observer = observer_service
        .open(
            &mut observer_service.lock_lifecycle(&agent).await,
            &owned,
            AgentMode::Durable,
            None,
            metadata,
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        observer.current_oplog_index().await,
        OplogIndex::from_u64(captured)
    );
    for entry in &entries[captured as usize..] {
        writer.add(entry.clone()).await.unwrap();
    }
    writer.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(observer.length().await.unwrap(), 10);

    let service = MultiLayerOplogService::new(observer_service, nev![first, deepest], 100, 100);
    let observer = MultiLayerOplog::new(
        owned,
        AgentMode::Durable,
        account,
        observer,
        service,
        Box::new(|| {}),
        None,
    )
    .await
    .unwrap();
    let layered = downcast_oplog::<MultiLayerOplog>(&observer).unwrap();
    assert_eq!(
        layered.last_transfer_point.get(),
        OplogIndex::from_u64(archived)
    );
    assert_eq!(
        observer.current_oplog_index().await,
        OplogIndex::from_u64(captured)
    );
    for (index, entry) in entries[..captured as usize].iter().enumerate() {
        assert_eq!(
            &observer
                .read(OplogIndex::from_u64(index as u64 + 1))
                .await
                .unwrap(),
            entry
        );
    }
    assert_eq!(
        writer.current_oplog_index().await,
        OplogIndex::from_u64(archived + 10)
    );
    assert_eq!(writer.length().await.unwrap(), 10);
    assert_eq!(
        deep_archive.get_last_index().await.unwrap(),
        OplogIndex::from_u64(archived)
    );
}

#[derive(Debug, Clone)]
enum Operation {
    Construct,
    Delete,
    Yield,
}

fn operations() -> impl Strategy<Value = Vec<Operation>> {
    prop::collection::vec(
        prop_oneof![
            Just(Operation::Construct),
            Just(Operation::Delete),
            Just(Operation::Yield),
        ],
        1..32,
    )
}

async fn cancel_transfer(transfer_fiber: &TransferFiber) {
    MultiLayerOplogService::cancel_transfer(transfer_fiber)
        .await
        .unwrap();
}

proptest! {
    /// A transfer registered before its task is constructed must remain inert when deletion wins
    /// the race. This is the lifecycle fence used by both durable and ephemeral oplogs.
    #[test]
    fn deleted_transfer_never_starts(operations in operations()) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async move {
            let transfer_fiber = new_transfer_fiber();
            let started = Arc::new(AtomicUsize::new(0));
            let mut write_gate = None;
            let mut constructed = false;
            let mut deleted = false;

            for operation in operations {
                match operation {
                    Operation::Construct if !constructed => {
                        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
                        let (write_tx, write_rx) = tokio::sync::oneshot::channel();
                        let started = started.clone();
                        let transfer = tokio::spawn(async move {
                            if start_rx.await.is_ok() && write_rx.await.is_ok() {
                                started.fetch_add(1, Ordering::SeqCst);
                            }
                        });

                        MultiLayerOplogService::start_transfer(
                            &transfer_fiber,
                            start_tx,
                            transfer,
                        )
                        .await;
                        write_gate = Some(write_tx);
                        constructed = true;
                    }
                    Operation::Delete if !deleted => {
                        cancel_transfer(&transfer_fiber).await;
                        deleted = true;
                    }
                    Operation::Yield => tokio::task::yield_now().await,
                    _ => {}
                }
            }

            if let Some(write_gate) = write_gate {
                if deleted {
                    drop(write_gate);
                    prop_assert_eq!(started.load(Ordering::SeqCst), 0);
                } else {
                    let _ = write_gate.send(());
                    let transfer = {
                        transfer_fiber
                            .lock()
                            .unwrap()
                            .closed
                            .clone()
                    };
                    if let Some(transfer) = transfer {
                        transfer.await.unwrap();
                    }
                    prop_assert_eq!(started.load(Ordering::SeqCst), 1);
                }
            }

            Ok(())
        })
        .unwrap();
    }
}
