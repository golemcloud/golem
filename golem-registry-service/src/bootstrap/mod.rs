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

pub mod login;

use self::login::LoginSystem;
use crate::config::RegistryServiceConfig;
use crate::repo::account::{AccountRepo, DbAccountRepo};
use crate::repo::account_usage::{AccountUsageRepo, DbAccountUsageRepo};
use crate::repo::application::{ApplicationRepo, DbApplicationRepo};
use crate::repo::component::{ComponentRepo, DbComponentRepo};
use crate::repo::deployment::{DbDeploymentRepo, DeploymentRepo};
use crate::repo::domain_registration::{DbDomainRegistrationRepo, DomainRegistrationRepo};
use crate::repo::environment::{DbEnvironmentRepo, EnvironmentRepo};
use crate::repo::environment_plugin_grant::{
    DbEnvironmentPluginGrantRepo, EnvironmentPluginGrantRepo,
};
use crate::repo::environment_share::{DbEnvironmentShareRepo, EnvironmentShareRepo};
use crate::repo::http_api_deployment::{DbHttpApiDeploymentRepo, HttpApiDeploymentRepo};
use crate::repo::mcp_deployment::{DbMcpDeploymentRepo, McpDeploymentRepo};
use crate::repo::oauth2_token::{DbOAuth2TokenRepo, OAuth2TokenRepo};
use crate::repo::oauth2_webflow_state::{DbOAuth2WebflowStateRepo, OAuth2WebflowStateRepo};
use crate::repo::plan::{DbPlanRepo, PlanRepo};
use crate::repo::plugin::{DbPluginRepo, PluginRepo};
use crate::repo::reports::{DbReportsRepo, ReportsRepo};
use crate::repo::security_scheme::{DbSecuritySchemeRepo, SecuritySchemeRepo};
use crate::repo::token::{DbTokenRepo, TokenRepo};
use crate::services::account::AccountService;
use crate::services::account_usage::AccountUsageService;
use crate::services::application::ApplicationService;
use crate::services::auth::AuthService;
use crate::services::component::{ComponentService, ComponentWriteService};
use crate::services::component_compilation::ComponentCompilationService;
use crate::services::component_object_store::ComponentObjectStore;
use crate::services::component_resolver::ComponentResolverService;
use crate::services::deployment::{
    DeployedMcpService, DeployedRoutesService, DeploymentService, DeploymentWriteService,
};
use crate::services::domain_registration::DomainRegistrationService;
use crate::services::environment::EnvironmentService;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantService;
use crate::services::environment_share::EnvironmentShareService;
use crate::services::http_api_deployment::HttpApiDeploymentService;
use crate::services::mcp_deployment::McpDeploymentService;
use crate::services::plan::PlanService;
use crate::services::plugin_registration::PluginRegistrationService;
use crate::services::reports::ReportsService;
use crate::services::security_scheme::SecuritySchemeService;
use crate::services::token::TokenService;
use anyhow::{Context, anyhow};
use golem_common::IntoAnyhow;
use golem_common::config::DbConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use include_dir::include_dir;
use std::sync::Arc;

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

#[derive(Clone)]
pub struct Services {
    pub account_service: Arc<AccountService>,
    pub account_usage_service: Arc<AccountUsageService>,
    pub application_service: Arc<ApplicationService>,
    pub auth_service: Arc<AuthService>,
    pub component_compilation_service: Arc<dyn ComponentCompilationService>,
    pub component_resolver_service: Arc<ComponentResolverService>,
    pub component_service: Arc<ComponentService>,
    pub component_write_service: Arc<ComponentWriteService>,
    pub deployed_routes_service: Arc<DeployedRoutesService>,
    pub deployed_mcp_service: Arc<DeployedMcpService>,
    pub deployment_service: Arc<DeploymentService>,
    pub deployment_write_service: Arc<DeploymentWriteService>,
    pub domain_registration_service: Arc<DomainRegistrationService>,
    pub environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
    pub environment_service: Arc<EnvironmentService>,
    pub environment_share_service: Arc<EnvironmentShareService>,
    pub http_api_deployment_service: Arc<HttpApiDeploymentService>,
    pub mcp_deployment_service: Arc<McpDeploymentService>,
    pub login_system: LoginSystem,
    pub plan_service: Arc<PlanService>,
    pub plugin_registration_service: Arc<PluginRegistrationService>,
    pub reports_service: Arc<ReportsService>,
    pub security_scheme_service: Arc<SecuritySchemeService>,
    pub token_service: Arc<TokenService>,
}

