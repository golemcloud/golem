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

use crate::invocation_session::SessionTransportError;
use crate::{Error, api};
use std::time::Duration;

pub use crate::invocation_session::ReconnectPolicy;

/// The reconnect window of a configuration that does not set one.
pub const DEFAULT_RECONNECT_WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub enum ClientError {
    InvocationFailed {
        message: String,
    },
    /// A streaming call could not reach the server within its reconnect window.
    Unreachable {
        endpoint: String,
        window: Duration,
        last_cause: String,
    },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::InvocationFailed { message } => write!(f, "{}", message),
            ClientError::Unreachable {
                endpoint,
                window,
                last_cause,
            } => write!(f, "{endpoint} was unreachable for {window:?}: {last_cause}"),
        }
    }
}

impl From<SessionTransportError> for ClientError {
    fn from(error: SessionTransportError) -> Self {
        match error {
            SessionTransportError::Unreachable {
                endpoint,
                window,
                last_cause,
            } => ClientError::Unreachable {
                endpoint,
                window,
                last_cause: last_cause.to_string(),
            },
            error => ClientError::InvocationFailed {
                message: error.to_string(),
            },
        }
    }
}

impl std::error::Error for ClientError {}

impl From<Error<api::AgentError>> for ClientError {
    fn from(err: Error<api::AgentError>) -> Self {
        ClientError::InvocationFailed {
            message: err.to_string(),
        }
    }
}

pub enum GolemServer {
    Local,
    Cloud { token: String },
    Custom { url: reqwest::Url, token: String },
}

impl GolemServer {
    pub fn url(&self) -> reqwest::Url {
        match self {
            GolemServer::Local => reqwest::Url::parse("http://localhost:9881").unwrap(),
            GolemServer::Cloud { .. } => {
                reqwest::Url::parse("https://release.api.golem.cloud").unwrap()
            }
            GolemServer::Custom { url, .. } => url.clone(),
        }
    }

    pub fn token(&self) -> crate::Security {
        match self {
            GolemServer::Local => {
                crate::Security::Bearer(crate::LOCAL_WELL_KNOWN_TOKEN.to_string())
            }
            GolemServer::Cloud { token } => crate::Security::Bearer(token.clone()),
            GolemServer::Custom { token, .. } => crate::Security::Bearer(token.clone()),
        }
    }
}

pub struct Configuration {
    pub app_name: String,
    pub env_name: String,
    pub server: GolemServer,
    /// How long a streaming call keeps reconnecting without progress. Progress is a connection
    /// that stays up for 5 seconds after the server accepted its start or a resume.
    pub reconnect: ReconnectPolicy,
}

/// Why the ID of an environment could not be resolved.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EnvironmentLookupError {
    /// The server could not be reached, or it answered with a temporary failure.
    #[error("the environment could not be resolved: {0}")]
    Unavailable(String),
    /// The server answered, and the environment does not exist in the account of the token or
    /// the lookup was refused.
    #[error("{0}")]
    Failed(String),
}

type EnvironmentCacheKey = (String, String, String, String);

static ENVIRONMENT_IDS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<EnvironmentCacheKey, uuid::Uuid>>,
> = std::sync::OnceLock::new();

/// Resolves the ID of the environment `env_name` of the application `app_name` in the account of
/// the token. Environments that other accounts share with that account can have the same names,
/// so only the account's own environment matches. A successful resolution is cached for the
/// server, token and names.
pub async fn environment_id(
    base_url: &reqwest::Url,
    security: &crate::Security,
    app_name: &str,
    env_name: &str,
) -> Result<uuid::Uuid, EnvironmentLookupError> {
    let token = match security {
        crate::Security::Bearer(token) => token.clone(),
        crate::Security::Empty => String::new(),
    };
    let key = (
        base_url.to_string(),
        token,
        app_name.to_string(),
        env_name.to_string(),
    );
    let cache = ENVIRONMENT_IDS.get_or_init(Default::default);
    if let Some(id) = cache.lock().unwrap().get(&key) {
        return Ok(*id);
    }
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| EnvironmentLookupError::Failed(error.to_string()))?;
    let token: crate::model::Token =
        environment_lookup_get(&client, base_url, security, &["v1", "me", "token"], &[]).await?;
    let environments: crate::model::PageEnvironmentWithDetails = environment_lookup_get(
        &client,
        base_url,
        security,
        &["v1", "me", "visible-environments"],
        &[("app_name", app_name), ("env_name", env_name)],
    )
    .await?;
    let id = environments
        .values
        .iter()
        .find(|candidate| {
            candidate.account.id == token.account_id
                && candidate.application.name.0 == app_name
                && candidate.environment.name.0 == env_name
        })
        .map(|candidate| candidate.environment.id.0)
        .ok_or_else(|| {
            EnvironmentLookupError::Failed(format!(
                "Environment '{env_name}' of application '{app_name}' does not exist in the account of the token"
            ))
        })?;
    cache.lock().unwrap().insert(key, id);
    Ok(id)
}

