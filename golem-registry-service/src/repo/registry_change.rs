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
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult};
use indoc::indoc;
use sqlx::Row;
use uuid::Uuid;

/// Newtype for registry change event IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChangeEventId(pub i64);

/// The type of registry change event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i16)]
pub enum RegistryEventType {
    DeploymentChanged = 0,
    AccountTokensInvalidated = 1,
    EnvironmentPermissionsChanged = 2,
    DomainRegistrationChanged = 3,
}

impl TryFrom<i16> for RegistryEventType {
    type Error = RepoError;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(RegistryEventType::DeploymentChanged),
            1 => Ok(RegistryEventType::AccountTokensInvalidated),
            2 => Ok(RegistryEventType::EnvironmentPermissionsChanged),
            3 => Ok(RegistryEventType::DomainRegistrationChanged),
            other => Err(RepoError::InternalError(anyhow::anyhow!(
                "Unknown registry event type: {other}"
            ))),
        }
    }
}

impl From<RegistryEventType> for i16 {
    fn from(value: RegistryEventType) -> Self {
        value as i16
    }
}

/// A registry change event with a typed payload.
#[derive(Debug, Clone)]
pub enum RegistryChangeEvent {
    DeploymentChanged {
        event_id: ChangeEventId,
        environment_id: Uuid,
        deployment_revision_id: i64,
    },
    AccountTokensInvalidated {
        event_id: ChangeEventId,
        account_id: Uuid,
    },
    EnvironmentPermissionsChanged {
        event_id: ChangeEventId,
        environment_id: Uuid,
        grantee_account_id: Uuid,
    },
    DomainRegistrationChanged {
        event_id: ChangeEventId,
        environment_id: Uuid,
        domains: Vec<String>,
    },
}

impl RegistryChangeEvent {
    pub fn event_id(&self) -> ChangeEventId {
        match self {
            Self::DeploymentChanged { event_id, .. } => *event_id,
            Self::AccountTokensInvalidated { event_id, .. } => *event_id,
            Self::EnvironmentPermissionsChanged { event_id, .. } => *event_id,
            Self::DomainRegistrationChanged { event_id, .. } => *event_id,
        }
    }
}

/// Flat row representation matching the DB schema, used only for deserialization.
struct RegistryChangeEventRow {
    event_id: ChangeEventId,
    event_type: RegistryEventType,
    environment_id: Option<Uuid>,
    deployment_revision_id: Option<i64>,
    account_id: Option<Uuid>,
    grantee_account_id: Option<Uuid>,
    domains: Vec<String>,
}

impl TryFrom<RegistryChangeEventRow> for RegistryChangeEvent {
    type Error = RepoError;

    fn try_from(row: RegistryChangeEventRow) -> Result<Self, Self::Error> {
        match row.event_type {
            RegistryEventType::DeploymentChanged => {
                let environment_id = row.environment_id.ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "DeploymentChanged event missing environment_id"
                    ))
                })?;
                let deployment_revision_id = row.deployment_revision_id.ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "DeploymentChanged event missing deployment_revision_id"
                    ))
                })?;
                Ok(RegistryChangeEvent::DeploymentChanged {
                    event_id: row.event_id,
                    environment_id,
                    deployment_revision_id,
                })
            }
            RegistryEventType::AccountTokensInvalidated => {
                let account_id = row.account_id.ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "AccountTokensInvalidated event missing account_id"
                    ))
                })?;
                Ok(RegistryChangeEvent::AccountTokensInvalidated {
                    event_id: row.event_id,
                    account_id,
                })
            }
            RegistryEventType::EnvironmentPermissionsChanged => {
                let environment_id = row.environment_id.ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "EnvironmentPermissionsChanged event missing environment_id"
                    ))
                })?;
                let grantee_account_id = row.grantee_account_id.ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "EnvironmentPermissionsChanged event missing grantee_account_id"
                    ))
                })?;
                Ok(RegistryChangeEvent::EnvironmentPermissionsChanged {
                    event_id: row.event_id,
                    environment_id,
                    grantee_account_id,
                })
            }
            RegistryEventType::DomainRegistrationChanged => {
                let environment_id = row.environment_id.ok_or_else(|| {
                    RepoError::InternalError(anyhow::anyhow!(
                        "DomainRegistrationChanged event missing environment_id"
                    ))
                })?;
                Ok(RegistryChangeEvent::DomainRegistrationChanged {
                    event_id: row.event_id,
                    environment_id,
                    domains: row.domains,
                })
            }
        }
    }
}

