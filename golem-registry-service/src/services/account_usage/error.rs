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

use golem_common::model::account::AccountId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::repo::RepoError;

#[derive(Debug, thiserror::Error)]
#[error("Limit {limit_name} exceeded, limit: {limit_value}, current: {current_value}")]
pub struct LimitExceededError {
    pub limit_name: String,
    pub limit_value: u64,
    pub current_value: u64,
}

impl SafeDisplay for LimitExceededError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountUsageError {
    #[error(transparent)]
    LimitExceeded(LimitExceededError),
    #[error("Component size exceeds global limits {0}")]
    ComponentTooLarge(u64),
    #[error("Account {0} not found")]
    AccountNotfound(AccountId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AccountUsageError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::LimitExceeded { .. } => self.to_string(),
            Self::ComponentTooLarge(_) => self.to_string(),
            Self::AccountNotfound(_) => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AccountUsageError, RepoError);
