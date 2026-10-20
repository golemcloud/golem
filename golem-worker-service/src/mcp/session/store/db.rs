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

//! Each record is one row of `mcp_session`; `expires_at` holds Unix milliseconds. A read
//! moves the expiry forward only when [`expiry_write`] asks for it, so a busy session costs at
//! most one write per slack period.

use super::{McpSessionId, McpSessionRecord, McpSessionStore, millis};
use crate::custom_api::oidc::session_store::SessionStoreError;
use async_trait::async_trait;
use chrono::Utc;
use conditional_trait_gen::trait_gen;
use futures::StreamExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use sqlx::Row;
use std::time::Duration;
use tokio_stream::wrappers::IntervalStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, error};

const SVC_NAME: &str = "mcp_session_store";

/// The longest time a database expiry write may be skipped.
const MAX_SLACK: Duration = Duration::from_secs(30);

fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

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

/// The expiry that a read at `now_ms` which keeps the session live for `idle` writes, or `None`
/// while the stored expiry is already at least `now_ms + idle`. The written value is
/// `now_ms + idle + slack`, with `slack = min(30 s, idle / 4)`: a session never ends earlier than
/// `idle` after its last read, and a busy session is written at most once per slack.
fn expiry_write(stored_ms: i64, now_ms: i64, idle: Duration) -> Option<i64> {
    let due = now_ms.saturating_add(millis(idle));
    let slack = millis(MAX_SLACK.min(idle / 4));
    (stored_ms < due).then(|| due.saturating_add(slack))
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
                    .for_each(|_| Self::delete_expired(&cleanup_pool, now_millis()))
                    .await
            }
            .in_current_span(),
        );
        Self {
            pool,
            cleanup_cancel,
        }
    }

    async fn delete_expired(pool: &SqlitePool, now_ms: i64) {
        if let Err(err) = pool
            .with_rw(SVC_NAME, "delete_expired")
            .execute(sqlx::query("DELETE FROM mcp_session WHERE expires_at <= $1").bind(now_ms))
            .await
        {
            error!(error = %err, "Failed to delete expired MCP sessions");
        }
    }

    /// Moves the expiry of a record that is live at `now_ms` forward to `expires_at`. `false`
    /// when the record is not live, or its expiry is already at least `expires_at`.
    async fn extend(
        &self,
        id: McpSessionId,
        now_ms: i64,
        expires_at: i64,
    ) -> Result<bool, SessionStoreError> {
        let result = self
            .pool
            .with_rw(SVC_NAME, "extend")
            .execute(
                sqlx::query(
                    "UPDATE mcp_session SET expires_at = $3
                     WHERE session_id = $1 AND expires_at > $2 AND expires_at < $3",
                )
                .bind(id.uuid())
                .bind(now_ms)
                .bind(expires_at),
            )
            .await?;
        Ok(result.rows_affected() == 1)
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
    ) -> Result<bool, SessionStoreError> {
        let result = self
            .pool
            .with_rw(SVC_NAME, "create")
            .execute(
                sqlx::query(
                    "INSERT INTO mcp_session (session_id, value, expires_at) VALUES ($1, $2, $3)
                     ON CONFLICT (session_id) DO NOTHING",
                )
                .bind(id.uuid())
                .bind(record.encode()?)
                .bind(now_millis().saturating_add(millis(ttl))),
            )
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn get(
        &self,
        id: McpSessionId,
        extend: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, SessionStoreError> {
        let now_ms = now_millis();
        let row = self
            .pool
            .with_ro(SVC_NAME, "get")
            .fetch_optional(
                sqlx::query(
                    "SELECT value, expires_at FROM mcp_session
                     WHERE session_id = $1 AND expires_at > $2",
                )
                .bind(id.uuid())
                .bind(now_ms),
            )
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let value: Vec<u8> = row.get(0);
        let stored_ms: i64 = row.get(1);
        if let Some(expires_at) = extend.and_then(|idle| expiry_write(stored_ms, now_ms, idle)) {
            self.extend(id, now_ms, expires_at).await?;
        }
        McpSessionRecord::decode(&value).map(Some)
    }

    async fn replace(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
    ) -> Result<bool, SessionStoreError> {
        let result = self
            .pool
            .with_rw(SVC_NAME, "replace")
            .execute(
                sqlx::query(
                    "UPDATE mcp_session SET value = $2 WHERE session_id = $1 AND expires_at > $3",
                )
                .bind(id.uuid())
                .bind(record.encode()?)
                .bind(now_millis()),
            )
            .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete(&self, id: McpSessionId) -> Result<(), SessionStoreError> {
        self.pool
            .with_rw(SVC_NAME, "delete")
            .execute(sqlx::query("DELETE FROM mcp_session WHERE session_id = $1").bind(id.uuid()))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{SqliteMcpSessionStore, expiry_write, now_millis};
    use crate::custom_api::oidc::session_store::migrated_sqlite_pool;
    use crate::mcp::session::store::{McpSessionId, McpSessionRecord, McpSessionStore};
    use golem_common::config::DbSqliteConfig;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::EnvironmentId;
    use std::time::Duration;
    use test_r::test;

    const SECOND: i64 = 1000;

    #[test]
    fn expiry_write_table() {
        let t0 = 1_000_000_000;
        let five_minutes = Duration::from_secs(300);
        [
            // Stored expiry at least `now + idle`: no write.
            (t0 + 400 * SECOND, t0, five_minutes, None),
            (t0 + 300 * SECOND, t0, five_minutes, None),
            // Stored expiry earlier than `now + idle`: write `now + idle + slack`.
            (
                t0 + 300 * SECOND - 1,
                t0,
                five_minutes,
                Some(t0 + 330 * SECOND),
            ),
            (t0 - 1, t0, five_minutes, Some(t0 + 330 * SECOND)),
            // The slack is a quarter of a short idle timeout.
            (t0, t0, Duration::from_secs(2), Some(t0 + 2_500)),
            // Idle 4 s, created at t0, read at t0 + 0.6 s: the write keeps the session live
            // until t0 + 5.6 s, later than `idle` after the read.
            (
                t0 + 4 * SECOND,
                t0 + 600,
                Duration::from_secs(4),
                Some(t0 + 5_600),
            ),
            // A written value is skipped until a slack has passed.
            (t0 + 5_600, t0 + 1_599, Duration::from_secs(4), None),
            (
                t0 + 5_600,
                t0 + 1_601,
                Duration::from_secs(4),
                Some(t0 + 6_601),
            ),
            // No overflow.
            (i64::MAX, i64::MAX - 1, Duration::MAX, None),
            (0, i64::MAX - 1, Duration::MAX, Some(i64::MAX)),
        ]
        .into_iter()
        .for_each(|(stored, now, idle, expected)| {
            assert_eq!(
                expiry_write(stored, now, idle),
                expected,
                "stored {stored}, now {now}, idle {idle:?}"
            )
        });
    }

    #[test]
    async fn extend_does_not_revive_an_expired_row() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let pool = migrated_sqlite_pool(&DbSqliteConfig {
            database: db_file.path().to_string_lossy().to_string(),
            max_connections: 2,
            foreign_keys: false,
        })
        .await
        .unwrap();
        let store = SqliteMcpSessionStore::new(pool, Duration::from_secs(3600));
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

        // Two minutes later the row has expired, but the cleanup has not deleted it yet.
        let later = now_millis() + 120 * SECOND;
        assert!(!store.extend(id, later, later + 300 * SECOND).await.unwrap());
        // While the row is live, the same statement moves its expiry.
        let now = now_millis();
        assert!(store.extend(id, now, now + 300 * SECOND).await.unwrap());
    }
}
