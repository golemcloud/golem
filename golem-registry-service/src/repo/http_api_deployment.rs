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
use crate::repo::model::http_api_deployment::{
    HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRecord, HttpApiDeploymentRepoError,
    HttpApiDeploymentRevisionIdentityRecord, HttpApiDeploymentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult, ResultExt};
use indoc::indoc;
use sqlx::Database;
use std::fmt::Debug;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait HttpApiDeploymentRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: Uuid,
        domain: &str,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRepoError>;

    async fn update(
        &self,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRepoError>;

    async fn delete(
        &self,
        user_account_id: Uuid,
        http_api_deployment_id: Uuid,
        revision_id: i64,
    ) -> Result<(), HttpApiDeploymentRepoError>;

    async fn get_staged_by_id(
        &self,
        http_api_deployment_id: Uuid,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_staged_by_domain(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_by_id_and_revision(
        &self,
        http_api_deployment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<HttpApiDeploymentExtRevisionRecord>>;

    async fn list_by_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_in_deployment_by_domain(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        domain: &str,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>>;
}

pub struct LoggedHttpApiDeploymentRepo<Repo: HttpApiDeploymentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "http_api_deployment repository";

impl<Repo: HttpApiDeploymentRepo> LoggedHttpApiDeploymentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(environment_id: Uuid, domain: &str) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, domain)
    }

    fn span_env(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_env_and_deployment(environment_id: Uuid, deployment_revision_id: i64) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, deployment_revision_id)
    }

    fn span_id(http_api_deployment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, http_api_deployment_id = %http_api_deployment_id)
    }

    fn span_id_and_revision(http_api_deployment_id: Uuid, revision_id: i64) -> Span {
        info_span!(SPAN_NAME, http_api_deployment_id = %http_api_deployment_id, revision_id)
    }
}

