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

use crate::config::CloudServiceConfig;
use crate::login;
use crate::login::LoginSystem;
use crate::model::ProjectPluginInstallationTarget;
use crate::repo;
use crate::service;
use crate::service::api_mapper::ApiMapper;
use golem_common::config::DbConfig;
use golem_service_base::clients::plugin::PluginServiceClientDefault;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::Pool;
use std::sync::Arc;

#[derive(Clone)]
pub struct Services {
    pub auth_service: Arc<dyn service::auth::AuthService>,
    pub account_service: Arc<dyn service::account::AccountService>,
    pub account_grant_service: Arc<dyn service::account_grant::AccountGrantService>,
    pub account_summary_service: Arc<dyn service::account_summary::AccountSummaryService>,
    pub plan_service: Arc<dyn service::plan::PlanService>,
    pub plan_limit_service: Arc<dyn service::plan_limit::PlanLimitService>,
    pub token_service: Arc<dyn service::token::TokenService>,
    pub project_service: Arc<dyn service::project::ProjectService>,
    pub project_policy_service: Arc<dyn service::project_policy::ProjectPolicyService>,
    pub project_grant_service: Arc<dyn service::project_grant::ProjectGrantService>,
    pub api_mapper: Arc<ApiMapper>,
    pub login_system: Arc<LoginSystem>,
}

impl Services {
    pub async fn new(config: &CloudServiceConfig) -> Result<Self, String> {
        match config.db.clone() {
            DbConfig::Postgres(db_config) => {
                let db_pool = PostgresPool::configured(&db_config)
                    .await
                    .map_err(|e| e.to_string())?;
                Self::make_with_db(config, db_pool).await
            }
            DbConfig::Sqlite(db_config) => {
                let db_pool = SqlitePool::configured(&db_config)
                    .await
                    .map_err(|e| e.to_string())?;
                Self::make_with_db(config, db_pool).await
            }
        }
    }

