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
use crate::repo::model::component::{
    ComponentExtRevisionRecord, ComponentFileRecord, ComponentPluginInstallationRecord,
    ComponentRepoError, ComponentRevisionIdentityRecord, ComponentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt, TryStreamExt, stream};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult, ResultExt};
use indoc::indoc;
use sqlx::{Database, Row};
use std::fmt::Debug;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait ComponentRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
        version_check: bool,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError>;

    async fn update(
        &self,
        revision: ComponentRevisionRecord,
        version_check: bool,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError>;

    async fn delete(
        &self,
        user_account_id: &Uuid,
        component_id: &Uuid,
        revision_id: i64,
    ) -> Result<(), ComponentRepoError>;

    async fn get_staged_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn get_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn get_all_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn get_by_id_and_revision(
        &self,
        component_id: &Uuid,
        revision_id: i64,
        include_deleted: bool,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn list_by_deployment(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn get_by_deployment_and_name(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;
}

pub struct LoggedComponentRepo<Repo: ComponentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "component repository";

impl<Repo: ComponentRepo> LoggedComponentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(environment_id: &Uuid, name: &str) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, name)
    }

    fn span_id(component_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, component_id = %component_id)
    }

    fn span_id_and_revision(component_id: &Uuid, revision_id: i64) -> Span {
        info_span!(SPAN_NAME, component_id = %component_id, revision_id)
    }

    fn span_env(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_env_and_deployment_revision(
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, deployment_revision_id)
    }

    fn span_env_and_deployment_revision_and_name(
        environment_id: &Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, deployment_revision_id, name)
    }
}

