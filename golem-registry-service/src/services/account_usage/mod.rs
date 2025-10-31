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

use crate::repo::account_usage::AccountUsageRepo;
use crate::repo::model::account_usage::{AccountUsage as RepoAccountUsage, UsageType};
use crate::repo::model::datetime::SqlDateTime;
use crate::services::account_usage::error::AccountUsageError;
use golem_common::model::account::AccountId;
use std::sync::Arc;
use tracing::error;
use golem_service_base::model::ResourceLimits;
use golem_service_base::model::auth::{AccountAction, AuthCtx};

pub mod error;

pub struct AccountUsage {
    account_usage: RepoAccountUsage,
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    acked: bool,
}

impl AccountUsage {
    pub fn ack(&mut self) {
        self.acked = true;
    }
}

impl Drop for AccountUsage {
    fn drop(&mut self) {
        if self.acked {
            return;
        }

        let account_usage = self.account_usage.clone();
        let account_usage_repo = self.account_usage_repo.clone();
        tokio::spawn(async move {
            if let Err(err) = account_usage_repo.rollback(&account_usage).await {
                error!("Failed to rollback account usage: {account_usage:?}, {err}");
            }
        });
    }
}

pub struct AccountUsageService {
    account_usage_repo: Arc<dyn AccountUsageRepo>,
}

// TODO: do we want to add component max size limit?
//       if so, probably should be much bigger then the previous 50mb
impl AccountUsageService {
    pub fn new(account_usage_repo: Arc<dyn AccountUsageRepo>) -> Self {
        Self { account_usage_repo }
    }

    pub async fn add_application(
        &self,
        account_id: &AccountId,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalAppCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalAppCount, 1)?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_environment(
        &self,
        account_id: &AccountId,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalEnvCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalEnvCount, 1)?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_component(
        &self,
        account_id: &AccountId,
        component_size_bytes: i64,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self.get_account_usage(account_id, None).await?;

        self.add_checked(&mut account_usage, UsageType::TotalAppCount, 1)?;
        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes,
        )?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_component_version(
        &self,
        account_id: &AccountId,
        component_size_bytes: i64,
    ) -> Result<AccountUsage, AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalComponentStorageBytes))
            .await?;

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes,
        )?;

        Ok(self.wrapped_account_usage(account_usage))
    }

    pub async fn add_worker(
        &self,
        account_id: &AccountId,
        auth: &AuthCtx
    ) -> Result<(), AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
        let mut account_usage = self.get_account_usage(account_id, Some(UsageType::TotalWorkerCount)).await?;
        self.add_checked(&mut account_usage, UsageType::TotalWorkerCount, 1)?;
        Ok(())
    }

    pub async fn remove_worker(
        &self,
        account_id: &AccountId,
        auth: &AuthCtx
    ) -> Result<(), AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
        let mut account_usage = self.get_account_usage(account_id, Some(UsageType::TotalWorkerCount)).await?;
        self.add_checked(&mut account_usage, UsageType::TotalWorkerCount, -1)?;
        Ok(())
    }

    pub async fn get_account_usage(
        &self,
        account_id: &AccountId,
        usage_type: Option<UsageType>,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let usage = match usage_type {
            Some(usage_type) => {
                self.account_usage_repo
                    .get_for_type(&account_id.0, &SqlDateTime::now(), usage_type)
                    .await?
            }
            None => {
                self.account_usage_repo
                    .get(&account_id.0, &SqlDateTime::now())
                    .await?
            }
        };

        match usage {
            Some(usage) => Ok(usage),
            None => Err(AccountUsageError::AccountNotfound(account_id.clone())),
        }
    }

    pub async fn get_resouce_limits(&self, account_id: &AccountId, auth: &AuthCtx) -> Result<ResourceLimits, AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::ViewUsage)?;

        let record = self.get_account_usage(account_id, Some(UsageType::MonthlyGasLimit)).await?;

        let available_fuel = record
            .plan
            .monthly_gas_limit
            .checked_sub(*record.usage.get(&UsageType::MonthlyGasLimit).unwrap_or(&0))
            .unwrap_or(0);

        Ok(ResourceLimits { available_fuel: available_fuel, max_memory_per_worker: record.plan.max_memory_per_worker as u64 })
    }

    fn add_checked(
        &self,
        account_usage: &mut RepoAccountUsage,
        usage_type: UsageType,
        value: i64,
    ) -> Result<(), AccountUsageError> {
        if !account_usage.add_checked(usage_type, value)? {
            return Err(AccountUsageError::LimitExceeded {
                limit_name: format!("{usage_type:?}"),
                limit_value: account_usage.plan.limit(usage_type),
                current_value: account_usage.usage(usage_type),
            });
        }

        Ok(())
    }

    fn wrapped_account_usage(&self, account_usage: RepoAccountUsage) -> AccountUsage {
        AccountUsage {
            account_usage,
            account_usage_repo: self.account_usage_repo.clone(),
            acked: false,
        }
    }
}
