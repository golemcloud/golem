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

use crate::repo::model::component::{
    ComponentFileRecord, ComponentRecord, ComponentRevisionIdentityRecord, ComponentRevisionRecord,
};
use crate::repo::model::BindFields;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::future::BoxFuture;
use futures::{stream, FutureExt, StreamExt, TryStreamExt};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo;
use golem_service_base::repo::RepoError;
use indoc::indoc;
use sqlx::Database;
use tracing::{info_span, Instrument, Span};
use uuid::Uuid;

#[async_trait]
pub trait ComponentRepo: Send + Sync {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<Option<ComponentRevisionRecord>>;

    async fn update(
        &self,
        current_revision_id: i64,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<Option<ComponentRevisionRecord>>;

    async fn delete(
        &self,
        user_account_id: &Uuid,
        component_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool>;

    async fn get_staged_by_id(
        &self,
        component_id: &Uuid,
    ) -> repo::Result<Option<ComponentRevisionRecord>>;

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ComponentRevisionRecord>>;

    async fn get_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> repo::Result<Option<ComponentRevisionRecord>>;

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ComponentRevisionRecord>>;

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<ComponentRevisionRecord>>;

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<ComponentRevisionRecord>>;
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

    fn span_component_id(component_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, component_id = %component_id)
    }

    fn span_environment_id(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }
}

#[async_trait]
impl<Repo: ComponentRepo> ComponentRepo for LoggedComponentRepo<Repo> {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        self.repo
            .create(environment_id, name, revision)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let span = Self::span_component_id(&revision.component_id);
        self.repo
            .update(current_revision_id, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        component_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        self.repo
            .delete(user_account_id, component_id, current_revision_id)
            .instrument(Self::span_component_id(component_id))
            .await
    }

