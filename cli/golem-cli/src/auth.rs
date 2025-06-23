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

use crate::config::{
    AuthSecret, AuthenticationConfig, OAuth2AuthenticationConfig, OAuth2AuthenticationData,
};
use crate::config::{Config, ProfileName};
use crate::error::service::AnyhowMapServiceError;
use crate::log::LogColorize;
use crate::model::AccountId;
use anyhow::{anyhow, bail, Context};
use colored::Colorize;
use golem_client::api::{LoginClient, LoginClientLive, LoginOauth2WebFlowPollError};
use golem_client::model::{Token, TokenSecret, UnsafeToken, WebFlowAuthorizeUrlResponse};
use golem_client::Security;
use indoc::printdoc;
use std::path::Path;
use tracing::info;
use uuid::Uuid;

#[derive(Clone, PartialEq, Debug)]
pub struct Authentication(pub UnsafeToken);

impl Authentication {
    pub fn header(&self) -> String {
        token_header(&self.0.secret)
    }

    pub fn account_id(&self) -> AccountId {
        AccountId(self.0.data.account_id.clone())
    }
}

impl From<&OAuth2AuthenticationData> for Authentication {
    fn from(val: &OAuth2AuthenticationData) -> Self {
        Authentication(UnsafeToken {
            data: Token {
                id: val.id,
                account_id: val.account_id.to_string(),
                created_at: val.created_at,
                expires_at: val.expires_at,
            },
            secret: TokenSecret {
                value: val.secret.0,
            },
        })
    }
}

pub struct Auth {
    login_client: LoginClientLive,
}

impl Auth {
    pub fn new(login_client: LoginClientLive) -> Self {
        Self { login_client }
    }

    pub async fn authenticate(
        &self,
        token_override: Option<Uuid>,
        auth_config: &AuthenticationConfig,
        config_dir: &Path,
        profile_name: &ProfileName,
    ) -> anyhow::Result<Authentication> {
        if let Some(token_override) = token_override {
            let secret = TokenSecret {
                value: token_override,
            };
            let data = self.token_details(secret.clone()).await?;

            Ok(Authentication(UnsafeToken { data, secret }))
        } else {
            self.profile_authentication(auth_config, config_dir, profile_name)
                .await
        }
    }

    fn save_auth(
        &self,
        token: &UnsafeToken,
        profile_name: &ProfileName,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        let named_profile = Config::get_profile(config_dir, profile_name)?.ok_or(anyhow!(
            "Can't find profile {} in config",
            profile_name.0.log_color_highlight()
        ))?;

        let mut profile = named_profile.profile;

        profile.auth = AuthenticationConfig::OAuth2(OAuth2AuthenticationConfig {
            data: Some(unsafe_token_to_auth_data(token)),
        });
        Config::set_profile(profile_name.clone(), profile, config_dir)
            .with_context(|| "Failed to save auth token")?;

        Ok(())
    }

    async fn oauth2(
        &self,
        profile_name: &ProfileName,
        config_dir: &Path,
    ) -> anyhow::Result<Authentication> {
        let data = self.start_oauth2().await?;
        inform_user(&data);
        let token = self.complete_oauth2(data.state).await?;
        self.save_auth(&token, profile_name, config_dir)?;
        Ok(Authentication(token))
    }

    async fn profile_authentication(
        &self,
        auth_config: &AuthenticationConfig,
        config_dir: &Path,
        profile_name: &ProfileName,
    ) -> anyhow::Result<Authentication> {
        match auth_config {
            AuthenticationConfig::Static(inner) => {
                let secret: TokenSecret = inner.secret.into();
                let data = self.token_details(secret.clone()).await?;
                Ok(Authentication(UnsafeToken { data, secret }))
            }
            AuthenticationConfig::OAuth2(inner) => {
                if let Some(data) = &inner.data {
                    Ok(data.into())
                } else {
                    self.oauth2(profile_name, config_dir).await
                }
            }
        }
    }

    async fn token_details(&self, token_secret: TokenSecret) -> anyhow::Result<Token> {
        info!("Getting token info");
        let mut context = self.login_client.context.clone();
        context.security_token = Security::Bearer(token_secret.value.to_string());

        let client = LoginClientLive { context };

        client.current_login_token().await.map_service_error()
    }

    async fn start_oauth2(&self) -> anyhow::Result<WebFlowAuthorizeUrlResponse> {
        info!("Start OAuth2 workflow");
        self.login_client
            .oauth_2_web_flow_start("github", Some("https://golem.cloud"))
            .await
            .map_service_error()
    }

    async fn complete_oauth2(&self, state: String) -> anyhow::Result<UnsafeToken> {
        use tokio::time::{sleep, Duration};

        info!("Complete OAuth2 workflow");
        let mut attempts = 0;
        let max_attempts = 60;
        let delay = Duration::from_secs(1);

        loop {
            let status = self.login_client.oauth_2_web_flow_poll(&state).await;
            match status {
                Ok(token) => return Ok(token),
                Err(err) => match err {
                    golem_client::Error::Item(LoginOauth2WebFlowPollError::Error202(_)) => {
                        attempts += 1;
                        if attempts >= max_attempts {
                            bail!("OAuth2 workflow timeout")
                        }

                        sleep(delay).await;
                    }
                    _ => return Err(err).map_service_error(),
                },
            }
        }
    }
}

fn inform_user(data: &WebFlowAuthorizeUrlResponse) {
    let url = &data.url.underline();

    printdoc! {
        "
        ┌────────────────────────────────────────┐
        │       Authenticate with GitHub         │
        │                                        │
        │  Visit the following URL in a browser  │
        │                                        │
        └────────────────────────────────────────┘
        {url}
        ──────────────────────────────────────────
        "
    }

    println!("Waiting for authentication...");
}

fn token_header(secret: &TokenSecret) -> String {
    format!("bearer {}", secret.value)
}

fn unsafe_token_to_auth_data(value: &UnsafeToken) -> OAuth2AuthenticationData {
    OAuth2AuthenticationData {
        id: value.data.id,
        account_id: value.data.account_id.to_string(),
        created_at: value.data.created_at,
        expires_at: value.data.expires_at,
        secret: AuthSecret(value.secret.value),
    }
}