/// Data for inserting a new registry change event (no event_id yet).
#[derive(Debug, Clone)]
pub struct NewRegistryChangeEvent {
    pub event_type: RegistryEventType,
    pub environment_id: Option<Uuid>,
    pub deployment_revision_id: Option<i64>,
    pub account_id: Option<Uuid>,
    pub grantee_account_id: Option<Uuid>,
    pub domains: Vec<String>,
}

impl NewRegistryChangeEvent {
    pub fn deployment_changed(
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> Self {
        Self {
            event_type: RegistryEventType::DeploymentChanged,
            environment_id: Some(environment_id),
            deployment_revision_id: Some(deployment_revision_id),
            account_id: None,
            grantee_account_id: None,
            domains: Vec::new(),
        }
    }

    pub fn domain_registration_changed(environment_id: Uuid, domains: Vec<String>) -> Self {
        Self {
            event_type: RegistryEventType::DomainRegistrationChanged,
            environment_id: Some(environment_id),
            deployment_revision_id: None,
            account_id: None,
            grantee_account_id: None,
            domains,
        }
    }

    pub fn account_tokens_invalidated(account_id: Uuid) -> Self {
        Self {
            event_type: RegistryEventType::AccountTokensInvalidated,
            environment_id: None,
            deployment_revision_id: None,
            account_id: Some(account_id),
            grantee_account_id: None,
            domains: Vec::new(),
        }
    }

    pub fn environment_permissions_changed(environment_id: Uuid, grantee_account_id: Uuid) -> Self {
        Self {
            event_type: RegistryEventType::EnvironmentPermissionsChanged,
            environment_id: Some(environment_id),
            deployment_revision_id: None,
            account_id: None,
            grantee_account_id: Some(grantee_account_id),
            domains: Vec::new(),
        }
    }
}

/// Database operations for the registry_change_events outbox table.
#[async_trait]
pub trait RegistryChangeRepo: Send + Sync {
    /// Record a registry change event in the outbox table.
    async fn record_change_event(
        &self,
        event: &NewRegistryChangeEvent,
    ) -> RepoResult<ChangeEventId>;

    /// Fetch all events with event_id > last_seen, ordered by event_id ASC.
    async fn get_events_since(
        &self,
        last_seen_event_id: ChangeEventId,
    ) -> RepoResult<Vec<RegistryChangeEvent>>;

    /// Get the latest event_id, if any.
    async fn get_latest_event_id(&self) -> RepoResult<Option<ChangeEventId>>;

