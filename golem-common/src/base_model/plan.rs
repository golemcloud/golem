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

use crate::{declare_structs, declare_transparent_newtypes, newtype_uuid};

newtype_uuid!(PlanId, golem_api_grpc::proto::golem::account::PlanId);

declare_transparent_newtypes! {
    pub struct PlanName(pub String);
}

declare_structs! {
    pub struct Plan {
        pub plan_id: PlanId,
        pub name: PlanName,
        pub app_limit: u64,
        pub env_limit: u64,
        pub component_limit: u64,
        pub worker_limit: u64,
        pub worker_connection_limit: u64,
        pub storage_limit: u64,
        pub monthly_gas_limit: u64,
        pub monthly_upload_limit: u64,
        pub max_memory_per_worker: u64,
    }
}
