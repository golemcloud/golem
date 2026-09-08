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

use crate::repo::model::BindFields;
use crate::repo::model::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantRecord, EnvironmentToolMiddlewareGrantWithDetailsRecord,
};
use crate::repo::model::tool_middleware_release::{
    TOOL_RELEASE_LIFECYCLE_PUBLISHED, TOOL_RELEASE_LIFECYCLE_SUPERSEDED,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::error_forwarding;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{
    BindingsStack, PoolLabelledTransaction, RepoError, RepoResult, ResultExt, SqlDateTime,
};
use indoc::indoc;
use tracing::{Instrument, info_span};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentToolMiddlewareGrantRepoError {
    #[error("A grant for this environment and tool_middleware release already exists")]
    GrantAlreadyExists,
    #[error("Environment tool_middleware grant was modified concurrently")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(EnvironmentToolMiddlewareGrantRepoError, RepoError);

#[async_trait]
pub trait EnvironmentToolMiddlewareGrantRepo: Send + Sync {
    async fn create(
        &self,
        record: EnvironmentToolMiddlewareGrantRecord,
    ) -> Result<
        EnvironmentToolMiddlewareGrantWithDetailsRecord,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn get_by_id(
        &self,
        grant_id: Uuid,
        include_deleted: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn get_by_environment_and_release(
        &self,
        environment_id: Uuid,
        release_id: Uuid,
        include_deleted: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<
        Vec<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn get_active_by_release_ids(
        &self,
        environment_id: Uuid,
        release_ids: &[Uuid],
    ) -> Result<
        Vec<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn delete(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic_only: bool,
    ) -> Result<bool, EnvironmentToolMiddlewareGrantRepoError>;

    async fn set_management(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
        automatic: bool,
        follow_coordinates: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn restore(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
        automatic: bool,
        follow_coordinates: Option<bool>,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;

    async fn restore_protected(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    >;
}

pub struct LoggedEnvironmentToolMiddlewareGrantRepo<Repo: EnvironmentToolMiddlewareGrantRepo> {
    repo: Repo,
}

impl<Repo: EnvironmentToolMiddlewareGrantRepo> LoggedEnvironmentToolMiddlewareGrantRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl<Repo: EnvironmentToolMiddlewareGrantRepo> EnvironmentToolMiddlewareGrantRepo
    for LoggedEnvironmentToolMiddlewareGrantRepo<Repo>
{
    async fn create(
        &self,
        record: EnvironmentToolMiddlewareGrantRecord,
    ) -> Result<
        EnvironmentToolMiddlewareGrantWithDetailsRecord,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        let span = info_span!("environment tool_middleware grant repository", grant_id = %record.environment_tool_middleware_grant_id);
        self.repo.create(record).instrument(span).await
    }

    async fn get_by_id(
        &self,
        grant_id: Uuid,
        include_deleted: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .get_by_id(grant_id, include_deleted)
            .instrument(
                info_span!("environment tool_middleware grant repository", grant_id = %grant_id),
            )
            .await
    }

    async fn get_by_environment_and_release(
        &self,
        environment_id: Uuid,
        release_id: Uuid,
        include_deleted: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .get_by_environment_and_release(environment_id, release_id, include_deleted)
            .instrument(info_span!(
                "environment tool_middleware grant repository",
                environment_id = %environment_id,
                release_id = %release_id,
            ))
            .await
    }

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<
        Vec<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .list_by_environment(environment_id)
            .instrument(
                info_span!("environment tool_middleware grant repository", environment_id = %environment_id),
            )
            .await
    }

    async fn get_active_by_release_ids(
        &self,
        environment_id: Uuid,
        release_ids: &[Uuid],
    ) -> Result<
        Vec<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .get_active_by_release_ids(environment_id, release_ids)
            .instrument(
                info_span!("environment tool_middleware grant repository", environment_id = %environment_id),
            )
            .await
    }

    async fn delete(
        &self,
        grant_id: Uuid,
        actor: Uuid,
        automatic_only: bool,
    ) -> Result<bool, EnvironmentToolMiddlewareGrantRepoError> {
        self.repo
            .delete(grant_id, actor, automatic_only)
            .instrument(
                info_span!("environment tool_middleware grant repository", grant_id = %grant_id),
            )
            .await
    }

    async fn set_management(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
        automatic: bool,
        follow_coordinates: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .set_management(
                grant_id,
                environment_id,
                release_id,
                actor,
                automatic,
                follow_coordinates,
            )
            .instrument(
                info_span!("environment tool_middleware grant repository", grant_id = %grant_id),
            )
            .await
    }

    async fn restore(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
        automatic: bool,
        follow_coordinates: Option<bool>,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .restore(
                grant_id,
                environment_id,
                release_id,
                actor,
                automatic,
                follow_coordinates,
            )
            .instrument(
                info_span!("environment tool_middleware grant repository", grant_id = %grant_id),
            )
            .await
    }

    async fn restore_protected(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.repo
            .restore_protected(grant_id, environment_id, release_id, actor)
            .instrument(
                info_span!("environment tool_middleware grant repository", grant_id = %grant_id),
            )
            .await
    }
}

pub struct DbEnvironmentToolMiddlewareGrantRepo<DBP: Pool> {
    db_pool: DBP,
}

const METRICS_SVC_NAME: &str = "environment_tool_middleware_grants";

impl<DBP: Pool> DbEnvironmentToolMiddlewareGrantRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedEnvironmentToolMiddlewareGrantRepo<Self>
    where
        Self: EnvironmentToolMiddlewareGrantRepo,
    {
        LoggedEnvironmentToolMiddlewareGrantRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

impl DbEnvironmentToolMiddlewareGrantRepo<PostgresPool> {
    async fn grantable_release_exists(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        environment_id: Uuid,
        release_id: Uuid,
        follow_coordinates: bool,
    ) -> RepoResult<bool> {
        let release_exists = tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT tool_middleware_release_id
                    FROM tool_middleware_releases
                    WHERE tool_middleware_release_id = $1
                        AND (lifecycle = $2 OR ($3 AND lifecycle = $4))
                    FOR SHARE
                "#})
                .bind(release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED)
                .bind(!follow_coordinates)
                .bind(TOOL_RELEASE_LIFECYCLE_SUPERSEDED),
            )
            .await?
            .is_some();
        if !release_exists {
            return Ok(false);
        }

        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tr.tool_middleware_release_id
                FROM tool_middleware_releases tr
                WHERE tr.tool_middleware_release_id = $2
                    AND (
                        NOT EXISTS (
                            SELECT 1 FROM environments e WHERE e.environment_id = $1
                        )
                        OR EXISTS (
                            SELECT 1
                            FROM environments e
                            JOIN environment_revisions er
                                ON er.environment_id = e.environment_id
                                AND er.revision_id = e.current_revision_id
                            WHERE e.environment_id = $1
                                AND (NOT er.version_check OR tr.immutable)
                        )
                    )
            "#})
                .bind(environment_id)
                .bind(release_id),
            )
            .await?
            .is_some())
    }

    async fn grant_has_available_release(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        follow_coordinates: Option<bool>,
    ) -> RepoResult<bool> {
        let release_exists = tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT tr.tool_middleware_release_id
                    FROM tool_middleware_releases tr
                    JOIN environment_tool_middleware_grants etg
                        ON etg.tool_middleware_release_id = tr.tool_middleware_release_id
                    WHERE etg.environment_tool_middleware_grant_id = $1
                        AND etg.environment_id = $2
                        AND tr.tool_middleware_release_id = $3
                        AND (
                            tr.lifecycle = $4
                            OR (NOT COALESCE($5, etg.follow_coordinates) AND tr.lifecycle = $6)
                        )
                    FOR SHARE OF tr
                "#})
                .bind(grant_id)
                .bind(environment_id)
                .bind(release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED)
                .bind(follow_coordinates)
                .bind(TOOL_RELEASE_LIFECYCLE_SUPERSEDED),
            )
            .await?
            .is_some();
        if !release_exists {
            return Ok(false);
        }

        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tr.tool_middleware_release_id
                FROM tool_middleware_releases tr
                JOIN environment_tool_middleware_grants etg
                    ON etg.tool_middleware_release_id = tr.tool_middleware_release_id
                JOIN environments e ON e.environment_id = etg.environment_id
                JOIN environment_revisions er
                    ON er.environment_id = e.environment_id
                    AND er.revision_id = e.current_revision_id
                WHERE etg.environment_tool_middleware_grant_id = $1
                    AND etg.environment_id = $2
                    AND tr.tool_middleware_release_id = $3
                    AND (NOT er.version_check OR tr.immutable)
            "#})
                .bind(grant_id)
                .bind(environment_id)
                .bind(release_id),
            )
            .await?
            .is_some())
    }
}

