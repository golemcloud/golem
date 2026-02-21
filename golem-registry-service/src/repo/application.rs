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

use super::model::application::{ApplicationExtRevisionRecord, ApplicationRepoError};
use crate::repo::model::BindFields;
use crate::repo::model::application::{ApplicationRecord, ApplicationRevisionRecord};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{RepoError, ResultExt};
use indoc::indoc;
use sqlx::Database;
use std::fmt::Debug;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait ApplicationRepo: Send + Sync {
    async fn get_by_name(
        &self,
        owner_account_id: Uuid,
        name: &str,
    ) -> Result<Option<ApplicationExtRevisionRecord>, ApplicationRepoError>;

    async fn get_by_id(
        &self,
        application_id: Uuid,
    ) -> Result<Option<ApplicationExtRevisionRecord>, ApplicationRepoError>;

    async fn list_by_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<ApplicationExtRevisionRecord>, ApplicationRepoError>;

    async fn create(
        &self,
        account_id: Uuid,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError>;

    async fn update(
        &self,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError>;

    async fn delete(
        &self,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError>;
}

pub struct LoggedApplicationRepo<Repo: ApplicationRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "application repository";

impl<Repo: ApplicationRepo> LoggedApplicationRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(application_name: &str) -> Span {
        info_span!(SPAN_NAME, application_name)
    }

    fn span_app_id(application_id: Uuid) -> Span {
        info_span!(SPAN_NAME, application_id=%application_id)
    }

    fn span_owner_id(owner_account_id: Uuid) -> Span {
        info_span!(SPAN_NAME, owner_account_id=%owner_account_id)
    }
}

#[async_trait]
impl<Repo: ApplicationRepo> ApplicationRepo for LoggedApplicationRepo<Repo> {
    async fn get_by_name(
        &self,
        owner_account_id: Uuid,
        name: &str,
    ) -> Result<Option<ApplicationExtRevisionRecord>, ApplicationRepoError> {
        self.repo
            .get_by_name(owner_account_id, name)
            .instrument(Self::span_name(name))
            .await
    }

    async fn get_by_id(
        &self,
        application_id: Uuid,
    ) -> Result<Option<ApplicationExtRevisionRecord>, ApplicationRepoError> {
        self.repo
            .get_by_id(application_id)
            .instrument(Self::span_app_id(application_id))
            .await
    }

    async fn list_by_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<ApplicationExtRevisionRecord>, ApplicationRepoError> {
        self.repo
            .list_by_owner(owner_account_id)
            .instrument(Self::span_owner_id(owner_account_id))
            .await
    }

    async fn create(
        &self,
        account_id: Uuid,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError> {
        self.repo
            .create(account_id, revision)
            .instrument(Self::span_owner_id(account_id))
            .await
    }

    async fn update(
        &self,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError> {
        let span = Self::span_app_id(revision.application_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError> {
        let span = Self::span_app_id(revision.application_id);
        self.repo.delete(revision).instrument(span).await
    }
}

pub struct DbApplicationRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "application";

impl<DBP: Pool> DbApplicationRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedApplicationRepo<Self>
    where
        Self: ApplicationRepo,
    {
        LoggedApplicationRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx_err<R, E, F>(&self, api_name: &'static str, f: F) -> Result<R, E>
    where
        R: Send,
        E: Debug + Send + From<RepoError>,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, E>>
            + Send,
    {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, api_name, f)
            .await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ApplicationRepo for DbApplicationRepo<PostgresPool> {
    async fn get_by_name(
        &self,
        owner_account_id: Uuid,
        name: &str,
    ) -> Result<Option<ApplicationExtRevisionRecord>, ApplicationRepoError> {
        let result: Option<ApplicationExtRevisionRecord> = self
            .with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        ap.account_id,
                        ap.created_at as entity_created_at,
                        r.application_id, r.revision_id, r.name,
                        r.created_at, r.created_by, r.deleted
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN application_revisions r
                        ON r.application_id = ap.application_id
                        AND r.revision_id = ap.current_revision_id
                    WHERE
                        a.account_id = $1
                        AND ap.name = $2
                        AND a.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                "#})
                .bind(owner_account_id)
                .bind(name),
            )
            .await?;

        Ok(result)
    }

    async fn get_by_id(
        &self,
        application_id: Uuid,
    ) -> Result<Option<ApplicationExtRevisionRecord>, ApplicationRepoError> {
        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        ap.account_id,
                        ap.created_at as entity_created_at,
                        r.application_id, r.revision_id, r.name,
                        r.created_at, r.created_by, r.deleted
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN application_revisions r
                        ON r.application_id = ap.application_id
                        AND r.revision_id = ap.current_revision_id
                    WHERE
                        ap.application_id = $1
                        AND a.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                "#})
                .bind(application_id),
            )
            .await?;

        Ok(result)
    }

    async fn list_by_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<ApplicationExtRevisionRecord>, ApplicationRepoError> {
        let result = self
            .with_ro("list_by_owner")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        ap.account_id,
                        ap.created_at as entity_created_at,
                        r.application_id, r.revision_id, r.name,
                        r.created_at, r.created_by, r.deleted
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN application_revisions r
                        ON r.application_id = ap.application_id
                        AND r.revision_id = ap.current_revision_id
                    WHERE
                        a.account_id = $1
                        AND a.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                    ORDER BY r.name
                "#})
                .bind(owner_account_id),
            )
            .await?;

        Ok(result)
    }

    async fn create(
        &self,
        account_id: Uuid,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError> {
        self.with_tx_err("create", |tx| async move {
            tx.execute(
                sqlx::query(indoc! { r#"
                    INSERT INTO applications
                    (application_id, name, account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                    VALUES ($1, $2, $3, $4, $4, NULL, $5, 0)
                    RETURNING application_id, name, account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                "# })
                    .bind(revision.application_id)
                    .bind(&revision.name)
                    .bind(account_id)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
            ).await
            .to_error_on_unique_violation(ApplicationRepoError::ApplicationViolatesUniqueness)?;

            let revision = Self::insert_revision(tx, revision).await?;

            Ok(ApplicationExtRevisionRecord {
                account_id,
                entity_created_at: revision.audit.created_at.clone(),
                revision,
            })
        }.boxed()).await
    }

    async fn update(
        &self,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let revision = Self::insert_revision(tx, revision).await?;

                let application_record: ApplicationRecord = tx.fetch_optional_as(
                    sqlx::query_as(indoc! { r#"
                        UPDATE applications
                        SET name = $1, updated_at = $2, modified_by = $3, current_revision_id = $4
                        WHERE application_id = $5
                        RETURNING application_id, name, account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                    "#})
                    .bind(&revision.name)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.application_id)
                )
                .await
                .to_error_on_unique_violation(ApplicationRepoError::ApplicationViolatesUniqueness)?
                .ok_or(ApplicationRepoError::ConcurrentModification)?;

                Ok(ApplicationExtRevisionRecord {
                    account_id: application_record.account_id,
                    entity_created_at: application_record.audit.created_at,
                    revision,
                })
            }
            .boxed()
        })
        .await
    }

    async fn delete(
        &self,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationExtRevisionRecord, ApplicationRepoError> {
        self.with_tx_err("delete", |tx| {
            async move {
                let revision = Self::insert_revision(tx, revision).await?;

                let application_record: ApplicationRecord = tx.fetch_optional_as(
                    sqlx::query_as(indoc! { r#"
                        UPDATE applications
                        SET name = $1, updated_at = $2, deleted_at = $2, modified_by = $3, current_revision_id = $4
                        WHERE application_id = $5
                        RETURNING application_id, name, account_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                    "#})
                    .bind(&revision.name)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.application_id)
                )
                .await?
                .ok_or(ApplicationRepoError::ConcurrentModification)?;

                Ok(ApplicationExtRevisionRecord {
                    account_id: application_record.account_id,
                    entity_created_at: application_record.audit.created_at,
                    revision,
                })
            }
            .boxed()
        })
        .await
    }
}

#[async_trait]
trait ApplicationRepoInternal: ApplicationRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationRevisionRecord, ApplicationRepoError>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ApplicationRepoInternal for DbApplicationRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ApplicationRevisionRecord,
    ) -> Result<ApplicationRevisionRecord, ApplicationRepoError> {
        let revision = tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO application_revisions (application_id, revision_id, name, created_at, created_by, deleted)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING application_id, revision_id, name, created_at, created_by, deleted
            "#})
                .bind(revision.application_id)
                .bind(revision.revision_id)
                .bind(revision.name)
                .bind_deletable_revision_audit(revision.audit)
        ).await
        .to_error_on_unique_violation(ApplicationRepoError::ConcurrentModification)?;

        Ok(revision)
    }
}
