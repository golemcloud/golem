// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::application::{
    OAM_COMPONENT_TYPE_WASM, OAM_COMPONENT_TYPE_WASM_BUILD, OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD,
    OAM_TRAIT_TYPE_WASM_RPC,
};
use crate::model::oam::{TypedComponentProperties, TypedTraitProperties};
use crate::model::unknown_properties::{HasUnknownProperties, UnknownProperties};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;

// -- WasmComponentProperties --

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmComponentProperties {
    pub wit: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build: Vec<BuildStep>,
    pub input_wasm: String,
    pub output_wasm: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<InitialFile>,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl HasUnknownProperties for WasmComponentProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

impl TypedComponentProperties for WasmComponentProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildStep {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialFile {
    pub source_path: Resource,
    pub target_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<InitialFilePermissions>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Resource {
    Url(Url),
    Path(PathBuf),
}

#[derive(Default, Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InitialFilePermissions {
    #[default]
    ReadOnly,
    ReadWrite,
}

impl InitialFilePermissions {
    // https://chmod-calculator.com/
    const READ_ONLY_NUMERIC: u32 = 0o444; // r--r--r--
    const READ_WRITE_NUMERIC: u32 = 0o666; // rw-rw-rw-

    pub fn to_unix_file_permissions(&self) -> u32 {
        match self {
            Self::ReadOnly => Self::READ_ONLY_NUMERIC,
            Self::ReadWrite => Self::READ_WRITE_NUMERIC,
        }
    }

    pub fn from_unix_file_permissions(permissions: u32) -> Option<Self> {
        match permissions {
            Self::READ_ONLY_NUMERIC => Some(Self::ReadOnly),
            Self::READ_WRITE_NUMERIC => Some(Self::ReadWrite),
            _ => None,
        }
    }
}

// -- ComponentBuildProperties --

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentBuildProperties {
    pub include: Option<String>,
    pub build_dir: Option<String>,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl TypedComponentProperties for ComponentBuildProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM_BUILD
    }
}

impl HasUnknownProperties for ComponentBuildProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

// -- ComponentStubBuildProperties --

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStubBuildProperties {
    pub component_name: Option<String>,
    pub build_dir: Option<String>,
    pub wasm: Option<String>,
    pub wit: Option<String>,
    pub world: Option<String>,
    pub always_inline_types: Option<bool>,
    pub crate_version: Option<String>,
    pub wasm_rpc_path: Option<String>,
    pub wasm_rpc_version: Option<String>,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl TypedComponentProperties for ComponentStubBuildProperties {
    fn component_type() -> &'static str {
        OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD
    }
}

impl HasUnknownProperties for ComponentStubBuildProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

// -- WasmRpcTraitProperties --

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmRpcTraitProperties {
    pub component_name: String,
    #[serde(flatten)]
    pub unknown_properties: UnknownProperties,
}

impl HasUnknownProperties for WasmRpcTraitProperties {
    fn unknown_properties(&self) -> &UnknownProperties {
        &self.unknown_properties
    }
}

impl TypedTraitProperties for WasmRpcTraitProperties {
    fn trait_type() -> &'static str {
        OAM_TRAIT_TYPE_WASM_RPC
    }
}
