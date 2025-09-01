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

use std::collections::HashMap;

use golem_wasm_ast::analysis::AnalysedExport;
use serde::{Deserialize, Serialize};

use crate::model::{Component, ComponentName};
use golem_common::model::agent::AgentType;
use golem_common::model::component::{ComponentOwner, VersionedComponentId};
use golem_common::model::component_metadata::{
    ComponentMetadata, DynamicLinkedInstance, LinearMemory,
};
use golem_common::model::{
    AccountId, ComponentId, ComponentType, ComponentVersion, InitialComponentFile, ProjectId,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileSystemComponentMetadata {
    pub account_id: AccountId,
    pub project_id: ProjectId,
    pub component_id: ComponentId,
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub component_name: String,
    pub wasm_filename: String,

    #[serde(default)]
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    pub agent_types: Vec<AgentType>,
}

impl From<LocalFileSystemComponentMetadata> for Component {
    fn from(value: LocalFileSystemComponentMetadata) -> Self {
        Self {
            owner: ComponentOwner {
                account_id: value.account_id,
                project_id: value.project_id,
            },
            versioned_component_id: VersionedComponentId {
                component_id: value.component_id,
                version: value.version,
            },
            component_name: ComponentName(value.component_name),
            component_size: value.size,
            metadata: ComponentMetadata::from_parts(
                value.exports,
                value.memories,
                value.dynamic_linking,
                None,
                None,
                value.agent_types,
            ),
            created_at: Default::default(),
            component_type: value.component_type,
            files: value.files,
            installed_plugins: vec![],
            env: value.env,
        }
    }
}