struct Repos {
    account_repo: Arc<dyn AccountRepo>,
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    application_repo: Arc<dyn ApplicationRepo>,
    component_repo: Arc<dyn ComponentRepo>,
    deployment_repo: Arc<dyn DeploymentRepo>,
    domain_registration_repo: Arc<dyn DomainRegistrationRepo>,
    environment_plugin_grant_repo: Arc<dyn EnvironmentPluginGrantRepo>,
    environment_repo: Arc<dyn EnvironmentRepo>,
    environment_share_repo: Arc<dyn EnvironmentShareRepo>,
    http_api_deployment_repo: Arc<dyn HttpApiDeploymentRepo>,
    mcp_deployment_repo: Arc<dyn McpDeploymentRepo>,
    oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
    oauth2_webflow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
    plan_repo: Arc<dyn PlanRepo>,
    plugin_repo: Arc<dyn PluginRepo>,
    reports_repo: Arc<dyn ReportsRepo>,
    security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
    token_repo: Arc<dyn TokenRepo>,
}

impl Services {
    pub async fn new(config: &RegistryServiceConfig) -> anyhow::Result<Self> {
        let repos = make_repos(&config.db).await?;

        let blob_storage = make_blob_storage(&config.blob_storage).await?;

        let initial_component_files =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));
        let component_object_store = Arc::new(ComponentObjectStore::new(blob_storage));

        let component_compilation_service =
            crate::services::component_compilation::configured(&config.component_compilation);

        let account_usage_service = Arc::new(AccountUsageService::new(repos.account_usage_repo));

        let plan_service = Arc::new(PlanService::new(repos.plan_repo));
        plan_service
            .create_initial_plans(&config.initial_plans)
            .await
            .map_err(|e| e.into_anyhow())?;

        let default_plan_id = config
            .initial_plans
            .get("default")
            .ok_or(anyhow!("No default plan"))?
            .plan_id;

        let account_service = Arc::new(AccountService::new(
            repos.account_repo.clone(),
            plan_service.clone(),
            default_plan_id,
        ));
        account_service
            .create_initial_accounts(&config.initial_accounts)
            .await
            .map_err(|e| e.into_anyhow())?;

        let token_service = Arc::new(TokenService::new(repos.token_repo, account_service.clone()));
        {
            let initial_tokens = config
                .initial_accounts
                .values()
                .map(|v| (v.id, v.token.clone()))
                .collect::<Vec<_>>();
            token_service
                .create_initial_tokens(&initial_tokens)
                .await
                .map_err(|e| e.into_anyhow())?;
        }

        let auth_service = Arc::new(AuthService::new(repos.account_repo.clone()));

        let application_service = Arc::new(ApplicationService::new(
            repos.application_repo.clone(),
            account_service.clone(),
            account_usage_service.clone(),
        ));

        let environment_service = Arc::new(EnvironmentService::new(
            repos.environment_repo.clone(),
            application_service.clone(),
            account_usage_service.clone(),
        ));

        let environment_share_service = Arc::new(EnvironmentShareService::new(
            repos.environment_share_repo.clone(),
            environment_service.clone(),
        ));

        let deployment_service = Arc::new(DeploymentService::new(
            environment_service.clone(),
            application_service.clone(),
            repos.deployment_repo.clone(),
        ));

        let component_service = Arc::new(ComponentService::new(
            repos.component_repo.clone(),
            component_object_store.clone(),
            environment_service.clone(),
            deployment_service.clone(),
        ));

        let plugin_registration_service = Arc::new(PluginRegistrationService::new(
            repos.plugin_repo.clone(),
            account_service.clone(),
            component_service.clone(),
        ));

        let environment_plugin_grant_service = Arc::new(EnvironmentPluginGrantService::new(
            repos.environment_plugin_grant_repo.clone(),
            environment_service.clone(),
            plugin_registration_service.clone(),
        ));

        let component_write_service = Arc::new(ComponentWriteService::new(
            repos.component_repo,
            component_object_store,
            component_compilation_service.clone(),
            initial_component_files,
            account_usage_service.clone(),
            environment_service.clone(),
            environment_plugin_grant_service.clone(),
        ));

        let login_system = LoginSystem::new(
            &config.login,
            account_service.clone(),
            token_service.clone(),
            repos.oauth2_token_repo.clone(),
            repos.oauth2_webflow_state_repo.clone(),
        )?;

        let reports_service = Arc::new(ReportsService::new(repos.reports_repo.clone()));

        let component_resolver_service = Arc::new(ComponentResolverService::new(
            account_service.clone(),
            application_service.clone(),
            environment_service.clone(),
            component_service.clone(),
        ));

        let domain_provisioner = crate::services::domain_registration::provisioner::configured(
            &config.environment,
            &config.workspace,
            &config.domain_provisioner,
        )
        .await?;

        let domain_registration_service = Arc::new(DomainRegistrationService::new(
            repos.domain_registration_repo.clone(),
            environment_service.clone(),
            domain_provisioner.clone(),
        ));

        let security_scheme_service = Arc::new(SecuritySchemeService::new(
            repos.security_scheme_repo.clone(),
            environment_service.clone(),
        ));

        let http_api_deployment_service = Arc::new(HttpApiDeploymentService::new(
            repos.http_api_deployment_repo.clone(),
            environment_service.clone(),
            deployment_service.clone(),
            domain_registration_service.clone(),
        ));

        let mcp_deployment_service = Arc::new(McpDeploymentService::new(
            repos.mcp_deployment_repo.clone(),
            environment_service.clone(),
            domain_registration_service.clone(),
        ));

        let deployment_write_service = Arc::new(DeploymentWriteService::new(
            environment_service.clone(),
            repos.deployment_repo.clone(),
            component_service.clone(),
            http_api_deployment_service.clone(),
            mcp_deployment_service.clone(),
        ));

        let deployed_routes_service =
            Arc::new(DeployedRoutesService::new(repos.deployment_repo.clone()));

        let deployed_mcp_service = Arc::new(DeployedMcpService::new(repos.deployment_repo.clone()));

        Ok(Self {
            account_service,
            account_usage_service,
            application_service,
            auth_service,
            component_compilation_service,
            component_resolver_service,
            component_service,
            component_write_service,
            deployed_routes_service,
            deployed_mcp_service,
            deployment_service,
            deployment_write_service,
            domain_registration_service,
            environment_plugin_grant_service,
            environment_service,
            environment_share_service,
            http_api_deployment_service,
            mcp_deployment_service,
            login_system,
            plan_service,
            plugin_registration_service,
            reports_service,
            security_scheme_service,
            token_service,
        })
    }
}

