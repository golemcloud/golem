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

use super::model::resource_definition::{
    ResourceDefinitionCreationArgs, ResourceDefinitionExtRevisionRecord,
    ResourceDefinitionRepoError, ResourceDefinitionRevisionRecord,
};
use crate::repo::model::BindFields;
use crate::repo::model::resource_definition::ResourceDefinitionRecord;
use crate::repo::registry_change::{DbRegistryChangeRepo, NewRegistryChangeEvent};
use crate::repo::registry_change::{RequiresNotificationSignal, RequiresSignalExt};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::repo::{PoolLabelledTransaction, RepoError, RepoResult, ResultExt};
use indoc::indoc;
use std::fmt::Debug;
use tracing::{Instrument, info_span};
use uuid::Uuid;

#[async_trait]
pub trait ResourceDefinitionRepo: Send + Sync {
    async fn create(
        &self,
        args: ResourceDefinitionCreationArgs,
    ) -> Result<RequiresNotificationSignal<ResourceDefinitionExtRevisionRecord>, ResourceDefinitionRepoError>;

    async fn update(
        &self,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<RequiresNotificationSignal<ResourceDefinitionExtRevisionRecord>, ResourceDefinitionRepoError>;

    async fn delete(
        &self,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<RequiresNotificationSignal<()>, ResourceDefinitionRepoError>;

    async fn get(
        &self,
        resource_definition_id: Uuid,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>>;

    async fn get_by_environment_and_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>>;

    async fn get_revision(
        &self,
        resource_definition_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>>;

    async fn list_in_environment(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ResourceDefinitionExtRevisionRecord>>;
}

pub struct LoggedResourceDefinitionRepo<Repo> {
    repo: Repo,
}

impl<Repo> LoggedResourceDefinitionRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

static SPAN_NAME: &str = "resource_definition_repository";

#[async_trait]
impl<Repo: ResourceDefinitionRepo> ResourceDefinitionRepo for LoggedResourceDefinitionRepo<Repo> {
    async fn create(
        &self,
        args: ResourceDefinitionCreationArgs,
    ) -> Result<RequiresNotificationSignal<ResourceDefinitionExtRevisionRecord>, ResourceDefinitionRepoError> {
        let span = info_span!(
            SPAN_NAME,
            resource_definition_id = %args.revision.resource_definition_id,
        );

        self.repo.create(args).instrument(span).await
    }

    async fn update(
        &self,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<RequiresNotificationSignal<ResourceDefinitionExtRevisionRecord>, ResourceDefinitionRepoError> {
        let span = info_span!(
            SPAN_NAME,
            resource_definition_id = %revision.resource_definition_id,
        );

        self.repo.update(revision).instrument(span).await
    }

    async fn delete(
        &self,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<RequiresNotificationSignal<()>, ResourceDefinitionRepoError> {
        let span = info_span!(
            SPAN_NAME,
            resource_definition_id = %revision.resource_definition_id,
            revision_id = revision.revision_id
        );

        self.repo.delete(revision).instrument(span).await
    }

    async fn get(
        &self,
        resource_definition_id: Uuid,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>> {
        let span = info_span!(
            SPAN_NAME,
            resource_definition_id = %resource_definition_id,
        );

        self.repo.get(resource_definition_id).instrument(span).await
    }

    async fn get_by_environment_and_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>> {
        let span = info_span!(
            SPAN_NAME,
            environment_id = %environment_id,
            name
        );

        self.repo
            .get_by_environment_and_name(environment_id, name)
            .instrument(span)
            .await
    }

    async fn get_revision(
        &self,
        resource_definition_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>> {
        let span = info_span!(
            SPAN_NAME,
            resource_definition_id = %resource_definition_id,
            revision_id
        );

        self.repo
            .get_revision(resource_definition_id, revision_id)
            .instrument(span)
            .await
    }

    async fn list_in_environment(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ResourceDefinitionExtRevisionRecord>> {
        let span = info_span!(
            SPAN_NAME,
            environment_id = %environment_id
        );

        self.repo
            .list_in_environment(environment_id)
            .instrument(span)
            .await
    }
}

pub struct DbResourceDefinitionRepo<DBP> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "resource_definition_repo";

impl<DBP: Pool> DbResourceDefinitionRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedResourceDefinitionRepo<Self> {
        LoggedResourceDefinitionRepo::new(Self::new(db_pool))
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
impl DbResourceDefinitionRepo<PostgresPool> {
    async fn insert_revision(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<ResourceDefinitionRevisionRecord, ResourceDefinitionRepoError> {
        tx.fetch_one_as(
            sqlx::query_as(indoc! { r#"
                    INSERT INTO resource_definition_revisions
                    (
                        resource_definition_id,
                        revision_id,
                        hash,
                        created_at,
                        created_by,
                        deleted,
                        limit_value,
                        limit_period,
                        limit_max,
                        enforcement_action,
                        unit,
                        units
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                    RETURNING
                        resource_definition_id,
                        revision_id,
                        hash,
                        created_at,
                        created_by,
                        deleted,
                        limit_value,
                        limit_period,
                        limit_max,
                        enforcement_action,
                        unit,
                        units
                "# })
            .bind(revision.resource_definition_id)
            .bind(revision.revision_id)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit)
            .bind(revision.limit.limit_value)
            .bind(revision.limit.limit_period)
            .bind(revision.limit.limit_max)
            .bind(revision.enforcement_action)
            .bind(revision.unit)
            .bind(revision.units),
        )
        .await
        .to_error_on_unique_violation(ResourceDefinitionRepoError::ConcurrentModification)
    }

    pub async fn create_within_transaction(
        tx: &mut PoolLabelledTransaction<PostgresPool>,
        args: ResourceDefinitionCreationArgs,
    ) -> Result<ResourceDefinitionExtRevisionRecord, ResourceDefinitionRepoError> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO resource_definitions
                (
                    resource_definition_id,
                    environment_id,
                    limit_type,
                    name,
                    created_at,
                    updated_at,
                    deleted_at,
                    modified_by,
                    current_revision_id
                )
                VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8)
            "# })
            .bind(args.revision.resource_definition_id)
            .bind(args.environment_id)
            .bind(args.limit_type)
            .bind(&args.name)
            .bind(&args.revision.audit.created_at)
            .bind(&args.revision.audit.created_at)
            .bind(args.revision.audit.created_by)
            .bind(args.revision.revision_id),
        )
        .await
        .to_error_on_unique_violation(
            ResourceDefinitionRepoError::ResourceDefinitionViolatesUniqueness,
        )?;

        let revision = Self::insert_revision(tx, args.revision).await?;

        let change_event = NewRegistryChangeEvent::resource_definition_changed(
            args.environment_id,
            revision.resource_definition_id,
            args.name.clone(),
        );
        DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event).await?;

        Ok(ResourceDefinitionExtRevisionRecord {
            environment_id: args.environment_id,
            limit_type: args.limit_type,
            name: args.name,
            revision,
        })
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl ResourceDefinitionRepo for DbResourceDefinitionRepo<PostgresPool> {
    async fn create(
        &self,
        args: ResourceDefinitionCreationArgs,
    ) -> Result<RequiresNotificationSignal<ResourceDefinitionExtRevisionRecord>, ResourceDefinitionRepoError> {
        self.with_tx_err("create", |tx| {
            Self::create_within_transaction(tx, args).boxed()
        })
        .await
        .map(RequiresSignalExt::requires_signal)
    }

    async fn update(
        &self,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<RequiresNotificationSignal<ResourceDefinitionExtRevisionRecord>, ResourceDefinitionRepoError> {
        self.with_tx_err("update", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision).await?;

                let main_record: ResourceDefinitionRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! { r#"
                            UPDATE resource_definitions
                            SET updated_at = $1, modified_by = $2, current_revision_id = $3, deleted_at = NULL
                            WHERE resource_definition_id = $4
                            RETURNING resource_definition_id, environment_id, limit_type, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                        .bind(&revision_record.audit.created_at)
                        .bind(revision_record.audit.created_by)
                        .bind(revision_record.revision_id)
                        .bind(revision_record.resource_definition_id)
                    )
                    .await?
                    .ok_or(ResourceDefinitionRepoError::ConcurrentModification)?;

                let change_event = NewRegistryChangeEvent::resource_definition_changed(
                    main_record.environment_id,
                    revision_record.resource_definition_id,
                    main_record.name.clone(),
                );
                DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                    .await?;

                Ok(ResourceDefinitionExtRevisionRecord {
                    environment_id: main_record.environment_id,
                    limit_type: main_record.limit_type,
                    name: main_record.name,
                    revision: revision_record,
                })
            }
            .boxed()
        })
        .await
        .map(RequiresSignalExt::requires_signal)
    }

    async fn delete(
        &self,
        revision: ResourceDefinitionRevisionRecord,
    ) -> Result<RequiresNotificationSignal<()>, ResourceDefinitionRepoError> {
        self.with_tx_err("delete", |tx| {
            async move {
                let revision_record = Self::insert_revision(tx, revision).await?;

                let main_record: ResourceDefinitionRecord = tx
                    .fetch_optional_as(
                        sqlx::query_as(indoc! { r#"
                            UPDATE resource_definitions
                            SET updated_at = $1, deleted_at = $1, modified_by = $2, current_revision_id = $3
                            WHERE resource_definition_id = $4
                            RETURNING resource_definition_id, environment_id, limit_type, name, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                        .bind(&revision_record.audit.created_at)
                        .bind(revision_record.audit.created_by)
                        .bind(revision_record.revision_id)
                        .bind(revision_record.resource_definition_id),
                    )
                    .await?
                    .ok_or(ResourceDefinitionRepoError::ConcurrentModification)?;

                let change_event = NewRegistryChangeEvent::resource_definition_changed(
                    main_record.environment_id,
                    revision_record.resource_definition_id,
                    main_record.name,
                );
                DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &change_event)
                    .await?;

                Ok(())
            }
            .boxed()
        })
        .await
        .map(RequiresSignalExt::requires_signal)
    }

    async fn get(
        &self,
        resource_definition_id: Uuid,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>> {
        self.with_ro("get_staged_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.limit_type,
                        r.name,
                        r.created_at as entity_created_at,
                        rr.resource_definition_id,
                        rr.revision_id,
                        rr.hash,
                        rr.created_at,
                        rr.created_by,
                        rr.deleted,
                        rr.limit_value,
                        rr.limit_period,
                        rr.limit_max,
                        rr.enforcement_action,
                        rr.unit,
                        rr.units
                    FROM resource_definitions r
                    JOIN resource_definition_revisions rr
                        ON rr.resource_definition_id = r.resource_definition_id AND r.current_revision_id = rr.revision_id
                    WHERE r.resource_definition_id = $1 AND r.deleted_at IS NULL
                "#})
                    .bind(resource_definition_id),
            )
            .await
    }

    async fn get_by_environment_and_name(
        &self,
        environment_id: Uuid,
        name: &str,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>> {
        self.with_ro("get_staged_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.limit_type,
                        r.name,
                        r.created_at as entity_created_at,
                        rr.resource_definition_id,
                        rr.revision_id,
                        rr.hash,
                        rr.created_at,
                        rr.created_by,
                        rr.deleted,
                        rr.limit_value,
                        rr.limit_period,
                        rr.limit_max,
                        rr.enforcement_action,
                        rr.unit,
                        rr.units
                    FROM resource_definitions r
                    JOIN resource_definition_revisions rr
                        ON rr.resource_definition_id = r.resource_definition_id AND r.current_revision_id = rr.revision_id
                    WHERE r.environment_id = $1 AND r.name = $2 AND r.deleted_at IS NULL
                "#})
                    .bind(environment_id)
                    .bind(name)
            )
            .await
    }

    async fn get_revision(
        &self,
        resource_definition_id: Uuid,
        revision_id: i64,
    ) -> RepoResult<Option<ResourceDefinitionExtRevisionRecord>> {
        self.with_ro("get_by_id_and_revision")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.limit_type,
                        r.name,
                        r.created_at as entity_created_at,
                        rr.resource_definition_id,
                        rr.revision_id,
                        rr.hash,
                        rr.created_at,
                        rr.created_by,
                        rr.deleted,
                        rr.limit_value,
                        rr.limit_period,
                        rr.limit_max,
                        rr.enforcement_action,
                        rr.unit,
                        rr.units
                    FROM resource_definitions r
                    JOIN resource_definition_revisions rr
                        ON rr.resource_definition_id = r.resource_definition_id AND r.current_revision_id = rr.revision_id
                    WHERE r.resource_definition_id = $1 AND rr.revision_id = $2 AND r.deleted_at IS NULL
                "#})
                    .bind(resource_definition_id)
                    .bind(revision_id),
            )
            .await
    }

    async fn list_in_environment(
        &self,
        environment_id: Uuid,
    ) -> RepoResult<Vec<ResourceDefinitionExtRevisionRecord>> {
        self.with_ro("list_staged")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        r.environment_id,
                        r.limit_type,
                        r.name,
                        r.created_at as entity_created_at,
                        rr.resource_definition_id,
                        rr.revision_id,
                        rr.hash,
                        rr.created_at,
                        rr.created_by,
                        rr.deleted,
                        rr.limit_value,
                        rr.limit_period,
                        rr.limit_max,
                        rr.enforcement_action,
                        rr.unit,
                        rr.units
                    FROM resource_definitions r
                    JOIN resource_definition_revisions rr
                        ON rr.resource_definition_id = r.resource_definition_id AND r.current_revision_id = rr.revision_id
                    WHERE r.environment_id = $1 AND r.deleted_at IS NULL
                    ORDER BY r.name, r.limit_type
                "#})
                    .bind(environment_id),
            )
            .await
    }
}
