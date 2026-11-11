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

use crate::base_model::component::{InitialAgentFile, InstalledPlugin};
use crate::base_model::tool::{ToolDeploymentMetadata, ToolName};
use crate::base_model::tool_middleware::{ToolMiddlewareDeploymentMetadata, ToolMiddlewareName};
use crate::base_model::worker::TypedAgentConfigEntry;
use crate::model::agent::AgentTypeName;
use crate::model::card::PolymorphicCard;
use crate::schema::{AgentTypeSchema, ComponentConfigSchema};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

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

/// Compact index of known exported interfaces in a component.
/// Instead of persisting the full `AnalysedExport` tree, we store only
/// the exact versioned interface name for each supported capability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct KnownExports {
    /// Exact exported interface name for `golem:agent/guest`, e.g. `golem:agent/guest@1.5.0`
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub agent_guest_interface: Option<String>,
    /// Exact exported interface name for `golem:api/save-snapshot`
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub save_snapshot_interface: Option<String>,
    /// Exact exported interface name for `golem:api/load-snapshot`
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub load_snapshot_interface: Option<String>,
    /// Exact exported interface name for `golem:api/oplog-processor`
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub oplog_processor_interface: Option<String>,
    /// Exact exported interface name for `golem:tool/guest`, e.g. `golem:tool/guest@0.1.0`
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub tool_guest_interface: Option<String>,
    /// Exact exported interface name for `golem:tool/tool-middleware-guest`.
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub tool_middleware_guest_interface: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LinearMemory {
    /// Initial size of the linear memory in bytes
    pub initial: u64,
    /// Optional maximal size of the linear memory in bytes
    pub maximum: Option<u64>,
    /// Whether the linear memory is shared between WebAssembly threads
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub shared: bool,
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
}

impl Debug for ComponentMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentMetadata")
            .field("known_exports", &self.data.known_exports)
            .field("producers", &self.data.producers)
            .field("memories", &self.data.memories)
            .field("agent_types", &self.data.agent_types)
            .field("agent_method_streams", &self.data.agent_method_streams)
            .field(
                "agent_type_provision_configs",
                &self.data.agent_type_provision_configs,
            )
            .field("tools", &self.data.tools)
            .field("tool_middlewares", &self.data.tool_middlewares)
            .finish()
    }
}

impl PartialEq for ComponentMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

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
        })
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
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
    pub known_exports: KnownExports,
    pub producers: Vec<Producers>,
    pub memories: Vec<LinearMemory>,

    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub config_schema: ComponentConfigSchema,

    pub component_provision_config: ComponentProvisionConfig,

    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub agent_types: Vec<AgentTypeSchema>,

    /// Registry-assigned identities, separate from guest declarations.
    pub agent_type_ids: BTreeMap<AgentTypeName, crate::base_model::type_id::TypeId>,
    pub tool_type_ids: BTreeMap<ToolName, crate::base_model::type_id::TypeId>,

    /// Server-derived streaming classification for each declared agent method.
    pub agent_method_streams: BTreeMap<AgentTypeName, BTreeMap<String, AgentMethodStreamMetadata>>,

    /// Per-agent-type provisioning configuration: env, config, plugins, files.
    /// Kept separate from agent type declarations so AgentType stays a pure declaration type.
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,

    /// Complete deployment input for every tool implemented by this component.
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub tools: BTreeMap<ToolName, ToolDeploymentMetadata>,

    /// Complete deployment input for every tool middleware implemented by this component.
    #[serde(default)]
    #[cfg_attr(feature = "full", oai(default))]
    pub tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
}