    /// Delete events older than the given cutoff.
    async fn cleanup_old_events(&self, retention_seconds: i64) -> RepoResult<u64>;
}

static METRICS_SVC_NAME: &str = "registry_change";

pub struct DbRegistryChangeRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbRegistryChangeRepo<DBP> {
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
impl RegistryChangeRepo for DbRegistryChangeRepo<PostgresPool> {
    async fn record_change_event(
        &self,
        event: &NewRegistryChangeEvent,
    ) -> RepoResult<ChangeEventId> {
        let event_type: i16 = event.event_type.into();
        let domains: &[String] = &event.domains;
        let row = self
            .with_rw("record_change_event")
            .fetch_one(
                sqlx::query(indoc! { r#"
                    INSERT INTO registry_change_events
                        (event_type, environment_id, deployment_revision_id,
                         account_id, grantee_account_id, domains)
                    VALUES ($1, $2, $3, $4, $5, $6::text[])
                    RETURNING event_id
                "#})
                .bind(event_type)
                .bind(event.environment_id)
                .bind(event.deployment_revision_id)
                .bind(event.account_id)
                .bind(event.grantee_account_id)
                .bind(domains),
            )
            .await?;

        let event_id = ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?);

        let _ = self
            .with_rw("pg_notify")
            .execute(sqlx::query("SELECT pg_notify('registry_change', $1::text)").bind(event_id.0))
            .await;

        Ok(event_id)
    }

    async fn get_events_since(
        &self,
        last_seen_event_id: ChangeEventId,
    ) -> RepoResult<Vec<RegistryChangeEvent>> {
        let rows = self
            .with_ro("get_events_since")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    SELECT event_id, event_type, environment_id,
                           deployment_revision_id, account_id,
                           grantee_account_id, domains
                    FROM registry_change_events
                    WHERE event_id > $1
                    ORDER BY event_id ASC
                "#})
                .bind(last_seen_event_id.0),
            )
            .await?;

        rows.iter()
            .map(|row| {
                let event_type_raw: i16 = row.try_get("event_type").map_err(RepoError::from)?;
                let domains: Option<Vec<String>> =
                    row.try_get("domains").map_err(RepoError::from)?;
                RegistryChangeEventRow {
                    event_id: ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?),
                    event_type: RegistryEventType::try_from(event_type_raw)?,
                    environment_id: row.try_get("environment_id").map_err(RepoError::from)?,
                    deployment_revision_id: row
                        .try_get("deployment_revision_id")
                        .map_err(RepoError::from)?,
                    account_id: row.try_get("account_id").map_err(RepoError::from)?,
                    grantee_account_id: row
                        .try_get("grantee_account_id")
                        .map_err(RepoError::from)?,
                    domains: domains.unwrap_or_default(),
                }
                .try_into()
            })
            .collect()
    }

    async fn get_latest_event_id(&self) -> RepoResult<Option<ChangeEventId>> {
        let row = self
            .with_ro("get_latest_event_id")
            .fetch_optional(sqlx::query(indoc! { r#"
                    SELECT event_id FROM registry_change_events
                    ORDER BY event_id DESC
                    LIMIT 1
                "#}))
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
                    DELETE FROM registry_change_events WHERE changed_at < $1
                "#})
                .bind(cutoff),
            )
            .await?;

        Ok(result.rows_affected())
    }
}

// SQLite implementation — no pg_notify; in-process notify() is sufficient for single-node.
#[async_trait]
impl RegistryChangeRepo for DbRegistryChangeRepo<SqlitePool> {
    async fn record_change_event(
        &self,
        event: &NewRegistryChangeEvent,
    ) -> RepoResult<ChangeEventId> {
        let event_type: i16 = event.event_type.into();
        let domains_json = if event.domains.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&event.domains).map_err(|e| {
                RepoError::InternalError(anyhow::anyhow!("Failed to serialize domains: {e}"))
            })?)
        };
        let row = self
            .with_rw("record_change_event")
            .fetch_one(
                sqlx::query(indoc! { r#"
                    INSERT INTO registry_change_events
                        (event_type, environment_id, deployment_revision_id,
                         account_id, grantee_account_id, domains)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING event_id
                "#})
                .bind(event_type)
                .bind(event.environment_id)
                .bind(event.deployment_revision_id)
                .bind(event.account_id)
                .bind(event.grantee_account_id)
                .bind(&domains_json),
            )
            .await?;

        let event_id = ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?);
        Ok(event_id)
    }

