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

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::{error_forwarders, into_internal_error, SafeDisplay};
use std::fmt::{Debug, Display, Formatter};
use std::time::Duration;
use url::Url;
use anyhow::{anyhow, Context};
use crate::model::login::ExternalLogin;
use super::oauth2_github_client::{OAuth2GithubClient, OAuth2GithubClientError};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum OAuth2ProviderClientError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error)
}

into_internal_error!(OAuth2ProviderClientError);
error_forwarders!(OAuth2ProviderClientError, OAuth2GithubClientError);

impl SafeDisplay for OAuth2ProviderClientError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal Error".to_string(),
        }
    }
}

pub struct OAuth2ProviderClient {
    github: Arc<dyn OAuth2GithubClient>
}

impl OAuth2ProviderClient {
    pub fn new(
        github: Arc<dyn OAuth2GithubClient>
    ) -> Self {
        Self {
            github
        }
    }

    pub async fn external_user_id(
        &self,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2ProviderClientError> {
        let result = self.github.external_user_id(access_token).await?;
        Ok(result)
    }
}
