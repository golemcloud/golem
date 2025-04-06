use crate::model::ProjectPluginInstallationTarget;
use crate::repo::account::{AccountRepo, DbAccountRepo};
use crate::repo::account_components::{AccountComponentsRepo, DbAccountComponentsRepo};
use crate::repo::account_connections::{AccountConnectionsRepo, DbAccountConnectionsRepo};
use crate::repo::account_fuel::{AccountFuelRepo, DbAccountFuelRepo};
use crate::repo::account_grant::{AccountGrantRepo, DbAccountGrantRepo};
use crate::repo::account_summary::{AccountSummaryRepo, DbAccountSummaryRepo};
use crate::repo::account_uploads::{AccountUploadsRepo, DbAccountUploadsRepo};
use crate::repo::account_used_storage::{AccountUsedStorageRepo, DbAccountUsedStorageRepo};
use crate::repo::account_workers::{AccountWorkersRepo, DbAccountWorkerRepo};
use crate::repo::oauth2_token::{DbOAuth2TokenRepo, OAuth2TokenRepo};
use crate::repo::plan::{DbPlanRepo, PlanRepo};
use crate::repo::project::{DbProjectRepo, ProjectRepo};
use crate::repo::project_grant::{DbProjectGrantRepo, ProjectGrantRepo};
use crate::repo::project_policy::{DbProjectPolicyRepo, ProjectPolicyRepo};
use crate::repo::token::{DbTokenRepo, TokenRepo};
use cloud_common::model::CloudPluginOwner;
use golem_service_base::db::Pool;
use golem_service_base::repo::plugin_installation::{
    DbPluginInstallationRepoQueries, PluginInstallationRepoQueries,
};
use oauth2_web_flow_state::{DbOAuth2FlowState, OAuth2WebFlowStateRepo};
use std::sync::Arc;

pub mod account;
pub mod account_components;
pub mod account_connections;
pub mod account_fuel;
pub mod account_grant;
pub mod account_summary;
pub mod account_uploads;
pub mod account_used_storage;
pub mod account_workers;
pub mod oauth2_token;
pub mod oauth2_web_flow_state;
pub mod plan;
pub mod plugin_installation;
pub mod project;
pub mod project_grant;
pub mod project_policy;
pub mod token;

#[derive(Clone)]
pub struct Repositories {
    pub plan_repo: Arc<dyn PlanRepo + Sync + Send>,
    pub account_repo: Arc<dyn AccountRepo + Sync + Send>,
    pub account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync>,
    pub account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
    pub account_connections_repo: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    pub account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send>,
    pub account_components_repo: Arc<dyn AccountComponentsRepo + Sync + Send>,
    pub account_used_storage_repo: Arc<dyn AccountUsedStorageRepo + Sync + Send>,
    pub account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
    pub account_fuel_repo: Arc<dyn AccountFuelRepo + Sync + Send>,
    pub oauth2_token_repo: Arc<dyn OAuth2TokenRepo + Sync + Send>,
    pub project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
    pub project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
    pub project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    pub token_repo: Arc<dyn TokenRepo + Sync + Send>,
    pub oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo + Sync + Send>,
}

impl Repositories {
    pub fn new<DB: Pool + Clone + Send + Sync + 'static>(db_pool: DB) -> Self
    where
        DbPlanRepo<DB>: PlanRepo,
        DbAccountRepo<DB>: AccountRepo,
        DbAccountSummaryRepo<DB>: AccountSummaryRepo,
        DbAccountGrantRepo<DB>: AccountGrantRepo,
        DbOAuth2TokenRepo<DB>: OAuth2TokenRepo,
        DbAccountConnectionsRepo<DB>: AccountConnectionsRepo,
        DbAccountWorkerRepo<DB>: AccountWorkersRepo,
        DbAccountComponentsRepo<DB>: AccountComponentsRepo,
        DbAccountUsedStorageRepo<DB>: AccountUsedStorageRepo,
        DbAccountUploadsRepo<DB>: AccountUploadsRepo,
        DbAccountFuelRepo<DB>: AccountFuelRepo,
        DbProjectPolicyRepo<DB>: ProjectPolicyRepo,
        DbProjectGrantRepo<DB>: ProjectGrantRepo,
        DbProjectRepo<DB>: ProjectRepo,
        DbTokenRepo<DB>: TokenRepo,
        DbOAuth2FlowState<DB>: OAuth2WebFlowStateRepo,
        DbPluginInstallationRepoQueries<DB::Db>: PluginInstallationRepoQueries<
            DB::Db,
            CloudPluginOwner,
            ProjectPluginInstallationTarget,
        >,
    {
        let plan_repo: Arc<dyn PlanRepo + Sync + Send> = Arc::new(DbPlanRepo::new(db_pool.clone()));

        let account_repo: Arc<dyn AccountRepo + Sync + Send> =
            Arc::new(DbAccountRepo::new(db_pool.clone()));

        let account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync> =
            Arc::new(DbAccountSummaryRepo::new(db_pool.clone()));

        let account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync> =
            Arc::new(DbAccountGrantRepo::new(db_pool.clone()));

        let oauth2_token_repo: Arc<dyn OAuth2TokenRepo + Sync + Send> =
            Arc::new(DbOAuth2TokenRepo::new(db_pool.clone()));

        let account_connections_repo: Arc<dyn AccountConnectionsRepo + Send + Sync> =
            Arc::new(DbAccountConnectionsRepo::new(db_pool.clone()));

        let account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send> =
            Arc::new(DbAccountWorkerRepo::new(db_pool.clone()));

        let account_components_repo: Arc<dyn AccountComponentsRepo + Sync + Send> =
            Arc::new(DbAccountComponentsRepo::new(db_pool.clone()));

        let account_used_storage_repo: Arc<dyn AccountUsedStorageRepo + Sync + Send> =
            Arc::new(DbAccountUsedStorageRepo::new(db_pool.clone()));

        let account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send> =
            Arc::new(DbAccountUploadsRepo::new(db_pool.clone()));

        let account_fuel_repo: Arc<dyn AccountFuelRepo + Sync + Send> =
            Arc::new(DbAccountFuelRepo::new(db_pool.clone()));

        let project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send> =
            Arc::new(DbProjectPolicyRepo::new(db_pool.clone()));

        let project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send> =
            Arc::new(DbProjectGrantRepo::new(db_pool.clone()));

        let project_repo: Arc<dyn ProjectRepo + Sync + Send> =
            Arc::new(DbProjectRepo::new(db_pool.clone()));

        let token_repo: Arc<dyn TokenRepo + Sync + Send> =
            Arc::new(DbTokenRepo::new(db_pool.clone()));

        let oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo + Sync + Send> =
            Arc::new(DbOAuth2FlowState::new(db_pool.clone()));

        Repositories {
            plan_repo,
            account_repo,
            account_summary_repo,
            account_grant_repo,
            account_connections_repo,
            account_workers_repo,
            account_components_repo,
            account_used_storage_repo,
            account_uploads_repo,
            account_fuel_repo,
            oauth2_token_repo,
            project_policy_repo,
            project_grant_repo,
            project_repo,
            token_repo,
            oauth2_web_flow_state_repo,
        }
    }
}
