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

use super::{AccountId, PlanId};
use crate::declare_structs;

declare_structs! {
    pub struct Account {
        pub id: AccountId,
        pub name: String,
        pub email: String,
        pub plan_id: PlanId,
    }

    pub struct AccountData {
        pub name: String,
        pub email: String,
    }

    pub struct Plan {
        pub plan_id: PlanId,
        pub plan_data: PlanData,
    }

    pub struct PlanData {
        pub project_limit: i32,
        pub component_limit: i32,
        pub worker_limit: i32,
        pub storage_limit: i32,
        pub monthly_gas_limit: i64,
        pub monthly_upload_limit: i32,
    }
}