#[async_trait]
impl<Repo: ComponentRepo> ComponentRepo for LoggedComponentRepo<Repo> {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
        version_check: bool,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        self.repo
            .create(environment_id, name, revision, version_check)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn update(
        &self,
        revision: ComponentRevisionRecord,
        version_check: bool,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        let span = Self::span_id(&revision.component_id);
        self.repo
            .update(revision, version_check)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        component_id: &Uuid,
        revision_id: i64,
    ) -> Result<(), ComponentRepoError> {
        self.repo
            .delete(user_account_id, component_id, revision_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_staged_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_staged_by_id(component_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_staged_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn get_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_deployed_by_id(component_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_deployed_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn get_all_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        self.repo
            .get_all_deployed_by_id(component_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_by_id_and_revision(
        &self,
        component_id: &Uuid,
        revision_id: i64,
        include_deleted: bool,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_by_id_and_revision(component_id, revision_id, include_deleted)
            .instrument(Self::span_id_and_revision(component_id, revision_id))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        self.repo
            .list_deployed(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        self.repo
            .list_by_deployment(environment_id, deployment_revision_id)
            .instrument(Self::span_env_and_deployment_revision(
                environment_id,
                deployment_revision_id,
            ))
            .await
    }

    async fn get_by_deployment_and_name(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_by_deployment_and_name(environment_id, deployment_revision_id, name)
            .instrument(Self::span_env_and_deployment_revision_and_name(
                environment_id,
                deployment_revision_id,
                name,
            ))
            .await
    }
}

pub struct DbComponentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "component";

impl<DBP: Pool> DbComponentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedComponentRepo<Self>
    where
        Self: ComponentRepo,
    {
        LoggedComponentRepo::new(Self::new(db_pool))
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
impl ComponentRepo for DbComponentRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
        version_check: bool,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        let opt_deleted_revision: Option<ComponentRevisionIdentityRecord> = self.with_ro("create - get opt deleted").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT c.component_id, c.name, cr.revision_id, cr.revision_id, cr.version, cr.hash
                FROM components c
                JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                WHERE c.environment_id = $1 AND c.name = $2 AND c.deleted_at IS NOT NULL
            "#})
                .bind(environment_id)
                .bind(name)
        ).await?;

        if let Some(deleted_revision) = opt_deleted_revision {
            let recreated_revision = revision
                .for_recreation(deleted_revision.component_id, deleted_revision.revision_id)?;
            return self.update(recreated_revision, version_check).await;
        }

        let environment_id = *environment_id;
        let name = name.to_owned();

        self.with_tx_err("create", |tx| {
            async move {
                tx.execute(
                    sqlx::query(indoc! { r#"
                        INSERT INTO components
                        (component_id, name, environment_id,
                            created_at, updated_at, deleted_at, modified_by,
                            current_revision_id)
                        VALUES ($1, $2, $3, $4, $5, NULL, $6, 0)
                    "# })
                    .bind(revision.component_id)
                    .bind(&name)
                    .bind(environment_id)
                    .bind(&revision.audit.created_at)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by),
                )
                .await
                .to_error_on_unique_violation(ComponentRepoError::ComponentViolatesUniqueness)?;

                let revision = Self::insert_revision(tx, version_check, revision).await?;

                Ok(ComponentExtRevisionRecord {
                    name,
                    environment_id,
                    revision,
                })
            }
            .boxed()
        })
        .await
    }

    async fn update(
        &self,
        revision: ComponentRevisionRecord,
        version_check: bool,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let revision: ComponentRevisionRecord = Self::insert_revision(
                    tx,
                    version_check,
                    revision,
                )
                .await?;

                let ext = tx
                    .fetch_one(
                        sqlx::query(indoc! { r#"
                            UPDATE components
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3, deleted_at = NULL
                            WHERE component_id = $4
                            RETURNING name, environment_id
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.component_id),
                    )
                    .await?;

                Ok(ComponentExtRevisionRecord {
                    name: ext.try_get("name").map_err(RepoError::from)?,
                    environment_id: ext.try_get("environment_id").map_err(RepoError::from)?,
                    revision,
                })
            }
            .boxed()
        })
        .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        component_id: &Uuid,
        revision_id: i64,
    ) -> Result<(), ComponentRepoError> {
        let user_account_id = *user_account_id;
        let component_id = *component_id;

        self.with_tx_err("delete", |tx| {
            async move {
                let revision: ComponentRevisionRecord = Self::insert_revision(
                    tx,
                    false,
                    ComponentRevisionRecord::deletion(user_account_id, component_id, revision_id),
                )
                .await?;

                tx.execute(
                    sqlx::query(indoc! { r#"
                        UPDATE components
                        SET deleted_at = $1, modified_by = $2, current_revision_id = $3
                        WHERE component_id = $4
                    "#})
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.component_id),
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
        component_id: &Uuid,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision: Option<ComponentExtRevisionRecord> = self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.component_id = $1 AND c.deleted_at IS NULL
                "#})
                    .bind(component_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.enrich_component(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND c.name = $2 AND c.deleted_at IS NULL
                "#})
                    .bind(environment_id)
                    .bind(name),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.enrich_component(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                        cr.component_id, cr.revision_id, cr.version, cr.hash,
                        cr.created_at, cr.created_by, cr.deleted,
                        cr.size, cr.metadata, cr.original_env, cr.env,
                        cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM current_deployments cd
                    JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id AND cdr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id AND dcr.deployment_revision_id = cdr.deployment_revision_id
                    JOIN component_revisions cr
                        ON cr.component_id = dcr.component_id AND cr.revision_id = dcr.component_revision_id
                    JOIN components c
                        ON c.component_id  = cr.component_id
                    WHERE c.component_id = $1
                "#})
                    .bind(component_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.enrich_component(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM current_deployments cd
                    JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id AND cdr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id AND dcr.deployment_revision_id = cdr.deployment_revision_id
                    JOIN component_revisions cr
                        ON cr.component_id = dcr.component_id AND cr.revision_id = dcr.component_revision_id
                    JOIN components c
                        ON c.component_id  = cr.component_id
                    WHERE cd.environment_id = $1 AND c.name = $2
                "#})
                    .bind(environment_id)
                    .bind(name),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.enrich_component(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_all_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self.with_ro("get_all_deployed_by_id")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    WITH distinct_revs AS (
                        SELECT DISTINCT cr.revision_id
                        FROM current_deployment_revisions cdr
                        JOIN deployment_component_revisions dcr
                            ON dcr.environment_id = cdr.environment_id AND dcr.deployment_revision_id = cdr.deployment_revision_id
                        JOIN component_revisions cr
                            ON cr.component_id = dcr.component_id AND cr.revision_id = dcr.component_revision_id
                        JOIN components c
                            ON c.component_id = cr.component_id
                        WHERE c.component_id = $1
                    )
                    SELECT
                          c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM distinct_revs dr
                    JOIN component_revisions cr
                        ON cr.revision_id = dr.revision_id
                    JOIN components c
                        ON c.component_id = cr.component_id
                    WHERE c.component_id = $1
                    ORDER BY cr.revision_id;
                "#})
                    .bind(component_id),
            )
            .await?;

        Ok(revisions)
    }

    async fn get_by_id_and_revision(
        &self,
        component_id: &Uuid,
        revision_id: i64,
        include_deleted: bool,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self
            .with_ro("get_by_id_and_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    WHERE c.component_id = $1 AND cr.revision_id = $2 AND ($3 OR cr.deleted = FALSE)
                "#})
                .bind(component_id)
                .bind(revision_id)
                .bind(include_deleted),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.enrich_component(revision).await?)),
            None => Ok(None),
        }
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND c.deleted_at IS NULL
                    ORDER BY c.name
                "#})
                    .bind(environment_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.enrich_component(revision))
            .try_collect()
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self.with_ro("list_deployed")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM current_deployments cd
                    JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id AND cdr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id AND dcr.deployment_revision_id = cdr.deployment_revision_id
                    JOIN component_revisions cr
                        ON cr.component_id = dcr.component_id AND cr.revision_id = dcr.component_revision_id
                    JOIN components c
                        ON c.component_id  = cr.component_id
                    WHERE cd.environment_id = $1
                    ORDER BY c.name
                "#})
                    .bind(environment_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.enrich_component(revision))
            .try_collect()
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self
            .with_ro("list_by_deployment")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.component_id = c.component_id
                           AND dcr.component_revision_id = cr.revision_id
                    WHERE dcr.environment_id = $1 AND dcr.deployment_revision_id = $2
                    ORDER BY c.name
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.enrich_component(revision))
            .try_collect()
            .await
    }

    async fn get_by_deployment_and_name(
        &self,
        environment_id: &Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata, cr.original_env, cr.env,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN deployment_component_revisions dcr ON dcr.component_id = c.component_id AND dcr.component_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND dcr.deployment_revision_id = $2 AND c.name = $3
                "#})
                    .bind(environment_id)
                    .bind(deployment_revision_id)
                    .bind(name),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.enrich_component(revision).await?)),
            None => Ok(None),
        }
    }
}

#[async_trait]
trait ComponentRepoInternal: ComponentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn get_original_component_files(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentFileRecord>>;

    async fn get_component_plugins(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentPluginInstallationRecord>>;

    async fn get_component_plugins_tx(
        tx: &mut Self::Tx,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentPluginInstallationRecord>>;

    async fn get_component_files(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentFileRecord>>;

    // TODO: create a variant the accepts multiple component records, and which batches
    //       queries (e.g. with "IN")
    async fn enrich_component(
        &self,
        mut component: ComponentExtRevisionRecord,
    ) -> RepoResult<ComponentExtRevisionRecord> {
        component.revision.original_files = self
            .get_original_component_files(
                &component.revision.component_id,
                component.revision.revision_id,
            )
            .await?;
        component.revision.plugins = self
            .get_component_plugins(
                &component.revision.component_id,
                component.revision.revision_id,
            )
            .await?;
        component.revision.files = self
            .get_component_files(
                &component.revision.component_id,
                component.revision.revision_id,
            )
            .await?;
        Ok(component)
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        version_check: bool,
        revision: ComponentRevisionRecord,
    ) -> Result<ComponentRevisionRecord, ComponentRepoError>;

    async fn insert_original_file(
        tx: &mut Self::Tx,
        file: ComponentFileRecord,
    ) -> RepoResult<ComponentFileRecord>;

    async fn insert_file(
        tx: &mut Self::Tx,
        file: ComponentFileRecord,
    ) -> RepoResult<ComponentFileRecord>;

    async fn insert_plugin(
        tx: &mut Self::Tx,
        plugin: ComponentPluginInstallationRecord,
    ) -> RepoResult<()>;

    async fn version_exists(
        tx: &mut Self::Tx,
        component_id: &Uuid,
        version: &str,
    ) -> RepoResult<bool>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ComponentRepoInternal for DbComponentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn get_original_component_files(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentFileRecord>> {
        self.with_ro("get_original_component_files")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT component_id, revision_id, file_path,
                           created_at, created_by, file_content_hash, file_permissions
                    FROM original_component_files
                    WHERE component_id = $1 AND revision_id = $2
                    ORDER BY file_path
                "#})
                .bind(component_id)
                .bind(revision_id),
            )
            .await
    }

    async fn get_component_plugins(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentPluginInstallationRecord>> {
        self.with_ro("get_component_plugins")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        cpi.component_id,
                        cpi.revision_id,
                        cpi.priority,
                        cpi.environment_plugin_grant_id,
                        cpi.parameters,
                        p.plugin_id AS plugin_registration_id,
                        p.name AS plugin_name,
                        p.version AS plugin_version,
                        p.component_id AS oplog_processor_component_id,
                        p.component_revision_id AS oplog_processor_component_revision_id,
                        cpi.created_at,
                        cpi.created_by
                    FROM component_plugin_installations cpi
                    INNER JOIN environment_plugin_grants epg
                        ON epg.environment_plugin_grant_id = cpi.environment_plugin_grant_id
                    INNER JOIN plugins p
                        ON p.plugin_id = epg.plugin_id
                    WHERE cpi.component_id = $1 AND cpi.revision_id = $2
                    ORDER BY priority
                "#})
                .bind(component_id)
                .bind(revision_id),
            )
            .await
    }

    async fn get_component_plugins_tx(
        tx: &mut Self::Tx,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentPluginInstallationRecord>> {
        tx.fetch_all_as(
            sqlx::query_as(indoc! { r#"
                SELECT
                       cpi.component_id,
                       cpi.revision_id,
                       cpi.priority,
                       cpi.environment_plugin_grant_id,
                       cpi.parameters,
                       p.plugin_id AS plugin_registration_id,
                       p.name AS plugin_name,
                       p.version AS plugin_version,
                       p.component_id AS oplog_processor_component_id,
                       p.component_revision_id AS oplog_processor_component_revision_id,
                       cpi.created_at,
                       cpi.created_by
                FROM component_plugin_installations cpi
                INNER JOIN environment_plugin_grants epg
                    ON epg.environment_plugin_grant_id = cpi.environment_plugin_grant_id
                INNER JOIN plugins p
                    ON p.plugin_id = epg.plugin_id
                WHERE cpi.component_id = $1 AND cpi.revision_id = $2
                ORDER BY priority
            "#})
            .bind(component_id)
            .bind(revision_id),
        )
        .await
    }

    async fn get_component_files(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<ComponentFileRecord>> {
        self.with_ro("get_component_files")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT component_id, revision_id, file_path,
                           created_at, created_by, file_content_hash, file_permissions
                    FROM component_files
                    WHERE component_id = $1 AND revision_id = $2
                    ORDER BY file_path
                "#})
                .bind(component_id)
                .bind(revision_id),
            )
            .await
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        version_check: bool,
        revision: ComponentRevisionRecord,
    ) -> Result<ComponentRevisionRecord, ComponentRepoError> {
        if version_check
            && Self::version_exists(tx, &revision.component_id, &revision.version).await?
        {
            Err(ComponentRepoError::VersionAlreadyExists {
                version: revision.version.clone(),
            })?
        }

        let revision = revision.with_updated_hash();
        let original_files = revision.original_files;
        let plugins = revision.plugins;
        let files = revision.files;

        let mut revision: ComponentRevisionRecord = {
            tx.fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO component_revisions
                    (component_id, revision_id, version, hash,
                        created_at, created_by, deleted,
                        size, metadata, original_env, env,
                        object_store_key, binary_hash, transformed_object_store_key)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                    RETURNING component_id, revision_id, version, hash,
                        created_at, created_by, deleted,
                        size, metadata, original_env, env,
                        object_store_key, binary_hash, transformed_object_store_key
                "# })
                .bind(revision.component_id)
                .bind(revision.revision_id)
                .bind(revision.version)
                .bind(revision.hash)
                .bind_deletable_revision_audit(revision.audit)
                .bind(revision.size)
                .bind(revision.metadata)
                .bind(revision.original_env)
                .bind(revision.env)
                .bind(revision.object_store_key)
                .bind(revision.binary_hash)
                .bind(revision.transformed_object_store_key),
            )
            .await
            .to_error_on_unique_violation(ComponentRepoError::ConcurrentModification)?
        };

        revision.original_files = {
            let mut inserted_files = Vec::with_capacity(original_files.len());
            for file in original_files {
                inserted_files.push(Self::insert_original_file(tx, file).await?);
            }
            inserted_files.sort_by(|a, b| a.file_path.cmp(&b.file_path));
            inserted_files
        };

        revision.files = {
            let mut inserted_files = Vec::with_capacity(files.len());
            for file in files {
                inserted_files.push(Self::insert_file(tx, file).await?);
            }
            inserted_files.sort_by(|a, b| a.file_path.cmp(&b.file_path));
            inserted_files
        };

        revision.plugins = {
            for plugin in plugins {
                Self::insert_plugin(tx, plugin).await?
            }

            Self::get_component_plugins_tx(tx, &revision.component_id, revision.revision_id).await?
        };

        Ok(revision)
    }

    async fn insert_original_file(
        tx: &mut Self::Tx,
        file: ComponentFileRecord,
    ) -> RepoResult<ComponentFileRecord> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO original_component_files
                (component_id, revision_id, file_path, created_at, created_by, file_content_hash, file_permissions)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING component_id, revision_id, file_path, created_at, created_by, file_content_hash, file_permissions
            "#})
                .bind(file.component_id)
                .bind(file.revision_id)
                .bind(file.file_path)
                .bind_revision_audit(file.audit)
                .bind(file.file_content_hash)
                .bind(file.file_permissions)
        ).await
    }

    async fn insert_file(
        tx: &mut Self::Tx,
        file: ComponentFileRecord,
    ) -> RepoResult<ComponentFileRecord> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO component_files
                (component_id, revision_id, file_path, created_at, created_by, file_content_hash, file_permissions)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING component_id, revision_id, file_path, created_at, created_by, file_content_hash, file_permissions
            "#})
                .bind(file.component_id)
                .bind(file.revision_id)
                .bind(file.file_path)
                .bind_revision_audit(file.audit)
                .bind(file.file_content_hash)
                .bind(file.file_permissions)
        ).await
    }

    async fn insert_plugin(
        tx: &mut Self::Tx,
        plugin: ComponentPluginInstallationRecord,
    ) -> RepoResult<()> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO component_plugin_installations
                (component_id, revision_id, priority, created_at, created_by, environment_plugin_grant_id, parameters)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING component_id, revision_id, priority, created_at, created_by, environment_plugin_grant_id, parameters
            "#})
                .bind(plugin.component_id)
                .bind(plugin.revision_id)
                .bind(plugin.priority)
                .bind_revision_audit(plugin.audit)
                .bind(plugin.environment_plugin_grant_id)
                .bind(plugin.parameters)
        ).await
    }

    async fn version_exists(
        tx: &mut Self::Tx,
        component_id: &Uuid,
        version: &str,
    ) -> RepoResult<bool> {
        Ok(tx
            .fetch_optional(
                sqlx::query(indoc! { r#"
                    SELECT 1
                    FROM component_revisions r
                    JOIN deployment_component_revisions dr
                        ON dr.component_id = r.component_id AND dr.component_revision_id = r.revision_id
                    WHERE dr.component_id = $1 AND version = $2
                    GROUP BY dr.component_id
                    LIMIT 1
                "#})
                    .bind(component_id)
                    .bind(version),
            )
            .await?
            .is_some())
    }
}
