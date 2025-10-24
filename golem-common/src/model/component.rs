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

use super::agent::AgentType;
use super::auth::EnvironmentRole;
use super::component_metadata::DynamicLinkedInstance;
use super::environment::EnvironmentId;
use super::environment_plugin_grant::EnvironmentPluginGrantId;
use super::plugin_registration::PluginRegistrationId;
use crate::model::account::AccountId;
use crate::model::application::ApplicationId;
use crate::model::component_metadata::ComponentMetadata;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, declare_unions,
    newtype_uuid,
};
use bincode::{Decode, Encode};
use derive_more::Display;
use golem_wasm_derive::IntoValue;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;
use strum_macros::FromRepr;
use typed_path::Utf8UnixPathBuf;
use uuid::Uuid;

newtype_uuid!(
    ComponentId,
    golem_api_grpc::proto::golem::component::ComponentId
);

declare_revision!(ComponentRevision);

declare_transparent_newtypes! {
    #[derive(Display)]
    pub struct ComponentName(pub String);

    /// Key that can be used to identify a component file.
    /// All files with the same content will have the same key.
    #[derive(Display, Eq, Hash)]
    pub struct InitialComponentFileKey(pub String);

    /// Priority of a given plugin. Plugins with a lower priority will be applied before plugins with a higher priority.
    /// There can only be a single plugin with a given priority installed to a component.
    #[derive(Copy, PartialOrd, Eq, Hash, Ord, derive_more::Display, Encode, Decode, IntoValue)]
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
        // TODO: Add validations (non-empty, no "/", no " ", ...)
        Ok(ComponentName(value.to_string()))
    }
}

impl FromStr for ComponentName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

declare_structs! {
    pub struct ComponentCreation {
        pub component_name: ComponentName,
        pub component_type: Option<ComponentType>,
        #[serde(default)]
        #[oai(default)]
        pub file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
        #[serde(default)]
        #[oai(default)]
        pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        #[serde(default)]
        #[oai(default)]
        pub env: BTreeMap<String, String>,
        #[serde(default)]
        #[oai(default)]
        pub agent_types: Vec<AgentType>,
        #[serde(default)]
        #[oai(default)]
        pub plugins: Vec<PluginInstallation>,
    }

    pub struct ComponentUpdate {
        pub current_revision: ComponentRevision,
        pub component_type: Option<ComponentType>,
        #[serde(default)]
        #[oai(default)]
        pub removed_files: Vec<ComponentFilePath>,
        #[serde(default)]
        #[oai(default)]
        pub new_file_options: BTreeMap<ComponentFilePath, ComponentFileOptions>,
        pub dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        pub env: Option<BTreeMap<String, String>>,
        pub agent_types: Option<Vec<AgentType>>,
        #[serde(default)]
        #[oai(default)]
        pub plugin_updates: Vec<PluginInstallationAction>,
    }

    pub struct ComponentDto {
        pub id: ComponentId,
        pub revision: ComponentRevision,
        pub environment_id: EnvironmentId,
        pub application_id: ApplicationId,
        pub account_id: AccountId,
        pub component_name: ComponentName,
        pub component_size: u64,
        pub metadata: ComponentMetadata,
        pub created_at: chrono::DateTime<chrono::Utc>,
        pub component_type: ComponentType,
        pub files: Vec<InitialComponentFile>,
        pub installed_plugins: Vec<InstalledPlugin>,
        pub env: BTreeMap<String, String>,
        pub wasm_hash: crate::model::diff::Hash,
        pub environment_roles_from_shares: HashSet<EnvironmentRole>
    }

    #[derive(Default)]
    pub struct ComponentFileOptions {
        /// Path of the file in the uploaded archive
        pub permissions: ComponentFilePermissions,
    }

    pub struct InstalledPlugin {
        pub plugin_registration_id: PluginRegistrationId,
        pub priority: PluginPriority,
        pub parameters: BTreeMap<String, String>,
    }

    pub struct PluginInstallation {
        pub environment_plugin_grant_id: EnvironmentPluginGrantId,
        /// Plugins will be applied in order of increasing priority
        pub priority: PluginPriority,
        pub parameters: BTreeMap<String, String>,
    }

    pub struct PluginInstallationUpdate {
        /// Priority will be used to identify the plugin to update
        pub plugin_priority: PluginPriority,
        pub new_priority: Option<PluginPriority>,
        pub new_parameters: Option<BTreeMap<String, String>>,
    }

    pub struct PluginUninstallation {
        /// Priority will be used to identify the plugin to delete
        pub plugin_priority: PluginPriority
    }

    pub struct InitialComponentFile {
        pub key: InitialComponentFileKey,
        pub path: ComponentFilePath,
        pub permissions: ComponentFilePermissions,
    }
}

impl InitialComponentFile {
    pub fn is_read_only(&self) -> bool {
        self.permissions == ComponentFilePermissions::ReadOnly
    }
}

