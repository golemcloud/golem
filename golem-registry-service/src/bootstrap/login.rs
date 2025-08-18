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

use crate::config::LoginConfig;
use crate::services::account::AccountService;
use crate::services::token::TokenService;
use golem_service_base::db::Pool;
use std::sync::Arc;
use crate::repo::oauth2_token::{DbOAuth2TokenRepo, OAuth2TokenRepo};
use crate::repo::oauth2_webflow_state::{DbOAuth2WebflowStateRepo, OAuth2WebflowStateRepo};
use crate::services::oauth2_github_client::{OAuth2GithubClient, OAuth2GithubClientDefault};
use crate::services::oauth2::OAuth2Service;

#[derive(Clone)]
pub struct LoginSystemEnabled {
    pub oauth2_service: Arc<OAuth2Service>,
}

#[derive(Clone)]
pub enum LoginSystem {
    Enabled(LoginSystemEnabled),
    Disabled,
}

impl LoginSystem {
    pub fn new(
        config: &LoginConfig,
        account_service: Arc<AccountService>,
        token_service: Arc<TokenService>,
        oauth2_token_repo: Arc<dyn OAuth2TokenRepo>,
        oauth2_webflow_state_repo: Arc<dyn OAuth2WebflowStateRepo>
    ) -> anyhow::Result<Self>
    where
    {
        match config {
            LoginConfig::Disabled(_) => Ok(Self::Disabled),
            LoginConfig::OAuth2(oauth2_config) => {

                let oauth2_github_client: Arc<dyn OAuth2GithubClient> =
                    Arc::new(OAuth2GithubClientDefault {
                        config: oauth2_config.github.clone(),
                    });

                let oauth2_service: Arc<OAuth2Service> =
                    Arc::new(OAuth2Service::new(
                        oauth2_github_client,
                        account_service,
                        token_service,
                        oauth2_token_repo,
                        oauth2_webflow_state_repo,
                        &oauth2_config.ed_dsa
                    )?);

                Ok(Self::Enabled(LoginSystemEnabled {
                    oauth2_service,
                }))
            }
        }
    }
}
