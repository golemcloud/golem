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

use super::sql_session_store::wait_for_row_count;
use super::{Tracing, unique_postgres_config};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_common::config::DbSqliteConfig;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_service::custom_api::oidc::session_store::{
    migrated_postgres_pool, migrated_sqlite_pool,
};
use golem_worker_service::mcp::session::store::{
    McpSessionId, McpSessionRecord, McpSessionStore, PostgresMcpSessionStore, SqliteMcpSessionStore,
};
use sqlx::Row;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::{define_matrix_dimension, inherit_test_dep, test, timeout};
use tokio::time::{Instant, sleep, sleep_until};

inherit_test_dep!(Tracing);
inherit_test_dep!(DockerPostgresRdb);
inherit_test_dep!(#[tagged_as("mcp_redis")] Arc<dyn McpSessionStore>);
inherit_test_dep!(#[tagged_as("mcp_sqlite")] Arc<dyn McpSessionStore>);
inherit_test_dep!(#[tagged_as("mcp_postgres")] Arc<dyn McpSessionStore>);

define_matrix_dimension!(mcp_store: Arc<dyn McpSessionStore> -> "mcp_redis", "mcp_sqlite", "mcp_postgres");

const LONG: Duration = Duration::from_secs(60);

fn record(environment_id: EnvironmentId, revision: u64) -> McpSessionRecord {
    McpSessionRecord {
        domain: Domain("mcp.example.com".to_string()),
        environment_id,
        deployment_revision: DeploymentRevision::new(revision).unwrap(),
    }
}

fn new_id() -> McpSessionId {
    McpSessionId::generate().unwrap()
}

#[test]
#[timeout("30s")]
async fn mcp_session_create_get_delete(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let id = new_id();
    let first = record(EnvironmentId::new(), 1);

    assert!(store.create(id, &first, LONG).await?);
    assert_eq!(store.get(id, None).await?, Some(first.clone()));
    assert_eq!(store.get(id, Some(LONG)).await?, Some(first));

    store.delete(id).await?;
    assert_eq!(store.get(id, None).await?, None);
    store.delete(id).await?;
    assert_eq!(store.get(new_id(), None).await?, None);
    Ok(())
}

#[test]
#[timeout("30s")]
async fn mcp_session_create_refuses_a_taken_id(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let id = new_id();
    let environment_id = EnvironmentId::new();

    assert!(store.create(id, &record(environment_id, 1), LONG).await?);
    assert!(!store.create(id, &record(environment_id, 2), LONG).await?);
    assert_eq!(store.get(id, None).await?, Some(record(environment_id, 1)));
    Ok(())
}

#[test]
#[timeout("30s")]
async fn mcp_session_expires_and_get_with_extend_keeps_it_live(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let extended = new_id();
    let untouched = new_id();
    let session = record(EnvironmentId::new(), 1);

    assert!(
        store
            .create(extended, &session, Duration::from_secs(1))
            .await?
    );
    assert!(
        store
            .create(untouched, &session, Duration::from_secs(1))
            .await?
    );
    assert_eq!(
        store.get(extended, Some(Duration::from_secs(3))).await?,
        Some(session.clone())
    );

    sleep(Duration::from_millis(1500)).await;
    assert_eq!(store.get(untouched, None).await?, None);
    assert_eq!(store.get(extended, None).await?, Some(session));
    Ok(())
}

#[test]
#[timeout("30s")]
async fn mcp_session_get_without_extend_keeps_the_expiry(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let id = new_id();
    let session = record(EnvironmentId::new(), 1);

    assert!(store.create(id, &session, Duration::from_secs(1)).await?);
    assert_eq!(store.get(id, None).await?, Some(session));

    sleep(Duration::from_millis(1500)).await;
    assert_eq!(store.get(id, None).await?, None);
    Ok(())
}

#[test]
#[timeout("30s")]
async fn mcp_session_replace_writes_the_pin_and_keeps_the_expiry(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let id = new_id();
    let environment_id = EnvironmentId::new();

    assert!(
        store
            .create(id, &record(environment_id, 1), Duration::from_secs(1))
            .await?
    );
    assert!(store.replace(id, &record(environment_id, 2)).await?);
    assert_eq!(store.get(id, None).await?, Some(record(environment_id, 2)));

    sleep(Duration::from_millis(1500)).await;
    assert_eq!(store.get(id, None).await?, None);
    Ok(())
}

#[test]
#[timeout("30s")]
async fn mcp_session_replace_does_not_revive_a_record(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let environment_id = EnvironmentId::new();

    let expired = new_id();
    assert!(
        store
            .create(
                expired,
                &record(environment_id, 1),
                Duration::from_millis(200)
            )
            .await?
    );
    sleep(Duration::from_millis(400)).await;
    assert!(!store.replace(expired, &record(environment_id, 2)).await?);
    assert_eq!(store.get(expired, None).await?, None);

    let deleted = new_id();
    assert!(
        store
            .create(deleted, &record(environment_id, 1), LONG)
            .await?
    );
    store.delete(deleted).await?;
    assert!(!store.replace(deleted, &record(environment_id, 2)).await?);
    assert_eq!(store.get(deleted, None).await?, None);
    Ok(())
}

/// Idle 4 s: a session created at t0 and read at t0 + 0.6 s is still live at t0 + 4.3 s.
#[test]
#[timeout("30s")]
async fn mcp_session_never_expires_before_the_idle_timeout(
    #[dimension(mcp_store)] store: &Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let idle = Duration::from_secs(4);
    let id = new_id();
    let session = record(EnvironmentId::new(), 1);

    let t0 = Instant::now();
    assert!(store.create(id, &session, idle).await?);
    sleep_until(t0 + Duration::from_millis(600)).await;
    assert_eq!(store.get(id, Some(idle)).await?, Some(session.clone()));

    sleep_until(t0 + Duration::from_millis(4300)).await;
    assert_eq!(store.get(id, None).await?, Some(session));
    Ok(())
}

const EXPIRING: Duration = Duration::from_secs(1);

/// Creates one MCP session that expires after [`EXPIRING`] and one that stays live.
async fn create_expiring_and_live_sessions(
    store: &dyn McpSessionStore,
) -> anyhow::Result<McpSessionId> {
    let environment_id = EnvironmentId::new();
    assert!(
        store
            .create(new_id(), &record(environment_id, 1), EXPIRING)
            .await?
    );
    let live = new_id();
    assert!(
        store
            .create(live, &record(environment_id, 1), Duration::from_secs(300))
            .await?
    );
    Ok(live)
}

/// The number of rows of `mcp_session`.
#[async_trait]
trait SessionRows: Send + Sync {
    async fn session_rows(&self) -> anyhow::Result<i64>;
}

#[trait_gen(SqlitePool -> SqlitePool, PostgresPool)]
#[async_trait]
impl SessionRows for SqlitePool {
    async fn session_rows(&self) -> anyhow::Result<i64> {
        let row = self
            .with_ro("mcp_session_store_test", "row_count")
            .fetch_one(sqlx::query("SELECT COUNT(*) FROM mcp_session"))
            .await?;
        Ok(row.try_get::<i64, _>(0)?)
    }
}

// The writer store runs its cleanup only once, at construction, before the sessions exist. After
// the first session has expired, a cleaner store with a short interval starts on the same pool;
// its cleanup must delete exactly the expired row.
async fn cleanup_deletes_expired_mcp_sessions<P: SessionRows + Clone>(
    pool: P,
    store: impl Fn(P, Duration) -> Arc<dyn McpSessionStore>,
) -> anyhow::Result<()> {
    let writer = store(pool.clone(), Duration::from_secs(3600));
    let live = create_expiring_and_live_sessions(writer.as_ref()).await?;
    assert_eq!(pool.session_rows().await?, 2);

    sleep(EXPIRING).await;
    let _cleaner = store(pool.clone(), Duration::from_millis(50));
    wait_for_row_count(1, || pool.session_rows()).await?;
    assert!(writer.get(live, None).await?.is_some());
    Ok(())
}

#[test]
#[timeout("30s")]
async fn sqlite_cleanup_deletes_expired_mcp_sessions(_tracing: &Tracing) -> anyhow::Result<()> {
    let db_file = NamedTempFile::new()?;
    let pool = migrated_sqlite_pool(&DbSqliteConfig {
        database: db_file.path().to_string_lossy().to_string(),
        max_connections: 10,
        foreign_keys: false,
    })
    .await?;
    cleanup_deletes_expired_mcp_sessions(pool, |pool, cleanup_interval| {
        Arc::new(SqliteMcpSessionStore::new(pool, cleanup_interval))
    })
    .await
}

#[test]
#[timeout("60s")]
async fn postgres_cleanup_deletes_expired_mcp_sessions(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    let pool = migrated_postgres_pool(&unique_postgres_config(postgres)).await?;
    cleanup_deletes_expired_mcp_sessions(pool, |pool, cleanup_interval| {
        Arc::new(PostgresMcpSessionStore::new(pool, cleanup_interval))
    })
    .await
}

/// How long the sessions of the lock-wait tests live. The database clock must pass their expiry
/// while a write waits for a row lock, so each of these tests waits this long once.
const LOCK_WAIT_EXPIRY: Duration = Duration::from_secs(2);

/// A Postgres store and a pool for another transaction on the same schema, with one session
/// that expires after [`LOCK_WAIT_EXPIRY`]. Then the other transaction holds an uncommitted
/// update of the session's row, which `write` waits behind.
async fn postgres_write_behind_a_held_row_lock<T: Send + 'static>(
    postgres: &DockerPostgresRdb,
    write: impl FnOnce(Arc<PostgresMcpSessionStore>, McpSessionId) -> tokio::task::JoinHandle<T>,
) -> anyhow::Result<(Arc<PostgresMcpSessionStore>, McpSessionId, T)> {
    let config = unique_postgres_config(postgres);
    let store = Arc::new(PostgresMcpSessionStore::new(
        migrated_postgres_pool(&config).await?,
        Duration::from_secs(3600),
    ));
    let holder_pool = PostgresPool::configured(&config).await?;
    let id = new_id();
    let created = Instant::now();
    assert!(
        store
            .create(id, &record(EnvironmentId::new(), 1), LOCK_WAIT_EXPIRY)
            .await?
    );

    let mut holder = holder_pool
        .with_rw("mcp_session_store_test", "hold")
        .begin()
        .await?;
    holder
        .execute(sqlx::query("UPDATE mcp_session SET value = value"))
        .await?;
    let waiting = write(store.clone(), id);
    sleep_until(created + LOCK_WAIT_EXPIRY + Duration::from_millis(200)).await;
    assert_eq!(
        store.get(id, None).await?,
        None,
        "another reader sees the expiry"
    );
    holder.rollback().await?;
    Ok((store.clone(), id, waiting.await?))
}

// Postgres evaluates a statement's row filter before the statement waits for a row lock, and
// keeps that result when the lock holder rolls back. A read that refreshes the session waits
// behind the holder while the session expires; it must then find the row expired.
#[test]
#[timeout("60s")]
async fn postgres_mcp_session_refresh_behind_a_lock_does_not_revive_it(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    let (store, id, read) = postgres_write_behind_a_held_row_lock(postgres, |store, id| {
        tokio::spawn(async move { store.get(id, Some(Duration::from_secs(300))).await })
    })
    .await?;
    assert_eq!(read?, None, "the refresh returns no record");
    assert_eq!(store.get(id, None).await?, None, "the session stays gone");
    Ok(())
}

// A replace that waits behind the holder while the session expires must not report success.
#[test]
#[timeout("60s")]
async fn postgres_mcp_session_replace_behind_a_lock_does_not_succeed(
    _tracing: &Tracing,
    postgres: &DockerPostgresRdb,
) -> anyhow::Result<()> {
    let (store, id, replaced) = postgres_write_behind_a_held_row_lock(postgres, |store, id| {
        tokio::spawn(async move { store.replace(id, &record(EnvironmentId::new(), 2)).await })
    })
    .await?;
    assert!(!replaced?, "the replace reports a session that is not live");
    assert_eq!(store.get(id, None).await?, None, "the session stays gone");
    Ok(())
}
