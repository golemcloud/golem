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

use super::oauth2_provider_client::{OAuth2ProviderClient, OAuth2ProviderClientError};
use async_trait::async_trait;
use chrono::Utc;
use golem_common::model::auth::TokenSecret;
use golem_common::{error_forwarders, into_internal_error, SafeDisplay};
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tracing::debug;
use tracing::error;
use crate::services::account::AccountError;
use crate::services::token::TokenError;

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("Unknown token state: {0}")]
    UnknownTokenState(String),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error)
}

impl SafeDisplay for LoginError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::UnknownTokenState(_) => self.to_string(),
            Self::InternalError(_) => "Internal Error".to_string()
        }
    }
}

into_internal_error!(LoginError);

error_forwarders!(LoginError, AccountError, TokenError, RepoError);
