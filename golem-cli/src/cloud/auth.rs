// Copyright 2024 Golem Cloud
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

use std::fmt::{Debug, Formatter};
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_cloud_client::model::{OAuth2Data, Token, TokenSecret, UnsafeToken};
use indoc::printdoc;
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::cloud::clients::login::LoginClient;
use crate::cloud::clients::CloudAuthentication;
use crate::config::{CloudProfile, Config, Profile, ProfileName};
use crate::model::GolemError;

#[async_trait]
pub trait Auth {
    async fn authenticate(
        &self,
        manual_token: Option<Uuid>,
        profile_name: &ProfileName,
        profile: &CloudProfile,
        config_dir: &Path,
    ) -> Result<CloudAuthentication, GolemError>;
}

pub struct AuthLive {
    pub login: Box<dyn LoginClient + Send + Sync>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAuthenticationConfig {
    data: CloudAuthenticationConfigData,
    secret: AuthSecret,
}

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

impl From<&UnsafeToken> for CloudAuthenticationConfig {
    fn from(value: &UnsafeToken) -> Self {
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
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthSecret(pub Uuid);

impl Debug for AuthSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AuthSecret").field(&"*******").finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAuthenticationConfigData {
    id: Uuid,
    account_id: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl AuthLive {
    fn save_auth_unsafe(
        &self,
        token: &UnsafeToken,
        profile_name: &ProfileName,
        config_dir: &Path,
    ) -> Result<(), GolemError> {
        let profile = Config::get_profile(profile_name, config_dir).ok_or(GolemError(format!(
            "Can't find profile {profile_name} in config"
        )))?;

        match profile {
            Profile::Golem(_) => Err(GolemError(format!(
                "Profile {profile_name} is an OOS profile. Cloud profile expected."
            ))),
            Profile::GolemCloud(mut profile) => {
                profile.auth = Some(token.into());
                Config::set_profile(
                    profile_name.clone(),
                    Profile::GolemCloud(profile),
                    config_dir,
                )?;

                Ok(())
            }
        }
    }

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
    ) -> Result<CloudAuthentication, GolemError> {
        let data = self.login.start_oauth2().await?;
        inform_user(&data);
        let token = self.login.complete_oauth2(data.encoded_session).await?;
        self.save_auth(&token, profile_name, config_dir);
        Ok(CloudAuthentication(token))
    }

    async fn profile_authentication(
        &self,
        profile_name: &ProfileName,
        profile: &CloudProfile,
        config_dir: &Path,
    ) -> Result<CloudAuthentication, GolemError> {
        if let Some(data) = &profile.auth {
            Ok(data.into())
        } else {
            self.oauth2(profile_name, config_dir).await
        }
    }
}

fn inform_user(data: &OAuth2Data) {
    let box_url_line = String::from_utf8(vec![b'-'; data.url.len() + 2]).unwrap();
    let box_code_line = String::from_utf8(vec![b'-'; data.user_code.len() + 2]).unwrap();
    let expires: DateTime<Utc> = data.expires;
    let expires_in = expires.signed_duration_since(Utc::now()).num_minutes();
    let expires_at = expires.format("%T");
    let url = &data.url;
    let user_code = &data.user_code;

    printdoc! {"
        >>
        >>  Application requests to perform OAuth2
        >>  authorization.
        >>
        >>  Visit following URL in a browser:
        >>
        >>   ┏{box_url_line}┓
        >>   ┃ {url} ┃
        >>   ┗{box_url_line}┛
        >>
        >>  And enter following code:
        >>
        >>   ┏{box_code_line}┓
        >>   ┃ {user_code} ┃
        >>   ┗{box_code_line}┛
        >>
        >>  Code will expire in {expires_in} minutes at {expires_at}.
        >>
        Waiting...
    "}
}

#[async_trait]
impl Auth for AuthLive {
    async fn authenticate(
        &self,
        manual_token: Option<Uuid>,
        profile_name: &ProfileName,
        profile: &CloudProfile,
        config_dir: &Path,
    ) -> Result<CloudAuthentication, GolemError> {
        if let Some(manual_token) = manual_token {
            let secret = TokenSecret {
                value: manual_token,
            };
            let data = self.login.token_details(secret.clone()).await?;

            Ok(CloudAuthentication(UnsafeToken { data, secret }))
        } else {
            self.profile_authentication(profile_name, profile, config_dir)
                .await
        }
    }
}
