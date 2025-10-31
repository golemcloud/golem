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
use crate::repo::account::AccountRepo;
use crate::repo::model::account::{AccountRepoError, AccountRevisionRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use anyhow::anyhow;
use golem_common::model::account::PlanId;
use golem_common::model::account::{Account, AccountCreation, AccountId, AccountUpdate};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AccountAction, GlobalAction};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{error, info};
use super::account::{AccountError, AccountService};
use super::account_usage::error::AccountUsageError;
use super::account_usage::AccountUsageService;
use golem_service_base::model::ResourceLimits;
use crate::repo::model::account_usage::UsageType;

#[derive(Debug, thiserror::Error)]
pub enum ResourceLimitsError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ResourceLimitsError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AccountNotFound(_) => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(ResourceLimitsError, AccountError, PlanError, AccountUsageError);

pub struct ResourceLimitsService {
    account_service: Arc<AccountService>,
    plan_service: Arc<PlanService>,
    account_usage_service: Arc<AccountUsageService>
}

impl ResourceLimitsService {
    pub fn new(
        account_service: Arc<AccountService>,
        plan_service: Arc<PlanService>,
        account_usage_service: Arc<AccountUsageService>
    ) -> Self {
        Self {
            account_service,
            plan_service,
            account_usage_service,
        }
    }

    pub async fn get_resouce_limits(&self, account_id: &AccountId, auth: &AuthCtx) -> Result<ResourceLimits, ResourceLimitsError> {
        // TODO: this is called quite often and should be optimized.
        let account = self.account_service.get(&account_id, auth).await.map_err(|e| match e {
            AccountError::AccountNotFound(account_id) => ResourceLimitsError::AccountNotFound(account_id),
            other => other.into()
        })?;

        let plan = self.plan_service.get(&account.plan_id, auth).await?;
        let account_usage = self.account_usage_service.get_account_usage(account_id, Some(UsageType::MonthlyGasLimit)).await?;

        let available_fuel = plan
            .monthly_gas_limit
            .checked_sub(*account_usage.usage.get(&UsageType::MonthlyGasLimit).unwrap_or(&0))
            .unwrap_or(0);

        Ok(ResourceLimits { available_fuel: available_fuel, max_memory_per_worker: plan. })
    }
}
