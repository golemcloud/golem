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

use crate::repo::model::account_resource_override::persisted_admin_reason;
use crate::repo::model::plan::PlanRecord;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AccountUsagePeriod, AdminResourceGrant, AdminResourceGrantDimension,
    BYTE_NANOSECONDS_PER_GB_SECOND, MonthlyPlanAmountError, MonthlyPlanAmounts, MonthlyUsageMode,
    MonthlyUsageModeTransition, MonthlyUsageModeTransitionSource, StorageLimit,
};
use golem_service_base::clients::registry::ResourceUsageMetering;
use golem_service_base::model::{MonthlyResourcePolicy, ResourceLimits};
use golem_service_base::repo::NumericU64;
use golem_service_base::repo::{RepoError, RepoResult, SqlDateTime};
use sqlx::FromRow;
use std::collections::BTreeMap;
use strum_macros::EnumIter;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageGrouping {
    Total,
    Monthly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageTracking {
    Stats,
    SelectTotalAppCount,
    SelectTotalEnvCount,
    SelectTotalComponentCount,
    SelectTotalComponentSize,
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type, EnumIter)]
#[sqlx(type_name = "integer")]
pub enum UsageType {
    TotalWorkerConnectionCount = 1,
    MonthlyGasLimit = 2,
    MonthlyComponentUploadLimitBytes = 3,
    TotalAppCount = 4,
    TotalEnvCount = 5,
    TotalComponentCount = 6,
    TotalComponentStorageBytes = 7,
    MonthlyHttpCalls = 8,
    MonthlyRpcCalls = 9,
    MonthlyDurableAgentStorageByteSeconds = 10,
    MonthlyEphemeralStorageByteSeconds = 11,
    MonthlyMemoryGbSeconds = 12,
}

impl UsageType {
    pub fn grouping(&self) -> UsageGrouping {
        match self {
            UsageType::TotalAppCount
            | UsageType::TotalEnvCount
            | UsageType::TotalComponentCount
            | UsageType::TotalWorkerConnectionCount
            | UsageType::TotalComponentStorageBytes => UsageGrouping::Total,
            UsageType::MonthlyGasLimit
            | UsageType::MonthlyComponentUploadLimitBytes
            | UsageType::MonthlyHttpCalls
            | UsageType::MonthlyRpcCalls
            | UsageType::MonthlyDurableAgentStorageByteSeconds
            | UsageType::MonthlyEphemeralStorageByteSeconds => UsageGrouping::Monthly,
            UsageType::MonthlyMemoryGbSeconds => UsageGrouping::Monthly,
        }
    }

