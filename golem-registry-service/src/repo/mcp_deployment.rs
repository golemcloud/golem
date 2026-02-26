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

use crate::repo::model::mcp_deployment::{
    McpDeploymentExtRevisionRecord, McpDeploymentRepoError, McpDeploymentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult, ResultExt};
use golem_service_base::repo::blob::Blob;
use indoc::indoc;
use std::fmt::Debug;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait McpDeploymentRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: Uuid,
        domain: &str,
        revision: McpDeploymentRevisionRecord,
    ) -> Result<McpDeploymentExtRevisionRecord, McpDeploymentRepoError>;

    async fn update(
        &self,
        revision: McpDeploymentRevisionRecord,
    ) -> Result<McpDeploymentExtRevisionRecord, McpDeploymentRepoError>;

    async fn delete(
        &self,
        user_account_id: Uuid,
        mcp_deployment_id: Uuid,
        revision_id: i64,
    ) -> Result<(), McpDeploymentRepoError>;

    async fn get_staged_by_id(
        &self,
        mcp_deployment_id: Uuid,
    ) -> RepoResult<Option<McpDeploymentExtRevisionRecord>>;

    async fn get_staged_by_domain(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> RepoResult<Option<McpDeploymentExtRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<McpDeploymentExtRevisionRecord>>;
}

pub struct LoggedMcpDeploymentRepo<Repo: McpDeploymentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "mcp_deployment repository";

impl<Repo: McpDeploymentRepo> LoggedMcpDeploymentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span(span_name: &'static str) -> Span {
        info_span!(SPAN_NAME, span = span_name)
    }

    fn span_env(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }
}

