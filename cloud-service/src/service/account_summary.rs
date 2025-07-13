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

use super::auth::AuthServiceError;
use crate::model::AccountSummary;
use crate::repo::account_summary::AccountSummaryRepo;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum AccountSummaryServiceError {
    #[error("Internal error: {0}")]
    Internal(#[from] RepoError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
}

impl SafeDisplay for AccountSummaryServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AccountSummaryServiceError::Internal(inner) => inner.to_safe_string(),
            AccountSummaryServiceError::AuthError(inner) => inner.to_safe_string(),
        }
    }
}

#[async_trait]
pub trait AccountSummaryService: Send + Sync {
    async fn get(
        &self,
        skip: i32,
        limit: i32,
    ) -> Result<Vec<AccountSummary>, AccountSummaryServiceError>;

    async fn count(&self) -> Result<u64, AccountSummaryServiceError>;
}

pub struct AccountSummaryServiceDefault {
    account_summary_repo: Arc<dyn AccountSummaryRepo>,
}

impl AccountSummaryServiceDefault {
    pub fn new(account_summary_repo: Arc<dyn AccountSummaryRepo>) -> Self {
        Self {
            account_summary_repo,
        }
    }
}

#[async_trait]
impl AccountSummaryService for AccountSummaryServiceDefault {
    async fn get(
        &self,
        skip: i32,
        limit: i32,
    ) -> Result<Vec<AccountSummary>, AccountSummaryServiceError> {
        match self.account_summary_repo.get(skip, limit).await {
            Ok(account_summary) => Ok(account_summary),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn count(&self) -> Result<u64, AccountSummaryServiceError> {
        match self.account_summary_repo.count().await {
            Ok(count) => Ok(count),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }
}