    async fn get_events_since(
        &self,
        last_seen_event_id: ChangeEventId,
    ) -> RepoResult<Vec<RegistryChangeEvent>> {
        let rows = self
            .with_ro("get_events_since")
            .fetch_all(
                sqlx::query(indoc! { r#"
                    SELECT event_id, event_type, environment_id,
                           deployment_revision_id, account_id,
                           grantee_account_id, domains
                    FROM registry_change_events
                    WHERE event_id > $1
                    ORDER BY event_id ASC
                "#})
                .bind(last_seen_event_id.0),
            )
            .await?;

        rows.iter()
            .map(|row| {
                let event_type_raw: i16 = row.try_get("event_type").map_err(RepoError::from)?;
                let domains_json: Option<String> =
                    row.try_get("domains").map_err(RepoError::from)?;
                let domains: Vec<String> = match domains_json {
                    Some(json) if !json.is_empty() => serde_json::from_str(&json).map_err(|e| {
                        RepoError::InternalError(anyhow::anyhow!(
                            "Failed to deserialize domains: {e}"
                        ))
                    })?,
                    _ => Vec::new(),
                };
                RegistryChangeEventRow {
                    event_id: ChangeEventId(row.try_get("event_id").map_err(RepoError::from)?),
                    event_type: RegistryEventType::try_from(event_type_raw)?,
                    environment_id: row.try_get("environment_id").map_err(RepoError::from)?,
                    deployment_revision_id: row
                        .try_get("deployment_revision_id")
                        .map_err(RepoError::from)?,
                    account_id: row.try_get("account_id").map_err(RepoError::from)?,
                    grantee_account_id: row
                        .try_get("grantee_account_id")
                        .map_err(RepoError::from)?,
                    domains,
                }
                .try_into()
            })
            .collect()
    }

    async fn get_latest_event_id(&self) -> RepoResult<Option<ChangeEventId>> {
        let row = self
            .with_ro("get_latest_event_id")
            .fetch_optional(sqlx::query(indoc! { r#"
                    SELECT event_id FROM registry_change_events
                    ORDER BY event_id DESC
                    LIMIT 1
                "#}))
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
                    DELETE FROM registry_change_events WHERE changed_at < $1
                "#})
                .bind(cutoff),
            )
            .await?;

        Ok(result.rows_affected())
    }
}

/// Helpers for inserting registry change events inside existing transactions.
pub mod pg {
    use super::*;

    pub async fn insert_change_event_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        event: &NewRegistryChangeEvent,
    ) -> RepoResult<ChangeEventId> {
        let event_type: i16 = event.event_type.into();
        let domains: &[String] = &event.domains;
        let row = tx
            .fetch_one(
                sqlx::query(indoc! { r#"
                    INSERT INTO registry_change_events
                        (event_type, environment_id, deployment_revision_id,
                         account_id, grantee_account_id, domains)
                    VALUES ($1, $2, $3, $4, $5, $6::text[])
                    RETURNING event_id
                "#})
                .bind(event_type)
                .bind(event.environment_id)
                .bind(event.deployment_revision_id)
                .bind(event.account_id)
                .bind(event.grantee_account_id)
                .bind(domains),
            )
            .await?;

        Ok(ChangeEventId(
            row.try_get("event_id").map_err(RepoError::from)?,
        ))
    }
}

pub mod sqlite {
    use super::*;

    pub async fn insert_change_event_in_tx(
        tx: &mut <<SqlitePool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        event: &NewRegistryChangeEvent,
    ) -> RepoResult<ChangeEventId> {
        let event_type: i16 = event.event_type.into();
        let domains_json = if event.domains.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&event.domains).map_err(|e| {
                RepoError::InternalError(anyhow::anyhow!("Failed to serialize domains: {e}"))
            })?)
        };
        let row = tx
            .fetch_one(
                sqlx::query(indoc! { r#"
                    INSERT INTO registry_change_events
                        (event_type, environment_id, deployment_revision_id,
                         account_id, grantee_account_id, domains)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING event_id
                "#})
                .bind(event_type)
                .bind(event.environment_id)
                .bind(event.deployment_revision_id)
                .bind(event.account_id)
                .bind(event.grantee_account_id)
                .bind(&domains_json),
            )
            .await?;

        Ok(ChangeEventId(
            row.try_get("event_id").map_err(RepoError::from)?,
        ))
    }
}
