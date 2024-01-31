use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait OAuth2GithubClient {
    async fn initiate_device_workflow(&self)
        -> Result<DeviceWorkflowData, OAuth2GithubClientError>;
    async fn get_access_token(
        &self,
        device_code: &str,
        interval: std::time::Duration,
        expires: DateTime<Utc>,
    ) -> Result<String, OAuth2GithubClientError>;
}

#[derive(Debug)]
pub struct DeviceWorkflowData {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: Duration,
    pub interval: Duration,
}

#[derive(Debug)]
pub enum OAuth2GithubClientError {
    Unexpected(String),
}

pub struct OAuth2GithubClientDefault {
    pub config: crate::config::OAuth2Config,
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
                ("client_id", &self.config.github_client_id),
                ("scope", &String::from("user:email")),
            ])
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                OAuth2GithubClientError::Unexpected(format!(
                    "Failed to retrieve device code with status: {}",
                    e
                ))
            })?;

        if res.status().is_success() {
            let response: DeviceCodeResponse = res.json().await.map_err(|e| {
                OAuth2GithubClientError::Unexpected(format!("Failed to read response body: {}", e))
            })?;

            Ok(DeviceWorkflowData {
                device_code: response.device_code,
                user_code: response.user_code,
                verification_uri: response.verification_uri,
                expires_in: Duration::from_secs(response.expires_in),
                interval: Duration::from_secs(response.interval),
            })
        } else {
            Err(OAuth2GithubClientError::Unexpected(format!(
                "Failed to retrieve device code with status: {}",
                res.status()
            )))
        }
    }

    async fn get_access_token(
        &self,
        device_code: &str,
        interval: std::time::Duration,
        expires: DateTime<Utc>,
    ) -> Result<String, OAuth2GithubClientError> {
        let client = reqwest::Client::new();
        let mut interval = interval;

        loop {
            let now = chrono::Utc::now();
            if now > expires {
                break Err(OAuth2GithubClientError::Unexpected(
                    "Device code expired".into(),
                ));
            }

            let response =
                execute_access_token_request(&self.config.github_client_id, &client, device_code)
                    .await?;

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
                            .map(std::time::Duration::from_secs)
                            .unwrap_or(std::time::Duration::from_secs(5));
                        interval = new_interval;
                    } else {
                        break Err(OAuth2GithubClientError::Unexpected(format!(
                            "Failed to retrieve access token: {:?}",
                            error
                        )));
                    }
                }
            };

            tokio::time::sleep(interval).await;
        }
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
        .await
        .map_err(|e| {
            OAuth2GithubClientError::Unexpected(format!(
                "Github Access Token Request Failed: {}",
                e
            ))
        })?;

    let body = response.text().await.map_err(|e| {
        OAuth2GithubClientError::Unexpected(format!("Failed to extract response body: {}", e))
    })?;

    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(error_response) => Ok(AccessTokenResponse::ErrorResponse(error_response)),

        Err(_) => {
            let access_token_response =
                serde_json::from_str::<AccessToken>(&body).map_err(|e| {
                    OAuth2GithubClientError::Unexpected(format!(
                        "Failed to parse access token response: {}",
                        e
                    ))
                })?;
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
struct ErrorResponse {
    error: ErrorResponseKind,
    error_description: Option<String>,
    error_uri: Option<String>,
    interval: Option<u64>,
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

// Manual integration test.
#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT_ID: &str = "1031b4cbcc32449a9151";

    #[ignore]
    #[tokio::test]
    async fn test_device_flow() {
        let client = OAuth2GithubClientDefault {
            config: crate::config::OAuth2Config {
                github_client_id: CLIENT_ID.into(),
            },
        };

        let device = client
            .initiate_device_workflow()
            .await
            .expect("Failed to initiate workflow");

        println!("Device: {:?}", device);

        let access_token = client
            .get_access_token(
                &device.device_code,
                device.interval,
                Utc::now() + device.expires_in,
            )
            .await
            .expect("Failed to get access token");

        println!("Access Token: {}", access_token)
    }
}
