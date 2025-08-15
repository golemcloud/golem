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

use crate::repo::account::{AccountRecord, AccountRepo};
use golem_common::model::PlanId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{error, info};
use golem_common::model::account::{Account, AccountId, NewAccountData};
use super::plan::{PlanError, PlanService};
use crate::repo::model::audit::AuditFields;
use anyhow::anyhow;
use crate::config::AccountsConfig;
use super::token::TokenService;
use golem_common::model::auth::TokenSecret;
use chrono::{DateTime, Utc};

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AccountError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AccountNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl From<RepoError> for AccountError {
    fn from(value: RepoError) -> Self {
        Self::InternalError(anyhow::Error::new(value).context("from RepoError"))
    }
}

impl From<PlanError> for AccountError {
    fn from(value: PlanError) -> Self {
        Self::InternalError(anyhow::Error::new(value).context("from PlanError"))
    }
}


pub struct AccountService {
    account_repo: Arc<dyn AccountRepo>,
    plan_service: Arc<PlanService>,
    token_service: Arc<TokenService>,
    config: AccountsConfig,
}

impl AccountService {
    pub fn new(
        account_repo: Arc<dyn AccountRepo>,
        plan_service: Arc<PlanService>,
        token_service: Arc<TokenService>,
        config: AccountsConfig,
    ) -> Self {
        Self {
            account_repo,
            plan_service,
            token_service,
            config
        }
    }

    pub async fn create_initial_accounts(&self) -> Result<(), AccountError> {
        for (name, account) in &self.config.accounts {
            let account_id = AccountId(account.id);
            let existing_account = self.get_optional(&account_id).await?;

            match existing_account {
                None => {
                    info!("Creating initial account {} with id {}", name, account.id);
                    self.create_internal(
                        account_id.clone(),
                        NewAccountData { name: account.name.clone(), email: account.email.clone() },
                        PlanId(account.plan_id.clone())
                    ).await?;
                    // TODO: Deal with failure here
                    self.token_service.create_known_secret(account_id, TokenSecret(account.token), DateTime::<Utc>::MAX_UTC).await?;
                }
                Some(_existing_account) => {
                    // TODO: We need to update the account here
                    // TODO: We need to rotate the secret here
                }
            }
        };
        Ok(())
    }

    pub async fn create(&self, account: NewAccountData) -> Result<Account, AccountError> {
        let id = AccountId::new_v4();
        let plan_id = self.get_default_plan_id().await?;
        info!("Creating account: {}", id);
        self.create_internal(id, account, plan_id).await
    }

    pub async fn get(&self, account_id: &AccountId) -> Result<Account, AccountError> {
        self.get_optional(account_id).await?.ok_or(AccountError::AccountNotFound(account_id.clone()))
    }

    // pub async fn update(
    //     &self,
    //     account_id: &AccountId,
    //     account: &AccountData,
    // ) -> Result<Account, AccountError> {
    //     info!("Updating account: {}", account_id);
    //     let current_account = self.account_repo.get_by_id(&account_id.0).await?;
    //     let plan_id = match current_account {
    //         Some(current_account) => current_account.plan_id,
    //         None => self.get_default_plan_id().await?.0,
    //     };

    //     let result = self
    //         .account_repo
    //         .update(&AccountRecord {
    //             id: account_id.value.clone(),
    //             name: account.name.clone(),
    //             email: account.email.clone(),
    //             plan_id,
    //         })
    //         .await;

    //     match result {
    //         Ok(account_record) => Ok(account_record.into()),
    //         Err(err) => {
    //             error!("DB call failed. {}", err);
    //             Err(err.into())
    //         }
    //     }
    // }

    // /// Get all users
    // pub async fn find(
    //     &self,
    //     email: Option<&str>,
    //     viewable_accounts: ViewableAccounts,
    // ) -> Result<Vec<Account>, AccountError> {
    //     let results = match viewable_accounts {
    //         ViewableAccounts::All => self.account_repo.find_all(email).await?,
    //         ViewableAccounts::Limited { account_ids } => {
    //             let ids = account_ids
    //                 .into_iter()
    //                 .map(|ai| ai.value)
    //                 .collect::<Vec<_>>();
    //             self.account_repo.find(&ids, email).await?
    //         }
    //     };

    //     Ok(results.into_iter().map(|v| v.into()).collect())
    // }

    // pub async fn get_plan(&self, account_id: &AccountId) -> Result<Plan, AccountError> {
    //     info!("Get plan: {}", account_id);

    //     let result = self.account_repo.get(&account_id.value).await;
    //     match result {
    //         Ok(Some(account_record)) => {
    //             match self.plan_service.get(&PlanId(account_record.plan_id)).await {
    //                 Ok(Some(plan)) => Ok(plan),
    //                 Ok(None) => Err(format!(
    //                     "Could not find plan with id: {}",
    //                     account_record.plan_id
    //                 )
    //                 .into()),
    //                 Err(err) => {
    //                     error!("DB call failed. {:?}", err);
    //                     Err(err.into())
    //                 }
    //             }
    //         }
    //         Ok(None) => Err(AccountError::AccountNotFound(account_id.clone())),
    //         Err(err) => {
    //             error!("DB call failed. {}", err);
    //             Err(err.into())
    //         }
    //     }
    // }

    // pub async fn delete(&self, account_id: &AccountId) -> Result<(), AccountError> {
    //     let result = self.account_repo.delete(&account_id.value).await;
    //     match result {
    //         Ok(_) => Ok(()),
    //         Err(err) => {
    //             error!("DB call failed. {}", err);
    //             Err(err.into())
    //         }
    //     }
    // }

    async fn create_internal(
        &self,
        id: AccountId,
        account: NewAccountData,
        plan_id: PlanId,
    ) -> Result<Account, AccountError> {
        let record = self
            .account_repo
            .create(AccountRecord {
                account_id: id.0,
                name: account.name,
                email: account.email,
                plan_id: plan_id.0,
                audit: AuditFields::new(id.0),
            })
            .await?
            .ok_or(anyhow!("Duplicated account on fresh id: {id}"))?;

        Ok(record.into())
    }

    pub async fn get_optional(&self, account_id: &AccountId) -> Result<Option<Account>, AccountError> {
        info!("Get account: {}", account_id);

        let result = self
            .account_repo
            .get_by_id(&account_id.0)
            .await?
            .map(|a| a.into());

        Ok(result)
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
