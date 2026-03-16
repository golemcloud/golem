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

use crate::repo::model::datetime::SqlDateTime;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::registry::v1::DeploymentInvalidationEvent;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult};
use indoc::indoc;
use sqlx::Row;
use uuid::Uuid;

/// Newtype for deployment change event IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChangeEventId(pub i64);

/// A deployment change event emitted when the current deployment changes.
#[derive(Debug, Clone)]
pub struct DeploymentChangeEvent {
    pub event_id: ChangeEventId,
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
}

impl From<&DeploymentChangeEvent> for DeploymentInvalidationEvent {
    fn from(event: &DeploymentChangeEvent) -> Self {
        DeploymentInvalidationEvent {
            event_id: event.event_id.0 as u64,
            environment_id: Some(EnvironmentId(event.environment_id).into()),
            deployment_revision: event.deployment_revision_id as u64,
            cursor_expired: false,
        }
    }
}

/// Database operations for the deployment_change_events outbox table.
#[async_trait]
pub trait DeploymentChangeRepo: Send + Sync {
    /// Record a deployment change event in the outbox table.
    /// Returns the event_id of the inserted row.
    async fn record_change_event(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<ChangeEventId>;

    /// Fetch all events with event_id > last_seen, ordered by event_id ASC.
    async fn get_events_since(
        &self,
        last_seen_event_id: ChangeEventId,
    ) -> RepoResult<Vec<DeploymentChangeEvent>>;

    /// Get the latest event_id, if any.
    async fn get_latest_event_id(&self) -> RepoResult<Option<ChangeEventId>>;

    /// Delete events older than the given cutoff.
    async fn cleanup_old_events(&self, retention_seconds: i64) -> RepoResult<u64>;
}

static METRICS_SVC_NAME: &str = "deployment_change";

pub struct DbDeploymentChangeRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbDeploymentChangeRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

// Postgres implementation — includes pg_notify for multi-node propagation.
#[async_trait]
impl DeploymentChangeRepo for DbDeploymentChangeRepo<PostgresPool> {
    async fn record_change_event(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<ChangeEventId> {
        let row = self
            .with_rw("record_change_event")
            .fetch_one(
                sqlx::query(indoc! { r#"
                    INSERT INTO deployment_change_events (environment_id, deployment_revision_id)
                    VALUES ($1, $2)
                    RETURNING event_id
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id),
            )
            .await?;

        let event_id = ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?);

        // Notify other registry nodes via Postgres LISTEN/NOTIFY.
        // Each node runs a PgListener on the "deployment_change" channel
        // and feeds received events into its local broadcast.
        let _ = self
            .with_rw("pg_notify")
            .execute(
                sqlx::query("SELECT pg_notify('deployment_change', $1::text)")
                    .bind(event_id.0),
            )
            .await;

        Ok(event_id)
    }

    async fn get_events_since(
        &self,
        last_seen_event_id: ChangeEventId,
    ) -> RepoResult<Vec<DeploymentChangeEvent>> {
        let rows = self
            .with_ro("get_events_since")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    SELECT event_id, environment_id, deployment_revision_id
                    FROM deployment_change_events
                    WHERE event_id > $1
                    ORDER BY event_id ASC
                "#})
                .bind(last_seen_event_id.0),
            )
            .await?;

        rows.iter()
            .map(|row| {
                Ok(DeploymentChangeEvent {
                    event_id: ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?),
                    environment_id: row.try_get("environment_id").map_err(RepoError::from)?,
                    deployment_revision_id: row
                        .try_get("deployment_revision_id")
                        .map_err(RepoError::from)?,
                })
            })
            .collect()
    }

    async fn get_latest_event_id(&self) -> RepoResult<Option<ChangeEventId>> {
        let row = self
            .with_ro("get_latest_event_id")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT event_id FROM deployment_change_events
                    ORDER BY event_id DESC
                    LIMIT 1
                "#}),
            )
            .await?;

        match row {
            Some(row) => Ok(Some(ChangeEventId(
                row.try_get("event_id").map_err(RepoError::from)?,
            ))),
            None => Ok(None),
        }
    }

    async fn cleanup_old_events(&self, retention_seconds: i64) -> RepoResult<u64> {
        let cutoff =
            SqlDateTime::new(chrono::Utc::now() - chrono::Duration::seconds(retention_seconds));
        let result = self
            .with_rw("cleanup_old_events")
            .execute(
                sqlx::query(indoc! { r#"
                    DELETE FROM deployment_change_events WHERE changed_at < $1
                "#})
                .bind(cutoff),
            )
            .await?;

        Ok(result.rows_affected())
    }
}

// SQLite implementation — no pg_notify; in-process notify() is sufficient for single-node.
#[async_trait]
impl DeploymentChangeRepo for DbDeploymentChangeRepo<SqlitePool> {
    async fn record_change_event(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<ChangeEventId> {
        let row = self
            .with_rw("record_change_event")
            .fetch_one(
                sqlx::query(indoc! { r#"
                    INSERT INTO deployment_change_events (environment_id, deployment_revision_id)
                    VALUES ($1, $2)
                    RETURNING event_id
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id),
            )
            .await?;

        let event_id = ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?);
        Ok(event_id)
    }

    async fn get_events_since(
        &self,
        last_seen_event_id: ChangeEventId,
    ) -> RepoResult<Vec<DeploymentChangeEvent>> {
        let rows = self
            .with_ro("get_events_since")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    SELECT event_id, environment_id, deployment_revision_id
                    FROM deployment_change_events
                    WHERE event_id > $1
                    ORDER BY event_id ASC
                "#})
                .bind(last_seen_event_id.0),
            )
            .await?;

        rows.iter()
            .map(|row| {
                Ok(DeploymentChangeEvent {
                    event_id: ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?),
                    environment_id: row.try_get("environment_id").map_err(RepoError::from)?,
                    deployment_revision_id: row
                        .try_get("deployment_revision_id")
                        .map_err(RepoError::from)?,
                })
            })
            .collect()
    }

    async fn get_latest_event_id(&self) -> RepoResult<Option<ChangeEventId>> {
        let row = self
            .with_ro("get_latest_event_id")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT event_id FROM deployment_change_events
                    ORDER BY event_id DESC
                    LIMIT 1
                "#}),
            )
            .await?;

        match row {
            Some(row) => Ok(Some(ChangeEventId(
                row.try_get("event_id").map_err(RepoError::from)?,
            ))),
            None => Ok(None),
        }
    }

    async fn cleanup_old_events(&self, retention_seconds: i64) -> RepoResult<u64> {
        let cutoff =
            SqlDateTime::new(chrono::Utc::now() - chrono::Duration::seconds(retention_seconds));
        let result = self
            .with_rw("cleanup_old_events")
            .execute(
                sqlx::query(indoc! { r#"
                    DELETE FROM deployment_change_events WHERE changed_at < $1
                "#})
                .bind(cutoff),
            )
            .await?;

        Ok(result.rows_affected())
    }
}
