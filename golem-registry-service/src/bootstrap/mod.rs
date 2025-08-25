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
use crate::repo::environment::{DbEnvironmentRepo, EnvironmentRepo};
use crate::repo::environment_share::{DbEnvironmentShareRepo, EnvironmentShareRepo};
use crate::repo::oauth2_token::{DbOAuth2TokenRepo, OAuth2TokenRepo};
use crate::repo::oauth2_webflow_state::{DbOAuth2WebflowStateRepo, OAuth2WebflowStateRepo};
use crate::repo::plan::{DbPlanRepo, PlanRepo};
use crate::repo::token::{DbTokenRepo, TokenRepo};
use crate::services::account::AccountService;
use crate::services::account_usage::AccountUsageService;
use crate::services::application::ApplicationService;
use crate::services::component::ComponentService;
use crate::services::component_compilation::ComponentCompilationServiceDisabled;
use crate::services::component_object_store::ComponentObjectStore;
use crate::services::environment::EnvironmentService;
use crate::services::environment_share::EnvironmentShareService;
use crate::services::plan::PlanService;
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
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use include_dir::include_dir;
use std::sync::Arc;

static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

#[derive(Clone)]
pub struct Services {
    pub account_service: Arc<AccountService>,
    pub application_service: Arc<ApplicationService>,
    pub component_service: Arc<ComponentService>,
    pub environment_service: Arc<EnvironmentService>,
    pub login_system: LoginSystem,
    pub plan_service: Arc<PlanService>,
    pub token_service: Arc<TokenService>,
    pub environment_share_service: Arc<EnvironmentShareService>,
}

struct Repos {
    account_repo: Arc<dyn AccountRepo>,
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    application_repo: Arc<dyn ApplicationRepo>,
    component_repo: Arc<dyn ComponentRepo>,
    environment_repo: Arc<dyn EnvironmentRepo>,
    plan_repo: Arc<dyn PlanRepo>,
    token_repo: Arc<dyn TokenRepo>,
    oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
    oauth2_webflow_state_repo: Arc<dyn OAuth2WebflowStateRepo>,
    environment_share_repo: Arc<dyn EnvironmentShareRepo>,
}

impl Services {
    pub async fn new(config: &RegistryServiceConfig) -> anyhow::Result<Self> {
        let repos = make_repos(&config.db).await?;

        let blob_storage = make_blob_storage(&config.blob_storage).await?;

        let initial_component_files =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));
        let plugin_wasm_files = Arc::new(PluginWasmFilesService::new(blob_storage.clone()));
        let component_object_store = Arc::new(ComponentObjectStore::new(blob_storage));

        let component_compilation_service = Arc::new(ComponentCompilationServiceDisabled);

        let account_usage_service = Arc::new(AccountUsageService::new(repos.account_usage_repo));

        let plan_service = Arc::new(PlanService::new(repos.plan_repo, config.plans.clone()));
        plan_service
            .create_initial_plans()
            .await
            .map_err(|e| e.into_anyhow())?;

        let token_service = Arc::new(TokenService::new(repos.token_repo));

        let account_service = Arc::new(AccountService::new(
            repos.account_repo.clone(),
            plan_service.clone(),
            token_service.clone(),
            config.accounts.clone(),
        ));
        account_service
            .create_initial_accounts()
            .await
            .map_err(|e| e.into_anyhow())?;

        let application_service = Arc::new(ApplicationService::new(repos.application_repo.clone()));

        let environment_service = Arc::new(EnvironmentService::new(repos.environment_repo.clone()));

        let environment_share_service = Arc::new(EnvironmentShareService::new(
            repos.environment_share_repo.clone(),
        ));

        let component_service = Arc::new(ComponentService::new(
            repos.component_repo,
            component_object_store,
            component_compilation_service,
            initial_component_files,
            plugin_wasm_files,
            account_usage_service,
            environment_service.clone(),
            application_service.clone(),
        ));

        let login_system = LoginSystem::new(
            &config.login,
            account_service.clone(),
            token_service.clone(),
            repos.oauth2_token_repo.clone(),
            repos.oauth2_webflow_state_repo.clone(),
        )?;

        Ok(Self {
            account_service,
            application_service,
            component_service,
            environment_service,
            token_service,
            login_system,
            plan_service,
            environment_share_service,
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

            Ok(Repos {
                account_repo,
                account_usage_repo,
                application_repo,
                component_repo,
                environment_repo,
                plan_repo,
                token_repo,
                oauth2_token_repo,
                oauth2_webflow_state_repo,
                environment_share_repo,
            })
        }
        DbConfig::Sqlite(sqlite_config) => {
            db::sqlite::migrate(sqlite_config, migrations.postgres_migrations())
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

            Ok(Repos {
                account_repo,
                account_usage_repo,
                application_repo,
                component_repo,
                environment_repo,
                plan_repo,
                token_repo,
                oauth2_token_repo,
                oauth2_webflow_state_repo,
                environment_share_repo,
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
