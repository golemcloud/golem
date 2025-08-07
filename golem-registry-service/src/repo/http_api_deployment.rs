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
use crate::repo::model::http_api_definition::HttpApiDefinitionRevisionIdentityRecord;
use crate::repo::model::http_api_deployment::{
    HttpApiDeploymentExtRevisionRecord, HttpApiDeploymentRecord,
    HttpApiDeploymentRevisionIdentityRecord, HttpApiDeploymentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt, TryStreamExt, stream};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use sqlx::{Database, Row};
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait HttpApiDeploymentRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn update(
        &self,
        current_revision_id: i64,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn delete(
        &self,
        user_account_id: &Uuid,
        http_api_deployment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool>;

    async fn get_staged_by_id(
        &self,
        http_api_deployment_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_deployed_by_id(
        &self,
        http_api_deployment_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_by_id_and_revision(
        &self,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn get_by_name_and_revision(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>>;

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>>;

    async fn list_by_deployment(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>>;
}

pub struct LoggedHttpApiDeploymentRepo<Repo: HttpApiDeploymentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "http_api_deployment repository";

impl<Repo: HttpApiDeploymentRepo> LoggedHttpApiDeploymentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(environment_id: &Uuid, host: &str, subdomain: Option<&str>) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, host, subdomain = ?subdomain)
    }

    fn span_env(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_env_and_deployment(environment_id: &Uuid, deployment_revision_id: i64) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, deployment_revision_id)
    }

    fn span_name_and_revision(
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision: i64,
    ) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, host, subdomain = ?subdomain, revision)
    }

    fn span_id(http_api_deployment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, http_api_deployment_id = %http_api_deployment_id)
    }

    fn span_id_and_revision(http_api_deployment_id: &Uuid, revision_id: i64) -> Span {
        info_span!(SPAN_NAME, http_api_deployment_id = %http_api_deployment_id, revision_id)
    }
}

