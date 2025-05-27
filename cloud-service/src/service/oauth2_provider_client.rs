use crate::model::{ExternalLogin, OAuth2Provider};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use std::fmt::Debug;

#[async_trait]
pub trait OAuth2ProviderClient {
    async fn external_user_id(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2ProviderClientError>;
}

#[derive(Debug, thiserror::Error)]
pub enum OAuth2ProviderClientError {
    #[error("External error: {0}")]
    External(String),
    #[error("Internal error: {0}")]
    InternalClientError(#[from] reqwest::Error),
    #[error("Internal error: {0}")]
    InternalParseError(#[from] serde_json::Error),
}

impl OAuth2ProviderClientError {
    fn external(error: impl AsRef<str>) -> Self {
        Self::External(error.as_ref().to_string())
    }
}

impl SafeDisplay for OAuth2ProviderClientError {
    fn to_safe_string(&self) -> String {
        match self {
            OAuth2ProviderClientError::External(_) => self.to_string(),
            OAuth2ProviderClientError::InternalClientError(_) => self.to_string(),
            OAuth2ProviderClientError::InternalParseError(_) => self.to_string(),
        }
    }
}

pub struct OAuth2ProviderClientDefault {}

#[async_trait]
impl OAuth2ProviderClient for OAuth2ProviderClientDefault {
    async fn external_user_id(
        &self,
        provider: &OAuth2Provider,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2ProviderClientError> {
        match provider {
            OAuth2Provider::Github => github_user(access_token).await,
        }
    }
}

async fn github_user(access_token: &str) -> Result<ExternalLogin, OAuth2ProviderClientError> {
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

async fn github_user_details(
    access_token: &str,
) -> Result<GithubUserDetails, OAuth2ProviderClientError> {
    let client = reqwest::Client::new();

    let response = add_headers(client.get("https://api.github.com/user"), access_token)
        .send()
        .await?;

    let details = response_json::<GithubUserDetails>(response, "Github User Details").await?;

    Ok(details)
}

async fn github_user_email(
    access_token: &str,
) -> Result<Vec<GithubUserEmail>, OAuth2ProviderClientError> {
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

fn add_headers(request: reqwest::RequestBuilder, access_token: &str) -> reqwest::RequestBuilder {
    request
        .header("Accept", "application/json")
        .header("Authorization", format!("token {}", access_token))
        // see https://docs.github.com/en/rest/overview/resources-in-the-rest-api?apiVersion=2022-11-28#user-agent-required
        .header("User-Agent", "Golem Cloud")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

// Include body and status in error message.
async fn response_json<T>(
    response: reqwest::Response,
    prefix: &str,
) -> Result<T, OAuth2ProviderClientError>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        let body = response.text().await?;
        Err(OAuth2ProviderClientError::external(format!(
            "Request failed {prefix}: {status}, Body: {body}",
        )))
    } else {
        let full = response.bytes().await?;
        let json = serde_json::from_slice(&full)?;
        Ok(json)
    }
}

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct GithubUserDetails {
    login: String,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubUserEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[cfg(test)]
mod test {
    use test_r::test;

    use super::*;

    #[ignore]
    #[test]
    async fn manual_test() -> Result<(), OAuth2ProviderClientError> {
        let access_token = "ACCESS_TOKEN";
        let client = OAuth2ProviderClientDefault {};
        client
            .external_user_id(&OAuth2Provider::Github, access_token)
            .await?;
        Ok(())
    }
}
