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

use crate::repo::model::application::{ApplicationRecord, ApplicationRevisionRecord};
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use crate::repo::model::{new_repo_uuid, BindFields};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use futures::FutureExt;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::Database;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[async_trait]
pub trait ApplicationRepo: Send + Sync {
    async fn get_by_name(
        &self,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ApplicationRecord>>;

    async fn get_by_id(&self, application_id: &Uuid) -> repo::Result<Option<ApplicationRecord>>;

    async fn get_all_by_owner(
        &self,
        owner_account_id: &Uuid,
    ) -> repo::Result<Vec<ApplicationRecord>>;

    async fn get_revisions(
        &self,
        application_id: &Uuid,
    ) -> repo::Result<Vec<ApplicationRevisionRecord>>;

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<ApplicationRecord>;

    async fn delete(&self, user_account_id: &Uuid, application_id: &Uuid) -> repo::Result<()>;
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

    fn span_app_id(application_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, application_id=%application_id)
    }

    fn span_owner_id(owner_account_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, owner_account_id=%owner_account_id)
    }
}

#[async_trait]
impl<Repo: ApplicationRepo> ApplicationRepo for LoggedApplicationRepo<Repo> {
    async fn get_by_name(
        &self,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ApplicationRecord>> {
        self.repo
            .get_by_name(owner_account_id, name)
            .instrument(Self::span_name(name))
            .await
    }

    async fn get_by_id(&self, application_id: &Uuid) -> repo::Result<Option<ApplicationRecord>> {
        self.repo
            .get_by_id(application_id)
            .instrument(Self::span_app_id(application_id))
            .await
    }

    async fn get_all_by_owner(
        &self,
        owner_account_id: &Uuid,
    ) -> repo::Result<Vec<ApplicationRecord>> {
        self.repo
            .get_all_by_owner(owner_account_id)
            .instrument(Self::span_owner_id(owner_account_id))
            .await
    }

    async fn get_revisions(
        &self,
        application_id: &Uuid,
    ) -> repo::Result<Vec<ApplicationRevisionRecord>> {
        self.repo
            .get_revisions(application_id)
            .instrument(Self::span_app_id(application_id))
            .await
    }

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<ApplicationRecord> {
        self.repo
            .ensure(user_account_id, owner_account_id, name)
            .instrument(Self::span_name(name))
            .await
    }

    async fn delete(&self, user_account_id: &Uuid, application_id: &Uuid) -> repo::Result<()> {
        self.repo
            .delete(user_account_id, application_id)
            .instrument(Self::span_app_id(application_id))
            .await
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

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> repo::Result<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, RepoError>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ApplicationRepo for DbApplicationRepo<PostgresPool> {
    async fn get_by_name(
        &self,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ApplicationRecord>> {
        self.with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT application_id, name, account_id, created_at, updated_at, deleted_at, modified_by
                    FROM applications
                    WHERE account_id = $1 AND name = $2 AND deleted_at IS NULL
                "#})
                    .bind(owner_account_id)
                    .bind(name),
            )
            .await
    }

    async fn get_by_id(&self, application_id: &Uuid) -> repo::Result<Option<ApplicationRecord>> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT application_id, name, account_id, created_at, updated_at, deleted_at, modified_by
                    FROM applications
                    WHERE application_id = $1 AND deleted_at IS NULL
                "#})
                    .bind(application_id),
            )
            .await
    }

    async fn get_all_by_owner(
        &self,
        owner_account_id: &Uuid,
    ) -> repo::Result<Vec<ApplicationRecord>> {
        self.with_ro("get_all_by_owner")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT application_id, name, account_id, created_at, updated_at, deleted_at, modified_by
                    FROM applications
                    WHERE account_id = $1 AND deleted_at IS NULL
                    ORDER BY name
                "#})
                    .bind(owner_account_id),
            ).await
    }

