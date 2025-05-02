use crate::config::CloudServiceConfig;
use crate::repo;
use crate::service::api_mapper::RemoteCloudApiMapper;
use cloud_common::clients::plugin::PluginServiceClientDefault;
use golem_common::config::DbConfig;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use std::sync::Arc;

pub mod account;
pub mod account_grant;
pub mod account_summary;
pub mod api_mapper;
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
pub mod project_grant;
pub mod project_policy;
pub mod token;

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
    pub project_service: Arc<dyn project::ProjectService + Sync + Send>,
    pub project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send>,
    pub project_grant_service: Arc<dyn project_grant::ProjectGrantService + Sync + Send>,
    pub api_mapper: Arc<RemoteCloudApiMapper>,
}

impl Services {
    pub async fn new(config: &CloudServiceConfig) -> Result<Services, String> {
        let repositories = match config.db.clone() {
            DbConfig::Postgres(config) => {
                let db_pool = PostgresPool::configured(&config)
                    .await
                    .map_err(|e| e.to_string())?;
                repo::Repositories::new(db_pool)
            }
            DbConfig::Sqlite(config) => {
                let db_pool = SqlitePool::configured(&config)
                    .await
                    .map_err(|e| e.to_string())?;
                repo::Repositories::new(db_pool)
            }
        };

        let oauth2_token_service: Arc<dyn oauth2_token::OAuth2TokenService + Sync + Send> =
            Arc::new(oauth2_token::OAuth2TokenServiceDefault::new(
                repositories.oauth2_token_repo.clone(),
                repositories.token_repo.clone(),
                repositories.account_repo.clone(),
            ));

        let token_service: Arc<dyn token::TokenService + Sync + Send> =
            Arc::new(token::TokenServiceDefault::new(
                repositories.token_repo.clone(),
                repositories.oauth2_web_flow_state_repo.clone(),
                repositories.account_repo.clone(),
                oauth2_token_service.clone(),
            ));

        let auth_service: Arc<dyn auth::AuthService + Sync + Send> =
            Arc::new(auth::AuthServiceDefault::new(
                token_service.clone(),
                repositories.account_repo.clone(),
                repositories.account_grant_repo.clone(),
                repositories.project_repo.clone(),
                repositories.project_policy_repo.clone(),
                repositories.project_grant_repo.clone(),
            ));

        let plan_service: Arc<dyn plan::PlanService + Sync + Send> = Arc::new(
            plan::PlanServiceDefault::new(repositories.plan_repo.clone(), config.plans.clone()),
        );

        let plan_limit_service: Arc<dyn plan_limit::PlanLimitService + Sync + Send> =
            Arc::new(plan_limit::PlanLimitServiceDefault::new(
                auth_service.clone(),
                repositories.plan_repo.clone(),
                repositories.account_repo.clone(),
                repositories.account_workers_repo.clone(),
                repositories.account_connections_repo.clone(),
                repositories.account_components_repo.clone(),
                repositories.account_used_storage_repo.clone(),
                repositories.account_uploads_repo.clone(),
                repositories.project_repo.clone(),
                repositories.account_fuel_repo.clone(),
            ));

        let account_service: Arc<dyn account::AccountService + Sync + Send> =
            Arc::new(account::AccountServiceDefault::new(
                auth_service.clone(),
                repositories.account_repo.clone(),
                plan_service.clone(),
            ));

        let account_summary_service: Arc<dyn account_summary::AccountSummaryService + Sync + Send> =
            Arc::new(account_summary::AccountSummaryServiceDefault::new(
                auth_service.clone(),
                repositories.account_summary_repo,
            ));

        let account_grant_service: Arc<dyn account_grant::AccountGrantService + Sync + Send> =
            Arc::new(account_grant::AccountGrantServiceDefault::new(
                auth_service.clone(),
                repositories.account_grant_repo.clone(),
                repositories.account_repo.clone(),
            ));

        let oauth2_token_service: Arc<dyn oauth2_token::OAuth2TokenService + Sync + Send> =
            Arc::new(oauth2_token::OAuth2TokenServiceDefault::new(
                repositories.oauth2_token_repo,
                repositories.token_repo.clone(),
                repositories.account_repo.clone(),
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

        let login_service: Arc<dyn login::LoginService + Sync + Send> =
            Arc::new(login::LoginServiceDefault::new(
                oauth2_provider_client.clone(),
                account_service.clone(),
                account_grant_service.clone(),
                token_service.clone(),
                oauth2_token_service.clone(),
                config.accounts.clone(),
            ));

        let project_policy_service: Arc<dyn project_policy::ProjectPolicyService + Sync + Send> =
            Arc::new(project_policy::ProjectPolicyServiceDefault::new(
                repositories.project_policy_repo.clone(),
            ));

        let project_grant_service: Arc<dyn project_grant::ProjectGrantService + Sync + Send> =
            Arc::new(project_grant::ProjectGrantServiceDefault::new(
                repositories.project_grant_repo.clone(),
                repositories.project_policy_repo.clone(),
                repositories.account_repo.clone(),
                auth_service.clone(),
            ));

        let plugin_service_client =
            Arc::new(PluginServiceClientDefault::new(&config.component_service));

        let project_service: Arc<dyn project::ProjectService + Sync + Send> =
            Arc::new(project::ProjectServiceDefault::new(
                auth_service.clone(),
                repositories.project_repo.clone(),
                plan_limit_service.clone(),
                plugin_service_client.clone(),
            ));

        let api_mapper = Arc::new(RemoteCloudApiMapper::new(plugin_service_client));

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
            project_policy_service,
            project_grant_service,
            project_service,
            token_service,
            login_service,
            api_mapper,
        })
    }
}
