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

pub use super::parsed_function_name::{
    ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, SemVer,
};
use crate::SafeDisplay;
use crate::base_model::component::InitialAgentFile;
pub use crate::base_model::component_metadata::*;
use crate::base_model::worker::TypedAgentConfigEntry;
use crate::component_introspection::AnalysisFailure;
use crate::component_introspection::metadata::Producers as IntrospectionProducers;
use crate::model::agent::{AgentTypeName, OwnerKind};
use crate::model::card::PolymorphicCard;
use crate::model::component::InstalledPlugin;
use crate::model::tool::{ToolDeploymentMetadata, ToolName};
use crate::model::tool_middleware::{ToolMiddlewareDeploymentMetadata, ToolMiddlewareName};
use crate::schema::agent::{
    AgentTypeSchema, ComponentConfigSchema, FieldSource, contains_stream_in_graph,
};
use std::collections::BTreeMap;
use std::fmt::{self, Debug, Display, Formatter};
use std::sync::Arc;

impl ComponentMetadata {
    fn derive_agent_method_streams(
        agent_types: &[AgentTypeSchema],
    ) -> BTreeMap<AgentTypeName, BTreeMap<String, AgentMethodStreamMetadata>> {
        agent_types
            .iter()
            .map(|agent_type| {
                let methods = agent_type
                    .methods
                    .iter()
                    .map(|method| {
                        let input = method
                            .input_schema
                            .fields()
                            .iter()
                            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
                            .any(|field| {
                                contains_stream_in_graph(&agent_type.schema, &field.schema)
                            });
                        let output = method.output_schema.schema().is_some_and(|schema| {
                            contains_stream_in_graph(&agent_type.schema, schema)
                        });
                        (
                            method.name.clone(),
                            AgentMethodStreamMetadata { input, output },
                        )
                    })
                    .collect();
                (agent_type.type_name.clone(), methods)
            })
            .collect()
    }

    pub fn from_parts(
        known_exports: KnownExports,
        memories: Vec<LinearMemory>,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    ) -> Self {
        Self::from_parts_with_tools(
            known_exports,
            memories,
            agent_types,
            agent_type_provision_configs,
            BTreeMap::new(),
        )
    }

    pub fn from_parts_with_tools(
        known_exports: KnownExports,
        memories: Vec<LinearMemory>,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
    ) -> Self {
        Self::from_parts_with_tools_and_middlewares(
            known_exports,
            memories,
            agent_types,
            agent_type_provision_configs,
            tools,
            BTreeMap::new(),
        )
    }

