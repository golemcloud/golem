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

use crate::model::diff::{hash_from_serialized_value, BTreeMapDiff, Diffable, Hash, Hashable};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use desert_rust::BinaryCodec;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, BinaryCodec)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
pub struct McpDeploymentAgentOptions {
    // TODO: MCP agent configuration options coming soon
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeploymentAgentOptionsDiff {
    // TODO: MCP agent configuration diff tracking coming soon
}

impl Diffable for McpDeploymentAgentOptions {
    type DiffResult = McpDeploymentAgentOptionsDiff;

    fn diff(_new: &Self, _current: &Self) -> Option<Self::DiffResult> {
        // TODO: Implement diff when configuration options are added
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeployment {
    pub agents: BTreeMap<String, McpDeploymentAgentOptions>,
}

impl Hashable for McpDeployment {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeploymentDiff {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents_changes: BTreeMapDiff<String, McpDeploymentAgentOptions>,
}

impl Diffable for McpDeployment {
    type DiffResult = McpDeploymentDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let agents_changes = new
            .agents
            .diff_with_current(&current.agents)
            .unwrap_or_default();

        if !agents_changes.is_empty() {
            Some(Self::DiffResult { agents_changes })
        } else {
            None
        }
    }
}
