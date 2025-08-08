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

use crate::model::diff::{hash_from_serialized_value, Diffable, Hash, Hashable};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDiff {
    pub compatibility_check_changed: bool,
    pub version_check_changed: bool,
    pub security_overrides_changed: bool,
}

impl Diffable for Environment {
    type DiffResult = EnvironmentDiff;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        let diff = EnvironmentDiff {
            compatibility_check_changed: local.compatibility_check != server.compatibility_check,
            version_check_changed: local.version_check != server.version_check,
            security_overrides_changed: local.security_overrides != server.security_overrides,
        };

        let any_changed = diff.compatibility_check_changed
            || diff.version_check_changed
            || diff.security_overrides_changed;

        any_changed.then_some(diff)
    }
}

impl Hashable for Environment {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}
