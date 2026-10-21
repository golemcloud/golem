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
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use test_r::test;
use tokio::sync::oneshot;

fn oplog_namespace(name: &str) -> IndexedStorageNamespace {
    IndexedStorageNamespace::OpLog {
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: name.to_string(),
        },
        agent_mode: AgentMode::Durable,
    }
}

async fn warm(storage: &MultiSqliteIndexedStorage, db: &str) {
    storage
        .with_database(db.to_string(), |_| async { Ok(()) })
        .await
        .unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn cancelled_warm_read_poisons_before_exclusive_admission() {
    use golem_service_base::db::LabelledPoolTransaction;
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("cancelled-read");
    storage
        .append(
            "test",
            "seed",
            "entry",
            ns.clone(),
            "key",
            3,
            vec![37],
            None,
        )
        .await
        .unwrap();
    let db = storage.namespace_to_db(&ns).await;
    let file = FileLifetime::get(storage.root.root_dir.join(&db));
    let cached = storage.root.cache.get(&db).await.unwrap();
    let connection = cached.hold_connection_for_test().await;
    drop(cached);
    let worker = storage.clone();
    let reading_ns = ns.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let read = tokio::spawn(async move {
        worker
            .with_existing(db, move |pool| async move {
                entered_tx.send(()).unwrap();
                pool.length("test", "read", reading_ns, "key").await
            })
            .await
    });
    entered_rx.await.unwrap();
    let exclusive = file.gate.clone().write_owned();
    tokio::pin!(exclusive);
    assert!(futures::poll!(&mut exclusive).is_pending());
    read.abort();
    assert!(read.await.unwrap_err().is_cancelled());
    let lease = exclusive.await;
    assert!(file.is_poisoned());
    drop(lease);
    // The held reader also proves poisoned deletion skips the emptiness query.
    storage
        .delete("test", "delete", ns.clone(), "key")
        .await
        .unwrap();
    assert!(file.path.exists());
    assert!(
        storage
            .root
            .epoch_storage()
            .await
            .unwrap()
            .database_epoch_snapshot(file.path.file_name().unwrap().to_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    connection.rollback().await.unwrap();
    assert_eq!(storage.length("test", "read", ns, "key").await.unwrap(), 0);

    let other = oplog_namespace("successful-read");
    storage
        .append(
            "test",
            "seed",
            "entry",
            other.clone(),
            "key",
            5,
            vec![53],
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        storage
            .length("test", "read", other.clone(), "key")
            .await
            .unwrap(),
        1
    );
    let other_file = FileLifetime::get(
        storage
            .root
            .root_dir
            .join(storage.namespace_to_db(&other).await),
    );
    assert!(!other_file.is_poisoned());
    storage
        .delete("test", "delete", other, "key")
        .await
        .unwrap();
    assert!(!other_file.path.exists());
}

#[test]
async fn cancelling_read_before_admission_does_not_poison() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("not-admitted");
    storage
        .append(
            "test",
            "seed",
            "entry",
            ns.clone(),
            "key",
            7,
            vec![71],
            None,
        )
        .await
        .unwrap();
    let file = FileLifetime::get(
        storage
            .root
            .root_dir
            .join(storage.namespace_to_db(&ns).await),
    );
    let lease = file.gate.clone().write_owned().await;
    let mut read = Box::pin(storage.length("test", "read", ns.clone(), "key"));
    assert!(futures::poll!(&mut read).is_pending());
    drop(read);
    assert!(!file.is_poisoned());
    drop(lease);
    assert_eq!(
        storage
            .length("test", "read", ns.clone(), "key")
            .await
            .unwrap(),
        1
    );
    storage.delete("test", "delete", ns, "key").await.unwrap();
    assert!(!file.path.exists());
}

#[test]
#[test_r::timeout("30s")]
async fn shared_readers_overlap_exclusive_waits_and_other_file_progresses() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 2, false);
    let second = MultiSqliteIndexedStorage::new(&dir.path().join("."), 2, false);
    assert!(Arc::ptr_eq(&storage.root, &second.root));
    warm(&storage, "first.db").await;
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let first = tokio::spawn(async move {
        second
            .with_database("first.db".into(), move |_| async move {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(())
            })
            .await
    });
    entered_rx.await.unwrap();
    // A second same-file reader completes while the first is held.
    warm(&storage, "first.db").await;
    let file = FileLifetime::get(storage.root.root_dir.join("first.db"));
    let exclusive = file.gate.clone().write_owned();
    tokio::pin!(exclusive);
    assert!(futures::poll!(&mut exclusive).is_pending());
    warm(&storage, "other.db").await;
    release_tx.send(()).unwrap();
    first.await.unwrap().unwrap();
    drop(exclusive.await);
}

#[test]
#[test_r::timeout("30s")]
async fn eviction_during_operation_keeps_registration_until_completion() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    warm(&storage, "active.db").await;
    let file = FileLifetime::get(storage.root.root_dir.join("active.db"));
    let worker = storage.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let work = tokio::spawn(async move {
        worker
            .with_database("active.db".into(), move |s| async move {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                // Retain the actual managed value through operation completion.
                assert!(s.storage.is_some());
                Ok(())
            })
            .await
    });
    entered_rx.await.unwrap();
    storage.root.cache.remove(&"active.db".to_string()).await;
    assert_eq!(file.registrations.load(Ordering::Acquire), 1);
    let retired = file.wait_retired();
    tokio::pin!(retired);
    assert!(futures::poll!(&mut retired).is_pending());
    release_tx.send(()).unwrap();
    work.await.unwrap().unwrap();
    retired.await;
    assert!(!file.is_poisoned());
}