#[async_trait]
impl<Repo: HttpApiDeploymentRepo> HttpApiDeploymentRepo for LoggedHttpApiDeploymentRepo<Repo> {
    async fn create(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .create(environment_id, host, subdomain, revision)
            .instrument(Self::span_name(environment_id, host, subdomain))
            .await
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let span = Self::span_id(&revision.http_api_deployment_id);
        self.repo
            .update(current_revision_id, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        http_api_deployment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        self.repo
            .delete(user_account_id, http_api_deployment_id, current_revision_id)
            .instrument(Self::span_id(http_api_deployment_id))
            .await
    }

    async fn get_staged_by_id(
        &self,
        http_api_deployment_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_staged_by_id(http_api_deployment_id)
            .instrument(Self::span_id(http_api_deployment_id))
            .await
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_staged_by_name(environment_id, host, subdomain)
            .instrument(Self::span_name(environment_id, host, subdomain))
            .await
    }

    async fn get_deployed_by_id(
        &self,
        http_api_deployment_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_deployed_by_id(http_api_deployment_id)
            .instrument(Self::span_id(http_api_deployment_id))
            .await
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_deployed_by_name(environment_id, host, subdomain)
            .instrument(Self::span_name(environment_id, host, subdomain))
            .await
    }

    async fn get_by_id_and_revision(
        &self,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_by_id_and_revision(http_api_deployment_id, revision_id)
            .instrument(Self::span_id_and_revision(
                http_api_deployment_id,
                revision_id,
            ))
            .await
    }

    async fn get_by_name_and_revision(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .get_by_name_and_revision(environment_id, host, subdomain, revision_id)
            .instrument(Self::span_name_and_revision(
                environment_id,
                host,
                subdomain,
                revision_id,
            ))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .list_deployed(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>> {
        self.repo
            .list_by_deployment(environment_id, deployment_revision_id)
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

static METRICS_SVC_NAME: &str = "environment";

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

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> repo::Result<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, repo::Result<R>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl HttpApiDeploymentRepo for DbHttpApiDeploymentRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let opt_deleted_revision: Option<HttpApiDeploymentRevisionIdentityRecord> =
            self.with_ro("create - get opt deleted").fetch_optional_as(
                match subdomain {
                    Some(subdomain) => {
                        sqlx::query_as(indoc! { r#"
                            SELECT h.http_api_deployment_id, h.host, h.subdomain, hr.revision_id, hr.hash
                            FROM http_api_deployments h
                            JOIN http_api_deployment_revisions hr
                                ON h.http_api_deployment_id = hr.http_api_deployment_id
                                    AND h.current_revision_id = hr.revision_id
                            WHERE environment_id = $1 AND host = $2 AND subdomain = $3 AND deleted_at IS NOT NULL
                        "#})
                            .bind(environment_id)
                            .bind(host)
                            .bind(subdomain)
                    }
                    None =>
                        sqlx::query_as(indoc! { r#"
                            SELECT h.http_api_deployment_id, h.host, h.subdomain, hr.revision_id, hr.hash
                            FROM http_api_deployments h
                            JOIN http_api_deployment_revisions hr
                                ON h.http_api_deployment_id = hr.http_api_deployment_id
                                    AND h.current_revision_id = hr.revision_id
                            WHERE environment_id = $1 AND host = $2 AND subdomain IS NULL AND deleted_at IS NOT NULL
                        "#})
                            .bind(environment_id)
                            .bind(host)
                }
            ).await?;

        if let Some(deleted_revision) = opt_deleted_revision {
            let revision = HttpApiDeploymentRevisionRecord {
                http_api_deployment_id: revision.http_api_deployment_id,
                ..revision
            };
            return self.update(deleted_revision.revision_id, revision).await;
        }

        let environment_id = *environment_id;
        let host = host.to_owned();
        let subdomain = subdomain.map(|s| s.to_owned());
        let revision = revision.ensure_first();

        self.with_tx("create", |tx| {
            async move {
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO http_api_deployments
                        (http_api_deployment_id, environment_id, host, subdomain,
                            created_at, updated_at, deleted_at, modified_by,
                            current_revision_id)
                        VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, 0)
                    "# })
                    .bind(revision.http_api_deployment_id)
                    .bind(environment_id)
                    .bind(&host)
                    .bind(&subdomain)
                    .bind(&revision.audit.created_at)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by),
                )
                .await?;

                let revision = Self::insert_revision(tx, revision).await?;

                Ok(HttpApiDeploymentExtRevisionRecord {
                    environment_id,
                    host,
                    subdomain,
                    revision,
                })
            }
            .boxed()
        })
        .await
        .none_on_unique_violation()
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let Some(_checked_http_api_deployment) = self
            .check_current_revision(&revision.http_api_deployment_id, current_revision_id)
            .await?
        else {
            return Ok(None);
        };

        self.with_tx("update", |tx| {
            async move {
                let revision: HttpApiDeploymentRevisionRecord =
                    Self::insert_revision(tx, revision.ensure_new(current_revision_id)).await?;

                let ext = tx
                    .fetch_one(
                        sqlx::query(indoc! { r#"
                        UPDATE http_api_deployments
                        SET updated_at = $1, modified_by = $2, current_revision_id = $3
                        WHERE http_api_deployment_id = $4
                        RETURNING environment_id, host, subdomain
                    "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.http_api_deployment_id),
                    )
                    .await?;

                Ok(HttpApiDeploymentExtRevisionRecord {
                    environment_id: ext.try_get("environment_id")?,
                    host: ext.try_get("host")?,
                    subdomain: ext.try_get("subdomain")?,
                    revision,
                })
            }
            .boxed()
        })
        .await
        .none_on_unique_violation()
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        http_api_deployment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        let user_account_id = *user_account_id;
        let http_api_deployment_id = *http_api_deployment_id;

        let Some(_checked_env) = self
            .check_current_revision(&http_api_deployment_id, current_revision_id)
            .await?
        else {
            return Ok(false);
        };

        self.with_tx("delete", |tx| {
            async move {
                let revision: HttpApiDeploymentRevisionRecord = Self::insert_revision(
                    tx,
                    HttpApiDeploymentRevisionRecord::deletion(
                        user_account_id,
                        http_api_deployment_id,
                        current_revision_id,
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
        .false_on_unique_violation()
    }

    async fn get_staged_by_id(
        &self,
        http_api_deployment_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let revision = self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.host, d.subdomain,
                           dr.http_api_deployment_id, dr.revision_id, dr.hash,
                           dr.created_at, dr.created_by, dr.deleted
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                    WHERE d.http_api_deployment_id = $1 AND d.deleted_at IS NULL
                "#})
                    .bind(http_api_deployment_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_http_api_definitions(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let revision = self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                match subdomain {
                    Some(subdomain) => {
                        sqlx::query_as(indoc! { r#"
                            SELECT d.environment_id, d.host, d.subdomain,
                                   dr.http_api_deployment_id, dr.revision_id, dr.hash,
                                   dr.created_at, dr.created_by, dr.deleted
                            FROM http_api_deployments d
                            JOIN http_api_deployment_revisions dr
                                ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                            WHERE d.environment_id = $1 AND d.host = $2 AND d.subdomain = $3 AND d.deleted_at IS NULL
                        "#})
                            .bind(environment_id)
                            .bind(host)
                            .bind(subdomain)
                    }
                    None => {
                        sqlx::query_as(indoc! { r#"
                            SELECT dr.http_api_deployment_id, dr.revision_id, dr.hash,
                                   dr.created_at, dr.created_by, dr.deleted
                            FROM http_api_deployments d
                            JOIN http_api_deployment_revisions dr
                                ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                            WHERE d.environment_id = $1 AND d.host = $2 AND d.subdomain IS NULL AND d.deleted_at IS NULL
                        "#})
                            .bind(environment_id)
                            .bind(host)
                    }
                }
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_http_api_definitions(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_id(
        &self,
        http_api_deployment_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.environment_id, had.host, had.subdomain,
                           hadr.http_api_deployment_id, hadr.revision_id, hadr.hash,
                           hadr.created_at, hadr.created_by, hadr.deleted
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN current_deployments cd ON had.environment_id = cd.environment_id
                    JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                        AND cd.current_revision_id = dr.revision_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.environment_id = dr.environment_id
                            AND dhadr.deployment_revision_id = dr.revision_id
                            AND dhadr.http_api_definition_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE had.http_api_deployment_id = $1 AND had.deleted_at IS NULL
                "#})
                    .bind(http_api_deployment_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_http_api_definitions(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_name")
            .fetch_optional_as(
                match subdomain {
                    Some(subdomain) => {
                        sqlx::query_as(indoc! { r#"
                            SELECT had.environment_id, had.host, had.subdomain,
                                   hadr.http_api_deployment_id, hadr.revision_id, hadr.hash,
                                   hadr.created_at, hadr.created_by, hadr.deleted
                            FROM http_api_deployments had
                            JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                            JOIN current_deployments cd ON had.environment_id = cd.environment_id
                            JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                                AND cd.current_revision_id = dr.revision_id
                            JOIN deployment_http_api_definition_revisions dhadr
                                ON dhadr.environment_id = dr.environment_id
                                    AND dhadr.deployment_revision_id = dr.revision_id
                                    AND dhadr.http_api_definition_id = hadr.http_api_deployment_id
                                    AND dhadr.http_api_definition_revision_id = hadr.revision_id
                            WHERE had.environment_id = $1 AND had.host = $2 AND had.subdomain = $3 AND had.deleted_at IS NULL
                        "#})
                            .bind(environment_id)
                            .bind(host)
                            .bind(subdomain)
                    }

                    None => {
                        sqlx::query_as(indoc! { r#"
                            SELECT had.environment_id, had.host, had.subdomain,
                                   hadr.http_api_deployment_id, hadr.revision_id, hadr.hash,
                                   hadr.created_at, hadr.created_by, hadr.deleted
                            FROM http_api_deployments had
                            JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                            JOIN current_deployments cd ON had.environment_id = cd.environment_id
                            JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                                AND cd.current_revision_id = dr.revision_id
                            JOIN deployment_http_api_definition_revisions dhadr
                                ON dhadr.environment_id = dr.environment_id
                                    AND dhadr.deployment_revision_id = dr.revision_id
                                    AND dhadr.http_api_definition_id = hadr.http_api_deployment_id
                                    AND dhadr.http_api_definition_revision_id = hadr.revision_id
                            WHERE had.environment_id = $1 AND had.host = $2 AND had.subdomain IS NULL AND had.deleted_at IS NULL
                        "#})
                            .bind(environment_id)
                            .bind(host)
                            .bind(subdomain)
                    }
                })
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_http_api_definitions(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_by_id_and_revision(
        &self,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let revision = self.with_ro("get_by_id_and_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.host, d.subdomain,
                           dr.http_api_deployment_id, dr.revision_id, dr.hash,
                           dr.created_at, dr.created_by, dr.deleted
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id
                    WHERE d.http_api_deployment_id = $1 AND dr.revision_id = $2 AND dr.deleted = FALSE
                "#})
                    .bind(http_api_deployment_id)
                    .bind(revision_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_http_api_definitions(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_by_name_and_revision(
        &self,
        environment_id: &Uuid,
        host: &str,
        subdomain: Option<&str>,
        revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentExtRevisionRecord>> {
        let revision = self
            .with_ro("get_by_name_and_revision")
            .fetch_optional_as(match subdomain {
                Some(subdomain) => sqlx::query_as(indoc! { r#"
                            SELECT d.environment_id, d.host, d.subdomain,
                                   dr.http_api_deployment_id, dr.revision_id, dr.hash,
                                   dr.created_at, dr.created_by, dr.deleted
                            FROM http_api_deployments d
                            JOIN http_api_deployment_revisions dr
                                ON d.http_api_deployment_id = dr.http_api_deployment_id
                            WHERE d.environment_id = $1 AND d.host = $2 AND d.subdomain = $3
                                AND dr.revision_id = $4 AND dr.deleted = FALSE
                        "#})
                .bind(environment_id)
                .bind(host)
                .bind(subdomain)
                .bind(revision_id),
                None => sqlx::query_as(indoc! { r#"
                            SELECT dr.http_api_deployment_id, dr.revision_id, dr.hash,
                                   dr.created_at, dr.created_by, dr.deleted
                            FROM http_api_deployments d
                            JOIN http_api_deployment_revisions dr
                                ON d.http_api_deployment_id = dr.http_api_deployment_id
                            WHERE d.environment_id = $1 AND d.host = $2 AND d.subdomain IS NULL
                                AND dr.revision_id = $3 AND dr.deleted = FALSE
                        "#})
                .bind(environment_id)
                .bind(host)
                .bind(revision_id),
            })
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_http_api_definitions(revision).await?)),
            None => Ok(None),
        }
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>> {
        let revisions = self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.environment_id, d.host, d.subdomain,
                           dr.http_api_deployment_id, dr.revision_id, dr.hash,
                           dr.created_at, dr.created_by, dr.deleted
                    FROM http_api_deployments d
                    JOIN http_api_deployment_revisions dr
                        ON d.http_api_deployment_id = dr.http_api_deployment_id AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.deleted_at IS NULL
                    ORDER BY d.host, d.subdomain
                "#})
                    .bind(environment_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.with_http_api_definitions(revision))
            .try_collect()
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>> {
        let revisions = self.with_ro("list_deployed")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.environment_id, had.host, had.subdomain,
                           hadr.http_api_deployment_id, hadr.revision_id, hadr.hash,
                           hadr.created_at, hadr.created_by, hadr.deleted
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN current_deployments cd ON had.environment_id = cd.environment_id
                    JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                        AND cd.current_revision_id = dr.revision_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.environment_id = dr.environment_id
                            AND dhadr.deployment_revision_id = dr.revision_id
                            AND dhadr.http_api_definition_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE had.environment_id = $1 AND had.deleted_at IS NULL
                    ORDER BY had.host, had.subdomain
                "#})
                    .bind(environment_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.with_http_api_definitions(revision))
            .try_collect()
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> repo::Result<Vec<HttpApiDeploymentExtRevisionRecord>> {
        let revisions = self.with_ro("list_by_deployment")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT had.environment_id, had.host, had.subdomain,
                           hadr.http_api_deployment_id, hadr.revision_id, hadr.hash,
                           hadr.created_at, hadr.created_by, hadr.deleted
                    FROM http_api_deployments had
                    JOIN http_api_deployment_revisions hadr ON had.http_api_deployment_id = hadr.http_api_deployment_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.http_api_definition_id = hadr.http_api_deployment_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE dhadr.environment_id = $1 AND dhadr.deployment_revision_id = $2
                        AND hadr.deleted = FALSE
                    ORDER BY had.host, had.subdomain
                "#})
                    .bind(environment_id)
                    .bind(deployment_revision_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.with_http_api_definitions(revision))
            .try_collect()
            .await
    }
}

#[async_trait]
trait HttpApiDeploymentRepoInternal: HttpApiDeploymentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn check_current_revision(
        &self,
        http_api_deployment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentRecord>>;

    async fn get_http_api_definitions(
        &self,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionIdentityRecord>>;

    async fn with_http_api_definitions(
        &self,
        mut deployment: HttpApiDeploymentExtRevisionRecord,
    ) -> repo::Result<HttpApiDeploymentExtRevisionRecord> {
        deployment.revision.http_api_definitions = self
            .get_http_api_definitions(
                &deployment.revision.http_api_deployment_id,
                deployment.revision.revision_id,
            )
            .await?;
        Ok(deployment)
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<HttpApiDeploymentRevisionRecord>;

    async fn insert_definition(
        tx: &mut Self::Tx,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
        http_definition_id: &Uuid,
    ) -> repo::Result<HttpApiDefinitionRevisionIdentityRecord>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl HttpApiDeploymentRepoInternal for DbHttpApiDeploymentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn check_current_revision(
        &self,
        http_api_deployment_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<HttpApiDeploymentRecord>> {
        self.with_ro("check_current_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT http_api_deployment_id, environment_id, host, subdomain,
                           created_at, updated_at, deleted_at, modified_by,
                           current_revision_id
                    FROM http_api_deployments
                    WHERE http_api_deployment_id = $1 AND current_revision_id = $2 and deleted_at IS NULL
                "#})
                    .bind(http_api_deployment_id)
                    .bind(current_revision_id),
            )
            .await
    }

    async fn get_http_api_definitions(
        &self,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionIdentityRecord>> {
        self.with_ro("get_http_api_definitions")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT d.http_api_definition_id, d.name, dr.revision_id, dr.version, dr.hash
                    FROM http_api_deployment_definitions dd
                    JOIN http_api_definitions d ON dd.http_definition_id = d.http_api_definition_id
                    JOIN http_api_definition_revisions dr
                        ON d.http_api_definition_id = dr.http_api_definition_id AND d.current_revision_id = dr.revision_id
                    WHERE dd.http_api_deployment_id = $1 AND dd.revision_id = $2
                    ORDER BY d.name
                "#})
                    .bind(http_api_deployment_id)
                    .bind(revision_id),
            )
            .await
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: HttpApiDeploymentRevisionRecord,
    ) -> repo::Result<HttpApiDeploymentRevisionRecord> {
        let definitions = revision.http_api_definitions;

        let mut revision: HttpApiDeploymentRevisionRecord = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO http_api_deployment_revisions
                    (http_api_deployment_id, revision_id, hash,
                        created_at, created_by, deleted)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING http_api_deployment_id, revision_id, hash,
                        created_at, created_by, deleted
                "# })
                .bind(revision.http_api_deployment_id)
                .bind(revision.revision_id)
                .bind(revision.hash)
                .bind_deletable_revision_audit(revision.audit),
            )
            .await?;

        revision.http_api_definitions = {
            let mut inserted_definitions = Vec::with_capacity(definitions.len());
            for definition in definitions.iter() {
                inserted_definitions.push(
                    Self::insert_definition(
                        tx,
                        &revision.http_api_deployment_id,
                        revision.revision_id,
                        &definition.http_api_definition_id,
                    )
                    .await?,
                );
            }
            inserted_definitions
        };

        Ok(revision)
    }

    async fn insert_definition(
        tx: &mut Self::Tx,
        http_api_deployment_id: &Uuid,
        revision_id: i64,
        http_definition_id: &Uuid,
    ) -> repo::Result<HttpApiDefinitionRevisionIdentityRecord> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO http_api_deployment_definitions
                (http_api_deployment_id, revision_id, http_definition_id)
                VALUES ($1, $2, $3)
            "#})
            .bind(http_api_deployment_id)
            .bind(revision_id)
            .bind(http_definition_id),
        )
        .await?;

        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                SELECT d.http_api_definition_id, d.name, dr.revision_id, dr.version, dr.hash
                FROM http_api_definitions d
                JOIN http_api_definition_revisions dr ON
                    d.http_api_definition_id = dr.http_api_definition_id AND
                    d.current_revision_id = dr.revision_id
                WHERE dr.http_api_definition_id = $1
            "#})
            .bind(http_definition_id),
        )
        .await
    }
}