    async fn make_with_db<DB>(config: &CloudServiceConfig, db_pool: DB) -> Result<Self, String>
    where
        DB: Pool + Clone + Send + Sync + 'static,
        repo::plan::DbPlanRepo<DB>: repo::plan::PlanRepo,
        repo::account::DbAccountRepo<DB>: repo::account::AccountRepo,
        repo::account_summary::DbAccountSummaryRepo<DB>: repo::account_summary::AccountSummaryRepo,
        repo::account_grant::DbAccountGrantRepo<DB>: repo::account_grant::AccountGrantRepo,
        repo::account_connections::DbAccountConnectionsRepo<DB>:
            repo::account_connections::AccountConnectionsRepo,
        repo::account_workers::DbAccountWorkerRepo<DB>: repo::account_workers::AccountWorkersRepo,
        repo::account_components::DbAccountComponentsRepo<DB>:
            repo::account_components::AccountComponentsRepo,
        repo::account_used_storage::DbAccountUsedStorageRepo<DB>:
            repo::account_used_storage::AccountUsedStorageRepo,
        repo::account_uploads::DbAccountUploadsRepo<DB>: repo::account_uploads::AccountUploadsRepo,
        repo::account_fuel::DbAccountFuelRepo<DB>: repo::account_fuel::AccountFuelRepo,
        repo::project_policy::DbProjectPolicyRepo<DB>: repo::project_policy::ProjectPolicyRepo,
        repo::project_grant::DbProjectGrantRepo<DB>: repo::project_grant::ProjectGrantRepo,
        repo::project::DbProjectRepo<DB>: repo::project::ProjectRepo,
        repo::token::DbTokenRepo<DB>: repo::token::TokenRepo,
        golem_service_base::repo::plugin_installation::DbPluginInstallationRepoQueries<DB::Db>:
            golem_service_base::repo::plugin_installation::PluginInstallationRepoQueries<
                DB::Db,
                ProjectPluginInstallationTarget,
            >,
        login::DbOAuth2TokenRepo<DB>: login::OAuth2TokenRepo,
        login::DbOAuth2FlowState<DB>: login::OAuth2WebFlowStateRepo,
    {
        let plan_repo: Arc<dyn repo::plan::PlanRepo> =
            Arc::new(repo::plan::DbPlanRepo::new(db_pool.clone()));

        let account_repo: Arc<dyn repo::account::AccountRepo> =
            Arc::new(repo::account::DbAccountRepo::new(db_pool.clone()));

        let account_summary_repo: Arc<dyn repo::account_summary::AccountSummaryRepo> = Arc::new(
            repo::account_summary::DbAccountSummaryRepo::new(db_pool.clone()),
        );

        let account_grant_repo: Arc<dyn repo::account_grant::AccountGrantRepo> = Arc::new(
            repo::account_grant::DbAccountGrantRepo::new(db_pool.clone()),
        );

        let account_connections_repo: Arc<dyn repo::account_connections::AccountConnectionsRepo> =
            Arc::new(repo::account_connections::DbAccountConnectionsRepo::new(
                db_pool.clone(),
            ));

        let account_workers_repo: Arc<dyn repo::account_workers::AccountWorkersRepo> = Arc::new(
            repo::account_workers::DbAccountWorkerRepo::new(db_pool.clone()),
        );

        let account_components_repo: Arc<dyn repo::account_components::AccountComponentsRepo> =
            Arc::new(repo::account_components::DbAccountComponentsRepo::new(
                db_pool.clone(),
            ));

        let account_used_storage_repo: Arc<dyn repo::account_used_storage::AccountUsedStorageRepo> =
            Arc::new(repo::account_used_storage::DbAccountUsedStorageRepo::new(
                db_pool.clone(),
            ));

        let account_uploads_repo: Arc<dyn repo::account_uploads::AccountUploadsRepo> = Arc::new(
            repo::account_uploads::DbAccountUploadsRepo::new(db_pool.clone()),
        );

        let account_fuel_repo: Arc<dyn repo::account_fuel::AccountFuelRepo> =
            Arc::new(repo::account_fuel::DbAccountFuelRepo::new(db_pool.clone()));

        let project_policy_repo: Arc<dyn repo::project_policy::ProjectPolicyRepo> = Arc::new(
            repo::project_policy::DbProjectPolicyRepo::new(db_pool.clone()),
        );

        let project_grant_repo: Arc<dyn repo::project_grant::ProjectGrantRepo> = Arc::new(
            repo::project_grant::DbProjectGrantRepo::new(db_pool.clone()),
        );

        let project_repo: Arc<dyn repo::project::ProjectRepo> =
            Arc::new(repo::project::DbProjectRepo::new(db_pool.clone()));

        let token_repo: Arc<dyn repo::token::TokenRepo> =
            Arc::new(repo::token::DbTokenRepo::new(db_pool.clone()));

        let token_service: Arc<dyn service::token::TokenService> = Arc::new(
            service::token::TokenServiceDefault::new(token_repo.clone(), account_repo.clone()),
        );

        let auth_service: Arc<dyn service::auth::AuthService> =
            Arc::new(service::auth::AuthServiceDefault::new(
                token_service.clone(),
                account_repo.clone(),
                account_grant_repo.clone(),
                project_repo.clone(),
                project_policy_repo.clone(),
                project_grant_repo.clone(),
            ));

        let plan_service: Arc<dyn service::plan::PlanService> = Arc::new(
            service::plan::PlanServiceDefault::new(plan_repo.clone(), config.plans.clone()),
        );

        let plan_limit_service: Arc<dyn service::plan_limit::PlanLimitService> =
            Arc::new(service::plan_limit::PlanLimitServiceDefault::new(
                plan_repo.clone(),
                account_repo.clone(),
                account_workers_repo.clone(),
                account_connections_repo.clone(),
                account_components_repo.clone(),
                account_used_storage_repo.clone(),
                account_uploads_repo.clone(),
                project_repo.clone(),
                account_fuel_repo.clone(),
            ));

        let account_service: Arc<dyn service::account::AccountService> =
            Arc::new(service::account::AccountServiceDefault::new(
                account_repo.clone(),
                plan_service.clone(),
            ));

        let account_summary_service: Arc<dyn service::account_summary::AccountSummaryService> =
            Arc::new(service::account_summary::AccountSummaryServiceDefault::new(
                account_summary_repo,
            ));

        let account_grant_service: Arc<dyn service::account_grant::AccountGrantService> =
            Arc::new(service::account_grant::AccountGrantServiceDefault::new(
                account_grant_repo.clone(),
                account_repo.clone(),
            ));

        let project_policy_service: Arc<dyn service::project_policy::ProjectPolicyService> =
            Arc::new(service::project_policy::ProjectPolicyServiceDefault::new(
                project_policy_repo.clone(),
            ));

        let project_grant_service: Arc<dyn service::project_grant::ProjectGrantService> =
            Arc::new(service::project_grant::ProjectGrantServiceDefault::new(
                project_grant_repo.clone(),
                project_policy_repo.clone(),
                account_repo.clone(),
            ));

        let plugin_service_client =
            Arc::new(PluginServiceClientDefault::new(&config.component_service));

        let project_service: Arc<dyn service::project::ProjectService> =
            Arc::new(service::project::ProjectServiceDefault::new(
                auth_service.clone(),
                project_repo.clone(),
                plan_limit_service.clone(),
                plugin_service_client.clone(),
            ));

        let api_mapper = Arc::new(ApiMapper::new(plugin_service_client));

        let login_system = Arc::new(LoginSystem::new(
            &config.login,
            &account_service,
            &token_service,
            &db_pool,
        ));

        Ok(Self {
            auth_service,
            account_service,
            account_grant_service,
            account_summary_service,
            plan_service,
            plan_limit_service,
            project_policy_service,
            project_grant_service,
            project_service,
            token_service,
            api_mapper,
            login_system,
        })
    }
}