#[async_trait]
impl<Repo: McpDeploymentRepo> McpDeploymentRepo for LoggedMcpDeploymentRepo<Repo> {
    async fn create(
        &self,
        environment_id: Uuid,
        domain: &str,
        revision: McpDeploymentRevisionRecord,
    ) -> Result<McpDeploymentExtRevisionRecord, McpDeploymentRepoError> {
        self.repo
            .create(environment_id, domain, revision)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn update(
        &self,
        revision: McpDeploymentRevisionRecord,
    ) -> Result<McpDeploymentExtRevisionRecord, McpDeploymentRepoError> {
        self.repo
            .update(revision)
            .instrument(Self::span("update"))
            .await
    }

    async fn delete(
        &self,
        user_account_id: Uuid,
        mcp_deployment_id: Uuid,
        revision_id: i64,
    ) -> Result<(), McpDeploymentRepoError> {
        self.repo
            .delete(user_account_id, mcp_deployment_id, revision_id)
            .instrument(Self::span("delete"))
            .await
    }

    async fn get_staged_by_id(
        &self,
        mcp_deployment_id: Uuid,
    ) -> RepoResult<Option<McpDeploymentExtRevisionRecord>> {
        self.repo
            .get_staged_by_id(mcp_deployment_id)
            .instrument(Self::span("get_staged_by_id"))
            .await
    }

    async fn get_staged_by_domain(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> RepoResult<Option<McpDeploymentExtRevisionRecord>> {
        self.repo
            .get_staged_by_domain(environment_id, domain)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<McpDeploymentExtRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }
}

pub struct DbMcpDeploymentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "mcp_deployment_repo";

impl<DBP: Pool> DbMcpDeploymentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedMcpDeploymentRepo<Self>
    where
        Self: McpDeploymentRepo,
    {
        LoggedMcpDeploymentRepo::new(Self::new(db_pool))
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
impl McpDeploymentRepo for DbMcpDeploymentRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: Uuid,
        domain: &str,
        revision: McpDeploymentRevisionRecord,
    ) -> Result<McpDeploymentExtRevisionRecord, McpDeploymentRepoError> {
        let domain = domain.to_owned();

        self.with_tx_err("create", |tx| {
            async move {
                tx
                    .execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO mcp_deployments
                            (mcp_deployment_id, environment_id, domain, created_at, deleted_at, modified_by, current_revision_id)
                            VALUES ($1, $2, $3, $4, NULL, $5, 0)
                        "# })
                        .bind(revision.mcp_deployment_id)
                        .bind(environment_id)
                        .bind(&domain)
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by),
                    )
                    .await
                    .to_error_on_unique_violation(McpDeploymentRepoError::McpDeploymentViolatesUniqueness)?;

                let revision = revision.with_updated_hash();

                let revision: McpDeploymentRevisionRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            INSERT INTO mcp_deployment_revisions
                            (mcp_deployment_id, revision_id, hash, domain, data, created_at, created_by, deleted)
                            VALUES ($1, $2, $3, $4, $5, $6, $7, false)
                            RETURNING mcp_deployment_id, revision_id, hash, domain, data, created_at, created_by, deleted
                        "# })
                        .bind(revision.mcp_deployment_id)
                        .bind(revision.revision_id)
                        .bind(revision.hash)
                        .bind(&revision.domain)
                        .bind(&revision.data)
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by),
                    )
                    .await?;

                Ok(McpDeploymentExtRevisionRecord {
                    environment_id,
                    domain: revision.domain.clone(),
                    entity_created_at: revision.audit.created_at.clone(),
                    revision,
                })
            }
            .boxed()
        })
        .await
    }

    async fn update(
        &self,
        revision: McpDeploymentRevisionRecord,
    ) -> Result<McpDeploymentExtRevisionRecord, McpDeploymentRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let revision = revision.with_updated_hash();

                let revision: McpDeploymentRevisionRecord = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            INSERT INTO mcp_deployment_revisions
                            (mcp_deployment_id, revision_id, hash, domain, data, created_at, created_by, deleted)
                            VALUES ($1, $2, $3, $4, $5, $6, $7, false)
                            RETURNING mcp_deployment_id, revision_id, hash, domain, data, created_at, created_by, deleted
                        "# })
                        .bind(revision.mcp_deployment_id)
                        .bind(revision.revision_id)
                        .bind(revision.hash)
                        .bind(&revision.domain)
                        .bind(&revision.data)
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by),
                    )
                    .await?;

                // Fetch environment_id from mcp_deployments
                let mcp_deployment: (Uuid,) = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            UPDATE mcp_deployments
                            SET current_revision_id = $1, deleted_at = NULL
                            WHERE mcp_deployment_id = $2
                            RETURNING environment_id
                        "# })
                        .bind(revision.revision_id)
                        .bind(revision.mcp_deployment_id),
                    )
                    .await?;

                Ok(McpDeploymentExtRevisionRecord {
                    environment_id: mcp_deployment.0,
                    domain: revision.domain.clone(),
                    entity_created_at: revision.audit.created_at.clone(),
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
        mcp_deployment_id: Uuid,
        revision_id: i64,
    ) -> Result<(), McpDeploymentRepoError> {
        self.with_tx_err("delete", |tx| {
            async move {
                // Check that the current revision matches the provided revision
                let current_revision: (i64,) = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            SELECT current_revision_id
                            FROM mcp_deployments
                            WHERE mcp_deployment_id = $1
                        "# })
                        .bind(mcp_deployment_id),
                    )
                    .await?;

                if current_revision.0 != revision_id {
                    return Err(McpDeploymentRepoError::ConcurrentModification);
                }

                // Get the current domain from the current revision
                let current_domain: (String,) = tx
                    .fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            SELECT domain
                            FROM mcp_deployment_revisions
                            WHERE mcp_deployment_id = $1 AND revision_id = $2
                        "# })
                        .bind(mcp_deployment_id)
                        .bind(revision_id),
                    )
                    .await?;

                // Insert a deletion revision
                let deletion_data = Blob::new(crate::repo::model::mcp_deployment::McpDeploymentData { agents: Default::default() });
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO mcp_deployment_revisions
                        (mcp_deployment_id, revision_id, hash, domain, data, created_at, created_by, deleted)
                        VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP, $6, true)
                    "# })
                    .bind(mcp_deployment_id)
                    .bind(revision_id + 1)
                    .bind(crate::repo::model::hash::SqlBlake3Hash::empty())
                    .bind(&current_domain.0)
                    .bind(&deletion_data)
                    .bind(user_account_id),
                )
                .await?;

                // Update the main table to point to the deletion revision
                tx.execute(
                    sqlx::query(indoc! { r#"
                        UPDATE mcp_deployments
                        SET deleted_at = CURRENT_TIMESTAMP, modified_by = $1, current_revision_id = $2
                        WHERE mcp_deployment_id = $3
                    "# })
                    .bind(user_account_id)
                    .bind(revision_id + 1)
                    .bind(mcp_deployment_id),
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
        mcp_deployment_id: Uuid,
    ) -> RepoResult<Option<McpDeploymentExtRevisionRecord>> {
        self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT 
                        m.mcp_deployment_id,
                        m.environment_id,
                        mr.revision_id,
                        mr.hash,
                        mr.created_at,
                        mr.created_by,
                        mr.deleted,
                        mr.domain,
                        m.created_at as entity_created_at
                    FROM mcp_deployments m
                    JOIN mcp_deployment_revisions mr
                        ON m.mcp_deployment_id = mr.mcp_deployment_id
                            AND m.current_revision_id = mr.revision_id
                    WHERE m.mcp_deployment_id = $1 AND mr.deleted = false
                "# })
                .bind(mcp_deployment_id),
            )
            .await
    }

    async fn get_staged_by_domain(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> RepoResult<Option<McpDeploymentExtRevisionRecord>> {
        self.with_ro("get_staged_by_domain")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT 
                        m.mcp_deployment_id,
                        m.environment_id,
                        mr.revision_id,
                        mr.hash,
                        mr.created_at,
                        mr.created_by,
                        mr.deleted,
                        mr.domain,
                        m.created_at as entity_created_at
                    FROM mcp_deployments m
                    JOIN mcp_deployment_revisions mr
                        ON m.mcp_deployment_id = mr.mcp_deployment_id
                            AND m.current_revision_id = mr.revision_id
                    WHERE m.environment_id = $1 AND mr.domain = $2 AND mr.deleted = false
                "# })
                .bind(environment_id)
                .bind(domain),
            )
            .await
    }

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<McpDeploymentExtRevisionRecord>> {
        self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT 
                        m.mcp_deployment_id,
                        m.environment_id,
                        mr.revision_id,
                        mr.hash,
                        mr.created_at,
                        mr.created_by,
                        mr.deleted,
                        mr.domain,
                        mr.data,
                        m.created_at as entity_created_at
                    FROM mcp_deployments m
                    JOIN mcp_deployment_revisions mr
                        ON m.mcp_deployment_id = mr.mcp_deployment_id
                            AND m.current_revision_id = mr.revision_id
                    WHERE m.environment_id = $1 AND mr.deleted = false
                "# })
                .bind(environment_id),
            )
            .await
    }
}
