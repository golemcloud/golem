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

use super::plan::{PlanError, PlanService};
use super::token::{TokenError, TokenService};
use crate::config::AccountsConfig;
use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountRepoError, AccountRevisionRecord, AccountRoleRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use chrono::{DateTime, Utc};
use golem_common::model::PlanId;
use golem_common::model::account::{
    Account, AccountId, AccountRevision, NewAccountData, UpdatedAccountData,
};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::{SafeDisplay, error_forwarders, into_internal_error};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{error, info};

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Email already in use")]
    EmailAlreadyInUse,
    #[error("Concurrent update")]
    ConcurrentUpdate,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AccountError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AccountNotFound(_) => self.to_string(),
            Self::EmailAlreadyInUse => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

into_internal_error!(AccountError);

error_forwarders!(AccountError, PlanError, TokenError, AccountRepoError);

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
            config,
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
                        NewAccountData {
                            name: account.name.clone(),
                            email: account.email.clone(),
                        },
                        vec![account.role.clone()],
                        PlanId(account.plan_id),
                        account_id.clone(),
                    )
                    .await?;
                    // TODO: Deal with failure here
                    self.token_service
                        .create_known_secret(
                            account_id,
                            TokenSecret(account.token),
                            DateTime::<Utc>::MAX_UTC,
                        )
                        .await?;
                }
                Some(_existing_account) => {
                    // TODO: We need to update the account here
                    // TODO: We need to rotate the secret here
                }
            }
        }
        Ok(())
    }

    pub async fn create(
        &self,
        account: NewAccountData,
        actor: AccountId,
    ) -> Result<Account, AccountError> {
        let id = AccountId::new_v4();
        let plan_id = self.get_default_plan_id().await?;
        info!("Creating account: {}", id);
        self.create_internal(id, account, Vec::new(), plan_id, actor)
            .await
    }

    /// create account with the account being the user creating it
    pub async fn create_bootstrapped(
        &self,
        account: NewAccountData,
    ) -> Result<Account, AccountError> {
        let id = AccountId::new_v4();
        info!("Creating account: {}", id);
        let plan_id = self.get_default_plan_id().await?;
        self.create_internal(id.clone(), account, Vec::new(), plan_id, id)
            .await
    }

    pub async fn update(
        &self,
        account_id: &AccountId,
        update: UpdatedAccountData,
        actor: AccountId,
    ) -> Result<Account, AccountError> {
        info!("Updating account: {}", account_id);

        let mut account: Account = self
            .account_repo
            .get_by_id(&account_id.0)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id.clone()))?
            .try_into()?;

        account.name = update.name;
        account.email = update.email;

        self.update_internal(account, actor).await
    }

    pub async fn set_roles(
        &self,
        account_id: &AccountId,
        roles: Vec<AccountRole>,
        actor: AccountId,
    ) -> Result<Account, AccountError> {
        info!("Updating account: {}", account_id);

        let mut account: Account = self
            .account_repo
            .get_by_id(&account_id.0)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id.clone()))?
            .try_into()?;

        account.roles = roles;

        self.update_internal(account, actor).await
    }

    pub async fn get(&self, account_id: &AccountId) -> Result<Account, AccountError> {
        self.get_optional(account_id)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id.clone()))
    }

    async fn create_internal(
        &self,
        id: AccountId,
        account: NewAccountData,
        roles: Vec<AccountRole>,
        plan_id: PlanId,
        actor: AccountId,
    ) -> Result<Account, AccountError> {
        let revision = AccountRevision::INITIAL;
        let result = self
            .account_repo
            .create(AccountRevisionRecord {
                account_id: id.0,
                revision_id: revision.0 as i64,
                name: account.name,
                email: account.email,
                plan_id: plan_id.0,
                audit: DeletableRevisionAuditFields::new(actor.0),
                roles: roles
                    .into_iter()
                    .map(|role| AccountRoleRecord::from_model(id.clone(), revision, role))
                    .collect(),
            })
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(other) => Err(other)?,
        }
    }

    async fn update_internal(
        &self,
        account: Account,
        actor: AccountId,
    ) -> Result<Account, AccountError> {
        let revision = account.revision.next()?;
        let result = self
            .account_repo
            .update(AccountRevisionRecord {
                account_id: account.id.0,
                revision_id: revision.0 as i64,
                name: account.name,
                email: account.email,
                plan_id: account.plan_id.0,
                audit: DeletableRevisionAuditFields::new(actor.0),
                roles: account
                    .roles
                    .into_iter()
                    .map(|role| AccountRoleRecord::from_model(account.id.clone(), revision, role))
                    .collect(),
            })
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(AccountRepoError::VersionAlreadyExists { .. }) => {
                Err(AccountError::ConcurrentUpdate)?
            }
            Err(other) => Err(other)?,
        }
    }

    pub async fn get_optional(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<Account>, AccountError> {
        let record = self.account_repo.get_by_id(&account_id.0).await?;
        Ok(record.map(|r| r.try_into()).transpose()?)
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
