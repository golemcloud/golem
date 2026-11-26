// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::model::agent_secrets::{
    AgentSecretAuthExtRevisionRecord, AgentSecretCreationRecord, AgentSecretDestinations,
    AgentSecretExtRevisionRecord, AgentSecretRepoError, AgentSecretRevisionRecord,
};
use super::registry_change::{RequiresNotificationSignal, RequiresSignalExt};
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::agent_secrets::AgentSecretRecord;
use crate::repo::registry_change::{DbRegistryChangeRepo, NewRegistryChangeEvent};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::model::agent_secret::{
    AgentSecretId, AgentSecretOwner, AgentSecretReadMode, AgentSecretRevisionSelector,
};
use golem_common::model::type_id::TypeId;
use golem_common::model::{AgentFingerprint, AgentId};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{PoolLabelledTransaction, ResultExt};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait AgentSecretRepo: Send + Sync {
    async fn create(
        &self,
        record: AgentSecretCreationRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    async fn update(
        &self,
        revision: AgentSecretRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    async fn delete(
        &self,
        revision: AgentSecretRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    async fn get_by_id(
        &self,
        agent_secret_id: Uuid,
    ) -> Result<Option<AgentSecretAuthExtRevisionRecord>, AgentSecretRepoError>;

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    async fn get_for_environment_and_path(
        &self,
        environment_id: Uuid,
        path: Vec<String>,
    ) -> Result<Option<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    async fn get_for_agent_owner(
        &self,
        environment_id: Uuid,
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    async fn get_for_agent_type(
        &self,
        environment_id: Uuid,
        type_id: TypeId,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;

    /// Historical reads include soft-deleted secrets and parents, but never deletion revisions.
    async fn get_revision(
        &self,
        environment_id: Uuid,
        owner: &AgentSecretOwner,
        agent_secret_id: AgentSecretId,
        path: Vec<String>,
        selector: AgentSecretRevisionSelector,
        mode: AgentSecretReadMode,
    ) -> Result<Option<AgentSecretExtRevisionRecord>, AgentSecretRepoError>;
}

pub struct LoggedAgentSecretRepo<Repo: AgentSecretRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "agent secret repository";

impl<Repo: AgentSecretRepo> LoggedAgentSecretRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_environment_id(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id=%environment_id)
    }

    fn span_agent_secret_id(agent_secret_id: Uuid) -> Span {
        info_span!(SPAN_NAME, agent_secret_id=%agent_secret_id)
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl<Repo: AgentSecretRepo> AgentSecretRepo for LoggedAgentSecretRepo<Repo> {
    async fn create(
        &self,
        record: AgentSecretCreationRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>
    {
        let span = Self::span_environment_id(record.environment_id);
        self.repo.create(record).instrument(span).await
    }

    async fn update(
        &self,
        revision: AgentSecretRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>
    {
        let span = Self::span_agent_secret_id(revision.agent_secret_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: AgentSecretRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>
    {
        let span = Self::span_agent_secret_id(revision.agent_secret_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        agent_secret_id: Uuid,
    ) -> Result<Option<AgentSecretAuthExtRevisionRecord>, AgentSecretRepoError> {
        self.repo
            .get_by_id(agent_secret_id)
            .instrument(Self::span_agent_secret_id(agent_secret_id))
            .await
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        self.repo
            .get_for_environment(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn get_for_environment_and_path(
        &self,
        environment_id: Uuid,
        path: Vec<String>,
    ) -> Result<Option<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        self.repo
            .get_for_environment_and_path(environment_id, path)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn get_for_agent_owner(
        &self,
        environment_id: Uuid,
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        self.repo
            .get_for_agent_owner(environment_id, agent_id, fingerprint)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn get_for_agent_type(
        &self,
        environment_id: Uuid,
        type_id: TypeId,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        self.repo
            .get_for_agent_type(environment_id, type_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn get_revision(
        &self,
        environment_id: Uuid,
        owner: &AgentSecretOwner,
        agent_secret_id: AgentSecretId,
        path: Vec<String>,
        selector: AgentSecretRevisionSelector,
        mode: AgentSecretReadMode,
    ) -> Result<Option<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        self.repo
            .get_revision(environment_id, owner, agent_secret_id, path, selector, mode)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }
}

pub struct DbAgentSecretRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "agent-secret";

impl<DBP: Pool> DbAgentSecretRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedAgentSecretRepo<Self>
    where
        Self: AgentSecretRepo,
    {
        LoggedAgentSecretRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbAgentSecretRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        revision: AgentSecretRevisionRecord,
    ) -> Result<AgentSecretRevisionRecord, AgentSecretRepoError> {
        if matches!(&revision.destinations, AgentSecretDestinations::Set(destinations) if !destinations.is_empty())
        {
            let (owner_kind,): (String,) = tx
                .fetch_one_as(
                    sqlx::query_as(
                        "SELECT owner_kind FROM agent_secrets WHERE agent_secret_id = $1",
                    )
                    .bind(revision.agent_secret_id),
                )
                .await?;
            if owner_kind != "environment" {
                return Err(AgentSecretRepoError::DestinationsRequireEnvironmentOwner);
            }
        }
        let destinations = if revision.audit.deleted {
            AgentSecretDestinations::Set(vec![])
        } else {
            revision.destinations
        };
        let query = match destinations {
            AgentSecretDestinations::Copy => sqlx::query_as(indoc! { r#"
                INSERT INTO agent_secret_revisions
                (agent_secret_id, revision_id, agent_secret_revision_data, created_at, created_by, deleted, destinations)
                SELECT $1, $2, $3, $4, $5, $6, cur.destinations
                FROM agent_secrets sec
                JOIN agent_secret_revisions cur ON cur.agent_secret_id = sec.agent_secret_id
                    AND cur.revision_id = sec.current_revision_id
                WHERE sec.agent_secret_id = $1
                RETURNING agent_secret_id, revision_id, agent_secret_revision_data, created_at, created_by, deleted, destinations
            "# }).bind(revision.agent_secret_id)
                .bind(revision.revision_id)
                .bind(revision.agent_secret_revision_data)
                .bind_deletable_revision_audit(revision.audit),
            AgentSecretDestinations::Set(destinations) => {
                let mut canonical = destinations.into_iter()
                    .map(|destination| destination.to_string().parse().map_err(anyhow::Error::from))
                    .collect::<anyhow::Result<Vec<golem_common::model::agent_secret::SecretDestination>>>()?;
                canonical.sort_by_cached_key(ToString::to_string);
                canonical.dedup();
                sqlx::query_as(indoc! { r#"
                    INSERT INTO agent_secret_revisions
                    (agent_secret_id, revision_id, agent_secret_revision_data, created_at, created_by, deleted, destinations)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    RETURNING agent_secret_id, revision_id, agent_secret_revision_data, created_at, created_by, deleted, destinations
                "# }).bind(revision.agent_secret_id)
                    .bind(revision.revision_id)
                    .bind(revision.agent_secret_revision_data)
                    .bind_deletable_revision_audit(revision.audit)
                    .bind(sqlx::types::Json(canonical))
            }
        };
        let revision: AgentSecretRevisionRecord = tx
            .fetch_one_as(query)
            .await
            .to_error_on_unique_violation(AgentSecretRepoError::ConcurrentModification)?;

        Ok(revision)
    }

    pub async fn create_within_transaction(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        record: AgentSecretCreationRecord,
    ) -> Result<AgentSecretExtRevisionRecord, AgentSecretRepoError> {
        let owner = AgentSecretOwner::try_from(&record.owner)?;
        if owner != AgentSecretOwner::Environment
            && matches!(&record.revision.destinations, AgentSecretDestinations::Set(destinations) if !destinations.is_empty())
        {
            return Err(AgentSecretRepoError::DestinationsRequireEnvironmentOwner);
        }
        let agent_secret_record: AgentSecretRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! {r#"
                    INSERT INTO agent_secrets (agent_secret_id, environment_id, path, agent_secret_data, created_at, updated_at, deleted_at, modified_by, current_revision_id, owner_kind, agent_id, agent_fingerprint, agent_type, tool_name)
                    VALUES ($1, $2, $3, $4, $5, $5, NULL, $6, $7, $8, $9, $10, $11, $12)
                    RETURNING agent_secret_id, environment_id, path, agent_secret_data, created_at, updated_at, deleted_at, modified_by, current_revision_id, owner_kind, agent_id, agent_fingerprint, agent_type, tool_name
                "#})
                    .bind(record.revision.agent_secret_id)
                    .bind(record.environment_id)
                    .bind(&record.path)
                    .bind(&record.agent_secret_data)
                    .bind(&record.revision.audit.created_at)
                    .bind(record.revision.audit.created_by)
                    .bind(record.revision.revision_id)
                    .bind(record.owner.owner_kind)
                    .bind(record.owner.agent_id)
                    .bind(record.owner.agent_fingerprint)
                    .bind(record.owner.agent_type)
                    .bind(record.owner.tool_name)
            )
            .await
            .to_error_on_unique_violation(AgentSecretRepoError::SecretViolatesUniqueness)?;

        let revision = Self::insert_revision(tx, record.revision).await?;

        if owner == AgentSecretOwner::Environment {
            let change_event = NewRegistryChangeEvent::agent_secret_changed(record.environment_id);
            DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                .await?;
        }

        Ok(AgentSecretExtRevisionRecord {
            environment_id: agent_secret_record.environment_id,
            path: agent_secret_record.path,
            owner: agent_secret_record.owner,
            agent_secret_data: agent_secret_record.agent_secret_data,
            entity_created_at: agent_secret_record.audit.created_at,
            revision,
        })
    }

    pub async fn update_within_transaction(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: AgentSecretRevisionRecord,
    ) -> Result<AgentSecretExtRevisionRecord, AgentSecretRepoError> {
        let revision = Self::insert_revision(tx, revision).await?;

        let agent_secret_record: AgentSecretRecord = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    UPDATE agent_secrets
                    SET updated_at = $1, modified_by = $2, current_revision_id = $3
                    WHERE agent_secret_id = $4
                    RETURNING agent_secret_id, environment_id, path, agent_secret_data, created_at, updated_at, deleted_at, modified_by, current_revision_id, owner_kind, agent_id, agent_fingerprint, agent_type, tool_name
                "#})
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.agent_secret_id)
            ).await?
            .ok_or(AgentSecretRepoError::ConcurrentModification)?;

        if AgentSecretOwner::try_from(&agent_secret_record.owner)? == AgentSecretOwner::Environment
        {
            let change_event =
                NewRegistryChangeEvent::agent_secret_changed(agent_secret_record.environment_id);
            DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                .await?;
        }

        Ok(AgentSecretExtRevisionRecord {
            environment_id: agent_secret_record.environment_id,
            path: agent_secret_record.path,
            owner: agent_secret_record.owner,
            agent_secret_data: agent_secret_record.agent_secret_data,
            entity_created_at: agent_secret_record.audit.created_at,
            revision,
        })
    }

    pub async fn delete_within_transaction(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: AgentSecretRevisionRecord,
    ) -> Result<AgentSecretExtRevisionRecord, AgentSecretRepoError> {
        let revision = Self::insert_revision(tx, revision.clone()).await?;

        let agent_secret_record: AgentSecretRecord = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    UPDATE agent_secrets
                    SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3
                    WHERE agent_secret_id = $4
                    RETURNING agent_secret_id, environment_id, path, agent_secret_data, created_at, updated_at, deleted_at, modified_by, current_revision_id, owner_kind, agent_id, agent_fingerprint, agent_type, tool_name
                "#})
                .bind(&revision.audit.created_at)
                .bind(revision.audit.created_by)
                .bind(revision.revision_id)
                .bind(revision.agent_secret_id),
            )
            .await?
            .ok_or(AgentSecretRepoError::ConcurrentModification)?;

        if AgentSecretOwner::try_from(&agent_secret_record.owner)? == AgentSecretOwner::Environment
        {
            let change_event =
                NewRegistryChangeEvent::agent_secret_changed(agent_secret_record.environment_id);
            DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                .await?;
        }

        Ok(AgentSecretExtRevisionRecord {
            environment_id: agent_secret_record.environment_id,
            path: agent_secret_record.path,
            owner: agent_secret_record.owner,
            agent_secret_data: agent_secret_record.agent_secret_data,
            entity_created_at: agent_secret_record.audit.created_at,
            revision,
        })
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
#[golem_async_trait::region_simple]
impl AgentSecretRepo for DbAgentSecretRepo<PostgresPool> {
    async fn create(
        &self,
        record: AgentSecretCreationRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                Self::create_within_transaction(tx, record).boxed()
            })
            .await
            .map(RequiresSignalExt::requires_notification_signal)
    }

    async fn update(
        &self,
        revision: AgentSecretRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "update", |tx| {
                Self::update_within_transaction(tx, revision).boxed()
            })
            .await
            .map(RequiresSignalExt::requires_notification_signal)
    }

    async fn delete(
        &self,
        revision: AgentSecretRevisionRecord,
    ) -> Result<RequiresNotificationSignal<AgentSecretExtRevisionRecord>, AgentSecretRepoError>
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "update", |tx| {
                Self::delete_within_transaction(tx, revision).boxed()
            })
            .await
            .map(RequiresSignalExt::requires_notification_signal)
    }

    async fn get_by_id(
        &self,
        agent_secret_id: Uuid,
    ) -> Result<Option<AgentSecretAuthExtRevisionRecord>, AgentSecretRepoError> {
        let result: Option<AgentSecretAuthExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                        sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name,
                        er.name AS environment_name,
                        ap.name AS application_name,
                        a.email AS owner_account_email
                    FROM agent_secrets sec
                    JOIN environments e ON e.environment_id = sec.environment_id
                    JOIN environment_revisions er
                        ON er.environment_id = e.environment_id
                        AND er.revision_id = e.current_revision_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = sec.current_revision_id
                    WHERE sec.agent_secret_id = $1
                        AND sec.deleted_at IS NULL
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
                "#})
                    .bind(agent_secret_id),
            )
            .await?;

        Ok(result)
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        let results: Vec<AgentSecretExtRevisionRecord> = self.with_ro("get_for_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                        sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name
                    FROM agent_secrets sec
                    JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = sec.current_revision_id
                    WHERE sec.environment_id = $1 AND sec.deleted_at IS NULL AND sec.owner_kind = 'environment'
                "#})
                    .bind(environment_id),
            )
            .await?;

        Ok(results)
    }

    async fn get_for_environment_and_path(
        &self,
        environment_id: Uuid,
        path: Vec<String>,
    ) -> Result<Option<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        Ok(self.with_ro("get_for_environment_and_path").fetch_optional_as(sqlx::query_as(indoc! {r#"
            SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name
            FROM agent_secrets sec
            JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = sec.current_revision_id
            WHERE sec.environment_id = $1 AND sec.path = $2 AND sec.deleted_at IS NULL AND sec.owner_kind = 'environment'
        "#}).bind(environment_id).bind(sqlx::types::Json(path))).await?)
    }

    async fn get_for_agent_owner(
        &self,
        environment_id: Uuid,
        agent_id: AgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        Ok(self.with_ro("get_for_agent_owner").fetch_all_as(sqlx::query_as(indoc! {r#"
            SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name
            FROM agent_secrets sec
            JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = sec.current_revision_id
            JOIN environments e ON e.environment_id = sec.environment_id
            JOIN applications ap ON ap.application_id = e.application_id
            JOIN accounts a ON a.account_id = ap.account_id
            WHERE sec.environment_id = $1 AND sec.agent_id = $2 AND sec.agent_fingerprint = $3
                AND sec.owner_kind = 'agent' AND sec.deleted_at IS NULL
                AND e.deleted_at IS NULL AND ap.deleted_at IS NULL AND a.deleted_at IS NULL
        "#}).bind(environment_id).bind(agent_id.0).bind(fingerprint.0)).await?)
    }

    async fn get_for_agent_type(
        &self,
        environment_id: Uuid,
        type_id: TypeId,
    ) -> Result<Vec<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        Ok(self.with_ro("get_for_agent_type").fetch_all_as(sqlx::query_as(indoc! {r#"
            SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name
            FROM agent_secrets sec
            JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = sec.current_revision_id
            JOIN environments e ON e.environment_id = sec.environment_id
            JOIN applications ap ON ap.application_id = e.application_id
            JOIN accounts a ON a.account_id = ap.account_id
            WHERE sec.environment_id = $1 AND sec.agent_type = $2
                AND sec.owner_kind = 'agent' AND sec.deleted_at IS NULL
                AND e.deleted_at IS NULL AND ap.deleted_at IS NULL AND a.deleted_at IS NULL
        "#}).bind(environment_id).bind(i64::from(type_id.get()))).await?)
    }

    async fn get_revision(
        &self,
        environment_id: Uuid,
        owner: &AgentSecretOwner,
        agent_secret_id: AgentSecretId,
        path: Vec<String>,
        selector: AgentSecretRevisionSelector,
        mode: AgentSecretReadMode,
    ) -> Result<Option<AgentSecretExtRevisionRecord>, AgentSecretRepoError> {
        let revision_id: Option<i64> = match selector {
            AgentSecretRevisionSelector::Current => None,
            AgentSecretRevisionSelector::Fixed(revision) => Some(revision.into()),
        };

        let result: Option<AgentSecretExtRevisionRecord> = if mode
            == AgentSecretReadMode::Historical
        {
            self.with_ro("get_revision")
                .fetch_optional_as(
                    sqlx::query_as(indoc! {r#"
                        SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                            sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name
                        FROM agent_secrets sec
                        JOIN environments e ON e.environment_id = sec.environment_id
                        JOIN applications ap ON ap.application_id = e.application_id
                        JOIN accounts a ON a.account_id = ap.account_id
                        JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = COALESCE($4, sec.current_revision_id)
                        WHERE sec.environment_id = $1 AND sec.agent_secret_id = $2 AND sec.path = $3 AND secr.deleted = FALSE
                    "#})
                        .bind(environment_id)
                        .bind(agent_secret_id.0)
                        .bind(sqlx::types::Json(path))
                        .bind(revision_id),
                )
                .await?
        } else {
            self.with_ro("get_revision")
                .fetch_optional_as(
                    sqlx::query_as(indoc! {r#"
                        SELECT sec.environment_id, sec.path, sec.agent_secret_data, sec.created_at AS entity_created_at, secr.agent_secret_id, secr.revision_id, secr.agent_secret_revision_data, secr.created_at, secr.created_by, secr.deleted, secr.destinations,
                            sec.owner_kind, sec.agent_id, sec.agent_fingerprint, sec.agent_type, sec.tool_name
                        FROM agent_secrets sec
                        JOIN environments e ON e.environment_id = sec.environment_id
                        JOIN applications ap ON ap.application_id = e.application_id
                        JOIN accounts a ON a.account_id = ap.account_id
                        JOIN agent_secret_revisions secr ON secr.agent_secret_id = sec.agent_secret_id AND secr.revision_id = COALESCE($4, sec.current_revision_id)
                        WHERE sec.environment_id = $1 AND sec.agent_secret_id = $2 AND sec.path = $3 AND secr.deleted = FALSE
                            AND sec.deleted_at IS NULL
                            AND e.deleted_at IS NULL
                            AND ap.deleted_at IS NULL
                            AND a.deleted_at IS NULL
                    "#})
                        .bind(environment_id)
                        .bind(agent_secret_id.0)
                        .bind(sqlx::types::Json(path))
                        .bind(revision_id),
                )
                .await?
        };

        match result {
            Some(record) if AgentSecretOwner::try_from(&record.owner)? == *owner => {
                Ok(Some(record))
            }
            _ => Ok(None),
        }
    }
}
