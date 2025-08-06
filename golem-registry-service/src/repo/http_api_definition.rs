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
use crate::repo::model::http_api_definition::{
    HttpApiDefinitionRecord, HttpApiDefinitionRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::Database;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait HttpApiDefinitionRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>>;

    async fn update(
        &self,
        current_revision_id: i64,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>>;

    async fn delete(
        &self,
        user_account_id: &Uuid,
        http_api_definition_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool>;

    async fn get_staged_by_id(
        &self,
        http_api_definition_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>>;

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>>;

    async fn get_deployed_by_id(
        &self,
        http_api_definition_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>>;

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionRecord>>;

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionRecord>>;
}

pub struct LoggedHttpApiDefinitionRepo<Repo: HttpApiDefinitionRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "http_api_definition repository";

impl<Repo: HttpApiDefinitionRepo> LoggedHttpApiDefinitionRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(environment_id: &Uuid, name: &str) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, name)
    }

    fn span_http_api_definition_id(http_api_definition_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, http_api_definition_id = %http_api_definition_id)
    }

    fn span_environment_id(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }
}

#[async_trait]
impl<Repo: HttpApiDefinitionRepo> HttpApiDefinitionRepo for LoggedHttpApiDefinitionRepo<Repo> {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .create(environment_id, name, revision)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        let span = Self::span_http_api_definition_id(&revision.http_api_definition_id);
        self.repo
            .update(current_revision_id, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        http_api_definition_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        self.repo
            .delete(user_account_id, http_api_definition_id, current_revision_id)
            .instrument(Self::span_http_api_definition_id(http_api_definition_id))
            .await
    }

