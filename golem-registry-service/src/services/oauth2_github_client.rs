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

use crate::model::login::ExternalLogin;
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::login::OAuth2WebflowStateId;
use golem_common::{SafeDisplay, error_forwarding};
use std::fmt::{Debug, Display, Formatter};
use std::time::Duration;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2GithubClientError {
    #[error("Github device code expired, expires at: {expires_at}, current time: {current_time}")]
    Expired {
        expires_at: DateTime<Utc>,
        current_time: DateTime<Utc>,
    },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(OAuth2GithubClientError);

impl SafeDisplay for OAuth2GithubClientError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal Error".to_string(),
            Self::Expired { .. } => self.to_string(),
        }
    }
}

impl From<reqwest::Error> for OAuth2GithubClientError {
    fn from(value: reqwest::Error) -> Self {
        Self::InternalError(value.into())
    }
}

#[async_trait]
pub trait OAuth2GithubClient: Send + Sync {
    async fn initiate_device_workflow(&self)
    -> Result<DeviceWorkflowData, OAuth2GithubClientError>;

    async fn get_device_workflow_access_token(
        &self,
        device_code: &str,
        interval: Duration,
        expires: DateTime<Utc>,
    ) -> Result<String, OAuth2GithubClientError>;

    async fn get_authorize_url(&self, state: &OAuth2WebflowStateId) -> String;

    async fn exchange_code_for_token(
        &self,
        code: &str,
        state: &OAuth2WebflowStateId,
    ) -> Result<String, OAuth2GithubClientError>;

    async fn get_external_login(
        &self,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2GithubClientError>;
}

#[derive(Debug)]
pub struct DeviceWorkflowData {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: Duration,
    pub interval: Duration,
}

pub struct OAuth2GithubClientDefault {
    pub config: crate::config::GitHubOAuth2Config,
}

#[async_trait]
impl OAuth2GithubClient for OAuth2GithubClientDefault {
    async fn initiate_device_workflow(
        &self,
    ) -> Result<DeviceWorkflowData, OAuth2GithubClientError> {
        let client = reqwest::Client::new();

        let res = client
            .post("https://github.com/login/device/code")
            .query(&[
                ("client_id", &self.config.client_id),
                ("scope", &String::from("user:email")),
            ])
            .header("Accept", "application/json")
            .send()
            .await?;

        if res.status().is_success() {
            let response: DeviceCodeResponse = res.json().await?;

            Ok(DeviceWorkflowData {
                device_code: response.device_code,
                user_code: response.user_code,
                verification_uri: response.verification_uri,
                expires_in: Duration::from_secs(response.expires_in),
                interval: Duration::from_secs(response.interval),
            })
        } else {
            Err(anyhow!(
                "Failed to start devicde flow with status: {}",
                res.status()
            ))?
        }
    }

    async fn get_device_workflow_access_token(
        &self,
        device_code: &str,
        interval: Duration,
        expires: DateTime<Utc>,
    ) -> Result<String, OAuth2GithubClientError> {
        let client = reqwest::Client::new();
        let mut interval = interval;

        loop {
            let now = Utc::now();
            if now > expires {
                break Err(OAuth2GithubClientError::Expired {
                    expires_at: expires,
                    current_time: now,
                });
            }

            let response =
                execute_access_token_request(&self.config.client_id, &client, device_code).await?;

            match response {
                AccessTokenResponse::AccessToken(token) => {
                    break Ok(token.access_token);
                }
                AccessTokenResponse::ErrorResponse(error) => {
                    if error.error == ErrorResponseKind::AuthorizationPending {
                        // Do nothing.
                    } else if error.error == ErrorResponseKind::SlowDown {
                        let new_interval = error
                            .interval
                            .map(Duration::from_secs)
                            .unwrap_or(Duration::from_secs(5));
                        interval = new_interval;
                    } else {
                        Err(anyhow!(error)).context("Failed to retrieve access token")?
                    }
                }
            };

            tokio::time::sleep(interval).await;
        }
    }

    async fn get_authorize_url(&self, state: &OAuth2WebflowStateId) -> String {
        Url::parse_with_params(
            "https://github.com/login/oauth/authorize",
            &[
                ("client_id", self.config.client_id.as_str()),
                ("redirect_uri", self.config.redirect_uri.as_str()),
                ("state", &state.to_string()),
                ("scope", "user:email"),
            ],
        )
        .expect("Failed to construct GitHub authorize URL")
        .to_string()
    }

