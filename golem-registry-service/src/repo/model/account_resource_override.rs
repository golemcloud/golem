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

use golem_service_base::repo::{NumericU64, SqlDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountResourceOverrideDimension {
    MaxDiskSpacePerWorker,
}

impl AccountResourceOverrideDimension {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxDiskSpacePerWorker => "max_disk_space_per_worker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountResourceOverrideReason {
    UserSelfServe,
    DowngradeClamp,
}

impl AccountResourceOverrideReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserSelfServe => "user_self_serve",
            Self::DowngradeClamp => "downgrade_clamp",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountResourceOverrideRecord {
    pub account_id: Uuid,
    pub dimension: AccountResourceOverrideDimension,
    pub override_value: NumericU64,
    pub reason: AccountResourceOverrideReason,
    pub expires_at: Option<SqlDateTime>,
    pub created_by: Uuid,
    pub created_at: SqlDateTime,
}