    async fn get_revisions(
        &self,
        application_id: &Uuid,
    ) -> repo::Result<Vec<ApplicationRevisionRecord>> {
        self.with_ro("get_revisions")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT application_id, revision_id, name, account_id, created_at, created_by, deleted
                    FROM application_revisions
                    WHERE application_id = $1 AND deleted = false
                    ORDER BY revision_id DESC
                "#})
                    .bind(application_id),
            ).await
    }

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        owner_account_id: &Uuid,
        name: &str,
    ) -> repo::Result<ApplicationRecord> {
        if let Some(app) = self.get_by_name(owner_account_id, name).await? {
            return Ok(app);
        }

        let result: repo::Result<ApplicationRecord> = {
            let user_id = *user_account_id;
            let owner_id = *owner_account_id;
            let name = name.to_owned();

            self.with_tx("ensure", |tx| async move {
                let app: ApplicationRecord = tx.fetch_one_as(
                    sqlx::query_as(indoc! {r#"
                        INSERT INTO applications (application_id, name, account_id, created_at, updated_at, deleted_at, modified_by)
                        VALUES ($1, $2, $3, $4, $5, $6, $7)
                        RETURNING application_id, name, account_id, created_at, updated_at, deleted_at, modified_by
                    "#})
                        .bind(new_repo_uuid())
                        .bind(&name)
                        .bind(owner_id)
                        .bind_audit(AuditFields::new(user_id))
                ).await?;

                Self::insert_revision(
                    tx,
                    ApplicationRevisionRecord {
                        application_id: app.application_id,
                        revision_id: 0,
                        name: app.name.clone(),
                        account_id: app.account_id,
                        audit: DeletableRevisionAuditFields::new(user_id),
                    },
                ).await?;

                Ok(app)
            }.boxed()).await
        };

        let result = match result {
            Err(err) if err.is_unique_violation() => None,
            result => Some(result),
        };
        if let Some(result) = result {
            return result;
        }

        match self.get_by_name(owner_account_id, name).await? {
            Some(app) => Ok(app),
            None => Err(RepoError::Internal(
                "illegal state: missing application".to_string(),
            )),
        }
    }

    async fn delete(&self, user_account_id: &Uuid, application_id: &Uuid) -> repo::Result<()> {
        let application_id = *application_id;
        let user_account_id = *user_account_id;
        let result = self.with_tx("delete", |tx| async move {
            let latest_revision: Option<ApplicationRevisionRecord> =
                tx.fetch_optional_as(
                    sqlx::query_as(indoc! {r#"
                        SELECT application_id, revision_id, name, account_id, created_at, created_by, deleted
                        FROM application_revisions
                        WHERE application_id = $1
                        ORDER BY revision_id DESC
                        LIMIT 1
                    "#})
                        .bind(application_id)
                )
                    .await?;
            let Some(latest_revision) = latest_revision else {
                return Ok(());
            };
            if latest_revision.audit.deleted {
                return Ok(());
            }

            let revision = ApplicationRevisionRecord {
                application_id: latest_revision.application_id,
                revision_id: latest_revision.revision_id + 1,
                name: latest_revision.name,
                account_id: latest_revision.account_id,
                audit: DeletableRevisionAuditFields::deletion(user_account_id),
            };
            let deleted_at = revision.audit.created_at.clone();

            Self::insert_revision(tx, revision).await?;

            tx.execute(
                sqlx::query(indoc! {r#"
                    UPDATE applications
                    SET deleted_at = $1, modified_by = $2
                    WHERE application_id = $3
                "#})
                    .bind(deleted_at)
                    .bind(user_account_id)
                    .bind(application_id)
            ).await?;

            Ok(())
        }.boxed()).await;

        match result {
            Ok(()) => Ok(()),
            Err(err) if err.is_unique_violation() => Ok(()),
            Err(err) => Err(err),
        }
    }
}

#[async_trait]
trait ApplicationRepoInternal: ApplicationRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ApplicationRevisionRecord,
    ) -> repo::Result<()>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ApplicationRepoInternal for DbApplicationRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ApplicationRevisionRecord,
    ) -> repo::Result<()> {
        tx.execute(
            sqlx::query(indoc! {r#"
                INSERT INTO application_revisions (application_id, revision_id, name, account_id, created_at, created_by, deleted)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#})
                .bind(revision.application_id)
                .bind(revision.revision_id)
                .bind(revision.name)
                .bind(revision.account_id)
                .bind_deletable_revision_audit(revision.audit)
        ).await?;

        Ok(())
    }
}
