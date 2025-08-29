use super::model::environment_plugin_grant::{
    EnvironmentPluginGrantRecord, EnvironmentPluginGrantRepoError,
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
    ) -> Result<Option<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError>;

    async fn get_by_id(
        &self,
        environment_plugin_grant_id: &Uuid,
    ) -> Result<Option<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError>;

    async fn list_by_environment(
        &self,
        environment_id: &Uuid,
    ) -> Result<Vec<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError>;
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
        info_span!(SPAN_NAME, envenvironment_plugin_grant_idironment_id=%environment_plugin_grant_id)
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
    ) -> Result<Option<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError> {
        let span = Self::span_id(environment_plugin_grant_id);
        self.repo
            .delete(environment_plugin_grant_id, actor)
            .instrument(span)
            .await
    }

    async fn get_by_id(
        &self,
        environment_plugin_grant_id: &Uuid,
    ) -> Result<Option<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError> {
        let span = Self::span_id(environment_plugin_grant_id);
        self.repo
            .get_by_id(environment_plugin_grant_id)
            .instrument(span)
            .await
    }

    async fn list_by_environment(
        &self,
        environment_id: &Uuid,
    ) -> Result<Vec<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError> {
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
    ) -> Result<Option<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError> {
        let deleted_at = SqlDateTime::now();

        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    UPDATE environment_plugin_grants
                    SET
                        deleted_at = $2, deleted_by = $3
                    WHERE
                        environment_plugin_grant_id = $1
                        AND deleted_at IS NULL
                    RETURNING
                        environment_plugin_grant_id, environment_id, plugin_id,
                        created_at, created_by, deleted_at, deleted_by
                "#})
                .bind(environment_plugin_grant_id)
                .bind(deleted_at)
                .bind(actor),
            )
            .await?;

        Ok(result)
    }

    async fn get_by_id(
        &self,
        environment_plugin_grant_id: &Uuid,
    ) -> Result<Option<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError> {
        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        environment_plugin_grant_id, environment_id, plugin_id,
                        created_at, created_by, deleted_at, deleted_by
                    FROM environment_plugin_grants
                    WHERE
                        environment_plugin_grant_id = $1
                        AND deleted_at IS NULL
                "#})
                .bind(environment_plugin_grant_id),
            )
            .await?;

        Ok(result)
    }

    async fn list_by_environment(
        &self,
        environment_id: &Uuid,
    ) -> Result<Vec<EnvironmentPluginGrantRecord>, EnvironmentPluginGrantRepoError> {
        let result = self
            .with_ro("list_by_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        environment_plugin_grant_id, environment_id, plugin_id,
                        created_at, created_by, deleted_at, deleted_by
                    FROM environment_plugin_grants
                    WHERE
                        environment_id = $1
                        AND deleted_at IS NULL
                "#})
                .bind(environment_id),
            )
            .await?;

        Ok(result)
    }

    // async fn delete(&self, plugin_id: &Uuid, actor: &Uuid) -> RepoResult<Option<PluginRecord>> {
    //     let deleted_at = SqlDateTime::now();

    //     self.with_rw("delete")
    //         .fetch_optional_as(
    //             sqlx::query_as(indoc! {r#"
    //                 UPDATE plugins
    //                 SET
    //                     deleted_at = $2, deleted_by = $3
    //                 WHERE
    //                     plugin_id = $1 AND deleted_at is NULL
    //                 RETURNING
    //                     plugin_id, account_id, name, version,
    //                     created_at, created_by, deleted_at, deleted_by,
    //                     description, icon, homepage, plugin_type,
    //                     provided_wit_package,
    //                     json_schema, validate_url, transform_url,
    //                     component_id, component_revision_id,
    //                     blob_storage_key
    //             "#})
    //             .bind(plugin_id)
    //             .bind(deleted_at)
    //             .bind(actor),
    //         )
    //         .await
    // }

    // async fn get_by_id(&self, plugin_id: &Uuid) -> RepoResult<Option<PluginRecord>> {
    //     self.with_ro("get_by_id")
    //         .fetch_optional_as(
    //             sqlx::query_as(indoc! { r#"
    //                 SELECT
    //                     p.plugin_id, p.account_id, p.name, p.version,
    //                     p.created_at, p.created_by, p.deleted_at, p.deleted_by,
    //                     p.description, p.icon, p.homepage, p.plugin_type,
    //                     p.provided_wit_package,
    //                     p.json_schema, p.validate_url, p.transform_url,
    //                     p.component_id, p.component_revision_id,
    //                     p.blob_storage_key
    //                 FROM accounts a
    //                 JOIN plugins p
    //                     ON p.account_id = a.account_id
    //                 WHERE
    //                     p.plugin_id = $1
    //                     AND a.deleted_at IS NULL
    //                     AND p.deleted_at IS NULL
    //             "#})
    //             .bind(plugin_id),
    //         )
    //         .await
    // }

    // async fn get_by_name_and_version(
    //     &self,
    //     account_id: &Uuid,
    //     name: &str,
    //     version: &str,
    // ) -> RepoResult<Option<PluginRecord>> {
    //     self.with_ro("get_by_name_and_version")
    //         .fetch_optional_as(
    //             sqlx::query_as(indoc! {r#"
    //                 SELECT
    //                     p.plugin_id, p.account_id, p.name, p.version,
    //                     p.created_at, p.created_by, p.deleted_at, p.deleted_by,
    //                     p.description, p.icon, p.homepage, p.plugin_type,
    //                     p.provided_wit_package,
    //                     p.json_schema, p.validate_url, p.transform_url,
    //                     p.component_id, p.component_revision_id,
    //                     p.blob_storage_key
    //                 FROM accounts a
    //                 JOIN plugins p
    //                     ON p.account_id = a.account_id
    //                 WHERE
    //                     a.account_id = $1
    //                     AND p.name = $2
    //                     AND p.version = $3
    //                     AND a.deleted_at IS NULL
    //                     AND p.deleted_at IS NULL
    //             "#})
    //             .bind(account_id)
    //             .bind(name)
    //             .bind(version),
    //         )
    //         .await
    // }

    // async fn list_by_account(&self, account_id: &Uuid) -> RepoResult<Vec<PluginRecord>> {
    //     self.with_ro("list_by_account")
    //         .fetch_all_as(
    //             sqlx::query_as(indoc! {r#"
    //                 SELECT
    //                     p.plugin_id, p.account_id, p.name, p.version,
    //                     p.created_at, p.created_by, p.deleted_at, p.deleted_by,
    //                     p.description, p.icon, p.homepage, p.plugin_type,
    //                     p.provided_wit_package,
    //                     p.json_schema, p.validate_url, p.transform_url,
    //                     p.component_id, p.component_revision_id,
    //                     p.blob_storage_key
    //                 FROM accounts a
    //                 JOIN plugins p
    //                     ON p.account_id = a.account_id
    //                 WHERE
    //                     a.account_id = $1
    //                     AND a.deleted_at IS NULL
    //                     AND p.deleted_at IS NULL
    //             "#})
    //             .bind(account_id),
    //         )
    //         .await
    // }
}
