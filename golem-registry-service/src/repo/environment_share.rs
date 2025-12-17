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

use super::model::environment_share::{
    EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError, EnvironmentShareRevisionRecord,
};
use crate::repo::model::BindFields;
pub use crate::repo::model::account::AccountRecord;
use crate::repo::model::environment_share::EnvironmentShareRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait EnvironmentShareRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: Uuid,
        revision: EnvironmentShareRevisionRecord,
        grantee_account_id: Uuid,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError>;

    async fn update(
        &self,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError>;

    async fn delete(
        &self,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError>;

    async fn get_by_id(
        &self,
        environment_share_id: Uuid,
    ) -> Result<Option<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError>;

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError>;

    async fn get_for_environment_and_grantee(
        &self,
        environment_id: Uuid,
        grantee_account_id: Uuid,
    ) -> Result<Option<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError>;
}

pub struct LoggedEnvironmentShareRepo<Repo: EnvironmentShareRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "environment share repository";

impl<Repo: EnvironmentShareRepo> LoggedEnvironmentShareRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_environment_id(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id=%environment_id)
    }

    fn span_environment_share_id(environment_share_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_share_id=%environment_share_id)
    }
}

#[async_trait]
impl<Repo: EnvironmentShareRepo> EnvironmentShareRepo for LoggedEnvironmentShareRepo<Repo> {
    async fn create(
        &self,
        environment_id: Uuid,
        revision: EnvironmentShareRevisionRecord,
        grantee_account_id: Uuid,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError> {
        let span = Self::span_environment_id(environment_id);
        self.repo
            .create(environment_id, revision, grantee_account_id)
            .instrument(span)
            .await
    }

    async fn update(
        &self,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError> {
        let span = Self::span_environment_share_id(revision.environment_share_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError> {
        let span = Self::span_environment_share_id(revision.environment_share_id);
        self.repo.delete(revision).instrument(span).await
    }

    async fn get_by_id(
        &self,
        environment_share_id: Uuid,
    ) -> Result<Option<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError> {
        self.repo
            .get_by_id(environment_share_id)
            .instrument(Self::span_environment_share_id(environment_share_id))
            .await
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError> {
        self.repo
            .get_for_environment(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn get_for_environment_and_grantee(
        &self,
        environment_id: Uuid,
        grantee_account_id: Uuid,
    ) -> Result<Option<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError> {
        self.repo
            .get_for_environment_and_grantee(environment_id, grantee_account_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }
}

pub struct DbEnvironmentShareRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment-share";

impl<DBP: Pool> DbEnvironmentShareRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedEnvironmentShareRepo<Self>
    where
        Self: EnvironmentShareRepo,
    {
        LoggedEnvironmentShareRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbEnvironmentShareRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareRevisionRecord, EnvironmentShareRepoError> {
        let revision: EnvironmentShareRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO environment_share_revisions
                    (environment_share_id, revision_id, roles, created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING environment_share_id, revision_id, roles, created_at, created_by, deleted
                "# })
                .bind(revision.environment_share_id)
                .bind(revision.revision_id)
                .bind(revision.roles)
                .bind_deletable_revision_audit(revision.audit),
            )
            .await
            .to_error_on_unique_violation(EnvironmentShareRepoError::ConcurrentModification)?;

        Ok(revision)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentShareRepo for DbEnvironmentShareRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: Uuid,
        revision: EnvironmentShareRevisionRecord,
        grantee_account_id: Uuid,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "create", |tx| {
            async move {
                let environment_share_record: EnvironmentShareRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! {r#"
                            INSERT INTO environment_shares (environment_id, environment_share_id, grantee_account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                            VALUES ($1, $2, $3, $4, $4, NULL, $5, $6)
                            RETURNING environment_id, environment_share_id, grantee_account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(environment_id)
                            .bind(revision.environment_share_id)
                            .bind(grantee_account_id)
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                    )
                    .await
                    .to_error_on_unique_violation(EnvironmentShareRepoError::ShareViolatesUniqueness)?;

                let revision_record = Self::insert_revision(tx, revision).await?;

                Ok(EnvironmentShareExtRevisionRecord {
                    environment_id: environment_share_record.environment_id,
                    grantee_account_id: environment_share_record.grantee_account_id,
                    entity_created_at: environment_share_record.audit.created_at,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn update(
        &self,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision = Self::insert_revision(tx, revision).await?;

                let environment_share_record: EnvironmentShareRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE environment_shares
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE environment_share_id = $4
                            RETURNING environment_id, environment_share_id, grantee_account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.environment_share_id)
                    ).await?
                    .ok_or(EnvironmentShareRepoError::ConcurrentModification)?;

                Ok(EnvironmentShareExtRevisionRecord {
                    environment_id: environment_share_record.environment_id,
                    grantee_account_id: environment_share_record.grantee_account_id,
                    entity_created_at: environment_share_record.audit.created_at,
                    revision
                })
            }.boxed()
        }).await
    }

    async fn delete(
        &self,
        revision: EnvironmentShareRevisionRecord,
    ) -> Result<EnvironmentShareExtRevisionRecord, EnvironmentShareRepoError> {
        self.db_pool.with_tx_err(METRICS_SVC_NAME, "update", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision.clone()).await?;

                let environment_share_record: EnvironmentShareRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! {r#"
                            UPDATE environment_shares
                            SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE environment_share_id = $4
                            RETURNING environment_id, environment_share_id, grantee_account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(&revision.audit.created_at)
                            .bind(revision.audit.created_by)
                            .bind(revision.revision_id)
                            .bind(revision.environment_share_id)
                    ).await?
                    .ok_or(EnvironmentShareRepoError::ConcurrentModification)?;

                Ok(EnvironmentShareExtRevisionRecord {
                    environment_id: environment_share_record.environment_id,
                    grantee_account_id: environment_share_record.grantee_account_id,
                    entity_created_at: environment_share_record.audit.created_at,
                    revision: revision_record
                })
            }.boxed()
        }).await
    }

    async fn get_by_id(
        &self,
        environment_share_id: Uuid,
    ) -> Result<Option<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError> {
        let result: Option<EnvironmentShareExtRevisionRecord> = self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT es.environment_id, es.grantee_account_id, es.created_at AS entity_created_at, esr.environment_share_id, esr.revision_id, esr.roles, esr.created_at, esr.created_by, esr.deleted
                    FROM environment_shares es
                    JOIN environment_share_revisions esr ON esr.environment_share_id = es.environment_share_id AND esr.revision_id = es.current_revision_id
                    WHERE es.environment_share_id = $1 AND es.deleted_at IS NULL
                "#})
                    .bind(environment_share_id),
            )
            .await?;

        Ok(result)
    }

    async fn get_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError> {
        let results: Vec<EnvironmentShareExtRevisionRecord> = self.with_ro("get_for_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT es.environment_id, es.grantee_account_id, es.created_at AS entity_created_at, esr.environment_share_id, esr.revision_id, esr.roles, esr.created_at, esr.created_by, esr.deleted
                    FROM environment_shares es
                    JOIN environment_share_revisions esr ON esr.environment_share_id = es.environment_share_id AND esr.revision_id = es.current_revision_id
                    WHERE es.environment_id = $1 AND es.deleted_at IS NULL
                "#})
                    .bind(environment_id),
            )
            .await?;

        Ok(results)
    }

    async fn get_for_environment_and_grantee(
        &self,
        environment_id: Uuid,
        grantee_account_id: Uuid,
    ) -> Result<Option<EnvironmentShareExtRevisionRecord>, EnvironmentShareRepoError> {
        let result: Option<EnvironmentShareExtRevisionRecord> = self.with_ro("get_for_environment_and_grantee")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT es.environment_id, es.grantee_account_id, es.created_at AS entity_created_at, esr.environment_share_id, esr.revision_id, esr.roles, esr.created_at, esr.created_by, esr.deleted
                    FROM environment_shares es
                    JOIN environment_share_revisions esr ON esr.environment_share_id = es.environment_share_id AND esr.revision_id = es.current_revision_id
                    WHERE es.environment_id = $1 AND  es.grantee_account_id = $2 AND es.deleted_at IS NULL
                "#})
                .bind(environment_id)
                .bind(grantee_account_id)
            )
            .await?;

        Ok(result)
    }
}
