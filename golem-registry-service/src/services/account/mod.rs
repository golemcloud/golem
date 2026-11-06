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
use crate::repo::model::account::{
    AccountDeletionHoldRecord, AccountExtRevisionRecord, AccountRepoError, AccountRevisionRecord,
};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::plan::PlanRecord;
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use anyhow::anyhow;
use golem_common::model::account::{
    Account, AccountCreation, AccountDeletionHold, AccountDeletionHoldCreation, AccountEmail,
    AccountId, AccountRevision, AccountSetPlan, AccountUpdate,
};
use golem_common::model::card::owner::{AccountOwnerPattern, EmptyOwnerPattern};
use golem_common::model::card::{
    AccountResourcePattern, AccountVerb, ClassPermissionTarget, PermissionTarget,
    SystemResourcePattern, SystemVerb,
};
use golem_common::model::plan::{Plan, PlanId};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
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
    #[error("The account cannot be deleted while it has deletion holds: {}", .0.join("; "))]
    DeletionHeld(Vec<String>),
    #[error("Invalid deletion hold: {0}")]
    InvalidDeletionHold(String),
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
            Self::DeletionHeld(_) => self.to_string(),
            Self::InvalidDeletionHold(_) => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AccountError, PlanError, AccountRepoError, RepoError);

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
            match self.get(account.id, &AuthCtx::System).await {
                Ok(_) => {}
                Err(AccountError::AccountNotFound(_)) => {
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
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }

    pub async fn create(
        &self,
        account: AccountCreation,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        authorize_system_permission(auth, SystemVerb::CreateAccount)?;

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
        let mut account = self.get(account_id, auth).await?;

        authorize_account_permission(auth, &account.email, AccountVerb::Update)?;

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
        let mut account = self.get(account_id, auth).await?;

        authorize_account_permission(auth, &account.email, AccountVerb::SetPlan)?;

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
        let record = Self::next_revision_record(account, auth)?;
        let result = self
            .account_repo
            .update_plan_and_reconcile_overrides(record)
            .await;
        Self::map_update_result(result)
    }

    pub async fn delete(
        &self,
        account_id: AccountId,
        current_revision: AccountRevision,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let mut account = self.get(account_id, auth).await?;

        authorize_account_permission(auth, &account.email, AccountVerb::Delete)?;

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
            Err(AccountRepoError::DeletionHeld(holds)) => Err(AccountError::DeletionHeld(
                holds.into_iter().map(|hold| hold.reason).collect(),
            ))?,
            Err(other) => Err(other)?,
        }
    }

    pub async fn list_deletion_holds(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<AccountDeletionHold>, AccountError> {
        self.get(account_id, auth).await?;

        Ok(self
            .account_repo
            .list_deletion_holds(account_id.0)
            .await?
            .into_iter()
            .map(AccountDeletionHold::from)
            .collect())
    }

    /// Places the deletion hold of `holder_account_id` on the account, or replaces its reason.
    pub async fn place_deletion_hold(
        &self,
        account_id: AccountId,
        holder_account_id: AccountId,
        creation: AccountDeletionHoldCreation,
        auth: &AuthCtx,
    ) -> Result<AccountDeletionHold, AccountError> {
        let account = self.get(account_id, auth).await?;

        authorize_deletion_hold_permission(auth, &account.email, holder_account_id)?;

        let reason = validate_deletion_hold_reason(creation.reason)?;

        info!(
            "Placing the deletion hold of account {} on account {}",
            holder_account_id, account_id
        );

        let record = self
            .account_repo
            .place_deletion_hold(AccountDeletionHoldRecord::new(
                account_id,
                holder_account_id,
                reason,
            ))
            .await?
            .ok_or(AccountError::AccountNotFound(account_id))?;

        Ok(record.into())
    }

    /// Removes the deletion hold of `holder_account_id` from the account. Removing a hold that
    /// does not exist succeeds.
    pub async fn remove_deletion_hold(
        &self,
        account_id: AccountId,
        holder_account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountError> {
        let account = self.get(account_id, auth).await?;

        authorize_deletion_hold_permission(auth, &account.email, holder_account_id)?;

        info!(
            "Removing the deletion hold of account {} from account {}",
            holder_account_id, account_id
        );

        self.account_repo
            .remove_deletion_hold(account_id.0, holder_account_id.0)
            .await?;

        Ok(())
    }

    pub async fn get(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let account: Account = self
            .account_repo
            .get_by_id(account_id.0)
            .await?
            .ok_or(AccountError::AccountNotFound(account_id))?
            .try_into()?;
        authorize_account_permission(auth, &account.email, AccountVerb::View)
            .map_err(|_| AccountError::AccountNotFound(account_id))?;

        Ok(account)
    }

    pub async fn get_plan(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Plan, AccountError> {
        let account = self.get(account_id, auth).await?;
        authorize_account_permission(auth, &account.email, AccountVerb::ViewPlan)?;

        self.plan_service
            .get(&account.plan_id, &AuthCtx::System)
            .await
            .map_err(|e| match e {
                PlanError::PlanNotFound(plan_id) => AccountError::PlanByIdNotFound(plan_id),
                other => other.into(),
            })
    }

    pub(crate) async fn get_plan_record(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<PlanRecord, AccountError> {
        let account = self.get(account_id, auth).await?;
        authorize_account_permission(auth, &account.email, AccountVerb::ViewPlan)?;

        self.plan_service
            .get_record(&account.plan_id, &AuthCtx::System)
            .await
            .map_err(|error| match error {
                PlanError::PlanNotFound(plan_id) => AccountError::PlanByIdNotFound(plan_id),
                other => other.into(),
            })
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

        authorize_account_permission(auth, &account.email, AccountVerb::View)
            .map_err(|_| AccountError::AccountByEmailNotFound(account_email.to_string()))?;

        Ok(account)
    }

    /// Resolves an account email to its id **without** enforcing `AccountVerb::View`.
    ///
    /// For feeding a subsequently-authorized resource lookup (e.g. the by-email plugin
    /// endpoints) so that a caller holding only a resource grant is not additionally required
    /// to have account-view. The full-account endpoint ([`Self::get_by_email`]) keeps its
    /// `AccountVerb::View` gate. Only ever use the returned id to build a resource
    /// authorization that is then checked; never expose it or other account data unchecked.
    pub async fn resolve_account_id_by_email_unchecked(
        &self,
        account_email: &str,
    ) -> Result<AccountId, AccountError> {
        let account: Account = self
            .account_repo
            .get_by_email(account_email)
            .await?
            .ok_or(AccountError::AccountByEmailNotFound(
                account_email.to_string(),
            ))?
            .try_into()?;

        Ok(account.id)
    }

    async fn create_internal(
        &self,
        id: AccountId,
        account: AccountCreation,
        plan_id: PlanId,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        authorize_system_permission(auth, SystemVerb::CreateAccount)?;

        if id == AccountId::SYSTEM {
            Err(anyhow!("Cannot create account with reserved account id"))?
        };

        let email = account.email.clone().into_inner();
        let account_root_card = account_root_card_record(id, account.email.clone(), &account.roles);

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

    async fn update_internal(
        &self,
        account: Account,
        auth: &AuthCtx,
    ) -> Result<Account, AccountError> {
        let record = Self::next_revision_record(account, auth)?;

        let result = self.account_repo.update(record).await;

        Self::map_update_result(result)
    }

    fn next_revision_record(
        mut account: Account,
        auth: &AuthCtx,
    ) -> Result<AccountRevisionRecord, AccountError> {
        account.revision = account.revision.next()?;
        Ok(AccountRevisionRecord::from_model(
            account,
            DeletableRevisionAuditFields::new(auth.actor_account_id().0),
        ))
    }

    fn map_update_result(
        result: Result<AccountExtRevisionRecord, AccountRepoError>,
    ) -> Result<Account, AccountError> {
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
    account_email: &AccountEmail,
    verb: AccountVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&account_permission_target(account_email, verb))
}

/// A caller places and removes its own deletion hold with `AccountVerb::HoldDeletion` on the
/// account. Only `SystemVerb::OverrideDeletionHold` acts on the hold of another holder, so the
/// owner of an account, who has every account verb on it, cannot remove the hold of another
/// account.
fn authorize_deletion_hold_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    holder_account_id: AccountId,
) -> Result<(), AuthorizationError> {
    if holder_account_id == auth.access_account_id() {
        authorize_account_permission(auth, account_email, AccountVerb::HoldDeletion)
    } else {
        authorize_system_permission(auth, SystemVerb::OverrideDeletionHold)
    }
}

const MAX_DELETION_HOLD_REASON_CHARS: usize = 1000;

fn validate_deletion_hold_reason(reason: String) -> Result<String, AccountError> {
    if reason.trim().is_empty() {
        return Err(AccountError::InvalidDeletionHold(
            "the reason is empty".to_string(),
        ));
    }
    if reason.chars().count() > MAX_DELETION_HOLD_REASON_CHARS {
        return Err(AccountError::InvalidDeletionHold(format!(
            "the reason is longer than {MAX_DELETION_HOLD_REASON_CHARS} characters"
        )));
    }
    Ok(reason)
}

fn authorize_system_permission(auth: &AuthCtx, verb: SystemVerb) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::System(ClassPermissionTarget {
        verb: Some(verb),
        owner: EmptyOwnerPattern,
        resource: SystemResourcePattern,
    }))
}

fn account_permission_target(account_email: &AccountEmail, verb: AccountVerb) -> PermissionTarget {
    PermissionTarget::Account(ClassPermissionTarget {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: account_email.clone(),
        },
        resource: AccountResourcePattern,
    })
}
