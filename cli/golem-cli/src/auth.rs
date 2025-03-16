// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::cloud::{
    AccountId, AuthSecret, CloudAuthenticationConfig, CloudAuthenticationConfigData,
};
use crate::config::{Config, Profile, ProfileName};
use crate::error::service::AnyhowMapServiceError;
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_cloud_client::api::{LoginClient, LoginClientLive, LoginOauth2WebFlowPollError};
use golem_cloud_client::model::{Token, TokenSecret, UnsafeToken, WebFlowAuthorizeUrlResponse};
use golem_cloud_client::Security;
use golem_wasm_rpc_stubgen::log::LogColorize;
use indoc::printdoc;
use std::path::Path;
use tracing::{info, warn};
use uuid::Uuid;

impl From<&CloudAuthenticationConfig> for CloudAuthentication {
    fn from(val: &CloudAuthenticationConfig) -> Self {
        CloudAuthentication(UnsafeToken {
            data: Token {
                id: val.data.id,
                account_id: val.data.account_id.to_string(),
                created_at: val.data.created_at,
                expires_at: val.data.expires_at,
            },
            secret: TokenSecret {
                value: val.secret.0,
            },
        })
    }
}

pub fn unsafe_token_to_auth_config(value: &UnsafeToken) -> CloudAuthenticationConfig {
    CloudAuthenticationConfig {
        data: CloudAuthenticationConfigData {
            id: value.data.id,
            account_id: value.data.account_id.to_string(),
            created_at: value.data.created_at,
            expires_at: value.data.expires_at,
        },
        secret: AuthSecret(value.secret.value),
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
        profile_name: &ProfileName,
        auth_config: Option<&CloudAuthenticationConfig>,
        config_dir: &Path,
    ) -> anyhow::Result<CloudAuthentication> {
        if let Some(token_override) = token_override {
            let secret = TokenSecret {
                value: token_override,
            };
            let data = self.token_details(secret.clone()).await?;

            Ok(CloudAuthentication(UnsafeToken { data, secret }))
        } else {
            self.profile_authentication(profile_name, auth_config, config_dir)
                .await
        }
    }

    fn save_auth_unsafe(
        &self,
        token: &UnsafeToken,
        profile_name: &ProfileName,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        let profile = Config::get_profile(profile_name, config_dir)?.ok_or(anyhow!(
            "Can't find profile {} in config",
            profile_name.0.log_color_highlight()
        ))?;

        match profile {
            Profile::Golem(_) => Err(anyhow!(
                "Profile {} is an OSS profile. Cloud profile expected.",
                profile_name.0.log_color_highlight()
            )),
            Profile::GolemCloud(mut profile) => {
                profile.auth = Some(unsafe_token_to_auth_config(token));
                Config::set_profile(
                    profile_name.clone(),
                    Profile::GolemCloud(profile),
                    config_dir,
                )?;

                Ok(())
            }
        }
    }

    // TODO: do we need a safe one?
    fn save_auth(&self, token: &UnsafeToken, profile_name: &ProfileName, config_dir: &Path) {
        match self.save_auth_unsafe(token, profile_name, config_dir) {
            Ok(_) => {}
            Err(err) => {
                warn!("Failed to save auth data: {err}")
            }
        }
    }

    async fn oauth2(
        &self,
        profile_name: &ProfileName,
        config_dir: &Path,
    ) -> anyhow::Result<CloudAuthentication> {
        let data = self.start_oauth2().await?;
        inform_user(&data);
        let token = self.complete_oauth2(data.state).await?;
        self.save_auth(&token, profile_name, config_dir);
        Ok(CloudAuthentication(token))
    }

    async fn profile_authentication(
        &self,
        profile_name: &ProfileName,
        auth_config: Option<&CloudAuthenticationConfig>,
        config_dir: &Path,
    ) -> anyhow::Result<CloudAuthentication> {
        if let Some(data) = auth_config {
            Ok(data.into())
        } else {
            self.oauth2(profile_name, config_dir).await
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
                    golem_cloud_client::Error::Item(LoginOauth2WebFlowPollError::Error202(_)) => {
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

#[derive(Clone, PartialEq, Debug)]
pub struct CloudAuthentication(pub UnsafeToken);

impl CloudAuthentication {
    pub fn header(&self) -> String {
        token_header(&self.0.secret)
    }

    pub fn account_id(&self) -> AccountId {
        AccountId(self.0.data.account_id.clone())
    }
}

pub fn token_header(secret: &TokenSecret) -> String {
    format!("bearer {}", secret.value)
}
