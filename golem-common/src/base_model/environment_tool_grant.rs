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

use crate::base_model::account::{AccountId, AccountSummary};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool_release::{ToolReleaseId, ToolReleaseMetadata, ToolReleaseReference};
use crate::{declare_enums, declare_structs, newtype_uuid};
use chrono::{DateTime, Utc};

newtype_uuid!(EnvironmentToolGrantId);

declare_enums! {
    pub enum EnvironmentToolGrantLifecycle {
        Active,
        Deleted,
    }
}

declare_structs! {
    pub struct EnvironmentToolGrant {
        pub id: EnvironmentToolGrantId,
        pub environment_id: EnvironmentId,
        pub tool_release_id: ToolReleaseId,
        pub protected: bool,
        pub automatic: bool,
        pub lifecycle: EnvironmentToolGrantLifecycle,
        pub created_at: DateTime<Utc>,
        pub created_by: AccountId,
        pub state_changed_at: DateTime<Utc>,
        pub state_changed_by: AccountId,
    }

    pub struct EnvironmentToolGrantWithDetails {
        pub grant: EnvironmentToolGrant,
        pub release: ToolReleaseMetadata,
        pub release_owner: AccountSummary,
    }

    pub struct EnvironmentToolGrantCreation {
        pub release: ToolReleaseReference,
    }

    pub struct EnvironmentToolGrantReconciliation {
        pub creations: Vec<EnvironmentToolGrantCreation>,
        pub deletions: Vec<EnvironmentToolGrantId>,
    }
}