impl DbEnvironmentToolMiddlewareGrantRepo<SqlitePool> {
    async fn grantable_release_exists(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        environment_id: Uuid,
        release_id: Uuid,
        follow_coordinates: bool,
    ) -> RepoResult<bool> {
        let release_exists = tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT tool_middleware_release_id
                    FROM tool_middleware_releases
                    WHERE tool_middleware_release_id = $1
                        AND (lifecycle = $2 OR ($3 AND lifecycle = $4))
                "#})
                .bind(release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED)
                .bind(!follow_coordinates)
                .bind(TOOL_RELEASE_LIFECYCLE_SUPERSEDED),
            )
            .await?
            .is_some();
        if !release_exists {
            return Ok(false);
        }

        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tr.tool_middleware_release_id
                FROM tool_middleware_releases tr
                WHERE tr.tool_middleware_release_id = $2
                    AND (
                        NOT EXISTS (
                            SELECT 1 FROM environments e WHERE e.environment_id = $1
                        )
                        OR EXISTS (
                            SELECT 1
                            FROM environments e
                            JOIN environment_revisions er
                                ON er.environment_id = e.environment_id
                                AND er.revision_id = e.current_revision_id
                            WHERE e.environment_id = $1
                                AND (NOT er.version_check OR tr.immutable)
                        )
                    )
            "#})
                .bind(environment_id)
                .bind(release_id),
            )
            .await?
            .is_some())
    }

    async fn grant_has_available_release(
        tx: &mut PoolLabelledTransaction<SqlitePool>,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        follow_coordinates: Option<bool>,
    ) -> RepoResult<bool> {
        let release_exists = tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT tr.tool_middleware_release_id
                    FROM tool_middleware_releases tr
                    JOIN environment_tool_middleware_grants etg
                        ON etg.tool_middleware_release_id = tr.tool_middleware_release_id
                    WHERE etg.environment_tool_middleware_grant_id = $1
                        AND etg.environment_id = $2
                        AND tr.tool_middleware_release_id = $3
                        AND (
                            tr.lifecycle = $4
                            OR (NOT COALESCE($5, etg.follow_coordinates) AND tr.lifecycle = $6)
                        )
                "#})
                .bind(grant_id)
                .bind(environment_id)
                .bind(release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED)
                .bind(follow_coordinates)
                .bind(TOOL_RELEASE_LIFECYCLE_SUPERSEDED),
            )
            .await?
            .is_some();
        if !release_exists {
            return Ok(false);
        }

        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                SELECT tr.tool_middleware_release_id
                FROM tool_middleware_releases tr
                JOIN environment_tool_middleware_grants etg
                    ON etg.tool_middleware_release_id = tr.tool_middleware_release_id
                JOIN environments e ON e.environment_id = etg.environment_id
                JOIN environment_revisions er
                    ON er.environment_id = e.environment_id
                    AND er.revision_id = e.current_revision_id
                WHERE etg.environment_tool_middleware_grant_id = $1
                    AND etg.environment_id = $2
                    AND tr.tool_middleware_release_id = $3
                    AND (NOT er.version_check OR tr.immutable)
            "#})
                .bind(grant_id)
                .bind(environment_id)
                .bind(release_id),
            )
            .await?
            .is_some())
    }
}