#[test]
#[test_r::timeout("30s")]
async fn retirement_wait_includes_earlier_evicted_generation() {
    use golem_service_base::db::{LabelledPoolTransaction, PoolApi};
    for entrypoint in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("retiring");
        storage
            .set_key_epoch("test", "epoch", ns.clone(), "key", ShardEpoch(11))
            .await
            .unwrap();
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "key",
                3,
                vec![37],
                Some(ShardEpoch(11)),
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        let cached = storage.root.cache.get(&db).await.unwrap();
        // A real acquired connection holds SQLx graceful close, with no cleanup gate involved.
        let mut connection = cached.hold_connection_for_test().await;
        storage.root.cache.remove(&db).await;
        drop(cached);
        let worker = storage.clone();
        let requested_ns = ns.clone();
        let reopened = tokio::spawn(async move {
            match entrypoint {
                0 => {
                    worker
                        .read("test", "read", "entry", requested_ns, "key", 1, 9)
                        .await
                }
                1 => worker
                    .set_key_epoch("test", "epoch", requested_ns, "key", ShardEpoch(19))
                    .await
                    .map(|_| Vec::new()),
                2 => worker
                    .delete("test", "delete", requested_ns, "key")
                    .await
                    .map(|_| Vec::new()),
                _ => worker
                    .drop_prefix(
                        "test",
                        "prefix",
                        requested_ns,
                        "key",
                        3,
                        Some(ShardEpoch(11)),
                    )
                    .await
                    .map(|_| Vec::new()),
            }
        });
        file.waiting_retired.notified().await;
        assert!(!reopened.is_finished());
        assert!(storage.root.cache.get(&db).await.is_none());
        assert_eq!(file.registrations.load(Ordering::Acquire), 1);
        warm(&storage, "unrelated.db").await;
        let unchanged: (i64, i64) = connection.fetch_one_as(sqlx::query_as(
                "SELECT (SELECT epoch FROM indexed_key_epoch WHERE key = 'key'), (SELECT COUNT(*) FROM index_storage WHERE key = 'key');",
            )).await.unwrap();
        assert_eq!(unchanged, (11, 1));
        connection.rollback().await.unwrap();
        let result = reopened.await.unwrap().unwrap();
        if entrypoint == 0 {
            assert_eq!(result, vec![(3, vec![37])]);
        }
        assert_eq!(
            storage
                .read("test", "read", "entry", ns.clone(), "key", 1, 9)
                .await
                .unwrap(),
            if entrypoint < 2 {
                vec![(3, vec![37])]
            } else {
                Vec::new()
            }
        );
        if entrypoint == 1 {
            assert!(matches!(
                storage
                    .append(
                        "test",
                        "stale",
                        "entry",
                        ns,
                        "key",
                        4,
                        vec![8],
                        Some(ShardEpoch(11))
                    )
                    .await,
                Err(IndexedStorageError::Fenced {
                    actual: Some(ShardEpoch(19)),
                    ..
                })
            ));
        }
        storage.root.cache.remove(&db).await;
        file.wait_retired().await;
        assert!(!file.is_poisoned());
    }
}

#[test]
#[test_r::timeout("30s")]
async fn cancellation_retains_admission_until_owned_operation_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let worker = storage.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (finished_tx, finished_rx) = oneshot::channel();
    let request = tokio::spawn(async move {
        worker
            .with_database("cancel.db".into(), move |_| async move {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                finished_tx.send(()).unwrap();
                Ok(())
            })
            .await
    });
    entered_rx.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    let file = FileLifetime::get(storage.root.root_dir.join("cancel.db"));
    let exclusive = file.gate.clone().write_owned();
    tokio::pin!(exclusive);
    assert!(futures::poll!(&mut exclusive).is_pending());
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    drop(exclusive.await);
    assert!(!file.is_poisoned());
}

#[test]
#[test_r::timeout("30s")]
async fn completed_errors_retire_without_poison_and_reopening_waits() {
    use golem_service_base::db::LabelledPoolTransaction;
    for warm_read in [false, true] {
        for kind in 0..3 {
            let dir = tempfile::tempdir().unwrap();
            let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
            let db = "failed.db".to_string();
            warm(&storage, &db).await;
            let file = FileLifetime::get(storage.root.root_dir.join(&db));
            let cached = storage.root.cache.get(&db).await.unwrap();
            let connection = cached.hold_connection_for_test().await;
            drop(cached);
            let failure = move |_| async move {
                Err::<(), _>(match kind {
                    0 => IndexedStorageError::Transient("completed failure".into()),
                    1 => IndexedStorageError::Indeterminate("completed failure".into()),
                    _ => IndexedStorageError::Other("completed failure".into()),
                })
            };
            let result = if warm_read {
                storage.with_existing(db.clone(), failure).await
            } else {
                storage
                    .with_access(db.clone(), Access::Create, failure)
                    .await
            };
            assert!(result.is_err());
            assert!(!file.is_poisoned());
            assert!(storage.root.cache.get(&db).await.is_none());
            assert!(file.path.exists());
            let worker = storage.clone();
            let reopened = tokio::spawn(async move { warm(&worker, &db).await });
            file.waiting_retired.notified().await;
            assert!(!reopened.is_finished());
            assert_eq!(file.registrations.load(Ordering::Acquire), 1);
            connection.rollback().await.unwrap();
            reopened.await.unwrap();
            assert!(!file.is_poisoned());
        }
    }
}

#[test]
async fn initialization_failure_poison_survives_reconstruction() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let path = storage.root.root_dir.join("bad.db");
    std::fs::create_dir(&path).unwrap();
    assert!(
        storage
            .with_database("bad.db".into(), |_| async { Ok(()) })
            .await
            .is_err()
    );
    drop(storage);
    assert!(FileLifetime::get(path).is_poisoned());
}

