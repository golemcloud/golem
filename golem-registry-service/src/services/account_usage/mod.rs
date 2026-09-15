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
use crate::repo::account_usage::SetMonthlyUsageModeError;
use crate::repo::model::account_usage::{
    AccountMonthlyUsageMode, AccountUsage as RepoAccountUsage, AccountUsageRecord,
    MonthlyUsageAttribution, UsageType,
};
use crate::services::account_usage::error::AccountUsageError;
use chrono::{TimeZone, Utc};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::account_usage::{
    AccountResourcePolicy, AccountUsage, AccountUsageMetering, AccountUsageMetrics,
    AccountUsagePeriod, AdminResourceGrant, AdminResourceGrantDimension,
    BYTE_NANOSECONDS_PER_GB_SECOND, BYTE_SECONDS_PER_GB_MONTH, MeteringStatus, MonthlyComputeLimit,
    MonthlyComputeUnit, MonthlyLimitBehavior, MonthlyMemoryLimit, MonthlyMemoryUnit,
    MonthlyResourceLimits, MonthlyStorageLimit, MonthlyStorageUnit, MonthlyUsageMode,
    MonthlyUsageModeTransition, MonthlyUsageModeTransitionSource, byte_seconds_to_gb_month,
    fuel_to_gcu,
};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    AccountUsageResourcePattern, AccountUsageVerb, ClassPermissionTarget, PermissionTarget,
};
use golem_service_base::clients::registry::ResourceUsageMetering;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::AuthorizationError;
use golem_service_base::model::{AccountResourceLimits, MonthlyResourcePolicy, ResourceLimits};
use golem_service_base::repo::SqlDateTime;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct ResourceUsageUpdate {
    pub period: AccountUsagePeriod,
    pub monthly_usage_mode_revision: u64,
    pub monthly_policy_revision: u64,
    pub memory_byte_nanoseconds_remainder: u64,
    pub durable_storage_byte_nanoseconds_remainder: u64,
    pub ephemeral_storage_byte_nanoseconds_remainder: u64,
    pub fuel_delta: i64,
    pub http_call_count_delta: u64,
    pub rpc_call_count_delta: u64,
    pub durable_storage_byte_seconds_delta: i64,
    pub ephemeral_storage_byte_seconds_delta: i64,
    pub memory_gb_seconds_delta: i64,
    pub metering: ResourceUsageMetering,
}

