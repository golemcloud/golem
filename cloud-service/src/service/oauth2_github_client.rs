use std::fmt::{Debug, Display, Formatter};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::SafeDisplay;
use http::StatusCode;
use url::Url;

#[async_trait]
pub trait OAuth2GithubClient {
    async fn initiate_device_workflow(&self)
        -> Result<DeviceWorkflowData, OAuth2GithubClientError>;
    async fn get_access_token(
        &self,
        device_code: &str,
        interval: Duration,
        expires: DateTime<Utc>,
    ) -> Result<String, OAuth2GithubClientError>;

    async fn get_authorize_url(&self, state: &str) -> String;

    async fn exchange_code_for_token(
        &self,
        code: &str,
        state: &str,
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

#[derive(Debug, thiserror::Error)]
pub enum OAuth2GithubClientError {
    #[error("Failed to parse access token: {0}")]
    FailedToParseAccessToken(reqwest::Error),
    #[error("Failed to read GitHub response body: {0}")]
    FailedToReadResponseBody(reqwest::Error),
    #[error("Failed to parse GitHub response body: {0}")]
    FailedToParseResponseBody(serde_json::Error),
    #[error("Failed to retrieve GitHub device code: {0}")]
    FailedToRetrieveDeviceCode(reqwest::Error),
    #[error("Failed to retrieve GitHub device code: {0}")]
    FailedToRetrieveDeviceCodeNonOk(StatusCode),
    #[error("Failed to retrieve GitHub access token: {0}")]
    FailedToRetrieveAccessToken(reqwest::Error),
    #[error("Failed to retrieve GitHub access token: {0}")]
    ErrorResponseToRetrieveAccessToken(ErrorResponse),
    #[error("Failed to exchange code for token: {0}")]
    FailedToExchangeCode(reqwest::Error),
    #[error("Failed to exchange code for token: {0}")]
    FailedToExchangeCodeNonOk(StatusCode),
    #[error("Github device code expired, expires at: {expires_at}, current time: {current_time}")]
    Expired {
        expires_at: DateTime<Utc>,
        current_time: DateTime<Utc>,
    },
}

impl SafeDisplay for OAuth2GithubClientError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
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
            .map_err(OAuth2GithubClientError::FailedToRetrieveDeviceCode)?;

        if res.status().is_success() {
            let response: DeviceCodeResponse = res
                .json()
                .await
                .map_err(OAuth2GithubClientError::FailedToReadResponseBody)?;

            Ok(DeviceWorkflowData {
                device_code: response.device_code,
                user_code: response.user_code,
                verification_uri: response.verification_uri,
                expires_in: Duration::from_secs(response.expires_in),
                interval: Duration::from_secs(response.interval),
            })
        } else {
            Err(OAuth2GithubClientError::FailedToRetrieveDeviceCodeNonOk(
                res.status(),
            ))
        }
    }

    async fn get_access_token(
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
                            .map(Duration::from_secs)
                            .unwrap_or(Duration::from_secs(5));
                        interval = new_interval;
                    } else {
                        break Err(OAuth2GithubClientError::ErrorResponseToRetrieveAccessToken(
                            error,
                        ));
                    }
                }
            };

            tokio::time::sleep(interval).await;
        }
    }

    async fn get_authorize_url(&self, state: &str) -> String {
        Url::parse_with_params(
            "https://github.com/login/oauth/authorize",
            &[
                ("client_id", self.config.github_client_id.as_str()),
                ("redirect_uri", self.config.github_redirect_uri.as_str()),
                ("state", state),
                ("scope", "user:email"),
            ],
        )
        .expect("Failed to construct GitHub authorize URL")
        .to_string()
    }

    async fn exchange_code_for_token(
        &self,
        code: &str,
        state: &str,
    ) -> Result<String, OAuth2GithubClientError> {
        let client = reqwest::Client::new();

        let res = client
            .post("https://github.com/login/oauth/access_token")
            .query(&[
                ("client_id", &self.config.github_client_id),
                ("client_secret", &self.config.github_client_secret),
                ("code", &String::from(code)),
                ("state", &String::from(state)),
            ])
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(OAuth2GithubClientError::FailedToExchangeCode)?;

        if res.status().is_success() {
            let access_token: AccessToken = res
                .json()
                .await
                .map_err(OAuth2GithubClientError::FailedToParseAccessToken)?;
            Ok(access_token.access_token)
        } else {
            Err(OAuth2GithubClientError::FailedToExchangeCodeNonOk(
                res.status(),
            ))
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
        .map_err(OAuth2GithubClientError::FailedToRetrieveAccessToken)?;

    let body = response
        .text()
        .await
        .map_err(OAuth2GithubClientError::FailedToReadResponseBody)?;

    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(error_response) => Ok(AccessTokenResponse::ErrorResponse(error_response)),

        Err(_) => {
            let access_token_response = serde_json::from_str::<AccessToken>(&body)
                .map_err(OAuth2GithubClientError::FailedToParseResponseBody)?;
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
            write!(f, ": {}", descr)?;
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

// Manual integration test.
#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    const CLIENT_ID: &str = "1031b4cbcc32449a9151";

    #[ignore]
    #[test]
    async fn test_device_flow() {
        let client = OAuth2GithubClientDefault {
            config: crate::config::OAuth2Config {
                github_client_id: CLIENT_ID.into(),
                github_client_secret: "".into(),
                github_redirect_uri: Url::parse(
                    "http://localhost:8085/v1/login/oauth2/web/callback/github",
                )
                .unwrap(),
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
