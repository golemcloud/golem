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

use super::auth::{AuthServiceError, ViewableAccounts};
use crate::model::{Account, AccountData, Plan};
use crate::repo::account::{AccountRecord, AccountRepo};
use crate::service::plan::{PlanError, PlanService};
use async_trait::async_trait;
use golem_common::model::AccountId;
use golem_common::model::PlanId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{error, info};

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Arg Validation error: {}", .0.join(", "))]
    ArgValidation(Vec<String>),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Internal error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error(transparent)]
    InternalPlanError(#[from] PlanError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
}

impl SafeDisplay for AccountError {
    fn to_safe_string(&self) -> String {
        match self {
            AccountError::AccountNotFound(_) => self.to_string(),
            AccountError::ArgValidation(_) => self.to_string(),
            AccountError::Internal(_) => self.to_string(),
            AccountError::InternalRepoError(inner) => inner.to_safe_string(),
            AccountError::InternalPlanError(inner) => inner.to_safe_string(),
            AccountError::AuthError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<String> for AccountError {
    fn from(error: String) -> Self {
        AccountError::Internal(error)
    }
}

#[async_trait]
pub trait AccountService: Send + Sync {
    async fn create(&self, id: &AccountId, account: &AccountData) -> Result<Account, AccountError>;

    async fn update(
        &self,
        account_id: &AccountId,
        account: &AccountData,
    ) -> Result<Account, AccountError>;

    async fn get(&self, account_id: &AccountId) -> Result<Account, AccountError>;

    /// Get all matching accounts. This will return your account + all accounts that you got access through at least one grant.
    async fn find(
        &self,
        email: Option<&str>,
        viewable_accounts: ViewableAccounts,
    ) -> Result<Vec<Account>, AccountError>;

    async fn get_plan(&self, account_id: &AccountId) -> Result<Plan, AccountError>;

    async fn delete(&self, account_id: &AccountId) -> Result<(), AccountError>;
}

pub struct AccountServiceDefault {
    account_repo: Arc<dyn AccountRepo>,
    plan_service: Arc<dyn PlanService>,
}

impl AccountServiceDefault {
    pub fn new(account_repo: Arc<dyn AccountRepo>, plan_service: Arc<dyn PlanService>) -> Self {
        AccountServiceDefault {
            account_repo,
            plan_service,
        }
    }

    async fn get_default_plan_id(&self) -> Result<PlanId, AccountError> {
        let plan_id = self
            .plan_service
            .get_default_plan()
            .await
            .map(|plan| plan.plan_id)?;

        Ok(plan_id)
    }
}

#[async_trait]
impl AccountService for AccountServiceDefault {
    async fn create(&self, id: &AccountId, account: &AccountData) -> Result<Account, AccountError> {
        let plan_id = self.get_default_plan_id().await?;
        info!("Creating account: {}", id);
        match self
            .account_repo
            .create(&AccountRecord {
                id: id.clone().value,
                name: account.name.clone(),
                email: account.email.clone(),
                plan_id: plan_id.0,
            })
            .await
        {
            Ok(Some(account_record)) => Ok(account_record.into()),
            Ok(None) => Err(format!("Duplicated account on fresh id: {id}").into()),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    async fn update(
        &self,
        account_id: &AccountId,
        account: &AccountData,
    ) -> Result<Account, AccountError> {
        info!("Updating account: {}", account_id);
        let current_account = self.account_repo.get(&account_id.value).await?;
        let plan_id = match current_account {
            Some(current_account) => current_account.plan_id,
            None => self.get_default_plan_id().await?.0,
        };
        let result = self
            .account_repo
            .update(&AccountRecord {
                id: account_id.value.clone(),
                name: account.name.clone(),
                email: account.email.clone(),
                plan_id,
            })
            .await;
        match result {
            Ok(account_record) => Ok(account_record.into()),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    async fn get(&self, account_id: &AccountId) -> Result<Account, AccountError> {
        info!("Get account: {}", account_id);

        let result = self.account_repo.get(&account_id.value).await;
        match result {
            Ok(Some(account_record)) => Ok(account_record.into()),
            Ok(None) => Err(AccountError::AccountNotFound(account_id.clone())),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    /// Get all users
    async fn find(
        &self,
        email: Option<&str>,
        viewable_accounts: ViewableAccounts,
    ) -> Result<Vec<Account>, AccountError> {
        let results = match viewable_accounts {
            ViewableAccounts::All => self.account_repo.find_all(email).await?,
            ViewableAccounts::Limited { account_ids } => {
                let ids = account_ids
                    .into_iter()
                    .map(|ai| ai.value)
                    .collect::<Vec<_>>();
                self.account_repo.find(&ids, email).await?
            }
        };

        Ok(results.into_iter().map(|v| v.into()).collect())
    }

    async fn get_plan(&self, account_id: &AccountId) -> Result<Plan, AccountError> {
        info!("Get plan: {}", account_id);

        let result = self.account_repo.get(&account_id.value).await;
        match result {
            Ok(Some(account_record)) => {
                match self.plan_service.get(&PlanId(account_record.plan_id)).await {
                    Ok(Some(plan)) => Ok(plan),
                    Ok(None) => Err(format!(
                        "Could not find plan with id: {}",
                        account_record.plan_id
                    )
                    .into()),
                    Err(err) => {
                        error!("DB call failed. {:?}", err);
                        Err(err.into())
                    }
                }
            }
            Ok(None) => Err(AccountError::AccountNotFound(account_id.clone())),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    async fn delete(&self, account_id: &AccountId) -> Result<(), AccountError> {
        let result = self.account_repo.delete(&account_id.value).await;
        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }
}
