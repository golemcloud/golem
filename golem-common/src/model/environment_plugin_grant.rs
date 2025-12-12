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

use super::environment::EnvironmentId;
use super::plugin_registration::{PluginRegistrationDto, PluginRegistrationId};
use crate::{declare_structs, newtype_uuid};

newtype_uuid!(
    EnvironmentPluginGrantId,
    golem_api_grpc::proto::golem::component::EnvironmentPluginGrantId
);

declare_structs! {
    pub struct EnvironmentPluginGrant {
        pub id: EnvironmentPluginGrantId,
        pub environment_id: EnvironmentId,
        pub plugin: PluginRegistrationDto
    }

    pub struct EnvironmentPluginGrantCreation {
        pub plugin_registration_id: PluginRegistrationId,
    }
}
