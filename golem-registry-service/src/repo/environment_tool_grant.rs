// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::repo::model::environment_tool_grant::{
    ENVIRONMENT_TOOL_GRANT_LIFECYCLE_ACTIVE, ENVIRONMENT_TOOL_GRANT_LIFECYCLE_DELETED,
    EnvironmentToolGrantRecord, EnvironmentToolGrantWithDetailsRecord,
};
use crate::repo::model::tool_release::TOOL_RELEASE_LIFECYCLE_PUBLISHED;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::error_forwarding;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{
    PoolLabelledTransaction, RepoError, RepoResult, ResultExt, SqlDateTime,
};
use indoc::indoc;
use tracing::{Instrument, info_span};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentToolGrantRepoError {
    #[error("A grant for this environment and tool release already exists")]
    GrantAlreadyExists,
    #[error("Environment tool grant was modified concurrently")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(EnvironmentToolGrantRepoError, RepoError);

#[async_trait]
pub trait EnvironmentToolGrantRepo: Send + Sync {
    async fn create(
        &self,
        record: EnvironmentToolGrantRecord,
    ) -> Result<EnvironmentToolGrantWithDetailsRecord, EnvironmentToolGrantRepoError>;

    async fn get_by_id(
        &self,
        grant_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError>;

    async fn get_by_environment_and_release(
        &self,
        environment_id: Uuid,
        release_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError>;

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError>;

    async fn get_active_by_release_ids(
        &self,
        environment_id: Uuid,
        release_ids: &[Uuid],
    ) -> Result<Vec<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError>;

    async fn delete(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic_only: bool,
    ) -> Result<bool, EnvironmentToolGrantRepoError>;

    async fn set_automatic(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError>;

    async fn restore(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError>;
}

pub struct LoggedEnvironmentToolGrantRepo<Repo: EnvironmentToolGrantRepo> {
    repo: Repo,
}

impl<Repo: EnvironmentToolGrantRepo> LoggedEnvironmentToolGrantRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl<Repo: EnvironmentToolGrantRepo> EnvironmentToolGrantRepo
    for LoggedEnvironmentToolGrantRepo<Repo>
{
    async fn create(
        &self,
        record: EnvironmentToolGrantRecord,
    ) -> Result<EnvironmentToolGrantWithDetailsRecord, EnvironmentToolGrantRepoError> {
        let span = info_span!("environment tool grant repository", grant_id = %record.environment_tool_grant_id);
        self.repo.create(record).instrument(span).await
    }

    async fn get_by_id(
        &self,
        grant_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.repo
            .get_by_id(grant_id, include_deleted)
            .instrument(info_span!("environment tool grant repository", grant_id = %grant_id))
            .await
    }

    async fn get_by_environment_and_release(
        &self,
        environment_id: Uuid,
        release_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.repo
            .get_by_environment_and_release(environment_id, release_id, include_deleted)
            .instrument(info_span!(
                "environment tool grant repository",
                environment_id = %environment_id,
                release_id = %release_id,
            ))
            .await
    }

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.repo
            .list_by_environment(environment_id)
            .instrument(
                info_span!("environment tool grant repository", environment_id = %environment_id),
            )
            .await
    }

    async fn get_active_by_release_ids(
        &self,
        environment_id: Uuid,
        release_ids: &[Uuid],
    ) -> Result<Vec<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.repo
            .get_active_by_release_ids(environment_id, release_ids)
            .instrument(
                info_span!("environment tool grant repository", environment_id = %environment_id),
            )
            .await
    }

    async fn delete(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic_only: bool,
    ) -> Result<bool, EnvironmentToolGrantRepoError> {
        self.repo
            .delete(grant_id, actor, automatic_only)
            .instrument(info_span!("environment tool grant repository", grant_id = %grant_id))
            .await
    }

    async fn set_automatic(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.repo
            .set_automatic(grant_id, actor, automatic)
            .instrument(info_span!("environment tool grant repository", grant_id = %grant_id))
            .await
    }

    async fn restore(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.repo
            .restore(grant_id, actor, automatic)
            .instrument(info_span!("environment tool grant repository", grant_id = %grant_id))
            .await
    }
}

pub struct DbEnvironmentToolGrantRepo<DBP: Pool> {
    db_pool: DBP,
}

const METRICS_SVC_NAME: &str = "environment_tool_grants";

impl<DBP: Pool> DbEnvironmentToolGrantRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedEnvironmentToolGrantRepo<Self>
    where
        Self: EnvironmentToolGrantRepo,
    {
        LoggedEnvironmentToolGrantRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

impl DbEnvironmentToolGrantRepo<PostgresPool> {
    async fn published_release_exists(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        release_id: Uuid,
    ) -> RepoResult<bool> {
        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tool_release_id
                FROM tool_releases
                WHERE tool_release_id = $1 AND lifecycle = $2
                FOR SHARE
            "#})
                .bind(release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED),
            )
            .await?
            .is_some())
    }

    async fn grant_has_published_release(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        grant_id: Uuid,
    ) -> RepoResult<bool> {
        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tr.tool_release_id
                FROM tool_releases tr
                JOIN environment_tool_grants etg
                    ON etg.tool_release_id = tr.tool_release_id
                WHERE etg.environment_tool_grant_id = $1 AND tr.lifecycle = $2
                FOR SHARE OF tr
            "#})
                .bind(grant_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED),
            )
            .await?
            .is_some())
    }
}