    pub fn tracking(&self) -> UsageTracking {
        match self {
            UsageType::TotalAppCount => UsageTracking::SelectTotalAppCount,
            UsageType::TotalEnvCount => UsageTracking::SelectTotalEnvCount,
            UsageType::TotalComponentCount => UsageTracking::SelectTotalComponentCount,
            UsageType::TotalComponentStorageBytes => UsageTracking::SelectTotalComponentSize,
            UsageType::TotalWorkerConnectionCount
            | UsageType::MonthlyGasLimit
            | UsageType::MonthlyComponentUploadLimitBytes
            | UsageType::MonthlyHttpCalls
            | UsageType::MonthlyRpcCalls
            | UsageType::MonthlyDurableAgentStorageByteSeconds
            | UsageType::MonthlyEphemeralStorageByteSeconds => UsageTracking::Stats,
            UsageType::MonthlyMemoryGbSeconds => UsageTracking::Stats,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountUsageStatsRecord {
    pub account_id: Uuid,
    pub usage_type: i32,
    pub usage_key: String,
    pub value: NumericU64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    pub account_id: Uuid,

    pub year: i32,
    pub month: u32,

    pub usage: BTreeMap<UsageType, u64>,
    pub plan: PlanRecord,
    pub storage_limit: StorageLimit,
    pub max_memory_per_worker: golem_common::model::account_usage::MemoryLimit,
    pub admin_grant_values: AdminResourceGrantValues,
    pub admin_grants: Vec<AdminResourceGrant>,
    pub metering: Option<ResourceUsageMetering>,
    pub monthly_usage_mode: MonthlyUsageMode,
    pub monthly_usage_mode_revision: u64,
    pub monthly_memory_byte_nanoseconds_remainder: u128,
    pub monthly_durable_storage_byte_nanoseconds_remainder: u128,
    pub monthly_ephemeral_storage_byte_nanoseconds_remainder: u128,
    pub monthly_usage_attribution: Option<MonthlyUsageAttribution>,
    pub changes: BTreeMap<UsageType, i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MonthlyUsageAttribution {
    pub revision: u64,
    pub memory_byte_nanoseconds_remainder: u64,
    pub durable_storage_byte_nanoseconds_remainder: u64,
    pub ephemeral_storage_byte_nanoseconds_remainder: u64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountUsagePlan {
    #[sqlx(flatten)]
    pub plan: PlanRecord,
    pub storage_override_value: Option<NumericU64>,
    pub max_memory_override_value: Option<NumericU64>,
    pub monthly_compute_grant_value: Option<NumericU64>,
    pub monthly_compute_grant_reason: Option<String>,
    pub monthly_compute_grant_expires_at: Option<SqlDateTime>,
    pub monthly_compute_grant_created_by: Option<Uuid>,
    pub monthly_compute_grant_created_at: Option<SqlDateTime>,
    pub monthly_memory_grant_value: Option<NumericU64>,
    pub monthly_memory_grant_reason: Option<String>,
    pub monthly_memory_grant_expires_at: Option<SqlDateTime>,
    pub monthly_memory_grant_created_by: Option<Uuid>,
    pub monthly_memory_grant_created_at: Option<SqlDateTime>,
    pub monthly_durable_storage_grant_value: Option<NumericU64>,
    pub monthly_durable_storage_grant_reason: Option<String>,
    pub monthly_durable_storage_grant_expires_at: Option<SqlDateTime>,
    pub monthly_durable_storage_grant_created_by: Option<Uuid>,
    pub monthly_durable_storage_grant_created_at: Option<SqlDateTime>,
    pub monthly_ephemeral_storage_grant_value: Option<NumericU64>,
    pub monthly_ephemeral_storage_grant_reason: Option<String>,
    pub monthly_ephemeral_storage_grant_expires_at: Option<SqlDateTime>,
    pub monthly_ephemeral_storage_grant_created_by: Option<Uuid>,
    pub monthly_ephemeral_storage_grant_created_at: Option<SqlDateTime>,
    pub max_memory_grant_value: Option<NumericU64>,
    pub max_memory_grant_reason: Option<String>,
    pub max_memory_grant_expires_at: Option<SqlDateTime>,
    pub max_memory_grant_created_by: Option<Uuid>,
    pub max_memory_grant_created_at: Option<SqlDateTime>,
    pub storage_grant_value: Option<NumericU64>,
    pub storage_grant_reason: Option<String>,
    pub storage_grant_expires_at: Option<SqlDateTime>,
    pub storage_grant_created_by: Option<Uuid>,
    pub storage_grant_created_at: Option<SqlDateTime>,
    pub monthly_usage_mode: String,
    pub monthly_usage_mode_revision: NumericU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdminResourceGrantValues {
    pub monthly_compute_gcu: Option<u64>,
    pub monthly_memory_gb_seconds: Option<u64>,
    pub monthly_durable_storage_gb_month: Option<u64>,
    pub monthly_ephemeral_storage_gb_month: Option<u64>,
    pub max_memory_per_worker: Option<u64>,
    pub max_disk_space_per_worker: Option<u64>,
}

impl AccountUsagePlan {
    pub fn monthly_usage_mode(&self) -> RepoResult<MonthlyUsageMode> {
        let persisted_mode = monthly_usage_mode(&self.monthly_usage_mode)?;
        if persisted_mode == MonthlyUsageMode::AllowOverage && !self.plan.overage_eligible {
            Ok(MonthlyUsageMode::HardLimit)
        } else {
            Ok(persisted_mode)
        }
    }

    pub fn admin_grant_values(&self) -> AdminResourceGrantValues {
        AdminResourceGrantValues {
            monthly_compute_gcu: self
                .monthly_compute_grant_value
                .as_ref()
                .map(NumericU64::get),
            monthly_memory_gb_seconds: self
                .monthly_memory_grant_value
                .as_ref()
                .map(NumericU64::get),
            monthly_durable_storage_gb_month: self
                .monthly_durable_storage_grant_value
                .as_ref()
                .map(NumericU64::get),
            monthly_ephemeral_storage_gb_month: self
                .monthly_ephemeral_storage_grant_value
                .as_ref()
                .map(NumericU64::get),
            max_memory_per_worker: self.max_memory_grant_value.as_ref().map(NumericU64::get),
            max_disk_space_per_worker: self.storage_grant_value.as_ref().map(NumericU64::get),
        }
    }

    pub fn admin_grants(&self) -> RepoResult<Vec<AdminResourceGrant>> {
        let candidates = [
            (
                AdminResourceGrantDimension::MonthlyComputeGcu,
                self.monthly_compute_grant_value.as_ref(),
                self.monthly_compute_grant_reason.as_deref(),
                self.monthly_compute_grant_expires_at.as_ref(),
                self.monthly_compute_grant_created_by,
                self.monthly_compute_grant_created_at.as_ref(),
            ),
            (
                AdminResourceGrantDimension::MonthlyMemoryGbSeconds,
                self.monthly_memory_grant_value.as_ref(),
                self.monthly_memory_grant_reason.as_deref(),
                self.monthly_memory_grant_expires_at.as_ref(),
                self.monthly_memory_grant_created_by,
                self.monthly_memory_grant_created_at.as_ref(),
            ),
            (
                AdminResourceGrantDimension::MonthlyDurableStorageGbMonth,
                self.monthly_durable_storage_grant_value.as_ref(),
                self.monthly_durable_storage_grant_reason.as_deref(),
                self.monthly_durable_storage_grant_expires_at.as_ref(),
                self.monthly_durable_storage_grant_created_by,
                self.monthly_durable_storage_grant_created_at.as_ref(),
            ),
            (
                AdminResourceGrantDimension::MonthlyEphemeralStorageGbMonth,
                self.monthly_ephemeral_storage_grant_value.as_ref(),
                self.monthly_ephemeral_storage_grant_reason.as_deref(),
                self.monthly_ephemeral_storage_grant_expires_at.as_ref(),
                self.monthly_ephemeral_storage_grant_created_by,
                self.monthly_ephemeral_storage_grant_created_at.as_ref(),
            ),
            (
                AdminResourceGrantDimension::MaxMemoryPerAgent,
                self.max_memory_grant_value.as_ref(),
                self.max_memory_grant_reason.as_deref(),
                self.max_memory_grant_expires_at.as_ref(),
                self.max_memory_grant_created_by,
                self.max_memory_grant_created_at.as_ref(),
            ),
            (
                AdminResourceGrantDimension::MaxStoragePerAgent,
                self.storage_grant_value.as_ref(),
                self.storage_grant_reason.as_deref(),
                self.storage_grant_expires_at.as_ref(),
                self.storage_grant_created_by,
                self.storage_grant_created_at.as_ref(),
            ),
        ];
        let mut grants = Vec::new();
        for (dimension, value, reason, expires_at, created_by, created_at) in candidates {
            let Some(value) = value else {
                continue;
            };
            let missing = |field| {
                RepoError::InternalError(anyhow::anyhow!(
                    "Active {dimension} admin grant is missing {field}"
                ))
            };
            grants.push(AdminResourceGrant {
                dimension,
                value: value.get(),
                reason: persisted_admin_reason(reason.ok_or_else(|| missing("reason"))?)?,
                actor_account_id: AccountId(created_by.ok_or_else(|| missing("created_by"))?),
                granted_at: created_at
                    .ok_or_else(|| missing("created_at"))?
                    .clone()
                    .into_utc(),
                expires_at: expires_at.cloned().map(SqlDateTime::into_utc),
            });
        }
        Ok(grants)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountUsageRecord {
    pub period: AccountUsagePeriod,
    pub as_of: Option<DateTime<Utc>>,
    pub durable_storage_byte_seconds: u64,
    pub ephemeral_storage_byte_seconds: u64,
    pub compute_fuel: u64,
    pub memory_gb_seconds: u64,
    pub metering: Option<ResourceUsageMetering>,
}

#[derive(FromRow, Debug, Clone, PartialEq, Eq)]
pub struct MonthlyUsageModeStateRecord {
    pub mode: String,
    pub revision: NumericU64,
    pub overage_eligible: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountMonthlyUsageMode {
    pub mode: MonthlyUsageMode,
    pub revision: u64,
    pub overage_eligible: bool,
    pub latest_owner_transition: Option<MonthlyUsageModeTransition>,
}

impl MonthlyUsageModeStateRecord {
    pub fn mode(&self) -> RepoResult<MonthlyUsageMode> {
        let persisted_mode = monthly_usage_mode(&self.mode)?;
        if persisted_mode == MonthlyUsageMode::AllowOverage && !self.overage_eligible {
            Ok(MonthlyUsageMode::HardLimit)
        } else {
            Ok(persisted_mode)
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct MonthlyUsageModeTransitionRecord {
    pub revision: NumericU64,
    pub actor_account_id: Uuid,
    pub changed_at: golem_service_base::repo::SqlDateTime,
    pub source: String,
    pub previous_mode: String,
    pub new_mode: String,
    pub period_year: i32,
    pub period_month: i32,
    pub compute_fuel: NumericU64,
    pub memory_gb_seconds: NumericU64,
    pub durable_storage_byte_seconds: NumericU64,
    pub ephemeral_storage_byte_seconds: NumericU64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonthlyUsageTransitionBaseline {
    pub period: AccountUsagePeriod,
    pub compute_fuel: u64,
    pub memory_gb_seconds: u64,
    pub durable_storage_byte_seconds: u64,
    pub ephemeral_storage_byte_seconds: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedMonthlyUsageModeTransition {
    pub revision: u64,
    pub actor_account_id: golem_common::model::account::AccountId,
    pub changed_at: DateTime<Utc>,
    pub source: MonthlyUsageModeTransitionSource,
    pub previous_mode: MonthlyUsageMode,
    pub new_mode: MonthlyUsageMode,
    pub usage_baseline: MonthlyUsageTransitionBaseline,
}

impl PersistedMonthlyUsageModeTransition {
    pub fn into_public(self) -> MonthlyUsageModeTransition {
        MonthlyUsageModeTransition {
            actor_account_id: self.actor_account_id,
            changed_at: self.changed_at,
            source: self.source,
            previous_mode: self.previous_mode,
            new_mode: self.new_mode,
        }
    }
}

impl MonthlyUsageModeTransitionRecord {
    pub fn into_model(self) -> RepoResult<PersistedMonthlyUsageModeTransition> {
        Ok(PersistedMonthlyUsageModeTransition {
            revision: self.revision.get(),
            actor_account_id: golem_common::model::account::AccountId(self.actor_account_id),
            changed_at: self.changed_at.into_utc(),
            source: monthly_usage_mode_transition_source(&self.source)?,
            previous_mode: monthly_usage_mode(&self.previous_mode)?,
            new_mode: monthly_usage_mode(&self.new_mode)?,
            usage_baseline: MonthlyUsageTransitionBaseline {
                period: AccountUsagePeriod {
                    year: self.period_year,
                    month: u32::try_from(self.period_month).expect("stored month is valid"),
                },
                compute_fuel: self.compute_fuel.get(),
                memory_gb_seconds: self.memory_gb_seconds.get(),
                durable_storage_byte_seconds: self.durable_storage_byte_seconds.get(),
                ephemeral_storage_byte_seconds: self.ephemeral_storage_byte_seconds.get(),
            },
        })
    }
}

pub fn monthly_usage_mode(value: &str) -> RepoResult<MonthlyUsageMode> {
    match value {
        "hard_limit" => Ok(MonthlyUsageMode::HardLimit),
        "allow_overage" => Ok(MonthlyUsageMode::AllowOverage),
        _ => Err(RepoError::InternalError(anyhow::anyhow!(
            "Unknown persisted monthly usage mode: {value}"
        ))),
    }
}

pub fn monthly_usage_mode_str(value: MonthlyUsageMode) -> &'static str {
    match value {
        MonthlyUsageMode::HardLimit => "hard_limit",
        MonthlyUsageMode::AllowOverage => "allow_overage",
    }
}

pub fn monthly_usage_mode_transition_source(
    value: &str,
) -> RepoResult<MonthlyUsageModeTransitionSource> {
    match value {
        "owner" => Ok(MonthlyUsageModeTransitionSource::Owner),
        "administrator" => Ok(MonthlyUsageModeTransitionSource::Administrator),
        "plan_eligibility_removed" => Ok(MonthlyUsageModeTransitionSource::PlanEligibilityRemoved),
        "ineligible_plan_assigned" => Ok(MonthlyUsageModeTransitionSource::IneligiblePlanAssigned),
        _ => Err(RepoError::InternalError(anyhow::anyhow!(
            "Unknown persisted monthly usage mode transition source: {value}"
        ))),
    }
}

pub fn monthly_usage_mode_transition_source_str(
    value: MonthlyUsageModeTransitionSource,
) -> &'static str {
    match value {
        MonthlyUsageModeTransitionSource::Owner => "owner",
        MonthlyUsageModeTransitionSource::Administrator => "administrator",
        MonthlyUsageModeTransitionSource::PlanEligibilityRemoved => "plan_eligibility_removed",
        MonthlyUsageModeTransitionSource::IneligiblePlanAssigned => "ineligible_plan_assigned",
    }
}

impl AccountUsageRecord {
    pub fn new(period: AccountUsagePeriod) -> Self {
        Self {
            period,
            as_of: None,
            durable_storage_byte_seconds: 0,
            ephemeral_storage_byte_seconds: 0,
            compute_fuel: 0,
            memory_gb_seconds: 0,
            metering: None,
        }
    }

    pub fn apply(&mut self, usage_type: UsageType, value: u64, updated_at: DateTime<Utc>) {
        if self.as_of.is_none_or(|as_of| updated_at > as_of) {
            self.as_of = Some(updated_at);
        }
        match usage_type {
            UsageType::MonthlyDurableAgentStorageByteSeconds => {
                self.durable_storage_byte_seconds = value
            }
            UsageType::MonthlyEphemeralStorageByteSeconds => {
                self.ephemeral_storage_byte_seconds = value
            }
            UsageType::MonthlyGasLimit => self.compute_fuel = value,
            UsageType::MonthlyMemoryGbSeconds => self.memory_gb_seconds = value,
            _ => {}
        }
    }

    pub fn apply_metering(
        &mut self,
        compute: bool,
        memory: bool,
        filesystem: bool,
        updated_at: DateTime<Utc>,
    ) {
        if self.as_of.is_none_or(|as_of| updated_at > as_of) {
            self.as_of = Some(updated_at);
        }
        self.metering = Some(ResourceUsageMetering {
            compute,
            memory,
            filesystem,
        });
    }
}

impl AccountUsage {
    pub fn monthly_plan_amounts(&self) -> MonthlyPlanAmounts {
        MonthlyPlanAmounts {
            compute_gcu: self
                .admin_grant_values
                .monthly_compute_gcu
                .unwrap_or_else(|| self.plan.monthly_compute_gcu.get()),
            memory_gb_seconds: self
                .admin_grant_values
                .monthly_memory_gb_seconds
                .unwrap_or_else(|| self.plan.monthly_memory_gb_seconds.get()),
            durable_storage_gb_month: self
                .admin_grant_values
                .monthly_durable_storage_gb_month
                .unwrap_or_else(|| self.plan.monthly_durable_storage_gb_month.get()),
            ephemeral_storage_gb_month: self
                .admin_grant_values
                .monthly_ephemeral_storage_gb_month
                .unwrap_or_else(|| self.plan.monthly_ephemeral_storage_gb_month.get()),
        }
    }

    pub fn usage(&self, usage_type: UsageType) -> u64 {
        self.usage.get(&usage_type).copied().unwrap_or(0)
    }

    pub fn change(&self, usage_type: UsageType) -> i64 {
        self.changes.get(&usage_type).copied().unwrap_or(0)
    }

    pub fn final_value(&self, usage_type: UsageType) -> u64 {
        let base = self.usage(usage_type);
        let delta = self.change(usage_type);

        if delta >= 0 {
            // Safe addition, clamp at u64::MAX
            base.saturating_add(delta as u64)
        } else {
            // Safe subtraction, clamp at 0
            let delta_abs = delta.unsigned_abs();
            base.saturating_sub(delta_abs)
        }
    }

    pub fn add_change(&mut self, usage_type: UsageType, change: i64) -> bool {
        self.changes
            .entry(usage_type)
            .and_modify(|e| *e = e.saturating_add(change))
            .or_insert(change);

        self.final_value(usage_type) <= self.plan.limit(usage_type)
    }

    pub fn resource_limits(&self) -> Result<ResourceLimits, MonthlyPlanAmountError> {
        let monthly_amounts = self.monthly_plan_amounts().resolve()?;
        let available_fuel = monthly_amounts
            .compute_fuel
            .saturating_sub(self.final_value(UsageType::MonthlyGasLimit));
        let available_memory_byte_nanoseconds = (monthly_amounts.memory_gb_seconds as u128)
            .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
            .saturating_sub(
                (self.final_value(UsageType::MonthlyMemoryGbSeconds) as u128)
                    .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                    .saturating_add(self.monthly_memory_byte_nanoseconds_remainder),
            );
        let available_memory_gb_seconds = (available_memory_byte_nanoseconds
            / BYTE_NANOSECONDS_PER_GB_SECOND)
            .min(u64::MAX as u128) as u64;
        let available_memory_byte_nanoseconds_remainder =
            (available_memory_byte_nanoseconds % BYTE_NANOSECONDS_PER_GB_SECOND) as u64;
        let available_durable_storage_byte_nanoseconds =
            (monthly_amounts.durable_storage_byte_seconds as u128)
                .saturating_mul(1_000_000_000)
                .saturating_sub(
                    (self.final_value(UsageType::MonthlyDurableAgentStorageByteSeconds) as u128)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(self.monthly_durable_storage_byte_nanoseconds_remainder),
                );
        let available_ephemeral_storage_byte_nanoseconds =
            (monthly_amounts.ephemeral_storage_byte_seconds as u128)
                .saturating_mul(1_000_000_000)
                .saturating_sub(
                    (self.final_value(UsageType::MonthlyEphemeralStorageByteSeconds) as u128)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(self.monthly_ephemeral_storage_byte_nanoseconds_remainder),
                );

        let http_limit = self.plan.limit(UsageType::MonthlyHttpCalls);
        let available_http_calls =
            http_limit.saturating_sub(self.final_value(UsageType::MonthlyHttpCalls));

        let rpc_limit = self.plan.limit(UsageType::MonthlyRpcCalls);
        let available_rpc_calls =
            rpc_limit.saturating_sub(self.final_value(UsageType::MonthlyRpcCalls));

        Ok(ResourceLimits {
            monthly_usage_mode_revision: self.monthly_usage_mode_revision,
            monthly_policy: MonthlyResourcePolicy {
                period: AccountUsagePeriod {
                    year: self.year,
                    month: self.month,
                },
                mode: self.monthly_usage_mode,
                available_fuel,
                available_memory_gb_seconds,
                available_memory_byte_nanoseconds_remainder,
                available_durable_storage_byte_seconds: (available_durable_storage_byte_nanoseconds
                    / 1_000_000_000)
                    .min(u64::MAX as u128)
                    as u64,
                available_durable_storage_byte_nanoseconds_remainder:
                    (available_durable_storage_byte_nanoseconds % 1_000_000_000) as u64,
                available_ephemeral_storage_byte_seconds:
                    (available_ephemeral_storage_byte_nanoseconds / 1_000_000_000)
                        .min(u64::MAX as u128) as u64,
                available_ephemeral_storage_byte_nanoseconds_remainder:
                    (available_ephemeral_storage_byte_nanoseconds % 1_000_000_000) as u64,
            },
            max_memory_per_worker: self.max_memory_per_worker.effective_value,
            max_table_elements_per_worker: self.plan.max_table_elements_per_worker.get(),
            max_disk_space_per_worker: self.storage_limit.executor_value(),
            per_invocation_http_call_limit: self.plan.per_invocation_http_call_limit.get(),
            per_invocation_rpc_call_limit: self.plan.per_invocation_rpc_call_limit.get(),
            available_http_calls,
            available_rpc_calls,
            max_concurrent_agents_per_executor: self.plan.max_concurrent_agents_per_executor.get(),
            oplog_writes_per_second: self.plan.oplog_writes_per_second.get(),
            usage_update_applied: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccountUsage, AccountUsageRecord, AdminResourceGrantValues, MonthlyUsageModeStateRecord,
        MonthlyUsageModeTransitionRecord, UsageType, monthly_usage_mode,
        monthly_usage_mode_transition_source,
    };
    use crate::repo::model::plan::PlanRecord;
    use chrono::{DateTime, Utc};
    use golem_common::model::account_usage::{
        AccountUsagePeriod, MemoryLimit, MonthlyUsageMode, MonthlyUsageModeTransitionSource,
        StorageLimit,
    };
    use golem_service_base::repo::{NumericU64, SqlDateTime};
    use std::collections::BTreeMap;
    use test_r::test;
    use uuid::Uuid;

    fn timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    #[test]
    fn account_usage_record_keeps_latest_usage_timestamp() {
        let mut record = AccountUsageRecord::new(AccountUsagePeriod {
            year: 2026,
            month: 4,
        });

        record.apply(UsageType::MonthlyGasLimit, 1, timestamp(100));
        record.apply(UsageType::MonthlyMemoryGbSeconds, 2, timestamp(200));
        record.apply(
            UsageType::MonthlyDurableAgentStorageByteSeconds,
            3,
            timestamp(150),
        );

        assert_eq!(record.as_of, Some(timestamp(200)));
    }

    #[test]
    fn account_usage_record_maps_storage_usage_by_class() {
        let mut record = AccountUsageRecord::new(AccountUsagePeriod {
            year: 2026,
            month: 4,
        });

        record.apply(
            UsageType::MonthlyDurableAgentStorageByteSeconds,
            17,
            timestamp(100),
        );
        record.apply(
            UsageType::MonthlyEphemeralStorageByteSeconds,
            29,
            timestamp(100),
        );

        assert_eq!(record.durable_storage_byte_seconds, 17);
        assert_eq!(record.ephemeral_storage_byte_seconds, 29);
    }

    #[test]
    fn resource_limits_preserve_storage_whole_seconds_and_remainders() {
        let plan = PlanRecord {
            plan_id: Uuid::new_v4(),
            name: "storage-conversion".to_string(),
            max_memory_per_worker: NumericU64::new(u64::MAX),
            max_memory_per_worker_ceiling: NumericU64::new(u64::MAX),
            max_memory_per_worker_user_configurable: false,
            monthly_compute_gcu: NumericU64::new(0),
            monthly_memory_gb_seconds: NumericU64::new(u64::MAX),
            monthly_durable_storage_gb_month: NumericU64::new(1),
            monthly_ephemeral_storage_gb_month: NumericU64::new(2),
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
            total_component_storage_bytes: NumericU64::new(u64::MAX),
            monthly_gas_limit: NumericU64::new(u64::MAX),
            monthly_component_upload_limit_bytes: NumericU64::new(u64::MAX),
            per_invocation_http_call_limit: NumericU64::new(u64::MAX),
            per_invocation_rpc_call_limit: NumericU64::new(u64::MAX),
            monthly_http_call_limit: NumericU64::new(u64::MAX),
            monthly_rpc_call_limit: NumericU64::new(u64::MAX),
            oplog_writes_per_second: NumericU64::new(u64::MAX),
        };
        let mut usage_values = BTreeMap::new();
        usage_values.insert(UsageType::MonthlyDurableAgentStorageByteSeconds, 3);
        usage_values.insert(UsageType::MonthlyEphemeralStorageByteSeconds, 5);
        let usage = AccountUsage {
            account_id: Uuid::new_v4(),
            year: 2026,
            month: 4,
            usage: usage_values,
            plan,
            storage_limit: StorageLimit::resolve(false, 0, None, 0, false),
            max_memory_per_worker: MemoryLimit::resolve(u64::MAX, None, u64::MAX, false),
            admin_grant_values: AdminResourceGrantValues::default(),
            admin_grants: Vec::new(),
            metering: None,
            monthly_usage_mode: MonthlyUsageMode::HardLimit,
            monthly_usage_mode_revision: 0,
            monthly_memory_byte_nanoseconds_remainder: 0,
            monthly_durable_storage_byte_nanoseconds_remainder: 400_000_000,
            monthly_ephemeral_storage_byte_nanoseconds_remainder: 250_000_000,
            monthly_usage_attribution: None,
            changes: BTreeMap::new(),
        };

        let policy = usage.resource_limits().unwrap().monthly_policy;
        let durable_total = usage
            .monthly_plan_amounts()
            .resolve()
            .unwrap()
            .durable_storage_byte_seconds;
        let ephemeral_total = usage
            .monthly_plan_amounts()
            .resolve()
            .unwrap()
            .ephemeral_storage_byte_seconds;
        assert_eq!(
            policy.available_durable_storage_byte_seconds,
            durable_total - 4
        );
        assert_eq!(
            policy.available_durable_storage_byte_nanoseconds_remainder,
            600_000_000
        );
        assert_eq!(
            policy.available_ephemeral_storage_byte_seconds,
            ephemeral_total - 6
        );
        assert_eq!(
            policy.available_ephemeral_storage_byte_nanoseconds_remainder,
            750_000_000
        );
    }

    #[test]
    fn account_usage_record_keeps_latest_metering_timestamp() {
        let mut record = AccountUsageRecord::new(AccountUsagePeriod {
            year: 2026,
            month: 4,
        });

        record.apply_metering(true, true, true, timestamp(100));
        record.apply_metering(false, false, false, timestamp(200));
        record.apply_metering(true, false, true, timestamp(150));

        assert_eq!(record.as_of, Some(timestamp(200)));
    }

    #[test]
    fn unknown_persisted_transition_source_is_rejected() {
        assert!(monthly_usage_mode_transition_source("unknown").is_err());
    }

    #[test]
    fn persisted_transition_sources_are_parsed_explicitly() {
        for (persisted, expected) in [
            ("owner", MonthlyUsageModeTransitionSource::Owner),
            (
                "administrator",
                MonthlyUsageModeTransitionSource::Administrator,
            ),
            (
                "plan_eligibility_removed",
                MonthlyUsageModeTransitionSource::PlanEligibilityRemoved,
            ),
            (
                "ineligible_plan_assigned",
                MonthlyUsageModeTransitionSource::IneligiblePlanAssigned,
            ),
        ] {
            assert_eq!(
                monthly_usage_mode_transition_source(persisted).unwrap(),
                expected
            );
        }
    }

    fn transition_record(previous_mode: &str, new_mode: &str) -> MonthlyUsageModeTransitionRecord {
        MonthlyUsageModeTransitionRecord {
            revision: NumericU64::new(1),
            actor_account_id: Uuid::nil(),
            changed_at: SqlDateTime::new(timestamp(100)),
            source: "owner".to_string(),
            previous_mode: previous_mode.to_string(),
            new_mode: new_mode.to_string(),
            period_year: 2026,
            period_month: 4,
            compute_fuel: NumericU64::new(0),
            memory_gb_seconds: NumericU64::new(0),
            durable_storage_byte_seconds: NumericU64::new(0),
            ephemeral_storage_byte_seconds: NumericU64::new(0),
        }
    }

    #[test]
    fn persisted_monthly_usage_modes_are_parsed_explicitly() {
        assert_eq!(
            monthly_usage_mode("hard_limit").unwrap(),
            MonthlyUsageMode::HardLimit
        );
        assert_eq!(
            monthly_usage_mode("allow_overage").unwrap(),
            MonthlyUsageMode::AllowOverage
        );
    }

    #[test]
    fn ineligible_allow_overage_state_falls_back_to_hard_limit() {
        let state = MonthlyUsageModeStateRecord {
            mode: "allow_overage".to_string(),
            revision: NumericU64::new(1),
            overage_eligible: false,
        };

        assert_eq!(state.mode().unwrap(), MonthlyUsageMode::HardLimit);
    }

    #[test]
    fn eligible_allow_overage_state_is_preserved() {
        let state = MonthlyUsageModeStateRecord {
            mode: "allow_overage".to_string(),
            revision: NumericU64::new(1),
            overage_eligible: true,
        };

        assert_eq!(state.mode().unwrap(), MonthlyUsageMode::AllowOverage);
    }

    #[test]
    fn unknown_persisted_current_monthly_usage_mode_is_rejected() {
        let state = MonthlyUsageModeStateRecord {
            mode: "unknown".to_string(),
            revision: NumericU64::new(1),
            overage_eligible: false,
        };

        assert!(state.mode().is_err());
    }

    #[test]
    fn unknown_persisted_previous_monthly_usage_mode_is_rejected() {
        assert!(
            transition_record("unknown", "hard_limit")
                .into_model()
                .is_err()
        );
    }

    #[test]
    fn unknown_persisted_new_monthly_usage_mode_is_rejected() {
        assert!(
            transition_record("hard_limit", "unknown")
                .into_model()
                .is_err()
        );
    }
}
