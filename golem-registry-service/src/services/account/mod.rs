// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

mod card;

use self::card::account_root_card_record;
use super::plan::{PlanError, PlanService};
use crate::config::PrecreatedAccount;
use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountRepoError, AccountRevisionRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use anyhow::anyhow;
use golem_common::model::account::{
    Account, AccountCreation, AccountId, AccountRevision, AccountSetPlan, AccountUpdate,
};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    AccountResourcePattern, AccountVerb, ClassPermissionTarget, PermissionTarget,
};
use golem_common::model::plan::PlanId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::GlobalAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Account by email not found: {0}")]
    AccountByEmailNotFound(String),
    #[error("Plan for id not found: {0}")]
    PlanByIdNotFound(PlanId),
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
            Self::AccountByEmailNotFound(_) => self.to_string(),
            Self::EmailAlreadyInUse => self.to_string(),
            Self::PlanByIdNotFound(_) => self.to_string(),
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
    default_plan_id: PlanId,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl AccountService {
    pub fn new(
        account_repo: Arc<dyn AccountRepo>,
        plan_service: Arc<PlanService>,
        default_plan_id: PlanId,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            account_repo,
            plan_service,
            default_plan_id,
            registry_change_notifier,
        }
    }

    pub async fn create_initial_accounts(
        &self,
        accounts: &HashMap<String, PrecreatedAccount>,
    ) -> Result<(), AccountError> {
        for (name, account) in accounts {
            let existing_account = self.get_optional(account.id, &AuthCtx::System).await?;

            if existing_account.is_none() {
                info!("Creating initial account {} with id {}", name, account.id);
                self.create_internal(
                    account.id,
                    AccountCreation {
                        name: account.name.clone(),
                        email: account.email.clone(),
                        roles: vec![account.role],
                    },
                    account.plan_id,
                    &AuthCtx::System,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn create(
        &self,
        account: AccountCreation,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_global_action(GlobalAction::CreateAccount)?;

        let id = AccountId::new();
        info!("Creating account: {}", id);
        self.create_internal(id, account, self.default_plan_id, auth)
            .await
    }

    pub async fn update(
        &self,
        account_id: AccountId,
        update: AccountUpdate,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.load(account_id).await?;

        authorize_account_permission(auth, account_id, AccountVerb::Update)?;

        if update.current_revision != account.revision {
            return Err(AccountError::ConcurrentUpdate);
        };

        info!("Updating account: {}", account_id);

        if let Some(new_name) = update.name {
            account.name = new_name;
        }

        self.update_internal(account, auth).await
    }

    pub async fn set_plan(
        &self,
        account_id: AccountId,
        update: AccountSetPlan,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.load(account_id).await?;

        authorize_account_permission(auth, account_id, AccountVerb::SetPlan)?;

        if update.current_revision != account.revision {
            return Err(AccountError::ConcurrentUpdate);
        };

        info!("Updating account: {}", account_id);

        // check that plan exists
        self.plan_service
            .get(&update.plan, &AuthCtx::System)
            .await
            .map_err(|e| match e {
                PlanError::PlanNotFound(plan_id) => AccountError::PlanByIdNotFound(plan_id),
                other => other.into(),
            })?;

        account.plan_id = update.plan;

        self.update_internal(account, auth).await
    }

    pub async fn delete(
        &self,
        account_id: AccountId,
        current_revision: AccountRevision,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account: Account = self.load(account_id).await?;

        authorize_account_permission(auth, account_id, AccountVerb::Delete)?;

        if current_revision != account.revision {
            return Err(AccountError::ConcurrentUpdate);
        };

        info!("Deleting account: {}", account_id);

        account.revision = account.revision.next()?;

        let record = AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::deletion(auth.actor_account_id().0),
        );

        match self.account_repo.delete(record).await {
            Ok(record) => {
                let account: Account = record
                    .signal_new_events_available(&self.registry_change_notifier)
                    .try_into()?;
                Ok(account)
            }
            Err(AccountRepoError::ConcurrentModification) => Err(AccountError::ConcurrentUpdate)?,
            Err(other) => Err(other)?,
        }
    }

    pub async fn get(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        authorize_account_permission(auth, account_id, AccountVerb::View)
            .map_err(|_| AccountError::AccountNotFound(account_id))?;

        self.load(account_id).await
    }

    pub async fn get_by_email(
        &self,
        account_email: &str,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let account: Account = self
            .account_repo
            .get_by_email(account_email)
            .await?
            .ok_or(AccountError::AccountByEmailNotFound(
                account_email.to_string(),
            ))?
            .try_into()?;

        authorize_account_permission(auth, account.id, AccountVerb::View)
            .map_err(|_| AccountError::AccountByEmailNotFound(account_email.to_string()))?;

        Ok(account)
    }

    pub async fn get_optional(
        &self,
        account_id: AccountId,
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
        plan_id: PlanId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        auth.authorize_global_action(GlobalAction::CreateAccount)?;

        if id == AccountId::SYSTEM {
            Err(anyhow!("Cannot create account with reserved account id"))?
        };

        let email = account.email.into_inner();
        let account_root_card = account_root_card_record(id, &account.roles);

        let record = AccountRevisionRecord::new(
            id,
            account.name,
            email,
            plan_id,
            account.roles,
            auth.actor_account_id(),
        );

        let result = self.account_repo.create(record, account_root_card).await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(other) => Err(other)?,
        }
    }

    async fn load(&self, account_id: AccountId) -> Result<Account, AccountError> {
        self.account_repo
            .get_by_id(account_id.0)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id))?
            .try_into()
            .map_err(AccountError::from)
    }

    async fn update_internal(
        &self,
        mut account: Account,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        account.revision = account.revision.next()?;

        let record = AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::new(auth.actor_account_id().0),
        );

        let result = self.account_repo.update(record).await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AccountRepoError::AccountViolatesUniqueness) => {
                Err(AccountError::EmailAlreadyInUse)?
            }
            Err(AccountRepoError::ConcurrentModification) => Err(AccountError::ConcurrentUpdate)?,
            Err(other) => Err(other)?,
        }
    }
}

fn authorize_account_permission(
    auth: &AuthCtx,
    account_id: AccountId,
    verb: AccountVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&account_permission_target(account_id, verb))
}

fn account_permission_target(account_id: AccountId, verb: AccountVerb) -> PermissionTarget {
    PermissionTarget::Account(ClassPermissionTarget {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: account_id.to_string(),
        },
        resource: AccountResourcePattern,
    })
}
