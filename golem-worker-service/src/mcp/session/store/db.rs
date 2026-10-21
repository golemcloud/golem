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

//! Each record is one row of `mcp_session`; `expires_at` holds Unix milliseconds of the database
//! clock. Every statement reads that clock when it executes, so the expiry checks of all
//! replicas use one clock and no statement acts on a time taken before it ran. A read moves the
//! expiry forward only when [`expiry_write`] asks for it, so a busy session costs at most one
//! write per slack period.
//!
//! An expiry-guarded write (a refresh or a replace) runs in a transaction that holds the row's
//! write lock before any statement reads the clock: on Postgres `SELECT .. FOR UPDATE` takes the
//! row lock, and on SQLite the pool begins every write transaction with `BEGIN IMMEDIATE`, which
//! takes the database write lock. A clock read before a lock wait could otherwise judge a row
//! live that expired during the wait.

use super::{McpSessionId, McpSessionRecord, McpSessionStore, McpSessionStoreError, millis};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::StreamExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use sqlx::Row;
use std::time::Duration;
use tokio_stream::wrappers::IntervalStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, error};

const SVC_NAME: &str = "mcp_session_store";

/// The longest time a database expiry write may be skipped.
const MAX_SLACK: Duration = Duration::from_secs(30);

/// The longest lifetime a statement adds to the database clock, so that the sum stays in the
/// range of a 64-bit integer.
const MAX_LIFETIME_MS: i64 = i64::MAX / 4;

/// The statements of the store on one database, named as `Self::..` in the store's methods.
trait McpSessionSql {
    /// Binds the ID, the encoded record and the lifetime in milliseconds.
    const CREATE: &'static str;
    /// Binds the ID; returns the value, the expiry and the clock of the read.
    const GET: &'static str;
    /// Binds the ID and the lifetime in milliseconds.
    const EXTEND: &'static str;
    /// Binds the ID and the encoded record.
    const REPLACE: &'static str;
    const DELETE_EXPIRED: &'static str;
    /// Binds the ID; takes the row lock inside a transaction. `None` where beginning the write
    /// transaction already takes the lock.
    const LOCK: Option<&'static str>;
}

/// Implements [`McpSessionSql`] for the store type `$store`. `$now` is the database clock in Unix
/// milliseconds at its evaluation. `$cleanup_now` is the same clock at the start of the
/// statement, which an index scan of `expires_at` can use; an earlier clock only deletes fewer
/// rows. `$lock` is the statement that takes the row lock, if any.
macro_rules! mcp_session_sql {
    ($store:ty, $now:literal, $cleanup_now:literal, $lock:expr) => {
        impl McpSessionSql for $store {
            const CREATE: &'static str = concat!(
                "INSERT INTO mcp_session (session_id, value, expires_at) VALUES ($1, $2, ",
                $now,
                " + $3) ON CONFLICT (session_id) DO NOTHING"
            );
            const GET: &'static str = concat!(
                "SELECT value, expires_at, ",
                $now,
                " FROM mcp_session WHERE session_id = $1 AND expires_at > ",
                $now
            );
            const EXTEND: &'static str = concat!(
                "UPDATE mcp_session SET expires_at = ",
                $now,
                " + $2 WHERE session_id = $1 AND expires_at > ",
                $now,
                " AND expires_at < ",
                $now,
                " + $2"
            );
            const REPLACE: &'static str = concat!(
                "UPDATE mcp_session SET value = $2 WHERE session_id = $1 AND expires_at > ",
                $now
            );
            const DELETE_EXPIRED: &'static str =
                concat!("DELETE FROM mcp_session WHERE expires_at <= ", $cleanup_now);
            const LOCK: Option<&'static str> = $lock;
        }
    };
}

