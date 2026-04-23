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

use crate::base_model::account::AccountId;
use crate::base_model::agent::{AgentFileContentHash, AgentType};
use crate::base_model::application::ApplicationId;
use crate::base_model::component_metadata::ComponentMetadata;
use crate::base_model::diff;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::base_model::path::{AgentFilePath, ArchiveFilePath};
use crate::base_model::plugin_registration::PluginRegistrationId;
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::base_model::worker::AgentConfigEntryDto;
use crate::model::agent::AgentTypeName;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, declare_unions,
    newtype_uuid,
};
use derive_more::Display;
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::BTreeMap;
use std::str::FromStr;

newtype_uuid!(
    ComponentId,
    wit_name: "component-id",
    wit_owner: "golem:core@1.5.0/types",
    golem_api_grpc::proto::golem::component::ComponentId
);

declare_revision!(ComponentRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct ComponentName(pub String);

    /// Priority of a given plugin. Plugins with a lower priority will be applied before plugins with a higher priority.
    /// There can only be a single plugin with a given priority installed to a component.
    #[derive(Copy, PartialOrd, Eq, Hash, Ord, Display, IntoValue, FromValue)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct PluginPriority(pub i32);
}

impl TryFrom<&str> for ComponentName {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.to_string().try_into()
    }
}

impl TryFrom<String> for ComponentName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err("Component name cannot be empty".to_string());
        }

        if value.contains('@') {
            return Err("Component name cannot contain version suffix (@version)".to_string());
        }

        let parts: Vec<&str> = value.split(':').collect();
        if parts.len() != 2 {
            return Err("Component name must follow the format 'namespace:name'".to_string());
        }

        validate_lower_kebab_case_identifier("Namespace", parts[0])?;
        validate_lower_kebab_case_identifier("Name", parts[1])?;

        Ok(ComponentName(value))
    }
}

impl FromStr for ComponentName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl ComponentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ComponentName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

declare_structs! {
    pub struct ComponentDto {
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
        pub wasm_hash: diff::Hash,
    }

    pub struct ComponentCreation {
        pub component_name: ComponentName,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub agent_types: Vec<AgentType>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfigCreation>,
    }

    pub struct ComponentUpdate {
        pub current_revision: ComponentRevision,
        pub agent_types: Option<Vec<AgentType>>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub agent_type_provision_config_updates: Option<BTreeMap<AgentTypeName, AgentTypeProvisionConfigUpdate>>,
    }

    #[derive(Default)]
    pub struct AgentTypeProvisionConfigCreation {
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub env: BTreeMap<String, String>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub config: Vec<AgentConfigEntryDto>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub plugin_installations: Vec<PluginInstallation>,
        /// key = source path inside uploaded archive; value specifies target path + permissions
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub files: BTreeMap<ArchiveFilePath, AgentFileOptions>,
    }

    #[derive(Default)]
    pub struct AgentTypeProvisionConfigUpdate {
        pub env: Option<BTreeMap<String, String>>,
        pub config: Option<Vec<AgentConfigEntryDto>>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub plugin_updates: Vec<PluginInstallationAction>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub files_to_add_or_update: BTreeMap<ArchiveFilePath, AgentFileOptions>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub files_to_remove: Vec<AgentFilePath>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub file_permission_updates: BTreeMap<AgentFilePath, AgentFilePermissions>,
    }

    pub struct AgentFileOptions {
        pub target_path: AgentFilePath,
        pub permissions: AgentFilePermissions,
    }

    pub struct PluginInstallation {
        pub environment_plugin_grant_id: EnvironmentPluginGrantId,
        /// Plugins will be applied in order of increasing priority
        pub priority: PluginPriority,
        pub parameters: BTreeMap<String, String>,
    }

    pub struct PluginInstallationUpdate {
        /// EnvironmentPluginGrantId to identify the plugin to update
        pub environment_plugin_grant_id: EnvironmentPluginGrantId,
        pub new_priority: Option<PluginPriority>,
        pub new_parameters: Option<BTreeMap<String, String>>,
    }

    pub struct PluginUninstallation {
        /// EnvironmentPluginGrantId to identify the plugin to update
        pub environment_plugin_grant_id: EnvironmentPluginGrantId,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct InstalledPlugin {
        pub environment_plugin_grant_id: EnvironmentPluginGrantId,
        pub priority: PluginPriority,
        pub parameters: BTreeMap<String, String>,

        pub plugin_registration_id: PluginRegistrationId,
        pub plugin_name: String,
        pub plugin_version: String,

        // oplog processor only
        pub oplog_processor_component_id: Option<ComponentId>,
        pub oplog_processor_component_revision: Option<ComponentRevision>,
    }

    #[derive(Eq)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct InitialAgentFile {
        pub content_hash: AgentFileContentHash,
        pub path: AgentFilePath,
        pub permissions: AgentFilePermissions,
        pub size: u64,
    }
}

declare_unions! {
    pub enum PluginInstallationAction {
        Install(PluginInstallation),
        Uninstall(PluginUninstallation),
        Update(PluginInstallationUpdate),
    }
}

declare_enums! {
    #[derive(Default)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum AgentFilePermissions {
        #[default]
        ReadOnly,
        ReadWrite,
    }
}

impl AgentFilePermissions {
    pub fn as_compact_str(&self) -> &'static str {
        match self {
            AgentFilePermissions::ReadOnly => "ro",
            AgentFilePermissions::ReadWrite => "rw",
        }
    }
    pub fn from_compact_str(s: &str) -> Result<Self, String> {
        match s {
            "ro" => Ok(AgentFilePermissions::ReadOnly),
            "rw" => Ok(AgentFilePermissions::ReadWrite),
            _ => Err(format!("Unknown permissions: {s}")),
        }
    }
}