    pub fn from_parts_with_tools_and_middlewares(
        known_exports: KnownExports,
        memories: Vec<LinearMemory>,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
        tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
    ) -> Self {
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports,
                producers: vec![],
                memories,
                config_schema: ComponentConfigSchema::default(),
                component_provision_config: ComponentProvisionConfig::default(),
                agent_method_streams: Self::derive_agent_method_streams(&agent_types),
                agent_types,
                agent_type_provision_configs,
                tools,
                tool_middlewares,
            }),
        }
    }

    /// Returns a new `ComponentMetadata` with the provision configs replaced.
    /// All other analysed fields (known_exports, memories, agent types, etc.) are preserved.
    /// Use this when provision configs are updated independently of the WASM binary.
    pub fn with_provision_configs(
        &self,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    ) -> Self {
        let data = self.data.as_ref();
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports: data.known_exports.clone(),
                producers: data.producers.clone(),
                memories: data.memories.clone(),
                config_schema: data.config_schema.clone(),
                component_provision_config: data.component_provision_config.clone(),
                agent_types: data.agent_types.clone(),
                agent_method_streams: data.agent_method_streams.clone(),
                agent_type_provision_configs,
                tools: data.tools.clone(),
                tool_middlewares: data.tool_middlewares.clone(),
            }),
        }
    }

    pub fn redact_host_managed_values_for_external(&mut self) {
        let data = Arc::make_mut(&mut self.data);
        for entry in &mut data.component_provision_config.config {
            entry.value = crate::schema::redact_host_managed_typed_value(entry.value.clone());
        }
        for provision in data.agent_type_provision_configs.values_mut() {
            for entry in &mut provision.config {
                entry.value = crate::schema::redact_host_managed_typed_value(entry.value.clone());
            }
        }
    }

    /// Returns a new `ComponentMetadata` with its tools replaced.
    /// All component analysis and agent metadata is preserved.
    pub fn with_tools(&self, tools: BTreeMap<ToolName, ToolDeploymentMetadata>) -> Self {
        let data = self.data.as_ref();
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports: data.known_exports.clone(),
                producers: data.producers.clone(),
                memories: data.memories.clone(),
                config_schema: data.config_schema.clone(),
                component_provision_config: data.component_provision_config.clone(),
                agent_types: data.agent_types.clone(),
                agent_method_streams: data.agent_method_streams.clone(),
                agent_type_provision_configs: data.agent_type_provision_configs.clone(),
                tools,
                tool_middlewares: data.tool_middlewares.clone(),
            }),
        }
    }

    pub fn with_tool_middlewares(
        &self,
        tool_middlewares: BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata>,
    ) -> Self {
        let mut data = self.data.as_ref().clone();
        data.tool_middlewares = tool_middlewares;
        Self {
            data: Arc::new(data),
        }
    }

    pub fn producers(&self) -> &[Producers] {
        &self.data.producers
    }

    pub fn memories(&self) -> &[LinearMemory] {
        &self.data.memories
    }

    pub fn initial_linear_memory_bytes(&self) -> u64 {
        self.data
            .memories
            .iter()
            .map(|memory| memory.initial)
            .fold(0, u64::saturating_add)
    }

    pub fn has_shared_linear_memory(&self) -> bool {
        self.data.memories.iter().any(|memory| memory.shared)
    }

    pub fn known_exports(&self) -> &KnownExports {
        &self.data.known_exports
    }

    pub fn agent_types(&self) -> &[AgentTypeSchema] {
        &self.data.agent_types
    }

    pub fn config_schema(&self) -> &ComponentConfigSchema {
        &self.data.config_schema
    }

    pub fn component_provision_config(&self) -> &ComponentProvisionConfig {
        &self.data.component_provision_config
    }

    pub fn with_component_config(
        &self,
        config_schema: ComponentConfigSchema,
        component_provision_config: ComponentProvisionConfig,
    ) -> Self {
        let mut data = self.data.as_ref().clone();
        data.config_schema = config_schema;
        data.component_provision_config = component_provision_config;
        Self {
            data: Arc::new(data),
        }
    }

    pub fn agent_method_stream_metadata(
        &self,
        agent_type: &AgentTypeName,
        method: &str,
    ) -> Option<AgentMethodStreamMetadata> {
        self.data
            .agent_method_streams
            .get(agent_type)?
            .get(method)
            .copied()
    }

    pub fn agent_type_provision_configs(
        &self,
    ) -> &BTreeMap<AgentTypeName, AgentTypeProvisionConfig> {
        &self.data.agent_type_provision_configs
    }

    pub fn agent_type_provision_config(
        &self,
        name: &AgentTypeName,
    ) -> Option<&AgentTypeProvisionConfig> {
        self.data.agent_type_provision_configs.get(name)
    }

    pub fn tools(&self) -> &BTreeMap<ToolName, ToolDeploymentMetadata> {
        &self.data.tools
    }

    pub fn tool(&self, name: &ToolName) -> Option<&ToolDeploymentMetadata> {
        self.data.tools.get(name)
    }

    pub fn tool_middlewares(
        &self,
    ) -> &BTreeMap<ToolMiddlewareName, ToolMiddlewareDeploymentMetadata> {
        &self.data.tool_middlewares
    }

    pub fn agent_type_initial_permission_card(
        &self,
        name: &AgentTypeName,
    ) -> Option<&PolymorphicCard> {
        self.agent_type_provision_config(name)
            .map(|config| &config.initial_permissions)
    }

    pub fn agent_type_env(&self, name: &AgentTypeName) -> Option<&BTreeMap<String, String>> {
        self.agent_type_provision_config(name)
            .map(|config| &config.env)
    }

    pub fn agent_type_config(&self, name: &AgentTypeName) -> Option<&[TypedAgentConfigEntry]> {
        self.agent_type_provision_config(name)
            .map(|config| config.config.as_slice())
    }

    pub fn agent_type_files(&self, name: &AgentTypeName) -> Option<&[InitialAgentFile]> {
        self.agent_type_provision_config(name)
            .map(|config| config.files.as_slice())
    }

    pub fn agent_type_plugins(&self, name: &AgentTypeName) -> Option<&[InstalledPlugin]> {
        self.agent_type_provision_config(name)
            .map(|config| config.plugins.as_slice())
    }

    pub fn owner_plugins(
        &self,
        owner_kind: OwnerKind,
        agent_type: Option<&AgentTypeName>,
    ) -> Option<&[InstalledPlugin]> {
        match owner_kind {
            OwnerKind::ComponentAgent => agent_type.and_then(|name| self.agent_type_plugins(name)),
            OwnerKind::EphemeralExternalTool => {
                Some(self.component_provision_config().plugins.as_slice())
            }
        }
    }

    pub fn is_agent(&self) -> bool {
        !self.data.agent_types.is_empty()
    }

    pub fn has_load_snapshot(&self) -> bool {
        self.data.known_exports.load_snapshot_interface.is_some()
    }

    pub fn has_save_snapshot(&self) -> bool {
        self.data.known_exports.save_snapshot_interface.is_some()
    }

    pub fn has_agent_guest(&self) -> bool {
        self.data.known_exports.agent_guest_interface.is_some()
    }

    pub fn has_oplog_processor(&self) -> bool {
        self.data.known_exports.oplog_processor_interface.is_some()
    }

    pub fn has_tool_guest(&self) -> bool {
        self.data.known_exports.tool_guest_interface.is_some()
    }

    pub fn has_tool_middleware_guest(&self) -> bool {
        self.data
            .known_exports
            .tool_middleware_guest_interface
            .is_some()
    }

    /// Returns the fully-qualified WIT function name for `golem:api/load-snapshot.load`
    pub fn load_snapshot_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .load_snapshot_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{load}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:api/save-snapshot.save`
    pub fn save_snapshot_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .save_snapshot_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{save}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:agent/guest.initialize`
    pub fn agent_initialize_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .agent_guest_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{initialize}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:agent/guest.invoke`
    pub fn agent_invoke_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .agent_guest_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{invoke}}"))
    }

    /// Returns the fully-qualified WIT function name for `golem:api/oplog-processor.process`
    pub fn oplog_processor_function_name(&self) -> Option<String> {
        self.data
            .known_exports
            .oplog_processor_interface
            .as_ref()
            .map(|iface| format!("{iface}.{{process}}"))
    }

    pub fn find_agent_type_by_name(&self, agent_type: &AgentTypeName) -> Option<AgentTypeSchema> {
        self.find_agent_type_by_name_ref(agent_type).cloned()
    }

    /// Borrowing variant of [`find_agent_type_by_name`](Self::find_agent_type_by_name).
    ///
    /// Hot paths (invocation lowering, read-only classification) only need to
    /// read a handful of fields and must not clone the whole [`AgentTypeSchema`]
    /// (which owns the agent's full [`SchemaGraph`]) on every call.
    pub fn find_agent_type_by_name_ref(
        &self,
        agent_type: &AgentTypeName,
    ) -> Option<&AgentTypeSchema> {
        self.data
            .agent_types
            .iter()
            .find(|t| &t.type_name == agent_type)
    }
}

/// Raw component metadata as returned by component introspection.
///
/// Carries the introspection helper's `Producers` type unchanged; the
/// public [`ComponentMetadata`] then maps it to the `golem-common` wire-level
/// shape.
#[derive(Default)]
pub struct RawComponentMetadata {
    pub known_exports: KnownExports,
    pub producers: Vec<IntrospectionProducers>,
    pub memories: Vec<LinearMemory>,
}

#[derive(Debug, thiserror::Error)]
pub enum ComponentProcessingError {
    Parsing(String),
    Analysis(AnalysisFailure),
    Metadata(String),
}

impl SafeDisplay for ComponentProcessingError {
    fn to_safe_string(&self) -> String {
        match self {
            ComponentProcessingError::Parsing(_) => self.to_string(),
            ComponentProcessingError::Analysis(_) => self.to_string(),
            ComponentProcessingError::Metadata(_) => self.to_string(),
        }
    }
}

impl Display for ComponentProcessingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ComponentProcessingError::Parsing(e) => write!(f, "Parsing error: {e}"),
            ComponentProcessingError::Analysis(source) => {
                let AnalysisFailure { reason } = source;
                write!(f, "Analysis error: {reason}")
            }
            ComponentProcessingError::Metadata(e) => write!(f, "Metadata error: {e}"),
        }
    }
}

mod analysis;
mod openapi;
mod protobuf;

pub use analysis::extract_known_exports;

#[cfg(test)]
mod tests;
