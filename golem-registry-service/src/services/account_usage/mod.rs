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
use super::account::{AccountError, AccountService};
use crate::repo::account_usage::AccountUsageRepo;
use crate::repo::model::account_usage::{
    AccountUsage as RepoAccountUsage, AccountUsageRecord, UsageType,
};
use crate::services::account_usage::error::AccountUsageError;
use chrono::{TimeZone, Utc};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::account_usage::{
    AccountResourcePolicy, AccountUsage, AccountUsageMetering, AccountUsageMetrics,
    AccountUsagePeriod, MeteringStatus, MonthlyComputeLimit, MonthlyComputeUnit,
    MonthlyLimitBehavior, MonthlyMemoryLimit, MonthlyMemoryUnit, MonthlyPlanAmounts,
    MonthlyResourceLimits, MonthlyStorageLimit, MonthlyStorageUnit, byte_seconds_to_gb_month,
    fuel_to_gcu,
};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    AccountUsageResourcePattern, AccountUsageVerb, ClassPermissionTarget, PermissionTarget,
};
use golem_service_base::clients::registry::ResourceUsageMetering;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::model::{AccountResourceLimits, ResourceLimits};
use golem_service_base::repo::SqlDateTime;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct ResourceUsageUpdate {
    pub fuel_delta: i64,
    pub http_call_count_delta: u64,
    pub rpc_call_count_delta: u64,
    pub durable_storage_byte_seconds_delta: i64,
    pub ephemeral_storage_byte_seconds_delta: i64,
    pub memory_gb_seconds_delta: i64,
    pub metering: ResourceUsageMetering,
}

pub struct AccountUsageService {
    account_usage_repo: Arc<dyn AccountUsageRepo>,
    account_service: Arc<AccountService>,
}

