// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::login::OAuth2WebflowStateId;
use golem_common::{SafeDisplay, error_forwarding};
use std::fmt::Debug;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2GithubClientError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(OAuth2GithubClientError);

impl SafeDisplay for OAuth2GithubClientError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal Error".to_string(),
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

pub struct OAuth2GithubClientDefault {
    pub config: crate::config::GitHubOAuth2Config,
}

#[async_trait]
impl OAuth2GithubClient for OAuth2GithubClientDefault {
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
            external_id: details.id.to_string(),
            name: details.name.or(Some(details.login)),
            email,
            verified_emails,
        })
    }
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
struct AccessToken {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubUserDetails {
    id: u64,
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
