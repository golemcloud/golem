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

use crate::repo::{SqlBlake3Hash, SqlDateTime};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::FromRow;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub application_id: Uuid,
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
    pub current_revision_id: Option<i64>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
    pub hash: SqlBlake3Hash,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct EnvironmentCurrentRevisionRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub application_id: Uuid,
    pub environment_created_at: SqlDateTime,
    pub environment_created_by: Uuid,
    pub current_revision_id: Option<i64>,
    pub created_at: Option<SqlDateTime>,
    pub created_by: Option<Uuid>,
    pub compatibility_check: Option<bool>,
    pub version_check: Option<bool>,
    pub security_overrides: Option<bool>,
    pub hash: Option<SqlBlake3Hash>,
}

impl EnvironmentCurrentRevisionRecord {
    pub fn to_revision(&self) -> Option<EnvironmentRevisionRecord> {
        self.current_revision_id?;
        Some(EnvironmentRevisionRecord {
            environment_id: self.environment_id,
            revision_id: self.current_revision_id.unwrap(),
            created_at: self.created_at.clone().unwrap(),
            created_by: self.created_by.unwrap(),
            compatibility_check: self.compatibility_check.unwrap(),
            version_check: self.version_check.unwrap(),
            security_overrides: self.security_overrides.unwrap(),
            hash: self.hash.unwrap(),
        })
    }
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

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<EnvironmentCurrentRevisionRecord>;

    async fn create_revision(
        &self,
        current_revision_id: Option<i64>,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentRevisionRecord>>;
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

    fn span_id(environment_id: &Uuid) -> Span {
        info_span!("environment repository", environment_id=%environment_id)
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
            .instrument(Self::span_id(environment_id))
            .await
    }

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<EnvironmentCurrentRevisionRecord> {
        self.repo
            .ensure(user_account_id, application_id, name)
            .instrument(Self::span_name(application_id, name))
            .await
    }

    async fn create_revision(
        &self,
        current_revision_id: Option<i64>,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentRevisionRecord>> {
        let span = Self::span_id(&revision.environment_id);
        self.repo
            .create_revision(current_revision_id, revision)
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
                        e.environment_id AS environment_id,
                        e.name AS name,
                        e.application_id AS application_id,
                        e.created_at AS environment_created_at,
                        e.created_by AS environment_created_by,
                        e.current_revision_id AS current_revision_id,
                        r.created_at AS created_at,
                        r.created_by AS created_by,
                        r.compatibility_check AS compatibility_check,
                        r.version_check AS version_check,
                        r.security_overrides AS security_overrides,
                        r.hash AS hash
                    FROM environments e
                    LEFT JOIN environment_revisions r
                        ON e.environment_id = r.environment_id AND e.current_revision_id = r.revision_id
                    WHERE e.application_id = $1 AND e.name = $2
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
                        e.environment_id AS environment_id,
                        e.name AS name,
                        e.application_id AS application_id,
                        e.created_at AS environment_created_at,
                        e.created_by AS environment_created_by,
                        e.current_revision_id AS current_revision_id,
                        r.created_at AS created_at,
                        r.created_by AS created_by,
                        r.compatibility_check AS compatibility_check,
                        r.version_check AS version_check,
                        r.security_overrides AS security_overrides,
                        r.hash AS hash
                    FROM environments e
                    LEFT JOIN environment_revisions r
                        ON e.environment_id = r.environment_id AND e.current_revision_id = r.revision_id
                    WHERE e.environment_id = $1
                "# })
                    .bind(environment_id),
            )
            .await
    }

    async fn ensure(
        &self,
        user_account_id: &Uuid,
        application_id: &Uuid,
        name: &str,
    ) -> repo::Result<EnvironmentCurrentRevisionRecord> {
        if let Some(env) = self.get_by_name(application_id, name).await? {
            return Ok(env);
        };

        let result: repo::Result<EnvironmentRecord> = self
            .with_rw("ensure - insert")
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO environments
                    (environment_id, name, application_id, created_at, created_by, current_revision_id)
                    VALUES ($1, $2, $3, $4, $5, NULL)
                    RETURNING environment_id, name, application_id, created_at, created_by, current_revision_id
                "# })
                    .bind(Uuid::new_v4())
                    .bind(name)
                    .bind(application_id)
                    .bind(SqlDateTime::now())
                    .bind(user_account_id)
            )
            .await;

        let result = result.map(|env| EnvironmentCurrentRevisionRecord {
            environment_id: env.environment_id,
            name: env.name,
            application_id: env.application_id,
            environment_created_at: env.created_at,
            environment_created_by: env.created_by,
            current_revision_id: None,
            created_at: None,
            created_by: None,
            compatibility_check: None,
            version_check: None,
            security_overrides: None,
            hash: None,
        });

        let result = match result {
            Err(err) if err.is_unique_violation() => None,
            result => Some(result),
        };
        if let Some(result) = result {
            return result;
        }

        match self.get_by_name(application_id, name).await? {
            Some(app) => Ok(app),
            None => Err(RepoError::Internal(
                "illegal state: missing environment".to_string(),
            )),
        }
    }

    async fn create_revision(
        &self,
        current_revision_id: Option<i64>,
        revision: EnvironmentRevisionRecord,
    ) -> repo::Result<Option<EnvironmentRevisionRecord>> {
        {
            let current_revision_matches = self
                .with_ro("create_revision - check current revision")
                .fetch_optional(
                    match current_revision_id {
                        Some(current_revision_id) => {
                            sqlx::query(indoc! { r#"
                                SELECT 1 FROM environments WHERE environment_id = $1 AND current_revision_id = $2
                            "#})
                                .bind(revision.environment_id)
                                .bind(current_revision_id)
                        }
                        None => {
                            sqlx::query(indoc! { r#"
                                SELECT 1 FROM environments WHERE environment_id = $1 AND current_revision_id IS NULL
                            "#})
                                .bind(revision.environment_id)
                        }
                    }
                )
                .await?
                .is_some();

            if !current_revision_matches {
                return Ok(None);
            }
        }

        let revision = self
            .with_tx("create_revision - insert and update", |tx| {
                async move {
                    let revision: EnvironmentRevisionRecord = tx
                        .fetch_one_as(
                            sqlx::query_as(indoc! { r#"
                                INSERT INTO environment_revisions
                                    (environment_id, revision_id, created_at, created_by,
                                     compatibility_check, version_check, security_overrides, hash)
                                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                                RETURNING
                                    environment_id, revision_id, created_at, created_by,
                                    compatibility_check, version_check, security_overrides, hash
                            "#})
                            .bind(revision.environment_id)
                            .bind(current_revision_id.unwrap_or(-1) + 1)
                            .bind(revision.created_at)
                            .bind(revision.created_by)
                            .bind(revision.compatibility_check)
                            .bind(revision.version_check)
                            .bind(revision.security_overrides)
                            .bind(revision.hash),
                        )
                        .await?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE environments SET current_revision_id = $1 WHERE environment_id = $2
                        "#})
                        .bind(revision.revision_id)
                        .bind(revision.environment_id),
                    )
                    .await?;

                    Ok(Some(revision))
                }
                .boxed()
            })
            .await;

        match revision {
            Err(err) if err.is_unique_violation() => Ok(None),
            result => result,
        }
    }
}
