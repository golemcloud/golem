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
    /// How long a streaming call keeps reconnecting while the server accepts neither its start
    /// nor a resume.
    pub reconnect: ReconnectPolicy,
}