const GRANT_DETAILS_SELECT: &str = r#"
    SELECT
        etg.environment_tool_middleware_grant_id, etg.environment_id, etg.protected, etg.automatic,
        etg.follow_coordinates,
        etg.created_at AS grant_created_at, etg.created_by AS grant_created_by,
        etg.state_changed_at AS grant_state_changed_at,
        etg.state_changed_by AS grant_state_changed_by,
        etg.deleted_at AS grant_deleted_at, etg.deleted_by AS grant_deleted_by,
        tr.tool_middleware_release_id, tr.owner_account_id, tr.tool_middleware_name, tr.tool_version,
        tr.source_kind, tr.tool_definition, tr.metadata_version, tr.metadata_digest,
        tr.immutable, tr.lifecycle, tr.origin, tr.system_availability,
        tr.created_at, tr.created_by, tr.state_changed_at, tr.state_changed_by,
        tr.component_id, tr.component_revision, tr.component_name,
        tr.host_tool_id, tr.implementation_version,
        ar.name AS owner_account_name, a.email AS owner_account_email
    FROM environment_tool_middleware_grants etg
    JOIN tool_middleware_releases tr ON tr.tool_middleware_release_id = etg.tool_middleware_release_id
    JOIN accounts a ON a.account_id = tr.owner_account_id
    JOIN account_revisions ar
        ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
