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

use crate::base_model::account_usage::{ResourceLimitValue, StorageResourceLimitValue};
use crate::{declare_structs, declare_transparent_newtypes, newtype_uuid};

newtype_uuid!(PlanId, golem_api_grpc::proto::golem::account::PlanId);

declare_transparent_newtypes! {
    pub struct PlanName(pub String);
}

declare_structs! {
    #[cfg_attr(feature = "full", oai(example))]
    pub struct Plan {
        pub plan_id: PlanId,
        pub name: PlanName,
        pub app_limit: u64,
        pub env_limit: u64,
        pub component_limit: u64,
        pub worker_connection_limit: u64,
        pub storage_limit: u64,
        pub monthly_upload_limit: u64,
        pub monthly_compute_gcu: u64,
        pub monthly_memory_gb_seconds: u64,
        pub monthly_durable_storage_gb_month: u64,
        pub monthly_ephemeral_storage_gb_month: u64,
        pub overage_allowed_by_plan: bool,
        pub max_memory_per_agent: ResourceLimitValue,
        pub max_memory_per_agent_ceiling: ResourceLimitValue,
        pub max_memory_per_agent_user_configurable: bool,
        pub max_table_elements_per_worker: u64,
        /// The default storage limit, or disabled when managed filesystem quotas are unavailable.
        pub max_storage_per_agent: StorageResourceLimitValue,
        /// The storage ceiling, or disabled when managed filesystem quotas are unavailable.
        pub max_storage_per_agent_ceiling: StorageResourceLimitValue,
        pub max_storage_per_agent_user_configurable: bool,
        pub per_invocation_http_call_limit: u64,
        pub per_invocation_rpc_call_limit: u64,
        pub monthly_http_call_limit: u64,
        pub monthly_rpc_call_limit: u64,
        pub max_concurrent_agents_per_executor: u64,
        pub oplog_writes_per_second: u64,
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for Plan {
    fn example() -> Self {
        Self {
            plan_id: PlanId(uuid::Uuid::from_u128(1)),
            name: PlanName("default".to_string()),
            app_limit: 10,
            env_limit: 40,
            component_limit: 100,
            worker_connection_limit: 100,
            storage_limit: 500_000_000,
            monthly_upload_limit: 1_000_000_000,
            monthly_compute_gcu: 100,
            monthly_memory_gb_seconds: 10_000,
            monthly_durable_storage_gb_month: 50,
            monthly_ephemeral_storage_gb_month: 25,
            overage_allowed_by_plan: false,
            max_memory_per_agent: ResourceLimitValue::from_memory_value(u64::MAX),
            max_memory_per_agent_ceiling: ResourceLimitValue::from_memory_value(u64::MAX),
            max_memory_per_agent_user_configurable: false,
            max_table_elements_per_worker: 16_384,
            max_storage_per_agent: StorageResourceLimitValue::disabled(),
            max_storage_per_agent_ceiling: StorageResourceLimitValue::disabled(),
            max_storage_per_agent_user_configurable: false,
            per_invocation_http_call_limit: 1_000,
            per_invocation_rpc_call_limit: 1_000,
            monthly_http_call_limit: 10_000,
            monthly_rpc_call_limit: 10_000,
            max_concurrent_agents_per_executor: 100,
            oplog_writes_per_second: 1_000,
        }
    }
}