async fn make_repos(db_config: &DbConfig) -> anyhow::Result<Repos> {
    let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);

    match db_config {
        DbConfig::Postgres(postgres_config) => {
            db::postgres::migrate(postgres_config, migrations.postgres_migrations())
                .await
                .context("Postgres DB migration")?;

            let db_pool: PostgresPool = PostgresPool::configured(postgres_config).await?;

            let account_repo = Arc::new(DbAccountRepo::logged(db_pool.clone()));
            let account_usage_repo = Arc::new(DbAccountUsageRepo::logged(db_pool.clone()));
            let application_repo = Arc::new(DbApplicationRepo::logged(db_pool.clone()));
            let component_repo = Arc::new(DbComponentRepo::logged(db_pool.clone()));
            let environment_repo = Arc::new(DbEnvironmentRepo::logged(db_pool.clone()));
            let plan_repo = Arc::new(DbPlanRepo::logged(db_pool.clone()));
            let token_repo = Arc::new(DbTokenRepo::logged(db_pool.clone()));
            let oauth2_token_repo = Arc::new(DbOAuth2TokenRepo::logged(db_pool.clone()));
            let oauth2_webflow_state_repo =
                Arc::new(DbOAuth2WebflowStateRepo::logged(db_pool.clone()));
            let environment_share_repo = Arc::new(DbEnvironmentShareRepo::logged(db_pool.clone()));
            let reports_repo = Arc::new(DbReportsRepo::logged(db_pool.clone()));
            let plugin_repo = Arc::new(DbPluginRepo::logged(db_pool.clone()));
            let environment_plugin_grant_repo =
                Arc::new(DbEnvironmentPluginGrantRepo::logged(db_pool.clone()));
            let deployment_repo = Arc::new(DbDeploymentRepo::logged(db_pool.clone()));
            let domain_registration_repo =
                Arc::new(DbDomainRegistrationRepo::logged(db_pool.clone()));
            let security_scheme_repo = Arc::new(DbSecuritySchemeRepo::logged(db_pool.clone()));
            let http_api_deployment_repo =
                Arc::new(DbHttpApiDeploymentRepo::logged(db_pool.clone()));
            let mcp_deployment_repo = Arc::new(DbMcpDeploymentRepo::logged(db_pool.clone()));

            Ok(Repos {
                account_repo,
                account_usage_repo,
                application_repo,
                component_repo,
                deployment_repo,
                domain_registration_repo,
                environment_plugin_grant_repo,
                environment_repo,
                environment_share_repo,
                http_api_deployment_repo,
                mcp_deployment_repo,
                oauth2_token_repo,
                oauth2_webflow_state_repo,
                plan_repo,
                plugin_repo,
                reports_repo,
                security_scheme_repo,
                token_repo,
            })
        }
        DbConfig::Sqlite(sqlite_config) => {
            db::sqlite::migrate(sqlite_config, migrations.sqlite_migrations())
                .await
                .context("Sqlite DB migration")?;

            let db_pool = SqlitePool::configured(sqlite_config).await?;

            let account_repo = Arc::new(DbAccountRepo::logged(db_pool.clone()));
            let account_usage_repo = Arc::new(DbAccountUsageRepo::logged(db_pool.clone()));
            let application_repo = Arc::new(DbApplicationRepo::logged(db_pool.clone()));
            let component_repo = Arc::new(DbComponentRepo::logged(db_pool.clone()));
            let environment_repo = Arc::new(DbEnvironmentRepo::logged(db_pool.clone()));
            let plan_repo = Arc::new(DbPlanRepo::logged(db_pool.clone()));
            let token_repo = Arc::new(DbTokenRepo::logged(db_pool.clone()));
            let oauth2_token_repo = Arc::new(DbOAuth2TokenRepo::logged(db_pool.clone()));
            let oauth2_webflow_state_repo =
                Arc::new(DbOAuth2WebflowStateRepo::logged(db_pool.clone()));
            let environment_share_repo = Arc::new(DbEnvironmentShareRepo::logged(db_pool.clone()));
            let reports_repo = Arc::new(DbReportsRepo::logged(db_pool.clone()));
            let plugin_repo = Arc::new(DbPluginRepo::logged(db_pool.clone()));
            let environment_plugin_grant_repo =
                Arc::new(DbEnvironmentPluginGrantRepo::logged(db_pool.clone()));
            let deployment_repo = Arc::new(DbDeploymentRepo::logged(db_pool.clone()));
            let domain_registration_repo =
                Arc::new(DbDomainRegistrationRepo::logged(db_pool.clone()));
            let security_scheme_repo = Arc::new(DbSecuritySchemeRepo::logged(db_pool.clone()));
            let http_api_deployment_repo =
                Arc::new(DbHttpApiDeploymentRepo::logged(db_pool.clone()));
            let mcp_deployment_repo = Arc::new(DbMcpDeploymentRepo::logged(db_pool.clone()));

            Ok(Repos {
                account_repo,
                account_usage_repo,
                application_repo,
                component_repo,
                deployment_repo,
                domain_registration_repo,
                environment_plugin_grant_repo,
                environment_repo,
                environment_share_repo,
                http_api_deployment_repo,
                mcp_deployment_repo,
                oauth2_token_repo,
                oauth2_webflow_state_repo,
                plan_repo,
                plugin_repo,
                reports_repo,
                security_scheme_repo,
                token_repo,
            })
        }
    }
}

async fn make_blob_storage(
    blob_storage_config: &BlobStorageConfig,
) -> anyhow::Result<Arc<dyn BlobStorage>> {
    match blob_storage_config {
        BlobStorageConfig::S3(config) => {
            let blob_storage =
                golem_service_base::storage::blob::s3::S3BlobStorage::new(config.clone()).await;
            Ok(Arc::new(blob_storage))
        }
        BlobStorageConfig::LocalFileSystem(config) => {
            let blob_storage =
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await?;
            Ok(Arc::new(blob_storage))
        }
        BlobStorageConfig::Sqlite(sqlite) => {
            let pool = SqlitePool::configured(sqlite).await?;
            let blob_storage = SqliteBlobStorage::new(pool.clone()).await?;
            Ok(Arc::new(blob_storage))
        }
        BlobStorageConfig::InMemory(_) => {
            let blob_storage =
                golem_service_base::storage::blob::memory::InMemoryBlobStorage::new();
            Ok(Arc::new(blob_storage))
        }
        _ => Err(anyhow!("Unsupported blob storage configuration")),
    }
}
