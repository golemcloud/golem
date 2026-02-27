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

use applying::Apply;
use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{
    ComponentId, ComponentName, ComponentRevision, InitialComponentFile, InstalledPlugin,
};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentId;
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use std::collections::BTreeMap;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct LocalAgentConfigEntry {
    pub agent: AgentTypeName,
    pub key: Vec<String>,
    pub value: ValueAndType,
}

impl From<LocalAgentConfigEntry> for golem_common::model::component::LocalAgentConfigEntry {
    fn from(value: LocalAgentConfigEntry) -> Self {
        Self {
            agent: value.agent,
            key: value.key,
            value: value
                .value
                .to_json_value()
                .expect("ValueAndType produced by service must be valid JSON"),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::LocalAgentConfigEntry>
    for LocalAgentConfigEntry
{
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::component::LocalAgentConfigEntry,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            agent: AgentTypeName(value.agent),
            key: value.key,
            value: value
                .value
                .ok_or_else(|| "Missing field: value".to_string())?
                .try_into()?,
        })
    }
}

impl From<LocalAgentConfigEntry>
    for golem_api_grpc::proto::golem::component::LocalAgentConfigEntry
{
    fn from(value: LocalAgentConfigEntry) -> Self {
        Self {
            agent: value.agent.0,
            key: value.key,
            value: Some(value.value.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Component {
    pub id: ComponentId,
    pub revision: ComponentRevision,
    pub environment_id: EnvironmentId,
    pub component_name: ComponentName,
    pub hash: diff::Hash,
    pub application_id: ApplicationId,
    pub account_id: AccountId,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<InstalledPlugin>,
    pub env: BTreeMap<String, String>,
    pub config_vars: BTreeMap<String, String>,
    pub local_agent_config: Vec<LocalAgentConfigEntry>,

    /// Hash of the wasm before any transformations
    pub wasm_hash: diff::Hash,

    pub original_files: Vec<InitialComponentFile>,
    pub original_env: BTreeMap<String, String>,
    pub original_config_vars: BTreeMap<String, String>,
    pub object_store_key: String,
    pub transformed_object_store_key: String,
}

impl From<Component> for golem_common::model::component::ComponentDto {
    fn from(value: Component) -> Self {
        Self {
            id: value.id,
            revision: value.revision,
            environment_id: value.environment_id,
            application_id: value.application_id,
            account_id: value.account_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: value.created_at,
            original_files: value.original_files,
            files: value.files,
            installed_plugins: value.installed_plugins,
            original_env: value.original_env,
            env: value.env,
            original_config_vars: value.original_config_vars,
            config_vars: value.config_vars,
            local_agent_config: value
                .local_agent_config
                .into_iter()
                .map(golem_common::model::component::LocalAgentConfigEntry::from)
                .collect(),
            wasm_hash: value.wasm_hash,
            hash: value.hash,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        let id = value
            .component_id
            .ok_or("Missing component id")?
            .try_into()
            .map_err(|e| format!("Invalid component id: {}", e))?;

        let revision = ComponentRevision::try_from(value.revision)?;

        let environment_id = value
            .environment_id
            .ok_or("Missing environment id")?
            .try_into()
            .map_err(|e| format!("Invalid environment id: {}", e))?;

        let application_id = value
            .application_id
            .ok_or("Missing application id")?
            .try_into()
            .map_err(|e| format!("Invalid application id: {}", e))?;

        let account_id = value
            .account_id
            .ok_or("Missing account id")?
            .try_into()
            .map_err(|e| format!("Invalid account id: {}", e))?;

        let component_name = ComponentName(value.component_name);
        let component_size = value.component_size;
        let metadata = value
            .metadata
            .ok_or("Missing metadata")?
            .try_into()
            .map_err(|e| format!("Invalid metadata: {}", e))?;

        let created_at = value
            .created_at
            .ok_or("missing created_at")?
            .apply(SystemTime::try_from)
            .map_err(|_| "Failed to convert timestamp".to_string())?
            .into();

        let original_files = value
            .original_files
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        let files = value
            .files
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        let installed_plugins = value
            .installed_plugins
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        let original_env = value.original_env.into_iter().collect::<BTreeMap<_, _>>();
        let env = value.env.into_iter().collect::<BTreeMap<_, _>>();

        let original_config_vars = value
            .original_config_vars
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let config_vars = value.config_vars.into_iter().collect::<BTreeMap<_, _>>();

        let local_agent_config = value
            .local_agent_config
            .into_iter()
            .map(LocalAgentConfigEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        let hash = value.hash.ok_or("Missing hash field")?.try_into()?;

        let wasm_hash = value
            .wasm_hash
            .ok_or("Missing wasm hash field")?
            .try_into()?;

        Ok(Self {
            id,
            revision,
            environment_id,
            application_id,
            account_id,
            component_name,
            component_size,
            metadata,
            created_at,
            original_files,
            files,
            installed_plugins,
            original_env,
            env,
            original_config_vars,
            config_vars,
            local_agent_config,
            wasm_hash,
            hash,
            object_store_key: value.object_store_key,
            transformed_object_store_key: value.transformed_object_store_key,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            component_id: Some(value.id.into()),
            revision: value.revision.into(),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            account_id: Some(value.account_id.into()),
            application_id: Some(value.application_id.into()),
            environment_id: Some(value.environment_id.into()),
            created_at: Some(prost_types::Timestamp::from(SystemTime::from(
                value.created_at,
            ))),
            original_files: value
                .original_files
                .into_iter()
                .map(|file| file.into())
                .collect(),
            files: value.files.into_iter().map(|file| file.into()).collect(),
            installed_plugins: value
                .installed_plugins
                .into_iter()
                .map(|plugin| plugin.into())
                .collect(),
            original_env: value.original_env.into_iter().collect(),
            env: value.env.into_iter().collect(),
            original_config_vars: value.original_config_vars.into_iter().collect(),
            config_vars: value.config_vars.into_iter().collect(),
            local_agent_config: value
                .local_agent_config
                .into_iter()
                .map(golem_api_grpc::proto::golem::component::LocalAgentConfigEntry::from)
                .collect(),
            wasm_hash: Some(value.wasm_hash.into()),
            hash: Some(value.hash.into()),
            object_store_key: value.object_store_key,
            transformed_object_store_key: value.transformed_object_store_key,
        }
    }
}