/// Sends one GET of the environment lookup. The status decides whether a failure is temporary,
/// before the body is read: a server or proxy that answers 429 or 5xx is unavailable whatever its
/// body holds.
async fn environment_lookup_get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    security: &crate::Security,
    path: &[&str],
    query: &[(&str, &str)],
) -> Result<T, EnvironmentLookupError> {
    let mut url = base_url.clone();
    url.path_segments_mut()
        .map_err(|_| EnvironmentLookupError::Failed(format!("invalid server URL {base_url}")))?
        .pop_if_empty()
        .extend(path);
    let mut request = client.get(url.clone()).query(query);
    if let crate::Security::Bearer(token) = security {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| {
        EnvironmentLookupError::Unavailable(format!("request to {url} failed: {error}"))
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(EnvironmentLookupError::Unavailable(format!(
            "{url} answered {status}"
        )));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(EnvironmentLookupError::Failed(format!(
            "Failed to resolve the environment: {url} answered {status}: {body}"
        )));
    }
    let bytes = response.bytes().await.map_err(|error| {
        EnvironmentLookupError::Unavailable(format!("reading the answer of {url} failed: {error}"))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        EnvironmentLookupError::Failed(format!(
            "Failed to resolve the environment: the answer of {url} does not decode: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Answers every request with `status`, `content_type` and `body`.
    async fn serve(status: &'static str, content_type: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                });
            }
        });
        format!("http://{address}/")
    }

    async fn lookup(base_url: &str) -> Result<uuid::Uuid, EnvironmentLookupError> {
        environment_id(
            &reqwest::Url::parse(base_url).unwrap(),
            &crate::Security::Bearer(base_url.to_string()),
            "app",
            "env",
        )
        .await
    }

    #[test]
    async fn a_temporary_status_is_unavailable_whatever_the_body() {
        for (status, content_type, body) in [
            (
                "500 Internal Server Error",
                "application/json",
                r#"{"error":"boom"}"#,
            ),
            (
                "500 Internal Server Error",
                "text/html",
                "<html>Internal Server Error</html>",
            ),
            ("500 Internal Server Error", "text/plain", ""),
            ("502 Bad Gateway", "text/html", "<html>Bad Gateway</html>"),
            ("429 Too Many Requests", "text/plain", "slow down"),
        ] {
            let error = lookup(&serve(status, content_type, body).await)
                .await
                .unwrap_err();
            assert!(
                matches!(error, EnvironmentLookupError::Unavailable(_)),
                "{status} with {content_type} {body:?} gave {error:?}"
            );
        }
    }

    #[test]
    async fn a_refusal_or_an_undecodable_answer_fails() {
        for (status, content_type, body) in [
            ("403 Forbidden", "application/json", r#"{"error":"no"}"#),
            ("401 Unauthorized", "text/plain", ""),
            ("200 OK", "application/json", "{}"),
            ("200 OK", "text/html", "<html></html>"),
        ] {
            let error = lookup(&serve(status, content_type, body).await)
                .await
                .unwrap_err();
            assert!(
                matches!(error, EnvironmentLookupError::Failed(_)),
                "{status} with {content_type} {body:?} gave {error:?}"
            );
        }
    }
}
