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

use crate::repo::model::plan::PlanRecord;
use golem_service_base::model::ResourceLimits;
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
    TotalWorkerCount = 0,
    TotalWorkerConnectionCount = 1,
    MonthlyGasLimit = 2,
    MonthlyComponentUploadLimitBytes = 3,
    TotalAppCount = 4,
    TotalEnvCount = 5,
    TotalComponentCount = 6,
    TotalComponentStorageBytes = 7,
}

impl UsageType {
    pub fn grouping(&self) -> UsageGrouping {
        match self {
            UsageType::TotalAppCount
            | UsageType::TotalEnvCount
            | UsageType::TotalComponentCount
            | UsageType::TotalWorkerCount
            | UsageType::TotalWorkerConnectionCount
            | UsageType::TotalComponentStorageBytes => UsageGrouping::Total,
            UsageType::MonthlyGasLimit | UsageType::MonthlyComponentUploadLimitBytes => {
                UsageGrouping::Monthly
            }
        }
    }

    pub fn tracking(&self) -> UsageTracking {
        match self {
            UsageType::TotalAppCount => UsageTracking::SelectTotalAppCount,
            UsageType::TotalEnvCount => UsageTracking::SelectTotalEnvCount,
            UsageType::TotalComponentCount => UsageTracking::SelectTotalComponentCount,
            UsageType::TotalComponentStorageBytes => UsageTracking::SelectTotalComponentSize,
            UsageType::TotalWorkerCount
            | UsageType::TotalWorkerConnectionCount
            | UsageType::MonthlyGasLimit
            | UsageType::MonthlyComponentUploadLimitBytes => UsageTracking::Stats,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct AccountUsageStatsRecord {
    pub account_id: Uuid,
    pub usage_type: i32,
    pub usage_key: String,
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    pub account_id: Uuid,

    pub year: i32,
    pub month: u32,

    pub usage: BTreeMap<UsageType, i64>,
    pub plan: PlanRecord,

    pub changes: BTreeMap<UsageType, i64>,
}

impl AccountUsage {
    pub fn usage(&self, usage_type: UsageType) -> i64 {
        self.usage.get(&usage_type).copied().unwrap_or(0)
    }

    pub fn change(&self, usage_type: UsageType) -> i64 {
        self.changes.get(&usage_type).copied().unwrap_or(0)
    }

    pub fn final_value(&self, usage_type: UsageType) -> i64 {
        self.usage(usage_type)
            .saturating_add(self.change(usage_type))
    }

    pub fn add_change(&mut self, usage_type: UsageType, change: i64) -> bool {
        let limit = self.plan.limit(usage_type);

        self.changes
            .entry(usage_type)
            .and_modify(|e| *e = e.saturating_add(change))
            .or_insert(change);

        self.final_value(usage_type) <= limit
    }

    pub fn resource_limits(&self) -> ResourceLimits {
        let available_fuel = self
            .plan
            .monthly_gas_limit
            .saturating_sub(self.final_value(UsageType::MonthlyGasLimit));

        ResourceLimits {
            available_fuel,
            max_memory_per_worker: self.plan.max_memory_per_worker as u64,
        }
    }
}
