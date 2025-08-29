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
use golem_service_base::repo::RepoResult;
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
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type, EnumIter)]
#[sqlx(type_name = "integer")]
pub enum UsageType {
    TotalAppCount = 0,
    TotalEnvCount = 1,
    TotalComponentCount = 2,
    TotalWorkerCount = 3,
    TotalComponentStorageBytes = 4,
    MonthlyGasLimit = 5,
    MonthlyComponentUploadLimitBytes = 6,
}

impl UsageType {
    pub fn grouping(&self) -> UsageGrouping {
        match self {
            UsageType::TotalAppCount
            | UsageType::TotalEnvCount
            | UsageType::TotalComponentCount
            | UsageType::TotalWorkerCount
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
            UsageType::TotalWorkerCount
            | UsageType::TotalComponentStorageBytes
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

    pub increase: BTreeMap<UsageType, i64>,
}

impl AccountUsage {
    pub fn usage(&self, usage_type: UsageType) -> i64 {
        self.usage.get(&usage_type).copied().unwrap_or(0)
    }

    pub fn increase(&self, usage_type: UsageType) -> i64 {
        self.increase.get(&usage_type).copied().unwrap_or(0)
    }

    pub fn add_checked(&mut self, usage_type: UsageType, increase: i64) -> RepoResult<bool> {
        let Some(limit) = self.plan.limit(usage_type)? else {
            return Ok(true);
        };

        self.increase
            .entry(usage_type)
            .and_modify(|e| *e += increase)
            .or_insert(increase);

        let increase = self.increase.get(&usage_type).copied().unwrap_or(0);

        Ok(self.usage(usage_type) + increase <= limit)
    }
}
