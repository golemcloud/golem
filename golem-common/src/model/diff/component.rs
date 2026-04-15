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

use crate::model::diff::agent::AgentTypeProvisionConfig;
use crate::model::diff::hash::{Hash, HashOf, Hashable, hash_from_serialized_value};
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use serde::Serialize;
use std::collections::BTreeMap;

/// Top-level diffable component state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub wasm_hash: Hash,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub agent_type_provision_configs: BTreeMap<String, HashOf<AgentTypeProvisionConfig>>,
}

/// Top-level component diff result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentDiff {
    pub wasm_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_type_provision_config_changes: BTreeMapDiff<String, HashOf<AgentTypeProvisionConfig>>,
}

impl Diffable for Component {
    type DiffResult = ComponentDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let wasm_changed = new.wasm_hash != current.wasm_hash;
        let agent_type_provision_config_changes = new
            .agent_type_provision_configs
            .diff_with_current(&current.agent_type_provision_configs)
            .unwrap_or_default();

        if wasm_changed || !agent_type_provision_config_changes.is_empty() {
            Some(ComponentDiff {
                wasm_changed,
                agent_type_provision_config_changes,
            })
        } else {
            None
        }
    }
}

impl Hashable for Component {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}