"#;

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentToolMiddlewareGrantRepo for DbEnvironmentToolMiddlewareGrantRepo<PostgresPool> {
    async fn create(
        &self,
        record: EnvironmentToolMiddlewareGrantRecord,
    ) -> Result<
        EnvironmentToolMiddlewareGrantWithDetailsRecord,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        let grant_id = record.environment_tool_middleware_grant_id;
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "create", |tx| {
                async move {
                    if !Self::grantable_release_exists(
                        tx,
                        record.environment_id,
                        record.tool_middleware_release_id,
                        record.follow_coordinates,
                    )
                    .await?
                    {
                        return Err(EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification);
                    }
                    tx.execute(
                        sqlx::query(indoc! { r#"
                            INSERT INTO environment_tool_middleware_grants (
                                environment_tool_middleware_grant_id, environment_id, tool_middleware_release_id,
                                protected, automatic, follow_coordinates,
                                state_changed_at, state_changed_by,
                                created_at, created_by, deleted_at, deleted_by
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                        "#})
                        .bind(record.environment_tool_middleware_grant_id)
                        .bind(record.environment_id)
                        .bind(record.tool_middleware_release_id)
                        .bind(record.protected)
                        .bind(record.automatic)
                        .bind(record.follow_coordinates)
                        .bind(record.state_changed_at)
                        .bind(record.state_changed_by)
                        .bind_immutable_audit(record.audit),
                    )
                    .await
                    .to_error_on_unique_violation(
                        EnvironmentToolMiddlewareGrantRepoError::GrantAlreadyExists,
                    )?;

                    let query =
                        format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1");
                    tx.fetch_optional_as(sqlx::query_as(&query).bind(grant_id))
                        .await?
                        .ok_or(EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification)
                }
                .boxed()
            })
            .await
    }

    async fn get_by_id(
        &self,
        grant_id: Uuid,
        include_deleted: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1 AND ($2 OR (etg.deleted_at IS NULL AND tr.lifecycle IN ({TOOL_RELEASE_LIFECYCLE_PUBLISHED}, {TOOL_RELEASE_LIFECYCLE_SUPERSEDED})))"
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
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_id = $1 AND etg.tool_middleware_release_id = $2 AND ($3 OR (etg.deleted_at IS NULL AND tr.lifecycle IN ({TOOL_RELEASE_LIFECYCLE_PUBLISHED}, {TOOL_RELEASE_LIFECYCLE_SUPERSEDED})))"
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
    ) -> Result<
        Vec<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_id = $1 AND etg.deleted_at IS NULL AND tr.lifecycle IN ({TOOL_RELEASE_LIFECYCLE_PUBLISHED}, {TOOL_RELEASE_LIFECYCLE_SUPERSEDED}) ORDER BY tr.tool_middleware_name, tr.tool_version"
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
    ) -> Result<
        Vec<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        if release_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut bindings = BindingsStack::new(2);
        let placeholders = release_ids
            .iter()
            .map(|release_id| format!("${}", bindings.push(*release_id)))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "{GRANT_DETAILS_SELECT} WHERE etg.environment_id = $1 AND etg.tool_middleware_release_id IN ({placeholders}) AND etg.deleted_at IS NULL AND tr.lifecycle IN ({TOOL_RELEASE_LIFECYCLE_PUBLISHED}, {TOOL_RELEASE_LIFECYCLE_SUPERSEDED})"
        );
        let query = bindings.apply(sqlx::query_as(&query).bind(environment_id));
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
    ) -> Result<bool, EnvironmentToolMiddlewareGrantRepoError> {
        let result = self
            .with_rw("delete")
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    UPDATE environment_tool_middleware_grants
                    SET state_changed_at = $2, state_changed_by = $3,
                        deleted_at = $2, deleted_by = $3
                    WHERE environment_tool_middleware_grant_id = $1
                        AND deleted_at IS NULL
                        AND NOT protected
                        AND (NOT $4 OR automatic)
                    RETURNING environment_tool_middleware_grant_id
                "#})
                .bind(grant_id)
                .bind(SqlDateTime::now())
                .bind(actor)
                .bind(automatic_only),
            )
            .await?;
        Ok(result.is_some())
    }

    async fn set_management(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
        automatic: bool,
        follow_coordinates: bool,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "set_management", |tx| {
                async move {
                    if !Self::grantable_release_exists(
                        tx,
                        environment_id,
                        release_id,
                        follow_coordinates,
                    )
                    .await?
                    {
                        return if automatic {
                            Err(EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification)
                        } else {
                            Ok(None)
                        };
                    }
                    let updated = tx
                        .execute(
                            sqlx::query(indoc! { r#"
                                UPDATE environment_tool_middleware_grants
                                SET automatic = $4, follow_coordinates = $5,
                                    state_changed_at = $6, state_changed_by = $7
                                WHERE environment_tool_middleware_grant_id = $1
                                    AND environment_id = $2
                                    AND tool_middleware_release_id = $3
                                    AND deleted_at IS NULL
                                    AND NOT protected
                                    AND (NOT $4 OR automatic)
                            "#})
                            .bind(grant_id)
                            .bind(environment_id)
                            .bind(release_id)
                            .bind(automatic)
                            .bind(follow_coordinates)
                            .bind(SqlDateTime::now())
                            .bind(actor),
                        )
                        .await?;
                    if updated.rows_affected() != 1 {
                        if automatic {
                            let query = format!(
                                "{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1 AND etg.environment_id = $2 AND tr.tool_middleware_release_id = $3 AND etg.deleted_at IS NULL AND NOT etg.protected AND NOT etg.automatic"
                            );
                            return tx
                                .fetch_optional_as(
                                    sqlx::query_as(&query)
                                        .bind(grant_id)
                                        .bind(environment_id)
                                        .bind(release_id),
                                )
                                .await?
                                .map(Some)
                                .ok_or(EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification);
                        }
                        return Ok(None);
                    }
                    let query =
                        format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1");
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
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
        automatic: bool,
        follow_coordinates: Option<bool>,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        self.db_pool
            .with_tx_err(METRICS_SVC_NAME, "restore", |tx| {
                async move {
                    if !Self::grant_has_available_release(
                        tx,
                        grant_id,
                        environment_id,
                        release_id,
                        follow_coordinates,
                    )
                    .await?
                    {
                        return if automatic {
                            Err(EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification)
                        } else {
                            Ok(None)
                        };
                    }
                    let updated = tx
                        .execute(
                            sqlx::query(indoc! { r#"
                                UPDATE environment_tool_middleware_grants
                                SET state_changed_at = $4, state_changed_by = $5,
                                    automatic = $6,
                                    follow_coordinates = COALESCE($7, follow_coordinates),
                                    deleted_at = NULL, deleted_by = NULL
                                WHERE environment_tool_middleware_grant_id = $1
                                    AND environment_id = $2
                                    AND tool_middleware_release_id = $3
                                    AND deleted_at IS NOT NULL
                                    AND NOT protected
                            "#})
                            .bind(grant_id)
                            .bind(environment_id)
                            .bind(release_id)
                            .bind(SqlDateTime::now())
                            .bind(actor)
                            .bind(automatic)
                            .bind(follow_coordinates),
                        )
                        .await?;
                    if updated.rows_affected() != 1 {
                        if automatic {
                            let query = format!(
                                "{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1 AND etg.environment_id = $2 AND tr.tool_middleware_release_id = $3 AND etg.deleted_at IS NULL AND NOT etg.protected AND NOT etg.automatic"
                            );
                            return tx
                                .fetch_optional_as(
                                    sqlx::query_as(&query)
                                        .bind(grant_id)
                                        .bind(environment_id)
                                        .bind(release_id),
                                )
                                .await?
                                .map(Some)
                                .ok_or(EnvironmentToolMiddlewareGrantRepoError::ConcurrentModification);
                        }
                        return Ok(None);
                    }
                    let query =
                        format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1");
                    Ok(tx
                        .fetch_optional_as(sqlx::query_as(&query).bind(grant_id))
                        .await?)
                }
                .boxed()
            })
            .await
    }

    async fn restore_protected(
        &self,
        grant_id: Uuid,
        environment_id: Uuid,
        release_id: Uuid,
        actor: Uuid,
    ) -> Result<
        Option<EnvironmentToolMiddlewareGrantWithDetailsRecord>,
        EnvironmentToolMiddlewareGrantRepoError,
    > {
        let now = SqlDateTime::now();
        let updated = self
            .with_rw("restore_protected")
            .execute(
                sqlx::query(indoc! { r#"
                    UPDATE environment_tool_middleware_grants
                    SET state_changed_at = $4, state_changed_by = $5,
                        deleted_at = NULL, deleted_by = NULL
                    WHERE environment_tool_middleware_grant_id = $1
                        AND environment_id = $2
                        AND tool_middleware_release_id = $3
                        AND deleted_at IS NOT NULL
                        AND protected
                "#})
                .bind(grant_id)
                .bind(environment_id)
                .bind(release_id)
                .bind(now)
                .bind(actor),
            )
            .await?;
        if updated.rows_affected() != 1 {
            return Ok(None);
        }
        let query =
            format!("{GRANT_DETAILS_SELECT} WHERE etg.environment_tool_middleware_grant_id = $1");
        Ok(self
            .with_ro("restore_protected")
            .fetch_optional_as(sqlx::query_as(&query).bind(grant_id))
            .await?)
    }
}
