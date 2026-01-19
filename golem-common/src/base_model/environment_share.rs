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

use crate::base_model::account::AccountId;
use crate::base_model::auth::EnvironmentRole;
use crate::base_model::environment::EnvironmentId;
use crate::{declare_revision, declare_structs, newtype_uuid};
use std::collections::BTreeSet;

newtype_uuid!(EnvironmentShareId);

declare_revision!(EnvironmentShareRevision);

declare_structs! {
    pub struct EnvironmentShare {
        pub id: EnvironmentShareId,
        pub revision: EnvironmentShareRevision,
        pub environment_id: EnvironmentId,
        pub grantee_account_id: AccountId,
        pub roles: BTreeSet<EnvironmentRole>
    }

    pub struct EnvironmentShareCreation {
        pub grantee_account_id: AccountId,
        pub roles: BTreeSet<EnvironmentRole>
    }

    pub struct EnvironmentShareUpdate {
        pub current_revision: EnvironmentShareRevision,
        pub roles: BTreeSet<EnvironmentRole>
    }
}
