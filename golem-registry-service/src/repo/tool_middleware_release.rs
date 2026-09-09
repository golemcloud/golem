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

use crate::repo::model::tool_middleware_release::{
    TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED, TOOL_RELEASE_LIFECYCLE_PUBLISHED,
    TOOL_RELEASE_LIFECYCLE_SUPERSEDED, TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM,
    ToolMiddlewareReleaseRecord, ToolMiddlewareReleaseWithOwnerRecord,
};
use crate::repo::release_grant_lifecycle::{self as lifecycle, ReleaseKind};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::error_forwarding;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{RepoError, ResultExt, SqlDateTime};
use indoc::indoc;
use std::fmt::Debug;
use tracing::{Instrument, info_span};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ToolMiddlewareReleaseRepoError {
    #[error("ToolMiddleware release coordinate already exists")]
    CoordinateAlreadyExists,
    #[error("ToolMiddleware release coordinate exists with different immutable metadata")]
    ImmutableConflict,
    #[error("A de-published tool_middleware release must be restored explicitly")]
    DePublishedConflict,
    #[error("ToolMiddleware release was modified concurrently")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(ToolMiddlewareReleaseRepoError, RepoError);

#[async_trait]
pub trait ToolMiddlewareReleaseRepo: Send + Sync {
    async fn create(
        &self,
        record: ToolMiddlewareReleaseRecord,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseRepoError>;

    async fn get_by_id(
        &self,
        tool_middleware_release_id: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError>;

    async fn get_by_coordinates(
        &self,
        owner_account_id: Uuid,
        name: &str,
        version: &str,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError>;

    async fn strict_following_grant_exists(
        &self,
        tool_middleware_release_id: Uuid,
    ) -> Result<bool, ToolMiddlewareReleaseRepoError>;

    async fn list_by_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError>;

    async fn de_publish(
        &self,
        tool_middleware_release_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError>;

    async fn restore(
        &self,
        tool_middleware_release_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError>;
}

pub struct LoggedToolMiddlewareReleaseRepo<Repo: ToolMiddlewareReleaseRepo> {
    repo: Repo,
}

impl<Repo: ToolMiddlewareReleaseRepo> LoggedToolMiddlewareReleaseRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl<Repo: ToolMiddlewareReleaseRepo> ToolMiddlewareReleaseRepo
    for LoggedToolMiddlewareReleaseRepo<Repo>
{
    async fn create(
        &self,
        record: ToolMiddlewareReleaseRecord,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseRepoError> {
        let span = info_span!("tool_middleware release repository", tool_middleware_release_id = %record.tool_middleware_release_id);
        self.repo.create(record).instrument(span).await
    }

    async fn get_by_id(
        &self,
        tool_middleware_release_id: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        self.repo
            .get_by_id(tool_middleware_release_id)
            .instrument(info_span!("tool_middleware release repository", tool_middleware_release_id = %tool_middleware_release_id))
            .await
    }

    async fn get_by_coordinates(
        &self,
        owner_account_id: Uuid,
        name: &str,
        version: &str,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        self.repo
            .get_by_coordinates(owner_account_id, name, version)
            .instrument(info_span!(
                "tool_middleware release repository",
                owner_account_id = %owner_account_id,
                tool_middleware_name = name,
                middleware_version = version
            ))
            .await
    }

    async fn strict_following_grant_exists(
        &self,
        tool_middleware_release_id: Uuid,
    ) -> Result<bool, ToolMiddlewareReleaseRepoError> {
        self.repo
            .strict_following_grant_exists(tool_middleware_release_id)
            .instrument(info_span!(
                "tool_middleware release repository",
                tool_middleware_release_id = %tool_middleware_release_id
            ))
            .await
    }

    async fn list_by_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        self.repo
            .list_by_owner(owner_account_id)
            .instrument(info_span!("tool_middleware release repository", owner_account_id = %owner_account_id))
            .await
    }

    async fn de_publish(
        &self,
        tool_middleware_release_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        self.repo
            .de_publish(tool_middleware_release_id, actor)
            .instrument(info_span!("tool_middleware release repository", tool_middleware_release_id = %tool_middleware_release_id))
            .await
    }

    async fn restore(
        &self,
        tool_middleware_release_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        self.repo
            .restore(tool_middleware_release_id, actor)
            .instrument(info_span!("tool_middleware release repository", tool_middleware_release_id = %tool_middleware_release_id))
            .await
    }
}

pub struct DbToolMiddlewareReleaseRepo<DBP: Pool> {
    db_pool: DBP,
}

const METRICS_SVC_NAME: &str = "tool_middleware_releases";

impl<DBP: Pool> DbToolMiddlewareReleaseRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedToolMiddlewareReleaseRepo<Self>
    where
        Self: ToolMiddlewareReleaseRepo,
    {
        LoggedToolMiddlewareReleaseRepo::new(Self::new(db_pool))
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

const RELEASE_SELECT: &str = r#"
    SELECT
        tr.tool_middleware_release_id, tr.owner_account_id, tr.tool_middleware_name, tr.middleware_version,
        tr.tool_definition, tr.metadata_version, tr.metadata_digest,
        tr.immutable, tr.lifecycle, tr.origin,
        tr.created_at, tr.created_by, tr.state_changed_at, tr.state_changed_by,
        tr.component_id, tr.component_revision, tr.component_name,
        ar.name AS owner_account_name, a.email AS owner_account_email
    FROM tool_middleware_releases tr
    JOIN accounts a ON a.account_id = tr.owner_account_id
    JOIN account_revisions ar
        ON ar.account_id = a.account_id AND ar.revision_id = a.current_revision_id
"#;

pub(crate) struct ToolMiddlewareReleaseKind;

impl ReleaseKind for ToolMiddlewareReleaseKind {
    const RELEASE_TABLE: &'static str = "tool_middleware_releases";
    const RELEASE_ID: &'static str = "tool_middleware_release_id";
    const NAME: &'static str = "tool_middleware_name";
    const VERSION: &'static str = "middleware_version";
    const GRANT_TABLE: &'static str = "environment_tool_middleware_grants";
    const SELECT: &'static str = RELEASE_SELECT;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbToolMiddlewareReleaseRepo<PostgresPool> {
    pub async fn create_or_restore_within_transaction(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &ToolMiddlewareReleaseRecord,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseRepoError> {
        let inserted = tx
            .execute(
                sqlx::query(indoc! { r#"
                    INSERT INTO tool_middleware_releases (
                        tool_middleware_release_id, owner_account_id, tool_middleware_name, middleware_version,
                        component_id, component_revision, component_name,
                        tool_definition, metadata_version, metadata_digest,
                        immutable, lifecycle, origin,
                        created_at, created_by, state_changed_at, state_changed_by
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13, $14, $15, $16, $17
                    )
                    ON CONFLICT (owner_account_id, tool_middleware_name, middleware_version)
                        WHERE lifecycle != 2 DO NOTHING
                "#})
                .bind(record.tool_middleware_release_id)
                .bind(record.owner_account_id)
                .bind(&record.tool_middleware_name)
                .bind(&record.middleware_version)
                .bind(record.component_id)
                .bind(record.component_revision)
                .bind(&record.component_name)
                .bind(&record.tool_definition)
                .bind(&record.metadata_version)
                .bind(record.metadata_digest)
                .bind(record.immutable)
                .bind(record.lifecycle)
                .bind(record.origin)
                .bind(&record.created_at)
                .bind(record.created_by)
                .bind(&record.state_changed_at)
                .bind(record.state_changed_by),
            )
            .await?;
        let _ = inserted;

        let query = lifecycle::release_by_coordinates::<ToolMiddlewareReleaseKind>(
            TOOL_RELEASE_LIFECYCLE_SUPERSEDED,
        );
        let existing: ToolMiddlewareReleaseWithOwnerRecord = tx
            .fetch_one_as(
                sqlx::query_as(&query)
                    .bind(record.owner_account_id)
                    .bind(&record.tool_middleware_name)
                    .bind(&record.middleware_version),
            )
            .await?;
        let locked = tx
            .execute(
                sqlx::query(&lifecycle::lock_published_release::<
                    ToolMiddlewareReleaseKind,
                >())
                .bind(existing.release.tool_middleware_release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED),
            )
            .await?;
        if locked.rows_affected() != 1 {
            let current_lifecycle: Option<(i16,)> = tx
                .fetch_optional_as(
                    sqlx::query_as(&lifecycle::release_lifecycle::<ToolMiddlewareReleaseKind>())
                        .bind(existing.release.tool_middleware_release_id),
                )
                .await?;
            return match current_lifecycle {
                Some((TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED,)) => {
                    Err(ToolMiddlewareReleaseRepoError::DePublishedConflict)
                }
                _ => Err(ToolMiddlewareReleaseRepoError::ConcurrentModification),
            };
        }
        if !existing.release.publication_content_matches(record) {
            if record.immutable {
                return Err(ToolMiddlewareReleaseRepoError::ImmutableConflict);
            }

            let strict_following_grant_exists = tx
                .fetch_optional(
                    sqlx::query(&lifecycle::strict_following_grant_exists::<
                        ToolMiddlewareReleaseKind,
                    >())
                    .bind(existing.release.tool_middleware_release_id),
                )
                .await?
                .is_some();
            if strict_following_grant_exists {
                return Err(ToolMiddlewareReleaseRepoError::ImmutableConflict);
            }

            let superseded = tx
                .execute(
                    sqlx::query(&lifecycle::supersede_release::<ToolMiddlewareReleaseKind>())
                        .bind(existing.release.tool_middleware_release_id)
                        .bind(TOOL_RELEASE_LIFECYCLE_SUPERSEDED)
                        .bind(&record.state_changed_at)
                        .bind(record.state_changed_by)
                        .bind(TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM),
                )
                .await?;
            if superseded.rows_affected() != 1 {
                return Err(ToolMiddlewareReleaseRepoError::ConcurrentModification);
            }

            tx.execute(
                sqlx::query(indoc! { r#"
                    INSERT INTO tool_middleware_releases (
                        tool_middleware_release_id, owner_account_id, tool_middleware_name, middleware_version,
                        component_id, component_revision, component_name,
                        tool_definition, metadata_version, metadata_digest,
                        immutable, lifecycle, origin,
                        created_at, created_by, state_changed_at, state_changed_by
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13, $14, $15, $16, $17
                    )
                "#})
                .bind(record.tool_middleware_release_id)
                .bind(record.owner_account_id)
                .bind(&record.tool_middleware_name)
                .bind(&record.middleware_version)
                .bind(record.component_id)
                .bind(record.component_revision)
                .bind(&record.component_name)
                .bind(&record.tool_definition)
                .bind(&record.metadata_version)
                .bind(record.metadata_digest)
                .bind(record.immutable)
                .bind(record.lifecycle)
                .bind(record.origin)
                .bind(&record.created_at)
                .bind(record.created_by)
                .bind(&record.state_changed_at)
                .bind(record.state_changed_by),
            )
            .await?;

            tx.execute(
                sqlx::query(&lifecycle::move_following_grants::<ToolMiddlewareReleaseKind>())
                    .bind(existing.release.tool_middleware_release_id)
                    .bind(record.tool_middleware_release_id)
                    .bind(&record.state_changed_at)
                    .bind(record.state_changed_by),
            )
            .await?;

            let query = lifecycle::release_by_id::<ToolMiddlewareReleaseKind>();
            return tx
                .fetch_one_as(sqlx::query_as(&query).bind(record.tool_middleware_release_id))
                .await
                .map_err(Into::into);
        }

        Ok(existing)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ToolMiddlewareReleaseRepo for DbToolMiddlewareReleaseRepo<PostgresPool> {
    async fn create(
        &self,
        record: ToolMiddlewareReleaseRecord,
    ) -> Result<ToolMiddlewareReleaseWithOwnerRecord, ToolMiddlewareReleaseRepoError> {
        let release_id = record.tool_middleware_release_id;
        self.with_tx_err("create", |tx| {
            async move {
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO tool_middleware_releases (
                            tool_middleware_release_id, owner_account_id, tool_middleware_name, middleware_version,
                            component_id, component_revision, component_name,
                            tool_definition, metadata_version, metadata_digest,
                            immutable, lifecycle, origin,
                            created_at, created_by, state_changed_at, state_changed_by
                        )
                        VALUES (
                            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                            $11, $12, $13, $14, $15, $16, $17
                        )
                    "#})
                    .bind(record.tool_middleware_release_id)
                    .bind(record.owner_account_id)
                    .bind(&record.tool_middleware_name)
                    .bind(&record.middleware_version)
                    .bind(record.component_id)
                    .bind(record.component_revision)
                    .bind(&record.component_name)
                    .bind(&record.tool_definition)
                    .bind(&record.metadata_version)
                    .bind(record.metadata_digest)
                    .bind(record.immutable)
                    .bind(record.lifecycle)
                    .bind(record.origin)
                    .bind(&record.created_at)
                    .bind(record.created_by)
                    .bind(&record.state_changed_at)
                    .bind(record.state_changed_by),
                )
                .await
                .to_error_on_unique_violation(ToolMiddlewareReleaseRepoError::CoordinateAlreadyExists)?;

                let query = lifecycle::release_by_id::<ToolMiddlewareReleaseKind>();
                tx.fetch_one_as(sqlx::query_as(&query).bind(release_id))
                    .await
                    .map_err(Into::into)
            }
            .boxed()
        })
        .await
    }

    async fn get_by_id(
        &self,
        tool_middleware_release_id: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        let query = lifecycle::release_by_id::<ToolMiddlewareReleaseKind>();
        Ok(self
            .with_ro("get_by_id")
            .fetch_optional_as(sqlx::query_as(&query).bind(tool_middleware_release_id))
            .await?)
    }

    async fn get_by_coordinates(
        &self,
        owner_account_id: Uuid,
        name: &str,
        version: &str,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        let query = lifecycle::release_by_coordinates::<ToolMiddlewareReleaseKind>(
            TOOL_RELEASE_LIFECYCLE_SUPERSEDED,
        );
        Ok(self
            .with_ro("get_by_coordinates")
            .fetch_optional_as(
                sqlx::query_as(&query)
                    .bind(owner_account_id)
                    .bind(name)
                    .bind(version),
            )
            .await?)
    }

    async fn strict_following_grant_exists(
        &self,
        tool_middleware_release_id: Uuid,
    ) -> Result<bool, ToolMiddlewareReleaseRepoError> {
        Ok(self
            .with_ro("strict_following_grant_exists")
            .fetch_optional(
                sqlx::query(&lifecycle::strict_following_grant_exists::<
                    ToolMiddlewareReleaseKind,
                >())
                .bind(tool_middleware_release_id),
            )
            .await?
            .is_some())
    }

    async fn list_by_owner(
        &self,
        owner_account_id: Uuid,
    ) -> Result<Vec<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        let query = lifecycle::releases_by_owner::<ToolMiddlewareReleaseKind>();
        Ok(self
            .with_ro("list_by_owner")
            .fetch_all_as(sqlx::query_as(&query).bind(owner_account_id))
            .await?)
    }

    async fn de_publish(
        &self,
        tool_middleware_release_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        self.with_tx_err("de_publish", |tx| {
            async move {
                let now = SqlDateTime::now();
                let updated = tx
                    .fetch_optional(
                        sqlx::query(&lifecycle::change_release_lifecycle::<
                            ToolMiddlewareReleaseKind,
                        >(true))
                        .bind(tool_middleware_release_id)
                        .bind(TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED)
                        .bind(&now)
                        .bind(actor)
                        .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED)
                        .bind(TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM),
                    )
                    .await?;
                if updated.is_none() {
                    return Ok(None);
                }

                let query = lifecycle::release_by_id::<ToolMiddlewareReleaseKind>();
                Ok(tx
                    .fetch_optional_as(sqlx::query_as(&query).bind(tool_middleware_release_id))
                    .await?)
            }
            .boxed()
        })
        .await
    }

    async fn restore(
        &self,
        tool_middleware_release_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<ToolMiddlewareReleaseWithOwnerRecord>, ToolMiddlewareReleaseRepoError> {
        let now = SqlDateTime::now();
        let updated = self
            .db_pool
            .with_rw(METRICS_SVC_NAME, "restore")
            .execute(
                sqlx::query(&lifecycle::change_release_lifecycle::<
                    ToolMiddlewareReleaseKind,
                >(false))
                .bind(tool_middleware_release_id)
                .bind(TOOL_RELEASE_LIFECYCLE_PUBLISHED)
                .bind(now)
                .bind(actor)
                .bind(TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED)
                .bind(TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM),
            )
            .await?;
        if updated.rows_affected() == 0 {
            Ok(None)
        } else {
            self.get_by_id(tool_middleware_release_id).await
        }
    }
}
