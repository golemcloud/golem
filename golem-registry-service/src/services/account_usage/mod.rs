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

pub mod error;

use self::error::LimitExceededError;
use crate::repo::account_usage::AccountUsageRepo;
use crate::repo::model::account_usage::{AccountUsage as RepoAccountUsage, UsageType};
use crate::repo::model::datetime::SqlDateTime;
use crate::services::account_usage::error::AccountUsageError;
use golem_common::model::account::AccountId;
use golem_service_base::model::auth::{AccountAction, AuthCtx};
use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
use std::collections::HashMap;
use std::sync::Arc;

pub struct AccountUsageService {
    account_usage_repo: Arc<dyn AccountUsageRepo>,
}

// TODO: do we want to add component max size limit?
//       if so, probably should be much bigger then the previous 50mb
impl AccountUsageService {
    pub fn new(account_usage_repo: Arc<dyn AccountUsageRepo>) -> Self {
        Self { account_usage_repo }
    }

    pub async fn ensure_application_within_limits(
        &self,
        account_id: AccountId,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalAppCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalAppCount, 1)?;

        Ok(())
    }

    pub async fn ensure_environment_within_limits(
        &self,
        account_id: AccountId,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalEnvCount))
            .await?;

        self.add_checked(&mut account_usage, UsageType::TotalEnvCount, 1)?;

        Ok(())
    }

    pub async fn ensure_new_component_within_limits(
        &self,
        account_id: AccountId,
        component_size_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self.get_account_usage(account_id, None).await?;

        self.add_checked(&mut account_usage, UsageType::TotalComponentCount, 1)?;

        if component_size_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(component_size_bytes));
        }

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes as i64,
        )?;

        Ok(())
    }

    pub async fn ensure_updated_component_within_limits(
        &self,
        account_id: AccountId,
        component_size_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalComponentStorageBytes))
            .await?;

        if component_size_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(component_size_bytes));
        }

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            component_size_bytes as i64,
        )?;

        Ok(())
    }

    pub async fn add_worker(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalWorkerCount))
            .await?;
        self.add_checked(&mut account_usage, UsageType::TotalWorkerCount, 1)?;
        self.account_usage_repo.add(&account_usage).await?;
        Ok(())
    }

    pub async fn remove_worker(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalWorkerCount))
            .await?;
        self.add_checked(&mut account_usage, UsageType::TotalWorkerCount, -1)?;
        self.account_usage_repo.add(&account_usage).await?;
        Ok(())
    }

    pub async fn add_worker_connection(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalWorkerConnectionCount))
            .await?;
        self.add_checked(&mut account_usage, UsageType::TotalWorkerConnectionCount, 1)?;
        self.account_usage_repo.add(&account_usage).await?;
        Ok(())
    }

    pub async fn remove_worker_connection(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalWorkerConnectionCount))
            .await?;
        self.add_checked(
            &mut account_usage,
            UsageType::TotalWorkerConnectionCount,
            -1,
        )?;
        self.account_usage_repo.add(&account_usage).await?;
        Ok(())
    }

    pub async fn record_fuel_consumption(
        &self,
        updates: HashMap<AccountId, i64>,
        auth: &AuthCtx,
    ) -> Result<AccountResourceLimits, AccountUsageError> {
        let mut limits_of_updates_accounts = HashMap::new();
        for (account_id, update) in updates {
            auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
            match self
                .get_account_usage(account_id, Some(UsageType::MonthlyGasLimit))
                .await
            {
                Ok(mut account_usage) => {
                    // fuel usage is allowed to exceeded the montly limit slightly.
                    // The worker executor will stop the worker at the next opportunity.
                    account_usage.add_change(UsageType::MonthlyGasLimit, update);

                    tracing::debug!(
                        "Updating monthly fuel consumption for account {account_id}: {update}"
                    );

                    self.account_usage_repo.add(&account_usage).await?;
                    limits_of_updates_accounts.insert(account_id, account_usage.resource_limits());
                }
                Err(AccountUsageError::AccountNotfound(_)) => {
                    // we received an update for a deleted account
                    // return an empty set of limits to fence the executor more quickly
                    limits_of_updates_accounts.insert(
                        account_id,
                        ResourceLimits {
                            available_fuel: 0,
                            max_memory_per_worker: 0,
                        },
                    );
                }
                Err(other) => return Err(other),
            };
        }
        Ok(AccountResourceLimits(limits_of_updates_accounts))
    }

    pub async fn get_resouce_limits(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<ResourceLimits, AccountUsageError> {
        auth.authorize_account_action(account_id, AccountAction::ViewUsage)?;

        let account_usage = self
            .get_account_usage(account_id, Some(UsageType::MonthlyGasLimit))
            .await?;

        Ok(account_usage.resource_limits())
    }

    async fn get_account_usage(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let usage = match usage_type {
            Some(usage_type) => {
                self.account_usage_repo
                    .get_for_type(account_id.0, &SqlDateTime::now(), usage_type)
                    .await?
            }
            None => {
                self.account_usage_repo
                    .get(account_id.0, &SqlDateTime::now())
                    .await?
            }
        };

        match usage {
            Some(usage) => Ok(usage),
            None => Err(AccountUsageError::AccountNotfound(account_id)),
        }
    }

    fn add_checked(
        &self,
        account_usage: &mut RepoAccountUsage,
        usage_type: UsageType,
        value: i64,
    ) -> Result<(), AccountUsageError> {
        if !account_usage.add_change(usage_type, value) {
            return Err(AccountUsageError::LimitExceeded(LimitExceededError {
                limit_name: format!("{usage_type:?}"),
                limit_value: account_usage.plan.limit(usage_type),
                current_value: account_usage.usage(usage_type),
            }));
        }

        Ok(())
    }
}