#[test]
#[test_r::timeout("30s")]
async fn cancelled_cold_request_finishes_loading_without_pending_cache_entry() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let file = FileLifetime::get(storage.root.root_dir.join("cold.db"));
    let exclusive = file.gate.clone().write_owned().await;
    let (finished_tx, finished_rx) = oneshot::channel();
    let mut request = Box::pin(
        storage.with_database("cold.db".into(), move |_| async move {
            finished_tx.send(()).unwrap();
            Ok(())
        }),
    );
    // Poll far enough to launch the owned task, then cancel only the originating wait.
    assert!(futures::poll!(&mut request).is_pending());
    drop(request);
    drop(exclusive);
    finished_rx.await.unwrap();
    warm(&storage, "cold.db").await;
    assert!(
        storage
            .root
            .cache
            .get(&"cold.db".to_string())
            .await
            .is_some()
    );
    assert!(!file.is_poisoned());
}

#[test]
#[test_r::timeout("30s")]
async fn root_reconstruction_keeps_previous_retiring_lifetime() {
    use golem_service_base::db::LabelledPoolTransaction;
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    warm(&storage, "old.db").await;
    let path = storage.root.root_dir.join("old.db");
    let file = FileLifetime::get(path.clone());
    let old_file = Arc::downgrade(&file);
    let old_root = Arc::downgrade(&storage.root);
    let cached = storage.root.cache.get(&"old.db".to_string()).await.unwrap();
    let connection = cached.hold_connection_for_test().await;
    drop(file);
    drop(cached);
    drop(storage);
    assert!(old_root.upgrade().is_none());
    let replacement = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let file = FileLifetime::get(path);
    assert!(Arc::ptr_eq(&old_file.upgrade().unwrap(), &file));
    let reopened = warm(&replacement, "old.db");
    tokio::pin!(reopened);
    assert!(futures::poll!(&mut reopened).is_pending());
    assert_eq!(file.registrations.load(Ordering::Acquire), 1);
    connection.rollback().await.unwrap();
    reopened.await;
    replacement.root.cache.remove(&"old.db".to_string()).await;
    file.wait_retired().await;
    assert!(!file.is_poisoned());
}

#[test]
fn interrupted_retirement_permanently_poisons_reconstructed_path() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let storage = {
        let _entered = runtime.enter();
        MultiSqliteIndexedStorage::new(dir.path(), 1, false)
    };
    let path = storage.root.root_dir.join("interrupted.db");
    runtime.block_on(warm(&storage, "interrupted.db"));
    let cached = runtime
        .block_on(storage.root.cache.get(&"interrupted.db".to_string()))
        .unwrap();
    let connection = runtime.block_on(cached.hold_connection_for_test());
    drop(cached);
    // Final cache drop publishes retirement to the captured runtime even outside its context.
    drop(storage);
    runtime.block_on(async {
        tokio::task::yield_now().await;
    });
    drop(runtime);
    drop(connection);
    let file = FileLifetime::get(path.clone());
    assert!(file.is_poisoned());
    drop(file);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let replacement = {
        let _entered = runtime.enter();
        MultiSqliteIndexedStorage::new(dir.path(), 1, false)
    };
    runtime.block_on(warm(&replacement, "interrupted.db"));
    assert!(FileLifetime::get(path).is_poisoned());
    drop(replacement);
}

#[test]
async fn append_many_preserves_per_agent_database_routing() {
    let tempdir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(tempdir.path(), 1, false);
    let first_namespace = oplog_namespace("first-agent");
    let second_namespace = oplog_namespace("second-agent");

    storage
        .append_many(
            "test",
            "append_many",
            "entry",
            &first_namespace,
            "shared-key",
            vec![(1, Bytes::from_static(b"first-agent-value"))].into(),
            None,
        )
        .await
        .unwrap();
    storage
        .append_many(
            "test",
            "append_many",
            "entry",
            &second_namespace,
            "shared-key",
            vec![(1, Bytes::from_static(b"second-agent-value"))].into(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .read("test", "read", "entry", first_namespace, "shared-key", 1, 1,)
            .await
            .unwrap(),
        vec![(1, b"first-agent-value".to_vec())]
    );
    assert_eq!(
        storage
            .read(
                "test",
                "read",
                "entry",
                second_namespace,
                "shared-key",
                1,
                1,
            )
            .await
            .unwrap(),
        vec![(1, b"second-agent-value".to_vec())]
    );
}

#[test]
async fn staging_publication_and_fencing_survive_reclamation() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let visible = oplog_namespace("staged");
    let IndexedStorageNamespace::OpLog {
        agent_id,
        agent_mode,
    } = visible.clone()
    else {
        unreachable!()
    };
    let stage = IndexedStorageNamespace::StagedOpLog {
        agent_id,
        agent_mode,
    };
    storage
        .append(
            "test",
            "append",
            "entry",
            stage.clone(),
            "stage",
            1,
            vec![1],
            None,
        )
        .await
        .unwrap();
    assert!(
        storage
            .move_if_absent("test", "publish", stage, "stage", visible.clone(), "key", 1)
            .await
            .unwrap()
    );
    storage
        .set_key_epoch("test", "epoch", visible.clone(), "key", ShardEpoch(7))
        .await
        .unwrap();
    assert!(matches!(
        storage
            .append(
                "test",
                "stale",
                "entry",
                visible.clone(),
                "key",
                2,
                vec![2],
                Some(ShardEpoch(6))
            )
            .await,
        Err(IndexedStorageError::Fenced { .. })
    ));
    let db = storage.namespace_to_db(&visible).await;
    assert!(!FileLifetime::get(storage.root.root_dir.join(&db)).is_poisoned());
    storage
        .delete_with_epoch("test", "delete", visible, "key", Some(ShardEpoch(7)))
        .await
        .unwrap();
    assert!(!storage.root.root_dir.join(db).exists());
}

