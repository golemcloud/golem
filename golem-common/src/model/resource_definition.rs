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

use crate::model::diff;

pub use crate::base_model::resource_definition::*;

impl ResourceDefinition {
    pub fn to_diffable(&self) -> diff::ResourceDefinition {
        diff::ResourceDefinition {
            limit: self.limit.clone().into(),
            enforcement_action: self.enforcement_action,
            unit: self.unit.clone(),
            units: self.units.clone(),
        }
    }
}
