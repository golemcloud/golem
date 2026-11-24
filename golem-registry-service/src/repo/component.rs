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

use crate::repo::card::DbCardRepo;
use crate::repo::environment::DbEnvironmentRepo;
use crate::repo::model::BindFields;
use crate::repo::model::card::CardRecord;
use crate::repo::model::component::{
    ComponentAuthExtRevisionRecord, ComponentExactRevisionRecord, ComponentExtRevisionRecord,
    ComponentRepoError, ComponentRevisionIdentityRecord, ComponentRevisionRecord,
};
use crate::repo::registry_change::{RequiresNotificationSignal, RequiresSignalExt};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::model::account::AccountId;
use golem_common::model::card::{
    CardId, CardManagedBy, CardManagedByAgentInitial, CardManagedByComponentInitial,
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::Blob;
use golem_service_base::repo::{RepoError, RepoResult, ResultExt, SqlDateTime};
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, ExistsResult};
use indoc::indoc;
use sqlx::{Database, Row};
use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[derive(Clone)]
pub struct ComponentUpload {
    pub object_key: String,
    pub(crate) digest: String,
    pub(crate) storage: Arc<dyn BlobStorage>,
    pub(crate) bytes: Arc<[u8]>,
}

#[async_trait]
pub trait ComponentRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        self.create_with_upload(environment_id, name, revision, cards_to_create, None)
            .await
    }

    async fn create_with_upload(
        &self,
        environment_id: Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
        upload: Option<ComponentUpload>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError>;

    async fn update(
        &self,
        revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        self.update_with_upload(revision, cards_to_create, None)
            .await
    }

    async fn update_with_upload(
        &self,
        revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
        upload: Option<ComponentUpload>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError>;

    async fn register_upload(
        &self,
        account_id: Uuid,
        object_key: &str,
        expires: SqlDateTime,
    ) -> RepoResult<()>;
    async fn collect_uploads(
        &self,
        now: SqlDateTime,
        after: &str,
    ) -> RepoResult<(String, Vec<(Uuid, String)>)>;
    async fn abandon_upload(&self, object_key: &str) -> RepoResult<()>;
    async fn confirm_upload_deleted(&self, object_key: &str) -> RepoResult<()>;

    async fn delete(
        &self,
        user_account_id: Uuid,
        component_id: Uuid,
        revision_id: i64,
    ) -> Result<RequiresNotificationSignal<Vec<CardId>>, ComponentRepoError>;

    async fn get_staged_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Option<ComponentAuthExtRevisionRecord>>;

    async fn get_staged_by_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn get_deployed_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Option<ComponentAuthExtRevisionRecord>>;

    async fn get_deployed_by_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn get_all_deployed_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Vec<ComponentAuthExtRevisionRecord>>;

    async fn get_by_id_and_revision(
        &self,
        component_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<ComponentExactRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn list_deployed(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn list_by_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>>;

    async fn get_by_deployment_and_name(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>>;

    async fn list_initial_permission_card_ids_by_account(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<CardId>>;
}

pub struct LoggedComponentRepo<Repo: ComponentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "component repository";

impl<Repo: ComponentRepo> LoggedComponentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(environment_id: Uuid, name: &str) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, name)
    }

    fn span_id(component_id: Uuid) -> Span {
        info_span!(SPAN_NAME, component_id = %component_id)
    }

    fn span_id_and_revision(component_id: Uuid, revision_id: i64) -> Span {
        info_span!(SPAN_NAME, component_id = %component_id, revision_id)
    }

    fn span_env(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_env_and_deployment_revision(environment_id: Uuid, deployment_revision_id: i64) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, deployment_revision_id)
    }

    fn span_env_and_deployment_revision_and_name(
        environment_id: Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, deployment_revision_id, name)
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl<Repo: ComponentRepo> ComponentRepo for LoggedComponentRepo<Repo> {
    async fn register_upload(
        &self,
        account_id: Uuid,
        object_key: &str,
        expires: SqlDateTime,
    ) -> RepoResult<()> {
        self.repo
            .register_upload(account_id, object_key, expires)
            .await
    }

    async fn collect_uploads(
        &self,
        now: SqlDateTime,
        after: &str,
    ) -> RepoResult<(String, Vec<(Uuid, String)>)> {
        self.repo.collect_uploads(now, after).await
    }

    async fn abandon_upload(&self, object_key: &str) -> RepoResult<()> {
        self.repo.abandon_upload(object_key).await
    }

    async fn confirm_upload_deleted(&self, object_key: &str) -> RepoResult<()> {
        self.repo.confirm_upload_deleted(object_key).await
    }

    async fn create_with_upload(
        &self,
        environment_id: Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
        upload: Option<ComponentUpload>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        self.repo
            .create_with_upload(environment_id, name, revision, cards_to_create, upload)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn update_with_upload(
        &self,
        revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
        upload: Option<ComponentUpload>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        let span = Self::span_id(revision.component_id);
        self.repo
            .update_with_upload(revision, cards_to_create, upload)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: Uuid,
        component_id: Uuid,
        revision_id: i64,
    ) -> Result<RequiresNotificationSignal<Vec<CardId>>, ComponentRepoError> {
        self.repo
            .delete(user_account_id, component_id, revision_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_staged_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Option<ComponentAuthExtRevisionRecord>> {
        self.repo
            .get_staged_by_id(component_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_staged_by_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_staged_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn list_initial_permission_card_ids_by_account(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<CardId>> {
        self.repo
            .list_initial_permission_card_ids_by_account(account_id)
            .instrument(info_span!(SPAN_NAME, account_id = %account_id))
            .await
    }

    async fn get_deployed_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Option<ComponentAuthExtRevisionRecord>> {
        self.repo
            .get_deployed_by_id(component_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        self.repo
            .get_deployed_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn get_all_deployed_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Vec<ComponentAuthExtRevisionRecord>> {
        self.repo
            .get_all_deployed_by_id(component_id)
            .instrument(Self::span_id(component_id))
            .await
    }

    async fn get_by_id_and_revision(
        &self,
        component_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<ComponentExactRevisionRecord>> {
        self.repo
            .get_by_id_and_revision(component_id, revision_id)
            .instrument(Self::span_id_and_revision(component_id, revision_id))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        self.repo
            .list_deployed(environment_id)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_by_deployment(
        &self,
        environment_id: Uuid,
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
        environment_id: Uuid,
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
#[golem_async_trait::region_simple]
impl ComponentRepo for DbComponentRepo<PostgresPool> {
    async fn register_upload(
        &self,
        account_id: Uuid,
        object_key: &str,
        expires: SqlDateTime,
    ) -> RepoResult<()> {
        self.db_pool.with_rw(METRICS_SVC_NAME, "register_upload").execute(
            sqlx::query("INSERT INTO component_uploads (object_key, account_id, collect_after, pending) VALUES ($1, $2, $3, TRUE)")
                .bind(object_key).bind(account_id).bind(expires),
        ).await?;
        Ok(())
    }

    async fn collect_uploads(
        &self,
        now: SqlDateTime,
        after: &str,
    ) -> RepoResult<(String, Vec<(Uuid, String)>)> {
        let batch: Vec<(String, bool)> = self
            .db_pool
            .with_rw(METRICS_SVC_NAME, "find_uploads_to_collect")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                WITH page AS (
                    SELECT object_key, collect_after FROM component_uploads
                    WHERE object_key > $2 ORDER BY object_key LIMIT 100
                )
                SELECT u.object_key, u.collect_after <= $1 AND NOT EXISTS (
                    SELECT 1 FROM component_revisions cr
                    JOIN components c ON c.component_id = cr.component_id
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    WHERE cr.object_store_key = u.object_key
                      AND e.deleted_at IS NULL AND ap.deleted_at IS NULL AND a.deleted_at IS NULL
                ) AS eligible
                FROM page u ORDER BY u.object_key
            "# })
                .bind(&now)
                .bind(after),
            )
            .await?;

        // Page before checking references, so live uploads and repeated delete failures do not
        // make one sweep unbounded or keep later keys out of it.
        let next = if batch.len() == 100 {
            batch.last().unwrap().0.clone()
        } else {
            String::new()
        };

        let mut retired = Vec::new();
        for (key, eligible) in batch {
            if !eligible {
                continue;
            }
            let now = now.clone();
            if let Some(row) = self.with_tx_err::<_, RepoError, _>("retire_upload", |tx| async move {
                let row: Option<(Uuid, String)> = tx.fetch_optional_as(sqlx::query_as(
                    "UPDATE component_uploads SET pending = FALSE WHERE object_key = $1 AND collect_after <= $2 RETURNING account_id, object_key"
                ).bind(&key).bind(now)).await?;
                let Some(row) = row else { return Ok(None); };

                // Take the same mapping lock as a revision insert. A fresh statement after
                // acquiring it sees any reference that committed while we waited for the lock.
                tx.execute(sqlx::query("UPDATE account_component_digests SET digest = digest WHERE object_key = $1").bind(&key)).await?;
                let reference: Option<(i32,)> = tx.fetch_optional_as(sqlx::query_as(indoc! { r#"
                    SELECT 1 FROM component_revisions cr
                    JOIN components c ON c.component_id = cr.component_id
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    WHERE cr.object_store_key = $1
                      AND e.deleted_at IS NULL AND ap.deleted_at IS NULL AND a.deleted_at IS NULL
                    LIMIT 1
                "# }).bind(&key)).await?;
                if reference.is_some() { return Ok(None); }
                tx.execute(sqlx::query("DELETE FROM account_component_digests WHERE object_key = $1").bind(&key)).await?;
                Ok(Some(row))
            }.boxed()).await? {
                retired.push(row);
            }
        }
        Ok((next, retired))
    }

    async fn abandon_upload(&self, object_key: &str) -> RepoResult<()> {
        self.db_pool.with_rw(METRICS_SVC_NAME, "abandon_upload").execute(
            sqlx::query("UPDATE component_uploads SET pending = FALSE, collect_after = $2 WHERE object_key = $1 AND pending")
                .bind(object_key).bind(SqlDateTime::now()),
        ).await?;
        Ok(())
    }

    async fn confirm_upload_deleted(&self, object_key: &str) -> RepoResult<()> {
        self.db_pool
            .with_rw(METRICS_SVC_NAME, "confirm_upload_deleted")
            .execute(
                sqlx::query("DELETE FROM component_uploads WHERE object_key = $1").bind(object_key),
            )
            .await?;
        Ok(())
    }

    async fn create_with_upload(
        &self,
        environment_id: Uuid,
        name: &str,
        mut revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
        upload: Option<ComponentUpload>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        let opt_deleted_revision: Option<ComponentRevisionIdentityRecord> = self.with_ro("create - get opt deleted").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT c.component_id, c.name, cr.revision_id, cr.revision_id, cr.hash
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
            let cards_to_create = remap_agent_initial_card_records(
                cards_to_create,
                ComponentId(recreated_revision.component_id),
                recreated_revision.revision_id.try_into()?,
            );
            return self
                .update_with_upload(recreated_revision, cards_to_create, upload)
                .await;
        }

        let name = name.to_owned();

        self.with_tx_err("create", |tx| {
            async move {
                if !DbEnvironmentRepo::<PostgresPool>::lock_in_tx(tx, environment_id).await? {
                    return Err(ComponentRepoError::ConcurrentModification);
                }

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

                for card in cards_to_create {
                    DbCardRepo::<PostgresPool>::create_in_tx(tx, card).await?;
                }

                if let Some(upload) = upload {
                    revision.object_store_key =
                        Self::attach_upload(tx, environment_id, upload).await?;
                }
                let revision = Self::insert_revision(tx, revision).await?;

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

    async fn update_with_upload(
        &self,
        mut revision: ComponentRevisionRecord,
        cards_to_create: Vec<CardRecord>,
        upload: Option<ComponentUpload>,
    ) -> Result<ComponentExtRevisionRecord, ComponentRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let environment_id = tx
                    .fetch_optional(
                        sqlx::query(indoc! { r#"
                            SELECT environment_id
                            FROM components
                            WHERE component_id = $1
                        "#})
                        .bind(revision.component_id),
                    )
                    .await?
                    .ok_or(ComponentRepoError::ConcurrentModification)?
                    .try_get::<Uuid, _>("environment_id")
                    .map_err(RepoError::from)?;
                if !DbEnvironmentRepo::<PostgresPool>::lock_in_tx(tx, environment_id).await? {
                    return Err(ComponentRepoError::ConcurrentModification);
                }

                for card in cards_to_create {
                    DbCardRepo::<PostgresPool>::create_in_tx(tx, card).await?;
                }

                if let Some(upload) = upload {
                    revision.object_store_key = Self::attach_upload(tx, environment_id, upload).await?;
                }
                let revision: ComponentRevisionRecord = Self::insert_revision(
                    tx,
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
        user_account_id: Uuid,
        component_id: Uuid,
        revision_id: i64,
    ) -> Result<RequiresNotificationSignal<Vec<CardId>>, ComponentRepoError> {
        let deleted_cards = self
            .with_tx_err("delete", |tx| {
                async move {
                    let environment_id = tx
                        .fetch_optional(
                            sqlx::query(indoc! { r#"
                                SELECT environment_id
                                FROM components
                                WHERE component_id = $1
                                    AND deleted_at IS NULL
                            "#})
                            .bind(component_id),
                        )
                        .await?
                        .ok_or(ComponentRepoError::ConcurrentModification)?
                        .try_get::<Uuid, _>("environment_id")
                        .map_err(RepoError::from)?;
                    if !DbEnvironmentRepo::<PostgresPool>::lock_in_tx(tx, environment_id).await? {
                        return Err(ComponentRepoError::ConcurrentModification);
                    }

                    let source_reference = tx
                        .fetch_optional(
                            sqlx::query(indoc! { r#"
                                SELECT component_id
                                FROM tool_releases
                                WHERE component_id = $1
                                UNION ALL
                                SELECT component_id
                                FROM tool_middleware_releases
                                WHERE component_id = $1
                                UNION ALL
                                SELECT component_id
                                FROM deployment_component_revisions
                                WHERE component_id = $1
                                UNION ALL
                                SELECT component_id
                                FROM deployment_registered_tools
                                WHERE component_id = $1
                                LIMIT 1
                            "#})
                            .bind(component_id),
                        )
                        .await?;
                    if source_reference.is_some() {
                        return Err(ComponentRepoError::ComponentSourceInUse);
                    }

                    let active_revisions: Vec<ComponentRevisionRecord> = tx
                        .fetch_all_as(
                            sqlx::query_as(indoc! { r#"
                            SELECT cr.component_id, cr.revision_id, cr.hash,
                                   cr.created_at, cr.created_by, cr.deleted,
                                   cr.size, cr.metadata,
                                   cr.object_store_key, cr.binary_hash, cr.package_metadata
                            FROM component_revisions cr
                            WHERE cr.component_id = $1
                                AND NOT cr.deleted
                                AND cr.revision_id > COALESCE(
                                    (
                                        SELECT MAX(deleted_cr.revision_id)
                                        FROM component_revisions deleted_cr
                                        WHERE deleted_cr.component_id = cr.component_id
                                            AND deleted_cr.deleted
                                    ),
                                    -1
                                )
                        "#})
                            .bind(component_id),
                        )
                        .await?;

                    let mut card_roots = Vec::new();
                    for revision in active_revisions {
                        let component_revision = revision.revision_id.try_into()?;
                        let component_card_id = revision
                            .metadata
                            .value()
                            .component_provision_config()
                            .initial_permissions
                            .card_id;
                        card_roots.push((
                            component_card_id,
                            CardManagedBy::ComponentInitial(CardManagedByComponentInitial {
                                component_id: ComponentId(component_id),
                                component_revision,
                            }),
                        ));
                        for (agent_type, config) in revision
                            .metadata
                            .value()
                            .agent_type_provision_configs()
                            .iter()
                        {
                            let card_id = config.initial_permissions.card_id;
                            let managed_by =
                                CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                                    component_id: ComponentId(component_id),
                                    component_revision,
                                    agent_type: agent_type.clone(),
                                });
                            if !card_roots.contains(&(card_id, managed_by.clone())) {
                                card_roots.push((card_id, managed_by));
                            }
                        }
                    }
                    card_roots.sort_unstable_by_key(|(card_id, _)| *card_id);

                    let revision: ComponentRevisionRecord = Self::insert_revision(
                        tx,
                        ComponentRevisionRecord::deletion(
                            user_account_id,
                            component_id,
                            revision_id,
                        )?,
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

                    let mut deleted_cards = Vec::new();
                    let mut root_index = 0;
                    while root_index < card_roots.len() {
                        let card_id = card_roots[root_index].0;
                        let mut next_root_index = root_index + 1;
                        while next_root_index < card_roots.len()
                            && card_roots[next_root_index].0 == card_id
                        {
                            next_root_index += 1;
                        }

                        let record =
                            DbCardRepo::<PostgresPool>::get_for_update_in_tx(tx, card_id).await?;
                        let managed_by_matches = record
                            .as_ref()
                            .and_then(|record| record.managed_by.as_ref())
                            .is_some_and(|managed_by| {
                                card_roots[root_index..next_root_index]
                                    .iter()
                                    .any(|(_, expected)| managed_by.value() == expected)
                            });

                        if managed_by_matches {
                            deleted_cards.extend(
                                DbCardRepo::<PostgresPool>::delete_tree_in_tx(tx, card_id).await?,
                            );
                        }
                        root_index = next_root_index;
                    }

                    Ok::<_, ComponentRepoError>(deleted_cards)
                }
                .boxed()
            })
            .await?;

        Ok(deleted_cards.requires_notification_signal())
    }

    async fn get_staged_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Option<ComponentAuthExtRevisionRecord>> {
        let revision: Option<ComponentAuthExtRevisionRecord> = self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata,
                           ap.application_id, ap.name AS application_name,
                           a.account_id AS owner_account_id,
                           a.email AS owner_account_email,
                           er.name AS environment_name,
                           er.revision_id AS environment_revision_id,
                           er.compatibility_check AS environment_compatibility_check,
                           er.tool_compatibility_mode AS environment_tool_compatibility_mode,
                           er.version_check AS environment_version_check,
                           er.security_overrides AS environment_security_overrides
                    FROM components c
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN environment_revisions er
                        ON er.environment_id = e.environment_id
                        AND er.revision_id = e.current_revision_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.component_id = $1
                        AND c.deleted_at IS NULL
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
                "#})
                    .bind(component_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(revision)),
            None => Ok(None),
        }
    }

    async fn get_staged_by_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND c.name = $2 AND c.deleted_at IS NULL
                "#})
                    .bind(environment_id)
                    .bind(name),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(revision)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Option<ComponentAuthExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                        cr.component_id, cr.revision_id, cr.hash,
                        cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata,
                           ap.application_id, ap.name AS application_name,
                           a.account_id AS owner_account_id,
                           a.email AS owner_account_email,
                           er.name AS environment_name,
                           er.revision_id AS environment_revision_id,
                           er.compatibility_check AS environment_compatibility_check,
                           er.tool_compatibility_mode AS environment_tool_compatibility_mode,
                           er.version_check AS environment_version_check,
                           er.security_overrides AS environment_security_overrides
                    FROM current_deployments cd
                    JOIN current_deployment_revisions cdr
                        ON cdr.environment_id = cd.environment_id AND cdr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id AND dcr.deployment_revision_id = cdr.deployment_revision_id
                    JOIN component_revisions cr
                        ON cr.component_id = dcr.component_id AND cr.revision_id = dcr.component_revision_id
                    JOIN components c
                        ON c.component_id  = cr.component_id AND c.environment_id = cd.environment_id
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN environment_revisions er
                        ON er.environment_id = e.environment_id
                        AND er.revision_id = e.current_revision_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    WHERE c.component_id = $1
                        AND c.deleted_at IS NULL
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
                "#})
                    .bind(component_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(revision)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
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
            Some(revision) => Ok(Some(revision)),
            None => Ok(None),
        }
    }

    async fn get_all_deployed_by_id(
        &self,
        component_id: Uuid,
    ) -> RepoResult<Vec<ComponentAuthExtRevisionRecord>> {
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
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata,
                           ap.application_id, ap.name AS application_name,
                           a.account_id AS owner_account_id,
                           a.email AS owner_account_email,
                           er.name AS environment_name,
                           er.revision_id AS environment_revision_id,
                           er.compatibility_check AS environment_compatibility_check,
                           er.tool_compatibility_mode AS environment_tool_compatibility_mode,
                           er.version_check AS environment_version_check,
                           er.security_overrides AS environment_security_overrides
                    FROM distinct_revs dr
                    JOIN component_revisions cr
                        ON cr.revision_id = dr.revision_id
                    JOIN components c
                        ON c.component_id = cr.component_id
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN environment_revisions er
                        ON er.environment_id = e.environment_id
                        AND er.revision_id = e.current_revision_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    WHERE c.component_id = $1
                        AND c.deleted_at IS NULL
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
                    ORDER BY cr.revision_id;
                "#})
                    .bind(component_id),
            )
            .await?;

        Ok(revisions)
    }

    async fn get_by_id_and_revision(
        &self,
        component_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<ComponentExactRevisionRecord>> {
        let revision = self
            .with_ro("get_by_id_and_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           c.deleted_at AS component_deleted_at,
                           (
                               SELECT MAX(deleted_cr.revision_id)
                               FROM component_revisions deleted_cr
                               WHERE deleted_cr.component_id = c.component_id
                                   AND deleted_cr.deleted
                           ) AS latest_deletion_revision,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata,
                           ap.application_id, ap.name AS application_name,
                           a.account_id AS owner_account_id,
                           a.email AS owner_account_email,
                           er.name AS environment_name,
                           er.revision_id AS environment_revision_id,
                           er.compatibility_check AS environment_compatibility_check,
                           er.tool_compatibility_mode AS environment_tool_compatibility_mode,
                           er.version_check AS environment_version_check,
                           er.security_overrides AS environment_security_overrides
                    FROM components c
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN environment_revisions er
                        ON er.environment_id = e.environment_id
                        AND er.revision_id = e.current_revision_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    WHERE c.component_id = $1
                        AND cr.revision_id = $2
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
                "#})
                .bind(component_id)
                .bind(revision_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(revision)),
            None => Ok(None),
        }
    }

    async fn list_initial_permission_card_ids_by_account(
        &self,
        account_id: Uuid,
    ) -> RepoResult<Vec<CardId>> {
        let revisions: Vec<ComponentRevisionRecord> = self
            .with_ro("list_initial_permission_card_ids_by_account")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
                    FROM component_revisions cr
                    JOIN components c ON c.component_id = cr.component_id
                    JOIN environments e ON e.environment_id = c.environment_id
                    JOIN applications ap ON ap.application_id = e.application_id
                    JOIN accounts a ON a.account_id = ap.account_id
                    WHERE ap.account_id = $1
                        AND c.deleted_at IS NULL
                        AND NOT cr.deleted
                        AND c.current_revision_id = cr.revision_id
                        -- Component ids can be reused after deleting and recreating a same-name component.
                        -- Ignore all revisions at or before the latest deleted revision so old provision
                        -- metadata cannot reappear as an agent-initial card.
                        AND cr.revision_id > COALESCE(
                            (
                                SELECT MAX(deleted_cr.revision_id)
                                FROM component_revisions deleted_cr
                                WHERE deleted_cr.component_id = c.component_id
                                    AND deleted_cr.deleted
                            ),
                            -1
                        )
                        AND e.deleted_at IS NULL
                        AND ap.deleted_at IS NULL
                        AND a.deleted_at IS NULL
                    ORDER BY cr.created_at, cr.component_id, cr.revision_id
                "#})
                .bind(account_id),
            )
            .await?;

        let mut result = Vec::new();
        for revision in revisions {
            let component_card_id = revision
                .metadata
                .value()
                .component_provision_config()
                .initial_permissions
                .card_id;
            if !result.contains(&component_card_id) {
                result.push(component_card_id);
            }
            for config in revision
                .metadata
                .into_value()
                .agent_type_provision_configs()
                .values()
            {
                let card_id = config.initial_permissions.card_id;
                if !result.contains(&card_id) {
                    result.push(card_id);
                }
            }
        }

        Ok(result)
    }

    async fn list_staged(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND c.deleted_at IS NULL
                    ORDER BY c.name
                "#})
                    .bind(environment_id),
            )
            .await?;
        Ok(revisions)
    }

    async fn list_deployed(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self.with_ro("list_deployed")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
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
        Ok(revisions)
    }

    async fn list_by_deployment(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
    ) -> RepoResult<Vec<ComponentExtRevisionRecord>> {
        let revisions = self
            .with_ro("list_by_deployment")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
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
        Ok(revisions)
    }

    async fn get_by_deployment_and_name(
        &self,
        environment_id: Uuid,
        deployment_revision_id: i64,
        name: &str,
    ) -> RepoResult<Option<ComponentExtRevisionRecord>> {
        let revision = self
            .with_ro("get_deployed_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT c.environment_id, c.name,
                           cr.component_id, cr.revision_id, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.size, cr.metadata,
                           cr.object_store_key, cr.binary_hash, cr.package_metadata
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.component_id = c.component_id
                            AND dcr.component_revision_id = cr.revision_id
                            AND dcr.environment_id = $1
                    WHERE c.environment_id = $1 AND dcr.deployment_revision_id = $2 AND c.name = $3
                "#})
                .bind(environment_id)
                .bind(deployment_revision_id)
                .bind(name),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(revision)),
            None => Ok(None),
        }
    }
}

fn remap_agent_initial_card_records(
    cards: Vec<CardRecord>,
    component_id: ComponentId,
    component_revision: ComponentRevision,
) -> Vec<CardRecord> {
    cards
        .into_iter()
        .map(|mut card| {
            if let Some(managed_by) = card.managed_by.take() {
                card.managed_by = match managed_by.into_value() {
                    CardManagedBy::AgentInitial(CardManagedByAgentInitial {
                        agent_type, ..
                    }) => Some(Blob::new(CardManagedBy::AgentInitial(
                        CardManagedByAgentInitial {
                            component_id,
                            component_revision,
                            agent_type,
                        },
                    ))),
                    CardManagedBy::ComponentInitial(_) => Some(Blob::new(
                        CardManagedBy::ComponentInitial(CardManagedByComponentInitial {
                            component_id,
                            component_revision,
                        }),
                    )),
                    other => Some(Blob::new(other)),
                };
            }
            card
        })
        .collect()
}

#[async_trait]
trait ComponentRepoInternal: ComponentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn attach_upload(
        tx: &mut Self::Tx,
        environment_id: Uuid,
        upload: ComponentUpload,
    ) -> Result<String, ComponentRepoError>;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ComponentRevisionRecord,
    ) -> Result<ComponentRevisionRecord, ComponentRepoError>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
#[golem_async_trait::region_simple]
impl ComponentRepoInternal for DbComponentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn attach_upload(
        tx: &mut Self::Tx,
        environment_id: Uuid,
        upload: ComponentUpload,
    ) -> Result<String, ComponentRepoError> {
        let (account_id,): (Uuid,) = tx
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
            UPDATE component_uploads SET pending = FALSE, collect_after = $2
            WHERE object_key = $1 AND pending AND collect_after > $2
              AND account_id = (
                SELECT ap.account_id FROM environments e
                JOIN applications ap ON ap.application_id = e.application_id
                JOIN accounts a ON a.account_id = ap.account_id
                WHERE e.environment_id = $3 AND ap.deleted_at IS NULL AND a.deleted_at IS NULL
              )
            RETURNING account_id
        "# })
                .bind(&upload.object_key)
                .bind(SqlDateTime::now())
                .bind(environment_id),
            )
            .await?
            .ok_or(ComponentRepoError::ConcurrentModification)?;
        let (object_key,): (String,) = tx
            .fetch_one_as(
                sqlx::query_as(indoc! { r#"
            INSERT INTO account_component_digests (account_id, digest, object_key)
            VALUES ($1, $2, $3)
            ON CONFLICT (account_id, digest) DO UPDATE SET digest = excluded.digest
            RETURNING object_key
        "# })
                .bind(account_id)
                .bind(upload.digest)
                .bind(&upload.object_key),
            )
            .await?;

        // Check either key, so fresh and reused uploads take the same normal storage path.
        // Repair the object in place while the mapping lock still excludes collection.
        tokio::time::timeout(Duration::from_secs(30), async {
            let namespace = BlobStorageNamespace::ComponentUploads {
                account_id: AccountId(account_id),
            };
            let path = Path::new(&object_key);
            match upload
                .storage
                .exists("component_files", "attach_upload", namespace.clone(), path)
                .await?
            {
                ExistsResult::File => {}
                ExistsResult::DoesNotExist => {
                    let _ = upload
                        .storage
                        .put_raw_if_absent(
                            "component_files",
                            "attach_upload",
                            namespace,
                            path,
                            &upload.bytes,
                        )
                        .await?;
                }
                ExistsResult::Directory => {
                    anyhow::bail!("Found a directory at a component upload key")
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(anyhow::Error::from)??;
        Ok(object_key)
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ComponentRevisionRecord,
    ) -> Result<ComponentRevisionRecord, ComponentRepoError> {
        let revision = revision.with_updated_hash()?;

        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO component_revisions
                (component_id, revision_id, hash,
                    created_at, created_by, deleted,
                    size, metadata,
                    object_store_key, binary_hash, package_metadata)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                RETURNING component_id, revision_id, hash,
                    created_at, created_by, deleted,
                    size, metadata,
                    object_store_key, binary_hash, package_metadata
            "# })
            .bind(revision.component_id)
            .bind(revision.revision_id)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit)
            .bind(revision.size)
            .bind(revision.metadata)
            .bind(revision.object_store_key)
            .bind(revision.binary_hash)
            .bind(revision.package_metadata),
        )
        .await
        .to_error_on_unique_violation(ComponentRepoError::ConcurrentModification)
    }
}
