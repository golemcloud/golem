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

use super::environment::EnvironmentId;
use super::PluginInstallationId;
use crate::base_model::ComponentId;
use crate::model::account::AccountId;
use crate::{declare_structs, declare_transparent_newtypes};
use bincode::{Decode, Encode};
use core::fmt;
use golem_wasm_rpc_derive::IntoValue;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;
use typed_path::Utf8UnixPathBuf;
use crate::model::component_metadata::ComponentMetadata;
use std::collections::BTreeMap;

declare_transparent_newtypes! {
    // TODO: Add validations (non-empty, no "/", no " ", ...)
    pub struct ComponentName(pub String);

    #[derive(Copy, Hash, Eq, PartialOrd, Ord, derive_more::FromStr,  Encode, Decode, IntoValue)]
    pub struct ComponentRevision(pub u64);
}

declare_structs! {
    pub struct Component {
        pub environment_id: EnvironmentId,
        pub versioned_component_id: VersionedComponentId,
        pub component_name: ComponentName,
        pub component_size: u64,
        pub metadata: ComponentMetadata,
        pub created_at: chrono::DateTime<chrono::Utc>,
        pub component_type: ComponentType,
        pub files: Vec<InitialComponentFile>,
        pub installed_plugins: Vec<PluginInstallation>,
        pub env: BTreeMap<String, String>,
        pub wasm_hash: crate::model::diff::Hash,
    }

    #[derive(Default)]
    pub struct ComponentFileOptions {
        /// Path of the file in the uploaded archive
        pub permissions: ComponentFilePermissions,
    }

    pub struct PluginInstallation {
        pub id: PluginInstallationId,
        pub name: String,
        pub version: String,
        /// Whether the referenced plugin is still registered. If false, the installation will still work but the plugin will not show up when listing plugins.
        pub registered: bool,
        pub priority: i32,
        pub parameters: HashMap<String, String>,
    }
}

/// Key that can be used to identify a component file.
/// All files with the same content will have the same key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, derive_more::Display)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
pub struct InitialComponentFileKey(pub String);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Encode, Decode, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
#[repr(i32)]
pub enum ComponentType {
    Durable = 0,
    Ephemeral = 1,
}

impl TryFrom<i32> for ComponentType {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ComponentType::Durable),
            1 => Ok(ComponentType::Ephemeral),
            _ => Err(format!("Unknown Component Type: {value}")),
        }
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct InitialComponentFile {
    pub key: InitialComponentFileKey,
    pub path: ComponentFilePath,
    pub permissions: ComponentFilePermissions,
}

impl InitialComponentFile {
    pub fn is_read_only(&self) -> bool {
        self.permissions == ComponentFilePermissions::ReadOnly
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ComponentOwner {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
}

impl Display for ComponentOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.account_id, self.environment_id)
    }
}
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]

pub struct VersionedComponentId {
    pub component_id: ComponentId,
    pub version: ComponentRevision,
}

impl Display for VersionedComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.component_id, self.version)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "poem", oai(rename_all = "kebab-case"))]
pub enum ComponentFilePermissions {
    ReadOnly,
    ReadWrite,
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

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::ComponentRevision;
    use crate::model::component::VersionedComponentId;

    impl TryFrom<golem_api_grpc::proto::golem::component::VersionedComponentId>
        for VersionedComponentId
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::VersionedComponentId,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: value
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                version: ComponentRevision(value.version),
            })
        }
    }

    impl From<VersionedComponentId> for golem_api_grpc::proto::golem::component::VersionedComponentId {
        fn from(value: VersionedComponentId) -> Self {
            Self {
                component_id: Some(value.component_id.into()),
                version: value.version.0,
            }
        }
    }
}
