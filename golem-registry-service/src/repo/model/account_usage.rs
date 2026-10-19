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

use crate::repo::model::account_resource_override::{
    AccountResourceOverrideDimension, persisted_admin_reason,
};
use crate::repo::model::plan::PlanRecord;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    AccountUsagePeriod, AdminResourceGrant, AdminResourceGrantDimension,
    BYTE_NANOSECONDS_PER_GB_SECOND, MonthlyPlanAmountError, MonthlyPlanAmounts, MonthlyUsageMode,
    MonthlyUsageModeTransition, MonthlyUsageModeTransitionSource, ResolvedMonthlyPlanAmounts,
    StorageLimit,
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
    TotalBlobStorageBytes = 13,
}

impl UsageType {
    pub fn grouping(&self) -> UsageGrouping {
        match self {
            UsageType::TotalAppCount
            | UsageType::TotalEnvCount
            | UsageType::TotalComponentCount
            | UsageType::TotalWorkerConnectionCount
            | UsageType::TotalComponentStorageBytes => UsageGrouping::Total,
            UsageType::TotalBlobStorageBytes => UsageGrouping::Total,
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
            UsageType::TotalBlobStorageBytes => UsageTracking::Stats,
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
    pub max_disk_space_per_worker_value: u64,
    pub max_memory_per_worker_value: u64,
    pub admin_grant_values: AdminResourceGrantValues,
    pub admin_grants: Vec<AdminResourceGrant>,
    pub metering: Option<ResourceUsageMetering>,
    pub monthly_usage_mode: MonthlyUsageMode,
    pub monthly_usage_mode_revision: u64,
    pub monthly_policy_revision: u64,
    pub monthly_memory_byte_nanoseconds_remainder: u128,
    pub monthly_durable_storage_byte_nanoseconds_remainder: u128,
    pub monthly_ephemeral_storage_byte_nanoseconds_remainder: u128,
    pub monthly_usage_attribution: Option<MonthlyUsageAttribution>,
    pub changes: BTreeMap<UsageType, i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MonthlyUsageAttribution {
    pub policy_revision: u64,
    pub memory_byte_nanoseconds_remainder: u64,
    pub durable_storage_byte_nanoseconds_remainder: u64,
    pub ephemeral_storage_byte_nanoseconds_remainder: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyPolicySnapshot {
    pub mode: MonthlyUsageMode,
    pub resolved: ResolvedMonthlyPlanAmounts,
}

pub fn exact_usage_delta(whole: i64, remainder: u64, units_per_whole: u128) -> i128 {
    (whole as i128)
        .saturating_mul(units_per_whole.min(i128::MAX as u128) as i128)
        .saturating_add(remainder as i128)
}

pub fn billable_excess_delta(baseline: u128, allowance: u128, delta: i128) -> i128 {
    let baseline = baseline.min(i128::MAX as u128) as i128;
    let allowance = allowance.min(i128::MAX as u128) as i128;
    let final_usage = baseline.saturating_add(delta).max(0);
    final_usage
        .saturating_sub(allowance)
        .max(0)
        .saturating_sub(baseline.saturating_sub(allowance).max(0))
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

impl AdminResourceGrantValues {
    fn monthly_plan_amounts(&self, plan: &PlanRecord) -> MonthlyPlanAmounts {
        MonthlyPlanAmounts {
            compute_gcu: plan
                .monthly_compute_gcu
                .get()
                .max(self.monthly_compute_gcu.unwrap_or(0)),
            memory_gb_seconds: plan
                .monthly_memory_gb_seconds
                .get()
                .max(self.monthly_memory_gb_seconds.unwrap_or(0)),
            durable_storage_gb_month: plan
                .monthly_durable_storage_gb_month
                .get()
                .max(self.monthly_durable_storage_gb_month.unwrap_or(0)),
            ephemeral_storage_gb_month: plan
                .monthly_ephemeral_storage_gb_month
                .get()
                .max(self.monthly_ephemeral_storage_gb_month.unwrap_or(0)),
        }
    }
}

fn resolved_per_agent_value(
    plan_default: u64,
    override_value: Option<u64>,
    ceiling: u64,
    user_configurable: bool,
) -> u64 {
    override_value
        .filter(|_| user_configurable)
        .filter(|value| (plan_default..=ceiling).contains(value))
        .unwrap_or(plan_default)
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

    pub fn max_memory_per_worker_value(&self) -> u64 {
        AccountResourceOverrideDimension::MaxMemoryPerWorker.resolve_grant_value(
            resolved_per_agent_value(
                self.plan.max_memory_per_worker.get(),
                self.max_memory_override_value.as_ref().map(NumericU64::get),
                self.plan.max_memory_per_worker_ceiling.get(),
                self.plan.max_memory_per_worker_user_configurable,
            ),
            self.max_memory_grant_value.as_ref().map(NumericU64::get),
        )
    }

    pub fn max_disk_space_per_worker_value(&self) -> u64 {
        if self.plan.max_disk_space_per_worker_enabled {
            AccountResourceOverrideDimension::MaxDiskSpacePerWorker.resolve_grant_value(
                resolved_per_agent_value(
                    self.plan.max_disk_space_per_worker.get(),
                    self.storage_override_value.as_ref().map(NumericU64::get),
                    self.plan.max_disk_space_per_worker_ceiling.get(),
                    self.plan.max_disk_space_per_worker_user_configurable,
                ),
                self.storage_grant_value.as_ref().map(NumericU64::get),
            )
        } else {
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT
        }
    }

    pub fn monthly_policy_snapshot(
        &self,
        metering: ResourceUsageMetering,
    ) -> RepoResult<MonthlyPolicySnapshot> {
        let grants = self.admin_grant_values();
        Ok(MonthlyPolicySnapshot {
            mode: self.monthly_usage_mode()?,
            resolved: enabled_monthly_amounts(grants.monthly_plan_amounts(&self.plan), metering)
                .resolve()
                .map_err(|error| RepoError::InternalError(error.into()))?,
        })
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
    pub allow_overage_compute_fuel: u64,
    pub allow_overage_memory_byte_nanoseconds: u128,
    pub allow_overage_durable_storage_byte_nanoseconds: u128,
    pub allow_overage_ephemeral_storage_byte_nanoseconds: u128,
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
            allow_overage_compute_fuel: 0,
            allow_overage_memory_byte_nanoseconds: 0,
            allow_overage_durable_storage_byte_nanoseconds: 0,
            allow_overage_ephemeral_storage_byte_nanoseconds: 0,
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

fn enabled_monthly_amounts(
    mut amounts: MonthlyPlanAmounts,
    metering: ResourceUsageMetering,
) -> MonthlyPlanAmounts {
    if !metering.compute {
        amounts.compute_gcu = 0;
    }
    if !metering.memory {
        amounts.memory_gb_seconds = 0;
    }
    if !metering.filesystem {
        amounts.durable_storage_gb_month = 0;
        amounts.ephemeral_storage_gb_month = 0;
    }
    amounts
}

impl AccountUsage {
    pub fn monthly_policy_snapshot(&self) -> Result<MonthlyPolicySnapshot, MonthlyPlanAmountError> {
        Ok(MonthlyPolicySnapshot {
            mode: self.monthly_usage_mode,
            resolved: self.monthly_plan_amounts().resolve()?,
        })
    }

    pub fn plan_row_monthly_amounts(&self) -> MonthlyPlanAmounts {
        MonthlyPlanAmounts {
            compute_gcu: self.plan.monthly_compute_gcu.get(),
            memory_gb_seconds: self.plan.monthly_memory_gb_seconds.get(),
            durable_storage_gb_month: self.plan.monthly_durable_storage_gb_month.get(),
            ephemeral_storage_gb_month: self.plan.monthly_ephemeral_storage_gb_month.get(),
        }
    }

    pub fn monthly_plan_amounts(&self) -> MonthlyPlanAmounts {
        self.admin_grant_values.monthly_plan_amounts(&self.plan)
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

    pub fn resource_limits(
        &self,
        metering: ResourceUsageMetering,
    ) -> Result<ResourceLimits, MonthlyPlanAmountError> {
        let monthly_amounts =
            enabled_monthly_amounts(self.monthly_plan_amounts(), metering).resolve()?;
        let available_fuel = if metering.compute {
            monthly_amounts
                .compute_fuel
                .saturating_sub(self.final_value(UsageType::MonthlyGasLimit))
        } else {
            u64::MAX
        };
        let available_memory_byte_nanoseconds = if metering.memory {
            (monthly_amounts.memory_gb_seconds as u128)
                .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                .saturating_sub(
                    (self.final_value(UsageType::MonthlyMemoryGbSeconds) as u128)
                        .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                        .saturating_add(self.monthly_memory_byte_nanoseconds_remainder),
                )
        } else {
            (u64::MAX as u128) * BYTE_NANOSECONDS_PER_GB_SECOND
        };
        let available_memory_gb_seconds = (available_memory_byte_nanoseconds
            / BYTE_NANOSECONDS_PER_GB_SECOND)
            .min(u64::MAX as u128) as u64;
        let available_memory_byte_nanoseconds_remainder =
            (available_memory_byte_nanoseconds % BYTE_NANOSECONDS_PER_GB_SECOND) as u64;
        let available_durable_storage_byte_nanoseconds = if metering.filesystem {
            (monthly_amounts.durable_storage_byte_seconds as u128)
                .saturating_mul(1_000_000_000)
                .saturating_sub(
                    (self.final_value(UsageType::MonthlyDurableAgentStorageByteSeconds) as u128)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(self.monthly_durable_storage_byte_nanoseconds_remainder),
                )
        } else {
            (u64::MAX as u128) * 1_000_000_000
        };
        let available_ephemeral_storage_byte_nanoseconds = if metering.filesystem {
            (monthly_amounts.ephemeral_storage_byte_seconds as u128)
                .saturating_mul(1_000_000_000)
                .saturating_sub(
                    (self.final_value(UsageType::MonthlyEphemeralStorageByteSeconds) as u128)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(self.monthly_ephemeral_storage_byte_nanoseconds_remainder),
                )
        } else {
            (u64::MAX as u128) * 1_000_000_000
        };

        let http_limit = self.plan.limit(UsageType::MonthlyHttpCalls);
        let available_http_calls =
            http_limit.saturating_sub(self.final_value(UsageType::MonthlyHttpCalls));

        let rpc_limit = self.plan.limit(UsageType::MonthlyRpcCalls);
        let available_rpc_calls =
            rpc_limit.saturating_sub(self.final_value(UsageType::MonthlyRpcCalls));

        let blob_storage_limit = self.plan.limit(UsageType::TotalBlobStorageBytes);
        let available_blob_storage_bytes =
            blob_storage_limit.saturating_sub(self.final_value(UsageType::TotalBlobStorageBytes));

        Ok(ResourceLimits {
            monthly_usage_mode_revision: self.monthly_usage_mode_revision,
            monthly_policy_revision: self.monthly_policy_revision,
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
            max_memory_per_worker: self.max_memory_per_worker_value,
            max_table_elements_per_worker: self.plan.max_table_elements_per_worker.get(),
            max_disk_space_per_worker: self.max_disk_space_per_worker_value,
            per_invocation_http_call_limit: self.plan.per_invocation_http_call_limit.get(),
            per_invocation_rpc_call_limit: self.plan.per_invocation_rpc_call_limit.get(),
            available_http_calls,
            available_rpc_calls,
            available_blob_storage_bytes,
            max_concurrent_agents_per_executor: self.plan.max_concurrent_agents_per_executor.get(),
            oplog_writes_per_second: self.plan.oplog_writes_per_second.get(),
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccountUsage, AccountUsagePlan, AccountUsageRecord, AdminResourceGrantValues,
        MonthlyUsageModeStateRecord, MonthlyUsageModeTransitionRecord, UsageType,
        billable_excess_delta, exact_usage_delta, monthly_usage_mode,
        monthly_usage_mode_transition_source, resolved_per_agent_value,
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

    fn test_plan() -> PlanRecord {
        PlanRecord {
            total_blob_storage_bytes: u64::MAX.into(),
            plan_id: Uuid::new_v4(),
            name: "resource-limits".to_string(),
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
            monthly_component_upload_limit_bytes: NumericU64::new(u64::MAX),
            per_invocation_http_call_limit: NumericU64::new(u64::MAX),
            per_invocation_rpc_call_limit: NumericU64::new(u64::MAX),
            monthly_http_call_limit: NumericU64::new(u64::MAX),
            monthly_rpc_call_limit: NumericU64::new(u64::MAX),
            oplog_writes_per_second: NumericU64::new(u64::MAX),
        }
    }

    fn account_usage_plan(plan: PlanRecord) -> AccountUsagePlan {
        AccountUsagePlan {
            plan,
            storage_override_value: None,
            max_memory_override_value: None,
            monthly_compute_grant_value: None,
            monthly_compute_grant_reason: None,
            monthly_compute_grant_expires_at: None,
            monthly_compute_grant_created_by: None,
            monthly_compute_grant_created_at: None,
            monthly_memory_grant_value: None,
            monthly_memory_grant_reason: None,
            monthly_memory_grant_expires_at: None,
            monthly_memory_grant_created_by: None,
            monthly_memory_grant_created_at: None,
            monthly_durable_storage_grant_value: None,
            monthly_durable_storage_grant_reason: None,
            monthly_durable_storage_grant_expires_at: None,
            monthly_durable_storage_grant_created_by: None,
            monthly_durable_storage_grant_created_at: None,
            monthly_ephemeral_storage_grant_value: None,
            monthly_ephemeral_storage_grant_reason: None,
            monthly_ephemeral_storage_grant_expires_at: None,
            monthly_ephemeral_storage_grant_created_by: None,
            monthly_ephemeral_storage_grant_created_at: None,
            max_memory_grant_value: None,
            max_memory_grant_reason: None,
            max_memory_grant_expires_at: None,
            max_memory_grant_created_by: None,
            max_memory_grant_created_at: None,
            storage_grant_value: None,
            storage_grant_reason: None,
            storage_grant_expires_at: None,
            storage_grant_created_by: None,
            storage_grant_created_at: None,
            monthly_usage_mode: "hard_limit".to_string(),
            monthly_usage_mode_revision: NumericU64::new(0),
        }
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
    fn exact_usage_delta_preserves_fractional_refunds() {
        assert_eq!(exact_usage_delta(1, 250, 1_000), 1_250);
        assert_eq!(exact_usage_delta(-1, 250, 1_000), -750);
    }

    #[test]
    fn billable_excess_delta_crosses_and_refunds_the_threshold() {
        assert_eq!(billable_excess_delta(90, 100, 20), 10);
        assert_eq!(billable_excess_delta(150, 100, -100), -50);
        assert_eq!(billable_excess_delta(90, 100, -20), 0);
    }

    #[test]
    fn per_agent_resolution_preserves_internal_unlimited_values() {
        for value in [
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_MEMORY_LIMIT,
            u64::MAX,
        ] {
            assert_eq!(resolved_per_agent_value(value, None, value, false), value);
        }
    }

    #[test]
    fn per_agent_grants_never_reduce_plan_or_owner_limits() {
        let mut plan = test_plan();
        plan.max_memory_per_worker = 100.into();
        plan.max_memory_per_worker_ceiling = 1000.into();
        plan.max_memory_per_worker_user_configurable = true;
        plan.max_disk_space_per_worker_enabled = true;
        plan.max_disk_space_per_worker = 100.into();
        plan.max_disk_space_per_worker_ceiling = 1000.into();
        plan.max_disk_space_per_worker_user_configurable = true;
        let mut current = account_usage_plan(plan);
        current.max_memory_grant_value = Some(300.into());
        current.storage_grant_value = Some(300.into());
        assert_eq!(current.max_memory_per_worker_value(), 300);
        assert_eq!(current.max_disk_space_per_worker_value(), 300);

        current.plan.max_memory_per_worker = 400.into();
        current.plan.max_disk_space_per_worker = 400.into();
        assert_eq!(current.max_memory_per_worker_value(), 400);
        assert_eq!(current.max_disk_space_per_worker_value(), 400);
        current.max_memory_override_value = Some(500.into());
        current.storage_override_value = Some(500.into());
        assert_eq!(current.max_memory_per_worker_value(), 500);
        assert_eq!(current.max_disk_space_per_worker_value(), 500);

        for unlimited in [
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_MEMORY_LIMIT,
            u64::MAX,
        ] {
            current.plan.max_memory_per_worker = unlimited.into();
            current.plan.max_memory_per_worker_ceiling = unlimited.into();
            assert_eq!(current.max_memory_per_worker_value(), unlimited);
        }
        current.plan.max_disk_space_per_worker_enabled = false;
        assert_eq!(
            current.max_disk_space_per_worker_value(),
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT
        );
    }

    #[test]
    fn monthly_grants_never_reduce_an_upgraded_plan_snapshot() {
        use golem_common::model::account_usage::MonthlyPlanAmounts;
        use golem_service_base::clients::registry::ResourceUsageMetering;

        let mut plan = test_plan();
        plan.monthly_compute_gcu = 5.into();
        plan.monthly_memory_gb_seconds = 50.into();
        plan.monthly_durable_storage_gb_month = 7.into();
        plan.monthly_ephemeral_storage_gb_month = 11.into();
        let mut current = account_usage_plan(plan);
        current.monthly_compute_grant_value = Some(8.into());
        current.monthly_memory_grant_value = Some(70.into());
        current.monthly_durable_storage_grant_value = Some(9.into());
        current.monthly_ephemeral_storage_grant_value = Some(12.into());
        assert_eq!(
            current
                .monthly_policy_snapshot(ResourceUsageMetering::all_enabled())
                .unwrap()
                .resolved,
            MonthlyPlanAmounts {
                compute_gcu: 8,
                memory_gb_seconds: 70,
                durable_storage_gb_month: 9,
                ephemeral_storage_gb_month: 12
            }
            .resolve()
            .unwrap()
        );
        current.plan.monthly_compute_gcu = 10.into();
        current.plan.monthly_memory_gb_seconds = 80.into();
        current.plan.monthly_durable_storage_gb_month = 10.into();
        current.plan.monthly_ephemeral_storage_gb_month = 13.into();
        assert_eq!(
            current
                .monthly_policy_snapshot(ResourceUsageMetering::all_enabled())
                .unwrap()
                .resolved,
            MonthlyPlanAmounts {
                compute_gcu: 10,
                memory_gb_seconds: 80,
                durable_storage_gb_month: 10,
                ephemeral_storage_gb_month: 13
            }
            .resolve()
            .unwrap()
        );
        let disabled = current
            .monthly_policy_snapshot(ResourceUsageMetering {
                compute: false,
                memory: false,
                filesystem: false,
            })
            .unwrap();
        assert_eq!(disabled.mode, MonthlyUsageMode::HardLimit);
        assert_eq!(
            disabled.resolved,
            MonthlyPlanAmounts {
                compute_gcu: 0,
                memory_gb_seconds: 0,
                durable_storage_gb_month: 0,
                ephemeral_storage_gb_month: 0
            }
            .resolve()
            .unwrap()
        );
    }

    #[test]
    fn account_usage_plan_resolves_raw_per_agent_values() {
        let mut plan = test_plan();
        plan.max_memory_per_worker_user_configurable = true;
        plan.max_disk_space_per_worker_enabled = true;
        plan.max_disk_space_per_worker = NumericU64::new(50);
        plan.max_disk_space_per_worker_ceiling = NumericU64::new(200);
        plan.max_disk_space_per_worker_user_configurable = true;
        let mut account_plan = account_usage_plan(plan);

        assert_eq!(account_plan.max_memory_per_worker_value(), u64::MAX);
        assert_eq!(account_plan.max_disk_space_per_worker_value(), 50);

        account_plan.plan.max_memory_per_worker = NumericU64::new(50);
        account_plan.max_memory_override_value = Some(NumericU64::new(75));
        account_plan.storage_override_value = Some(NumericU64::new(75));
        assert_eq!(account_plan.max_memory_per_worker_value(), 75);
        assert_eq!(account_plan.max_disk_space_per_worker_value(), 75);

        account_plan.max_memory_grant_value = Some(NumericU64::new(125));
        account_plan.storage_grant_value = Some(NumericU64::new(150));
        assert_eq!(account_plan.max_memory_per_worker_value(), 125);
        assert_eq!(account_plan.max_disk_space_per_worker_value(), 150);

        account_plan.plan.max_disk_space_per_worker_enabled = false;
        assert_eq!(
            account_plan.max_disk_space_per_worker_value(),
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT
        );
    }

    #[test]
    fn resource_limits_preserve_internal_per_agent_values_and_storage_remainders() {
        let plan = test_plan();
        let mut usage_values = BTreeMap::new();
        usage_values.insert(UsageType::MonthlyDurableAgentStorageByteSeconds, 3);
        usage_values.insert(UsageType::MonthlyEphemeralStorageByteSeconds, 5);
        let mut usage = AccountUsage {
            account_id: Uuid::new_v4(),
            year: 2026,
            month: 4,
            usage: usage_values,
            plan,
            storage_limit: StorageLimit::resolve(false, 0, None, 0, false),
            max_memory_per_worker: MemoryLimit::resolve(u64::MAX, None, u64::MAX, false),
            max_disk_space_per_worker_value:
                golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
            max_memory_per_worker_value: u64::MAX,
            admin_grant_values: AdminResourceGrantValues::default(),
            admin_grants: Vec::new(),
            metering: None,
            monthly_usage_mode: MonthlyUsageMode::HardLimit,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
            monthly_memory_byte_nanoseconds_remainder: 0,
            monthly_durable_storage_byte_nanoseconds_remainder: 400_000_000,
            monthly_ephemeral_storage_byte_nanoseconds_remainder: 250_000_000,
            monthly_usage_attribution: None,
            changes: BTreeMap::new(),
        };

        for compute in [false, true] {
            for memory in [false, true] {
                for filesystem in [false, true] {
                    let limits = usage
                        .resource_limits(
                            golem_service_base::clients::registry::ResourceUsageMetering {
                                compute,
                                memory,
                                filesystem,
                            },
                        )
                        .unwrap();
                    assert_eq!(limits.max_memory_per_worker, u64::MAX);
                    assert_eq!(
                        limits.max_disk_space_per_worker,
                        usage.max_disk_space_per_worker_value
                    );
                    if !compute {
                        assert_eq!(limits.monthly_policy.available_fuel, u64::MAX);
                    }
                    if !memory {
                        assert_eq!(limits.monthly_policy.available_memory_gb_seconds, u64::MAX);
                        assert_eq!(
                            limits
                                .monthly_policy
                                .available_memory_byte_nanoseconds_remainder,
                            0
                        );
                    }
                    if !filesystem {
                        assert_eq!(
                            limits.monthly_policy.available_durable_storage_byte_seconds,
                            u64::MAX
                        );
                        assert_eq!(
                            limits
                                .monthly_policy
                                .available_ephemeral_storage_byte_seconds,
                            u64::MAX
                        );
                        assert_eq!(
                            limits
                                .monthly_policy
                                .available_durable_storage_byte_nanoseconds_remainder,
                            0
                        );
                        assert_eq!(
                            limits
                                .monthly_policy
                                .available_ephemeral_storage_byte_nanoseconds_remainder,
                            0
                        );
                    }
                }
            }
        }

        let policy = usage
            .resource_limits(
                golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
            )
            .unwrap()
            .monthly_policy;
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
        assert_eq!(
            usage
                .resource_limits(
                    golem_service_base::clients::registry::ResourceUsageMetering::all_enabled()
                )
                .unwrap()
                .max_memory_per_worker,
            u64::MAX
        );
        usage.max_memory_per_worker_value =
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_MEMORY_LIMIT;
        assert_eq!(
            usage
                .resource_limits(
                    golem_service_base::clients::registry::ResourceUsageMetering::all_enabled()
                )
                .unwrap()
                .max_memory_per_worker,
            golem_common::model::account_usage::EFFECTIVELY_UNLIMITED_MEMORY_LIMIT
        );

        usage.plan.monthly_memory_gb_seconds = NumericU64::new(10);
        usage.usage.insert(UsageType::MonthlyMemoryGbSeconds, 3);
        usage.monthly_memory_byte_nanoseconds_remainder =
            golem_common::model::account_usage::BYTE_NANOSECONDS_PER_GB_SECOND / 4;
        let memory_policy = usage
            .resource_limits(
                golem_service_base::clients::registry::ResourceUsageMetering::all_enabled(),
            )
            .unwrap()
            .monthly_policy;
        assert_eq!(memory_policy.available_memory_gb_seconds, 6);
        assert_eq!(
            memory_policy.available_memory_byte_nanoseconds_remainder,
            (golem_common::model::account_usage::BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 4) as u64
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