#[test]
#[test_r::timeout("30s")]
async fn failed_staging_move_rolls_back_and_later_drain_reclaims() {
    for cold in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let visible = oplog_namespace("invalid-move");
        let IndexedStorageNamespace::OpLog {
            agent_id,
            agent_mode,
        } = visible.clone()
        else {
            unreachable!()
        };
        let stage = IndexedStorageNamespace::StagedOpLog {
            agent_id,
            agent_mode,
        };
        for id in [1, 3] {
            storage
                .append(
                    "test",
                    "seed",
                    "entry",
                    stage.clone(),
                    "source",
                    id,
                    vec![id as u8],
                    None,
                )
                .await
                .unwrap();
        }
        let db = storage.namespace_to_db(&stage).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        if cold {
            storage.root.cache.remove(&db).await;
            file.wait_retired().await;
        }
        assert!(matches!(
            storage
                .move_if_absent(
                    "test",
                    "move",
                    stage.clone(),
                    "source",
                    visible.clone(),
                    "target",
                    3
                )
                .await,
            Err(IndexedStorageError::Other(_))
        ));
        assert!(!file.is_poisoned());
        assert!(storage.root.cache.get(&db).await.is_none());
        assert!(
            !storage
                .exists("test", "target", visible, "target")
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read("test", "source", "entry", stage.clone(), "source", 1, 3)
                .await
                .unwrap(),
            vec![(1, vec![1]), (3, vec![3])]
        );
        storage
            .delete("test", "drain", stage, "source")
            .await
            .unwrap();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            assert!(!storage.root.root_dir.join(format!("{db}{suffix}")).exists());
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn reclamation_churn_does_not_retain_empty_epoch_snapshots() {
    use golem_service_base::db::{LabelledPoolTransaction, PoolApi};
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    for agent in 0..9 {
        let mut ns = oplog_namespace(&format!("churn-{agent}"));
        if agent % 3 == 2 {
            let IndexedStorageNamespace::OpLog {
                agent_id,
                agent_mode,
            } = ns
            else {
                unreachable!()
            };
            ns = IndexedStorageNamespace::CompressedOpLog {
                agent_id,
                agent_mode,
                level: 1,
            };
        }
        let epoch = (agent % 3 == 1).then_some(ShardEpoch(17));
        if let Some(epoch) = epoch {
            storage
                .set_key_epoch("test", "epoch", ns.clone(), "key", epoch)
                .await
                .unwrap();
        }
        storage
            .append(
                "test",
                "seed",
                "entry",
                ns.clone(),
                "key",
                1,
                vec![37],
                epoch,
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        match agent % 3 {
            0 => storage.delete("test", "drain", ns, "key").await.unwrap(),
            1 => storage
                .delete_with_epoch("test", "drain", ns, "key", epoch)
                .await
                .unwrap(),
            _ => storage
                .drop_prefix("test", "drain", ns, "key", 1, None)
                .await
                .unwrap(),
        }
        assert!(!storage.root.root_dir.join(&db).exists());
        assert!(
            storage
                .root
                .epoch_storage()
                .await
                .unwrap()
                .database_epoch_snapshot(&db)
                .await
                .unwrap()
                .is_none()
        );
    }
    let mut connection = storage
        .root
        .epoch_storage()
        .await
        .unwrap()
        .hold_connection_for_test()
        .await;
    let (rows,): (i64,) = connection
        .fetch_one_as(sqlx::query_as(
            "SELECT COUNT(*) FROM index_storage WHERE namespace = 'database-epochs';",
        ))
        .await
        .unwrap();
    assert_eq!(rows, 0);
    connection.rollback().await.unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn deleting_last_snapshot_epoch_cleans_residual_files_before_metadata() {
    for fail_unlink in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("last-epoch");
        storage
            .append(
                "test",
                "seed",
                "entry",
                ns.clone(),
                "key",
                1,
                vec![53],
                None,
            )
            .await
            .unwrap();
        let db = storage.namespace_to_db(&ns).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        storage.root.cache.remove(&db).await;
        file.wait_retired().await;
        let metadata = storage.root.epoch_storage().await.unwrap();
        metadata
            .save_database_epoch_snapshot(
                &db,
                vec![(
                    SqliteIndexedStorage::namespace(ns.clone()),
                    "key".into(),
                    17,
                )],
            )
            .await
            .unwrap();
        if fail_unlink {
            std::fs::create_dir(storage.root.root_dir.join(format!("{db}-shm"))).unwrap();
        }
        let result = storage
            .delete_with_epoch("test", "delete", ns, "key", Some(ShardEpoch(17)))
            .await;
        if fail_unlink {
            assert!(result.is_err());
            assert!(file.path.exists());
            assert!(file.is_poisoned());
            assert_eq!(
                metadata.database_epoch_snapshot(&db).await.unwrap(),
                Some(Vec::new())
            );
        } else {
            result.unwrap();
            assert!(!file.path.exists());
            assert!(!file.is_poisoned());
            assert!(
                metadata
                    .database_epoch_snapshot(&db)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[test]
#[test_r::timeout("30s")]
async fn trailing_read_cleans_empty_snapshot_after_interrupted_unlink_handoff() {
    for residual_file in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("empty-handoff");
        let db = storage.namespace_to_db(&ns).await;
        if residual_file {
            storage
                .append(
                    "test",
                    "seed",
                    "entry",
                    ns.clone(),
                    "key",
                    1,
                    vec![97],
                    None,
                )
                .await
                .unwrap();
            storage.root.cache.remove(&db).await;
            FileLifetime::get(storage.root.root_dir.join(&db))
                .wait_retired()
                .await;
        }
        let metadata = storage.root.epoch_storage().await.unwrap();
        metadata
            .save_database_epoch_snapshot(&db, Vec::new())
            .await
            .unwrap();
        assert_eq!(storage.length("test", "read", ns, "key").await.unwrap(), 0);
        assert!(!storage.root.root_dir.join(&db).exists());
        assert!(
            metadata
                .database_epoch_snapshot(&db)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[test]
async fn scan_counts_empty_files_and_reads_each_file_whole() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    warm(&storage, "durable-oplog-000.db").await;
    let ns = oplog_namespace("scan");
    for key in ["a", "b", "c"] {
        storage
            .append("test", "append", "entry", ns.clone(), key, 1, vec![1], None)
            .await
            .unwrap();
    }
    let meta = IndexedStorageMetaNamespace::Oplog {
        agent_mode: AgentMode::Durable,
    };
    let listing = storage.namespace_db_files(&meta, true).await.unwrap();
    assert!(Arc::ptr_eq(
        &listing,
        &storage.namespace_db_files(&meta, false).await.unwrap()
    ));
    let (resume, keys) = storage
        .scan_stable("test", "scan", meta.clone(), None, None, 1)
        .await
        .unwrap();
    assert!(keys.is_empty());
    assert!(resume.is_some());
    let (_, keys) = storage
        .scan_stable("test", "scan", meta, None, resume, 1)
        .await
        .unwrap();
    assert_eq!(keys, vec!["a", "b", "c"]);
}

#[test]
#[test_r::timeout("30s")]
async fn reclamation_drains_pools_and_preserves_all_epochs_for_recreation() {
    use golem_service_base::db::LabelledPoolTransaction;
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 2, false);
    let ns = oplog_namespace("drain");
    let IndexedStorageNamespace::OpLog {
        agent_id,
        agent_mode,
    } = ns.clone()
    else {
        unreachable!()
    };
    let stage = IndexedStorageNamespace::StagedOpLog {
        agent_id,
        agent_mode,
    };
    storage
        .set_key_epoch("test", "epoch", ns.clone(), "visible", ShardEpoch(31))
        .await
        .unwrap();
    storage
        .set_key_epoch("test", "epoch", stage.clone(), "unrelated", ShardEpoch(73))
        .await
        .unwrap();
    storage
        .append(
            "test",
            "append",
            "entry",
            ns.clone(),
            "visible",
            1,
            vec![9],
            Some(ShardEpoch(31)),
        )
        .await
        .unwrap();
    let db = storage.namespace_to_db(&ns).await;
    let path = storage.root.root_dir.join(&db);
    let file = FileLifetime::get(path.clone());
    let cached = storage.root.cache.get(&db).await.unwrap();
    let connection = cached.hold_connection_for_test().await;
    drop(cached);
    let worker = storage.clone();
    let deleting_ns = ns.clone();
    let deletion = tokio::spawn(async move {
        worker
            .delete("test", "delete", deleting_ns, "visible")
            .await
    });
    let metadata = storage.root.epoch_storage().await.unwrap();
    let snapshot = loop {
        if let Some(epochs) = metadata.database_epoch_snapshot(&db).await.unwrap() {
            break epochs;
        }
        tokio::task::yield_now().await;
    };
    assert_eq!(snapshot.len(), 2);
    assert!(storage.root.cache.get(&db).await.is_none());
    assert!(path.exists());
    assert_eq!(file.registrations.load(Ordering::Acquire), 1);
    assert!(!deletion.is_finished());
    connection.rollback().await.unwrap();
    deletion.await.unwrap().unwrap();
    assert_eq!(file.registrations.load(Ordering::Acquire), 0);
    assert!(!path.exists());
    assert!(
        !storage
            .exists("test", "exists", ns.clone(), "visible")
            .await
            .unwrap()
    );
    assert!(!path.exists());
    assert!(
        storage
            .delete_empty_with_epoch(
                "test",
                "empty",
                stage.clone(),
                "unrelated",
                Some(ShardEpoch(73))
            )
            .await
            .unwrap()
    );
    storage
        .drop_prefix(
            "test",
            "prefix",
            ns.clone(),
            "visible",
            999,
            Some(ShardEpoch(31)),
        )
        .await
        .unwrap();
    assert!(matches!(
        storage
            .drop_prefix(
                "test",
                "prefix",
                ns.clone(),
                "missing",
                1,
                Some(ShardEpoch(31))
            )
            .await,
        Err(IndexedStorageError::Fenced { actual: None, .. })
    ));
    assert!(matches!(
        storage
            .set_key_epoch("test", "epoch", ns.clone(), "visible", ShardEpoch(30))
            .await,
        Err(IndexedStorageError::Fenced {
            actual: Some(ShardEpoch(31)),
            ..
        })
    ));
    storage
        .set_key_epoch("test", "epoch", ns.clone(), "visible", ShardEpoch(41))
        .await
        .unwrap();
    assert!(!path.exists());
    assert!(matches!(
        storage
            .append(
                "test",
                "stale",
                "entry",
                ns.clone(),
                "visible",
                1,
                vec![3],
                Some(ShardEpoch(31))
            )
            .await,
        Err(IndexedStorageError::Fenced {
            actual: Some(ShardEpoch(41)),
            ..
        })
    ));
    storage
        .append(
            "test",
            "append",
            "entry",
            ns.clone(),
            "visible",
            1,
            vec![4],
            Some(ShardEpoch(41)),
        )
        .await
        .unwrap();
    assert!(matches!(
        storage
            .append(
                "test",
                "stale",
                "entry",
                stage.clone(),
                "unrelated",
                1,
                vec![8],
                Some(ShardEpoch(41))
            )
            .await,
        Err(IndexedStorageError::Fenced {
            actual: Some(ShardEpoch(73)),
            ..
        })
    ));
    assert!(
        metadata
            .database_epoch_snapshot(&db)
            .await
            .unwrap()
            .is_none()
    );
    storage
        .delete_with_epoch("test", "delete", ns, "visible", Some(ShardEpoch(41)))
        .await
        .unwrap();
    assert!(!path.exists());
    let epochs = metadata
        .database_epoch_snapshot(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        epochs,
        vec![(
            SqliteIndexedStorage::namespace(stage),
            "unrelated".into(),
            73
        )]
    );
    assert!(!file.is_poisoned());
}

#[test]
async fn staged_rows_other_keys_and_presence_markers_prevent_reclamation() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("presence");
    let IndexedStorageNamespace::OpLog {
        agent_id,
        agent_mode,
    } = ns.clone()
    else {
        unreachable!()
    };
    let stage = IndexedStorageNamespace::StagedOpLog {
        agent_id,
        agent_mode,
    };
    for (namespace, key, bytes) in [
        (ns.clone(), "visible", vec![1]),
        (stage.clone(), "stage", vec![7]),
        (ns.clone(), "other", vec![11]),
    ] {
        storage
            .append("test", "append", "entry", namespace, key, 1, bytes, None)
            .await
            .unwrap();
    }
    let db = storage.namespace_to_db(&ns).await;
    let path = storage.root.root_dir.join(&db);
    storage
        .delete("test", "delete", ns.clone(), "visible")
        .await
        .unwrap();
    assert!(path.exists());
    storage
        .delete("test", "delete", stage, "stage")
        .await
        .unwrap();
    assert!(path.exists());
    storage
        .drop_prefix("test", "prefix", ns.clone(), "other", 1, None)
        .await
        .unwrap();
    assert!(path.exists());
    assert!(
        storage
            .delete_empty_with_epoch("test", "empty", ns.clone(), "other", None)
            .await
            .unwrap()
    );
    assert!(path.exists());
    // Removing a marker is not a reclamation trigger; a successful destructive operation is.
    storage
        .delete("test", "delete", ns, "nonexistent")
        .await
        .unwrap();
    assert!(!path.exists());
}

#[test]
async fn compressed_prefix_reclaims_and_retains_writer_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let IndexedStorageNamespace::OpLog {
        agent_id,
        agent_mode,
    } = oplog_namespace("compressed")
    else {
        unreachable!()
    };
    let ns = IndexedStorageNamespace::CompressedOpLog {
        agent_id,
        agent_mode,
        level: 3,
    };
    storage
        .set_key_epoch("test", "epoch", ns.clone(), "key", ShardEpoch(17))
        .await
        .unwrap();
    storage
        .append(
            "test",
            "append",
            "entry",
            ns.clone(),
            "key",
            5,
            vec![99],
            Some(ShardEpoch(17)),
        )
        .await
        .unwrap();
    let db = storage.namespace_to_db(&ns).await;
    storage
        .drop_prefix("test", "prefix", ns.clone(), "key", 5, Some(ShardEpoch(17)))
        .await
        .unwrap();
    assert!(!storage.root.root_dir.join(&db).exists());
    assert_eq!(
        storage
            .root
            .epoch_storage()
            .await
            .unwrap()
            .database_epoch_snapshot(&db)
            .await
            .unwrap(),
        Some(vec![(
            SqliteIndexedStorage::namespace(ns.clone()),
            "key".into(),
            17
        )])
    );
    assert_eq!(
        storage
            .length("test", "length", ns.clone(), "key")
            .await
            .unwrap(),
        0
    );
    assert!(
        storage
            .delete_empty_with_epoch("test", "empty", ns.clone(), "key", Some(ShardEpoch(17)))
            .await
            .unwrap()
    );
    assert!(!storage.root.root_dir.join(&db).exists());
    storage
        .append(
            "test",
            "append",
            "entry",
            ns.clone(),
            "key",
            6,
            vec![101],
            Some(ShardEpoch(17)),
        )
        .await
        .unwrap();
    assert_eq!(
        storage
            .read("test", "read", "entry", ns, "key", 1, 9)
            .await
            .unwrap(),
        vec![(6, vec![101])]
    );
}

#[test]
async fn snapshots_are_authoritative_over_leftovers_including_empty_and_partial_restore() {
    for authoritative in [
        Vec::new(),
        vec![
            ("durable-worker-oplog".into(), "writer".into(), 19),
            ("durable-worker-staged-oplog".into(), "other".into(), 83),
        ],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("interrupted");
        let db = storage.namespace_to_db(&ns).await;
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "leftover",
                1,
                vec![6],
                None,
            )
            .await
            .unwrap();
        let cached = storage.root.cache.get(&db).await.unwrap();
        // Simulate a prior process stopping with only part of the authoritative epochs restored.
        cached
            .restore_database_epochs(authoritative.iter().take(1).cloned().collect())
            .await
            .unwrap();
        drop(cached);
        storage.root.cache.remove(&db).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        file.wait_retired().await;
        let metadata = storage.root.epoch_storage().await.unwrap();
        metadata
            .save_database_epoch_snapshot(&db, authoritative.clone())
            .await
            .unwrap();
        assert!(
            !storage
                .exists("test", "exists", ns.clone(), "leftover")
                .await
                .unwrap()
        );
        assert_eq!(file.path.exists(), !authoritative.is_empty());
        assert!(storage.root.cache.get(&db).await.is_none());
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "new",
                1,
                vec![12],
                None,
            )
            .await
            .unwrap();
        let cached = storage.root.cache.get(&db).await.unwrap();
        assert!(
            !cached
                .exists("test", "exists", ns.clone(), "leftover")
                .await
                .unwrap()
        );
        cached.delete("test", "delete", ns, "new").await.unwrap();
        let mut restored = cached.empty_database_epochs().await.unwrap().unwrap();
        let mut expected = authoritative;
        restored.sort();
        expected.sort();
        assert_eq!(restored, expected);
        assert!(
            metadata
                .database_epoch_snapshot(&db)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[test]
async fn poisoned_successful_delete_leaves_empty_file_and_failed_mutation_never_reclaims() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("poisoned-delete");
    storage
        .set_key_epoch("test", "epoch", ns.clone(), "key", ShardEpoch(29))
        .await
        .unwrap();
    let db = storage.namespace_to_db(&ns).await;
    let file = FileLifetime::get(storage.root.root_dir.join(&db));
    assert!(matches!(
        storage
            .delete_with_epoch("test", "stale", ns.clone(), "key", Some(ShardEpoch(28)))
            .await,
        Err(IndexedStorageError::Fenced { .. })
    ));
    assert!(file.path.exists());
    assert!(!file.is_poisoned());
    assert!(
        storage
            .root
            .epoch_storage()
            .await
            .unwrap()
            .database_epoch_snapshot(&db)
            .await
            .unwrap()
            .is_none()
    );
    file.poison();
    storage
        .delete_with_epoch("test", "delete", ns, "key", Some(ShardEpoch(29)))
        .await
        .unwrap();
    assert!(file.path.exists());
    assert!(
        storage
            .root
            .cache
            .get(&db)
            .await
            .unwrap()
            .empty_database_epochs()
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
async fn absent_destructive_access_never_materializes_and_validates_snapshot_epochs() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("absent");
    let db = storage.namespace_to_db(&ns).await;
    let path = storage.root.root_dir.join(&db);
    storage
        .delete("test", "delete", ns.clone(), "key")
        .await
        .unwrap();
    storage
        .delete_with_epoch("test", "delete", ns.clone(), "key", Some(ShardEpoch(53)))
        .await
        .unwrap();
    assert!(
        storage
            .delete_empty_with_epoch("test", "empty", ns.clone(), "key", None)
            .await
            .unwrap()
    );
    assert!(matches!(
        storage
            .delete_empty_with_epoch("test", "empty", ns.clone(), "key", Some(ShardEpoch(53)))
            .await,
        Err(IndexedStorageError::Fenced { actual: None, .. })
    ));
    assert!(
        storage
            .move_if_absent(
                "test",
                "move",
                ns.clone(),
                "source",
                ns.clone(),
                "target",
                1
            )
            .await
            .is_err()
    );
    assert!(!path.exists());
    let metadata = storage.root.epoch_storage().await.unwrap();
    metadata
        .save_database_epoch_snapshot(
            &db,
            vec![
                (
                    SqliteIndexedStorage::namespace(ns.clone()),
                    "key".into(),
                    53,
                ),
                (
                    SqliteIndexedStorage::namespace(ns.clone()),
                    "other".into(),
                    97,
                ),
            ],
        )
        .await
        .unwrap();
    assert!(matches!(
        storage
            .delete_with_epoch("test", "delete", ns.clone(), "key", Some(ShardEpoch(52)))
            .await,
        Err(IndexedStorageError::Fenced {
            actual: Some(ShardEpoch(53)),
            ..
        })
    ));
    assert!(
        storage
            .delete_empty_with_epoch("test", "empty", ns.clone(), "key", Some(ShardEpoch(53)))
            .await
            .unwrap()
    );
    storage
        .delete("test", "delete", ns.clone(), "key")
        .await
        .unwrap();
    assert_eq!(
        metadata
            .database_epoch_snapshot(&db)
            .await
            .unwrap()
            .unwrap()
            .len(),
        2
    );
    storage
        .delete_with_epoch("test", "delete", ns.clone(), "key", Some(ShardEpoch(53)))
        .await
        .unwrap();
    assert_eq!(
        metadata.database_epoch_snapshot(&db).await.unwrap(),
        Some(vec![(
            SqliteIndexedStorage::namespace(ns.clone()),
            "other".into(),
            97
        )])
    );
    assert!(
        storage
            .set_key_epoch("test", "epoch", ns, "overflow", ShardEpoch(u64::MAX))
            .await
            .is_err()
    );
    assert!(!path.exists());
}

#[test]
#[test_r::timeout("30s")]
async fn committed_snapshot_save_error_evicts_cache_and_keeps_snapshot_authority() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("committed-save");
    storage
        .append(
            "test",
            "append",
            "entry",
            ns.clone(),
            "key",
            1,
            vec![15],
            None,
        )
        .await
        .unwrap();
    let db = storage.namespace_to_db(&ns).await;
    let cached = storage.root.cache.get(&db).await.unwrap();
    storage.root.snapshot_failure.store(1, Ordering::Release);
    assert!(
        storage
            .delete("test", "delete", ns.clone(), "key")
            .await
            .is_err()
    );
    assert!(storage.root.cache.get(&db).await.is_none());
    let file = FileLifetime::get(storage.root.root_dir.join(&db));
    assert!(file.path.exists());
    assert!(file.is_poisoned());
    assert_eq!(
        storage
            .root
            .epoch_storage()
            .await
            .unwrap()
            .database_epoch_snapshot(&db)
            .await
            .unwrap(),
        Some(Vec::new())
    );
    // Make the retained physical generation observably disagree with snapshot authority.
    cached
        .append(
            "test",
            "leftover",
            "entry",
            ns.clone(),
            "key",
            2,
            vec![91],
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        cached
            .read("test", "read", "entry", ns.clone(), "key", 1, 9)
            .await
            .unwrap(),
        vec![(2, vec![91])]
    );
    drop(cached);
    file.wait_retired().await;
    assert!(
        storage
            .read("test", "read", "entry", ns, "key", 1, 9)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(file.path.exists());
    assert!(storage.root.cache.get(&db).await.is_none());
}

#[test]
#[test_r::timeout("30s")]
async fn restored_snapshot_delete_failures_do_not_publish_or_append() {
    for boundary in [2, 3] {
        let dir = tempfile::tempdir().unwrap();
        let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
        let ns = oplog_namespace("snapshot-delete");
        let db = storage.namespace_to_db(&ns).await;
        let file = FileLifetime::get(storage.root.root_dir.join(&db));
        let records = vec![(
            SqliteIndexedStorage::namespace(ns.clone()),
            "key".into(),
            67,
        )];
        let metadata = storage.root.epoch_storage().await.unwrap();
        metadata
            .save_database_epoch_snapshot(&db, records.clone())
            .await
            .unwrap();
        storage
            .root
            .snapshot_failure
            .store(boundary, Ordering::Release);
        assert!(
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    ns.clone(),
                    "key",
                    1,
                    vec![3],
                    Some(ShardEpoch(67))
                )
                .await
                .is_err()
        );
        assert!(storage.root.cache.get(&db).await.is_none());
        file.wait_retired().await;
        assert!(file.path.exists());
        assert!(file.is_poisoned());
        assert_eq!(
            metadata.database_epoch_snapshot(&db).await.unwrap(),
            if boundary == 2 {
                Some(records.clone())
            } else {
                None
            }
        );
        assert!(
            storage
                .read("test", "read", "entry", ns.clone(), "key", 1, 9)
                .await
                .unwrap()
                .is_empty()
        );
        if boundary == 2 {
            assert!(storage.root.cache.get(&db).await.is_none());
            assert!(
                storage
                    .append(
                        "test",
                        "retry",
                        "entry",
                        ns,
                        "key",
                        2,
                        vec![4],
                        Some(ShardEpoch(67))
                    )
                    .await
                    .is_err()
            );
        } else {
            let cached = storage.root.cache.get(&db).await.unwrap();
            assert_eq!(cached.empty_database_epochs().await.unwrap(), Some(records));
            drop(cached);
            assert!(matches!(
                storage
                    .append(
                        "test",
                        "stale",
                        "entry",
                        ns.clone(),
                        "key",
                        2,
                        vec![4],
                        Some(ShardEpoch(66))
                    )
                    .await,
                Err(IndexedStorageError::Fenced {
                    actual: Some(ShardEpoch(67)),
                    ..
                })
            ));
            storage
                .append(
                    "test",
                    "retry",
                    "entry",
                    ns.clone(),
                    "key",
                    2,
                    vec![5],
                    Some(ShardEpoch(67)),
                )
                .await
                .unwrap();
            assert_eq!(
                storage
                    .read("test", "read", "entry", ns, "key", 1, 9)
                    .await
                    .unwrap(),
                vec![(2, vec![5])]
            );
        }
    }
}

#[test]
async fn metadata_save_failure_evicts_before_poison_and_never_unlinks() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("metadata-failure");
    storage
        .append(
            "test",
            "append",
            "entry",
            ns.clone(),
            "key",
            1,
            vec![15],
            None,
        )
        .await
        .unwrap();
    let db = storage.namespace_to_db(&ns).await;
    storage.root.epoch_storage().await.unwrap().close().await;
    assert!(storage.delete("test", "delete", ns, "key").await.is_err());
    assert!(storage.root.cache.get(&db).await.is_none());
    let file = FileLifetime::get(storage.root.root_dir.join(&db));
    file.wait_retired().await;
    assert!(file.path.exists());
    assert!(file.is_poisoned());
}

#[test]
async fn restoration_failure_keeps_snapshot_and_registered_unpublished_pools() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let ns = oplog_namespace("restore-failure");
    let db = storage.namespace_to_db(&ns).await;
    let file = FileLifetime::get(storage.root.root_dir.join(&db));
    let record = (
        SqliteIndexedStorage::namespace(ns.clone()),
        "key".into(),
        67,
    );
    let metadata = storage.root.epoch_storage().await.unwrap();
    metadata
        .save_database_epoch_snapshot(&db, vec![record.clone(), record.clone()])
        .await
        .unwrap();
    assert!(
        storage
            .append(
                "test",
                "append",
                "entry",
                ns.clone(),
                "key",
                1,
                vec![3],
                None
            )
            .await
            .is_err()
    );
    assert!(storage.root.cache.get(&db).await.is_none());
    file.wait_retired().await;
    assert!(file.is_poisoned());
    assert_eq!(
        metadata.database_epoch_snapshot(&db).await.unwrap(),
        Some(vec![record.clone(), record])
    );
    assert!(
        !storage
            .exists("test", "exists", ns.clone(), "key")
            .await
            .unwrap()
    );
    assert!(
        storage
            .append("test", "append", "entry", ns, "key", 1, vec![4], None)
            .await
            .is_err()
    );
    assert!(file.path.exists());
}

#[test]
async fn deletion_dirty_listing_is_stable_for_resume_and_refreshes_next_walk() {
    let dir = tempfile::tempdir().unwrap();
    let storage = MultiSqliteIndexedStorage::new(dir.path(), 1, false);
    let first = oplog_namespace("listing-first");
    let second = oplog_namespace("listing-second");
    for (ns, key) in [(first.clone(), "alpha"), (second.clone(), "zeta")] {
        storage
            .append("test", "append", "entry", ns, key, 1, vec![1], None)
            .await
            .unwrap();
    }
    let meta = IndexedStorageMetaNamespace::Oplog {
        agent_mode: AgentMode::Durable,
    };
    let listing = storage.namespace_db_files(&meta, true).await.unwrap();
    let (resume, keys) = storage
        .scan_stable("test", "scan", meta.clone(), None, None, 1)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert!(resume.is_some());
    storage
        .delete("test", "delete", first, "alpha")
        .await
        .unwrap();
    storage
        .delete("test", "delete", second, "zeta")
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &listing,
        &storage.namespace_db_files(&meta, false).await.unwrap()
    ));
    let (last, keys) = storage
        .scan_stable("test", "scan", meta.clone(), None, resume, 1)
        .await
        .unwrap();
    assert!(keys.is_empty());
    assert!(last.is_some()); // A stale filename still consumes the single-file page.
    for db in listing.iter() {
        assert!(!storage.root.root_dir.join(db).exists());
    }
    let fresh = storage.namespace_db_files(&meta, true).await.unwrap();
    assert!(fresh.is_empty());
    assert!(!Arc::ptr_eq(&listing, &fresh));
    let (resume, keys) = storage
        .scan_stable("test", "scan", meta, None, None, 1)
        .await
        .unwrap();
    assert!(resume.is_none());
    assert!(keys.is_empty());
}
