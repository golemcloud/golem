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
use crate::repo::account_resource_override::AccountResourceOverrideRepo;
use crate::repo::model::account_resource_override::{
    AccountResourceOverrideDimension, AccountResourceOverrideReason, AccountResourceOverrideRecord,
};
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::StorageLimit;
use golem_common::model::auth::AccountRole;
use golem_common::model::card::AccountUsageVerb;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::{RepoError, SqlDateTime};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AccountResourceOverrideError {
    #[error("Storage limit is not user configurable")]
    NotUserConfigurable,
    #[error("Storage limit exceeds plan ceiling {0}")]
    ExceedsPlanCeiling(u64),
    #[error("Only admins may set an override expiry")]
    ExpiryRequiresAdmin,
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
            Self::NotUserConfigurable
            | Self::ExceedsPlanCeiling(_)
            | Self::ExpiryRequiresAdmin
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
        expires_at: Option<SqlDateTime>,
        auth: &AuthCtx,
    ) -> Result<StorageLimit, AccountResourceOverrideError> {
        self.authorize(account_id, auth, AccountUsageVerb::Update)
            .await?;
        if expires_at.is_some() && !can_set_expiry(auth) {
            return Err(AccountResourceOverrideError::ExpiryRequiresAdmin);
        }
        let plan = self.account_service.get_plan(account_id, auth).await?;

        if !plan.max_disk_space_per_worker_user_configurable {
            return Err(AccountResourceOverrideError::NotUserConfigurable);
        }
        if value > plan.max_disk_space_per_worker_ceiling {
            return Err(AccountResourceOverrideError::ExceedsPlanCeiling(
                plan.max_disk_space_per_worker_ceiling,
            ));
        }

        let created_at = SqlDateTime::now();
        self.repo
            .upsert(AccountResourceOverrideRecord {
                account_id: account_id.0,
                dimension: AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
                override_value: value.into(),
                reason: AccountResourceOverrideReason::UserSelfServe,
                expires_at,
                created_by: auth.actor_account_id().0,
                created_at,
            })
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
        self.authorize(account_id, auth, AccountUsageVerb::Update)
            .await?;
        self.repo
            .delete(
                account_id.0,
                AccountResourceOverrideDimension::MaxDiskSpacePerWorker,
            )
            .await?;
        self.resolved_storage_limit(account_id).await
    }

    async fn resolved_storage_limit(
        &self,
        account_id: AccountId,
    ) -> Result<StorageLimit, AccountResourceOverrideError> {
        let plan = self
            .account_service
            .get_plan(account_id, &AuthCtx::System)
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
        Ok(StorageLimit::resolve(
            plan.max_disk_space_per_worker,
            override_value,
            plan.max_disk_space_per_worker_ceiling,
            plan.max_disk_space_per_worker_user_configurable,
        ))
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
}

fn can_set_expiry(auth: &AuthCtx) -> bool {
    auth.is_system() || auth.account_roles().contains(&AccountRole::Admin)
}