mcp_session_sql!(
    DbMcpSessionStore<SqlitePool>,
    "CAST(ROUND(unixepoch('subsec') * 1000) AS INTEGER)",
    "CAST(ROUND(unixepoch('subsec') * 1000) AS INTEGER)",
    None
);
mcp_session_sql!(
    DbMcpSessionStore<PostgresPool>,
    "FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT",
    "FLOOR(EXTRACT(EPOCH FROM statement_timestamp()) * 1000)::BIGINT",
    Some("SELECT 1 FROM mcp_session WHERE session_id = $1 FOR UPDATE")
);

pub type SqliteMcpSessionStore = DbMcpSessionStore<SqlitePool>;
pub type PostgresMcpSessionStore = DbMcpSessionStore<PostgresPool>;

/// MCP session store on a relational database whose pool holds the session store tables. A
/// background task deletes expired rows until the store is dropped.
pub struct DbMcpSessionStore<P> {
    pool: P,
    cleanup_cancel: CancellationToken,
}

impl<P> Drop for DbMcpSessionStore<P> {
    fn drop(&mut self) {
        self.cleanup_cancel.cancel();
    }
}

/// What a read of a live row saw: the stored expiry and the database clock at the read, both in
/// Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpiryRead {
    expires_at: i64,
    read_at: i64,
}

/// The lifetime that a write after `read` which keeps the session live for `idle` gives the
/// session, or `None` while the stored expiry is already at least `read_at + idle`. The lifetime
/// is `idle + slack`, with `slack = min(30 s, idle / 4)`: a session never ends earlier than
/// `idle` after its last read, and a busy session is written at most once per slack.
fn expiry_write(read: ExpiryRead, idle: Duration) -> Option<i64> {
    let idle_ms = millis(idle);
    let slack = millis(MAX_SLACK.min(idle / 4));
    (read.expires_at < read.read_at.saturating_add(idle_ms))
        .then(|| idle_ms.saturating_add(slack).min(MAX_LIFETIME_MS))
}

#[trait_gen(SqlitePool -> SqlitePool, PostgresPool)]
impl DbMcpSessionStore<SqlitePool> {
    /// A store on `pool`, which must hold the session store tables. Expired rows are deleted
    /// every `cleanup_interval`, the first time at once.
    pub fn new(pool: SqlitePool, cleanup_interval: Duration) -> Self {
        let cleanup_cancel = CancellationToken::new();
        let cleanup_pool = pool.clone();
        let cancelled = cleanup_cancel.clone().cancelled_owned();
        tokio::spawn(
            async move {
                let mut ticks = tokio::time::interval(cleanup_interval);
                ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                IntervalStream::new(ticks)
                    .take_until(cancelled)
                    .for_each(|_| Self::delete_expired(&cleanup_pool))
                    .await
            }
            .in_current_span(),
        );
        Self {
            pool,
            cleanup_cancel,
        }
    }

    async fn delete_expired(pool: &SqlitePool) {
        if let Err(err) = pool
            .with_rw(SVC_NAME, "delete_expired")
            .execute(sqlx::query(Self::DELETE_EXPIRED))
            .await
        {
            error!(error = %err, "Failed to delete expired MCP sessions");
        }
    }

    /// Takes the session row's write lock in `tx`, unless beginning the transaction already took
    /// the database write lock.
    async fn lock_row(
        tx: &mut <<SqlitePool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        id: McpSessionId,
    ) -> Result<(), McpSessionStoreError> {
        if let Some(lock) = Self::LOCK {
            tx.execute(sqlx::query(lock).bind(id.uuid())).await?;
        }
        Ok(())
    }

