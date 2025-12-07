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

use super::model::environment_plugin_grant::{
    EnvironmentPluginGrantRecord, EnvironmentPluginGrantRepoError,
    EnvironmentPluginGrantWithDetailsRecord,
};
use crate::repo::model::BindFields;
use crate::repo::model::datetime::SqlDateTime;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait EnvironmentPluginGrantRepo: Send + Sync {
    async fn create(
        &self,
        record: EnvironmentPluginGrantRecord,
    ) -> Result<EnvironmentPluginGrantRecord, EnvironmentPluginGrantRepoError>;

    async fn delete(
        &self,
        environment_plugin_grant_id: &Uuid,
        actor: &Uuid,
    ) -> Result<(), EnvironmentPluginGrantRepoError>;

    async fn get_by_id(
        &self,
        environment_plugin_grant_id: &Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentPluginGrantWithDetailsRecord>, EnvironmentPluginGrantRepoError>;

    async fn list_by_environment(
        &self,
        environment_id: &Uuid,
    ) -> Result<Vec<EnvironmentPluginGrantWithDetailsRecord>, EnvironmentPluginGrantRepoError>;
}

pub struct LoggedEnvironmentPluginGrantRepo<Repo: EnvironmentPluginGrantRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "environment plugin grant repository";

impl<Repo: EnvironmentPluginGrantRepo> LoggedEnvironmentPluginGrantRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_id(environment_plugin_grant_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_plugin_grant_id=%environment_plugin_grant_id)
    }

    fn span_environment(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id=%environment_id)
    }
}