    async fn exchange_code_for_token(
        &self,
        code: &str,
        state: &OAuth2WebflowStateId,
    ) -> Result<String, OAuth2GithubClientError> {
        let client = reqwest::Client::new();

        let res = client
            .post("https://github.com/login/oauth/access_token")
            .query(&[
                ("client_id", &self.config.client_id),
                ("client_secret", &self.config.client_secret),
                ("code", &String::from(code)),
                ("state", &state.0.to_string()),
            ])
            .header("Accept", "application/json")
            .send()
            .await?;

        if res.status().is_success() {
            let access_token: AccessToken = res.json().await?;

            Ok(access_token.access_token)
        } else {
            Err(anyhow!(
                "Failed to to exchange code, status: {}",
                res.status()
            ))?
        }
    }

    async fn get_external_login(
        &self,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2GithubClientError> {
        let details = github_user_details(access_token).await?;

        let emails = github_user_email(access_token).await?;

        let verified_emails = emails
            .iter()
            .filter(|email| email.verified)
            .map(|email| email.email.clone())
            .collect::<Vec<_>>();

        let email = emails
            .iter()
            .find(|email| email.primary && email.verified)
            .or(emails.iter().find(|email| email.verified))
            .or(emails.first())
            .map(|email| email.email.clone());

        Ok(ExternalLogin {
            external_id: details.login,
            name: details.name,
            email,
            verified_emails,
        })
    }
}

async fn execute_access_token_request(
    client_id: &String,
    client: &reqwest::Client,
    device_code: &str,
) -> Result<AccessTokenResponse, OAuth2GithubClientError> {
    let response = client
        .post("https://github.com/login/oauth/access_token")
        .query(&[
            ("client_id", client_id),
            ("device_code", &String::from(device_code)),
            (
                "grant_type",
                &String::from("urn:ietf:params:oauth:grant-type:device_code"),
            ),
        ])
        .header("Accept", "application/json")
        .send()
        .await?;

    let body = response.text().await?;

    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(error_response) => Ok(AccessTokenResponse::ErrorResponse(error_response)),

        Err(_) => {
            let access_token_response =
                serde_json::from_str::<AccessToken>(&body).map_err(anyhow::Error::from)?;
            Ok(AccessTokenResponse::AccessToken(access_token_response))
        }
    }
}

#[derive(serde::Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

enum AccessTokenResponse {
    AccessToken(AccessToken),
    ErrorResponse(ErrorResponse),
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct AccessToken {
    access_token: String,
    token_type: TokenType,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

#[allow(dead_code)]
#[derive(serde::Deserialize, Debug)]
pub struct ErrorResponse {
    error: ErrorResponseKind,
    error_description: Option<String>,
    error_uri: Option<String>,
    interval: Option<u64>,
}

impl Display for ErrorResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error: {}", self.error)?;
        if let Some(descr) = &self.error_description {
            write!(f, ": {descr}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenType {
    /// Bearer token
    /// ([OAuth 2.0 Bearer Tokens - RFC 6750](https://tools.ietf.org/html/rfc6750)).
    Bearer,
    /// MAC ([OAuth 2.0 Message Authentication Code (MAC)
    /// Tokens](https://tools.ietf.org/html/draft-ietf-oauth-v2-http-mac-05)).
    Mac,
    /// An extension not defined by RFC 6749.
    Extension(String),
}
impl TokenType {
    fn from_str(s: &str) -> Self {
        match s {
            "bearer" => TokenType::Bearer,
            "mac" => TokenType::Mac,
            ext => TokenType::Extension(ext.to_string()),
        }
    }
}
impl AsRef<str> for TokenType {
    fn as_ref(&self) -> &str {
        match *self {
            TokenType::Bearer => "bearer",
            TokenType::Mac => "mac",
            TokenType::Extension(ref ext) => ext.as_str(),
        }
    }
}
impl<'de> serde::Deserialize<'de> for TokenType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        let variant_str = String::deserialize(deserializer)?;
        Ok(Self::from_str(&variant_str))
    }
}
impl serde::ser::Serialize for TokenType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(self.as_ref())
    }
}

/// Basic access token error types.
///
/// These error types are defined in
///
/// https://datatracker.ietf.org/doc/html/rfc6749#section-5.2
/// https://datatracker.ietf.org/doc/html/rfc8628#section-3.5
#[derive(Clone, PartialEq, Debug)]
pub enum ErrorResponseKind {
    InvalidRequest,
    InvalidClient,
    InvalidGrant,
    UnauthorizedClient,
    UnsupportedGrantType,
    InvalidScope,
    AuthorizationPending,
    SlowDown,
    AccessDenied,
    ExpiredToken,
    Other(String),
}

