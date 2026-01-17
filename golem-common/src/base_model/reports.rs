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

use crate::declare_structs;
use crate::model::account::AccountId;
use chrono::DateTime;
use chrono::Utc;
use std::fmt::Debug;

declare_structs! {
    pub struct AccountSummaryReport {
        pub id: AccountId,
        pub name: String,
        pub email: String,
        pub components_count: u64,
        pub workers_count: u64,
        pub created_at: DateTime<Utc>,
    }

    pub struct AccountCountsReport {
        pub total_accounts: u64,
        pub total_active_accounts: u64,
        pub total_deleted_accounts: u64
    }
}
