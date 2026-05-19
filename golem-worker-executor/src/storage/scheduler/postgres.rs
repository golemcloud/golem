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
use crate::services::golem_config::SchedulerStoragePostgresConfig;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::SafeDisplay;
use golem_common::model::{ScheduleId, ScheduledAction, ShardAssignment, ShardId};
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::db::Pool;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use include_dir::include_dir;
use std::time::Duration;
use uuid::Uuid;

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration/scheduler");

#[derive(Debug, Clone)]
pub struct PostgresSchedulerStorage {
    pool: PostgresPool,
}

#[derive(sqlx::FromRow)]
struct ScheduledActionRow {
    schedule_id: Uuid,
    due_at_ms: i64,
    action: Vec<u8>,
    attempt_count: i32,
}

impl PostgresSchedulerStorage {
    pub async fn configured(config: &SchedulerStoragePostgresConfig) -> Result<Self, String> {
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);
        golem_service_base::db::postgres::migrate(
            &config.postgres,
            migrations.postgres_migrations(),
        )
        .await
        .map_err(|err| format!("Postgres scheduler storage migration failed: {err:?}"))?;

        let pool = PostgresPool::configured(&config.postgres)
            .await
            .map_err(|err| {
                format!("Postgres scheduler storage pool initialization failed: {err:?}")
            })?;

        Ok(Self { pool })
    }

    pub async fn new(pool: PostgresPool) -> Result<Self, String> {
        Ok(Self { pool })
    }
}

#[async_trait]
impl SchedulerStorage for PostgresSchedulerStorage {
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
            "INSERT INTO scheduled_actions (schedule_id, due_at_ms, available_at_ms, shard_id, action) VALUES ($1, $2, $2, $3, $4) ON CONFLICT (schedule_id) DO NOTHING;",
        )
        .bind(schedule_id.id)
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
        let query = sqlx::query("DELETE FROM scheduled_actions WHERE schedule_id = $1;")
            .bind(schedule_id.id);

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

        let query = sqlx::query_as::<_, ScheduledActionRow>(
            r#"
              WITH picked AS (
                  SELECT schedule_id
                    FROM scheduled_actions
                   WHERE shard_id = ANY($1)
                     AND available_at_ms <= $2
                   ORDER BY available_at_ms ASC, schedule_id ASC
                   LIMIT $3
                   FOR UPDATE SKIP LOCKED
              )
              UPDATE scheduled_actions s
                 SET lease_owner = $4,
                     lease_until_ms = $5,
                     available_at_ms = $5,
                     attempt_count = attempt_count + 1
                FROM picked
               WHERE s.schedule_id = picked.schedule_id
            RETURNING s.schedule_id, s.due_at_ms, s.action, s.attempt_count;
            "#,
        )
        .bind(shard_ids)
        .bind(now_ms)
        .bind(limit as i64)
        .bind(lease_owner)
        .bind(lease_until_ms);

        let rows = self
            .pool
            .with_rw("scheduler_storage", "claim_due")
            .fetch_all_as::<ScheduledActionRow, _>(query)
            .await
            .map_err(|err| err.to_safe_string())?;

        rows.into_iter()
            .map(|row| {
                Ok(ClaimedScheduledAction {
                    schedule_id: ScheduleId {
                        id: row.schedule_id,
                    },
                    action: deserialize(&row.action)?,
                    due_at: millis_to_datetime(row.due_at_ms)?,
                    lease_owner,
                    attempt_count: row.attempt_count as u32,
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
        let query = sqlx::query(
            "UPDATE scheduled_actions SET lease_until_ms = $3, available_at_ms = $3 WHERE schedule_id = $1 AND lease_owner = $2;",
        )
        .bind(schedule_id.id)
        .bind(lease_owner)
        .bind(datetime_to_millis(lease_until));

        self.pool
            .with_rw("scheduler_storage", "extend_lease")
            .execute(query)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|err| err.to_safe_string())
    }

    async fn ack(&self, schedule_id: &ScheduleId, lease_owner: Uuid) -> Result<bool, String> {
        let query = sqlx::query(
            "DELETE FROM scheduled_actions WHERE schedule_id = $1 AND lease_owner = $2;",
        )
        .bind(schedule_id.id)
        .bind(lease_owner);

        self.pool
            .with_rw("scheduler_storage", "ack")
            .execute(query)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|err| err.to_safe_string())
    }
}
