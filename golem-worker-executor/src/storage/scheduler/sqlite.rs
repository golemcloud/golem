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

use super::{ClaimedScheduledAction, SchedulerStorage, datetime_to_millis, millis_to_datetime};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::FutureExt;
use golem_common::SafeDisplay;
use golem_common::model::{ScheduleId, ScheduledAction, ShardAssignment, ShardId};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::db::Pool;
use golem_service_base::db::sqlite::SqlitePool;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SqliteSchedulerStorage {
    pool: SqlitePool,
}

#[derive(sqlx::FromRow, Clone)]
struct ScheduledActionRow {
    schedule_id: String,
    due_at_ms: i64,
    action: Vec<u8>,
    attempt_count: i32,
}

impl SqliteSchedulerStorage {
    pub async fn new(pool: SqlitePool) -> Result<Self, String> {
        let result = Self { pool };
        result.init().await?;
        Ok(result)
    }

    async fn init(&self) -> Result<(), String> {
        let api = self.pool.with_rw("scheduler_storage", "init");
        api.execute(sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS scheduled_actions (
                    schedule_id     TEXT NOT NULL,
                    due_at_ms       INTEGER NOT NULL,
                    available_at_ms INTEGER NOT NULL,
                    shard_id        INTEGER NOT NULL,
                    action          BLOB NOT NULL,
                    lease_owner     TEXT NULL,
                    lease_until_ms  INTEGER NULL,
                    attempt_count   INTEGER NOT NULL DEFAULT 0,
                    CONSTRAINT scheduled_actions_pk PRIMARY KEY (schedule_id)
                );
                "#,
        ))
        .await
        .map_err(|err| err.to_safe_string())?;
        api.execute(sqlx::query(
            "CREATE INDEX IF NOT EXISTS scheduled_actions_claim_idx ON scheduled_actions (shard_id, available_at_ms, schedule_id);",
        ))
        .await
        .map_err(|err| err.to_safe_string())?;
        Ok(())
    }
}

#[async_trait]
impl SchedulerStorage for SqliteSchedulerStorage {
    async fn insert(
        &self,
        schedule_id: ScheduleId,
        due_at: DateTime<Utc>,
        shard_id: ShardId,
        action: &ScheduledAction,
    ) -> Result<(), String> {
        let action = serialize(action)?;
        let due_at_ms = datetime_to_millis(due_at);
        let query = sqlx::query(
            "INSERT OR IGNORE INTO scheduled_actions (schedule_id, due_at_ms, available_at_ms, shard_id, action) VALUES (?, ?, ?, ?, ?);",
        )
        .bind(schedule_id.id.to_string())
        .bind(due_at_ms)
        .bind(due_at_ms)
        .bind(shard_id.value())
        .bind(action);

        self.pool
            .with_rw("scheduler_storage", "insert")
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn cancel(&self, schedule_id: &ScheduleId) -> Result<(), String> {
        let query = sqlx::query("DELETE FROM scheduled_actions WHERE schedule_id = ?;")
            .bind(schedule_id.id.to_string());

        self.pool
            .with_rw("scheduler_storage", "cancel")
            .execute(query)
            .await
            .map(|_| ())
            .map_err(|err| err.to_safe_string())
    }

    async fn claim_due(
        &self,
        now: DateTime<Utc>,
        assignment: &ShardAssignment,
        limit: u32,
        lease_ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, String> {
        if limit == 0 || assignment.shard_ids.is_empty() {
            return Ok(Vec::new());
        }

        let now_ms = datetime_to_millis(now);
        let lease_owner = Uuid::now_v7();
        let lease_until_ms = datetime_to_millis(now + lease_ttl);
        let shard_ids: Vec<i64> = assignment
            .shard_ids
            .iter()
            .map(|shard| shard.value())
            .collect();

        let mut shard_placeholders = String::with_capacity(shard_ids.len() * 2);
        for i in 0..shard_ids.len() {
            if i > 0 {
                shard_placeholders.push(',');
            }
            shard_placeholders.push('?');
        }
        let select_sql = format!(
            r#"
            SELECT schedule_id, due_at_ms, action, attempt_count
              FROM scheduled_actions
             WHERE shard_id IN ({shard_placeholders})
               AND available_at_ms <= ?
             ORDER BY available_at_ms ASC, schedule_id ASC
             LIMIT ?;
            "#
        );

        let rows = self
            .pool
            .with_tx("scheduler_storage", "claim_due", |tx| {
                async move {
                    let mut select_query = sqlx::query_as::<_, ScheduledActionRow>(&select_sql);
                    for shard_id in &shard_ids {
                        select_query = select_query.bind(*shard_id);
                    }
                    select_query = select_query.bind(now_ms).bind(limit as i64);
                    let selected = tx.fetch_all_as::<ScheduledActionRow, _>(select_query).await?;

                    for row in &selected {
                        tx.execute(
                            sqlx::query(
                                "UPDATE scheduled_actions SET lease_owner = ?, lease_until_ms = ?, available_at_ms = ?, attempt_count = attempt_count + 1 WHERE schedule_id = ?;",
                            )
                            .bind(lease_owner.to_string())
                            .bind(lease_until_ms)
                            .bind(lease_until_ms)
                            .bind(&row.schedule_id),
                        )
                        .await?;
                    }

                    Ok(selected)
                }
                .boxed()
            })
            .await
            .map_err(|err| err.to_safe_string())?;

        rows.into_iter()
            .map(|row| {
                Ok(ClaimedScheduledAction {
                    schedule_id: ScheduleId {
                        id: Uuid::parse_str(&row.schedule_id).map_err(|err| err.to_string())?,
                    },
                    action: deserialize(&row.action)?,
                    due_at: millis_to_datetime(row.due_at_ms)?,
                    lease_owner,
                    attempt_count: row.attempt_count as u32 + 1,
                })
            })
            .collect()
    }

    async fn extend_lease(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, String> {
        let lease_until_ms = datetime_to_millis(lease_until);
        let query = sqlx::query(
            "UPDATE scheduled_actions SET lease_until_ms = ?, available_at_ms = ? WHERE schedule_id = ? AND lease_owner = ?;",
        )
        .bind(lease_until_ms)
        .bind(lease_until_ms)
        .bind(schedule_id.id.to_string())
        .bind(lease_owner.to_string());

        self.pool
            .with_rw("scheduler_storage", "extend_lease")
            .execute(query)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|err| err.to_safe_string())
    }

    async fn ack(&self, schedule_id: &ScheduleId, lease_owner: Uuid) -> Result<bool, String> {
        let query =
            sqlx::query("DELETE FROM scheduled_actions WHERE schedule_id = ? AND lease_owner = ?;")
                .bind(schedule_id.id.to_string())
                .bind(lease_owner.to_string());

        self.pool
            .with_rw("scheduler_storage", "ack")
            .execute(query)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|err| err.to_safe_string())
    }
}