    /// Keeps the session live for at least `idle` from now, under the row's write lock. Returns
    /// the record's value when the row is live after the lock is taken and, where
    /// [`expiry_write`] asks for a write, still live when the write executes; otherwise `None`.
    async fn refresh(
        &self,
        id: McpSessionId,
        idle: Duration,
    ) -> Result<Option<Vec<u8>>, McpSessionStoreError> {
        self.pool
            .with_tx_err(SVC_NAME, "refresh", |tx| {
                Box::pin(async move {
                    Self::lock_row(tx, id).await?;
                    let Some(row) = tx
                        .fetch_optional(sqlx::query(Self::GET).bind(id.uuid()))
                        .await?
                    else {
                        return Ok(None);
                    };
                    let value: Vec<u8> = row.get(0);
                    let read = ExpiryRead {
                        expires_at: row.get(1),
                        read_at: row.get(2),
                    };
                    let Some(lifetime) = expiry_write(read, idle) else {
                        return Ok(Some(value));
                    };
                    let extended = tx
                        .execute(sqlx::query(Self::EXTEND).bind(id.uuid()).bind(lifetime))
                        .await?;
                    Ok((extended.rows_affected() == 1).then_some(value))
                })
            })
            .await
    }
}

#[trait_gen(SqlitePool -> SqlitePool, PostgresPool)]
#[async_trait]
impl McpSessionStore for DbMcpSessionStore<SqlitePool> {
    async fn create(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
        ttl: Duration,
    ) -> Result<bool, McpSessionStoreError> {
        let result = self
            .pool
            .with_rw(SVC_NAME, "create")
            .execute(
                sqlx::query(Self::CREATE)
                    .bind(id.uuid())
                    .bind(record.encode()?)
                    .bind(millis(ttl).min(MAX_LIFETIME_MS)),
            )
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn get(
        &self,
        id: McpSessionId,
        extend: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, McpSessionStoreError> {
        let row = self
            .pool
            .with_ro(SVC_NAME, "get")
            .fetch_optional(sqlx::query(Self::GET).bind(id.uuid()))
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let read = ExpiryRead {
            expires_at: row.get(1),
            read_at: row.get(2),
        };
        // This read decides without the row lock whether a refresh is due; the refresh takes the
        // lock and decides again.
        let value = match extend.filter(|idle| expiry_write(read, *idle).is_some()) {
            Some(idle) => self.refresh(id, idle).await?,
            None => Some(row.get(0)),
        };
        value
            .map(|value| McpSessionRecord::decode(&value))
            .transpose()
    }

    async fn replace(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
    ) -> Result<bool, McpSessionStoreError> {
        let value = record.encode()?;
        self.pool
            .with_tx_err(SVC_NAME, "replace", |tx| {
                Box::pin(async move {
                    Self::lock_row(tx, id).await?;
                    let replaced = tx
                        .execute(sqlx::query(Self::REPLACE).bind(id.uuid()).bind(value))
                        .await?;
                    Ok(replaced.rows_affected() == 1)
                })
            })
            .await
    }

    async fn delete(&self, id: McpSessionId) -> Result<(), McpSessionStoreError> {
        self.pool
            .with_rw(SVC_NAME, "delete")
            .execute(sqlx::query("DELETE FROM mcp_session WHERE session_id = $1").bind(id.uuid()))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DbMcpSessionStore, ExpiryRead, MAX_LIFETIME_MS, SqliteMcpSessionStore, expiry_write,
    };
    use crate::custom_api::oidc::session_store::migrated_sqlite_pool;
    use crate::mcp::session::store::{McpSessionId, McpSessionRecord, McpSessionStore};
    use golem_common::config::DbSqliteConfig;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::db::sqlite::SqlitePool;
    use sqlx::Connection;
    use sqlx::sqlite::{SqliteConnection, SqlitePoolOptions};
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use tokio::sync::Semaphore;
    use tokio::time::timeout;
    use tokio_util::sync::CancellationToken;

    const SECOND: i64 = 1000;

    #[test]
    fn expiry_write_table() {
        let t0 = 1_000_000_000;
        let five_minutes = Duration::from_secs(300);
        [
            // Stored expiry at least `now + idle`: no write.
            (t0 + 400 * SECOND, t0, five_minutes, None),
            (t0 + 300 * SECOND, t0, five_minutes, None),
            // Stored expiry earlier than `now + idle`: write a lifetime of `idle + slack`.
            (t0 + 300 * SECOND - 1, t0, five_minutes, Some(330 * SECOND)),
            (t0 - 1, t0, five_minutes, Some(330 * SECOND)),
            // The slack is a quarter of a short idle timeout.
            (t0, t0, Duration::from_secs(2), Some(2_500)),
            // Idle 4 s, created at t0, read at t0 + 0.6 s: a write at t0 + 0.6 s keeps the
            // session live until t0 + 5.6 s, later than `idle` after the read.
            (
                t0 + 4 * SECOND,
                t0 + 600,
                Duration::from_secs(4),
                Some(5_000),
            ),
            // A value written at t0 + 0.6 s is skipped until a slack has passed.
            (t0 + 5_600, t0 + 1_599, Duration::from_secs(4), None),
            (t0 + 5_600, t0 + 1_601, Duration::from_secs(4), Some(5_000)),
            // No overflow, and the lifetime stays within the bound.
            (i64::MAX, i64::MAX - 1, Duration::MAX, None),
            (0, i64::MAX - 1, Duration::MAX, Some(MAX_LIFETIME_MS)),
        ]
        .into_iter()
        .for_each(|(expires_at, read_at, idle, expected)| {
            assert_eq!(
                expiry_write(
                    ExpiryRead {
                        expires_at,
                        read_at
                    },
                    idle
                ),
                expected,
                "stored {expires_at}, read at {read_at}, idle {idle:?}"
            )
        });
    }

    // A request reads a live session that needs a refresh while another writer holds the
    // database write lock, and the session expires before that writer commits: the writer moves
    // the expiry two minutes back. The refresh then runs under the lock, finds the row expired
    // and writes nothing, so the read returns no record. The read pool tells the test when the
    // first read is done; the store runs no cleanup task.
    #[test]
    async fn a_refresh_after_a_committed_expiry_returns_no_record() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let config = DbSqliteConfig {
            database: db_file.path().to_string_lossy().to_string(),
            max_connections: 1,
            foreign_keys: false,
        };
        drop(migrated_sqlite_pool(&config).await.unwrap());
        let read_done = Arc::new(Semaphore::new(0));
        let released = read_done.clone();
        let read_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_release(move |_, _| {
                let released = released.clone();
                Box::pin(async move {
                    released.add_permits(1);
                    Ok(true)
                })
            })
            .connect_with(config.connect_options())
            .await
            .unwrap();
        let write_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(config.connect_options())
            .await
            .unwrap();
        let store: Arc<SqliteMcpSessionStore> = Arc::new(DbMcpSessionStore {
            pool: SqlitePool::new(read_pool, write_pool),
            cleanup_cancel: CancellationToken::new(),
        });
        let id = McpSessionId::generate().unwrap();
        let record = McpSessionRecord {
            domain: Domain("mcp.example.com".to_string()),
            environment_id: EnvironmentId::new(),
            deployment_revision: DeploymentRevision::INITIAL,
        };
        assert!(
            store
                .create(id, &record, Duration::from_secs(60))
                .await
                .unwrap()
        );

        let mut writer = SqliteConnection::connect_with(&config.connect_options())
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut writer)
            .await
            .unwrap();
        sqlx::query("UPDATE mcp_session SET expires_at = expires_at - 120000")
            .execute(&mut writer)
            .await
            .unwrap();
        let read = tokio::spawn({
            let store = store.clone();
            async move { store.get(id, Some(Duration::from_secs(300))).await }
        });
        timeout(Duration::from_secs(5), read_done.acquire())
            .await
            .expect("the first read is done")
            .unwrap()
            .forget();
        sqlx::query("COMMIT").execute(&mut writer).await.unwrap();

        assert_eq!(read.await.unwrap().unwrap(), None);
        assert_eq!(store.get(id, None).await.unwrap(), None);
    }
}