    async fn get_staged_by_id(
        &self,
        http_api_definition_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .get_staged_by_id(http_api_definition_id)
            .instrument(Self::span_http_api_definition_id(http_api_definition_id))
            .await
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .get_staged_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn get_deployed_by_id(
        &self,
        http_api_definition_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .get_deployed_by_id(http_api_definition_id)
            .instrument(Self::span_http_api_definition_id(http_api_definition_id))
            .await
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .get_deployed_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionRecord>> {
        self.repo
            .list_deployed(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }
}

pub struct DbHttpApiDefinitionRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment";

impl<DBP: Pool> DbHttpApiDefinitionRepo<DBP> {
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
impl HttpApiDefinitionRepo for DbHttpApiDefinitionRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        let environment_id = *environment_id;
        let name = name.to_owned();
        let revision = revision.ensure_first();

        let result: repo::Result<HttpApiDefinitionRevisionRecord> = self
            .with_tx("create", |tx| {
                async move {
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO http_api_definitions
                            (http_api_definition_id, name, environment_id,
                                created_at, updated_at, deleted_at, modified_by,
                                current_revision_id)
                            VALUES ($1, $2, $3, $4, $5, NULL, $6, 0)
                        "# })
                        .bind(revision.http_api_definition_id)
                        .bind(&name)
                        .bind(environment_id)
                        .bind(&revision.audit.created_at)
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by),
                    )
                    .await?;

                    Self::insert_revision(tx, revision).await
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

    async fn update(
        &self,
        current_revision_id: i64,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        let Some(_checked_http_api_definition) = self
            .check_current_revision(&revision.http_api_definition_id, current_revision_id)
            .await?
        else {
            return Ok(None);
        };

        // TODO: if env requires check version name uniqueness (but comparing only to deployed ones!)

        let result: repo::Result<HttpApiDefinitionRevisionRecord> = self
            .with_tx("update", |tx| {
                async move {
                    let revision: HttpApiDefinitionRevisionRecord =
                        Self::insert_revision(tx, revision.ensure_new(current_revision_id)).await?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE http_api_definitions
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE http_api_definition_id = $4
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.http_api_definition_id),
                    )
                    .await?;

                    Ok(revision)
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
        http_api_definition_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        let user_account_id = *user_account_id;
        let http_api_definition_id = *http_api_definition_id;

        let Some(_checked_env) = self
            .check_current_revision(&http_api_definition_id, current_revision_id)
            .await?
        else {
            return Ok(false);
        };

        let result: repo::Result<()> = self
            .with_tx("delete", |tx| {
                async move {
                    let revision: HttpApiDefinitionRevisionRecord = Self::insert_revision(
                        tx,
                        HttpApiDefinitionRevisionRecord::deletion(
                            user_account_id,
                            http_api_definition_id,
                            current_revision_id,
                        ),
                    )
                    .await?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE http_api_definitions
                            SET deleted_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE http_api_definition_id = $4
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.http_api_definition_id),
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

    async fn get_staged_by_id(
        &self,
        http_api_definition_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT dr.http_api_definition_id, dr.revision_id, dr.version, dr.hash,
                           dr.created_at, dr.created_by, dr.deleted, dr.definition
                    FROM http_api_definitions d
                    JOIN http_api_definition_revisions dr
                        ON d.http_api_definition_id = dr.http_api_definition_id
                            AND d.current_revision_id = dr.revision_id
                    WHERE d.http_api_definition_id = $1 AND d.deleted_at IS NULL
                "#})
                .bind(http_api_definition_id),
            )
            .await
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT dr.http_api_definition_id, dr.revision_id, dr.version, dr.hash,
                           dr.created_at, dr.created_by, dr.deleted, dr.definition
                    FROM http_api_definitions d
                    JOIN http_api_definition_revisions dr
                        ON d.http_api_definition_id = dr.http_api_definition_id
                            AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.name = $2 AND d.deleted_at IS NULL
                "#})
                .bind(environment_id)
                .bind(name),
            )
            .await
    }

    async fn get_deployed_by_id(
        &self,
        http_api_definition_id: &Uuid,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.with_ro("get_deployed_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT hadr.http_api_definition_id, hadr.revision_id, hadr.version, hadr.hash,
                           hadr.created_at, hadr.created_by, hadr.deleted, hadr.definition
                    FROM http_api_definitions had
                    JOIN http_api_definition_revisions hadr ON had.http_api_definition_id = hadr.http_api_definition_id
                    JOIN current_deployments cd ON had.environment_id = cd.environment_id
                    JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                        AND cd.current_revision_id = dr.revision_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.environment_id = dr.environment_id
                            AND dhadr.deployment_revision_id = dr.revision_id
                            AND dhadr.http_api_definition_id = hadr.http_api_definition_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE had.http_api_definition_id = $1 AND had.deleted_at IS NULL
                "#})
                .bind(http_api_definition_id),
            )
            .await
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<HttpApiDefinitionRevisionRecord>> {
        self.with_ro("get_deployed_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT hadr.http_api_definition_id, hadr.revision_id, hadr.version, hadr.hash,
                           hadr.created_at, hadr.created_by, hadr.deleted, hadr.definition
                    FROM http_api_definitions had
                    JOIN http_api_definition_revisions hadr ON had.http_api_definition_id = hadr.http_api_definition_id
                    JOIN current_deployments cd ON had.environment_id = cd.environment_id
                    JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                        AND cd.current_revision_id = dr.revision_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.environment_id = dr.environment_id
                            AND dhadr.deployment_revision_id = dr.revision_id
                            AND dhadr.http_api_definition_id = hadr.http_api_definition_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE had.environment_id = $1 AND had.name = $2 AND had.deleted_at IS NULL
                "#})
                .bind(environment_id)
                .bind(name),
            )
            .await
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionRecord>> {
        self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT dr.http_api_definition_id, dr.revision_id, dr.version, dr.hash,
                           dr.created_at, dr.created_by, dr.deleted, dr.definition
                    FROM http_api_definitions d
                    JOIN http_api_definition_revisions dr
                        ON d.http_api_definition_id = dr.http_api_definition_id AND d.current_revision_id = dr.revision_id
                    WHERE d.environment_id = $1 AND d.deleted_at IS NULL
                    ORDER BY d.name
                "#})
                .bind(environment_id),
            )
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<HttpApiDefinitionRevisionRecord>> {
        self.with_ro("list_deployed")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT hadr.http_api_definition_id, hadr.revision_id, hadr.version, hadr.hash,
                           hadr.created_at, hadr.created_by, hadr.deleted, hadr.definition
                    FROM http_api_definitions had
                    JOIN http_api_definition_revisions hadr ON had.http_api_definition_id = hadr.http_api_definition_id
                    JOIN current_deployments cd ON had.environment_id = cd.environment_id
                    JOIN deployment_revisions dr ON cd.environment_id = dr.environment_id
                        AND cd.current_revision_id = dr.revision_id
                    JOIN deployment_http_api_definition_revisions dhadr
                        ON dhadr.environment_id = dr.environment_id
                            AND dhadr.deployment_revision_id = dr.revision_id
                            AND dhadr.http_api_definition_id = hadr.http_api_definition_id
                            AND dhadr.http_api_definition_revision_id = hadr.revision_id
                    WHERE had.environment_id = $1 AND had.deleted_at IS NULL
                    ORDER BY had.name
                "#})
                .bind(environment_id),
            )
            .await
    }
}

#[async_trait]
trait HttpApiDefinitionRepoInternal: HttpApiDefinitionRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn check_current_revision(
        &self,
        http_api_definition_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<HttpApiDefinitionRecord>>;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<HttpApiDefinitionRevisionRecord>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl HttpApiDefinitionRepoInternal for DbHttpApiDefinitionRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn check_current_revision(
        &self,
        http_api_definition_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<HttpApiDefinitionRecord>> {
        self.with_ro("check_current_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT http_api_definition_id, name, environment_id,
                           created_at, updated_at, deleted_at, modified_by,
                           current_revision_id
                    FROM http_api_definitions
                    WHERE http_api_definition_id = $1 AND current_revision_id = $2 and deleted_at IS NULL
                "#})
                .bind(http_api_definition_id)
                .bind(current_revision_id),
            )
            .await
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: HttpApiDefinitionRevisionRecord,
    ) -> repo::Result<HttpApiDefinitionRevisionRecord> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO http_api_definition_revisions
                (http_api_definition_id, revision_id, version, hash,
                    created_at, created_by, deleted,
                    definition)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING http_api_definition_id, revision_id, version, hash,
                    created_at, created_by, deleted,
                    definition
            "# })
            .bind(revision.http_api_definition_id)
            .bind(revision.revision_id)
            .bind(revision.version)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit)
            .bind(revision.definition),
        )
        .await
    }
}
