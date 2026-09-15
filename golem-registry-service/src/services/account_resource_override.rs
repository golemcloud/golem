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

use super::account::{AccountError, AccountService};
use super::account_usage::error::AccountUsageError;
use super::account_usage::{authorize_account_usage_permission, map_account_error};
use crate::repo::account_resource_override::{
    AccountResourceOverrideRepo, AdminResourceGrantRepoError, OverridePolicy,
    OverridePolicyViolation, SetAccountResourceOverrideError,
};
use crate::repo::model::account_resource_override::AccountResourceOverrideDimension;
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AdminResourceGrantChange, AdminResourceGrantDimension, MemoryLimit, SetAdminResourceGrant,
    StorageLimit,
};
use golem_common::model::auth::AccountRole;
use golem_common::model::card::AccountUsageVerb;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::{RepoError, SqlDateTime};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AccountResourceOverrideError {
    #[error("{0} is not user configurable")]
    NotUserConfigurable(&'static str),
    #[error("{0} exceeds plan ceiling {1}")]
    ExceedsPlanCeiling(&'static str, u64),
    #[error("{0} is below plan default {1}")]
    BelowPlanDefault(&'static str, u64),
    #[error("{0} is disabled because managed filesystem quotas are unavailable")]
    FeatureDisabled(&'static str),
    #[error("Only account owners may change per-agent resource limits")]
    OwnerOnly,
    #[error("Only administrators may change resource grants")]
    AdminOnly,
    #[error("Grant value {value} does not exceed the currently resolved value {current}")]
    GrantDoesNotIncreaseResolvedValue { value: u64, current: u64 },
    #[error("Promotional grants require a future expiry")]
    PromotionalExpiryRequired,
    #[error("The grant value exceeds the supported internal range")]
    GrantValueOverflow,
    #[error("No active grant exists for this resource dimension")]
    GrantNotFound,
    #[error("Account {0} not found")]
    AccountNotFound(AccountId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AccountResourceOverrideError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NotUserConfigurable(_)
            | Self::ExceedsPlanCeiling(_, _)
            | Self::BelowPlanDefault(_, _)
            | Self::FeatureDisabled(_)
            | Self::OwnerOnly
            | Self::AdminOnly
            | Self::GrantDoesNotIncreaseResolvedValue { .. }
            | Self::PromotionalExpiryRequired
            | Self::GrantValueOverflow
            | Self::GrantNotFound
            | Self::AccountNotFound(_) => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AccountResourceOverrideError, RepoError, AccountError);

impl From<AccountUsageError> for AccountResourceOverrideError {
    fn from(value: AccountUsageError) -> Self {
        match value {
            AccountUsageError::AccountNotfound(account_id) => Self::AccountNotFound(account_id),
            AccountUsageError::Unauthorized(error) => Self::Unauthorized(error),
            AccountUsageError::InternalError(error) => Self::InternalError(error),
            other => Self::InternalError(anyhow::Error::new(other)),
        }
    }
}

pub struct AccountResourceOverrideService {
    repo: Arc<dyn AccountResourceOverrideRepo>,
    account_service: Arc<AccountService>,
}

impl AccountResourceOverrideService {
    pub fn new(
        repo: Arc<dyn AccountResourceOverrideRepo>,
        account_service: Arc<AccountService>,
    ) -> Self {
        Self {
            repo,
            account_service,
        }
    }

    pub async fn set_max_disk_space_per_worker(
        &self,
        account_id: AccountId,
        value: u64,
        auth: &AuthCtx,
    ) -> Result<StorageLimit, AccountResourceOverrideError> {
        self.authorize_owner(account_id, auth).await?;
        self.set_user_override(
            account_id,
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            value,
            auth,
        )
        .await?;
        self.resolved_storage_limit(account_id).await
    }

    pub async fn get_max_disk_space_per_worker(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<StorageLimit, AccountResourceOverrideError> {
        self.authorize(account_id, auth, AccountUsageVerb::View)
            .await?;
        self.resolved_storage_limit(account_id).await
    }

    pub async fn clear_max_disk_space_per_worker(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<StorageLimit, AccountResourceOverrideError> {
        self.authorize_owner(account_id, auth).await?;
        let plan = self
            .account_service
            .get_plan_record(account_id, auth)
            .await?;
        if !plan.max_disk_space_per_worker_enabled {
            return Err(AccountResourceOverrideError::FeatureDisabled(
                "Maximum storage per agent",
            ));
        }
        if !plan.max_disk_space_per_worker_user_configurable {
            return Err(AccountResourceOverrideError::NotUserConfigurable(
                "Maximum storage per agent",
            ));
        }
        self.repo
            .delete(
                account_id.0,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            )
            .await?;
        self.resolved_storage_limit(account_id).await
    }

    pub async fn set_max_memory_per_worker(
        &self,
        account_id: AccountId,
        value: u64,
        auth: &AuthCtx,
    ) -> Result<MemoryLimit, AccountResourceOverrideError> {
        self.authorize_owner(account_id, auth).await?;
        self.set_user_override(
            account_id,
            AccountResourceOverrideDimension::MaxMemoryPerWorker,
            value,
            auth,
        )
        .await?;
        self.resolved_memory_limit(account_id).await
    }

    pub async fn get_max_memory_per_worker(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<MemoryLimit, AccountResourceOverrideError> {
        self.authorize(account_id, auth, AccountUsageVerb::View)
            .await?;
        self.resolved_memory_limit(account_id).await
    }

    pub async fn clear_max_memory_per_worker(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<MemoryLimit, AccountResourceOverrideError> {
        self.authorize_owner(account_id, auth).await?;
        let plan = self.account_service.get_plan(account_id, auth).await?;
        if !plan.max_memory_per_agent_user_configurable {
            return Err(AccountResourceOverrideError::NotUserConfigurable(
                "Maximum memory per agent",
            ));
        }
        self.repo
            .delete(
                account_id.0,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
            )
            .await?;
        self.resolved_memory_limit(account_id).await
    }

    async fn set_user_override(
        &self,
        account_id: AccountId,
        dimension: AccountResourceOverrideDimension,
        value: u64,
        auth: &AuthCtx,
    ) -> Result<OverridePolicy, AccountResourceOverrideError> {
        self.repo
            .set_user_override(account_id.0, dimension, value, auth.actor_account_id().0)
            .await
            .map_err(|error| map_set_error(account_id, dimension, error))
    }

    async fn resolved_memory_limit(
        &self,
        account_id: AccountId,
    ) -> Result<MemoryLimit, AccountResourceOverrideError> {
        let plan = self
            .account_service
            .get_plan_record(account_id, &AuthCtx::System)
            .await?;
        let override_value = self
            .repo
            .get_active_value(
                account_id.0,
                AccountResourceOverrideDimension::MaxMemoryPerWorker,
                &SqlDateTime::now(),
            )
            .await?
            .map(Into::into);
        let admin_grant = self
            .repo
            .get_active_admin_grants(account_id.0, &SqlDateTime::now())
            .await?
            .into_iter()
            .find(|grant| grant.dimension == AdminResourceGrantDimension::MaxMemoryPerAgent);
        let mut limit = MemoryLimit::resolve(
            plan.max_memory_per_worker.get(),
            override_value,
            plan.max_memory_per_worker_ceiling.get(),
            plan.max_memory_per_worker_user_configurable,
        );
        if let Some(grant) = admin_grant {
            limit.effective_value =
                golem_common::model::account_usage::ResourceLimitValue::from_memory_value(
                    grant.value,
                );
        }
        Ok(limit)
    }

    async fn resolved_storage_limit(
        &self,
        account_id: AccountId,
    ) -> Result<StorageLimit, AccountResourceOverrideError> {
        let plan = self
            .account_service
            .get_plan_record(account_id, &AuthCtx::System)
            .await?;
        let override_value = self
            .repo
            .get_active_value(
                account_id.0,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                &SqlDateTime::now(),
            )
            .await?
            .map(Into::into);
        let admin_grant = self
            .repo
            .get_active_admin_grants(account_id.0, &SqlDateTime::now())
            .await?
            .into_iter()
            .find(|grant| grant.dimension == AdminResourceGrantDimension::MaxStoragePerAgent);
        let mut limit = StorageLimit::resolve(
            plan.max_disk_space_per_worker_enabled,
            plan.max_disk_space_per_worker.get(),
            override_value,
            plan.max_disk_space_per_worker_ceiling.get(),
            plan.max_disk_space_per_worker_user_configurable,
        );
        if plan.max_disk_space_per_worker_enabled
            && let Some(grant) = admin_grant
        {
            limit.effective_value =
                golem_common::model::account_usage::StorageResourceLimitValue::from_storage_value(
                    grant.value,
                );
        }
        Ok(limit)
    }

    pub async fn set_admin_grant(
        &self,
        account_id: AccountId,
        dimension: AdminResourceGrantDimension,
        request: SetAdminResourceGrant,
        auth: &AuthCtx,
    ) -> Result<AdminResourceGrantChange, AccountResourceOverrideError> {
        ensure_admin(auth)?;
        self.repo
            .set_admin_grant(
                account_id.0,
                dimension.into(),
                request.value,
                request.reason,
                request.expires_at.map(SqlDateTime::new),
                auth.actor_account_id().0,
            )
            .await
            .map_err(|error| map_admin_grant_error(account_id, error))
    }

    pub async fn clear_admin_grant(
        &self,
        account_id: AccountId,
        dimension: AdminResourceGrantDimension,
        auth: &AuthCtx,
    ) -> Result<AdminResourceGrantChange, AccountResourceOverrideError> {
        ensure_admin(auth)?;
        self.repo
            .clear_admin_grant(account_id.0, dimension.into(), auth.actor_account_id().0)
            .await
            .map_err(|error| map_admin_grant_error(account_id, error))
    }

    async fn authorize(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
        verb: AccountUsageVerb,
    ) -> Result<(), AccountResourceOverrideError> {
        let account = self
            .account_service
            .get(account_id, auth)
            .await
            .map_err(map_account_error(account_id))
            .map_err(AccountResourceOverrideError::from)?;
        authorize_account_usage_permission(auth, &account.email, verb)?;
        Ok(())
    }

    async fn authorize_owner(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountResourceOverrideError> {
        self.authorize(account_id, auth, AccountUsageVerb::Update)
            .await?;
        ensure_account_owner(account_id, auth)
    }
}

fn map_set_error(
    account_id: AccountId,
    dimension: AccountResourceOverrideDimension,
    error: SetAccountResourceOverrideError,
) -> AccountResourceOverrideError {
    let label = match dimension {
        AccountResourceOverrideDimension::MaxDiskSpacePerWorker => "Maximum storage per agent",
        AccountResourceOverrideDimension::MaxMemoryPerWorker => "Maximum memory per agent",
        AccountResourceOverrideDimension::MonthlyComputeGcu
        | AccountResourceOverrideDimension::MonthlyMemoryGbSeconds
        | AccountResourceOverrideDimension::MonthlyDurableStorageGbMonth
        | AccountResourceOverrideDimension::MonthlyEphemeralStorageGbMonth => {
            "Monthly resource amount"
        }
    };
    match error {
        SetAccountResourceOverrideError::AccountNotFound(_) => {
            AccountResourceOverrideError::AccountNotFound(account_id)
        }
        SetAccountResourceOverrideError::Policy(OverridePolicyViolation::FeatureDisabled) => {
            AccountResourceOverrideError::FeatureDisabled(label)
        }
        SetAccountResourceOverrideError::Policy(OverridePolicyViolation::NotUserConfigurable) => {
            AccountResourceOverrideError::NotUserConfigurable(label)
        }
        SetAccountResourceOverrideError::Policy(OverridePolicyViolation::BelowPlanDefault(
            default,
        )) => AccountResourceOverrideError::BelowPlanDefault(label, default),
        SetAccountResourceOverrideError::Policy(OverridePolicyViolation::ExceedsPlanCeiling(
            ceiling,
        )) => AccountResourceOverrideError::ExceedsPlanCeiling(label, ceiling),
        SetAccountResourceOverrideError::Internal(error) => error.into(),
    }
}

fn map_admin_grant_error(
    account_id: AccountId,
    error: AdminResourceGrantRepoError,
) -> AccountResourceOverrideError {
    match error {
        AdminResourceGrantRepoError::AccountNotFound(_) => {
            AccountResourceOverrideError::AccountNotFound(account_id)
        }
        AdminResourceGrantRepoError::FeatureDisabled => {
            AccountResourceOverrideError::FeatureDisabled("Maximum storage per agent")
        }
        AdminResourceGrantRepoError::DoesNotIncreaseResolvedValue { value, current } => {
            AccountResourceOverrideError::GrantDoesNotIncreaseResolvedValue { value, current }
        }
        AdminResourceGrantRepoError::PromotionalExpiryRequired => {
            AccountResourceOverrideError::PromotionalExpiryRequired
        }
        AdminResourceGrantRepoError::ValueOverflow => {
            AccountResourceOverrideError::GrantValueOverflow
        }
        AdminResourceGrantRepoError::GrantNotFound => AccountResourceOverrideError::GrantNotFound,
        AdminResourceGrantRepoError::Internal(error) => error.into(),
    }
}

fn ensure_account_owner(
    account_id: AccountId,
    auth: &AuthCtx,
) -> Result<(), AccountResourceOverrideError> {
    match auth {
        AuthCtx::User(user) if user.account_id == account_id => Ok(()),
        _ => Err(AccountResourceOverrideError::OwnerOnly),
    }
}

fn ensure_admin(auth: &AuthCtx) -> Result<(), AccountResourceOverrideError> {
    match auth {
        AuthCtx::User(user) if user.account_roles.contains(&AccountRole::Admin) => Ok(()),
        _ => Err(AccountResourceOverrideError::AdminOnly),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::account::AccountEmail;
    use golem_common::model::card::EffectiveSurface;
    use golem_common::model::plan::PlanId;
    use golem_service_base::model::auth::UserAuthCtx;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn only_the_account_owner_can_mutate_per_agent_limits() {
        let owner_id = AccountId::new();
        let owner = AuthCtx::User(UserAuthCtx {
            account_id: owner_id,
            account_email: AccountEmail::new("owner@example.com"),
            account_plan_id: PlanId::new(),
            account_roles: BTreeSet::new(),
            effective_surface: EffectiveSurface::default(),
            delegation_surface: None,
        });

        assert!(ensure_account_owner(owner_id, &owner).is_ok());
        assert!(matches!(
            ensure_account_owner(AccountId::new(), &owner),
            Err(AccountResourceOverrideError::OwnerOnly)
        ));
        assert!(matches!(
            ensure_account_owner(owner_id, &AuthCtx::System),
            Err(AccountResourceOverrideError::OwnerOnly)
        ));
    }

    #[test]
    fn only_administrators_can_mutate_resource_grants() {
        let admin_id = AccountId::new();
        let target_id = AccountId::new();
        let admin = AuthCtx::User(UserAuthCtx {
            account_id: admin_id,
            account_email: AccountEmail::new("admin@example.com"),
            account_plan_id: PlanId::new(),
            account_roles: BTreeSet::from([AccountRole::Admin]),
            effective_surface: EffectiveSurface::default(),
            delegation_surface: None,
        });
        let user = AuthCtx::User(UserAuthCtx {
            account_id: target_id,
            account_email: AccountEmail::new("user@example.com"),
            account_plan_id: PlanId::new(),
            account_roles: BTreeSet::new(),
            effective_surface: EffectiveSurface::default(),
            delegation_surface: None,
        });
        let impersonation = AuthCtx::admin_impersonation(
            admin_id,
            target_id,
            AccountEmail::new("user@example.com"),
            BTreeSet::new(),
            PlanId::new(),
            EffectiveSurface::default(),
        );

        assert!(ensure_admin(&admin).is_ok());
        assert!(matches!(
            ensure_admin(&user),
            Err(AccountResourceOverrideError::AdminOnly)
        ));
        assert!(matches!(
            ensure_admin(&AuthCtx::System),
            Err(AccountResourceOverrideError::AdminOnly)
        ));
        assert!(matches!(
            ensure_admin(&impersonation),
            Err(AccountResourceOverrideError::AdminOnly)
        ));
    }

    #[test]
    fn resource_override_errors_have_safe_public_messages() {
        assert_eq!(
            AccountResourceOverrideError::FeatureDisabled("Maximum storage per agent")
                .to_safe_string(),
            "Maximum storage per agent is disabled because managed filesystem quotas are unavailable"
        );
        assert_eq!(
            AccountResourceOverrideError::OwnerOnly.to_safe_string(),
            "Only account owners may change per-agent resource limits"
        );
    }
}