impl DbEnvironmentToolGrantRepo<SqlitePool> {
    async fn published_release_exists(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        release_id: Uuid,
    ) -> RepoResult<bool> {
        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tool_release_id
                FROM tool_releases
                WHERE tool_release_id = $1 AND lifecycle = $2
            "#})
                .bind(release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED),
            )
            .await?
            .is_some())
    }

    async fn grant_has_published_release(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        grant_id: Uuid,
    ) -> RepoResult<bool> {
        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tr.tool_release_id
                FROM tool_releases tr
                JOIN environment_tool_grants etg
                    ON etg.tool_release_id = tr.tool_release_id
                WHERE etg.environment_tool_grant_id = $1 AND tr.lifecycle = $2
            "#})
                .bind(grant_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED),
            )
            .await?
            .is_some())
    }
}

const GRANT_DETAILS_SELECT: &str = r#"
    SELECT
        etg.environment_tool_grant_id, etg.environment_id, etg.protected, etg.automatic,
        etg.lifecycle AS grant_lifecycle,
        etg.created_at AS grant_created_at, etg.created_by AS grant_created_by,
        etg.state_changed_at AS grant_state_changed_at,
        etg.state_changed_by AS grant_state_changed_by,
        etg.deleted_at AS grant_deleted_at, etg.deleted_by AS grant_deleted_by,
        tr.tool_release_id, tr.owner_account_id, tr.tool_name, tr.tool_version,
        tr.source_kind, tr.tool_definition, tr.metadata_version, tr.metadata_digest,
        tr.lifecycle, tr.origin, tr.system_availability,
        tr.created_at, tr.created_by, tr.state_changed_at, tr.state_changed_by,
        tr.component_id, tr.component_revision, tr.component_name,
        tr.host_tool_id, tr.implementation_version,
        ar.name AS owner_account_name, a.email AS owner_account_email
    FROM environment_tool_grants etg
    JOIN tool_releases tr ON tr.tool_release_id = etg.tool_release_id
    JOIN accounts a ON a.account_id = tr.owner_account_id
    JOIN account_revisions ar
        ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
