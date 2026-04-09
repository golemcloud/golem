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

pub mod error;

use self::error::LimitExceededError;
use crate::repo::account_usage::AccountUsageRepo;
use crate::repo::model::account_usage::{AccountUsage as RepoAccountUsage, UsageType};
use crate::services::account_usage::error::AccountUsageError;
use golem_common::model::account::AccountId;
use golem_service_base::model::auth::{AccountAction, AuthCtx};
use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
use golem_service_base::repo::SqlDateTime;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct ResourceUsageUpdate {
    pub fuel_delta: i64,
    pub http_call_count_delta: u64,
    pub rpc_call_count_delta: u64,
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
        new_component_size_bytes: u64,
        old_component_size_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        let mut account_usage = self
            .get_account_usage(account_id, Some(UsageType::TotalComponentStorageBytes))
            .await?;

        if new_component_size_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(
                new_component_size_bytes,
            ));
        }

        // Use the net delta so that replacing a large version with a smaller one
        // does not incorrectly count against the quota.
        let delta = new_component_size_bytes as i64 - old_component_size_bytes as i64;

        self.add_checked(
            &mut account_usage,
            UsageType::TotalComponentStorageBytes,
            delta,
        )?;

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

    pub async fn update_resource_usage(
        &self,
        updates: HashMap<AccountId, ResourceUsageUpdate>,
        auth: &AuthCtx,
    ) -> Result<AccountResourceLimits, AccountUsageError> {
        let mut limits_of_updates_accounts = HashMap::new();
        for (account_id, update) in updates {
            auth.authorize_account_action(account_id, AccountAction::UpdateUsage)?;
            match self.get_account_usage(account_id, None).await {
                Ok(mut account_usage) => {
                    // Usage can slightly exceed the monthly limit. The worker executor
                    // will suspend the worker at the next opportunity.
                    account_usage.add_change(UsageType::MonthlyGasLimit, update.fuel_delta);
                    account_usage.add_change(
                        UsageType::MonthlyHttpCalls,
                        i64::try_from(update.http_call_count_delta).unwrap_or(i64::MAX),
                    );
                    account_usage.add_change(
                        UsageType::MonthlyRpcCalls,
                        i64::try_from(update.rpc_call_count_delta).unwrap_or(i64::MAX),
                    );

                    tracing::debug!(
                        "Updating usage for account {account_id}: fuel_delta={}, http_call_count_delta={}, rpc_call_count_delta={}",
                        update.fuel_delta,
                        update.http_call_count_delta,
                        update.rpc_call_count_delta,
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
                            max_table_elements_per_worker: 0,
                            max_disk_space_per_worker: 0,
                            per_invocation_http_call_limit: 0,
                            per_invocation_rpc_call_limit: 0,
                            available_http_calls: 0,
                            available_rpc_calls: 0,
                            max_concurrent_agents_per_executor: 0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::model::account_usage::{AccountUsage, UsageType};
    use crate::repo::model::plan::PlanRecord;
    use golem_service_base::repo::NumericU64;
    use std::collections::BTreeMap;
    use test_r::test;
    use uuid::Uuid;

    test_r::enable!();

    /// Build a minimal `AccountUsage` with a given storage quota and current usage.
    fn make_usage(storage_limit: u64, current_storage_bytes: u64) -> AccountUsage {
        let plan = PlanRecord {
            plan_id: Uuid::new_v4(),
            name: "test".to_string(),
            max_memory_per_worker: NumericU64::new(u64::MAX),
            max_table_elements_per_worker: NumericU64::new(u64::MAX),
            max_disk_space_per_worker: NumericU64::new(u64::MAX),
            max_concurrent_agents_per_executor: NumericU64::new(u64::MAX),
            total_app_count: NumericU64::new(u64::MAX),
            total_env_count: NumericU64::new(u64::MAX),
            total_component_count: NumericU64::new(u64::MAX),
            total_worker_connection_count: NumericU64::new(u64::MAX),
            total_component_storage_bytes: NumericU64::new(storage_limit),
            monthly_gas_limit: NumericU64::new(u64::MAX),
            monthly_component_upload_limit_bytes: NumericU64::new(u64::MAX),
            per_invocation_http_call_limit: NumericU64::new(u64::MAX),
            per_invocation_rpc_call_limit: NumericU64::new(u64::MAX),
            monthly_http_call_limit: NumericU64::new(u64::MAX),
            monthly_rpc_call_limit: NumericU64::new(u64::MAX),
        };
        let mut usage = BTreeMap::new();
        usage.insert(UsageType::TotalComponentStorageBytes, current_storage_bytes);
        AccountUsage {
            account_id: Uuid::new_v4(),
            year: 2026,
            month: 1,
            usage,
            plan,
            changes: BTreeMap::new(),
        }
    }

    /// Simulates `ensure_updated_component_within_limits` inline so we can test
    /// the delta logic without needing a database-backed `AccountUsageRepo`.
    fn check_update(
        usage: &mut AccountUsage,
        new_bytes: u64,
        old_bytes: u64,
    ) -> Result<(), AccountUsageError> {
        if new_bytes > i64::MAX as u64 {
            return Err(AccountUsageError::ComponentTooLarge(new_bytes));
        }
        let delta = new_bytes as i64 - old_bytes as i64;
        if !usage.add_change(UsageType::TotalComponentStorageBytes, delta) {
            return Err(AccountUsageError::LimitExceeded(LimitExceededError {
                limit_name: "TotalComponentStorageBytes".to_string(),
                limit_value: usage.plan.limit(UsageType::TotalComponentStorageBytes),
                current_value: usage.usage(UsageType::TotalComponentStorageBytes),
            }));
        }
        Ok(())
    }

    #[test]
    fn update_with_smaller_version_is_allowed_near_quota() {
        // Quota: 1000 bytes. Current usage: 900 bytes (from existing component of 900 bytes).
        // Updating to a 500-byte version → net delta = 500 - 900 = -400 → should be allowed.
        let mut usage = make_usage(1000, 900);
        let result = check_update(&mut usage, 500, 900);
        assert!(
            result.is_ok(),
            "replacing 900-byte component with 500-byte version should be allowed near quota"
        );
        // Final projected usage = 900 + (-400) = 500
        assert_eq!(
            usage.final_value(UsageType::TotalComponentStorageBytes),
            500
        );
    }

    #[test]
    fn update_that_exceeds_quota_is_rejected() {
        // Quota: 1000 bytes. Current usage: 900 bytes (from existing 400-byte component).
        // Updating to an 800-byte version → net delta = 800 - 400 = +400 → 900 + 400 = 1300 > 1000.
        let mut usage = make_usage(1000, 900);
        let result = check_update(&mut usage, 800, 400);
        assert!(
            result.is_err(),
            "update that pushes total over quota must be rejected"
        );
    }

    #[test]
    fn update_to_same_size_is_allowed() {
        // Quota: 1000 bytes. Current usage: 900 bytes.
        // Updating to a component of the same size → net delta = 0 → always allowed.
        let mut usage = make_usage(1000, 900);
        let result = check_update(&mut usage, 500, 500);
        assert!(result.is_ok(), "same-size update must always be allowed");
        assert_eq!(
            usage.final_value(UsageType::TotalComponentStorageBytes),
            900
        );
    }

    #[test]
    fn update_within_quota_is_allowed() {
        // Quota: 1000 bytes. Current usage: 400 bytes (from existing 200-byte component).
        // Updating to a 400-byte version → net delta = 400 - 200 = +200 → 400 + 200 = 600 ≤ 1000.
        let mut usage = make_usage(1000, 400);
        let result = check_update(&mut usage, 400, 200);
        assert!(result.is_ok(), "update within quota must be allowed");
        assert_eq!(
            usage.final_value(UsageType::TotalComponentStorageBytes),
            600
        );
    }
}
