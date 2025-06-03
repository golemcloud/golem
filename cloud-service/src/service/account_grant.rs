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

use super::auth::{AuthService, AuthServiceError};
use crate::repo::account::AccountRepo;
use crate::repo::account_grant::AccountGrantRepo;
use crate::{auth::AccountAuthorisation, model::AccountAction};
use async_trait::async_trait;
use cloud_common::model::Role;
use golem_common::model::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum AccountGrantServiceError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Arg Validation error: {}", .0.join(", "))]
    ArgValidation(Vec<String>),
    #[error("Internal error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error(transparent)]
    InternalAuthError(#[from] AuthServiceError),
}

impl SafeDisplay for AccountGrantServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AccountGrantServiceError::AccountNotFound(_) => self.to_string(),
            AccountGrantServiceError::ArgValidation(_) => self.to_string(),
            AccountGrantServiceError::InternalRepoError(inner) => inner.to_safe_string(),
            AccountGrantServiceError::InternalAuthError(inner) => inner.to_safe_string(),
        }
    }
}

#[async_trait]
pub trait AccountGrantService {
    async fn get(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Role>, AccountGrantServiceError>;
    async fn add(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError>;
    async fn remove(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError>;
}

pub struct AccountGrantServiceDefault {
    auth_service: Arc<dyn AuthService>,
    account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
}

impl AccountGrantServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService>,
        account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
    ) -> Self {
        Self {
            auth_service,
            account_grant_repo,
            account_repo,
        }
    }
}

#[async_trait]
impl AccountGrantService for AccountGrantServiceDefault {
    async fn get(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Role>, AccountGrantServiceError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::ViewAccountGrants)
            .await?;

        let roles = match self.account_grant_repo.get(account_id).await {
            Ok(roles) => roles,
            Err(error) => {
                error!("DB call failed. {:?}", error);
                return Err(error.into());
            }
        };

        Ok(roles)
    }

    async fn add(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::CreateAccountGrant)
            .await?;

        let account = self.account_repo.get(account_id.value.as_str()).await?;

        if account.is_none() {
            Err(AccountGrantServiceError::AccountNotFound(
                account_id.clone(),
            ))
        } else {
            match self.account_grant_repo.add(account_id, role).await {
                Ok(_) => Ok(()),
                Err(error) => {
                    error!("DB call failed. {:?}", error);
                    Err(error.into())
                }
            }
        }
    }

    async fn remove(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::DeleteAccountGrant)
            .await?;

        if auth.token.account_id == *account_id && role == &Role::Admin {
            return Err(AccountGrantServiceError::ArgValidation(vec![
                "Cannot remove Admin role from current account.".to_string(),
            ]));
        };
        match self.account_grant_repo.remove(account_id, role).await {
            Ok(_) => Ok(()),
            Err(error) => {
                error!("DB call failed. {:?}", error);
                Err(error.into())
            }
        }
    }
}
