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

use crate::repo::model::plan::PlanRecord;
use golem_service_base::model::ResourceLimits;
use golem_service_base::repo::NumericU64;
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
    MonthlyHttpCalls = 8,
    MonthlyRpcCalls = 9,
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
            UsageType::MonthlyGasLimit
            | UsageType::MonthlyComponentUploadLimitBytes
            | UsageType::MonthlyHttpCalls
            | UsageType::MonthlyRpcCalls => UsageGrouping::Monthly,
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
            | UsageType::MonthlyComponentUploadLimitBytes
            | UsageType::MonthlyHttpCalls
            | UsageType::MonthlyRpcCalls => UsageTracking::Stats,
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
    pub changes: BTreeMap<UsageType, i64>,
}

impl AccountUsage {
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

    pub fn resource_limits(&self) -> ResourceLimits {
        let fuel_limit = self.plan.limit(UsageType::MonthlyGasLimit);
        let available_fuel =
            fuel_limit.saturating_sub(self.final_value(UsageType::MonthlyGasLimit));

        let http_limit = self.plan.limit(UsageType::MonthlyHttpCalls);
        let available_http_calls =
            http_limit.saturating_sub(self.final_value(UsageType::MonthlyHttpCalls));

        let rpc_limit = self.plan.limit(UsageType::MonthlyRpcCalls);
        let available_rpc_calls =
            rpc_limit.saturating_sub(self.final_value(UsageType::MonthlyRpcCalls));

        ResourceLimits {
            available_fuel,
            max_memory_per_worker: self.plan.max_memory_per_worker.get(),
            max_table_elements_per_worker: self.plan.max_table_elements_per_worker.get(),
            max_disk_space_per_worker: self.plan.max_disk_space_per_worker.get(),
            per_invocation_http_call_limit: self.plan.per_invocation_http_call_limit.get(),
            per_invocation_rpc_call_limit: self.plan.per_invocation_rpc_call_limit.get(),
            available_http_calls,
            available_rpc_calls,
            max_concurrent_agents_per_executor: self.plan.max_concurrent_agents_per_executor.get(),
            oplog_writes_per_second: self.plan.oplog_writes_per_second.get(),
        }
    }
}
