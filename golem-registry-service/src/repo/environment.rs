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

use crate::repo::model::BindFields;
pub use crate::repo::model::environment::{
    EnvironmentCurrentRevisionRecord, EnvironmentRecord, EnvironmentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base_next::db::postgres::PostgresPool;
use golem_service_base_next::db::sqlite::SqlitePool;
use golem_service_base_next::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base_next::repo;
use golem_service_base_next::repo::RepoError;
use indoc::indoc;
use sqlx::Database;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait EnvironmentRepo: Send + Sync {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>>;

    async fn get_by_id(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>>;

    async fn create(
        &self,
        application_id: &Uuid,
        name: &str,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>>;

    async fn update(
        &self,
        current_revision_id: i64,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>>;

    async fn delete(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool>;
}

pub struct LoggedEnvironmentRepo<Repo: EnvironmentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "environment repository";

impl<Repo: EnvironmentRepo> LoggedEnvironmentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(application_id: &Uuid, name: &str) -> Span {
        info_span!(SPAN_NAME, application_id = %application_id, name)
    }

    fn span_env_id(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_app_id(application_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, application_id = %application_id)
    }
}

#[async_trait]
impl<Repo: EnvironmentRepo> EnvironmentRepo for LoggedEnvironmentRepo<Repo> {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        self.repo
            .get_by_name(application_id, name)
            .instrument(Self::span_name(application_id, name))
            .await
    }

    async fn get_by_id(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        self.repo
            .get_by_id(environment_id)
            .instrument(Self::span_env_id(environment_id))
            .await
    }

    async fn create(
        &self,
        application_id: &Uuid,
        name: &str,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        self.repo
            .create(application_id, name, revision)
            .instrument(Self::span_app_id(application_id))
            .await
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        let span = Self::span_env_id(&revision.environment_id);
        self.repo
            .update(current_revision_id, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        let span = Self::span_env_id(user_account_id);
        self.repo
            .delete(user_account_id, environment_id, current_revision_id)
            .instrument(span)
            .await
    }
}

pub struct DbEnvironmentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment";

impl<DBP: Pool> DbEnvironmentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> Result<R, RepoError>
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
impl EnvironmentRepo for DbEnvironmentRepo<PostgresPool> {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        self.with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides
                    FROM environments e
                    INNER JOIN environment_revisions r
                        ON e.environment_id = r.environment_id AND e.current_revision_id = r.revision_id
                    WHERE e.application_id = $1 AND e.name = $2 AND e.deleted_at IS NULL
                "# })
                    .bind(application_id)
                    .bind(name),
            )
            .await
    }

    async fn get_by_id(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name,e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides
                    FROM environments e
                    INNER JOIN environment_revisions r
                        ON e.environment_id = r.environment_id AND e.current_revision_id = r.revision_id
                    WHERE e.environment_id = $1 AND e.deleted_at IS NULL
                "# })
                    .bind(environment_id),
            )
            .await
    }

    async fn create(
        &self,
        application_id: &Uuid,
        name: &str,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        let application_id = *application_id;
        let name = name.to_owned();
        let revision = revision.ensure_first();

        let result: repo::Result<EnvironmentCurrentRevisionRecord> =
            self.with_tx("create", |tx| async move {
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO environments
                        (environment_id, name, application_id, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                        VALUES ($1, $2, $3, $4, $5, NULL, $6, 0)
                    "# })
                        .bind(revision.environment_id)
                        .bind(&name)
                        .bind(application_id)
                        .bind(&revision.audit.created_at)
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                ).await?;

                let revision = Self::insert_revision(tx, revision).await?;

                Ok(EnvironmentCurrentRevisionRecord {
                    name,
                    application_id,
                    revision,
                })
            }.boxed()).await;

        match result {
            Ok(env) => Ok(Some(env)),
            Err(err) if err.is_unique_violation() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        let Some(checked_env) = self
            .check_current_revision(&revision.environment_id, current_revision_id)
            .await?
        else {
            return Ok(None);
        };

        let result: repo::Result<EnvironmentCurrentRevisionRecord> = self
            .with_tx("update", |tx| {
                async move {
                    let revision: EnvironmentRevisionRecord =
                        Self::insert_revision(tx, revision.ensure_new(current_revision_id)).await?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE environments
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE environment_id = $4
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.environment_id),
                    )
                    .await?;

                    Ok(EnvironmentCurrentRevisionRecord {
                        name: checked_env.name,
                        application_id: checked_env.application_id,
                        revision,
                    })
                }
                .boxed()
            })
            .await;

        match result {
            Ok(env) => Ok(Some(env)),
            Err(err) if err.is_unique_violation() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        let user_account_id = *user_account_id;
        let environment_id = *environment_id;

        let Some(_checked_env) = self
            .check_current_revision(&environment_id, current_revision_id)
            .await?
        else {
            return Ok(false);
        };

        let result: repo::Result<()> = self
            .with_tx("delete", |tx| {
                async move {
                    let revision: EnvironmentRevisionRecord = Self::insert_revision(
                        tx,
                        EnvironmentRevisionRecord::deletion(
                            user_account_id,
                            environment_id,
                            current_revision_id,
                        ),
                    )
                    .await?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE environments
                            SET deleted_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE environment_id = $4
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.environment_id),
                    )
                    .await?;

                    Ok(())
                }
                .boxed()
            })
            .await;

        match result {
            Ok(()) => Ok(true),
            Err(err) if err.is_unique_violation() => Ok(false),
            Err(err) => Err(err),
        }
    }
}

#[async_trait]
trait EnvironmentRepoInternal: EnvironmentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn check_current_revision(
        &self,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<EnvironmentRecord>>;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<EnvironmentRevisionRecord>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentRepoInternal for DbEnvironmentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn check_current_revision(
        &self,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<EnvironmentRecord>> {
        self.with_ro("check_current_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT environment_id, name, application_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                FROM environments
                WHERE environment_id = $1 AND current_revision_id = $2 and deleted_at IS NULL
            "#})
                .bind(environment_id)
                .bind(current_revision_id),
        )
            .await
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<EnvironmentRevisionRecord> {
        tx.fetch_one_as(sqlx::query_as(indoc! { r#"
            INSERT INTO environment_revisions
            (environment_id, revision_id, hash, created_at, created_by, deleted, compatibility_check, version_check, security_overrides)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING environment_id, revision_id, hash, created_at, created_by, deleted, compatibility_check, version_check, security_overrides
        "# })
            .bind(revision.environment_id)
            .bind(revision.revision_id)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit)
            .bind(revision.compatibility_check)
            .bind(revision.version_check)
            .bind(revision.security_overrides)).await
    }
}