crate::declare_structs! {
    /// Component metadata projected for REST responses, with identities beside declarations.
    pub struct ComponentMetadataDto {
        pub known_exports: KnownExports,
        pub producers: Vec<Producers>,
        pub memories: Vec<LinearMemory>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub config_schema: ComponentConfigSchema,
        pub component_provision_config: ComponentProvisionConfig,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub agent_types: Vec<ComponentAgentTypeDto>,
        pub agent_method_streams: BTreeMap<AgentTypeName, BTreeMap<String, AgentMethodStreamMetadata>>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub tools: BTreeMap<ToolName, ComponentToolDto>,
        #[serde(default)]
        #[cfg_attr(feature = "full", oai(default))]
        pub tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
    }

    pub struct ComponentAgentTypeDto {
        pub declaration: AgentTypeSchema,
        /// Registry-assigned identity when available for this declaration.
        #[cfg_attr(feature = "full", oai(nullable))]
        pub type_id: Option<crate::base_model::type_id::TypeId>,
    }

    pub struct ComponentToolDto {
        pub declaration: ToolDeploymentMetadata,
        /// Registry-assigned identity when available for this declaration.
        #[cfg_attr(feature = "full", oai(nullable))]
        pub type_id: Option<crate::base_model::type_id::TypeId>,
    }
}

impl From<ComponentMetadata> for ComponentMetadataDto {
    fn from(value: ComponentMetadata) -> Self {
        let data = Arc::unwrap_or_clone(value.data);
        Self {
            known_exports: data.known_exports,
            producers: data.producers,
            memories: data.memories,
            config_schema: data.config_schema,
            component_provision_config: data.component_provision_config,
            agent_types: data
                .agent_types
                .into_iter()
                .map(|declaration| ComponentAgentTypeDto {
                    type_id: data.agent_type_ids.get(&declaration.type_name).copied(),
                    declaration,
                })
                .collect(),
            agent_method_streams: data.agent_method_streams,
            agent_type_provision_configs: data.agent_type_provision_configs,
            tools: data
                .tools
                .into_iter()
                .map(|(name, declaration)| {
                    let type_id = data.tool_type_ids.get(&name).copied();
                    (
                        name,
                        ComponentToolDto {
                            declaration,
                            type_id,
                        },
                    )
                })
                .collect(),
            tool_middlewares: data.tool_middlewares,
        }
    }
}

impl From<ComponentMetadataDto> for ComponentMetadata {
    fn from(value: ComponentMetadataDto) -> Self {
        let mut agent_type_ids = BTreeMap::new();
        let agent_types = value
            .agent_types
            .into_iter()
            .map(|agent| {
                if let Some(id) = agent.type_id {
                    agent_type_ids.insert(agent.declaration.type_name.clone(), id);
                }
                agent.declaration
            })
            .collect();
        let mut tool_type_ids = BTreeMap::new();
        let tools = value
            .tools
            .into_iter()
            .map(|(name, tool)| {
                if let Some(id) = tool.type_id {
                    tool_type_ids.insert(name.clone(), id);
                }
                (name, tool.declaration)
            })
            .collect();
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports: value.known_exports,
                producers: value.producers,
                memories: value.memories,
                config_schema: value.config_schema,
                component_provision_config: value.component_provision_config,
                agent_types,
                agent_type_ids,
                tool_type_ids,
                agent_method_streams: value.agent_method_streams,
                agent_type_provision_configs: value.agent_type_provision_configs,
                tools,
                tool_middlewares: value.tool_middlewares,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentProvisionConfig {
    pub initial_permissions: PolymorphicCard,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub config: Vec<TypedAgentConfigEntry>,
    #[serde(default)]
    pub plugins: Vec<InstalledPlugin>,
    #[serde(default)]
    pub files: Vec<InitialAgentFile>,
}

impl Default for ComponentProvisionConfig {
    fn default() -> Self {
        Self {
            initial_permissions: crate::model::component::AgentTypeInitialPermissions::default()
                .to_polymorphic_card(),
            env: BTreeMap::new(),
            config: Vec::new(),
            plugins: Vec::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentMethodStreamMetadata {
    pub input: bool,
    pub output: bool,
}

impl AgentMethodStreamMetadata {
    pub fn uses_streams(self) -> bool {
        self.input || self.output
    }
}

/// Per-agent-type provisioning configuration stored alongside AgentType declarations
/// in ComponentMetadata. Holds runtime setup data separate from agent type declarations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct AgentTypeProvisionConfig {
    pub initial_permissions: PolymorphicCard,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub config: Vec<TypedAgentConfigEntry>,
    #[serde(default)]
    pub plugins: Vec<InstalledPlugin>,
    #[serde(default)]
    pub files: Vec<InitialAgentFile>,
}
