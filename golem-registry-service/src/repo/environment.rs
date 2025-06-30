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

use crate::repo::model::{AuditFields, RevisionAuditFields, SqlBlake3Hash};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::{FromRow, Row};
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub application_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: Option<i64>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
    pub hash: SqlBlake3Hash,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentCurrentRevisionRecord {
    pub name: String,
    pub application_id: Uuid,
    #[sqlx(flatten)]
    pub revision: EnvironmentRevisionRecord,
}

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
}

pub struct LoggedEnvironmentRepo<Repo: EnvironmentRepo> {
    repo: Repo,
}

impl<Repo: EnvironmentRepo> LoggedEnvironmentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(application_id: &Uuid, name: &str) -> Span {
        info_span!("environment repository", application_id=%application_id, name)
    }

    fn span_env_id(environment_id: &Uuid) -> Span {
        info_span!("environment repository", environment_id=%environment_id)
    }

    fn span_app_id(application_id: &Uuid) -> Span {
        info_span!("environment repository", application_id=%application_id)
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
}

pub struct DbEnvironmentRepo<DB: Pool> {
    db_pool: DB,
}

static METRICS_SVC_NAME: &str = "environment";

impl<DB: Pool> DbEnvironmentRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DB::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DB::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<F, R>(&self, api_name: &'static str, f: F) -> Result<R, RepoError>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DB::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, Result<R, RepoError>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(
    golem_service_base::db::postgres::PostgresPool ->
        golem_service_base::db::postgres::PostgresPool,
        golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl EnvironmentRepo for DbEnvironmentRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<EnvironmentCurrentRevisionRecord>> {
        self.with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name AS name,
                        e.application_id AS application_id,
                        r.environment_id AS environment_id,
                        r.revision_id AS revision_id,
                        r.created_at AS created_at,
                        r.created_by AS created_by,
                        r.deleted AS deleted,
                        r.compatibility_check AS compatibility_check,
                        r.version_check AS version_check,
                        r.security_overrides AS security_overrides,
                        r.hash AS hash
                    FROM environments e
                    LEFT JOIN environment_revisions r
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
                        e.name AS name,
                        e.application_id AS application_id,
                        r.environment_id AS environment_id,
                        r.revision_id AS revision_id,
                        r.created_at AS created_at,
                        r.created_by AS created_by,
                        r.deleted AS deleted,
                        r.compatibility_check AS compatibility_check,
                        r.version_check AS version_check,
                        r.security_overrides AS security_overrides,
                        r.hash AS hash
                    FROM environments e
                    LEFT JOIN environment_revisions r
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

                let revision = tx.fetch_one_as(
                    sqlx::query_as(indoc! { r#"
                        INSERT INTO environment_revisions
                        (environment_id, revision_id, created_at, created_by, deleted, compatibility_check, version_check, security_overrides, hash)
                        VALUES ($1, 0, $2, $3, FALSE, $4, $5, $6, $7)
                        RETURNING environment_id, revision_id, created_at, created_by, deleted, compatibility_check, version_check, security_overrides, hash
                    "# })
                        .bind(revision.environment_id)
                        .bind(revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.compatibility_check)
                        .bind(revision.version_check)
                        .bind(revision.security_overrides)
                        .bind(revision.hash)
                ).await?;

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
        #[derive(sqlx::FromRow)]
        struct AppIdAndName {
            application_id: Uuid,
            name: String,
        }

        let matching_current_revision: Option<AppIdAndName> = self
            .with_ro("update - check current revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT application_id, name FROM environments
                    WHERE environment_id = $1 AND current_revision_id = $2 AND deleted_at IS NULL
                "#})
                .bind(revision.environment_id)
                .bind(current_revision_id),
            )
            .await?;

        let Some(matching_current_revision) = matching_current_revision else {
            return Ok(None);
        };

        let result: repo::Result<EnvironmentCurrentRevisionRecord> = self
            .with_tx("update - insert and update", |tx| {
                async move {
                    let revision: EnvironmentRevisionRecord = tx
                        .fetch_one_as(
                            sqlx::query_as(indoc! { r#"
                                INSERT INTO environment_revisions
                                (environment_id, revision_id, created_at, created_by, deleted, compatibility_check, version_check, security_overrides, hash)
                                VALUES ($1, $2, $3, $4, FALSE, $5, $6, $7, $8)
                                RETURNING environment_id, revision_id, created_at, created_by, deleted, compatibility_check, version_check, security_overrides, hash
                            "#})
                                .bind(revision.environment_id)
                                .bind(current_revision_id + 1)
                                .bind(revision.audit.created_at)
                                .bind(revision.audit.created_by)
                                .bind(revision.compatibility_check)
                                .bind(revision.version_check)
                                .bind(revision.security_overrides)
                                .bind(revision.hash),
                        )
                        .await?;

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
                        name: matching_current_revision.name,
                        application_id: matching_current_revision.application_id,
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
}
