use golem_service_base::config::TemplateStoreConfig;
use golem_service_base::service::template_object_store;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::config::{CloudServiceConfig, DbConfig};
use crate::db;
use crate::repo;

pub mod account;
pub mod account_grant;
pub mod account_summary;
pub mod auth;
pub mod login;
pub mod oauth2;
pub mod oauth2_github_client;
pub mod oauth2_provider_client;
pub mod oauth2_session;
pub mod oauth2_token;
pub mod plan;
pub mod plan_limit;
pub mod project;
pub mod project_auth;
pub mod project_grant;
pub mod project_policy;
pub mod template;
pub mod token;
pub mod worker;

#[derive(Clone)]
pub struct Services {
    pub auth_service: Arc<dyn auth::AuthService + Sync + Send>,
    pub account_service: Arc<dyn account::AccountService + Sync + Send>,
    pub account_grant_service: Arc<dyn account_grant::AccountGrantService + Sync + Send>,
    pub account_summary_service: Arc<dyn account_summary::AccountSummaryService + Sync + Send>,
    pub plan_service: Arc<dyn plan::PlanService + Sync + Send>,
    pub plan_limit_service: Arc<dyn plan_limit::PlanLimitService + Sync + Send>,
    pub oauth2_token_service: Arc<dyn oauth2_token::OAuth2TokenService + Sync + Send>,
    pub oauth2_session_service: Arc<dyn oauth2_session::OAuth2SessionService + Sync + Send>,
    pub oauth2_service: Arc<dyn oauth2::OAuth2Service + Sync + Send>,
    pub token_service: Arc<dyn token::TokenService + Sync + Send>,
    pub login_service: Arc<dyn login::LoginService + Sync + Send>,
    pub template_service: Arc<dyn template::TemplateService + Sync + Send>,
    pub worker_service: Arc<dyn worker::WorkerService + Sync + Send>,
    pub project_auth_service: Arc<dyn project_auth::ProjectAuthorisationService + Sync + Send>,
    pub project_service: Arc<dyn project::ProjectService + Sync + Send>,
    pub project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send>,
    pub project_grant_service: Arc<dyn project_grant::ProjectGrantService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &CloudServiceConfig) -> Result<Services, String> {
        let repositories = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c, &config.workspace)
                    .await
                    .map_err(|e| e.to_string())?;
                repo::Repositories::new_postgres(Arc::new(db_pool))
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                repo::Repositories::new_sqlite(Arc::new(db_pool))
            }
        };

        let plan_service: Arc<dyn plan::PlanService + Sync + Send> = Arc::new(
            plan::PlanServiceDefault::new(repositories.plan_repo.clone(), config.plans.clone()),
        );

        let plan_limit_service: Arc<dyn plan_limit::PlanLimitService + Sync + Send> =
            Arc::new(plan_limit::PlanLimitServiceDefault::new(
                repositories.plan_repo.clone(),
                repositories.account_repo.clone(),
                repositories.account_workers_repo.clone(),
                repositories.account_uploads_repo.clone(),
                repositories.project_repo.clone(),
                repositories.template_repo.clone(),
                repositories.account_fuel_repo.clone(),
            ));

        let account_service: Arc<dyn account::AccountService + Sync + Send> = Arc::new(
            account::AccountServiceDefault::new(repositories.account_repo, plan_service.clone()),
        );

        let account_summary_service: Arc<dyn account_summary::AccountSummaryService + Sync + Send> =
            Arc::new(account_summary::AccountSummaryServiceDefault::new(
                repositories.account_summary_repo,
            ));

        let account_grant_service: Arc<dyn account_grant::AccountGrantService + Sync + Send> =
            Arc::new(account_grant::AccountGrantServiceDefault::new(
                repositories.account_grant_repo,
            ));

        let oauth2_token_service: Arc<dyn oauth2_token::OAuth2TokenService + Sync + Send> =
            Arc::new(oauth2_token::OAuth2TokenServiceDefault::new(
                repositories.oauth2_token_repo,
            ));

        let oauth2_session_service: Arc<dyn oauth2_session::OAuth2SessionService + Sync + Send> =
            Arc::new(
                oauth2_session::OAuth2SessionServiceDefault::from_config(&config.ed_dsa)
                    .expect("Valid Public and Private Keys"),
            );

        let oauth2_github_client: Arc<dyn oauth2_github_client::OAuth2GithubClient + Sync + Send> =
            Arc::new(oauth2_github_client::OAuth2GithubClientDefault {
                config: config.oauth2.clone(),
            });

        let oauth2_provider_client: Arc<
            dyn oauth2_provider_client::OAuth2ProviderClient + Sync + Send,
        > = Arc::new(oauth2_provider_client::OAuth2ProviderClientDefault {});

        let oauth2_service: Arc<dyn oauth2::OAuth2Service + Sync + Send> = Arc::new(
            oauth2::OAuth2ServiceDefault::new(oauth2_github_client, oauth2_session_service.clone()),
        );

        let token_service: Arc<dyn token::TokenService + Sync + Send> =
            Arc::new(token::TokenServiceDefault::new(
                repositories.token_repo.clone(),
                oauth2_token_service.clone(),
            ));

        let login_service: Arc<dyn login::LoginService + Sync + Send> =
            Arc::new(login::LoginServiceDefault::new(
                oauth2_provider_client.clone(),
                account_service.clone(),
                account_grant_service.clone(),
                token_service.clone(),
                oauth2_token_service.clone(),
                config.accounts.clone(),
            ));

        let auth_service: Arc<dyn auth::AuthService + Sync + Send> = Arc::new(
            auth::AuthServiceDefault::new(token_service.clone(), account_grant_service.clone()),
        );

        let project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send> =
            Arc::new(project_policy::ProjectPolicyServiceDefault::new(
                repositories.project_policy_repo.clone(),
            ));

        let project_grant_service: Arc<dyn project_grant::ProjectGrantService + Sync + Send> =
            Arc::new(project_grant::ProjectGrantServiceDefault::new(
                repositories.project_repo.clone(),
                repositories.project_grant_repo.clone(),
                repositories.project_policy_repo.clone(),
            ));

        let object_store: Arc<dyn template_object_store::TemplateObjectStore + Sync + Send> =
            match config.templates.store.clone() {
                TemplateStoreConfig::S3(c) => {
                    Arc::new(template_object_store::AwsS3TemplateObjectStore::new(&c).await)
                }
                TemplateStoreConfig::Local(c) => {
                    Arc::new(template_object_store::FsTemplateObjectStore::new(&c)?)
                }
            };

        let project_auth_service: Arc<dyn project_auth::ProjectAuthorisationService + Sync + Send> =
            Arc::new(project_auth::ProjectAuthorisationServiceDefault::new(
                repositories.project_repo.clone(),
                project_grant_service.clone(),
                project_policy_service.clone(),
                repositories.template_repo.clone(),
            ));

        let project_service: Arc<dyn project::ProjectService + Sync + Send> =
            Arc::new(project::ProjectServiceDefault::new(
                repositories.project_repo.clone(),
                project_auth_service.clone(),
                plan_limit_service.clone(),
                repositories.template_repo.clone(),
            ));

        let template_service: Arc<dyn template::TemplateService + Sync + Send> =
            Arc::new(template::TemplateServiceDefault::new(
                repositories.account_uploads_repo.clone(),
                repositories.template_repo.clone(),
                plan_limit_service.clone(),
                object_store.clone(),
                project_service.clone(),
                project_auth_service.clone(),
            ));

        let routing_table_service: Arc<
            dyn golem_service_base::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(
            golem_service_base::routing_table::RoutingTableServiceDefault::new(
                config.routing_table.clone(),
            ),
        );

        let worker_executor_clients: Arc<
            dyn golem_service_base::worker_executor_clients::WorkerExecutorClients + Sync + Send,
        > = Arc::new(
            golem_service_base::worker_executor_clients::WorkerExecutorClientsDefault::new(
                config.worker_executor_client_cache.max_capacity,
                config.worker_executor_client_cache.time_to_idle,
            ),
        );

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> = {
            use golem_worker_service_base::service::template::TemplateService as BaseTemplateService;
            use golem_worker_service_base::service::worker::WorkerServiceDefault;

            let wrapped_template_service: Arc<
                dyn BaseTemplateService<AccountAuthorisation> + Send + Sync,
            > = Arc::new(worker::TemplateServiceWrapper::new(
                template_service.clone(),
            ));

            let base = WorkerServiceDefault::new(
                worker_executor_clients.clone(),
                wrapped_template_service,
                routing_table_service.clone(),
            );

            Arc::new(worker::WorkerServiceDefault::new(
                Arc::new(base),
                project_auth_service.clone(),
                plan_limit_service.clone(),
                repositories.account_connections_repo.clone(),
                repositories.account_workers_repo.clone(),
            ))
        };

        Ok(Services {
            auth_service,
            account_service,
            account_grant_service,
            account_summary_service,
            plan_service,
            plan_limit_service,
            oauth2_token_service,
            oauth2_session_service,
            oauth2_service,
            project_auth_service,
            project_policy_service,
            project_grant_service,
            project_service,
            token_service,
            login_service,
            template_service,
            worker_service,
        })
    }

    pub fn noop() -> Self {
        let plan_service: Arc<dyn plan::PlanService + Sync + Send> =
            Arc::new(plan::PlanServiceNoOp::default());

        let plan_limit_service: Arc<dyn plan_limit::PlanLimitService + Sync + Send> =
            Arc::new(plan_limit::PlanLimitServiceNoOp::default());

        let account_service: Arc<dyn account::AccountService + Sync + Send> =
            Arc::new(account::AccountServiceNoOp::default());

        let account_summary_service: Arc<dyn account_summary::AccountSummaryService + Sync + Send> =
            Arc::new(account_summary::AccountSummaryServiceNoOp::default());

        let account_grant_service: Arc<dyn account_grant::AccountGrantService + Sync + Send> =
            Arc::new(account_grant::AccountGrantServiceNoOp::default());

        let oauth2_token_service: Arc<dyn oauth2_token::OAuth2TokenService + Sync + Send> =
            Arc::new(oauth2_token::OAuth2TokenServiceNoOp::default());

        let oauth2_session_service: Arc<dyn oauth2_session::OAuth2SessionService + Sync + Send> =
            Arc::new(oauth2_session::OAuth2SessionServiceNoOp::default());

        let oauth2_service: Arc<dyn oauth2::OAuth2Service + Sync + Send> =
            Arc::new(oauth2::OAuth2ServiceNoOp::default());

        let token_service: Arc<dyn token::TokenService + Sync + Send> =
            Arc::new(token::TokenServiceNoOp::default());

        let login_service: Arc<dyn login::LoginService + Sync + Send> =
            Arc::new(login::LoginServiceNoOp::default());

        let auth_service: Arc<dyn auth::AuthService + Sync + Send> =
            Arc::new(auth::AuthServiceNoOp::default());

        let project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send> =
            Arc::new(project_policy::ProjectPolicyServiceNoOp::default());

        let project_grant_service: Arc<dyn project_grant::ProjectGrantService + Sync + Send> =
            Arc::new(project_grant::ProjectGrantServiceNoOp::default());

        let template_service: Arc<dyn template::TemplateService + Sync + Send> =
            Arc::new(template::TemplateServiceNoOp::default());

        let project_auth_service: Arc<dyn project_auth::ProjectAuthorisationService + Sync + Send> =
            Arc::new(project_auth::ProjectAuthorisationServiceNoOp::default());

        let project_service: Arc<dyn project::ProjectService + Sync + Send> =
            Arc::new(project::ProjectServiceNoOp::default());

        let worker_service: Arc<dyn worker::WorkerService + Sync + Send> =
            Arc::new(worker::WorkerServiceNoOp::default());

        Services {
            auth_service,
            account_service,
            account_grant_service,
            account_summary_service,
            plan_service,
            plan_limit_service,
            oauth2_token_service,
            oauth2_session_service,
            oauth2_service,
            project_auth_service,
            project_policy_service,
            project_grant_service,
            project_service,
            login_service,
            token_service,
            template_service,
            worker_service,
        }
    }
}
