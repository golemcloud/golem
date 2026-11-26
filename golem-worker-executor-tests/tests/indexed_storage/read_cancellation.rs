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
use futures::{StreamExt, TryStreamExt};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::{OplogIndex, RetryConfig};
use golem_common::serialization::{DecodeError, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_worker_executor::services::oplog::{OplogService, PrimaryOplogService};
use pretty_assertions::assert_eq;
use std::time::Duration;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);

async fn cancel_blocked_read(
    pool: &sqlx::PgPool,
    blocker: i32,
    previous: Option<(i32, String)>,
) -> (i32, String) {
    let blocked = futures::stream::unfold((), |_| async {
        let blocked: Option<(i32, String)> = sqlx::query_as(
            "SELECT pid, query_start::text FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)) AND state = 'active'",
        ).bind(blocker).fetch_optional(pool).await.unwrap();
        tokio::task::yield_now().await;
        Some((blocked, ()))
    }).filter_map(|blocked| futures::future::ready(blocked.filter(|blocked| Some(blocked) != previous.as_ref())));
    tokio::pin!(blocked);
    let blocked = tokio::time::timeout(Duration::from_secs(10), blocked.next())
        .await
        .expect("no new blocked indexed read")
        .unwrap();
    let (cancelled,): (bool,) = sqlx::query_as("SELECT pg_cancel_backend($1)")
        .bind(blocked.0)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(cancelled);
    blocked
}

/// A select of the indexed storage, one for each read operation of the trait.
#[derive(Clone, Copy, Debug)]
enum Select {
    Exists,
    ScanStable,
    Length,
    Read,
    First,
    Last,
    LastId,
    Closest,
}

impl Select {
    const ALL: [Self; 8] = [
        Self::Exists,
        Self::ScanStable,
        Self::Length,
        Self::Read,
        Self::First,
        Self::Last,
        Self::LastId,
        Self::Closest,
    ];
}

async fn select(
    storage: &PostgresIndexedStorage,
    namespace: IndexedStorageNamespace,
    operation: Select,
) -> Result<(), IndexedStorageError> {
    match operation {
        Select::Exists => storage
            .exists("test", "cancel", namespace, KEY1)
            .await
            .map(|_| ()),
        Select::ScanStable => storage
            .scan_stable(
                "test",
                "cancel",
                IndexedStorageMetaNamespace::Oplog {
                    agent_mode: AgentMode::Durable,
                },
                None,
                None,
                10,
            )
            .await
            .map(|_| ()),
        Select::Length => storage
            .length("test", "cancel", namespace, KEY1)
            .await
            .map(|_| ()),
        Select::Read => storage
            .read("test", "cancel", "entry", namespace, KEY1, 1, 1)
            .await
            .map(|_| ()),
        Select::First => storage
            .first("test", "cancel", "entry", namespace, KEY1)
            .await
            .map(|_| ()),
        Select::Last => storage
            .last("test", "cancel", "entry", namespace, KEY1)
            .await
            .map(|_| ()),
        Select::LastId => storage
            .last_id("test", "cancel", "entry", namespace, KEY1)
            .await
            .map(|_| ()),
        Select::Closest => storage
            .closest("test", "cancel", "entry", namespace, KEY1, 1)
            .await
            .map(|_| ()),
    }
}