declare_enums! {
    #[derive(Encode, Decode, FromRepr)]
    #[repr(i32)]
    pub enum ComponentType {
        Durable = 0,
        Ephemeral = 1,
    }

    pub enum ComponentFilePermissions {
        ReadOnly,
        ReadWrite,
    }
}

impl Display for ComponentType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ComponentType::Durable => "Durable",
            ComponentType::Ephemeral => "Ephemeral",
        };
        write!(f, "{s}")
    }
}

impl FromStr for ComponentType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Durable" => Ok(ComponentType::Durable),
            "Ephemeral" => Ok(ComponentType::Ephemeral),
            _ => Err(format!("Unknown Component Type: {s}")),
        }
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

impl Default for ComponentFilePermissions {
    fn default() -> Self {
        Self::ReadOnly
    }
}

declare_unions! {

    pub enum PluginInstallationAction {
        Install(PluginInstallation),
        Uninstall(PluginUninstallation),
        Update(PluginInstallationUpdate),
    }

}

/// Path inside a component filesystem. Must be
/// - absolute (start with '/')
/// - not contain ".." components
/// - not contain "." components
/// - use '/' as a separator
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Hash, derive_more::Display)]
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

impl From<golem_wasm::ComponentId> for ComponentId {
    fn from(host: golem_wasm::ComponentId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<ComponentId> for golem_wasm::ComponentId {
    fn from(component_id: ComponentId) -> Self {
        let (high_bits, low_bits) = component_id.0.as_u64_pair();

        golem_wasm::ComponentId {
            uuid: golem_wasm::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

mod protobuf {
    use super::{ComponentDto, InstalledPlugin};
    use super::{ComponentName, ComponentRevision, ComponentType, PluginPriority};
    use crate::model::auth::EnvironmentRole;
    use applying::Apply;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    impl From<InstalledPlugin> for golem_api_grpc::proto::golem::component::PluginInstallation {
        fn from(value: InstalledPlugin) -> Self {
            Self {
                plugin_registration_id: Some(value.plugin_registration_id.into()),
                priority: value.priority.0,
                parameters: value.parameters.into_iter().collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginInstallation> for InstalledPlugin {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::component::PluginInstallation,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                plugin_registration_id: value
                    .plugin_registration_id
                    .ok_or("Missing plugin_registration_id")?
                    .try_into()?,
                priority: PluginPriority(value.priority),
                parameters: value.parameters.into_iter().collect(),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::Component> for ComponentDto {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::component::Component,
        ) -> Result<Self, Self::Error> {
            let id = value
                .component_id
                .ok_or("Missing component id")?
                .try_into()
                .map_err(|e| format!("Invalid component id: {}", e))?;

            let revision = ComponentRevision(value.revision);

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

            let component_type = value
                .component_type
                .ok_or("missing component type")?
                .apply(ComponentType::from_repr)
                .ok_or("Invalid component type")?;

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

            let env = value.env.into_iter().collect::<BTreeMap<_, _>>();

            let wasm_hash = value
                .wasm_hash_bytes
                .into_iter()
                .map(|b| b as u8)
                .collect::<Vec<_>>()
                .apply(|bs| blake3::Hash::from_slice(&bs))
                .map_err(|e| format!("Invalid wasm hash bytes: {e}"))?
                .apply(crate::model::diff::Hash::from);

            let environment_roles_from_shares = value
                .environment_roles_from_shares
                .into_iter()
                .map(|ar| {
                    golem_api_grpc::proto::golem::auth::EnvironmentRole::try_from(ar)
                        .map_err(|e| format!("Failed converting environment role: {e}"))
                        .map(EnvironmentRole::from)
                })
                .collect::<Result<_, _>>()?;

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
                component_type,
                files,
                installed_plugins,
                env,
                wasm_hash,
                environment_roles_from_shares,
            })
        }
    }
}

// #[derive(
//     Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
// )]
// #[serde(rename_all = "camelCase")]
// #[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
// #[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]

// pub struct VersionedComponentId {
//     pub component_id: ComponentId,
//     pub version: ComponentRevision,
// }

// impl Display for VersionedComponentId {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}#{}", self.component_id, self.version)
//     }
// }

// #[cfg(feature = "protobuf")]
// mod protobuf {
//     use super::ComponentRevision;
//     use crate::model::component::VersionedComponentId;

//     impl TryFrom<golem_api_grpc::proto::golem::component::VersionedComponentId>
//         for VersionedComponentId
//     {
//         type Error = String;

//         fn try_from(
//             value: golem_api_grpc::proto::golem::component::VersionedComponentId,
//         ) -> Result<Self, Self::Error> {
//             Ok(Self {
//                 component_id: value
//                     .component_id
//                     .ok_or("Missing component_id")?
//                     .try_into()?,
//                 version: ComponentRevision(value.version),
//             })
//         }
//     }

//     impl From<VersionedComponentId> for golem_api_grpc::proto::golem::component::VersionedComponentId {
//         fn from(value: VersionedComponentId) -> Self {
//             Self {
//                 component_id: Some(value.component_id.into()),
//                 version: value.version.0,
//             }
//         }
//     }
// }
