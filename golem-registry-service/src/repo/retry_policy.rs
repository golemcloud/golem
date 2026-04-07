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

use super::model::retry_policy::{
    RetryPolicyCreationRecord, RetryPolicyExtRevisionRecord, RetryPolicyRepoError,
    RetryPolicyRevisionRecord,
};
use super::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RequiresNotificationSignal, RequiresSignalExt,
};
use crate::repo::model::BindFields;
use crate::repo::model::retry_policy::RetryPolicyRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{PoolLabelledTransaction, ResultExt};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait RetryPolicyRepo: Send + Sync {
    async fn create(
        &self,
        record: RetryPolicyCreationRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>;

    async fn update(
        &self,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>;

    async fn delete(
        &self,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>;

    async fn get_by_id(
        &self,
        retry_policy_id: Uuid,
    ) -> Result<Option<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>;

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>;
}

pub struct LoggedRetryPolicyRepo<Repo: RetryPolicyRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "retry policy repository";

impl<Repo: RetryPolicyRepo> LoggedRetryPolicyRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_environment_id(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id=%environment_id)
    }

    fn span_retry_policy_id(retry_policy_id: Uuid) -> Span {
        info_span!(SPAN_NAME, retry_policy_id=%retry_policy_id)
    }
}

#[async_trait]
impl<Repo: RetryPolicyRepo> RetryPolicyRepo for LoggedRetryPolicyRepo<Repo> {
    async fn create(
        &self,
        record: RetryPolicyCreationRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>
    {
        let span = Self::span_environment_id(record.environment_id);
        self.repo.create(record).instrument(span).await
    }

    async fn update(
        &self,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>
    {
        let span = Self::span_retry_policy_id(revision.retry_policy_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>
    {
        let span = Self::span_retry_policy_id(revision.retry_policy_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        retry_policy_id: Uuid,
    ) -> Result<Option<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError> {
        self.repo
            .get_by_id(retry_policy_id)
            .instrument(Self::span_retry_policy_id(retry_policy_id))
            .await
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError> {
        self.repo
            .get_for_environment(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }
}

pub struct DbRetryPolicyRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "retry-policy";

impl<DBP: Pool> DbRetryPolicyRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedRetryPolicyRepo<Self>
    where
        Self: RetryPolicyRepo,
    {
        LoggedRetryPolicyRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbRetryPolicyRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RetryPolicyRevisionRecord, RetryPolicyRepoError> {
        let revision: RetryPolicyRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO retry_policy_revisions
                    (retry_policy_id, revision_id, priority, predicate_json, policy_json, created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    RETURNING retry_policy_id, revision_id, priority, predicate_json, policy_json, created_at, created_by, deleted
                "# })
                .bind(revision.retry_policy_id)
                .bind(revision.revision_id)
                .bind(revision.priority)
                .bind(revision.predicate_json)
                .bind(revision.policy_json)
                .bind_deletable_revision_audit(revision.audit),
            )
            .await
            .to_error_on_unique_violation(RetryPolicyRepoError::ConcurrentModification)?;

        Ok(revision)
    }

    pub async fn create_within_transaction(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        record: RetryPolicyCreationRecord,
    ) -> Result<RetryPolicyExtRevisionRecord, RetryPolicyRepoError> {
        let retry_policy_record: RetryPolicyRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! {r#"
                    INSERT INTO retry_policies (retry_policy_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                    VALUES ($1, $2, $3, $4, $4, NULL, $5, $6)
                    RETURNING retry_policy_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                "#})
                    .bind(record.revision.retry_policy_id)
                    .bind(record.environment_id)
                    .bind(&record.name)
                    .bind(&record.revision.audit.created_at)
                    .bind(record.revision.audit.created_by)
                    .bind(record.revision.revision_id)
            )
            .await
            .to_error_on_unique_violation(RetryPolicyRepoError::NameViolatesUniqueness)?;

        let revision = Self::insert_revision(tx, record.revision).await?;

        let change_event =
            NewRegistryChangeEvent::retry_policy_changed(retry_policy_record.environment_id);
        DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event).await?;

        Ok(RetryPolicyExtRevisionRecord {
            environment_id: retry_policy_record.environment_id,
            name: retry_policy_record.name,
            entity_created_at: retry_policy_record.audit.created_at,
            revision,
        })
    }

    pub async fn update_within_transaction(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RetryPolicyExtRevisionRecord, RetryPolicyRepoError> {
        let revision = Self::insert_revision(tx, revision).await?;

        let retry_policy_record: RetryPolicyRecord = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    UPDATE retry_policies
                    SET updated_at = $1, modified_by = $2, current_revision_id = $3
                    WHERE retry_policy_id = $4
                    RETURNING retry_policy_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                "#})
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.retry_policy_id)
            ).await?
            .ok_or(RetryPolicyRepoError::ConcurrentModification)?;

        let change_event =
            NewRegistryChangeEvent::retry_policy_changed(retry_policy_record.environment_id);
        DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event).await?;

        Ok(RetryPolicyExtRevisionRecord {
            environment_id: retry_policy_record.environment_id,
            name: retry_policy_record.name,
            entity_created_at: retry_policy_record.audit.created_at,
            revision,
        })
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl RetryPolicyRepo for DbRetryPolicyRepo<PostgresPool> {
    async fn create(
        &self,
        record: RetryPolicyCreationRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>
    {
        let result = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                Self::create_within_transaction(tx, record).boxed()
            })
            .await?;

        Ok(result.requires_notification_signal())
    }

    async fn update(
        &self,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>
    {
        let result = self
            .db_pool
            .with_tx_err(METRICS_SVC_NAME, "update", |tx| {
                Self::update_within_transaction(tx, revision).boxed()
            })
            .await?;

        Ok(result.requires_notification_signal())
    }

    async fn delete(
        &self,
        revision: RetryPolicyRevisionRecord,
    ) -> Result<RequiresNotificationSignal<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError>
    {
        let result = self.db_pool.with_tx_err(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision = Self::insert_revision(tx, revision.clone()).await?;

                let retry_policy_record: RetryPolicyRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE retry_policies
                            SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE retry_policy_id = $4
                            RETURNING retry_policy_id, environment_id, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.retry_policy_id)
                    ).await?
                    .ok_or(RetryPolicyRepoError::ConcurrentModification)?;

                let change_event =
                    NewRegistryChangeEvent::retry_policy_changed(retry_policy_record.environment_id);
                DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                    .await?;

                Ok::<_, RetryPolicyRepoError>(RetryPolicyExtRevisionRecord {
                    environment_id: retry_policy_record.environment_id,
                    name: retry_policy_record.name,
                    entity_created_at: retry_policy_record.audit.created_at,
                    revision
                })
            }.boxed()
        }).await?;

        Ok(result.requires_notification_signal())
    }

    async fn get_by_id(
        &self,
        retry_policy_id: Uuid,
    ) -> Result<Option<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError> {
        let result: Option<RetryPolicyExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT rp.environment_id, rp.name, rp.created_at AS entity_created_at, rev.retry_policy_id, rev.revision_id, rev.priority, rev.predicate_json, rev.policy_json, rev.created_at, rev.created_by, rev.deleted
                    FROM retry_policies rp
                    JOIN retry_policy_revisions rev ON rev.retry_policy_id = rp.retry_policy_id AND rev.revision_id = rp.current_revision_id
                    WHERE rp.retry_policy_id = $1 AND rp.deleted_at IS NULL
                "#})
                    .bind(retry_policy_id),
            )
            .await?;

        Ok(result)
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<RetryPolicyExtRevisionRecord>, RetryPolicyRepoError> {
        let results: Vec<RetryPolicyExtRevisionRecord> = self.with_ro("get_for_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT rp.environment_id, rp.name, rp.created_at AS entity_created_at, rev.retry_policy_id, rev.revision_id, rev.priority, rev.predicate_json, rev.policy_json, rev.created_at, rev.created_by, rev.deleted
                    FROM retry_policies rp
                    JOIN retry_policy_revisions rev ON rev.retry_policy_id = rp.retry_policy_id AND rev.revision_id = rp.current_revision_id
                    WHERE rp.environment_id = $1 AND rp.deleted_at IS NULL
                    ORDER BY rev.priority DESC, rp.name ASC
                "#})
                    .bind(environment_id),
            )
            .await?;

        Ok(results)
    }
}
