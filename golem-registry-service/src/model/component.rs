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

use golem_common::model::agent::AgentType;
use golem_common::model::component::InitialComponentFile;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component::{ComponentName, InstalledPlugin};
use golem_common::model::component_metadata::{ComponentMetadata, ComponentProcessingError};
use golem_common::model::diff::Hash;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::model::{Component, LocalAgentConfigEntry};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct NewComponentRevision {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub original_files: Vec<InitialComponentFile>,
    pub files: Vec<InitialComponentFile>,
    pub original_env: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub original_config_vars: BTreeMap<String, String>,
    pub config_vars: BTreeMap<String, String>,
    pub local_agent_config: Vec<LocalAgentConfigEntry>,
    pub wasm_hash: Hash,
    pub object_store_key: String,
    pub installed_plugins: Vec<InstalledPlugin>,
    pub agent_types: Vec<AgentType>,
}

impl NewComponentRevision {
    pub fn new(
        component_id: ComponentId,
        component_revision: ComponentRevision,
        environment_id: EnvironmentId,
        component_name: ComponentName,
        files: Vec<InitialComponentFile>,
        env: BTreeMap<String, String>,
        config_vars: BTreeMap<String, String>,
        local_agent_config: Vec<LocalAgentConfigEntry>,
        wasm_hash: Hash,
        object_store_key: String,
        installed_plugins: Vec<InstalledPlugin>,
        agent_types: Vec<AgentType>,
    ) -> Self {
        Self {
            component_id,
            component_revision,
            environment_id,
            component_name,
            original_files: files.clone(),
            files,
            original_env: env.clone(),
            env,
            original_config_vars: config_vars.clone(),
            config_vars,
            local_agent_config,
            wasm_hash,
            object_store_key,
            installed_plugins,
            agent_types,
        }
    }

    pub fn from_existing(value: Component) -> anyhow::Result<Self> {
        Ok(Self {
            component_id: value.id,
            component_revision: value.revision.next()?,
            environment_id: value.environment_id,
            component_name: value.component_name,
            original_files: value.original_files,
            files: value.files,
            original_env: value.original_env,
            env: value.env,
            original_config_vars: value.original_config_vars,
            config_vars: value.config_vars,
            local_agent_config: value.local_agent_config,
            wasm_hash: value.wasm_hash,
            object_store_key: value.object_store_key,
            installed_plugins: value.installed_plugins,
            agent_types: value.metadata.agent_types().to_vec(),
        })
    }

    pub fn with_transformed_component(
        self,
        transformed_object_store_key: String,
        transformed_data: &[u8],
    ) -> Result<FinalizedComponentRevision, ComponentProcessingError> {
        let metadata = ComponentMetadata::analyse_component(transformed_data, self.agent_types)?;

        Ok(FinalizedComponentRevision {
            component_id: self.component_id,
            component_revision: self.component_revision,
            environment_id: self.environment_id,
            component_name: self.component_name,
            original_files: self.original_files,
            files: self.files,
            original_env: self.original_env,
            env: self.env,
            original_config_vars: self.original_config_vars,
            config_vars: self.config_vars,
            local_agent_config: self.local_agent_config,
            wasm_hash: self.wasm_hash,
            object_store_key: self.object_store_key,
            installed_plugins: self.installed_plugins,
            transformed_object_store_key,
            metadata,
            component_size: transformed_data.len() as u64,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FinalizedComponentRevision {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub original_files: Vec<InitialComponentFile>,
    pub files: Vec<InitialComponentFile>,
    pub original_env: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub original_config_vars: BTreeMap<String, String>,
    pub config_vars: BTreeMap<String, String>,
    pub local_agent_config: Vec<LocalAgentConfigEntry>,
    pub wasm_hash: golem_common::model::diff::Hash,
    pub object_store_key: String,
    pub installed_plugins: Vec<InstalledPlugin>,

    pub transformed_object_store_key: String,
    pub metadata: ComponentMetadata,
    pub component_size: u64,
}