// TODO: do we want to add component max size limit?
//       if so, probably should be much bigger then the previous 50mb
impl AccountUsageService {
    pub fn new(
        account_usage_repo: Arc<dyn AccountUsageRepo>,
        account_service: Arc<AccountService>,
    ) -> Self {
        Self {
            account_usage_repo,
            account_service,
        }
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
        auth.authorize_system_only("update account usage")?;

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
        auth.authorize_system_only("update account usage")?;

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
        auth.authorize_system_only("update account usage")?;

        let mut limits_of_updated_accounts = HashMap::new();
        for (account_id, update) in updates {
            match self
                .get_account_usage(account_id, Some(UsageType::MonthlyGasLimit))
                .await
            {
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
                    account_usage.add_change(
                        UsageType::MonthlyDurableAgentStorageByteSeconds,
                        update.durable_storage_byte_seconds_delta,
                    );
                    account_usage.add_change(
                        UsageType::MonthlyEphemeralStorageByteSeconds,
                        update.ephemeral_storage_byte_seconds_delta,
                    );
                    account_usage.add_change(
                        UsageType::MonthlyMemoryGbSeconds,
                        update.memory_gb_seconds_delta,
                    );
                    account_usage.metering = Some(update.metering);

                    tracing::debug!(
                        %account_id,
                        fuel_delta = update.fuel_delta,
                        memory_gb_seconds_delta = update.memory_gb_seconds_delta,
                        durable_storage_byte_seconds_delta = update.durable_storage_byte_seconds_delta,
                        ephemeral_storage_byte_seconds_delta = update.ephemeral_storage_byte_seconds_delta,
                        http_call_count_delta = update.http_call_count_delta,
                        rpc_call_count_delta = update.rpc_call_count_delta,
                        "Updating account resource usage"
                    );

                    match self.account_usage_repo.add(&account_usage).await {
                        Ok(()) => {
                            limits_of_updated_accounts
                                .insert(account_id, account_usage.resource_limits());
                        }
                        Err(error) => {
                            tracing::error!(
                                %account_id,
                                %error,
                                "Failed to apply resource usage update"
                            );
                        }
                    }
                }
                Err(AccountUsageError::AccountNotfound(_)) => {
                    // We received an update for a deleted account. Return an empty
                    // set of limits to fence the executor more quickly.
                    limits_of_updated_accounts.insert(
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
                            oplog_writes_per_second: 0,
                            usage_update_applied: false,
                        },
                    );
                }
                Err(error) => {
                    tracing::error!(
                        %account_id,
                        %error,
                        "Failed to load account usage for resource update"
                    );
                }
            }
        }

        Ok(AccountResourceLimits(limits_of_updated_accounts))
    }

    pub async fn get_resouce_limits(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<ResourceLimits, AccountUsageError> {
        let account = self
            .account_service
            .get(account_id, auth)
            .await
            .map_err(map_account_error(account_id))?;

        authorize_account_usage_permission(auth, &account.email, AccountUsageVerb::View)?;

        let account_usage = self
            .get_account_usage(account_id, Some(UsageType::MonthlyGasLimit))
            .await?;

        Ok(account_usage.resource_limits())
    }

    pub async fn get_usage(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<AccountUsage, AccountUsageError> {
        self.get_usage_for_period(account_id, AccountUsagePeriod::current(), auth)
            .await
    }

    pub async fn get_resource_policy(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<AccountResourcePolicy, AccountUsageError> {
        self.authorize_usage(account_id, auth).await?;
        let account_usage = self.get_account_usage(account_id, None).await?;
        let report = self
            .account_usage_repo
            .get_usage_report(account_id.0, AccountUsagePeriod::current())
            .await?;

        Self::resource_policy(account_id, account_usage, report)
    }

    pub async fn get_usage_for_period(
        &self,
        account_id: AccountId,
        period: AccountUsagePeriod,
        auth: &AuthCtx,
    ) -> Result<AccountUsage, AccountUsageError> {
        self.authorize_usage(account_id, auth).await?;
        let report = self
            .account_usage_repo
            .get_usage_report(account_id.0, period)
            .await?;
        Ok(Self::usage(account_id, report))
    }

    pub async fn get_usage_history(
        &self,
        account_id: AccountId,
        last: usize,
        auth: &AuthCtx,
    ) -> Result<Vec<AccountUsage>, AccountUsageError> {
        let current_period = AccountUsagePeriod::current();
        self.authorize_usage(account_id, auth).await?;
        let history = self
            .account_usage_repo
            .get_usage_history(account_id.0, current_period, last)
            .await?;

        Ok(history
            .into_iter()
            .map(|report| Self::usage(account_id, report))
            .collect())
    }

    async fn authorize_usage(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<(), AccountUsageError> {
        let account = self
            .account_service
            .get(account_id, auth)
            .await
            .map_err(map_account_error(account_id))?;
        authorize_account_usage_permission(auth, &account.email, AccountUsageVerb::View)?;
        Ok(())
    }

    fn usage(account_id: AccountId, report: AccountUsageRecord) -> AccountUsage {
        let metering = report.metering.map_or_else(
            || AccountUsageMetering {
                compute: MeteringStatus::Unknown,
                memory: MeteringStatus::Unknown,
                durable_storage: MeteringStatus::Unknown,
                ephemeral_storage: MeteringStatus::Unknown,
            },
            |metering| {
                let filesystem = metering_status(metering.filesystem);
                AccountUsageMetering {
                    compute: metering_status(metering.compute),
                    memory: metering_status(metering.memory),
                    durable_storage: filesystem,
                    ephemeral_storage: filesystem,
                }
            },
        );
        AccountUsage {
            account_id,
            usage: AccountUsageMetrics {
                period: report.period,
                as_of: report.as_of.unwrap_or_else(Utc::now),
                compute_gcu: fuel_to_gcu(report.compute_fuel),
                memory_gb_seconds: report.memory_gb_seconds,
                durable_storage_gb_month: byte_seconds_to_gb_month(
                    report.durable_storage_byte_seconds,
                ),
                ephemeral_storage_gb_month: byte_seconds_to_gb_month(
                    report.ephemeral_storage_byte_seconds,
                ),
                metering,
            },
        }
    }

    fn resource_policy(
        account_id: AccountId,
        account_usage: RepoAccountUsage,
        report: AccountUsageRecord,
    ) -> Result<AccountResourcePolicy, AccountUsageError> {
        let plan_amounts = MonthlyPlanAmounts {
            compute_gcu: account_usage.plan.monthly_compute_gcu.get(),
            memory_gb_seconds: account_usage.plan.monthly_memory_gb_seconds.get(),
            durable_storage_gb_month: account_usage.plan.monthly_durable_storage_gb_month.get(),
            ephemeral_storage_gb_month: account_usage.plan.monthly_ephemeral_storage_gb_month.get(),
        };
        let resolved = plan_amounts
            .resolve()
            .map_err(|error| AccountUsageError::InternalError(error.into()))?;
        let metering = report.metering.map_or_else(
            || AccountUsageMetering {
                compute: MeteringStatus::Unknown,
                memory: MeteringStatus::Unknown,
                durable_storage: MeteringStatus::Unknown,
                ephemeral_storage: MeteringStatus::Unknown,
            },
            |metering| {
                let filesystem = metering_status(metering.filesystem);
                AccountUsageMetering {
                    compute: metering_status(metering.compute),
                    memory: metering_status(metering.memory),
                    durable_storage: filesystem,
                    ephemeral_storage: filesystem,
                }
            },
        );

        Ok(AccountResourcePolicy {
            account_id,
            monthly: MonthlyResourceLimits {
                compute_gcu: compute_limit(
                    metering.compute,
                    plan_amounts.compute_gcu,
                    resolved.compute_fuel,
                    report.compute_fuel,
                ),
                memory_gb_seconds: memory_limit(
                    metering.memory,
                    plan_amounts.memory_gb_seconds,
                    report.memory_gb_seconds,
                ),
                durable_storage_gb_month: storage_limit_policy(
                    metering.durable_storage,
                    plan_amounts.durable_storage_gb_month,
                    resolved.durable_storage_byte_seconds,
                    report.durable_storage_byte_seconds,
                ),
                ephemeral_storage_gb_month: storage_limit_policy(
                    metering.ephemeral_storage,
                    plan_amounts.ephemeral_storage_gb_month,
                    resolved.ephemeral_storage_byte_seconds,
                    report.ephemeral_storage_byte_seconds,
                ),
            },
            max_memory_per_agent: account_usage.max_memory_per_worker,
            max_storage_per_agent: account_usage.storage_limit,
        })
    }

    async fn get_account_usage(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        self.get_account_usage_at(account_id, usage_type, AccountUsagePeriod::current())
            .await
    }

    async fn get_account_usage_at(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
        period: AccountUsagePeriod,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let date = SqlDateTime::new(
            Utc.with_ymd_and_hms(period.year, period.month, 1, 0, 0, 0)
                .single()
                .expect("validated account usage period"),
        );
        let usage = match usage_type {
            Some(usage_type) => {
                self.account_usage_repo
                    .get_for_type(account_id.0, &date, usage_type)
                    .await?
            }
            None => self.account_usage_repo.get(account_id.0, &date).await?,
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

fn compute_limit(
    metering: MeteringStatus,
    monthly_amount: u64,
    amount_fuel: u64,
    usage_fuel: u64,
) -> MonthlyComputeLimit {
    match metering {
        MeteringStatus::Enabled => MonthlyComputeLimit {
            metering,
            monthly_amount: Some(monthly_amount),
            usage: Some(fuel_to_gcu(usage_fuel)),
            remaining: Some(fuel_to_gcu(amount_fuel.saturating_sub(usage_fuel))),
            unit: MonthlyComputeUnit::Gcu,
            behavior: Some(MonthlyLimitBehavior::HardLimit),
        },
        MeteringStatus::Disabled => MonthlyComputeLimit {
            metering,
            monthly_amount: None,
            usage: None,
            remaining: None,
            unit: MonthlyComputeUnit::Gcu,
            behavior: None,
        },
        MeteringStatus::Unknown => MonthlyComputeLimit {
            metering,
            monthly_amount: Some(monthly_amount),
            usage: None,
            remaining: None,
            unit: MonthlyComputeUnit::Gcu,
            behavior: None,
        },
    }
}

fn memory_limit(metering: MeteringStatus, monthly_amount: u64, usage: u64) -> MonthlyMemoryLimit {
    match metering {
        MeteringStatus::Enabled => MonthlyMemoryLimit {
            metering,
            monthly_amount: Some(monthly_amount),
            usage: Some(usage),
            remaining: Some(monthly_amount.saturating_sub(usage)),
            unit: MonthlyMemoryUnit::GbSeconds,
            behavior: Some(MonthlyLimitBehavior::HardLimit),
        },
        MeteringStatus::Disabled => MonthlyMemoryLimit {
            metering,
            monthly_amount: None,
            usage: None,
            remaining: None,
            unit: MonthlyMemoryUnit::GbSeconds,
            behavior: None,
        },
        MeteringStatus::Unknown => MonthlyMemoryLimit {
            metering,
            monthly_amount: Some(monthly_amount),
            usage: None,
            remaining: None,
            unit: MonthlyMemoryUnit::GbSeconds,
            behavior: None,
        },
    }
}

fn storage_limit_policy(
    metering: MeteringStatus,
    monthly_amount: u64,
    amount_byte_seconds: u64,
    usage_byte_seconds: u64,
) -> MonthlyStorageLimit {
    match metering {
        MeteringStatus::Enabled => MonthlyStorageLimit {
            metering,
            monthly_amount: Some(monthly_amount),
            usage: Some(byte_seconds_to_gb_month(usage_byte_seconds)),
            remaining: Some(byte_seconds_to_gb_month(
                amount_byte_seconds.saturating_sub(usage_byte_seconds),
            )),
            unit: MonthlyStorageUnit::GbMonth,
            behavior: Some(MonthlyLimitBehavior::HardLimit),
        },
        MeteringStatus::Disabled => MonthlyStorageLimit {
            metering,
            monthly_amount: None,
            usage: None,
            remaining: None,
            unit: MonthlyStorageUnit::GbMonth,
            behavior: None,
        },
        MeteringStatus::Unknown => MonthlyStorageLimit {
            metering,
            monthly_amount: Some(monthly_amount),
            usage: None,
            remaining: None,
            unit: MonthlyStorageUnit::GbMonth,
            behavior: None,
        },
    }
}

fn metering_status(enabled: bool) -> MeteringStatus {
    if enabled {
        MeteringStatus::Enabled
    } else {
        MeteringStatus::Disabled
    }
}

pub(crate) fn map_account_error(
    account_id: AccountId,
) -> impl FnOnce(AccountError) -> AccountUsageError {
    move |err| match err {
        AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
            AccountUsageError::AccountNotfound(account_id)
        }
        other => AccountUsageError::InternalError(other.into()),
    }
}

pub(crate) fn authorize_account_usage_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    verb: AccountUsageVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&account_usage_permission_target(account_email, verb))
}

fn account_usage_permission_target(
    account_email: &AccountEmail,
    verb: AccountUsageVerb,
) -> PermissionTarget {
    PermissionTarget::AccountUsage(ClassPermissionTarget {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: account_email.clone(),
        },
        resource: AccountUsageResourcePattern,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::model::account_usage::{AccountUsage, UsageType};
    use crate::repo::model::plan::PlanRecord;
    use golem_common::model::account_usage::{
        BYTE_SECONDS_PER_GB_MONTH, FUEL_PER_GCU, MemoryLimit, StorageLimit,
    };
    use golem_service_base::repo::NumericU64;
    use std::collections::BTreeMap;
    use test_r::test;
    use uuid::Uuid;

    /// Build a minimal `AccountUsage` with a given storage quota and current usage.
    fn make_usage(storage_limit: u64, current_storage_bytes: u64) -> AccountUsage {
        let plan = PlanRecord {
            plan_id: Uuid::new_v4(),
            name: "test".to_string(),
            max_memory_per_worker: NumericU64::new(u64::MAX),
            max_memory_per_worker_ceiling: NumericU64::new(u64::MAX),
            max_memory_per_worker_user_configurable: false,
            monthly_compute_gcu: NumericU64::new(0),
            monthly_memory_gb_seconds: NumericU64::new(u64::MAX),
            monthly_durable_storage_gb_month: NumericU64::new(0),
            monthly_ephemeral_storage_gb_month: NumericU64::new(0),
            max_table_elements_per_worker: NumericU64::new(u64::MAX),
            max_disk_space_per_worker_enabled: false,
            max_disk_space_per_worker: NumericU64::new(u64::MAX),
            max_disk_space_per_worker_ceiling: NumericU64::new(u64::MAX),
            max_disk_space_per_worker_user_configurable: false,
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
            oplog_writes_per_second: NumericU64::new(u64::MAX),
        };
        let mut usage = BTreeMap::new();
        usage.insert(UsageType::TotalComponentStorageBytes, current_storage_bytes);
        AccountUsage {
            account_id: Uuid::new_v4(),
            year: 2026,
            month: 1,
            usage,
            plan,
            storage_limit: StorageLimit {
                enabled: false,
                effective_value: None,
                plan_default: None,
                override_value: None,
                ceiling: None,
                user_configurable: false,
                disabled_reason: Some(
                    golem_common::model::account_usage::StorageLimitDisabledReason::ManagedFilesystemUnavailable,
                ),
            },
            max_memory_per_worker: golem_common::model::account_usage::MemoryLimit {
                effective_value: u64::MAX,
                plan_default: u64::MAX,
                override_value: None,
                ceiling: u64::MAX,
                user_configurable: false,
            },
            metering: None,
            changes: BTreeMap::new(),
        }
    }

    fn make_policy_usage() -> AccountUsage {
        let mut usage = make_usage(u64::MAX, 0);
        usage.plan.monthly_compute_gcu = NumericU64::new(5);
        usage.plan.monthly_memory_gb_seconds = NumericU64::new(50);
        usage.plan.monthly_durable_storage_gb_month = NumericU64::new(7);
        usage.plan.monthly_ephemeral_storage_gb_month = NumericU64::new(11);
        usage.storage_limit = StorageLimit::resolve(true, 5, Some(12), 20, true);
        usage.max_memory_per_worker = MemoryLimit::resolve(100, Some(150), 200, true);
        usage
    }

    fn make_policy_report(metering: Option<ResourceUsageMetering>) -> AccountUsageRecord {
        AccountUsageRecord {
            period: AccountUsagePeriod {
                year: 2026,
                month: 1,
            },
            as_of: None,
            compute_fuel: FUEL_PER_GCU + FUEL_PER_GCU / 2,
            memory_gb_seconds: 60,
            durable_storage_byte_seconds: 8 * BYTE_SECONDS_PER_GB_MONTH,
            ephemeral_storage_byte_seconds: 4 * BYTE_SECONDS_PER_GB_MONTH,
            metering,
        }
    }

    #[test]
    fn resource_policy_resolves_enabled_dimensions_in_internal_units() {
        let usage = make_policy_usage();
        let expected_storage_limit = usage.storage_limit.clone();
        let expected_memory_limit = usage.max_memory_per_worker.clone();
        let account_id = AccountId(usage.account_id);
        let policy = AccountUsageService::resource_policy(
            account_id,
            usage,
            make_policy_report(Some(ResourceUsageMetering::all_enabled())),
        )
        .unwrap();

        assert_eq!(policy.account_id, account_id);
        assert_eq!(policy.monthly.compute_gcu.monthly_amount, Some(5));
        assert_eq!(policy.monthly.compute_gcu.usage, Some(1.5));
        assert_eq!(policy.monthly.compute_gcu.remaining, Some(3.5));
        assert_eq!(
            policy.monthly.compute_gcu.behavior,
            Some(MonthlyLimitBehavior::HardLimit)
        );
        assert_eq!(policy.monthly.memory_gb_seconds.monthly_amount, Some(50));
        assert_eq!(policy.monthly.memory_gb_seconds.usage, Some(60));
        assert_eq!(policy.monthly.memory_gb_seconds.remaining, Some(0));
        assert_eq!(
            policy.monthly.durable_storage_gb_month.monthly_amount,
            Some(7)
        );
        assert_eq!(policy.monthly.durable_storage_gb_month.usage, Some(8.0));
        assert_eq!(policy.monthly.durable_storage_gb_month.remaining, Some(0.0));
        assert_eq!(
            policy.monthly.ephemeral_storage_gb_month.monthly_amount,
            Some(11)
        );
        assert_eq!(policy.monthly.ephemeral_storage_gb_month.usage, Some(4.0));
        assert_eq!(
            policy.monthly.ephemeral_storage_gb_month.remaining,
            Some(7.0)
        );
        assert_eq!(policy.max_storage_per_agent, expected_storage_limit);
        assert_eq!(policy.max_memory_per_agent, expected_memory_limit);
    }

    #[test]
    fn resource_policy_omits_disabled_dimension_values() {
        let usage = make_policy_usage();
        let account_id = AccountId(usage.account_id);
        let policy = AccountUsageService::resource_policy(
            account_id,
            usage,
            make_policy_report(Some(ResourceUsageMetering::default())),
        )
        .unwrap();
        let value = serde_json::to_value(policy).unwrap();

        for dimension in [
            "computeGcu",
            "memoryGbSeconds",
            "durableStorageGbMonth",
            "ephemeralStorageGbMonth",
        ] {
            let dimension = &value["monthly"][dimension];
            assert_eq!(dimension["metering"], "disabled");
            assert!(dimension.get("monthlyAmount").is_none());
            assert!(dimension.get("usage").is_none());
            assert!(dimension.get("remaining").is_none());
            assert!(dimension.get("behavior").is_none());
        }
    }

    #[test]
    fn resource_policy_exposes_only_amount_when_metering_is_unknown() {
        let usage = make_policy_usage();
        let account_id = AccountId(usage.account_id);
        let policy =
            AccountUsageService::resource_policy(account_id, usage, make_policy_report(None))
                .unwrap();
        let value = serde_json::to_value(policy).unwrap();

        for (dimension, monthly_amount) in [
            ("computeGcu", 5),
            ("memoryGbSeconds", 50),
            ("durableStorageGbMonth", 7),
            ("ephemeralStorageGbMonth", 11),
        ] {
            let dimension = &value["monthly"][dimension];
            assert_eq!(dimension["metering"], "unknown");
            assert_eq!(dimension["monthlyAmount"], monthly_amount);
            assert!(dimension.get("usage").is_none());
            assert!(dimension.get("remaining").is_none());
            assert!(dimension.get("behavior").is_none());
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
