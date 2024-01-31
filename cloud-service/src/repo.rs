use crate::repo::account::{AccountRepo, DbAccountRepo};
use crate::repo::account_connections::{AccountConnectionsRepo, DbAccountConnectionsRepo};
use crate::repo::account_fuel::{AccountFuelRepo, DbAccountFuelRepo};
use crate::repo::account_grant::{AccountGrantRepo, DbAccountGrantRepo};
use crate::repo::account_summary::{AccountSummaryRepo, DbAccountSummaryRepo};
use crate::repo::account_uploads::{AccountUploadsRepo, DbAccountUploadsRepo};
use crate::repo::account_workers::{AccountWorkersRepo, DbAccountWorkerRepo};
use crate::repo::oauth2_token::{DbOAuth2TokenRepo, OAuth2TokenRepo};
use crate::repo::plan::{DbPlanRepo, PlanRepo};
use crate::repo::project::{DbProjectRepo, ProjectRepo};
use crate::repo::project_grant::{DbProjectGrantRepo, ProjectGrantRepo};
use crate::repo::project_policy::{DbProjectPolicyRepo, ProjectPolicyRepo};
use crate::repo::template::{DbTemplateRepo, TemplateRepo};
use crate::repo::token::{DbTokenRepo, TokenRepo};
use sqlx::{Pool, Postgres, Sqlite};
use std::fmt::Display;
use std::sync::Arc;

pub mod account;
pub mod account_connections;
pub mod account_fuel;
pub mod account_grant;
pub mod account_summary;
pub mod account_uploads;
pub mod account_workers;
pub mod oauth2_token;
pub mod plan;
pub mod project;
pub mod project_grant;
pub mod project_policy;
pub mod template;
pub mod token;

#[derive(Debug)]
pub enum RepoError {
    Internal(String),
}

impl From<sqlx::Error> for RepoError {
    fn from(error: sqlx::Error) -> Self {
        RepoError::Internal(error.to_string())
    }
}

impl Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoError::Internal(error) => write!(f, "{}", error),
        }
    }
}

#[derive(Clone)]
pub struct Repositories {
    pub plan_repo: Arc<dyn PlanRepo + Sync + Send>,
    pub account_repo: Arc<dyn AccountRepo + Sync + Send>,
    pub account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync>,
    pub account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
    pub account_connections_repo: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    pub account_workers_repo: Arc<dyn AccountWorkersRepo + Sync + Send>,
    pub account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
    pub account_fuel_repo: Arc<dyn AccountFuelRepo + Sync + Send>,
    pub oauth2_token_repo: Arc<dyn OAuth2TokenRepo + Sync + Send>,
    pub project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
    pub project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
    pub project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    pub template_repo: Arc<dyn TemplateRepo + Sync + Send>,
    pub token_repo: Arc<dyn TokenRepo + Sync + Send>,
}

impl Repositories {
    pub fn new_postgres(db_pool: Arc<Pool<Postgres>>) -> Self {
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

        let template_repo: Arc<dyn TemplateRepo + Sync + Send> =
            Arc::new(DbTemplateRepo::new(db_pool.clone()));

        let token_repo: Arc<dyn TokenRepo + Sync + Send> =
            Arc::new(DbTokenRepo::new(db_pool.clone()));

        Repositories {
            plan_repo,
            account_repo,
            account_summary_repo,
            account_grant_repo,
            account_connections_repo,
            account_workers_repo,
            account_uploads_repo,
            account_fuel_repo,
            oauth2_token_repo,
            project_policy_repo,
            project_grant_repo,
            project_repo,
            template_repo,
            token_repo,
        }
    }

    pub fn new_sqlite(db_pool: Arc<Pool<Sqlite>>) -> Self {
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

        let template_repo: Arc<dyn TemplateRepo + Sync + Send> =
            Arc::new(DbTemplateRepo::new(db_pool.clone()));

        let token_repo: Arc<dyn TokenRepo + Sync + Send> =
            Arc::new(DbTokenRepo::new(db_pool.clone()));

        Repositories {
            plan_repo,
            account_repo,
            account_summary_repo,
            account_grant_repo,
            account_connections_repo,
            account_workers_repo,
            account_uploads_repo,
            account_fuel_repo,
            oauth2_token_repo,
            project_policy_repo,
            project_grant_repo,
            project_repo,
            template_repo,
            token_repo,
        }
    }
}
