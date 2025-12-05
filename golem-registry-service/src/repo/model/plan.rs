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

use crate::repo::model::account_usage::UsageType;
use golem_common::model::plan::{Plan, PlanId, PlanName};
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanRecord {
    pub plan_id: Uuid,
    pub name: String,

    pub max_memory_per_worker: i64,
    pub total_app_count: i64,
    pub total_env_count: i64,
    pub total_component_count: i64,
    pub total_worker_count: i64,
    pub total_worker_connection_count: i64,
    pub total_component_storage_bytes: i64,
    pub monthly_gas_limit: i64,
    pub monthly_component_upload_limit_bytes: i64,
}

impl PlanRecord {
    pub fn limit(&self, usage_type: UsageType) -> i64 {
        match usage_type {
            UsageType::MonthlyComponentUploadLimitBytes => {
                self.monthly_component_upload_limit_bytes
            }
            UsageType::MonthlyGasLimit => self.monthly_gas_limit,
            UsageType::TotalAppCount => self.total_app_count,
            UsageType::TotalEnvCount => self.total_env_count,
            UsageType::TotalComponentCount => self.total_component_count,
            UsageType::TotalComponentStorageBytes => self.total_component_storage_bytes,
            UsageType::TotalWorkerCount => self.total_worker_count,
            UsageType::TotalWorkerConnectionCount => self.total_worker_connection_count,
        }
    }
}

impl TryFrom<PlanRecord> for Plan {
    type Error = RepoError;

    fn try_from(value: PlanRecord) -> Result<Self, Self::Error> {
        // apply defaults here to migrate old data when new limits are added.
        Ok(Self {
            app_limit: value.total_app_count,
            env_limit: value.total_env_count,
            component_limit: value.total_component_count,
            worker_limit: value.total_worker_count,
            worker_connection_limit: value.total_worker_connection_count,
            storage_limit: value.total_component_storage_bytes,
            monthly_gas_limit: value.monthly_gas_limit,
            monthly_upload_limit: value.monthly_component_upload_limit_bytes,
            max_memory_per_worker: value.max_memory_per_worker as u64,
            plan_id: PlanId(value.plan_id),
            name: PlanName(value.name),
        })
    }
}
