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

mod oauth2;
mod oauth2_github_client;
mod oauth2_provider_client;
mod oauth2_session;
mod oauth2_token_repo;
mod oauth2_web_flow_state_repo;
mod service;

pub use self::oauth2::OAuth2Error;
use self::oauth2::OAuth2Service;
pub use self::oauth2_token_repo::{DbOAuth2TokenRepo, OAuth2TokenRepo};
pub use self::oauth2_web_flow_state_repo::{DbOAuth2FlowState, OAuth2WebFlowStateRepo};
pub use self::service::LoginError;
pub use self::service::LoginService;
use crate::config::LoginConfig;
use crate::service::account::AccountService;
use crate::service::token::TokenService;
use golem_service_base::db::Pool;
use std::sync::Arc;

pub struct LoginSystemEnabled {
    pub login_service: Arc<dyn LoginService>,
    pub oauth2_service: Arc<dyn OAuth2Service>,
}

pub enum LoginSystem {
    Enabled(LoginSystemEnabled),
    Disabled,
}

impl LoginSystem {
    pub fn new<DB>(
        config: &LoginConfig,
        account_service: &Arc<dyn AccountService>,
        token_service: &Arc<dyn TokenService>,
        db_pool: &DB,
    ) -> Self
    where
        DB: Pool + Clone + Send + Sync + 'static,
        DbOAuth2TokenRepo<DB>: OAuth2TokenRepo,
        DbOAuth2FlowState<DB>: OAuth2WebFlowStateRepo,
    {
        match config {
            LoginConfig::Disabled(_) => Self::Disabled,
            LoginConfig::OAuth2(oauth2_config) => {
                let oauth2_token_repo: Arc<dyn OAuth2TokenRepo> =
                    Arc::new(DbOAuth2TokenRepo::new(db_pool.clone()));

                let oauth2_web_flow_state_repo: Arc<dyn OAuth2WebFlowStateRepo> =
                    Arc::new(DbOAuth2FlowState::new(db_pool.clone()));

                let oauth2_session_service: Arc<dyn oauth2_session::OAuth2SessionService> =
                    Arc::new(
                        oauth2_session::OAuth2SessionServiceDefault::from_config(
                            &oauth2_config.ed_dsa,
                        )
                        .expect("Valid Public and Private Keys"),
                    );

                let oauth2_github_client: Arc<dyn oauth2_github_client::OAuth2GithubClient> =
                    Arc::new(oauth2_github_client::OAuth2GithubClientDefault {
                        config: oauth2_config.github.clone(),
                    });

                let oauth2_provider_client: Arc<dyn oauth2_provider_client::OAuth2ProviderClient> =
                    Arc::new(oauth2_provider_client::OAuth2ProviderClientDefault {});

                let oauth2_service: Arc<dyn oauth2::OAuth2Service> =
                    Arc::new(oauth2::OAuth2ServiceDefault::new(
                        oauth2_github_client,
                        oauth2_session_service.clone(),
                    ));

                let login_service: Arc<dyn service::LoginService> =
                    Arc::new(service::LoginServiceDefault::new(
                        oauth2_provider_client.clone(),
                        account_service.clone(),
                        token_service.clone(),
                        oauth2_token_repo,
                        oauth2_web_flow_state_repo,
                    ));

                Self::Enabled(LoginSystemEnabled {
                    login_service,
                    oauth2_service,
                })
            }
        }
    }
}
