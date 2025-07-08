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
    ComponentFileRecord, ComponentRecord, ComponentRevisionRecord,
};
use crate::repo::model::BindFields;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
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
        self.update(current_revision_id, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        component_id: &Uuid,
        current_revision_id: i64,
    ) -> repo::Result<bool> {
        self.delete(user_account_id, component_id, current_revision_id)
            .instrument(Self::span_component_id(component_id))
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

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<F, R>(&self, api_name: &'static str, f: F) -> Result<R, RepoError>
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
                    WHERE component_id = $1 AND current_revision_id = $2 and deleted_at IS NULL
                "#})
                .bind(component_id)
                .bind(current_revision_id),
            )
            .await
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
                .bind_revision_audit_fields(revision.audit)
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

        let mut inserted_files = Vec::<ComponentFileRecord>::with_capacity(files.len());
        for file in files {
            inserted_files.push(Self::insert_file(tx, file).await?);
        }
        revision.files = inserted_files;

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
            "#})
                .bind(file.component_id)
                .bind(file.revision_id)
                .bind(file.file_path)
                .bind(file.hash)
                .bind(file.created_at)
                .bind(file.created_by)
                .bind(file.file_key)
                .bind(file.file_permissions)
        ).await
    }
}
