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

use super::application::ApplicationId;
use crate::{declare_structs, declare_transparent_newtypes, newtype_uuid};
use super::auth::EnvironmentRole;
use super::environment::EnvironmentId;
use super::account::AccountId;
newtype_uuid!(EnvironmentShareId);

declare_structs! {
    pub struct EnvironmentShare {
        pub id: EnvironmentShareId,
        pub environment_id: EnvironmentId,
        pub grantee_account_id: AccountId,
        pub roles: Vec<EnvironmentRole>
    }

    pub struct NewEnvironmentShare {
        pub environment_id: EnvironmentId,
        pub grantee_account_id: AccountId,
        pub roles: Vec<EnvironmentRole>
    }
}