"#;

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentToolGrantRepo for DbEnvironmentToolGrantRepo<PostgresPool> {
    async fn create(
        &self,
        record: EnvironmentToolGrantRecord,
    ) -> Result<EnvironmentToolGrantWithDetailsRecord, EnvironmentToolGrantRepoError> {
        let grant_id = record.environment_tool_grant_id;
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                async move {
                    if !Self::published_release_exists(tx, record.tool_release_id).await? {
                        return Err(EnvironmentToolGrantRepoError::ConcurrentModification);
                    }
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO environment_tool_grants (
                                environment_tool_grant_id, environment_id, tool_release_id,
                                protected, automatic, lifecycle, created_at, created_by,
                                state_changed_at, state_changed_by, deleted_at, deleted_by
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL, NULL)
                        "#})
                        .bind(record.environment_tool_grant_id)
                        .bind(record.environment_id)
                        .bind(record.tool_release_id)
                        .bind(record.protected)
                        .bind(record.automatic)
                        .bind(record.lifecycle)
                        .bind(record.created_at)
                        .bind(record.created_by)
                        .bind(record.state_changed_at)
                        .bind(record.state_changed_by),
                    )
                    .await
                    .to_error_on_unique_violation(
                        EnvironmentToolGrantRepoError::GrantAlreadyExists,
                    )?;

                    let query =
                        format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_grant_id = $1");
                    tx.fetch_optional_as(sqlx::query_as(&query).bind(grant_id))
                        .await?
                        .ok_or(EnvironmentToolGrantRepoError::ConcurrentModification)
                }
                .boxed()
            })
            .await
    }

    async fn get_by_id(
        &self,
        grant_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_grant_id = $1 AND ($2 OR (etg.lifecycle = {ENVIRONMENT_TOOL_GRANT_LIFECYCLE_ACTIVE} AND tr.lifecycle = {TOOL_RELEASE_LIFECYCLE_PUBLISHED}))"
        );
        Ok(self
            .with_ro("get_by_id")
            .fetch_optional_as(sqlx::query_as(&query).bind(grant_id).bind(include_deleted))
            .await?)
    }

    async fn get_by_environment_and_release(
        &self,
        environment_id: Uuid,
        release_id: Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_id = $1 AND etg.tool_release_id = $2 AND ($3 OR (etg.lifecycle = {ENVIRONMENT_TOOL_GRANT_LIFECYCLE_ACTIVE} AND tr.lifecycle = {TOOL_RELEASE_LIFECYCLE_PUBLISHED}))"
        );
        Ok(self
            .with_ro("get_by_environment_and_release")
            .fetch_optional_as(
                sqlx::query_as(&query)
                    .bind(environment_id)
                    .bind(release_id)
                    .bind(include_deleted),
            )
            .await?)
    }

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_id = $1 AND etg.deleted_at IS NULL AND tr.lifecycle = {TOOL_RELEASE_LIFECYCLE_PUBLISHED} ORDER BY tr.tool_name, tr.tool_version"
        );
        Ok(self
            .with_ro("list_by_environment")
            .fetch_all_as(sqlx::query_as(&query).bind(environment_id))
            .await?)
    }

    async fn get_active_by_release_ids(
        &self,
        environment_id: Uuid,
        release_ids: &[Uuid],
    ) -> Result<Vec<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        if release_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..release_ids.len())
            .map(|index| format!("${}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_id = $1 AND etg.tool_release_id IN ({placeholders}) AND etg.deleted_at IS NULL AND tr.lifecycle = {TOOL_RELEASE_LIFECYCLE_PUBLISHED}"
        );
        let mut query = sqlx::query_as(&query).bind(environment_id);
        for release_id in release_ids {
            query = query.bind(*release_id);
        }
        Ok(self
            .with_ro("get_active_by_release_ids")
            .fetch_all_as(query)
            .await?)
    }

    async fn delete(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic_only: bool,
    ) -> Result<bool, EnvironmentToolGrantRepoError> {
        let result = self
            .with_rw("delete")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    UPDATE environment_tool_grants
                    SET lifecycle = $2, state_changed_at = $3, state_changed_by = $4,
                        deleted_at = $3, deleted_by = $4
                    WHERE environment_tool_grant_id = $1
                        AND lifecycle = $5
                        AND NOT protected
                        AND (NOT $6 OR automatic)
                    RETURNING environment_tool_grant_id
                "#})
                .bind(grant_id)
                .bind(ENVIRONMENT_TOOL_GRANT_LIFECYCLE_DELETED)
                .bind(SqlDateTime::now())
                .bind(actor)
                .bind(ENVIRONMENT_TOOL_GRANT_LIFECYCLE_ACTIVE)
                .bind(automatic_only),
            )
            .await?;
        Ok(result.is_some())
    }

    async fn set_automatic(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "set_automatic", |tx| {
                async move {
                    let updated = tx
                        .execute(
                            sqlx::query(indoc! { r#"
                                UPDATE environment_tool_grants
                                SET automatic = $2, state_changed_at = $3, state_changed_by = $4
                                WHERE environment_tool_grant_id = $1
                                    AND lifecycle = $5
                                    AND NOT protected
                            "#})
                            .bind(grant_id)
                            .bind(automatic)
                            .bind(SqlDateTime::now())
                            .bind(actor)
                            .bind(ENVIRONMENT_TOOL_GRANT_LIFECYCLE_ACTIVE),
                        )
                        .await?;
                    if updated.rows_affected() != 1 {
                        return Ok(None);
                    }
                    let query =
                        format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_grant_id = $1");
                    Ok(tx
                        .fetch_optional_as(sqlx::query_as(&query).bind(grant_id))
                        .await?)
                }
                .boxed()
            })
            .await
    }

    async fn restore(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic: bool,
    ) -> Result<Option<EnvironmentToolGrantWithDetailsRecord>, EnvironmentToolGrantRepoError> {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "restore", |tx| {
                async move {
                    if !Self::grant_has_published_release(tx, grant_id).await? {
                        return Ok(None);
                    }
                    let updated = tx
                        .execute(
                            sqlx::query(indoc! { r#"
                                UPDATE environment_tool_grants
                                SET lifecycle = $2, state_changed_at = $3, state_changed_by = $4,
                                    automatic = $5, deleted_at = NULL, deleted_by = NULL
                                WHERE environment_tool_grant_id = $1
                                    AND lifecycle = $6
                                    AND NOT protected
                            "#})
                            .bind(grant_id)
                            .bind(ENVIRONMENT_TOOL_GRANT_LIFECYCLE_ACTIVE)
                            .bind(SqlDateTime::now())
                            .bind(actor)
                            .bind(automatic)
                            .bind(ENVIRONMENT_TOOL_GRANT_LIFECYCLE_DELETED),
                        )
                        .await?;
                    if updated.rows_affected() != 1 {
                        return Ok(None);
                    }
                    let query =
                        format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_grant_id = $1");
                    Ok(tx
                        .fetch_optional_as(sqlx::query_as(&query).bind(grant_id))
                        .await?)
                }
                .boxed()
            })
            .await
    }
}