#[test]
#[test_r::timeout("90s")]
async fn cancelled_postgres_selects_are_transient_and_primary_reads_recover_or_exhaust(
    _deps: &WorkerExecutorTestDependencies,
) -> anyhow::Result<()> {
    let wrapper = PostgresIndexedStorageWrapper {
        postgres: DockerPostgresRdb::new(&Uuid::new_v4().to_string(), false).await,
    };
    let config = wrapper.fresh_database().await;
    let storage = Arc::new(
        PostgresIndexedStorage::configured(&config)
            .await
            .map_err(anyhow::Error::msg)?,
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect_with(config.postgres.connect_options())
        .await?;
    let id = golem_common::model::OwnedAgentId::from_storage_key(KEY1).unwrap();
    let namespace = IndexedStorageNamespace::OpLog {
        owned_agent_id: id.clone(),
        agent_mode: AgentMode::Durable,
    };
    let entry = OplogEntry::suspend().rounded();
    let bytes = serialize(&entry).map_err(anyhow::Error::msg)?;
    storage
        .append(
            "test",
            "seed",
            "entry",
            namespace.clone(),
            KEY1,
            1,
            bytes.clone(),
            None,
        )
        .await?;

    futures::stream::iter(Select::ALL)
        .then(|operation| {
            let storage = storage.clone();
            let namespace = namespace.clone();
            let pool = &pool;
            async move {
                let mut lock = pool.begin().await.unwrap();
                let (blocker,): (i32,) = sqlx::query_as("SELECT pg_backend_pid()")
                    .fetch_one(&mut *lock)
                    .await
                    .unwrap();
                sqlx::query("LOCK TABLE index_storage IN ACCESS EXCLUSIVE MODE")
                    .execute(&mut *lock)
                    .await
                    .unwrap();
                let read = tokio::spawn({
                    let storage = storage.clone();
                    let namespace = namespace.clone();
                    async move { select(&storage, namespace, operation).await }
                });
                cancel_blocked_read(pool, blocker, None).await;
                let error = read.await.unwrap().unwrap_err();
                assert!(
                    matches!(error, IndexedStorageError::Transient(_)),
                    "operation {operation:?}: {error:?}"
                );
                lock.commit().await.unwrap();
                select(&storage, namespace, operation).await.unwrap();
            }
        })
        .collect::<Vec<_>>()
        .await;

    let primary = Arc::new(
        PrimaryOplogService::new(
            storage.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            100,
            100,
            1024,
            32,
            RetryConfig {
                max_attempts: 2,
                min_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .await,
    );
    // A single real cancellation must retry the same content read after the lock is released.
    let mut lock = pool.begin().await?;
    let (blocker,): (i32,) = sqlx::query_as("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    sqlx::query("LOCK TABLE index_storage IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await?;
    let read = tokio::spawn({
        let primary = primary.clone();
        let id = id.clone();
        async move {
            primary
                .read_exact(&id, AgentMode::Durable, OplogIndex::INITIAL, 1)
                .await
        }
    });
    cancel_blocked_read(&pool, blocker, None).await;
    lock.commit().await?;
    assert_eq!(read.await??.get(&OplogIndex::INITIAL), Some(&entry));

    let mut lock = pool.begin().await?;
    let (blocker,): (i32,) = sqlx::query_as("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    sqlx::query("LOCK TABLE index_storage IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await?;
    let read = tokio::spawn({
        let primary = primary.clone();
        let id = id.clone();
        async move {
            primary
                .read_exact(&id, AgentMode::Durable, OplogIndex::INITIAL, 1)
                .await
        }
    });
    let first = cancel_blocked_read(&pool, blocker, None).await;
    cancel_blocked_read(&pool, blocker, Some(first)).await;
    let error: WorkerExecutorError = read.await?.unwrap_err().into();
    assert!(
        matches!(error, WorkerExecutorError::RecoveryRequired { .. }),
        "{error:?}"
    );
    lock.commit().await?;

    futures::stream::iter([
        (vec![255], DecodeError::UnknownVersion(255)),
        (vec![], DecodeError::MissingVersion),
    ])
    .map(Ok::<_, anyhow::Error>)
    .try_for_each(|(corrupt, decode_error)| {
        let pool = &pool;
        let primary = primary.clone();
        let id = &id;
        let storage = storage.clone();
        let namespace = namespace.clone();
        async move {
            sqlx::query("UPDATE index_storage SET value = $1")
                .bind(&corrupt)
                .execute(pool)
                .await?;
            let error: WorkerExecutorError = primary
                .read_exact(id, AgentMode::Durable, OplogIndex::INITIAL, 1)
                .await
                .unwrap_err()
                .into();
            assert!(
                matches!(error, WorkerExecutorError::OplogFailed { .. }),
                "{error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&format!("failed to decode oplog entry 1: {decode_error}")),
                "{message}"
            );
            assert_eq!(
                storage
                    .read("test", "unchanged", "entry", namespace, KEY1, 1, 1)
                    .await?,
                vec![(1, corrupt)]
            );
            Ok(())
        }
    })
    .await?;
    sqlx::query("DROP TABLE index_storage")
        .execute(&pool)
        .await?;
    assert!(matches!(
        select(&storage, namespace, Select::Read).await,
        Err(IndexedStorageError::Other(_))
    ));
    let error: WorkerExecutorError = primary
        .read_exact(&id, AgentMode::Durable, OplogIndex::INITIAL, 1)
        .await
        .unwrap_err()
        .into();
    assert!(
        matches!(error, WorkerExecutorError::OplogFailed { .. }),
        "{error:?}"
    );
    pool.close().await;
    Ok(())
}
