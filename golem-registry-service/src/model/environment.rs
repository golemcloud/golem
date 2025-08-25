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

use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};

#[derive(Debug)]
pub struct Environment {
    pub id: EnvironmentId,
    pub application_id: ApplicationId,
    pub account_id: AccountId,
    pub roles_from_shares: Vec<EnvironmentRole>,
    pub name: EnvironmentName,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

impl Environment {
    pub fn into_common(self) -> golem_common::model::environment::Environment {
        golem_common::model::environment::Environment {
            id: self.id,
            application_id: self.application_id,
            name: self.name,
            compatibility_check: self.compatibility_check,
            version_check: self.version_check,
            security_overrides: self.security_overrides,
        }
    }
}
