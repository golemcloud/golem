use crate::repo::model::BindFields;
use crate::repo::model::plugin::PluginRecord;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{RepoResult, ResultExt};
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait PluginRepo: Send + Sync {
    async fn create(&self, plugin: PluginRecord) -> RepoResult<Option<PluginRecord>>;

    async fn get_by_id(&self, plugin_id: &Uuid) -> RepoResult<Option<PluginRecord>>;

    async fn get_by_name_and_version(
        &self,
        name: &str,
        version: &str,
    ) -> RepoResult<Option<PluginRecord>>;

    async fn list_by_account(&self, account_id: Uuid) -> RepoResult<Vec<PluginRecord>>;
}

pub struct LoggedPluginRepo<Repo: PluginRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "plugin repository";

impl<Repo: PluginRepo> LoggedPluginRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_id(plugin_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, plugin_id=%plugin_id)
    }

    fn span_account(account_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, account_id=%account_id)
    }

    fn span_name_and_version(name: &str, version: &str) -> Span {
        info_span!(SPAN_NAME, plugin_name=%name, plugin_version=%version)
    }
}

#[async_trait]
impl<Repo: PluginRepo> PluginRepo for LoggedPluginRepo<Repo> {
    async fn create(&self, plugin: PluginRecord) -> RepoResult<Option<PluginRecord>> {
        let span = Self::span_id(&plugin.plugin_id);
        self.repo.create(plugin).instrument(span).await
    }

    async fn get_by_id(&self, plugin_id: &Uuid) -> RepoResult<Option<PluginRecord>> {
        self.repo
            .get_by_id(plugin_id)
            .instrument(Self::span_id(plugin_id))
            .await
    }

    async fn get_by_name_and_version(
        &self,
        name: &str,
        version: &str,
    ) -> RepoResult<Option<PluginRecord>> {
        self.repo
            .get_by_name_and_version(name, version)
            .instrument(Self::span_name_and_version(name, version))
            .await
    }

    async fn list_by_account(&self, account_id: Uuid) -> RepoResult<Vec<PluginRecord>> {
        self.repo
            .list_by_account(account_id)
            .instrument(Self::span_account(&account_id))
            .await
    }
}

pub struct DbPluginRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "token";

impl<DBP: Pool> DbPluginRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedPluginRepo<Self>
    where
        Self: PluginRepo,
    {
        LoggedPluginRepo::new(Self::new(db_pool))
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
impl PluginRepo for DbPluginRepo<PostgresPool> {
    async fn create(&self, plugin: PluginRecord) -> RepoResult<Option<PluginRecord>> {
        self.with_rw("create").fetch_one_as(
            sqlx::query_as(indoc! {r#"
                INSERT INTO plugins
                (plugin_id, account_id, name, version,
                 created_at, created_by, deleted,
                 description, icon, homepage, plugin_type,
                 provided_wit_package,
                 json_schema, validate_url, transform_url,
                 component_id, component_revision_id,
                 blob_storage_key)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
                RETURNING
                plugin_id, account_id, name, version,
                created_at, created_by, deleted,
                description, icon, homepage, plugin_type,
                provided_wit_package,
                json_schema, validate_url, transform_url,
                component_id, component_revision_id,
                blob_storage_key
            "#})
                .bind(plugin.plugin_id)
                .bind(plugin.account_id)
                .bind(plugin.name)
                .bind(plugin.version)
                .bind_deletable_revision_audit(plugin.audit)
                .bind(plugin.description)
                .bind(plugin.icon)
                .bind(plugin.homepage)
                .bind(plugin.plugin_type)
                .bind(plugin.provided_wit_package)
                .bind(plugin.json_schema)
                .bind(plugin.validate_url)
                .bind(plugin.transform_url)
                .bind(plugin.component_id)
                .bind(plugin.component_revision_id)
                .bind(plugin.blob_storage_key),
        ).await.none_on_unique_violation()
    }

    async fn get_by_id(&self, plugin_id: &Uuid) -> RepoResult<Option<PluginRecord>> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r"
                    SELECT
                    plugin_id, account_id, name, version,
                    created_at, created_by, deleted,
                    description, icon, homepage, plugin_type,
                    provided_wit_package,
                    json_schema, validate_url, transform_url,
                    component_id, component_revision_id,
                    blob_storage_key
                    FROM plugins
                    WHERE plugin_id = $1 AND deleted = FALSE
                #"})
                .bind(plugin_id),
            )
            .await
    }

    async fn get_by_name_and_version(
        &self,
        name: &str,
        version: &str,
    ) -> RepoResult<Option<PluginRecord>> {
        self.with_ro("get_by_name_and_version")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                    plugin_id, account_id, name, version,
                    created_at, created_by, deleted,
                    description, icon, homepage, plugin_type,
                    provided_wit_package,
                    json_schema, validate_url, transform_url,
                    component_id, component_revision_id,
                    blob_storage_key
                    FROM plugins
                    WHERE name = $1 AND version = $2 AND deleted = FALSE
                "#})
                .bind(name)
                .bind(version),
            )
            .await
    }

    async fn list_by_account(&self, account_id: Uuid) -> RepoResult<Vec<PluginRecord>> {
        self.with_ro("list_by_account")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                    plugin_id, account_id, name, version,
                    created_at, created_by, deleted,
                    description, icon, homepage, plugin_type,
                    provided_wit_package,
                    json_schema, validate_url, transform_url,
                    component_id, component_revision_id,
                    blob_storage_key
                    FROM plugins
                    WHERE account_id = $1 AND deleted = FALSE
                    ORDER BY name, version
                "#})
                    .bind(account_id)
            )
            .await
    }
}
