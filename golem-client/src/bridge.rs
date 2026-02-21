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

use crate::{api, Error};

#[derive(Debug, Clone)]
pub enum ClientError {
    InvocationFailed { message: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::InvocationFailed { message } => write!(f, "{}", message),
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
            GolemServer::Cloud { .. } => reqwest::Url::parse("https://api.golem.cloud").unwrap(),
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
}
