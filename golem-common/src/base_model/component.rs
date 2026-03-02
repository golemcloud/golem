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

use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentType;
use crate::base_model::application::ApplicationId;
use crate::base_model::component_metadata::ComponentMetadata;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::base_model::plugin_registration::PluginRegistrationId;
use crate::base_model::{diff, validate_lower_kebab_case_identifier};
use crate::model::agent::AgentTypeName;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, declare_unions,
    newtype_uuid,
};
use derive_more::Display;
use golem_wasm_derive::{FromValue, IntoValue};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::str::FromStr;
use typed_path::Utf8UnixPathBuf;

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

    /// Key that can be used to identify a component file.
    /// All files with the same content will have the same key.
    #[derive(Copy, Display, Eq, Hash)]
    pub struct ComponentFileContentHash(pub diff::Hash);

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
    pub struct LocalAgentConfigEntry {
        pub agent: AgentTypeName,
        pub key: Vec<String>,
        pub value: serde_json::Value
    }

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
        pub files: Vec<InitialComponentFile>,
        pub installed_plugins: Vec<InstalledPlugin>,
        pub env: BTreeMap<String, String>,
        pub config_vars: BTreeMap<String, String>,
        pub local_agent_config: Vec<LocalAgentConfigEntry>,
        pub wasm_hash: diff::Hash,
    }

    pub struct ComponentCreation {
        pub component_name: ComponentName,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub env: BTreeMap<String, String>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub config_vars: BTreeMap<String, String>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub local_agent_config: Vec<LocalAgentConfigEntry>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub agent_types: Vec<AgentType>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub plugins: Vec<PluginInstallation>,
    }

    pub struct ComponentUpdate {
        pub current_revision: ComponentRevision,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub removed_files: Vec<ComponentFilePath>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub new_file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
        pub env: Option<BTreeMap<String, String>>,
        pub config_vars: Option<BTreeMap<String, String>>,
        pub local_agent_config: Option<Vec<LocalAgentConfigEntry>>,
        pub agent_types: Option<Vec<AgentType>>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub plugin_updates: Vec<PluginInstallationAction>,
    }

    #[derive(Default)]
    pub struct ComponentFileOptions {
        /// Path of the file in the uploaded archive
        pub permissions: ComponentFilePermissions,
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

    pub struct InitialComponentFile {
        pub content_hash: ComponentFileContentHash,
        pub path: ComponentFilePath,
        pub permissions: ComponentFilePermissions,
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
    pub enum ComponentFilePermissions {
        #[default]
        ReadOnly,
        ReadWrite,
    }
}

impl ComponentFilePermissions {
    pub fn as_compact_str(&self) -> &'static str {
        match self {
            ComponentFilePermissions::ReadOnly => "ro",
            ComponentFilePermissions::ReadWrite => "rw",
        }
    }
    pub fn from_compact_str(s: &str) -> Result<Self, String> {
        match s {
            "ro" => Ok(ComponentFilePermissions::ReadOnly),
            "rw" => Ok(ComponentFilePermissions::ReadWrite),
            _ => Err(format!("Unknown permissions: {s}")),
        }
    }
}

/// Path inside a component filesystem. Must be
/// - absolute (start with '/')
/// - not contain ".." components
/// - not contain "." components
/// - use '/' as a separator
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, Display)]
pub struct ComponentFilePath(Utf8UnixPathBuf);

impl ComponentFilePath {
    pub fn from_abs_str(s: &str) -> Result<Self, String> {
        let buf: Utf8UnixPathBuf = s.into();
        if !buf.is_absolute() {
            return Err("Path must be absolute".to_string());
        }

        Ok(ComponentFilePath(buf.normalize()))
    }

    pub fn from_rel_str(s: &str) -> Result<Self, String> {
        Self::from_abs_str(&format!("/{s}"))
    }

    pub fn from_either_str(s: &str) -> Result<Self, String> {
        if s.starts_with('/') {
            Self::from_abs_str(s)
        } else {
            Self::from_rel_str(s)
        }
    }

    pub fn as_path(&self) -> &Utf8UnixPathBuf {
        &self.0
    }

    pub fn as_abs_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn to_abs_string(&self) -> String {
        self.0.to_string()
    }

    pub fn to_rel_string(&self) -> String {
        self.0.strip_prefix("/").unwrap().to_string()
    }

    pub fn extend(&mut self, path: &str) -> Result<(), String> {
        self.0.push_checked(path).map_err(|e| e.to_string())
    }
}

impl Serialize for ComponentFilePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        String::serialize(&self.to_string(), serializer)
    }
}

impl<'de> Deserialize<'de> for ComponentFilePath {
    fn deserialize<D>(deserializer: D) -> Result<ComponentFilePath, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = String::deserialize(deserializer)?;
        Self::from_abs_str(&str).map_err(serde::de::Error::custom)
    }
}

impl FromStr for ComponentFilePath {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_either_str(s)
    }
}
