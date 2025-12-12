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

use crate::config::Config;
use crate::config::{
    ApplicationEnvironmentConfig, AuthenticationConfig, AuthenticationConfigWithSource,
    AuthenticationSource, OAuth2AuthenticationConfig, OAuth2AuthenticationData,
};
use crate::error::service::AnyhowMapServiceError;
use crate::log::LogColorize;
use anyhow::{anyhow, bail, Context};
use colored::Colorize;
use golem_client::api::{LoginClient, LoginClientLive, LoginPollOauth2WebflowError};
use golem_client::model::{OAuth2Provider, OAuth2WebflowData, Token, TokenWithSecret};
use golem_client::Security;
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::login::OAuth2WebflowStateId;
use indoc::printdoc;
use std::path::Path;
use tracing::info;

#[derive(Clone, PartialEq, Debug)]
pub struct Authentication(pub TokenWithSecret);

impl Authentication {
    pub fn from_token_and_secret(token: Token, secret: TokenSecret) -> Self {
        Self(TokenWithSecret {
            id: token.id,
            secret,
            account_id: token.account_id,
            created_at: token.created_at,
            expires_at: token.expires_at,
        })
    }

    pub fn from_oauth2_config(auth: OAuth2AuthenticationData) -> Self {
        Self(TokenWithSecret {
            id: auth.id.into(),
            secret: TokenSecret::trusted(auth.secret.0.clone()),
            account_id: auth.account_id.into(),
            created_at: auth.created_at,
            expires_at: auth.expires_at,
        })
    }

    pub fn header(&self) -> String {
        format!("bearer {}", self.0.secret.secret())
    }

    pub fn account_id(&self) -> &AccountId {
        &self.0.account_id
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
        token_override: Option<String>,
        auth_config: &AuthenticationConfigWithSource,
        config_dir: &Path,
    ) -> anyhow::Result<Authentication> {
        match token_override {
            Some(secret) => {
                let secret = TokenSecret::trusted(secret);
                Ok(Authentication::from_token_and_secret(
                    self.token_details(&secret).await?,
                    secret,
                ))
            }
            None => match &auth_config.authentication {
                AuthenticationConfig::Static(inner) => {
                    let secret = TokenSecret::trusted(inner.secret.0.clone());
                    Ok(Authentication::from_token_and_secret(
                        self.token_details(&secret).await?,
                        secret,
                    ))
                }
                AuthenticationConfig::OAuth2(inner) => {
                    if let Some(data) = &inner.data {
                        Ok(Authentication::from_oauth2_config(data.clone()))
                    } else {
                        self.oauth2(&auth_config.source, config_dir).await
                    }
                }
            },
        }
    }

    fn save_auth(
        &self,
        token: TokenWithSecret,
        auth_source: &AuthenticationSource,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        match auth_source {
            AuthenticationSource::Profile(profile_name) => {
                let named_profile =
                    Config::get_profile(config_dir, profile_name)?.ok_or(anyhow!(
                        "Can't find profile {} in config",
                        profile_name.0.log_color_highlight()
                    ))?;
                let mut profile = named_profile.profile;
                profile.auth = AuthenticationConfig::from_token_with_secret(token);
                Config::set_profile(profile_name.clone(), profile, config_dir).with_context(
                    || format!("Failed to save auth token for profile {profile_name}"),
                )?;
            }
            AuthenticationSource::ApplicationEnvironment(app_env_id) => {
                let mut config = Config::get_application_environment(config_dir, app_env_id)?
                    .unwrap_or(ApplicationEnvironmentConfig {
                        auth: OAuth2AuthenticationConfig { data: None },
                    });
                config.auth = OAuth2AuthenticationConfig::from_token_with_secret(token);
                Config::set_application_environment(app_env_id, config, config_dir).with_context(
                    || {
                        format!(
                            "Failed to save auth token for application environment: {app_env_id}"
                        )
                    },
                )?;
            }
        }

        Ok(())
    }

    async fn oauth2(
        &self,
        auth_source: &AuthenticationSource,
        config_dir: &Path,
    ) -> anyhow::Result<Authentication> {
        let data = self.start_oauth2().await?;
        inform_user(&data);
        let token = self.complete_oauth2(data.state).await?;
        self.save_auth(token.clone(), auth_source, config_dir)?;
        Ok(Authentication(token))
    }

    async fn token_details(&self, token_secret: &TokenSecret) -> anyhow::Result<Token> {
        info!("Getting token info");
        let mut context = self.login_client.context.clone();
        context.security_token = Security::Bearer(token_secret.secret().to_string());

        let client = LoginClientLive { context };

        client.current_login_token().await.map_service_error()
    }

    async fn start_oauth2(&self) -> anyhow::Result<OAuth2WebflowData> {
        info!("Start OAuth2 workflow");
        self.login_client
            .start_oauth_2_webflow(&OAuth2Provider::Github, Some("https://golem.cloud"))
            .await
            .map_service_error()
    }

    async fn complete_oauth2(
        &self,
        state: OAuth2WebflowStateId,
    ) -> anyhow::Result<TokenWithSecret> {
        use tokio::time::{sleep, Duration};

        info!("Complete OAuth2 workflow");
        let mut attempts = 0;
        let max_attempts = 60;
        let delay = Duration::from_secs(1);

        loop {
            let status = self.login_client.poll_oauth_2_webflow(&state.0).await;
            match status {
                Ok(token) => return Ok(token),
                Err(err) => match err {
                    golem_client::Error::Item(LoginPollOauth2WebflowError::Error202(_)) => {
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

fn inform_user(data: &OAuth2WebflowData) {
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
