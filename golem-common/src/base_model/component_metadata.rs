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

use crate::base_model::agent::{AgentConstructor, AgentMethod, AgentType};
use crate::base_model::base64::Base64;
use golem_wasm::analysis::AnalysedExport;
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum AgentMethodOrConstructor {
    Method(AgentMethod),
    Constructor(AgentConstructor),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProducerField {
    pub name: String,
    pub values: Vec<VersionedName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct VersionedName {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct Producers {
    pub fields: Vec<ProducerField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LinearMemory {
    /// Initial size of the linear memory in bytes
    pub initial: u64,
    /// Optional maximal size of the linear memory in bytes
    pub maximum: Option<u64>,
}

impl LinearMemory {
    #[allow(dead_code)]
    pub(crate) const PAGE_SIZE: u64 = 65536;
}

#[derive(Clone, Default)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[allow(dead_code)]
pub struct ComponentMetadata {
    pub(crate) data: Arc<ComponentMetadataInnerData>,
    #[cfg(feature = "full")]
    #[cfg_attr(feature = "full", transient(Default::default()))]
    pub(crate) cache:
        Arc<std::sync::Mutex<crate::model::component_metadata::ComponentMetadataInnerCache>>,
}

impl Debug for ComponentMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentMetadata")
            .field("exports", &self.data.exports)
            .field("producers", &self.data.producers)
            .field("memories", &self.data.memories)
            .field("binary_wit_len", &self.data.binary_wit.len())
            .field("root_package_name", &self.data.root_package_name)
            .field("root_package_version", &self.data.root_package_version)
            .field("agent_types", &self.data.agent_types)
            .finish()
    }
}

impl PartialEq for ComponentMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl Eq for ComponentMetadata {}

impl Serialize for ComponentMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.data.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ComponentMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = ComponentMetadataInnerData::deserialize(deserializer)?;
        Ok(Self {
            data: Arc::new(data),
            #[cfg(feature = "full")]
            cache: Arc::default(),
        })
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(
    feature = "full",
    oai(rename = "ComponentMetadata", rename_all = "camelCase")
)]
#[serde(rename = "ComponentMetadata", rename_all = "camelCase")]
pub struct ComponentMetadataInnerData {
    pub exports: Vec<AnalysedExport>,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,
    #[serde(default)]
    pub binary_wit: Base64,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,

    #[serde(default)]
    pub agent_types: Vec<AgentType>,
}
