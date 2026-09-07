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
use crate::{declare_enums, declare_structs};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub const BYTE_SECONDS_PER_GB_MONTH: u64 = 1024 * 1024 * 1024 * 730 * 3600;
pub const FUEL_PER_GCU: u64 = 1_000_000;
pub const DEFAULT_ACCOUNT_USAGE_HISTORY_PERIODS: usize = 6;
pub const EFFECTIVELY_UNLIMITED_STORAGE_LIMIT: u64 = 10_000_000_000_000_000;
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

impl Display for MonthlyStorageUnit {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("GB-month")
    }
}

declare_structs! {
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
        pub monthly_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub remaining: Option<f64>,
        pub unit: MonthlyComputeUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub behavior: Option<MonthlyLimitBehavior>,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    pub struct MonthlyMemoryLimit {
        pub metering: MeteringStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub monthly_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub remaining: Option<u64>,
        pub unit: MonthlyMemoryUnit,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub behavior: Option<MonthlyLimitBehavior>,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    pub struct MonthlyStorageLimit {
        pub metering: MeteringStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub monthly_amount: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub remaining: Option<f64>,
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
        pub admin_grants: Vec<AdminResourceGrant>,
        pub monthly: MonthlyResourceLimits,
        pub max_memory_per_agent: MemoryLimit,
        pub max_storage_per_agent: StorageLimit,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", oai(example, skip_serializing_if_is_none))]
    pub struct StorageLimit {
        pub enabled: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub effective_value: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub plan_default: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub override_value: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub ceiling: Option<u64>,
        pub user_configurable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub disabled_reason: Option<StorageLimitDisabledReason>,
    }

    pub struct SetStorageLimit {
        pub value: u64,
    }

    #[derive(Eq)]
    pub struct MemoryLimit {
        pub effective_value: u64,
        pub plan_default: u64,
        pub override_value: Option<u64>,
        pub ceiling: u64,
        pub user_configurable: bool,
    }

    pub struct SetMemoryLimit {
        pub value: u64,
    }

    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    pub struct AdminResourceGrant {
        pub dimension: AdminResourceGrantDimension,
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
        pub old_value: u64,
        pub new_value: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub expires_at: Option<DateTime<Utc>>,
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
                enabled: true,
                effective_value: Some(override_value.unwrap_or(plan_default)),
                plan_default: Some(plan_default),
                override_value,
                ceiling: Some(ceiling),
                user_configurable,
                disabled_reason: None,
            }
        } else {
            Self {
                enabled: false,
                effective_value: None,
                plan_default: None,
                override_value: None,
                ceiling: None,
                user_configurable: false,
                disabled_reason: Some(StorageLimitDisabledReason::ManagedFilesystemUnavailable),
            }
        }
    }

    pub fn executor_value(&self) -> u64 {
        self.effective_value
            .unwrap_or(EFFECTIVELY_UNLIMITED_STORAGE_LIMIT)
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
            admin_grants: Vec::new(),
            monthly: MonthlyResourceLimits {
                compute_gcu: MonthlyComputeLimit {
                    metering: MeteringStatus::Enabled,
                    monthly_amount: Some(100),
                    usage: Some(25.0),
                    remaining: Some(75.0),
                    unit: MonthlyComputeUnit::Gcu,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                memory_gb_seconds: MonthlyMemoryLimit {
                    metering: MeteringStatus::Enabled,
                    monthly_amount: Some(10_000),
                    usage: Some(2_500),
                    remaining: Some(7_500),
                    unit: MonthlyMemoryUnit::GbSeconds,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                durable_storage_gb_month: MonthlyStorageLimit {
                    metering: MeteringStatus::Enabled,
                    monthly_amount: Some(50),
                    usage: Some(12.5),
                    remaining: Some(37.5),
                    unit: MonthlyStorageUnit::GbMonth,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
                ephemeral_storage_gb_month: MonthlyStorageLimit {
                    metering: MeteringStatus::Enabled,
                    monthly_amount: Some(25),
                    usage: Some(5.0),
                    remaining: Some(20.0),
                    unit: MonthlyStorageUnit::GbMonth,
                    behavior: Some(MonthlyLimitBehavior::HardLimit),
                },
            },
            max_memory_per_agent: MemoryLimit::resolve(
                1024 * 1024 * 1024,
                None,
                10 * 1024 * 1024 * 1024,
                true,
            ),
            max_storage_per_agent: <StorageLimit as poem_openapi::types::Example>::example(),
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
            new_value: 10,
            expires_at: None,
        }
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for StorageLimit {
    fn example() -> Self {
        Self::resolve(
            true,
            1024 * 1024 * 1024,
            None,
            10 * 1024 * 1024 * 1024,
            true,
        )
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
            effective_value: override_value.unwrap_or(plan_default),
            plan_default,
            override_value,
            ceiling,
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
        AccountUsageMetering, AccountUsageMetrics, AccountUsagePeriod, AdminResourceGrantReason,
        BYTE_SECONDS_PER_GB_MONTH, EFFECTIVELY_UNLIMITED_STORAGE_LIMIT, FUEL_PER_GCU, MemoryLimit,
        MeteringStatus, MonthlyComputeUnit, MonthlyLimitBehavior, MonthlyMemoryUnit,
        MonthlyPlanAmountError, MonthlyPlanAmounts, MonthlyStorageUnit, MonthlyUsageMode,
        MonthlyUsageModeTransitionSource, PERIOD_FORMAT_ERROR, ResolvedMonthlyPlanAmounts,
        SetAdminResourceGrant, StorageLimit, StorageLimitDisabledReason, byte_seconds_to_gb_month,
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
                effective_value: 150,
                plan_default: 100,
                override_value: Some(150),
                ceiling: 200,
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
                effective_value: 100,
                plan_default: 100,
                override_value: None,
                ceiling: 200,
                user_configurable: false,
            }
        );
    }

    #[test]
    fn enabled_storage_limit_resolves_only_in_range_override() {
        assert_eq!(
            StorageLimit::resolve(true, 100, Some(150), 200, true),
            StorageLimit {
                enabled: true,
                effective_value: Some(150),
                plan_default: Some(100),
                override_value: Some(150),
                ceiling: Some(200),
                user_configurable: true,
                disabled_reason: None,
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
                enabled: true,
                effective_value: Some(100),
                plan_default: Some(100),
                override_value: None,
                ceiling: Some(200),
                user_configurable: false,
                disabled_reason: None,
            }
        );
    }

    #[test]
    fn disabled_storage_limit_hides_values_and_resolves_to_unlimited_for_executor() {
        let limit = StorageLimit::resolve(false, 100, Some(300), 200, true);

        assert_eq!(
            limit,
            StorageLimit {
                enabled: false,
                effective_value: None,
                plan_default: None,
                override_value: None,
                ceiling: None,
                user_configurable: false,
                disabled_reason: Some(StorageLimitDisabledReason::ManagedFilesystemUnavailable),
            }
        );
        assert_eq!(limit.executor_value(), EFFECTIVELY_UNLIMITED_STORAGE_LIMIT);
    }

    #[cfg(feature = "full")]
    #[test]
    fn storage_limit_openapi_example_is_coherent() {
        let example = <StorageLimit as poem_openapi::types::Example>::example();

        assert_eq!(
            <StorageLimit as poem_openapi::types::ToJSON>::to_json(&example),
            Some(serde_json::json!({
                "enabled": true,
                "effectiveValue": 1024 * 1024 * 1024_u64,
                "planDefault": 1024 * 1024 * 1024_u64,
                "ceiling": 10 * 1024 * 1024 * 1024_u64,
                "userConfigurable": true,
            }))
        );

        let disabled = StorageLimit::resolve(false, 1, Some(2), 3, true);
        let expected = serde_json::json!({
            "enabled": false,
            "userConfigurable": false,
            "disabledReason": "managedFilesystemUnavailable",
        });
        assert_eq!(
            <StorageLimit as poem_openapi::types::ToJSON>::to_json(&disabled),
            Some(expected.clone())
        );
        assert_eq!(serde_json::to_value(disabled).unwrap(), expected);
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