#[async_trait]
impl<Repo: HttpApiDeploymentRepo> HttpApiDeploymentRepo for LoggedHttpApiDeploymentRepo<Repo> {
    async fn create(
        &self,
        environment_id: Uuid,
        domain: &str,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRepoError> {
        self.repo
            .create(environment_id, domain, revision)
            .instrument(Self::span_name(environment_id, domain))
            .await
    }

    async fn update(
        &self,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRepoError> {
        let span = Self::span_id(revision.http_api_deployment_id);
        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        user_account_id: Uuid,
        http_api_deployment_id: Uuid,
        current_revision_id: i64,
    ) -> Result<(), HttpApiDeploymentRepoError> {
        self.repo
            .delete(user_account_id, http_api_deployment_id, current_revision_id)
            .instrument(Self::span_id(http_api_deployment_id))
            .await
    }

    async fn get_staged_by_id(
        &self,
        http_api_deployment_id: Uuid,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_staged_by_id(http_api_deployment_id)
            .instrument(Self::span_id(http_api_deployment_id))
            .await
    }

    async fn get_staged_by_domain(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_staged_by_domain(environment_id, domain)
            .instrument(Self::span_name(environment_id, domain))
            .await
    }

    async fn get_by_id_and_revision(
        &self,
        http_api_deployment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_by_id_and_revision(http_api_deployment_id, revision_id)
            .instrument(Self::span_id_and_revision(
                http_api_deployment_id,
                revision_id,
            ))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .list_by_deployment(environment_id, deployment_revision_id)
            .instrument(Self::span_env_and_deployment(
                environment_id,
                deployment_revision_id,
            ))
            .await
    }

    async fn get_in_deployment_by_domain(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        domain: &str,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_in_deployment_by_domain(environment_id, deployment_revision_id, domain)
            .instrument(Self::span_env_and_deployment(
                environment_id,
                deployment_revision_id,
            ))
            .await
    }
}

pub struct DbHttpApiDeploymentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "http_api_deployment_repo";

impl<DBP: Pool> DbHttpApiDeploymentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedHttpApiDeploymentRepo<Self>
    where
        Self: HttpApiDeploymentRepo,
    {
        LoggedHttpApiDeploymentRepo::new(Self::new(db_pool))
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
impl HttpApiDeploymentRepo for DbHttpApiDeploymentRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: Uuid,
        domain: &str,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRepoError> {
        let opt_deleted_revision: Option<HttpApiDeploymentRevisionIdentityRecord> = self
            .with_ro("create - get opt deleted")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT h.http_api_deployment_id, h.domain, hr.revision_id, hr.hash, hr.data
                    FROM http_api_deployments h
                    JOIN http_api_deployment_revisions hr
                        ON h.http_api_deployment_id = hr.http_api_deployment_id
                            AND h.current_revision_id = hr.revision_id
                    WHERE environment_id = $1 AND domain = $2 AND deleted_at IS NOT NULL
                "#})
                .bind(environment_id)
                .bind(domain),
            )
            .await?;

        if let Some(deleted_revision) = opt_deleted_revision {
            let recreated_revision = revision.for_recreation(
                deleted_revision.http_api_deployment_id,
                deleted_revision.revision_id,
            )?;
            return self.update(recreated_revision).await;
        }

        let domain = domain.to_owned();

        self.with_tx_err("create", |tx| {
            async move {
                let main_record: HttpApiDeploymentRecord = tx.fetch_one_as(
                    sqlx::query_as(indoc! { r#"
                        INSERT INTO http_api_deployments
                        (http_api_deployment_id, environment_id, domain,
                            created_at, updated_at, deleted_at, modified_by,
                            current_revision_id)
                        VALUES ($1, $2, $3, $4, $5, NULL, $6, 0)
                        RETURNING http_api_deployment_id, environment_id, domain, created_at, updated_at, deleted_at, modified_by, current_revision_id
                    "# })
                    .bind(revision.http_api_deployment_id)
                    .bind(environment_id)
                    .bind(&domain)
                    .bind(&revision.audit.created_at)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by),
                )
                .await
                .to_error_on_unique_violation(HttpApiDeploymentRepoError::ApiDeploymentViolatesUniqueness)?;

                let revision = Self::insert_revision(tx, revision).await?;

                Ok(HttpApiDeploymentExtRevisionRecord {
                    environment_id,
                    domain,
                    entity_created_at: main_record.audit.created_at,
                    revision,
                })
            }
            .boxed()
        })
        .await
    }

    async fn update(
        &self,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let revision: HttpApiDeploymentRevisionRecord = Self::insert_revision(
                    tx,
                    revision,
                )
                .await?;

                let main_record: HttpApiDeploymentRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            UPDATE http_api_deployments
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3, deleted_at = NULL
                            WHERE http_api_deployment_id = $4
                            RETURNING http_api_deployment_id, environment_id, domain, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.http_api_deployment_id),
                    )
                    .await?;

                Ok(HttpApiDeploymentExtRevisionRecord {
                    environment_id: main_record.environment_id,
                    domain: main_record.domain,
                    entity_created_at: main_record.audit.created_at,
                    revision,
                })
            }
            .boxed()
        })
        .await
    }

    async fn delete(
        &self,
        user_account_id: Uuid,
        http_api_deployment_id: Uuid,
        revision_id: i64,
    ) -> Result<(), HttpApiDeploymentRepoError> {
        self.with_tx_err("delete", |tx| {
            async move {
                let revision: HttpApiDeploymentRevisionRecord = Self::insert_revision(
                    tx,
                    HttpApiDeploymentRevisionRecord::deletion(
                        user_account_id,
                        http_api_deployment_id,
                        revision_id,
                    ),
                )
                .await?;

                tx.execute(
                    sqlx::query(indoc! { r#"
                        UPDATE http_api_deployments
                        SET deleted_at = $1, modified_by = $2, current_revision_id = $3
                        WHERE http_api_deployment_id = $4
                    "#})
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.http_api_deployment_id),
                )
                .await?;

                Ok(())
            }
            .boxed()
        })
        .await
    }

    async fn get_staged_by_id(
        &self,
        http_api_deployment_id: Uuid,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.domain, dr.http_api_deployment_id,
                        dr.revision_id, dr.hash, dr.data,
                        dr.created_at, dr.created_by, dr.deleted,
                        d.created_at as entity_created_at
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                    WHERE d.http_api_deployment_id = $1 AND d.deleted_at IS NULL
                "#})
                    .bind(http_api_deployment_id),
            )
            .await
    }

    async fn get_staged_by_domain(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.domain, dr.http_api_deployment_id,
                        dr.revision_id, dr.hash, dr.data,
                        dr.created_at, dr.created_by, dr.deleted,
                        d.created_at as entity_created_at
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.domain = $2 AND d.deleted_at IS NULL
                "#})
                    .bind(environment_id)
                    .bind(domain)
            )
            .await
    }

    async fn get_by_id_and_revision(
        &self,
        http_api_deployment_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.with_ro("get_by_id_and_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.domain, dr.http_api_deployment_id,
                        dr.revision_id, dr.hash, dr.data,
                        dr.created_at, dr.created_by, dr.deleted,
                        d.created_at as entity_created_at
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id
                    WHERE d.http_api_deployment_id = $1 AND dr.revision_id = $2 AND dr.deleted = FALSE
                "#})
                    .bind(http_api_deployment_id)
                    .bind(revision_id),
            )
            .await
    }

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.domain, dr.http_api_deployment_id,
                        dr.revision_id, dr.hash, dr.data,
                        dr.created_at, dr.created_by, dr.deleted,
                        d.created_at as entity_created_at
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.deleted_at IS NULL
                    ORDER BY d.domain
                "#})
                    .bind(environment_id),
            )
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.with_ro("list_by_deployment")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.environment_id, had.domain, hadr.http_api_deployment_id,
                        hadr.revision_id, hadr.hash, hadr.data,
                        hadr.created_at, hadr.created_by, hadr.deleted,
                        had.created_at as entity_created_at
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN deployment_http_api_deployment_revisions dhadr
                        ON dhadr.http_api_deployment_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_deployment_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2
                    ORDER BY had.domain
                "#})
                    .bind(environment_id)
                    .bind(deployment_revision_id),
            )
            .await
    }

    async fn get_in_deployment_by_domain(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        domain: &str,
    ) -> RepoResult<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.with_ro("get_in_deployment_by_domain")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.environment_id, had.domain, hadr.http_api_deployment_id,
                        hadr.revision_id, hadr.hash, hadr.data,
                        hadr.created_at, hadr.created_by, hadr.deleted,
                        had.created_at as entity_created_at
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN deployment_http_api_deployment_revisions dhadr
                        ON dhadr.http_api_deployment_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_deployment_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2 AND had.domain = $3
                "#})
                    .bind(environment_id)
                    .bind(deployment_revision_id)
                    .bind(domain)
            )
            .await
    }
}

#[async_trait]
trait HttpApiDeploymentRepoInternal: HttpApiDeploymentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentRevisionRecord, HttpApiDeploymentRepoError>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl HttpApiDeploymentRepoInternal for DbHttpApiDeploymentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> Result<HttpApiDeploymentRevisionRecord, HttpApiDeploymentRepoError> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                    INSERT INTO http_api_deployment_revisions
                    (http_api_deployment_id, revision_id, data,
                        hash, created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    RETURNING http_api_deployment_id, revision_id, hash,
                        created_at, created_by, deleted, data
                "# })
            .bind(revision.http_api_deployment_id)
            .bind(revision.revision_id)
            .bind(&revision.data)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit),
        )
        .await
        .to_error_on_unique_violation(HttpApiDeploymentRepoError::ConcurrentModification)
    }
}
