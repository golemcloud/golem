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

pub mod auth;
pub mod limit;
pub mod plugin;
pub mod project;

use golem_common::config::{ConfigExample, HasConfigExamples};
use golem_common::model::auth::TokenSecret;
use golem_common::model::RetryConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tonic::metadata::MetadataMap;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteCloudServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl RemoteCloudServiceConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse CloudService URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build CloudService URI")
    }
}

impl Default for RemoteCloudServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            access_token: Uuid::parse_str("5c832d93-ff85-4a8f-9803-513950fdfdb1")
                .expect("invalid UUID"),
            retries: RetryConfig::default(),
        }
    }
}

impl HasConfigExamples<RemoteCloudServiceConfig> for RemoteCloudServiceConfig {
    fn examples() -> Vec<ConfigExample<RemoteCloudServiceConfig>> {
        vec![]
    }
}

pub fn authorised_request<T>(request: T, access_token: &Uuid) -> tonic::Request<T> {
    let mut req = tonic::Request::new(request);
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {access_token}").parse().unwrap(),
    );
    req
}

pub fn get_authorisation_token(metadata: MetadataMap) -> Option<TokenSecret> {
    let auth = metadata
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());

    match auth {
        Some(a) if a.to_lowercase().starts_with("bearer ") => {
            let t = &a[7..a.len()];
            TokenSecret::from_str(t.trim()).ok()
        }
        _ => None,
    }
}
