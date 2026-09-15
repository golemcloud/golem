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

use crate::base_model::account::AccountId;
use crate::{declare_enums, declare_structs, declare_unions};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub const BYTE_SECONDS_PER_GB_MONTH: u64 = 1024 * 1024 * 1024 * 730 * 3600;
pub const BYTE_NANOSECONDS_PER_GB_SECOND: u128 = (1024_u128 * 1024 * 1024) * 1_000_000_000;
pub const FUEL_PER_GCU: u64 = 1_000_000;
pub const DEFAULT_ACCOUNT_USAGE_HISTORY_PERIODS: usize = 6;
pub const EFFECTIVELY_UNLIMITED_STORAGE_LIMIT: u64 = 10_000_000_000_000_000;
pub const EFFECTIVELY_UNLIMITED_MEMORY_LIMIT: u64 = 1_000_000_000_000_000_000;
const PERIOD_FORMAT_ERROR: &str = "period must use YYYY-MM format";

declare_enums! {
    pub enum MeteringStatus {
        Enabled,
        Disabled,
        /// No usage producer has reported the metering state for this period yet.
        Unknown,
    }

}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[display(rename_all = "camelCase")]
pub enum MonthlyUsageMode {
    HardLimit,
    AllowOverage,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[display(rename_all = "camelCase")]
pub enum MonthlyUsageModeTransitionSource {
    Owner,
    Administrator,
    PlanEligibilityRemoved,
    IneligiblePlanAssigned,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[display(rename_all = "camelCase")]
pub enum AdminResourceGrantDimension {
    MonthlyComputeGcu,
    MonthlyMemoryGbSeconds,
    MonthlyDurableStorageGbMonth,
    MonthlyEphemeralStorageGbMonth,
    MaxMemoryPerAgent,
    MaxStoragePerAgent,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[display(rename_all = "camelCase")]
pub enum AdminResourceGrantReason {
    Promotional,
    Support,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
#[display(rename_all = "snake_case")]
pub enum AdminResourceGrantEventType {
    OverrideGranted,
    OverrideCleared,
    OverrideExpired,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[display(rename_all = "camelCase")]
pub enum StorageLimitDisabledReason {
    ManagedFilesystemUnavailable,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, derive_more::Display,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[display(rename_all = "camelCase")]
pub enum MonthlyLimitBehavior {
    HardLimit,
    IncludedAllowance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
pub enum MonthlyComputeUnit {
    #[serde(rename = "GCU")]
    #[cfg_attr(feature = "full", oai(rename = "GCU"))]
    Gcu,
}

impl Display for MonthlyComputeUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("GCU")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
pub enum MonthlyMemoryUnit {
    #[serde(rename = "GB-seconds")]
    #[cfg_attr(feature = "full", oai(rename = "GB-seconds"))]
    GbSeconds,
}

impl Display for MonthlyMemoryUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("GB-seconds")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
pub enum MonthlyStorageUnit {
    #[serde(rename = "GB-month")]
    #[cfg_attr(feature = "full", oai(rename = "GB-month"))]
    GbMonth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
pub enum PerAgentLimitUnit {
    #[serde(rename = "bytes")]
    #[cfg_attr(feature = "full", oai(rename = "bytes"))]
    Bytes,
}

impl Display for PerAgentLimitUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("bytes")
    }
}

impl Display for MonthlyStorageUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("GB-month")
    }
}

declare_structs! {
    #[derive(Copy, Eq)]
    /// A finite customer-visible resource limit.
    pub struct FiniteResourceLimit {
        pub value: u64,
    }

    #[derive(Copy, Eq)]
    /// A customer-visible resource limit with no finite bound.
    pub struct UnlimitedResourceLimit {}

    #[derive(Copy, Eq)]
    /// A storage limit that cannot be enforced because managed filesystem quotas are unavailable.
    pub struct DisabledResourceLimit {
        pub reason: StorageLimitDisabledReason,
    }

    #[derive(Copy, Eq, PartialOrd, Ord)]
    pub struct AccountUsagePeriod {
        pub year: i32,
        #[cfg_attr(feature = "full", oai(validator(minimum(value = "1"), maximum(value = "12"))))]
        pub month: u32,
    }

    pub struct AccountUsageMetering {
        pub compute: MeteringStatus,
        pub memory: MeteringStatus,
        pub durable_storage: MeteringStatus,
        pub ephemeral_storage: MeteringStatus,
    }

    pub struct AccountUsageMetrics {
        pub period: AccountUsagePeriod,
        pub as_of: DateTime<Utc>,
        pub compute_gcu: f64,
        pub memory_gb_seconds: u64,
        pub durable_storage_gb_month: f64,
        pub ephemeral_storage_gb_month: f64,
        pub metering: AccountUsageMetering,
    }

    pub struct AccountUsage {
        pub account_id: AccountId,
        pub usage: AccountUsageMetrics,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    pub struct MonthlyComputeLimit {
        pub metering: MeteringStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub plan_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub active_admin_grant: Option<AdminResourceGrant>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resolved_monthly_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub remaining: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Usage above the applied revision's resolved allowance that accrued with overage enabled.
        pub allow_overage_usage: Option<f64>,
        pub unit: MonthlyComputeUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub behavior: Option<MonthlyLimitBehavior>,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    pub struct MonthlyMemoryLimit {
        pub metering: MeteringStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub plan_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub active_admin_grant: Option<AdminResourceGrant>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resolved_monthly_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub remaining: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Usage above the applied revision's resolved allowance that accrued with overage enabled.
        pub allow_overage_usage: Option<f64>,
        pub unit: MonthlyMemoryUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub behavior: Option<MonthlyLimitBehavior>,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    pub struct MonthlyStorageLimit {
        pub metering: MeteringStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub plan_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub active_admin_grant: Option<AdminResourceGrant>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resolved_monthly_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub remaining: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Usage above the applied revision's resolved allowance that accrued with overage enabled.
        pub allow_overage_usage: Option<f64>,
        pub unit: MonthlyStorageUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub behavior: Option<MonthlyLimitBehavior>,
    }

    pub struct MonthlyResourceLimits {
        pub compute_gcu: MonthlyComputeLimit,
        pub memory_gb_seconds: MonthlyMemoryLimit,
        pub durable_storage_gb_month: MonthlyStorageLimit,
        pub ephemeral_storage_gb_month: MonthlyStorageLimit,
    }

    #[cfg_attr(feature = "full", oai(example))]
    pub struct MonthlyUsageModeTransition {
        pub actor_account_id: AccountId,
        pub changed_at: DateTime<Utc>,
        pub source: MonthlyUsageModeTransitionSource,
        pub previous_mode: MonthlyUsageMode,
        pub new_mode: MonthlyUsageMode,
    }

    pub struct SetMonthlyUsageMode {
        pub mode: MonthlyUsageMode,
    }

    #[cfg_attr(feature = "full", oai(example, skip_serializing_if_is_none))]
    pub struct AccountResourcePolicy {
        pub account_id: AccountId,
        pub monthly_usage_mode: MonthlyUsageMode,
        pub overage_allowed_by_plan: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub latest_owner_transition: Option<MonthlyUsageModeTransition>,
        pub monthly: MonthlyResourceLimits,
        pub max_memory_per_agent: MemoryLimit,
        pub max_storage_per_agent: StorageLimit,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", oai(example, skip_serializing_if_is_none))]
    pub struct StorageLimit {
        pub unit: PerAgentLimitUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub active_admin_grant: Option<AdminResourceGrant>,
        pub effective_value: StorageResourceLimitValue,
        pub plan_default: ResourceLimitValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub override_value: Option<ResourceLimitValue>,
        pub ceiling: ResourceLimitValue,
        pub user_configurable: bool,
    }

    pub struct SetStorageLimit {
        pub value: u64,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", oai(example, skip_serializing_if_is_none))]
    pub struct MemoryLimit {
        pub unit: PerAgentLimitUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub active_admin_grant: Option<AdminResourceGrant>,
        pub effective_value: ResourceLimitValue,
        pub plan_default: ResourceLimitValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub override_value: Option<ResourceLimitValue>,
        pub ceiling: ResourceLimitValue,
        pub user_configurable: bool,
    }

    pub struct SetMemoryLimit {
        pub value: u64,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    #[derive(Eq)]
    pub struct AdminResourceGrant {
        pub dimension: AdminResourceGrantDimension,
        /// The finite grant amount in the unit associated with `dimension`.
        pub value: u64,
        pub reason: AdminResourceGrantReason,
        pub actor_account_id: AccountId,
        pub granted_at: DateTime<Utc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub expires_at: Option<DateTime<Utc>>,
    }

    #[cfg_attr(feature = "full", oai(example, skip_serializing_if_is_none))]
    pub struct SetAdminResourceGrant {
        pub value: u64,
        pub reason: AdminResourceGrantReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub expires_at: Option<DateTime<Utc>>,
    }

    #[cfg_attr(feature = "full", oai(example, skip_serializing_if_is_none))]
    pub struct AdminResourceGrantChange {
        pub account_id: AccountId,
        pub dimension: AdminResourceGrantDimension,
        pub event_type: AdminResourceGrantEventType,
        pub reason: AdminResourceGrantReason,
        pub actor_account_id: AccountId,
        pub changed_at: DateTime<Utc>,
        /// The previous finite amount in the unit associated with `dimension`.
        pub old_value: u64,
        pub new_value: AdminResourceGrantChangeValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub expires_at: Option<DateTime<Utc>>,
    }
}

declare_unions! {
    #[derive(Copy, Eq)]
    #[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
    #[serde(rename_all = "camelCase")]
    pub enum ResourceLimitValue {
        Finite(FiniteResourceLimit),
        Unlimited(UnlimitedResourceLimit),
    }

    #[derive(Copy, Eq)]
    #[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
    #[serde(rename_all = "camelCase")]
    /// A customer-visible storage limit, including unavailable managed filesystem quotas.
    pub enum StorageResourceLimitValue {
        Finite(FiniteResourceLimit),
        Unlimited(UnlimitedResourceLimit),
        Disabled(DisabledResourceLimit),
    }

    #[derive(Copy, Eq)]
    #[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
    #[serde(rename_all = "camelCase")]
    pub enum AdminResourceGrantChangeValue {
        Finite(FiniteResourceLimit),
        Unlimited(UnlimitedResourceLimit),
        Disabled(DisabledResourceLimit),
    }
}

impl ResourceLimitValue {
    pub fn from_memory_value(value: u64) -> Self {
        if value == u64::MAX || value == EFFECTIVELY_UNLIMITED_MEMORY_LIMIT {
            Self::Unlimited(UnlimitedResourceLimit {})
        } else {
            Self::finite(value)
        }
    }

    pub fn from_storage_value(value: u64) -> Self {
        if value >= EFFECTIVELY_UNLIMITED_STORAGE_LIMIT {
            Self::Unlimited(UnlimitedResourceLimit {})
        } else {
            Self::finite(value)
        }
    }

    fn finite(value: u64) -> Self {
        Self::Finite(FiniteResourceLimit { value })
    }
}

impl Display for ResourceLimitValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Finite(value) => Display::fmt(&value.value, f),
            Self::Unlimited(_) => f.write_str("unlimited"),
        }
    }
}

impl StorageResourceLimitValue {
    pub fn from_storage_value(value: u64) -> Self {
        match ResourceLimitValue::from_storage_value(value) {
            ResourceLimitValue::Finite(value) => Self::Finite(value),
            ResourceLimitValue::Unlimited(value) => Self::Unlimited(value),
        }
    }

    pub fn disabled() -> Self {
        Self::Disabled(DisabledResourceLimit {
            reason: StorageLimitDisabledReason::ManagedFilesystemUnavailable,
        })
    }
}

impl AdminResourceGrantChangeValue {
    pub fn from_raw(dimension: AdminResourceGrantDimension, value: u64) -> Self {
        match dimension {
            AdminResourceGrantDimension::MaxMemoryPerAgent
                if value == u64::MAX || value == EFFECTIVELY_UNLIMITED_MEMORY_LIMIT =>
            {
                Self::Unlimited(UnlimitedResourceLimit {})
            }
            AdminResourceGrantDimension::MaxStoragePerAgent
                if value >= EFFECTIVELY_UNLIMITED_STORAGE_LIMIT =>
            {
                Self::Disabled(DisabledResourceLimit {
                    reason: StorageLimitDisabledReason::ManagedFilesystemUnavailable,
                })
            }
            _ => Self::Finite(FiniteResourceLimit { value }),
        }
    }
}

impl Display for StorageResourceLimitValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Finite(value) => Display::fmt(&value.value, f),
            Self::Unlimited(_) => f.write_str("unlimited"),
            Self::Disabled(_) => f.write_str("disabled"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyPlanAmounts {
    pub compute_gcu: u64,
    pub memory_gb_seconds: u64,
    pub durable_storage_gb_month: u64,
    pub ephemeral_storage_gb_month: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMonthlyPlanAmounts {
    pub compute_fuel: u64,
    pub memory_gb_seconds: u64,
    pub durable_storage_byte_seconds: u64,
    pub ephemeral_storage_byte_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MonthlyPlanAmountError {
    #[error("monthly compute GCU amount exceeds the supported fuel range")]
    ComputeOverflow,
    #[error("monthly durable storage GB-month amount exceeds the supported byte-second range")]
    DurableStorageOverflow,
    #[error("monthly ephemeral storage GB-month amount exceeds the supported byte-second range")]
    EphemeralStorageOverflow,
}

impl MonthlyPlanAmounts {
    pub fn resolve(self) -> Result<ResolvedMonthlyPlanAmounts, MonthlyPlanAmountError> {
        Ok(ResolvedMonthlyPlanAmounts {
            compute_fuel: self
                .compute_gcu
                .checked_mul(FUEL_PER_GCU)
                .ok_or(MonthlyPlanAmountError::ComputeOverflow)?,
            memory_gb_seconds: self.memory_gb_seconds,
            durable_storage_byte_seconds: self
                .durable_storage_gb_month
                .checked_mul(BYTE_SECONDS_PER_GB_MONTH)
                .ok_or(MonthlyPlanAmountError::DurableStorageOverflow)?,
            ephemeral_storage_byte_seconds: self
                .ephemeral_storage_gb_month
                .checked_mul(BYTE_SECONDS_PER_GB_MONTH)
                .ok_or(MonthlyPlanAmountError::EphemeralStorageOverflow)?,
        })
    }
}

impl Display for AccountUsagePeriod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl AccountUsagePeriod {
    pub fn current() -> Self {
        let now = Utc::now();
        Self {
            year: now.year(),
            month: now.month(),
        }
    }
}

impl StorageLimit {
    pub fn resolve(
        enabled: bool,
        plan_default: u64,
        override_value: Option<u64>,
        ceiling: u64,
        user_configurable: bool,
    ) -> Self {
        if enabled {
            let override_value = override_value
                .filter(|_| user_configurable)
                .filter(|value| (plan_default..=ceiling).contains(value));
            Self {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: StorageResourceLimitValue::from_storage_value(
                    override_value.unwrap_or(plan_default),
                ),
                plan_default: ResourceLimitValue::from_storage_value(plan_default),
                override_value: override_value.map(ResourceLimitValue::from_storage_value),
                ceiling: ResourceLimitValue::from_storage_value(ceiling),
                user_configurable,
            }
        } else {
            Self {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: StorageResourceLimitValue::disabled(),
                plan_default: ResourceLimitValue::from_storage_value(plan_default),
                override_value: None,
                ceiling: ResourceLimitValue::from_storage_value(ceiling),
                user_configurable: false,
            }
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for MonthlyUsageModeTransition {
    fn example() -> Self {
        Self {
            actor_account_id: AccountId(uuid::Uuid::from_u128(1)),
            changed_at: DateTime::from_timestamp(1_700_000_000, 0)
                .expect("example timestamp is valid"),
            source: MonthlyUsageModeTransitionSource::Owner,
            previous_mode: MonthlyUsageMode::AllowOverage,
            new_mode: MonthlyUsageMode::HardLimit,
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for AccountResourcePolicy {
    fn example() -> Self {
        Self {
            account_id: AccountId(uuid::Uuid::from_u128(1)),
            monthly_usage_mode: MonthlyUsageMode::HardLimit,
            overage_allowed_by_plan: false,
            latest_owner_transition: None,
            monthly: MonthlyResourceLimits {
                compute_gcu: MonthlyComputeLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(100),
                    active_admin_grant: None,
                    resolved_monthly_amount: Some(100),
                    usage: Some(25.0),
                    remaining: Some(75.0),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyComputeUnit::Gcu,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                memory_gb_seconds: MonthlyMemoryLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(10_000),
                    active_admin_grant: None,
                    resolved_monthly_amount: Some(10_000),
                    usage: Some(2_500),
                    remaining: Some(7_500),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyMemoryUnit::GbSeconds,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                durable_storage_gb_month: MonthlyStorageLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(50),
                    active_admin_grant: None,
                    resolved_monthly_amount: Some(50),
                    usage: Some(12.5),
                    remaining: Some(37.5),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyStorageUnit::GbMonth,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                ephemeral_storage_gb_month: MonthlyStorageLimit {
                    metering: MeteringStatus::Enabled,
                    plan_amount: Some(25),
                    active_admin_grant: None,
                    resolved_monthly_amount: Some(25),
                    usage: Some(5.0),
                    remaining: Some(20.0),
                    allow_overage_usage: Some(0.0),
                    unit: MonthlyStorageUnit::GbMonth,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
            },
            max_memory_per_agent: MemoryLimit::resolve(u64::MAX, None, u64::MAX, false),
            max_storage_per_agent: StorageLimit::resolve(
                false,
                1024 * 1024 * 1024,
                None,
                10 * 1024 * 1024 * 1024,
                false,
            ),
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for SetAdminResourceGrant {
    fn example() -> Self {
        Self {
            value: 10,
            reason: AdminResourceGrantReason::Support,
            expires_at: None,
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for AdminResourceGrantChange {
    fn example() -> Self {
        Self {
            account_id: AccountId(uuid::Uuid::from_u128(1)),
            dimension: AdminResourceGrantDimension::MonthlyComputeGcu,
            event_type: AdminResourceGrantEventType::OverrideGranted,
            reason: AdminResourceGrantReason::Support,
            actor_account_id: AccountId(uuid::Uuid::from_u128(2)),
            changed_at: DateTime::from_timestamp(1_700_000_000, 0)
                .expect("example timestamp is valid"),
            old_value: 5,
            new_value: AdminResourceGrantChangeValue::from_raw(
                AdminResourceGrantDimension::MonthlyComputeGcu,
                10,
            ),
            expires_at: None,
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for StorageLimit {
    fn example() -> Self {
        let mut limit = Self::resolve(
            true,
            1024 * 1024 * 1024,
            None,
            10 * 1024 * 1024 * 1024,
            true,
        );
        limit.effective_value =
            StorageResourceLimitValue::from_storage_value(2 * 1024 * 1024 * 1024);
        limit.active_admin_grant = Some(AdminResourceGrant {
            dimension: AdminResourceGrantDimension::MaxStoragePerAgent,
            value: 2 * 1024 * 1024 * 1024,
            reason: AdminResourceGrantReason::Support,
            actor_account_id: AccountId(uuid::Uuid::from_u128(2)),
            granted_at: DateTime::from_timestamp(1_700_000_000, 0)
                .expect("example timestamp is valid"),
            expires_at: None,
        });
        limit
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for MemoryLimit {
    fn example() -> Self {
        let mut limit = Self::resolve(1024 * 1024 * 1024, None, 10 * 1024 * 1024 * 1024, true);
        limit.effective_value = ResourceLimitValue::from_memory_value(2 * 1024 * 1024 * 1024);
        limit.active_admin_grant = Some(AdminResourceGrant {
            dimension: AdminResourceGrantDimension::MaxMemoryPerAgent,
            value: 2 * 1024 * 1024 * 1024,
            reason: AdminResourceGrantReason::Support,
            actor_account_id: AccountId(uuid::Uuid::from_u128(2)),
            granted_at: DateTime::from_timestamp(1_700_000_000, 0)
                .expect("example timestamp is valid"),
            expires_at: None,
        });
        limit
    }
}

impl MemoryLimit {
    pub fn resolve(
        plan_default: u64,
        override_value: Option<u64>,
        ceiling: u64,
        user_configurable: bool,
    ) -> Self {
        let override_value = override_value
            .filter(|_| user_configurable)
            .filter(|value| (plan_default..=ceiling).contains(value));
        Self {
            unit: PerAgentLimitUnit::Bytes,
            active_admin_grant: None,
            effective_value: ResourceLimitValue::from_memory_value(
                override_value.unwrap_or(plan_default),
            ),
            plan_default: ResourceLimitValue::from_memory_value(plan_default),
            override_value: override_value.map(ResourceLimitValue::from_memory_value),
            ceiling: ResourceLimitValue::from_memory_value(ceiling),
            user_configurable,
        }
    }
}

impl FromStr for AccountUsagePeriod {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (year, month) = value
            .split_once('-')
            .ok_or_else(|| PERIOD_FORMAT_ERROR.to_string())?;
        if year.len() != 4 || month.len() != 2 {
            return Err(PERIOD_FORMAT_ERROR.to_string());
        }

        let year = year.parse().map_err(|_| PERIOD_FORMAT_ERROR.to_string())?;
        let month = month.parse().map_err(|_| PERIOD_FORMAT_ERROR.to_string())?;
        if !(1..=12).contains(&month) {
            return Err("period month must be between 01 and 12".to_string());
        }
        if NaiveDate::from_ymd_opt(year, month, 1).is_none() {
            return Err("period year is outside the supported range".to_string());
        }

        Ok(Self { year, month })
    }
}

pub fn fuel_to_gcu(fuel: u64) -> f64 {
    fuel as f64 / FUEL_PER_GCU as f64
}

pub fn byte_seconds_to_gb_month(byte_seconds: u64) -> f64 {
    byte_seconds as f64 / BYTE_SECONDS_PER_GB_MONTH as f64
}

impl AccountUsageMetrics {
    pub fn format_compute(&self) -> String {
        format_metered(self.compute_gcu, "GCU", self.metering.compute)
    }

    pub fn format_memory(&self) -> String {
        format_metered(self.memory_gb_seconds, "GB-seconds", self.metering.memory)
    }

    pub fn format_durable_storage(&self) -> String {
        format_metered(
            self.durable_storage_gb_month,
            "GB-month",
            self.metering.durable_storage,
        )
    }

    pub fn format_ephemeral_storage(&self) -> String {
        format_metered(
            self.ephemeral_storage_gb_month,
            "GB-month",
            self.metering.ephemeral_storage,
        )
    }
}

fn format_metered(value: impl Display, unit: &str, status: MeteringStatus) -> String {
    match status {
        MeteringStatus::Enabled => format!("{value} {unit}"),
        MeteringStatus::Disabled => format!("{value} {unit} (metering disabled)"),
        MeteringStatus::Unknown => format!("{value} {unit} (metering state unknown)"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccountUsageMetering, AccountUsageMetrics, AccountUsagePeriod,
        AdminResourceGrantChangeValue, AdminResourceGrantDimension, AdminResourceGrantReason,
        BYTE_SECONDS_PER_GB_MONTH, EFFECTIVELY_UNLIMITED_MEMORY_LIMIT,
        EFFECTIVELY_UNLIMITED_STORAGE_LIMIT, FUEL_PER_GCU, MemoryLimit, MeteringStatus,
        MonthlyComputeUnit, MonthlyLimitBehavior, MonthlyMemoryUnit, MonthlyPlanAmountError,
        MonthlyPlanAmounts, MonthlyStorageUnit, MonthlyUsageMode, MonthlyUsageModeTransitionSource,
        PERIOD_FORMAT_ERROR, PerAgentLimitUnit, ResolvedMonthlyPlanAmounts, ResourceLimitValue,
        SetAdminResourceGrant, StorageLimit, StorageResourceLimitValue, byte_seconds_to_gb_month,
        fuel_to_gcu,
    };
    use chrono::Utc;
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn account_usage_period_parses_year_and_month() {
        let period = AccountUsagePeriod::from_str("2026-04").unwrap();
        assert_eq!(
            period,
            AccountUsagePeriod {
                year: 2026,
                month: 4,
            }
        );
        assert_eq!(period.to_string(), "2026-04");
    }

    #[test]
    fn account_usage_period_rejects_invalid_month() {
        assert_eq!(
            AccountUsagePeriod::from_str("2026-13").unwrap_err(),
            "period month must be between 01 and 12"
        );
        assert_eq!(
            AccountUsagePeriod::from_str("26-04").unwrap_err(),
            PERIOD_FORMAT_ERROR
        );
        assert_eq!(
            AccountUsagePeriod::from_str("2026-4").unwrap_err(),
            PERIOD_FORMAT_ERROR
        );
    }

    #[test]
    fn memory_limit_resolves_only_in_range_override() {
        assert_eq!(
            MemoryLimit::resolve(100, Some(150), 200, true),
            MemoryLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: ResourceLimitValue::from_memory_value(150),
                plan_default: ResourceLimitValue::from_memory_value(100),
                override_value: Some(ResourceLimitValue::from_memory_value(150)),
                ceiling: ResourceLimitValue::from_memory_value(200),
                user_configurable: true,
            }
        );
        assert_eq!(
            MemoryLimit::resolve(100, Some(99), 200, true).override_value,
            None
        );
        assert_eq!(
            MemoryLimit::resolve(100, Some(201), 200, true).override_value,
            None
        );
        assert_eq!(
            MemoryLimit::resolve(100, Some(150), 200, false),
            MemoryLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: ResourceLimitValue::from_memory_value(100),
                plan_default: ResourceLimitValue::from_memory_value(100),
                override_value: None,
                ceiling: ResourceLimitValue::from_memory_value(200),
                user_configurable: false,
            }
        );
    }

    #[test]
    fn enabled_storage_limit_resolves_only_in_range_override() {
        assert_eq!(
            StorageLimit::resolve(true, 100, Some(150), 200, true),
            StorageLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: StorageResourceLimitValue::from_storage_value(150),
                plan_default: ResourceLimitValue::from_storage_value(100),
                override_value: Some(ResourceLimitValue::from_storage_value(150)),
                ceiling: ResourceLimitValue::from_storage_value(200),
                user_configurable: true,
            }
        );
        assert_eq!(
            StorageLimit::resolve(true, 100, Some(99), 200, true).override_value,
            None
        );
        assert_eq!(
            StorageLimit::resolve(true, 100, Some(201), 200, true).override_value,
            None
        );
        assert_eq!(
            StorageLimit::resolve(true, 100, Some(150), 200, false),
            StorageLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: StorageResourceLimitValue::from_storage_value(100),
                plan_default: ResourceLimitValue::from_storage_value(100),
                override_value: None,
                ceiling: ResourceLimitValue::from_storage_value(200),
                user_configurable: false,
            }
        );
    }

    #[test]
    fn disabled_storage_limit_hides_override_and_configurability() {
        let limit = StorageLimit::resolve(false, 100, Some(300), 200, true);

        assert_eq!(
            limit,
            StorageLimit {
                unit: PerAgentLimitUnit::Bytes,
                active_admin_grant: None,
                effective_value: StorageResourceLimitValue::disabled(),
                plan_default: ResourceLimitValue::from_storage_value(100),
                override_value: None,
                ceiling: ResourceLimitValue::from_storage_value(200),
                user_configurable: false,
            }
        );
    }

    #[cfg(feature = "full")]
    #[test]
    fn storage_limit_openapi_example_is_coherent() {
        let example = <StorageLimit as poem_openapi::types::Example>::example();

        assert_eq!(
            <StorageLimit as poem_openapi::types::ToJSON>::to_json(&example),
            Some(serde_json::json!({
                "unit": "bytes",
                "activeAdminGrant": {
                    "dimension": "maxStoragePerAgent",
                    "value": 2 * 1024 * 1024 * 1024_u64,
                    "reason": "support",
                    "actorAccountId": "00000000-0000-0000-0000-000000000002",
                    "grantedAt": "2023-11-14T22:13:20+00:00",
                },
                "effectiveValue": {
                    "type": "finite",
                    "value": 2 * 1024 * 1024 * 1024_u64,
                },
                "planDefault": {
                    "type": "finite",
                    "value": 1024 * 1024 * 1024_u64,
                },
                "ceiling": {
                    "type": "finite",
                    "value": 10 * 1024 * 1024 * 1024_u64,
                },
                "userConfigurable": true,
            }))
        );

        let disabled = StorageLimit::resolve(false, 1, Some(2), 3, true);
        let expected = serde_json::json!({
            "unit": "bytes",
            "effectiveValue": {
                "type": "disabled",
                "reason": "managedFilesystemUnavailable",
            },
            "planDefault": { "type": "finite", "value": 1 },
            "ceiling": { "type": "finite", "value": 3 },
            "userConfigurable": false,
        });
        assert_eq!(
            <StorageLimit as poem_openapi::types::ToJSON>::to_json(&disabled),
            Some(expected.clone())
        );
        assert_eq!(serde_json::to_value(disabled).unwrap(), expected);
    }

    #[cfg(feature = "full")]
    #[test]
    fn memory_limit_openapi_example_uses_memory_grant() {
        let example = <MemoryLimit as poem_openapi::types::Example>::example();
        let json = <MemoryLimit as poem_openapi::types::ToJSON>::to_json(&example).unwrap();

        assert_eq!(
            json["activeAdminGrant"]["dimension"],
            serde_json::json!("maxMemoryPerAgent")
        );
        assert_eq!(
            json["effectiveValue"],
            serde_json::json!({
                "type": "finite",
                "value": 2 * 1024 * 1024 * 1024_u64,
            })
        );
    }

    #[test]
    fn resource_limit_values_serialize_without_sentinels() {
        let finite = ResourceLimitValue::from_memory_value(1024);
        let unlimited_memory = ResourceLimitValue::from_memory_value(u64::MAX);
        let effectively_unlimited_memory =
            ResourceLimitValue::from_memory_value(EFFECTIVELY_UNLIMITED_MEMORY_LIMIT);
        let unlimited_storage =
            ResourceLimitValue::from_storage_value(EFFECTIVELY_UNLIMITED_STORAGE_LIMIT);
        let disabled_storage = StorageResourceLimitValue::disabled();

        assert_eq!(
            serde_json::to_value(finite).unwrap(),
            serde_json::json!({ "type": "finite", "value": 1024 })
        );
        assert_eq!(
            serde_json::to_value(unlimited_memory).unwrap(),
            serde_json::json!({ "type": "unlimited" })
        );
        assert_eq!(
            serde_json::to_value(disabled_storage).unwrap(),
            serde_json::json!({
                "type": "disabled",
                "reason": "managedFilesystemUnavailable",
            })
        );
        assert_eq!(finite.to_string(), "1024");
        assert_eq!(unlimited_memory.to_string(), "unlimited");
        assert_eq!(disabled_storage.to_string(), "disabled");
        assert!(
            serde_json::from_value::<ResourceLimitValue>(serde_json::json!({
                "type": "disabled",
                "reason": "managedFilesystemUnavailable",
            }))
            .is_err()
        );
        assert!(matches!(
            effectively_unlimited_memory,
            ResourceLimitValue::Unlimited(_)
        ));
        assert!(
            !serde_json::to_string(&unlimited_memory)
                .unwrap()
                .contains(&u64::MAX.to_string())
        );
        assert!(
            !serde_json::to_string(&unlimited_storage)
                .unwrap()
                .contains(&EFFECTIVELY_UNLIMITED_STORAGE_LIMIT.to_string())
        );
    }

    #[test]
    fn admin_grant_change_values_use_one_tagged_representation() {
        let finite = AdminResourceGrantChangeValue::from_raw(
            AdminResourceGrantDimension::MonthlyComputeGcu,
            10,
        );
        let finite_memory = AdminResourceGrantChangeValue::from_raw(
            AdminResourceGrantDimension::MaxMemoryPerAgent,
            10,
        );
        let finite_storage = AdminResourceGrantChangeValue::from_raw(
            AdminResourceGrantDimension::MaxStoragePerAgent,
            10,
        );
        let effectively_unlimited_memory = AdminResourceGrantChangeValue::from_raw(
            AdminResourceGrantDimension::MaxMemoryPerAgent,
            EFFECTIVELY_UNLIMITED_MEMORY_LIMIT,
        );
        let unlimited_memory = AdminResourceGrantChangeValue::from_raw(
            AdminResourceGrantDimension::MaxMemoryPerAgent,
            u64::MAX,
        );
        let disabled = AdminResourceGrantChangeValue::from_raw(
            AdminResourceGrantDimension::MaxStoragePerAgent,
            EFFECTIVELY_UNLIMITED_STORAGE_LIMIT,
        );

        assert_eq!(
            serde_json::to_value(finite).unwrap(),
            serde_json::json!({ "type": "finite", "value": 10 })
        );
        for value in [finite_memory, finite_storage] {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::json!({ "type": "finite", "value": 10 })
            );
        }
        for value in [effectively_unlimited_memory, unlimited_memory] {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::json!({ "type": "unlimited" })
            );
        }
        assert_eq!(
            serde_json::to_value(disabled).unwrap(),
            serde_json::json!({
                "type": "disabled",
                "reason": "managedFilesystemUnavailable",
            })
        );
        for value in [effectively_unlimited_memory, unlimited_memory, disabled] {
            let json = serde_json::to_string(&value).unwrap();
            assert!(!json.contains(&u64::MAX.to_string()));
            assert!(!json.contains(&EFFECTIVELY_UNLIMITED_STORAGE_LIMIT.to_string()));
        }
    }

    #[cfg(feature = "full")]
    #[test]
    fn admin_resource_grant_request_example_is_always_valid() {
        let example = <SetAdminResourceGrant as poem_openapi::types::Example>::example();

        assert_eq!(example.reason, AdminResourceGrantReason::Support);
        assert_eq!(example.expires_at, None);
    }

    #[test]
    fn account_usage_uses_canonical_customer_unit_conversions() {
        assert_eq!(BYTE_SECONDS_PER_GB_MONTH, 2_821_793_513_472_000);
        assert_eq!(fuel_to_gcu(FUEL_PER_GCU * 2), 2.0);
        assert_eq!(byte_seconds_to_gb_month(BYTE_SECONDS_PER_GB_MONTH * 2), 2.0);
    }

    #[test]
    fn monthly_resource_units_are_dimension_specific() {
        assert_eq!(MonthlyComputeUnit::Gcu.to_string(), "GCU");
        assert_eq!(MonthlyMemoryUnit::GbSeconds.to_string(), "GB-seconds");
        assert_eq!(MonthlyStorageUnit::GbMonth.to_string(), "GB-month");
        assert_eq!(PerAgentLimitUnit::Bytes.to_string(), "bytes");

        assert_eq!(
            serde_json::to_value(MonthlyComputeUnit::Gcu).unwrap(),
            serde_json::json!("GCU")
        );
        assert_eq!(
            serde_json::to_value(MonthlyMemoryUnit::GbSeconds).unwrap(),
            serde_json::json!("GB-seconds")
        );
        assert_eq!(
            serde_json::to_value(MonthlyStorageUnit::GbMonth).unwrap(),
            serde_json::json!("GB-month")
        );
        assert_eq!(
            serde_json::to_value(PerAgentLimitUnit::Bytes).unwrap(),
            serde_json::json!("bytes")
        );

        assert!(serde_json::from_str::<MonthlyComputeUnit>(r#""GB-seconds""#).is_err());
        assert!(serde_json::from_str::<MonthlyMemoryUnit>(r#""GB-month""#).is_err());
        assert!(serde_json::from_str::<MonthlyStorageUnit>(r#""GCU""#).is_err());
    }

    #[test]
    fn monthly_usage_mode_uses_public_camel_case_values() {
        assert_eq!(MonthlyUsageMode::HardLimit.to_string(), "hardLimit");
        assert_eq!(MonthlyUsageMode::AllowOverage.to_string(), "allowOverage");
        assert_eq!(
            serde_json::to_value(MonthlyUsageMode::AllowOverage).unwrap(),
            serde_json::json!("allowOverage")
        );
        assert_eq!(
            serde_json::to_value(MonthlyUsageModeTransitionSource::PlanEligibilityRemoved).unwrap(),
            serde_json::json!("planEligibilityRemoved")
        );
        assert_eq!(
            serde_json::to_value(MonthlyLimitBehavior::IncludedAllowance).unwrap(),
            serde_json::json!("includedAllowance")
        );
    }

    #[test]
    fn monthly_plan_amounts_resolve_all_dimensions_exactly() {
        assert_eq!(
            MonthlyPlanAmounts {
                compute_gcu: 3,
                memory_gb_seconds: 5,
                durable_storage_gb_month: 7,
                ephemeral_storage_gb_month: 11,
            }
            .resolve(),
            Ok(ResolvedMonthlyPlanAmounts {
                compute_fuel: 3 * FUEL_PER_GCU,
                memory_gb_seconds: 5,
                durable_storage_byte_seconds: 7 * BYTE_SECONDS_PER_GB_MONTH,
                ephemeral_storage_byte_seconds: 11 * BYTE_SECONDS_PER_GB_MONTH,
            })
        );
    }

    #[test]
    fn monthly_plan_amounts_reject_each_overflow() {
        let maximum_gcu = u64::MAX / FUEL_PER_GCU;
        let maximum_gb_month = u64::MAX / BYTE_SECONDS_PER_GB_MONTH;
        let valid = MonthlyPlanAmounts {
            compute_gcu: maximum_gcu,
            memory_gb_seconds: u64::MAX,
            durable_storage_gb_month: maximum_gb_month,
            ephemeral_storage_gb_month: maximum_gb_month,
        };
        assert!(valid.resolve().is_ok());

        assert_eq!(
            MonthlyPlanAmounts {
                compute_gcu: maximum_gcu + 1,
                ..valid
            }
            .resolve(),
            Err(MonthlyPlanAmountError::ComputeOverflow)
        );
        assert_eq!(
            MonthlyPlanAmounts {
                durable_storage_gb_month: maximum_gb_month + 1,
                ..valid
            }
            .resolve(),
            Err(MonthlyPlanAmountError::DurableStorageOverflow)
        );
        assert_eq!(
            MonthlyPlanAmounts {
                ephemeral_storage_gb_month: maximum_gb_month + 1,
                ..valid
            }
            .resolve(),
            Err(MonthlyPlanAmountError::EphemeralStorageOverflow)
        );
    }

    #[test]
    fn account_usage_formatting_distinguishes_metering_state_from_zero() {
        let usage = AccountUsageMetrics {
            period: AccountUsagePeriod {
                year: 2026,
                month: 4,
            },
            as_of: Utc::now(),
            compute_gcu: 0.0,
            memory_gb_seconds: 0,
            durable_storage_gb_month: 0.0,
            ephemeral_storage_gb_month: 0.0,
            metering: AccountUsageMetering {
                compute: MeteringStatus::Enabled,
                memory: MeteringStatus::Disabled,
                durable_storage: MeteringStatus::Unknown,
                ephemeral_storage: MeteringStatus::Unknown,
            },
        };

        assert_eq!(usage.format_compute(), "0 GCU");
        assert_eq!(usage.format_memory(), "0 GB-seconds (metering disabled)");
        assert_eq!(
            usage.format_durable_storage(),
            "0 GB-month (metering state unknown)"
        );
        assert_eq!(
            usage.format_ephemeral_storage(),
            "0 GB-month (metering state unknown)"
        );
    }
}
