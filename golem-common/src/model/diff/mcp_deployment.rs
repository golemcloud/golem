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

use crate::model::diff::{
    BTreeMapDiff, DiffError, Diffable, Hash, Hashable, hash_from_serialized_value,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeploymentAgentOptions {
    pub security_scheme: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeploymentAgentOptionsDiff {
    pub security_scheme_changed: bool,
}

impl Diffable for McpDeploymentAgentOptions {
    type DiffResult = McpDeploymentAgentOptionsDiff;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        Ok(if new.security_scheme != current.security_scheme {
            Some(McpDeploymentAgentOptionsDiff {
                security_scheme_changed: true,
            })
        } else {
            None
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDeployment {
    pub agents: BTreeMap<String, McpDeploymentAgentOptions>,
}

impl Hashable for McpDeployment {
    fn hash(&self) -> Result<Hash, DiffError> {
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

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let agents_changes = new
            .agents
            .diff_with_current(&current.agents)?
            .unwrap_or_default();

        Ok(if !agents_changes.is_empty() {
            Some(Self::DiffResult { agents_changes })
        } else {
            None
        })
    }
}
