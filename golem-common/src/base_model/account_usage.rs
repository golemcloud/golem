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
use crate::base_model::plan::{PlanId, PlanName};
use crate::declare_structs;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub const BYTE_SECONDS_PER_GB_MONTH: f64 = 1024.0 * 1024.0 * 1024.0 * 730.0 * 3600.0;
pub const FUEL_PER_GCU: u64 = 1_000_000;
pub const DEFAULT_STORAGE_USAGE_HISTORY_PERIODS: usize = 6;
const PERIOD_FORMAT_ERROR: &str = "period must use YYYY-MM format";

declare_structs! {
    #[derive(Copy, Eq, PartialOrd, Ord)]
    pub struct StorageUsagePeriod {
        pub year: i32,
        pub month: u32,
    }

    pub struct StorageUsageMetrics {
        pub period: StorageUsagePeriod,
        pub compute_gcu: f64,
        pub memory_gb_seconds: u64,
        pub durable_storage_gb_month: f64,
        pub ephemeral_storage_gb_month: f64,
    }

    pub struct StorageUsage {
        pub account_id: AccountId,
        pub plan_id: PlanId,
        pub plan_name: PlanName,
        pub usage: StorageUsageMetrics,
        pub max_storage_per_agent: StorageLimit,
        pub max_memory_per_agent: MemoryLimit,
        pub monthly_memory_gb_seconds: MemoryLimit,
    }

    pub struct StorageUsageHistory {
        pub account_id: AccountId,
        pub usage: StorageUsageMetrics,
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

    pub struct MemoryLimits {
        pub max_memory_per_agent: MemoryLimit,
        pub monthly_memory_gb_seconds: MemoryLimit,
    }

    pub struct SetMemoryLimit {
        pub value: u64,
        pub expires_at: Option<DateTime<Utc>>,
    }
}

impl Display for StorageUsagePeriod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl StorageUsagePeriod {
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

impl FromStr for StorageUsagePeriod {
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

#[cfg(test)]
mod tests {
    use super::{MemoryLimit, StorageUsagePeriod};
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn storage_usage_period_parses_year_and_month() {
        assert_eq!(
            StorageUsagePeriod::from_str("2026-04").unwrap(),
            StorageUsagePeriod {
                year: 2026,
                month: 4,
            }
        );
    }

    #[test]
    fn storage_usage_period_rejects_invalid_month() {
        assert_eq!(
            StorageUsagePeriod::from_str("2026-13").unwrap_err(),
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
}