fn monthly_usage_attribution(update: &ResourceUsageUpdate) -> MonthlyUsageAttribution {
    MonthlyUsageAttribution {
        policy_revision: update.monthly_policy_revision,
        memory_byte_nanoseconds_remainder: update.memory_byte_nanoseconds_remainder,
        durable_storage_byte_nanoseconds_remainder: update
            .durable_storage_byte_nanoseconds_remainder,
        ephemeral_storage_byte_nanoseconds_remainder: update
            .ephemeral_storage_byte_nanoseconds_remainder,
    }
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
            let now = SqlDateTime::now();
            match self
                .get_account_usage_at(account_id, None, update.period, &now)
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
                    account_usage.monthly_usage_attribution =
                        Some(monthly_usage_attribution(&update));
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

                    let monthly_usage_mode_revision = account_usage.monthly_usage_mode_revision;
                    let monthly_policy_revision = account_usage.monthly_policy_revision;
                    let fallback_limits = account_usage.resource_limits().ok().map(|limits| {
                        Self::fence_monthly_policy(
                            limits,
                            monthly_usage_mode_revision,
                            monthly_policy_revision,
                            true,
                        )
                    });
                    match self.account_usage_repo.add(&account_usage).await {
                        Ok(_) => match self.get_account_usage(account_id, None).await {
                            Ok(account_usage) => match account_usage.resource_limits() {
                                Ok(limits) => {
                                    limits_of_updated_accounts.insert(account_id, limits);
                                }
                                Err(error) => {
                                    tracing::error!(
                                        %account_id,
                                        %error,
                                        "Failed to resolve resource limits after usage update"
                                    );
                                    limits_of_updated_accounts.insert(
                                        account_id,
                                        fallback_limits.clone().unwrap_or_else(|| {
                                            Self::fenced_resource_limits(
                                                monthly_usage_mode_revision,
                                                monthly_policy_revision,
                                                true,
                                            )
                                        }),
                                    );
                                }
                            },
                            Err(AccountUsageError::AccountNotfound(_)) => {
                                limits_of_updated_accounts.insert(
                                    account_id,
                                    Self::fenced_resource_limits(
                                        monthly_usage_mode_revision,
                                        monthly_policy_revision,
                                        true,
                                    ),
                                );
                            }
                            Err(error) => {
                                tracing::error!(
                                    %account_id,
                                    %error,
                                    "Failed to reload account usage after resource update"
                                );
                                limits_of_updated_accounts.insert(
                                    account_id,
                                    fallback_limits.unwrap_or_else(|| {
                                        Self::fenced_resource_limits(
                                            monthly_usage_mode_revision,
                                            monthly_policy_revision,
                                            true,
                                        )
                                    }),
                                );
                            }
                        },
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
                    limits_of_updated_accounts
                        .insert(account_id, Self::fenced_resource_limits(0, 0, false));
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

        let account_usage = self.get_account_usage(account_id, None).await?;

        account_usage
            .resource_limits()
            .map_err(|error| AccountUsageError::InternalError(error.into()))
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
        let now = SqlDateTime::now();
        let account_usage = self
            .get_account_usage_at(account_id, None, AccountUsagePeriod::current(), &now)
            .await?;
        let report = self
            .account_usage_repo
            .get_usage_report(account_id.0, AccountUsagePeriod::current())
            .await?;
        let monthly_usage_mode = self
            .account_usage_repo
            .get_monthly_usage_mode(account_id.0)
            .await?
            .ok_or(AccountUsageError::AccountNotfound(account_id))?;

        Self::resource_policy(account_id, account_usage, report, monthly_usage_mode)
    }

    pub async fn set_monthly_usage_mode(
        &self,
        account_id: AccountId,
        target: MonthlyUsageMode,
        auth: &AuthCtx,
    ) -> Result<MonthlyUsageModeTransition, AccountUsageError> {
        let account = self
            .account_service
            .get(account_id, auth)
            .await
            .map_err(map_account_error(account_id))?;
        authorize_account_usage_permission(auth, &account.email, AccountUsageVerb::Update)?;

        let source = match auth {
            AuthCtx::User(user) if user.account_id == account_id => {
                MonthlyUsageModeTransitionSource::Owner
            }
            AuthCtx::AdminImpersonation(ctx)
                if ctx.target_account_id == account_id && target == MonthlyUsageMode::HardLimit =>
            {
                MonthlyUsageModeTransitionSource::Administrator
            }
            AuthCtx::System
            | AuthCtx::User(_)
            | AuthCtx::Agent(_)
            | AuthCtx::AdminImpersonation(_) => {
                return Err(AccountUsageError::MonthlyUsageModeChangeNotAllowed);
            }
        };

        self.account_usage_repo
            .set_monthly_usage_mode(account_id.0, target, auth.actor_account_id().0, source)
            .await
            .map_err(|error| match error {
                SetMonthlyUsageModeError::AccountNotFound(_) => {
                    AccountUsageError::AccountNotfound(account_id)
                }
                SetMonthlyUsageModeError::OverageNotEligible => {
                    AccountUsageError::OverageNotEligible
                }
                SetMonthlyUsageModeError::ModeUnchanged(mode) => {
                    AccountUsageError::MonthlyUsageModeUnchanged(mode)
                }
                SetMonthlyUsageModeError::InvalidConsentSource
                | SetMonthlyUsageModeError::InvalidConsentActor => {
                    AccountUsageError::MonthlyUsageModeChangeNotAllowed
                }
                SetMonthlyUsageModeError::Repo(error) => error.into(),
            })
            .map(
                crate::repo::model::account_usage::PersistedMonthlyUsageModeTransition::into_public,
            )
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
        monthly_usage_mode: AccountMonthlyUsageMode,
    ) -> Result<AccountResourcePolicy, AccountUsageError> {
        let plan_amounts = account_usage.plan_row_monthly_amounts();
        let resolved_amounts = account_usage.monthly_plan_amounts();
        let resolved = resolved_amounts
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
        let mut max_memory_per_agent = account_usage.max_memory_per_worker.clone();
        max_memory_per_agent.active_admin_grant = active_grant(
            &account_usage.admin_grants,
            AdminResourceGrantDimension::MaxMemoryPerAgent,
        );
        let mut max_storage_per_agent = account_usage.storage_limit.clone();
        if !matches!(
            max_storage_per_agent.effective_value,
            golem_common::model::account_usage::StorageResourceLimitValue::Disabled(_)
        ) {
            max_storage_per_agent.active_admin_grant = active_grant(
                &account_usage.admin_grants,
                AdminResourceGrantDimension::MaxStoragePerAgent,
            );
        }

        Ok(AccountResourcePolicy {
            account_id,
            monthly_usage_mode: monthly_usage_mode.mode,
            overage_allowed_by_plan: monthly_usage_mode.overage_eligible,
            latest_owner_transition: monthly_usage_mode.latest_owner_transition,
            monthly: MonthlyResourceLimits {
                compute_gcu: compute_limit(
                    metering.compute,
                    plan_amounts.compute_gcu,
                    active_grant(
                        &account_usage.admin_grants,
                        AdminResourceGrantDimension::MonthlyComputeGcu,
                    ),
                    resolved_amounts.compute_gcu,
                    resolved.compute_fuel,
                    report.compute_fuel,
                    report.allow_overage_compute_fuel,
                    monthly_usage_mode.mode,
                ),
                memory_gb_seconds: memory_limit(
                    metering.memory,
                    plan_amounts.memory_gb_seconds,
                    active_grant(
                        &account_usage.admin_grants,
                        AdminResourceGrantDimension::MonthlyMemoryGbSeconds,
                    ),
                    resolved_amounts.memory_gb_seconds,
                    report.memory_gb_seconds,
                    byte_nanoseconds_to_gb_seconds(report.allow_overage_memory_byte_nanoseconds),
                    monthly_usage_mode.mode,
                ),
                durable_storage_gb_month: storage_limit_policy(
                    metering.durable_storage,
                    plan_amounts.durable_storage_gb_month,
                    active_grant(
                        &account_usage.admin_grants,
                        AdminResourceGrantDimension::MonthlyDurableStorageGbMonth,
                    ),
                    resolved_amounts.durable_storage_gb_month,
                    resolved.durable_storage_byte_seconds,
                    report.durable_storage_byte_seconds,
                    byte_nanoseconds_to_gb_month(
                        report.allow_overage_durable_storage_byte_nanoseconds,
                    ),
                    monthly_usage_mode.mode,
                ),
                ephemeral_storage_gb_month: storage_limit_policy(
                    metering.ephemeral_storage,
                    plan_amounts.ephemeral_storage_gb_month,
                    active_grant(
                        &account_usage.admin_grants,
                        AdminResourceGrantDimension::MonthlyEphemeralStorageGbMonth,
                    ),
                    resolved_amounts.ephemeral_storage_gb_month,
                    resolved.ephemeral_storage_byte_seconds,
                    report.ephemeral_storage_byte_seconds,
                    byte_nanoseconds_to_gb_month(
                        report.allow_overage_ephemeral_storage_byte_nanoseconds,
                    ),
                    monthly_usage_mode.mode,
                ),
            },
            max_memory_per_agent,
            max_storage_per_agent,
        })
    }

    fn fenced_resource_limits(
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        usage_update_applied: bool,
    ) -> ResourceLimits {
        ResourceLimits {
            monthly_usage_mode_revision,
            monthly_policy_revision,
            monthly_policy: MonthlyResourcePolicy {
                period: AccountUsagePeriod::current(),
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: 0,
                available_memory_gb_seconds: 0,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: 0,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: 0,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            max_memory_per_worker: 0,
            max_table_elements_per_worker: 0,
            max_disk_space_per_worker: 0,
            per_invocation_http_call_limit: 0,
            per_invocation_rpc_call_limit: 0,
            available_http_calls: 0,
            available_rpc_calls: 0,
            max_concurrent_agents_per_executor: 0,
            oplog_writes_per_second: 0,
            usage_update_applied,
        }
    }

    fn fence_monthly_policy(
        mut limits: ResourceLimits,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        usage_update_applied: bool,
    ) -> ResourceLimits {
        limits.monthly_usage_mode_revision = monthly_usage_mode_revision;
        limits.monthly_policy_revision = monthly_policy_revision;
        limits.monthly_policy = MonthlyResourcePolicy {
            period: AccountUsagePeriod::current(),
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 0,
            available_memory_gb_seconds: 0,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 0,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        };
        limits.available_http_calls = 0;
        limits.available_rpc_calls = 0;
        limits.usage_update_applied = usage_update_applied;
        limits
    }

    async fn get_account_usage(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
    ) -> Result<RepoAccountUsage, AccountUsageError> {
        let now = SqlDateTime::now();
        self.get_account_usage_at(account_id, usage_type, AccountUsagePeriod::current(), &now)
            .await
    }

    async fn get_account_usage_at(
        &self,
        account_id: AccountId,
        usage_type: Option<UsageType>,
        period: AccountUsagePeriod,
        active_at: &SqlDateTime,
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
            None => {
                self.account_usage_repo
                    .get_with_active_overrides_at(account_id.0, &date, active_at)
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

fn compute_limit(
    metering: MeteringStatus,
    plan_amount: u64,
    active_admin_grant: Option<AdminResourceGrant>,
    resolved_monthly_amount: u64,
    amount_fuel: u64,
    usage_fuel: u64,
    allow_overage_usage_fuel: u64,
    mode: MonthlyUsageMode,
) -> MonthlyComputeLimit {
    match metering {
        MeteringStatus::Enabled => MonthlyComputeLimit {
            metering,
            plan_amount: Some(plan_amount),
            active_admin_grant,
            resolved_monthly_amount: Some(resolved_monthly_amount),
            usage: Some(fuel_to_gcu(usage_fuel)),
            remaining: Some(fuel_to_gcu(amount_fuel.saturating_sub(usage_fuel))),
            allow_overage_usage: Some(fuel_to_gcu(allow_overage_usage_fuel)),
            unit: MonthlyComputeUnit::Gcu,
            behavior: Some(monthly_limit_behavior(mode)),
        },
        MeteringStatus::Disabled => MonthlyComputeLimit {
            metering,
            plan_amount: None,
            active_admin_grant: None,
            resolved_monthly_amount: None,
            usage: None,
            remaining: None,
            allow_overage_usage: None,
            unit: MonthlyComputeUnit::Gcu,
            behavior: None,
        },
        MeteringStatus::Unknown => MonthlyComputeLimit {
            metering,
            plan_amount: None,
            active_admin_grant: None,
            resolved_monthly_amount: None,
            usage: None,
            remaining: None,
            allow_overage_usage: None,
            unit: MonthlyComputeUnit::Gcu,
            behavior: None,
        },
    }
}

fn memory_limit(
    metering: MeteringStatus,
    plan_amount: u64,
    active_admin_grant: Option<AdminResourceGrant>,
    resolved_monthly_amount: u64,
    usage: u64,
    allow_overage_usage: f64,
    mode: MonthlyUsageMode,
) -> MonthlyMemoryLimit {
    match metering {
        MeteringStatus::Enabled => MonthlyMemoryLimit {
            metering,
            plan_amount: Some(plan_amount),
            active_admin_grant,
            resolved_monthly_amount: Some(resolved_monthly_amount),
            usage: Some(usage),
            remaining: Some(resolved_monthly_amount.saturating_sub(usage)),
            allow_overage_usage: Some(allow_overage_usage),
            unit: MonthlyMemoryUnit::GbSeconds,
            behavior: Some(monthly_limit_behavior(mode)),
        },
        MeteringStatus::Disabled => MonthlyMemoryLimit {
            metering,
            plan_amount: None,
            active_admin_grant: None,
            resolved_monthly_amount: None,
            usage: None,
            remaining: None,
            allow_overage_usage: None,
            unit: MonthlyMemoryUnit::GbSeconds,
            behavior: None,
        },
        MeteringStatus::Unknown => MonthlyMemoryLimit {
            metering,
            plan_amount: None,
            active_admin_grant: None,
            resolved_monthly_amount: None,
            usage: None,
            remaining: None,
            allow_overage_usage: None,
            unit: MonthlyMemoryUnit::GbSeconds,
            behavior: None,
        },
    }
}

fn storage_limit_policy(
    metering: MeteringStatus,
    plan_amount: u64,
    active_admin_grant: Option<AdminResourceGrant>,
    resolved_monthly_amount: u64,
    amount_byte_seconds: u64,
    usage_byte_seconds: u64,
    allow_overage_usage_gb_month: f64,
    mode: MonthlyUsageMode,
) -> MonthlyStorageLimit {
    match metering {
        MeteringStatus::Enabled => MonthlyStorageLimit {
            metering,
            plan_amount: Some(plan_amount),
            active_admin_grant,
            resolved_monthly_amount: Some(resolved_monthly_amount),
            usage: Some(byte_seconds_to_gb_month(usage_byte_seconds)),
            remaining: Some(byte_seconds_to_gb_month(
                amount_byte_seconds.saturating_sub(usage_byte_seconds),
            )),
            allow_overage_usage: Some(allow_overage_usage_gb_month),
            unit: MonthlyStorageUnit::GbMonth,
            behavior: Some(monthly_limit_behavior(mode)),
        },
        MeteringStatus::Disabled => MonthlyStorageLimit {
            metering,
            plan_amount: None,
            active_admin_grant: None,
            resolved_monthly_amount: None,
            usage: None,
            remaining: None,
            allow_overage_usage: None,
            unit: MonthlyStorageUnit::GbMonth,
            behavior: None,
        },
        MeteringStatus::Unknown => MonthlyStorageLimit {
            metering,
            plan_amount: None,
            active_admin_grant: None,
            resolved_monthly_amount: None,
            usage: None,
            remaining: None,
            allow_overage_usage: None,
            unit: MonthlyStorageUnit::GbMonth,
            behavior: None,
        },
    }
}

fn byte_nanoseconds_to_gb_month(value: u128) -> f64 {
    value as f64 / (BYTE_SECONDS_PER_GB_MONTH as f64 * 1_000_000_000.0)
}

fn byte_nanoseconds_to_gb_seconds(value: u128) -> f64 {
    value as f64 / BYTE_NANOSECONDS_PER_GB_SECOND as f64
}

fn active_grant(
    grants: &[AdminResourceGrant],
    dimension: AdminResourceGrantDimension,
) -> Option<AdminResourceGrant> {
    grants
        .iter()
        .find(|grant| grant.dimension == dimension)
        .cloned()
}

fn monthly_limit_behavior(mode: MonthlyUsageMode) -> MonthlyLimitBehavior {
    match mode {
        MonthlyUsageMode::HardLimit => MonthlyLimitBehavior::HardLimit,
        MonthlyUsageMode::AllowOverage => MonthlyLimitBehavior::IncludedAllowance,
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
    use crate::repo::model::account_usage::{AccountUsage, AdminResourceGrantValues, UsageType};
    use crate::repo::model::plan::PlanRecord;
    use golem_common::model::account_usage::{
        AdminResourceGrant, BYTE_NANOSECONDS_PER_GB_SECOND, BYTE_SECONDS_PER_GB_MONTH,
        FUEL_PER_GCU, MemoryLimit, StorageLimit,
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
            overage_eligible: false,
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
            storage_limit: StorageLimit::resolve(false, u64::MAX, None, u64::MAX, false),
            max_memory_per_worker: MemoryLimit::resolve(u64::MAX, None, u64::MAX, false),
            max_disk_space_per_worker_value:
                golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
            max_memory_per_worker_value: u64::MAX,
            admin_grant_values: Default::default(),
            admin_grants: Vec::new(),
            metering: None,
            monthly_usage_mode: MonthlyUsageMode::HardLimit,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
            monthly_memory_byte_nanoseconds_remainder: 0,
            monthly_durable_storage_byte_nanoseconds_remainder: 0,
            monthly_ephemeral_storage_byte_nanoseconds_remainder: 0,
            monthly_usage_attribution: None,
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
        usage.max_disk_space_per_worker_value = 12;
        usage.max_memory_per_worker_value = 150;
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
            allow_overage_compute_fuel: FUEL_PER_GCU / 2,
            allow_overage_memory_byte_nanoseconds: 10 * BYTE_NANOSECONDS_PER_GB_SECOND,
            allow_overage_durable_storage_byte_nanoseconds: 2
                * BYTE_SECONDS_PER_GB_MONTH as u128
                * 1_000_000_000,
            allow_overage_ephemeral_storage_byte_nanoseconds: BYTE_SECONDS_PER_GB_MONTH as u128
                * 1_000_000_000,
            metering,
        }
    }

    fn hard_limit_mode() -> AccountMonthlyUsageMode {
        AccountMonthlyUsageMode {
            mode: MonthlyUsageMode::HardLimit,
            revision: 0,
            overage_eligible: false,
            latest_owner_transition: None,
        }
    }

    fn allow_overage_mode() -> AccountMonthlyUsageMode {
        AccountMonthlyUsageMode {
            mode: MonthlyUsageMode::AllowOverage,
            revision: 1,
            overage_eligible: true,
            latest_owner_transition: None,
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
            hard_limit_mode(),
        )
        .unwrap();

        assert_eq!(policy.account_id, account_id);
        assert_eq!(policy.monthly.compute_gcu.plan_amount, Some(5));
        assert_eq!(policy.monthly.compute_gcu.resolved_monthly_amount, Some(5));
        assert_eq!(policy.monthly.compute_gcu.usage, Some(1.5));
        assert_eq!(policy.monthly.compute_gcu.remaining, Some(3.5));
        assert_eq!(policy.monthly.compute_gcu.allow_overage_usage, Some(0.5));
        assert_eq!(
            policy.monthly.compute_gcu.behavior,
            Some(MonthlyLimitBehavior::HardLimit)
        );
        assert_eq!(policy.monthly.memory_gb_seconds.plan_amount, Some(50));
        assert_eq!(
            policy.monthly.memory_gb_seconds.resolved_monthly_amount,
            Some(50)
        );
        assert_eq!(policy.monthly.memory_gb_seconds.usage, Some(60));
        assert_eq!(policy.monthly.memory_gb_seconds.remaining, Some(0));
        assert_eq!(
            policy.monthly.memory_gb_seconds.allow_overage_usage,
            Some(10.0)
        );
        assert_eq!(policy.monthly.durable_storage_gb_month.plan_amount, Some(7));
        assert_eq!(
            policy
                .monthly
                .durable_storage_gb_month
                .resolved_monthly_amount,
            Some(7)
        );
        assert_eq!(policy.monthly.durable_storage_gb_month.usage, Some(8.0));
        assert_eq!(policy.monthly.durable_storage_gb_month.remaining, Some(0.0));
        assert_eq!(
            policy.monthly.durable_storage_gb_month.allow_overage_usage,
            Some(2.0)
        );
        assert_eq!(
            policy.monthly.ephemeral_storage_gb_month.plan_amount,
            Some(11)
        );
        assert_eq!(policy.monthly.ephemeral_storage_gb_month.usage, Some(4.0));
        assert_eq!(
            policy.monthly.ephemeral_storage_gb_month.remaining,
            Some(7.0)
        );
        assert_eq!(
            policy
                .monthly
                .ephemeral_storage_gb_month
                .allow_overage_usage,
            Some(1.0)
        );
        assert_eq!(policy.max_storage_per_agent, expected_storage_limit);
        assert_eq!(policy.max_memory_per_agent, expected_memory_limit);
    }

    #[test]
    fn resource_policy_uses_admin_grants_for_monthly_amounts() {
        let mut usage = make_policy_usage();
        usage.admin_grant_values = AdminResourceGrantValues {
            monthly_compute_gcu: Some(8),
            monthly_memory_gb_seconds: Some(70),
            monthly_durable_storage_gb_month: Some(9),
            monthly_ephemeral_storage_gb_month: Some(12),
            max_memory_per_worker: None,
            max_disk_space_per_worker: None,
        };
        let account_id = AccountId(usage.account_id);
        let grant = AdminResourceGrant {
            dimension:
                golem_common::model::account_usage::AdminResourceGrantDimension::MonthlyComputeGcu,
            value: 8,
            reason: golem_common::model::account_usage::AdminResourceGrantReason::Support,
            actor_account_id: AccountId::SYSTEM,
            granted_at: Utc::now(),
            expires_at: None,
        };
        usage.admin_grants = vec![grant.clone()];
        let policy = AccountUsageService::resource_policy(
            account_id,
            usage,
            make_policy_report(Some(ResourceUsageMetering::all_enabled())),
            hard_limit_mode(),
        )
        .unwrap();

        assert_eq!(policy.monthly.compute_gcu.plan_amount, Some(5));
        assert_eq!(policy.monthly.compute_gcu.active_admin_grant, Some(grant));
        assert_eq!(policy.monthly.compute_gcu.resolved_monthly_amount, Some(8));
        assert_eq!(policy.monthly.memory_gb_seconds.plan_amount, Some(50));
        assert_eq!(
            policy.monthly.memory_gb_seconds.resolved_monthly_amount,
            Some(70)
        );
        assert_eq!(policy.monthly.durable_storage_gb_month.plan_amount, Some(7));
        assert_eq!(
            policy
                .monthly
                .durable_storage_gb_month
                .resolved_monthly_amount,
            Some(9)
        );
        assert_eq!(
            policy.monthly.ephemeral_storage_gb_month.plan_amount,
            Some(11)
        );
        assert_eq!(
            policy
                .monthly
                .ephemeral_storage_gb_month
                .resolved_monthly_amount,
            Some(12)
        );
    }

    #[test]
    fn resource_policy_exposes_per_agent_admin_grants() {
        let mut usage = make_policy_usage();
        let memory_grant = AdminResourceGrant {
            dimension: AdminResourceGrantDimension::MaxMemoryPerAgent,
            value: usage.max_memory_per_worker_value,
            reason: golem_common::model::account_usage::AdminResourceGrantReason::Support,
            actor_account_id: AccountId::SYSTEM,
            granted_at: Utc::now(),
            expires_at: None,
        };
        let storage_grant = AdminResourceGrant {
            dimension: AdminResourceGrantDimension::MaxStoragePerAgent,
            value: usage.max_disk_space_per_worker_value,
            reason: golem_common::model::account_usage::AdminResourceGrantReason::Support,
            actor_account_id: AccountId::SYSTEM,
            granted_at: Utc::now(),
            expires_at: None,
        };
        usage.admin_grants = vec![memory_grant.clone(), storage_grant.clone()];
        let account_id = AccountId(usage.account_id);

        let policy = AccountUsageService::resource_policy(
            account_id,
            usage,
            make_policy_report(Some(ResourceUsageMetering::all_enabled())),
            hard_limit_mode(),
        )
        .unwrap();

        assert_eq!(
            policy.max_memory_per_agent.active_admin_grant,
            Some(memory_grant)
        );
        assert_eq!(
            policy.max_storage_per_agent.active_admin_grant,
            Some(storage_grant)
        );
    }

    #[test]
    fn resource_policy_preserves_fractional_memory_excess() {
        let usage = make_policy_usage();
        let account_id = AccountId(usage.account_id);
        let mut report = make_policy_report(Some(ResourceUsageMetering::all_enabled()));
        report.allow_overage_memory_byte_nanoseconds = BYTE_NANOSECONDS_PER_GB_SECOND / 2;

        let policy =
            AccountUsageService::resource_policy(account_id, usage, report, allow_overage_mode())
                .unwrap();

        assert_eq!(
            policy.monthly.memory_gb_seconds.allow_overage_usage,
            Some(0.5)
        );
    }

    #[test]
    fn resource_limits_resolve_monthly_resources_from_grants_and_current_usage() {
        let mut usage = make_policy_usage();
        usage.year = 2026;
        usage.month = 9;
        usage.monthly_usage_mode = MonthlyUsageMode::AllowOverage;
        usage.monthly_usage_mode_revision = 7;
        usage.admin_grant_values.monthly_compute_gcu = Some(3);
        usage.admin_grant_values.monthly_memory_gb_seconds = Some(70);
        usage
            .usage
            .insert(UsageType::MonthlyGasLimit, FUEL_PER_GCU + FUEL_PER_GCU / 2);
        usage.usage.insert(UsageType::MonthlyMemoryGbSeconds, 17);
        usage.monthly_memory_byte_nanoseconds_remainder = BYTE_NANOSECONDS_PER_GB_SECOND / 2;
        usage.usage.insert(
            UsageType::MonthlyDurableAgentStorageByteSeconds,
            2 * BYTE_SECONDS_PER_GB_MONTH + 3,
        );
        usage.usage.insert(
            UsageType::MonthlyEphemeralStorageByteSeconds,
            12 * BYTE_SECONDS_PER_GB_MONTH,
        );
        usage.monthly_durable_storage_byte_nanoseconds_remainder = 500_000_000;

        let limits = usage.resource_limits().unwrap();

        assert_eq!(
            limits.monthly_policy.period,
            AccountUsagePeriod {
                year: 2026,
                month: 9
            }
        );
        assert_eq!(limits.monthly_policy.mode, MonthlyUsageMode::AllowOverage);
        assert_eq!(limits.monthly_policy.available_fuel, 1_500_000);
        assert_eq!(limits.monthly_policy.available_memory_gb_seconds, 52);
        assert_eq!(
            limits
                .monthly_policy
                .available_memory_byte_nanoseconds_remainder,
            (BYTE_NANOSECONDS_PER_GB_SECOND / 2) as u64
        );
        assert_eq!(
            limits.monthly_policy.available_durable_storage_byte_seconds,
            5 * BYTE_SECONDS_PER_GB_MONTH - 4
        );
        assert_eq!(
            limits
                .monthly_policy
                .available_durable_storage_byte_nanoseconds_remainder,
            500_000_000
        );
        assert_eq!(
            limits
                .monthly_policy
                .available_ephemeral_storage_byte_seconds,
            0
        );
        assert_eq!(
            limits
                .monthly_policy
                .available_ephemeral_storage_byte_nanoseconds_remainder,
            0
        );
        assert_eq!(limits.monthly_usage_mode_revision, 7);

        usage.usage.insert(UsageType::MonthlyMemoryGbSeconds, 71);
        assert_eq!(
            usage
                .resource_limits()
                .unwrap()
                .monthly_policy
                .available_memory_gb_seconds,
            0
        );
    }

    #[test]
    fn deleted_account_limits_fence_monthly_policy_for_current_period() {
        let limits = AccountUsageService::fenced_resource_limits(0, 0, false);

        assert_eq!(limits.monthly_policy.period, AccountUsagePeriod::current());
        assert_eq!(limits.monthly_policy.mode, MonthlyUsageMode::HardLimit);
        assert_eq!(limits.monthly_policy.available_fuel, 0);
        assert_eq!(limits.monthly_policy.available_memory_gb_seconds, 0);
        assert!(!limits.usage_update_applied);
    }

    #[test]
    fn post_write_failure_fence_acknowledges_the_committed_update() {
        let limits = AccountUsageService::fenced_resource_limits(7, 9, true);

        assert_eq!(limits.monthly_policy.period, AccountUsagePeriod::current());
        assert_eq!(limits.monthly_policy.mode, MonthlyUsageMode::HardLimit);
        assert_eq!(limits.monthly_policy.available_fuel, 0);
        assert_eq!(limits.monthly_policy.available_memory_gb_seconds, 0);
        assert_eq!(limits.available_http_calls, 0);
        assert_eq!(limits.available_rpc_calls, 0);
        assert_eq!(limits.monthly_usage_mode_revision, 7);
        assert_eq!(limits.monthly_policy_revision, 9);
        assert!(limits.usage_update_applied);
    }

    #[test]
    fn post_write_failure_fence_preserves_per_agent_limits() {
        let mut limits = make_policy_usage().resource_limits().unwrap();
        let expected_memory = limits.max_memory_per_worker;
        let expected_table_elements = limits.max_table_elements_per_worker;
        let expected_disk_space = limits.max_disk_space_per_worker;
        let expected_http_limit = limits.per_invocation_http_call_limit;
        let expected_rpc_limit = limits.per_invocation_rpc_call_limit;
        let expected_concurrency = limits.max_concurrent_agents_per_executor;
        let expected_oplog_rate = limits.oplog_writes_per_second;

        limits = AccountUsageService::fence_monthly_policy(limits, 7, 9, true);

        assert_eq!(limits.monthly_policy.available_fuel, 0);
        assert_eq!(limits.monthly_policy.available_memory_gb_seconds, 0);
        assert_eq!(limits.monthly_usage_mode_revision, 7);
        assert_eq!(limits.monthly_policy_revision, 9);
        assert!(limits.usage_update_applied);
        assert_eq!(limits.max_memory_per_worker, expected_memory);
        assert_eq!(
            limits.max_table_elements_per_worker,
            expected_table_elements
        );
        assert_eq!(limits.max_disk_space_per_worker, expected_disk_space);
        assert_eq!(limits.per_invocation_http_call_limit, expected_http_limit);
        assert_eq!(limits.per_invocation_rpc_call_limit, expected_rpc_limit);
        assert_eq!(
            limits.max_concurrent_agents_per_executor,
            expected_concurrency
        );
        assert_eq!(limits.oplog_writes_per_second, expected_oplog_rate);
    }

    #[test]
    fn resource_policy_omits_disabled_dimension_values() {
        let usage = make_policy_usage();
        let account_id = AccountId(usage.account_id);
        let policy = AccountUsageService::resource_policy(
            account_id,
            usage,
            make_policy_report(Some(ResourceUsageMetering::default())),
            allow_overage_mode(),
        )
        .unwrap();
        assert_eq!(policy.monthly_usage_mode, MonthlyUsageMode::AllowOverage);
        let value = serde_json::to_value(policy).unwrap();

        for dimension in [
            "computeGcu",
            "memoryGbSeconds",
            "durableStorageGbMonth",
            "ephemeralStorageGbMonth",
        ] {
            let dimension = &value["monthly"][dimension];
            assert_eq!(dimension["metering"], "disabled");
            assert!(dimension.get("planAmount").is_none());
            assert!(dimension.get("activeAdminGrant").is_none());
            assert!(dimension.get("resolvedMonthlyAmount").is_none());
            assert!(dimension.get("usage").is_none());
            assert!(dimension.get("remaining").is_none());
            assert!(dimension.get("allowOverageUsage").is_none());
            assert!(dimension.get("behavior").is_none());
        }
    }

    #[test]
    fn resource_policy_omits_unknown_dimension_values() {
        let usage = make_policy_usage();
        let account_id = AccountId(usage.account_id);
        let policy = AccountUsageService::resource_policy(
            account_id,
            usage,
            make_policy_report(None),
            hard_limit_mode(),
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
            assert_eq!(dimension["metering"], "unknown");
            assert!(dimension.get("planAmount").is_none());
            assert!(dimension.get("activeAdminGrant").is_none());
            assert!(dimension.get("resolvedMonthlyAmount").is_none());
            assert!(dimension.get("usage").is_none());
            assert!(dimension.get("remaining").is_none());
            assert!(dimension.get("allowOverageUsage").is_none());
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