    async fn get_staged_by_id(
        &self,
        component_id: &Uuid,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        self.repo
            .get_staged_by_id(component_id)
            .instrument(Self::span_component_id(component_id))
            .await
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        self.repo
            .get_staged_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn get_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        self.repo
            .get_deployed_by_id(component_id)
            .instrument(Self::span_component_id(component_id))
            .await
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        self.repo
            .get_deployed_by_name(environment_id, name)
            .instrument(Self::span_name(environment_id, name))
            .await
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<ComponentRevisionRecord>> {
        self.repo
            .list_staged(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<ComponentRevisionRecord>> {
        self.repo
            .list_deployed(environment_id)
            .instrument(Self::span_environment_id(environment_id))
            .await
    }
}

pub struct DbComponentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment";

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
impl ComponentRepo for DbComponentRepo<PostgresPool> {
    async fn create(
        &self,
        environment_id: &Uuid,
        name: &str,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let opt_deleted_revision: Option<ComponentRevisionIdentityRecord> = self.with_ro("create - get opt deleted").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT c.component_id, c.name, cr.revision_id, cr.revision_id, cr.version, cr.status, cr.hash
                FROM components c
                JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                WHERE c.environment_id = $1 AND c.name = $2 AND c.deleted_at IS NOT NULL
            "#})
                .bind(environment_id)
                .bind(name)
        ).await?;

        if let Some(deleted_revision) = opt_deleted_revision {
            let revision = ComponentRevisionRecord {
                component_id: deleted_revision.component_id,
                ..revision
            };
            return self.update(deleted_revision.revision_id, revision).await;
        }

        let environment_id = *environment_id;
        let name = name.to_owned();
        let revision = revision.ensure_first();

        let result: repo::Result<ComponentRevisionRecord> = self
            .with_tx("create", |tx| {
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
        revision: ComponentRevisionRecord,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let Some(_checked_component) = self
            .check_current_revision(&revision.component_id, current_revision_id)
            .await?
        else {
            return Ok(None);
        };

        // TODO: if env requires check version name uniqueness (but comparing only to deployed ones!)

        let result: repo::Result<ComponentRevisionRecord> = self
            .with_tx("update", |tx| {
                async move {
                    let revision: ComponentRevisionRecord =
                        Self::insert_revision(tx, revision.ensure_new(current_revision_id)).await?;

                    tx.execute(
                        sqlx::query(indoc! { r#"
                            UPDATE components
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE component_id = $4
                        "#})
                        .bind(&revision.audit.created_at)
                        .bind(revision.audit.created_by)
                        .bind(revision.revision_id)
                        .bind(revision.component_id),
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
        component_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        let user_account_id = *user_account_id;
        let component_id = *component_id;

        let Some(_checked_env) = self
            .check_current_revision(&component_id, current_revision_id)
            .await?
        else {
            return Ok(false);
        };

        let result: repo::Result<()> = self
            .with_tx("delete", |tx| {
                async move {
                    let revision: ComponentRevisionRecord = Self::insert_revision(
                        tx,
                        ComponentRevisionRecord::deletion(
                            user_account_id,
                            component_id,
                            current_revision_id,
                        ),
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
            .await;

        match result {
            Ok(()) => Ok(true),
            Err(err) if err.is_unique_violation() => Ok(false),
            Err(err) => Err(err),
        }
    }

    async fn get_staged_by_id(
        &self,
        component_id: &Uuid,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let revision: Option<ComponentRevisionRecord> = self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.component_type, cr.size, cr.metadata, cr.env, cr.status,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id AND c.current_revision_id = cr.revision_id
                    WHERE c.component_id = $1 AND c.deleted_at IS NULL
                "#})
                    .bind(component_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_component_files(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_staged_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let revision = self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.component_type, cr.size, cr.metadata, cr.env, cr.status,
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
            Some(revision) => Ok(Some(self.with_component_files(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_id(
        &self,
        component_id: &Uuid,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.component_type, cr.size, cr.metadata, cr.env, cr.status,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN current_deployments cd ON cd.environment_id = c.environment_id
                    JOIN deployment_revisions dr
                        ON dr.environment_id = cd.environment_id AND dr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id AND dcr.deployment_revision_id = dr.revision_id
                               AND dcr.component_revision_id = cr.revision_id
                    WHERE c.component_id = $1 AND c.deleted_at IS NULL
                "#})
                    .bind(component_id),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_component_files(revision).await?)),
            None => Ok(None),
        }
    }

    async fn get_deployed_by_name(
        &self,
        environment_id: &Uuid,
        name: &str,
    ) -> repo::Result<Option<ComponentRevisionRecord>> {
        let revision = self.with_ro("get_deployed_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.component_type, cr.size, cr.metadata, cr.env, cr.status,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN current_deployments cd ON cd.environment_id = c.environment_id
                    JOIN deployment_revisions dr
                        ON dr.environment_id = cd.environment_id AND dr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id AND dcr.deployment_revision_id = dr.revision_id
                               AND dcr.component_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND c.name = $2 AND c.deleted_at IS NULL
                "#})
                    .bind(environment_id)
                    .bind(name),
            )
            .await?;

        match revision {
            Some(revision) => Ok(Some(self.with_component_files(revision).await?)),
            None => Ok(None),
        }
    }

    async fn list_staged(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<ComponentRevisionRecord>> {
        let revisions = self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.component_type, cr.size, cr.metadata, cr.env, cr.status,
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
            .then(|revision| self.with_component_files(revision))
            .try_collect()
            .await
    }

    async fn list_deployed(
        &self,
        environment_id: &Uuid,
    ) -> repo::Result<Vec<ComponentRevisionRecord>> {
        let revisions = self.with_ro("list_deployed")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT cr.component_id, cr.revision_id, cr.version, cr.hash,
                           cr.created_at, cr.created_by, cr.deleted,
                           cr.component_type, cr.size, cr.metadata, cr.env, cr.status,
                           cr.object_store_key, cr.binary_hash, cr.transformed_object_store_key
                    FROM components c
                    JOIN component_revisions cr ON c.component_id = cr.component_id
                    JOIN current_deployments cd ON cd.environment_id = c.environment_id
                    JOIN deployment_revisions dr
                        ON dr.environment_id = cd.environment_id AND dr.revision_id = cd.current_revision_id
                    JOIN deployment_component_revisions dcr
                        ON dcr.environment_id = cd.environment_id
                           AND dcr.deployment_revision_id = dr.revision_id
                           AND dcr.component_id = c.component_id
                           AND dcr.component_revision_id = cr.revision_id
                    WHERE c.environment_id = $1 AND c.deleted_at IS NULL
                    ORDER BY c.name
                "#})
                    .bind(environment_id),
            )
            .await?;

        stream::iter(revisions)
            .then(|revision| self.with_component_files(revision))
            .try_collect()
            .await
    }
}

#[async_trait]
trait ComponentRepoInternal: ComponentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn check_current_revision(
        &self,
        component_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<ComponentRecord>>;

    async fn get_component_files(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Vec<ComponentFileRecord>>;

    async fn with_component_files(
        &self,
        component: ComponentRevisionRecord,
    ) -> repo::Result<ComponentRevisionRecord>;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<ComponentRevisionRecord>;

    async fn insert_file(
        tx: &mut Self::Tx,
        file: ComponentFileRecord,
    ) -> repo::Result<ComponentFileRecord>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ComponentRepoInternal for DbComponentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn check_current_revision(
        &self,
        component_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<Option<ComponentRecord>> {
        self.with_ro("check_current_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT component_id, name, environment_id,
                      created_at, updated_at, deleted_at, modified_by,
                      current_revision_id
                    FROM components
                    WHERE component_id = $1 AND current_revision_id = $2
                "#})
                .bind(component_id)
                .bind(current_revision_id),
            )
            .await
    }

    async fn get_component_files(
        &self,
        component_id: &Uuid,
        revision_id: i64,
    ) -> repo::Result<Vec<ComponentFileRecord>> {
        self.with_ro("get_component_files")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT component_id, revision_id, file_path, hash,
                           created_at, created_by, file_key, file_permissions
                    FROM component_files
                    WHERE component_id = $1 AND revision_id = $2
                    ORDER BY file_path
                "#})
                .bind(component_id)
                .bind(revision_id),
            )
            .await
    }

    // TODO: create a variant the accepts multiple component records, and which batches
    //       queries (e.g. with "IN")
    async fn with_component_files(
        &self,
        mut component: ComponentRevisionRecord,
    ) -> repo::Result<ComponentRevisionRecord> {
        component.files = self
            .get_component_files(&component.component_id, component.revision_id)
            .await?;
        Ok(component)
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: ComponentRevisionRecord,
    ) -> repo::Result<ComponentRevisionRecord> {
        let files = revision.files;

        let mut revision: ComponentRevisionRecord = {
            tx.fetch_one_as(
                sqlx::query_as(indoc! { r#"
                    INSERT INTO component_revisions
                    (component_id, revision_id, version, hash,
                        created_at, created_by, deleted,
                        component_type,size, metadata, env, status,
                        object_store_key, binary_hash, transformed_object_store_key)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
                    RETURNING component_id, revision_id, version, hash,
                        created_at, created_by, deleted,
                        component_type,size, metadata, env, status,
                        object_store_key, binary_hash, transformed_object_store_key
                "# })
                .bind(revision.component_id)
                .bind(revision.revision_id)
                .bind(revision.version)
                .bind(revision.hash)
                .bind_deletable_revision_audit(revision.audit)
                .bind(revision.component_type)
                .bind(revision.size)
                .bind(revision.metadata)
                .bind(revision.env)
                .bind(revision.status)
                .bind(revision.object_store_key)
                .bind(revision.binary_hash)
                .bind(revision.transformed_object_store_key),
            )
            .await?
        };

        revision.files = {
            let mut inserted_files = Vec::<ComponentFileRecord>::with_capacity(files.len());
            for file in files {
                inserted_files.push(
                    Self::insert_file(
                        tx,
                        file.ensure_component(
                            revision.component_id,
                            revision.revision_id,
                            revision.audit.created_by,
                        ),
                    )
                    .await?,
                );
            }
            inserted_files
        };

        revision.files.sort_by(|a, b| a.file_path.cmp(&b.file_path));

        Ok(revision)
    }

    async fn insert_file(
        tx: &mut Self::Tx,
        file: ComponentFileRecord,
    ) -> repo::Result<ComponentFileRecord> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                INSERT INTO component_files
                (component_id, revision_id, file_path, hash, created_at, created_by, file_key, file_permissions)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING component_id, revision_id, file_path, hash, created_at, created_by, file_key, file_permissions
            "#})
                .bind(file.component_id)
                .bind(file.revision_id)
                .bind(file.file_path)
                .bind(file.hash)
                .bind_revision_audit(file.audit)
                .bind(file.file_key)
                .bind(file.file_permissions)
        ).await
    }
}
