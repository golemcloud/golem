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

pub const BYTE_SECONDS_PER_GB_MONTH: f64 = 1024.0 * 1024.0 * 1024.0 * 730.0 * 3600.0;
pub const FUEL_PER_GCU: u64 = 1_000_000;
pub const DEFAULT_ACCOUNT_USAGE_HISTORY_PERIODS: usize = 6;
const PERIOD_FORMAT_ERROR: &str = "period must use YYYY-MM format";

declare_enums! {
    pub enum MeteringStatus {
        Enabled,
        Disabled,
        /// No usage producer has reported the metering state for this period yet.
        Unknown,
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

    #[derive(Eq)]
    pub struct StorageLimit {
        pub effective_value: u64,
        pub plan_default: u64,
        pub override_value: Option<u64>,
        pub ceiling: u64,
        pub user_configurable: bool,
    }

    pub struct SetStorageLimit {
        pub value: u64,
        pub expires_at: Option<DateTime<Utc>>,
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
        pub expires_at: Option<DateTime<Utc>>,
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
        plan_default: u64,
        override_value: Option<u64>,
        ceiling: u64,
        user_configurable: bool,
    ) -> Self {
        Self {
            effective_value: override_value.unwrap_or(plan_default).min(ceiling),
            plan_default,
            override_value,
            ceiling,
            user_configurable,
        }
    }
}

impl MemoryLimit {
    pub fn resolve(
        plan_default: u64,
        override_value: Option<u64>,
        ceiling: u64,
        user_configurable: bool,
    ) -> Self {
        Self {
            effective_value: override_value.unwrap_or(plan_default).min(ceiling),
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
    byte_seconds as f64 / BYTE_SECONDS_PER_GB_MONTH
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
        AccountUsageMetering, AccountUsageMetrics, AccountUsagePeriod, FUEL_PER_GCU, MemoryLimit,
        MeteringStatus, byte_seconds_to_gb_month, fuel_to_gcu,
    };
    use chrono::Utc;
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn account_usage_period_parses_year_and_month() {
        assert_eq!(
            AccountUsagePeriod::from_str("2026-04").unwrap(),
            AccountUsagePeriod {
                year: 2026,
                month: 4,
            }
        );
    }

    #[test]
    fn account_usage_period_rejects_invalid_month() {
        assert_eq!(
            AccountUsagePeriod::from_str("2026-13").unwrap_err(),
            "period month must be between 01 and 12"
        );
    }

    #[test]
    fn memory_limit_resolves_override_and_clamps_to_ceiling() {
        assert_eq!(
            MemoryLimit::resolve(100, Some(300), 200, true),
            MemoryLimit {
                effective_value: 200,
                plan_default: 100,
                override_value: Some(300),
                ceiling: 200,
                user_configurable: true,
            }
        );
    }

    #[test]
    fn account_usage_uses_canonical_customer_unit_conversions() {
        assert_eq!(fuel_to_gcu(FUEL_PER_GCU * 2), 2.0);
        let byte_seconds_per_gb_month = 1024_u64.pow(3) * 730 * 3600;
        assert_eq!(byte_seconds_to_gb_month(byte_seconds_per_gb_month * 2), 2.0);
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