#[async_trait]
impl<Repo: EnvironmentPluginGrantRepo> EnvironmentPluginGrantRepo
    for LoggedEnvironmentPluginGrantRepo<Repo>
{
    async fn create(
        &self,
        record: EnvironmentPluginGrantRecord,
    ) -> Result<EnvironmentPluginGrantRecord, EnvironmentPluginGrantRepoError> {
        let span = Self::span_id(&record.environment_plugin_grant_id);
        self.repo.create(record).instrument(span).await
    }

    async fn delete(
        &self,
        environment_plugin_grant_id: &Uuid,
        actor: &Uuid,
    ) -> Result<(), EnvironmentPluginGrantRepoError> {
        let span = Self::span_id(environment_plugin_grant_id);
        self.repo
            .delete(environment_plugin_grant_id, actor)
            .instrument(span)
            .await
    }

    async fn get_by_id(
        &self,
        environment_plugin_grant_id: &Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentPluginGrantWithDetailsRecord>, EnvironmentPluginGrantRepoError>
    {
        let span = Self::span_id(environment_plugin_grant_id);
        self.repo
            .get_by_id(environment_plugin_grant_id, include_deleted)
            .instrument(span)
            .await
    }

    async fn list_by_environment(
        &self,
        environment_id: &Uuid,
    ) -> Result<Vec<EnvironmentPluginGrantWithDetailsRecord>, EnvironmentPluginGrantRepoError> {
        let span = Self::span_environment(environment_id);
        self.repo
            .list_by_environment(environment_id)
            .instrument(span)
            .await
    }
}

pub struct DbEnvironmentPluginGrantRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment_plugin_grants";

impl<DBP: Pool> DbEnvironmentPluginGrantRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedEnvironmentPluginGrantRepo<Self>
    where
        Self: EnvironmentPluginGrantRepo,
    {
        LoggedEnvironmentPluginGrantRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentPluginGrantRepo for DbEnvironmentPluginGrantRepo<PostgresPool> {
    async fn create(
        &self,
        record: EnvironmentPluginGrantRecord,
    ) -> Result<EnvironmentPluginGrantRecord, EnvironmentPluginGrantRepoError> {
        self.with_rw("create")
            .fetch_one_as(
                sqlx::query_as(indoc! {r#"
                INSERT INTO environment_plugin_grants (
                    environment_plugin_grant_id, environment_id, plugin_id,
                    created_at, created_by, deleted_at, deleted_by
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING
                    environment_plugin_grant_id, environment_id, plugin_id,
                    created_at, created_by, deleted_at, deleted_by
            "#})
                .bind(record.environment_plugin_grant_id)
                .bind(record.environment_id)
                .bind(record.plugin_id)
                .bind_immutable_audit(record.audit),
            )
            .await
            .to_error_on_unique_violation(
                EnvironmentPluginGrantRepoError::PluginGrantViolatesUniqueness,
            )
    }

    async fn delete(
        &self,
        environment_plugin_grant_id: &Uuid,
        actor: &Uuid,
    ) -> Result<(), EnvironmentPluginGrantRepoError> {
        let deleted_at = SqlDateTime::now();

        self.with_rw("delete")
            .execute(
                sqlx::query(indoc! {r#"
                    UPDATE environment_plugin_grants
                    SET
                        deleted_at = $2, deleted_by = $3
                    WHERE
                        environment_plugin_grant_id = $1
                        AND deleted_at IS NULL
                "#})
                .bind(environment_plugin_grant_id)
                .bind(deleted_at)
                .bind(actor),
            )
            .await?;

        Ok(())
    }

    async fn get_by_id(
        &self,
        environment_plugin_grant_id: &Uuid,
        include_deleted: bool,
    ) -> Result<Option<EnvironmentPluginGrantWithDetailsRecord>, EnvironmentPluginGrantRepoError>
    {
        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        epg.environment_plugin_grant_id,
                        epg.environment_id,
                        epg.created_at,
                        epg.created_by,
                        epg.deleted_at,
                        epg.deleted_by,

                        p.plugin_id,
                        p.account_id AS plugin_account_id,
                        p.name AS plugin_name,
                        p.version AS plugin_version,
                        p.description AS plugin_description,
                        p.icon AS plugin_icon,
                        p.homepage AS plugin_homepage,
                        p.plugin_type AS plugin_plugin_type,

                        p.provided_wit_package AS plugin_provided_wit_package,
                        p.json_schema AS plugin_json_schema,
                        p.validate_url AS plugin_validate_url,
                        p.transform_url AS plugin_transform_url,

                        p.component_id AS plugin_component_id,
                        p.component_revision_id AS plugin_component_revision_id,

                        p.wasm_content_hash AS plugin_wasm_content_hash,

                        p.created_at AS plugin_created_at,
                        p.created_by AS plugin_created_by,
                        p.deleted_at AS plugin_deleted_at,
                        p.deleted_by AS plugin_deleted_by

                        FROM environment_plugin_grants epg
                        INNER JOIN plugins p
                            ON p.plugin_id = epg.plugin_id
                        INNER JOIN accounts pa
                            ON pa.account_id = p.account_id
                        WHERE
                            epg.environment_plugin_grant_id = $1
                            AND (
                                $2
                                OR (
                                    epg.deleted_at IS NULL
                                    AND p.deleted_at IS NULL
                                    AND pa.deleted_at IS NULL
                                )
                            )
                "#})
                .bind(environment_plugin_grant_id)
                .bind(include_deleted),
            )
            .await?;

        Ok(result)
    }

    async fn list_by_environment(
        &self,
        environment_id: &Uuid,
    ) -> Result<Vec<EnvironmentPluginGrantWithDetailsRecord>, EnvironmentPluginGrantRepoError> {
        let result = self
            .with_ro("list_by_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        epg.environment_plugin_grant_id,
                        epg.environment_id,
                        epg.created_at,
                        epg.created_by,
                        epg.deleted_at,
                        epg.deleted_by,

                        p.plugin_id,
                        p.account_id AS plugin_account_id,
                        p.name AS plugin_name,
                        p.version AS plugin_version,
                        p.description AS plugin_description,
                        p.icon AS plugin_icon,
                        p.homepage AS plugin_homepage,
                        p.plugin_type AS plugin_plugin_type,

                        p.provided_wit_package AS plugin_provided_wit_package,
                        p.json_schema AS plugin_json_schema,
                        p.validate_url AS plugin_validate_url,
                        p.transform_url AS plugin_transform_url,

                        p.component_id AS plugin_component_id,
                        p.component_revision_id AS plugin_component_revision_id,

                        p.wasm_content_hash AS plugin_wasm_content_hash,

                        p.created_at AS plugin_created_at,
                        p.created_by AS plugin_created_by,
                        p.deleted_at AS plugin_deleted_at,
                        p.deleted_by AS plugin_deleted_by
                    FROM environment_plugin_grants epg
                    INNER JOIN plugins p
                        ON p.plugin_id = epg.plugin_id
                    INNER JOIN accounts pa
                        ON pa.account_id = p.account_id
                    WHERE
                        epg.environment_id = $1
                        AND epg.deleted_at IS NULL
                        AND p.deleted_at IS NULL
                        AND pa.deleted_at IS NULL
                "#})
                .bind(environment_id),
            )
            .await?;

        Ok(result)
    }
}
