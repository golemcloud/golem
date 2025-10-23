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
use crate::config::AccountsConfig;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountRepoError, AccountRevisionRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use anyhow::anyhow;
use golem_common::model::account::PlanId;
use golem_common::model::account::{Account, AccountCreation, AccountId, AccountUpdate};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_service_base::model::auth::{AccountAction, GlobalAction};
use golem_common::{SafeDisplay, error_forwarding};
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
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AccountError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AccountNotFound(_) => self.to_string(),
            Self::EmailAlreadyInUse => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AccountError, PlanError, AccountRepoError);

pub struct AccountService {
    account_repo: Arc<dyn AccountRepo>,
    plan_service: Arc<PlanService>,
    config: AccountsConfig,
}

impl AccountService {
    pub fn new(
        account_repo: Arc<dyn AccountRepo>,
        plan_service: Arc<PlanService>,
        config: AccountsConfig,
    ) -> Self {
        Self {
            account_repo,
            plan_service,
            config,
        }
    }

    pub async fn create_initial_accounts(&self, auth: &AuthCtx) -> Result<(), AccountError> {
        for (name, account) in &self.config.accounts {
            let account_id = AccountId(account.id);
            let existing_account = self.get_optional(&account_id, auth).await?;

            if existing_account.is_none() {
                info!("Creating initial account {} with id {}", name, account.id);
                self.create_internal(
                    account_id.clone(),
                    AccountCreation {
                        name: account.name.clone(),
                        email: account.email.clone(),
                    },
                    vec![account.role],
                    PlanId(account.plan_id),
                    auth,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub(super) fn initial_tokens(&self) -> Vec<(AccountId, TokenSecret)> {
        let mut result = Vec::with_capacity(self.config.accounts.len());
        for account in self.config.accounts.values() {
            result.push((AccountId(account.id), TokenSecret(account.token)));
        }
        result
    }

    pub async fn create(
        &self,
        account: AccountCreation,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_global_action(GlobalAction::CreateAccount)?;

        let id = AccountId::new_v4();
        let plan_id = self.get_default_plan_id(auth).await?;
        info!("Creating account: {}", id);
        self.create_internal(id, account, Vec::new(), plan_id, auth)
            .await
    }

    pub async fn update(
        &self,
        account_id: &AccountId,
        update: AccountUpdate,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.get(account_id, auth).await?;

        auth.authorize_account_action(account_id, AccountAction::UpdateAccount)?;

        info!("Updating account: {}", account_id);

        account.name = update.name;
        account.email = update.email;

        self.update_internal(account, auth).await
    }

    pub async fn set_roles(
        &self,
        account_id: &AccountId,
        roles: Vec<AccountRole>,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.get(account_id, auth).await?;

        auth.authorize_account_action(account_id, AccountAction::SetRoles)?;

        info!("Updating account: {}", account_id);

        account.roles = roles;

        self.update_internal(account, auth).await
    }

    pub async fn delete(
        &self,
        account_id: &AccountId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.get(account_id, auth).await?;

        auth.authorize_account_action(account_id, AccountAction::DeleteAccount)?;

        info!("Deleting account: {}", account_id);

        let current_revision = account.revision;

        account.revision = account.revision.next()?;

        let record = AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::deletion(auth.account_id().0.clone()),
        );

        let result = self
            .account_repo
            .delete(current_revision.into(), record)
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::ConcurrentModification) => Err(AccountError::ConcurrentUpdate)?,
            Err(other) => Err(other)?,
        }
    }

    pub async fn get(
        &self,
        account_id: &AccountId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_account_action(account_id, AccountAction::ViewAccount)
            // Visibility is not enforced in the repo, so we need to map permissions to visibility
            .map_err(|_| AccountError::AccountNotFound(account_id.clone()))?;

        let account = self
            .account_repo
            .get_by_id(&account_id.0)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id.clone()))?
            .try_into()?;

        Ok(account)
    }

    pub async fn get_optional(
        &self,
        account_id: &AccountId,
        auth: &AuthCtx,
    ) -> Result<Option<Account>, AccountError> {
        match self.get(account_id, auth).await {
            Ok(account) => Ok(Some(account)),
            Err(AccountError::AccountNotFound(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    async fn create_internal(
        &self,
        id: AccountId,
        account: AccountCreation,
        roles: Vec<AccountRole>,
        plan_id: PlanId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_global_action(GlobalAction::CreateAccount)?;

        if id == AccountId::SYSTEM {
            Err(anyhow!("Cannot create account with reserved account id"))?
        };

        let record = AccountRevisionRecord::new(
            id,
            account.name,
            account.email,
            plan_id,
            roles,
            auth.account_id().clone(),
        );

        let result = self.account_repo.create(record).await;

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
        mut account: Account,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let current_revision = account.revision;

        account.revision = account.revision.next()?;

        let record = AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::new(auth.account_id().0.clone()),
        );

        let result = self
            .account_repo
            .update(current_revision.into(), record)
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(AccountRepoError::ConcurrentModification) => Err(AccountError::ConcurrentUpdate)?,
            Err(other) => Err(other)?,
        }
    }

    async fn get_default_plan_id(&self, auth: &AuthCtx) -> Result<PlanId, AccountError> {
        let plan_id = self
            .plan_service
            .get_default_plan(auth)
            .await
            .map(|plan| plan.plan_id)?;

        Ok(plan_id)
    }
}