impl Display for ErrorResponseKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match *self {
            ErrorResponseKind::InvalidRequest => "Invalid Request",
            ErrorResponseKind::InvalidClient => "Invalid Client",
            ErrorResponseKind::InvalidGrant => "Invalid Grant",
            ErrorResponseKind::UnauthorizedClient => "Unauthorized Client",
            ErrorResponseKind::UnsupportedGrantType => "Unsupported Grant Type",
            ErrorResponseKind::InvalidScope => "Invalid Scope",
            ErrorResponseKind::AuthorizationPending => "Authorization Pending",
            ErrorResponseKind::SlowDown => "Slow Down",
            ErrorResponseKind::AccessDenied => "Access Denied",
            ErrorResponseKind::ExpiredToken => "Expired Token",
            ErrorResponseKind::Other(ref code) => code.as_str(),
        })
    }
}

impl ErrorResponseKind {
    fn from_str(s: &str) -> Self {
        match s {
            "invalid_request" => ErrorResponseKind::InvalidRequest,
            "invalid_client" => ErrorResponseKind::InvalidClient,
            "invalid_grant" => ErrorResponseKind::InvalidGrant,
            "unauthorized_client" => ErrorResponseKind::UnauthorizedClient,
            "unsupported_grant_type" => ErrorResponseKind::UnsupportedGrantType,
            "invalid_scope" => ErrorResponseKind::InvalidScope,
            "authorization_pending" => ErrorResponseKind::AuthorizationPending,
            "slow_down" => ErrorResponseKind::SlowDown,
            "access_denied" => ErrorResponseKind::AccessDenied,
            "expired_token" => ErrorResponseKind::ExpiredToken,
            code => ErrorResponseKind::Other(code.into()),
        }
    }
}

impl AsRef<str> for ErrorResponseKind {
    fn as_ref(&self) -> &str {
        match *self {
            ErrorResponseKind::InvalidRequest => "invalid_request",
            ErrorResponseKind::InvalidClient => "invalid_client",
            ErrorResponseKind::InvalidGrant => "invalid_grant",
            ErrorResponseKind::UnauthorizedClient => "unauthorized_client",
            ErrorResponseKind::UnsupportedGrantType => "unsupported_grant_type",
            ErrorResponseKind::InvalidScope => "invalid_scope",
            ErrorResponseKind::AuthorizationPending => "authorization_pending",
            ErrorResponseKind::SlowDown => "slow_down",
            ErrorResponseKind::AccessDenied => "access_denied",
            ErrorResponseKind::ExpiredToken => "expired_token",
            ErrorResponseKind::Other(ref code) => code.as_str(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ErrorResponseKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        let variant_str = String::deserialize(deserializer)?;
        Ok(Self::from_str(&variant_str))
    }
}

impl serde::ser::Serialize for ErrorResponseKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(self.as_ref())
    }
}

#[derive(Debug, serde::Deserialize)]
struct GithubUserDetails {
    login: String,
    name: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubUserEmail {
    email: String,
    primary: bool,
    verified: bool,
}

fn add_headers(request: reqwest::RequestBuilder, access_token: &str) -> reqwest::RequestBuilder {
    request
        .header("Accept", "application/json")
        .header("Authorization", format!("token {access_token}"))
        // see https://docs.github.com/en/rest/overview/resources-in-the-rest-api?apiVersion=2022-11-28#user-agent-required
        .header("User-Agent", "Golem Cloud")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

async fn github_user_details(
    access_token: &str,
) -> Result<GithubUserDetails, OAuth2GithubClientError> {
    let client = reqwest::Client::new();

    let response = add_headers(client.get("https://api.github.com/user"), access_token)
        .send()
        .await?;

    let details = response_json::<GithubUserDetails>(response, "Github User Details").await?;

    Ok(details)
}

async fn github_user_email(
    access_token: &str,
) -> Result<Vec<GithubUserEmail>, OAuth2GithubClientError> {
    let client = reqwest::Client::new();

    let response = add_headers(
        client.get("https://api.github.com/user/emails"),
        access_token,
    )
    .send()
    .await?;

    let emails = response_json::<Vec<GithubUserEmail>>(response, "Github User Emails").await?;

    Ok(emails)
}

async fn response_json<T>(
    response: reqwest::Response,
    prefix: &str,
) -> Result<T, OAuth2GithubClientError>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        let body = response.text().await?;
        Err(anyhow!("Request failed {prefix}: {status}, Body: {body}").into())
    } else {
        let full = response.bytes().await?;
        let json = serde_json::from_slice(&full).map_err(anyhow::Error::from)?;
        Ok(json)
    }
}
