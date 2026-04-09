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

use crate::repo::model::account_usage::UsageType;
use golem_common::model::plan::{Plan, PlanId, PlanName};
use golem_service_base::repo::NumericU64;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PlanRecord {
    pub plan_id: Uuid,
    pub name: String,

    pub max_memory_per_worker: NumericU64,
    pub max_table_elements_per_worker: NumericU64,
    pub max_disk_space_per_worker: NumericU64,
    pub max_concurrent_agents_per_executor: NumericU64,
    pub total_app_count: NumericU64,
    pub total_env_count: NumericU64,
    pub total_component_count: NumericU64,
    pub total_worker_connection_count: NumericU64,
    pub total_component_storage_bytes: NumericU64,
    pub monthly_gas_limit: NumericU64,
    pub monthly_component_upload_limit_bytes: NumericU64,
    pub per_invocation_http_call_limit: NumericU64,
    pub per_invocation_rpc_call_limit: NumericU64,
    pub monthly_http_call_limit: NumericU64,
    pub monthly_rpc_call_limit: NumericU64,
}

impl PlanRecord {
    pub fn limit(&self, usage_type: UsageType) -> u64 {
        match usage_type {
            UsageType::MonthlyComponentUploadLimitBytes => {
                self.monthly_component_upload_limit_bytes.get()
            }
            UsageType::MonthlyGasLimit => self.monthly_gas_limit.get(),
            UsageType::TotalAppCount => self.total_app_count.get(),
            UsageType::TotalEnvCount => self.total_env_count.get(),
            UsageType::TotalComponentCount => self.total_component_count.get(),
            UsageType::TotalComponentStorageBytes => self.total_component_storage_bytes.get(),
            UsageType::TotalWorkerConnectionCount => self.total_worker_connection_count.get(),
            UsageType::MonthlyHttpCalls => self.monthly_http_call_limit.get(),
            UsageType::MonthlyRpcCalls => self.monthly_rpc_call_limit.get(),
        }
    }
}

impl From<PlanRecord> for Plan {
    fn from(value: PlanRecord) -> Self {
        Self {
            app_limit: value.total_app_count.get(),
            env_limit: value.total_env_count.get(),
            component_limit: value.total_component_count.get(),
            worker_connection_limit: value.total_worker_connection_count.get(),
            storage_limit: value.total_component_storage_bytes.get(),
            monthly_gas_limit: value.monthly_gas_limit.get(),
            monthly_upload_limit: value.monthly_component_upload_limit_bytes.get(),
            max_memory_per_worker: value.max_memory_per_worker.get(),
            max_table_elements_per_worker: value.max_table_elements_per_worker.get(),
            max_disk_space_per_worker: value.max_disk_space_per_worker.get(),
            per_invocation_http_call_limit: value.per_invocation_http_call_limit.get(),
            per_invocation_rpc_call_limit: value.per_invocation_rpc_call_limit.get(),
            monthly_http_call_limit: value.monthly_http_call_limit.get(),
            monthly_rpc_call_limit: value.monthly_rpc_call_limit.get(),
            max_concurrent_agents_per_executor: value.max_concurrent_agents_per_executor.get(),
            plan_id: PlanId(value.plan_id),
            name: PlanName(value.name),
        }
    }
}
