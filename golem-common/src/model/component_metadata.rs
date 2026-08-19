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
use crate::component_introspection::metadata::Producers as IntrospectionProducers;
use crate::component_introspection::wit_parser::WitAnalysisContext;
use crate::component_introspection::{AnalysisFailure, AnalysisResult, TopLevelExport};
use crate::model::agent::AgentTypeName;
use crate::model::card::PolymorphicCard;
use crate::model::component::InstalledPlugin;
use crate::model::tool::{ToolDeploymentMetadata, ToolName};
use crate::schema::agent::AgentTypeSchema;
use std::collections::BTreeMap;
use std::fmt::{self, Debug, Display, Formatter};
use std::sync::Arc;

impl ComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
    ) -> Result<Self, ComponentProcessingError> {
        let raw = RawComponentMetadata::analyse_component(data)?;
        Ok(Self {
            data: Arc::new(raw.into_metadata(agent_types, agent_type_provision_configs, tools)),
        })
    }

    pub fn from_parts(
        known_exports: KnownExports,
        memories: Vec<LinearMemory>,
        root_package_name: Option<String>,
        root_package_version: Option<String>,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
    ) -> Self {
        Self::from_parts_with_tools(
            known_exports,
            memories,
            root_package_name,
            root_package_version,
            agent_types,
            agent_type_provision_configs,
            BTreeMap::new(),
        )
    }

    pub fn from_parts_with_tools(
        known_exports: KnownExports,
        memories: Vec<LinearMemory>,
        root_package_name: Option<String>,
        root_package_version: Option<String>,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
    ) -> Self {
        Self {
            data: Arc::new(ComponentMetadataInnerData {
                known_exports,
                producers: vec![],
                memories,
                root_package_name,
                root_package_version,
                agent_types,
                agent_type_provision_configs,
                tools,
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
                root_package_name: data.root_package_name.clone(),
                root_package_version: data.root_package_version.clone(),
                agent_types: data.agent_types.clone(),
                agent_type_provision_configs,
                tools: data.tools.clone(),
            }),
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
                root_package_name: data.root_package_name.clone(),
                root_package_version: data.root_package_version.clone(),
                agent_types: data.agent_types.clone(),
                agent_type_provision_configs: data.agent_type_provision_configs.clone(),
                tools,
            }),
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

    pub fn root_package_name(&self) -> &Option<String> {
        &self.data.root_package_name
    }

    pub fn root_package_version(&self) -> &Option<String> {
        &self.data.root_package_version
    }

    pub fn agent_types(&self) -> &[AgentTypeSchema] {
        &self.data.agent_types
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

impl poem_openapi::types::Type for ComponentMetadata {
    const IS_REQUIRED: bool =
        <ComponentMetadataInnerData as poem_openapi::types::Type>::IS_REQUIRED;
    type RawValueType = <ComponentMetadataInnerData as poem_openapi::types::Type>::RawValueType;
    type RawElementValueType =
        <ComponentMetadataInnerData as poem_openapi::types::Type>::RawElementValueType;

    fn name() -> std::borrow::Cow<'static, str> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::name()
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::schema_ref()
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::register(registry);
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::as_raw_value(&self.data)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::raw_element_iter(&self.data)
    }
}

impl poem_openapi::types::IsObjectType for ComponentMetadata {}

impl poem_openapi::types::ParseFromJSON for ComponentMetadata {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_json(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToJSON for ComponentMetadata {
    fn to_json(&self) -> Option<serde_json::Value> {
        self.data.to_json()
    }
}

impl poem_openapi::types::ParseFromXML for ComponentMetadata {
    fn parse_from_xml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_xml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToXML for ComponentMetadata {
    fn to_xml(&self) -> Option<serde_json::Value> {
        self.data.to_xml()
    }
}

impl poem_openapi::types::ParseFromYAML for ComponentMetadata {
    fn parse_from_yaml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_yaml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToYAML for ComponentMetadata {
    fn to_yaml(&self) -> Option<serde_json::Value> {
        self.data.to_yaml()
    }
}

impl From<wasmparser::MemoryType> for LinearMemory {
    fn from(value: wasmparser::MemoryType) -> Self {
        Self {
            initial: value.initial * LinearMemory::PAGE_SIZE,
            maximum: value.maximum.map(|m| m * LinearMemory::PAGE_SIZE),
            shared: value.shared,
        }
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
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

/// Interface name prefixes used to identify known Golem capabilities.
const AGENT_GUEST_PREFIX: &str = "golem:agent/guest";
const SAVE_SNAPSHOT_PREFIX: &str = "golem:api/save-snapshot";
const LOAD_SNAPSHOT_PREFIX: &str = "golem:api/load-snapshot";
const OPLOG_PROCESSOR_PREFIX: &str = "golem:api/oplog-processor";
const TOOL_GUEST_PREFIX: &str = "golem:tool/guest";

fn record_known_export(
    slot: &mut Option<String>,
    capability_name: &str,
    export_name: &str,
) -> AnalysisResult<()> {
    match slot {
        Some(previous) => Err(AnalysisFailure::failed(format!(
            "Duplicate {capability_name} export: found both {previous} and {export_name}"
        ))),
        None => {
            *slot = Some(export_name.to_string());
            Ok(())
        }
    }
}

/// Extract a `KnownExports` index from the top-level exports.
/// Only instance exports are considered; each supported interface prefix
/// is matched at most once (the exact versioned name is stored).
pub fn extract_known_exports(exports: &[TopLevelExport]) -> AnalysisResult<KnownExports> {
    let mut known = KnownExports::default();

    for export in exports {
        if let TopLevelExport::Instance(instance) = export {
            let name = &instance.name;
            if name == AGENT_GUEST_PREFIX || name.starts_with(&format!("{AGENT_GUEST_PREFIX}@")) {
                record_known_export(&mut known.agent_guest_interface, "agent guest", name)?;
            } else if name == SAVE_SNAPSHOT_PREFIX
                || name.starts_with(&format!("{SAVE_SNAPSHOT_PREFIX}@"))
            {
                record_known_export(&mut known.save_snapshot_interface, "save snapshot", name)?;
            } else if name == LOAD_SNAPSHOT_PREFIX
                || name.starts_with(&format!("{LOAD_SNAPSHOT_PREFIX}@"))
            {
                record_known_export(&mut known.load_snapshot_interface, "load snapshot", name)?;
            } else if name == OPLOG_PROCESSOR_PREFIX
                || name.starts_with(&format!("{OPLOG_PROCESSOR_PREFIX}@"))
            {
                record_known_export(
                    &mut known.oplog_processor_interface,
                    "oplog processor",
                    name,
                )?;
            } else if name == TOOL_GUEST_PREFIX
                || name.starts_with(&format!("{TOOL_GUEST_PREFIX}@"))
            {
                record_known_export(&mut known.tool_guest_interface, "tool guest", name)?;
            }
        }
    }

    Ok(known)
}

impl RawComponentMetadata {
    pub fn analyse_component(
        data: &[u8],
    ) -> Result<RawComponentMetadata, ComponentProcessingError> {
        let wit_analysis =
            WitAnalysisContext::new(data).map_err(ComponentProcessingError::Analysis)?;

        let exports = wit_analysis
            .get_top_level_exports()
            .map_err(ComponentProcessingError::Analysis)?;
        let root_package = wit_analysis.root_package_name();

        for warning in wit_analysis.warnings() {
            tracing::warn!("Wit analysis warning: {}", warning);
        }

        let known_exports =
            extract_known_exports(&exports).map_err(ComponentProcessingError::Analysis)?;

        let memories = wit_analysis
            .linear_memories()
            .iter()
            .cloned()
            .map(LinearMemory::from)
            .collect();

        let producers = wit_analysis.producers().to_vec();

        Ok(RawComponentMetadata {
            known_exports,
            producers,
            memories,
            root_package_name: root_package
                .as_ref()
                .map(|pkg| format!("{}:{}", pkg.namespace, pkg.name)),
            root_package_version: root_package.and_then(|pkg| pkg.version.map(|v| v.to_string())),
        })
    }

    pub fn into_metadata(
        self,
        agent_types: Vec<AgentTypeSchema>,
        agent_type_provision_configs: BTreeMap<AgentTypeName, AgentTypeProvisionConfig>,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
    ) -> ComponentMetadataInnerData {
        let producers = self
            .producers
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let memories = self.memories.into_iter().collect();

        ComponentMetadataInnerData {
            known_exports: self.known_exports,
            producers,
            memories,
            root_package_name: self.root_package_name,
            root_package_version: self.root_package_version,
            agent_types,
            agent_type_provision_configs,
            tools,
        }
    }
}

impl From<crate::component_introspection::metadata::Producers> for Producers {
    fn from(value: crate::component_introspection::metadata::Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<Producers> for crate::component_introspection::metadata::Producers {
    fn from(value: Producers) -> Self {
        Self {
            fields: value
                .fields
                .into_iter()
                .map(|p| p.into())
                .collect::<Vec<_>>(),
        }
    }
}

impl From<crate::component_introspection::metadata::ProducersField> for ProducerField {
    fn from(value: crate::component_introspection::metadata::ProducersField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(|value| VersionedName {
                    name: value.name,
                    version: value.version,
                })
                .collect(),
        }
    }
}

impl From<ProducerField> for crate::component_introspection::metadata::ProducersField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value
                .values
                .into_iter()
                .map(
                    |value| crate::component_introspection::metadata::VersionedName {
                        name: value.name,
                        version: value.version,
                    },
                )
                .collect(),
        }
    }
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

mod protobuf {
    use crate::base_model::component_metadata::{AgentTypeProvisionConfig, KnownExports};
    use crate::base_model::json::NormalizedJsonValue;
    use crate::model::account::{AccountEmail, AccountId};
    use crate::model::agent::AgentTypeName;
    use crate::model::agent_secret::CanonicalAgentSecretPath;
    use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
    use crate::model::component_metadata::{
        ComponentMetadata, ComponentMetadataInnerData, LinearMemory, ProducerField, Producers,
        VersionedName,
    };
    use crate::model::deployment::DeploymentRevision;
    use crate::model::tool::{
        CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolBindingInput,
        ToolDeploymentMetadata, ToolDeploymentState, ToolName, ToolProvisionConfig, ToolSource,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    impl From<golem_api_grpc::proto::golem::component::VersionedName> for VersionedName {
        fn from(value: golem_api_grpc::proto::golem::component::VersionedName) -> Self {
            Self {
                name: value.name,
                version: value.version,
            }
        }
    }

    impl From<VersionedName> for golem_api_grpc::proto::golem::component::VersionedName {
        fn from(value: VersionedName) -> Self {
            Self {
                name: value.name,
                version: value.version,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::ProducerField> for ProducerField {
        fn from(value: golem_api_grpc::proto::golem::component::ProducerField) -> Self {
            Self {
                name: value.name,
                values: value.values.into_iter().map(|value| value.into()).collect(),
            }
        }
    }

    impl From<ProducerField> for golem_api_grpc::proto::golem::component::ProducerField {
        fn from(value: ProducerField) -> Self {
            Self {
                name: value.name,
                values: value.values.into_iter().map(|value| value.into()).collect(),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::Producers> for Producers {
        fn from(value: golem_api_grpc::proto::golem::component::Producers) -> Self {
            Self {
                fields: value.fields.into_iter().map(|field| field.into()).collect(),
            }
        }
    }

    impl From<Producers> for golem_api_grpc::proto::golem::component::Producers {
        fn from(value: Producers) -> Self {
            Self {
                fields: value.fields.into_iter().map(|field| field.into()).collect(),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::component::LinearMemory> for LinearMemory {
        fn from(value: golem_api_grpc::proto::golem::component::LinearMemory) -> Self {
            Self {
                initial: value.initial,
                maximum: value.maximum,
                shared: value.shared,
            }
        }
    }

    impl From<LinearMemory> for golem_api_grpc::proto::golem::component::LinearMemory {
        fn from(value: LinearMemory) -> Self {
            Self {
                initial: value.initial,
                maximum: value.maximum,
                shared: value.shared,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata> for ComponentMetadata {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ComponentMetadata,
        ) -> Result<Self, Self::Error> {
            let inner_data = ComponentMetadataInnerData::try_from(value)?;
            Ok(Self {
                data: Arc::new(inner_data),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ComponentMetadata>
        for ComponentMetadataInnerData
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ComponentMetadata,
        ) -> Result<Self, Self::Error> {
            let known_exports = value
                .known_exports
                .map(KnownExports::from)
                .unwrap_or_default();
            Ok(Self {
                known_exports,
                producers: value
                    .producers
                    .into_iter()
                    .map(|producer| producer.into())
                    .collect(),
                memories: value
                    .memories
                    .into_iter()
                    .map(|memory| memory.into())
                    .collect(),
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value
                    .agent_types
                    .into_iter()
                    .map(|at| at.try_into())
                    .collect::<Result<_, _>>()?,
                agent_type_provision_configs: value
                    .agent_type_provision_configs
                    .into_iter()
                    .map(|(k, v)| {
                        AgentTypeProvisionConfig::try_from(v)
                            .map(|config| (AgentTypeName(k), config))
                    })
                    .collect::<Result<_, _>>()?,
                tools: value
                    .tools
                    .into_iter()
                    .map(|(name, metadata)| Ok((ToolName::try_from(name)?, metadata.try_into()?)))
                    .collect::<Result<_, String>>()?,
            })
        }
    }

    impl From<golem_api_grpc::proto::golem::component::KnownExports> for KnownExports {
        fn from(value: golem_api_grpc::proto::golem::component::KnownExports) -> Self {
            Self {
                agent_guest_interface: value.agent_guest_interface,
                save_snapshot_interface: value.save_snapshot_interface,
                load_snapshot_interface: value.load_snapshot_interface,
                oplog_processor_interface: value.oplog_processor_interface,
                tool_guest_interface: value.tool_guest_interface,
            }
        }
    }

    impl From<KnownExports> for golem_api_grpc::proto::golem::component::KnownExports {
        fn from(value: KnownExports) -> Self {
            Self {
                agent_guest_interface: value.agent_guest_interface,
                save_snapshot_interface: value.save_snapshot_interface,
                load_snapshot_interface: value.load_snapshot_interface,
                oplog_processor_interface: value.oplog_processor_interface,
                tool_guest_interface: value.tool_guest_interface,
            }
        }
    }

    impl TryFrom<ComponentMetadata> for golem_api_grpc::proto::golem::component::ComponentMetadata {
        type Error = String;

        fn try_from(value: ComponentMetadata) -> Result<Self, Self::Error> {
            value.data.as_ref().clone().try_into()
        }
    }

    impl TryFrom<ComponentMetadataInnerData>
        for golem_api_grpc::proto::golem::component::ComponentMetadata
    {
        type Error = String;

        fn try_from(value: ComponentMetadataInnerData) -> Result<Self, Self::Error> {
            Ok(Self {
                known_exports: Some(value.known_exports.into()),
                producers: value
                    .producers
                    .into_iter()
                    .map(|producer| producer.into())
                    .collect(),
                memories: value
                    .memories
                    .into_iter()
                    .map(|memory| memory.into())
                    .collect(),
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
                agent_types: value.agent_types.into_iter().map(|at| at.into()).collect(),
                agent_type_provision_configs: value
                    .agent_type_provision_configs
                    .into_iter()
                    .map(|(k, v)| v.try_into().map(|config| (k.0, config)))
                    .collect::<Result<_, _>>()?,
                tools: value
                    .tools
                    .into_iter()
                    .map(|(name, metadata)| (name.into_inner(), metadata.into()))
                    .collect(),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ToolDeploymentMetadata>
        for ToolDeploymentMetadata
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ToolDeploymentMetadata,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                definition: value
                    .definition
                    .ok_or_else(|| "Missing ToolDeploymentMetadata.definition".to_string())?
                    .try_into()?,
                provision: value
                    .provision
                    .ok_or_else(|| "Missing ToolDeploymentMetadata.provision".to_string())?
                    .try_into()?,
                environment_binding: value
                    .environment_binding
                    .map(TryInto::try_into)
                    .transpose()?,
                agent_bindings: value
                    .agent_bindings
                    .into_iter()
                    .map(|(name, binding)| {
                        ToolBindingInput::try_from(binding)
                            .map(|binding| (AgentTypeName(name), binding))
                    })
                    .collect::<Result<_, _>>()?,
            })
        }
    }

    impl From<ToolDeploymentMetadata>
        for golem_api_grpc::proto::golem::component::ToolDeploymentMetadata
    {
        fn from(value: ToolDeploymentMetadata) -> Self {
            Self {
                definition: Some(value.definition.into()),
                provision: Some(value.provision.into()),
                environment_binding: value.environment_binding.map(Into::into),
                agent_bindings: value
                    .agent_bindings
                    .into_iter()
                    .map(|(name, binding)| (name.0, binding.into()))
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ToolProvisionConfig> for ToolProvisionConfig {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ToolProvisionConfig,
        ) -> Result<Self, Self::Error> {
            let config = serde_json::from_str(&value.config_json)
                .map(NormalizedJsonValue::new)
                .map_err(|error| format!("Invalid ToolProvisionConfig.config_json: {error}"))?;
            Ok(Self {
                config,
                env: value.env.into_iter().collect(),
                plugins: value
                    .plugins
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                files: value
                    .files
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            })
        }
    }

    impl From<ToolProvisionConfig> for golem_api_grpc::proto::golem::component::ToolProvisionConfig {
        fn from(value: ToolProvisionConfig) -> Self {
            Self {
                config_json: value.config.to_string(),
                env: value.env.into_iter().collect(),
                plugins: value.plugins.into_iter().map(Into::into).collect(),
                files: value.files.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ToolBindingInput> for ToolBindingInput {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ToolBindingInput,
        ) -> Result<Self, Self::Error> {
            let parameters = serde_json::from_str(&value.parameters_json)
                .map(NormalizedJsonValue::new)
                .map_err(|error| format!("Invalid ToolBindingInput.parameters_json: {error}"))?;
            Ok(Self {
                version: value.version,
                parameters,
                account: value.account.map(AccountEmail::new),
                secret_keys_readable: value
                    .secret_keys_readable
                    .ok_or_else(|| "Missing ToolBindingInput.secret_keys_readable".to_string())?
                    .try_into()?,
                secret_keys_revealable: value
                    .secret_keys_revealable
                    .ok_or_else(|| "Missing ToolBindingInput.secret_keys_revealable".to_string())?
                    .try_into()?,
            })
        }
    }

    impl From<ToolBindingInput> for golem_api_grpc::proto::golem::component::ToolBindingInput {
        fn from(value: ToolBindingInput) -> Self {
            Self {
                version: value.version,
                parameters_json: value.parameters.to_string(),
                account: value.account.map(AccountEmail::into_inner),
                secret_keys_readable: Some(value.secret_keys_readable.into()),
                secret_keys_revealable: Some(value.secret_keys_revealable.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::SecretKeyScope> for SecretKeyScope {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::SecretKeyScope,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::component::secret_key_scope::Value;
            match value
                .value
                .ok_or_else(|| "Missing SecretKeyScope.value".to_string())?
            {
                Value::All(_) => Ok(Self::All),
                Value::Keys(keys) => Ok(Self::Keys(
                    keys.paths
                        .into_iter()
                        .map(|path| CanonicalAgentSecretPath(path.segments))
                        .collect::<BTreeSet<_>>(),
                )),
            }
        }
    }

    impl From<SecretKeyScope> for golem_api_grpc::proto::golem::component::SecretKeyScope {
        fn from(value: SecretKeyScope) -> Self {
            use golem_api_grpc::proto::golem::component::secret_key_scope::Value;
            let value = match value {
                SecretKeyScope::All => Value::All(golem_api_grpc::proto::golem::common::Empty {}),
                SecretKeyScope::Keys(keys) => {
                    Value::Keys(golem_api_grpc::proto::golem::component::SecretKeyPaths {
                        paths: keys
                            .into_iter()
                            .map(
                                |path| golem_api_grpc::proto::golem::component::SecretKeyPath {
                                    segments: path.0,
                                },
                            )
                            .collect(),
                    })
                }
            };
            Self { value: Some(value) }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig>
        for AgentTypeProvisionConfig
    {
        type Error = String;

        fn try_from(
            proto: golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig,
        ) -> Result<Self, Self::Error> {
            use crate::base_model::component::{InitialAgentFile, InstalledPlugin};
            use crate::base_model::worker::TypedAgentConfigEntry;

            let config = proto
                .config
                .into_iter()
                .map(TypedAgentConfigEntry::try_from)
                .collect::<Result<Vec<_>, _>>()?;

            let plugins = proto
                .plugins
                .into_iter()
                .map(InstalledPlugin::try_from)
                .collect::<Result<Vec<_>, _>>()?;

            let files = proto
                .files
                .into_iter()
                .map(InitialAgentFile::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            let initial_permission = crate::serialization::deserialize(&proto.initial_permissions)?;

            Ok(AgentTypeProvisionConfig {
                initial_permissions: initial_permission,
                env: proto.env.into_iter().collect(),
                config,
                plugins,
                files,
            })
        }
    }

    impl TryFrom<AgentTypeProvisionConfig>
        for golem_api_grpc::proto::golem::component::AgentTypeProvisionConfig
    {
        type Error = String;

        fn try_from(config: AgentTypeProvisionConfig) -> Result<Self, Self::Error> {
            use crate::base_model::component::{InitialAgentFile, InstalledPlugin};

            Ok(Self {
                initial_permissions: crate::serialization::serialize(&config.initial_permissions)
                    .expect("failed to serialize agent initial permission card"),
                env: config.env.into_iter().collect(),
                config: config
                    .config
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                plugins: config
                    .plugins
                    .into_iter()
                    .map(|p: InstalledPlugin| {
                        golem_api_grpc::proto::golem::component::PluginInstallation::from(p)
                    })
                    .collect(),
                files: config
                    .files
                    .into_iter()
                    .map(|f: InitialAgentFile| {
                        golem_api_grpc::proto::golem::component::InitialAgentFile::from(f)
                    })
                    .collect(),
            })
        }
    }

    impl From<ToolSource> for golem_api_grpc::proto::golem::registry::ComponentToolSource {
        fn from(value: ToolSource) -> Self {
            let ToolSource::Component {
                component_id,
                component_revision,
                component_name,
            } = value;
            Self {
                component_id: Some(component_id.into()),
                component_revision: component_revision.into(),
                component_name: component_name.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::ComponentToolSource> for ToolSource {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::ComponentToolSource,
        ) -> Result<Self, Self::Error> {
            Ok(Self::Component {
                component_id: ComponentId::try_from(
                    value
                        .component_id
                        .ok_or("missing ComponentToolSource.component_id")?,
                )?,
                component_revision: ComponentRevision::try_from(value.component_revision)?,
                component_name: ComponentName(value.component_name),
            })
        }
    }

    impl From<RegisteredTool> for golem_api_grpc::proto::golem::registry::RegisteredTool {
        fn from(value: RegisteredTool) -> Self {
            Self {
                deployment_revision: value.deployment_revision.into(),
                definition: Some(value.definition.into()),
                provision: Some(value.provision.into()),
                source: Some(value.source.into()),
                owner_account_id: Some(value.owner_account_id.into()),
                owner_account_email: value.owner_account_email.into_inner(),
                metadata_version: value.metadata_version,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredTool> for RegisteredTool {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::RegisteredTool,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                deployment_revision: DeploymentRevision::try_from(value.deployment_revision)?,
                definition: value
                    .definition
                    .ok_or("missing RegisteredTool.definition")?
                    .try_into()?,
                provision: value
                    .provision
                    .ok_or("missing RegisteredTool.provision")?
                    .try_into()?,
                source: value
                    .source
                    .ok_or("missing RegisteredTool.source")?
                    .try_into()?,
                owner_account_id: AccountId::try_from(
                    value
                        .owner_account_id
                        .ok_or("missing RegisteredTool.owner_account_id")?,
                )?,
                owner_account_email: AccountEmail::new(value.owner_account_email),
                metadata_version: value.metadata_version,
            })
        }
    }

    impl From<CompiledToolBinding> for golem_api_grpc::proto::golem::registry::CompiledToolBinding {
        fn from(value: CompiledToolBinding) -> Self {
            Self {
                deployment_revision: value.deployment_revision.into(),
                agent_type_name: value.agent_type_name.0,
                tool_name: value.tool_name.into_inner(),
                version: value.version,
                metadata_version: value.metadata_version,
                account_id: Some(value.account_id.into()),
                account_email: value.account_email.into_inner(),
                parameters_json: value.parameters.to_string(),
                secret_keys_readable: Some(value.secret_keys_readable.into()),
                secret_keys_revealable: Some(value.secret_keys_revealable.into()),
                source: Some(value.source.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::CompiledToolBinding> for CompiledToolBinding {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::CompiledToolBinding,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                deployment_revision: DeploymentRevision::try_from(value.deployment_revision)?,
                agent_type_name: AgentTypeName(value.agent_type_name),
                tool_name: ToolName::try_from(value.tool_name)?,
                version: value.version,
                metadata_version: value.metadata_version,
                account_id: AccountId::try_from(
                    value
                        .account_id
                        .ok_or("missing CompiledToolBinding.account_id")?,
                )?,
                account_email: AccountEmail::new(value.account_email),
                parameters: NormalizedJsonValue::new(
                    serde_json::from_str(&value.parameters_json)
                        .map_err(|error| format!("invalid tool binding parameters: {error}"))?,
                ),
                secret_keys_readable: value
                    .secret_keys_readable
                    .ok_or("missing CompiledToolBinding.secret_keys_readable")?
                    .try_into()?,
                secret_keys_revealable: value
                    .secret_keys_revealable
                    .ok_or("missing CompiledToolBinding.secret_keys_revealable")?
                    .try_into()?,
                source: value
                    .source
                    .ok_or("missing CompiledToolBinding.source")?
                    .try_into()?,
            })
        }
    }

    impl From<ToolDeploymentState> for golem_api_grpc::proto::golem::registry::ToolDeploymentState {
        fn from(value: ToolDeploymentState) -> Self {
            Self {
                deployment_revision: value.deployment_revision.into(),
                registered_tools: value
                    .registered_tools
                    .into_values()
                    .map(Into::into)
                    .collect(),
                agent_tool_bindings: value
                    .agent_tool_bindings
                    .into_values()
                    .flat_map(BTreeMap::into_values)
                    .map(Into::into)
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::registry::ToolDeploymentState> for ToolDeploymentState {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::registry::ToolDeploymentState,
        ) -> Result<Self, Self::Error> {
            let deployment_revision = DeploymentRevision::try_from(value.deployment_revision)?;
            let mut registered_tools = BTreeMap::new();
            for proto in value.registered_tools {
                let registered: RegisteredTool = proto.try_into()?;
                if registered.deployment_revision != deployment_revision {
                    return Err(format!(
                        "registered tool deployment revision {} does not match snapshot revision {}",
                        registered.deployment_revision, deployment_revision
                    ));
                }
                let name = ToolName::try_from(
                    registered
                        .definition
                        .name()
                        .ok_or("registered tool definition has no name")?,
                )?;
                if registered_tools.insert(name.clone(), registered).is_some() {
                    return Err(format!("duplicate registered tool {name}"));
                }
            }
            let mut agent_tool_bindings = BTreeMap::new();
            for proto in value.agent_tool_bindings {
                let binding: CompiledToolBinding = proto.try_into()?;
                if binding.deployment_revision != deployment_revision {
                    return Err(format!(
                        "compiled tool binding deployment revision {} does not match snapshot revision {}",
                        binding.deployment_revision, deployment_revision
                    ));
                }
                let registered = registered_tools.get(&binding.tool_name).ok_or_else(|| {
                    format!(
                        "compiled binding references unregistered tool {}",
                        binding.tool_name
                    )
                })?;
                if binding.source != registered.source
                    || binding.version != registered.definition.version
                    || binding.metadata_version != registered.metadata_version
                    || binding.account_id != registered.owner_account_id
                    || binding.account_email != registered.owner_account_email
                {
                    return Err(format!(
                        "compiled binding for tool {} does not match the registered implementation",
                        binding.tool_name
                    ));
                }
                if !binding
                    .secret_keys_revealable
                    .is_subset_of(&binding.secret_keys_readable)
                {
                    return Err(format!(
                        "compiled binding for tool {} has revealable secret keys outside its readable scope",
                        binding.tool_name
                    ));
                }
                if agent_tool_bindings
                    .entry(binding.agent_type_name.clone())
                    .or_insert_with(BTreeMap::new)
                    .insert(binding.tool_name.clone(), binding)
                    .is_some()
                {
                    return Err("duplicate compiled agent tool binding".to_string());
                }
            }
            Ok(Self {
                deployment_revision,
                registered_tools,
                agent_tool_bindings,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_introspection::{ExportedInstance, TopLevelExport};
    use crate::model::account::{AccountEmail, AccountId};
    use crate::model::agent::AgentTypeName;
    use crate::model::agent_secret::CanonicalAgentSecretPath;
    use crate::model::card::CardId;
    use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
    use crate::model::deployment::DeploymentRevision;
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::{
        CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolBindingInput,
        ToolDeploymentMetadata, ToolDeploymentState, ToolName, ToolProvisionConfig, ToolSource,
    };
    use crate::schema::SchemaGraph;
    use crate::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use std::collections::BTreeSet;
    use test_r::test;

    fn instance_export(name: &str) -> TopLevelExport {
        TopLevelExport::Instance(ExportedInstance {
            name: name.to_string(),
            functions: vec![],
        })
    }

    #[test]
    fn extract_known_exports_collects_supported_interfaces() {
        let exports = vec![
            instance_export("example:ignored/foo@1.0.0"),
            instance_export("golem:agent/guest@1.5.0"),
            instance_export("golem:api/save-snapshot@1.5.0"),
            instance_export("golem:api/load-snapshot@1.5.0"),
            instance_export("golem:api/oplog-processor@1.5.0"),
            instance_export("golem:tool/guest@0.1.0"),
        ];

        let known = extract_known_exports(&exports).unwrap();

        assert_eq!(
            known,
            KnownExports {
                agent_guest_interface: Some("golem:agent/guest@1.5.0".to_string()),
                save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
                load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".to_string()),
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
                tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
            }
        );
    }

    #[test]
    fn extract_known_exports_rejects_duplicate_capabilities() {
        let exports = vec![
            instance_export("golem:api/save-snapshot@1.5.0"),
            instance_export("golem:api/save-snapshot@1.6.0"),
        ];

        let error = extract_known_exports(&exports).unwrap_err();

        assert!(error.reason.contains("Duplicate save snapshot export"));
        assert!(error.reason.contains("golem:api/save-snapshot@1.5.0"));
        assert!(error.reason.contains("golem:api/save-snapshot@1.6.0"));
    }

    #[test]
    fn component_metadata_helpers_use_exact_known_export_names() {
        let metadata = ComponentMetadata::from_parts(
            KnownExports {
                agent_guest_interface: Some("golem:agent/guest@1.5.0".to_string()),
                save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
                load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".to_string()),
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
                tool_guest_interface: None,
            },
            vec![],
            None,
            None,
            vec![],
            BTreeMap::new(),
        );

        assert_eq!(
            metadata.agent_initialize_function_name(),
            Some("golem:agent/guest@1.5.0.{initialize}".to_string())
        );
        assert_eq!(
            metadata.agent_invoke_function_name(),
            Some("golem:agent/guest@1.5.0.{invoke}".to_string())
        );
        assert_eq!(
            metadata.save_snapshot_function_name(),
            Some("golem:api/save-snapshot@1.5.0.{save}".to_string())
        );
        assert_eq!(
            metadata.load_snapshot_function_name(),
            Some("golem:api/load-snapshot@1.5.0.{load}".to_string())
        );
        assert_eq!(
            metadata.oplog_processor_function_name(),
            Some("golem:api/oplog-processor@1.5.0.{process}".to_string())
        );
    }

    #[test]
    fn component_metadata_grpc_roundtrip_preserves_agent_initial_permissions() {
        let agent_type = AgentTypeName("Cart".to_string());
        let card_id = CardId::new();
        let card = crate::model::card::PolymorphicCard {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: crate::model::card::default_agent_initial_permission_grants(
                crate::model::card::recipient::RecipientPattern::Any,
            ),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: false,
        };

        let metadata = ComponentMetadata::from_parts(
            KnownExports::default(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            BTreeMap::from([(
                agent_type.clone(),
                AgentTypeProvisionConfig {
                    initial_permissions: card.clone(),
                    env: BTreeMap::new(),
                    config: Vec::new(),
                    plugins: Vec::new(),
                    files: Vec::new(),
                },
            )]),
        );

        let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
            metadata.try_into().unwrap();
        let decoded = ComponentMetadata::try_from(proto).unwrap();

        assert_eq!(
            decoded.agent_type_initial_permission_card(&agent_type),
            Some(&card)
        );
    }

    #[test]
    fn component_metadata_grpc_roundtrip_preserves_shared_memory_flag() {
        let metadata = ComponentMetadata::from_parts(
            KnownExports::default(),
            vec![LinearMemory {
                initial: LinearMemory::PAGE_SIZE,
                maximum: Some(2 * LinearMemory::PAGE_SIZE),
                shared: true,
            }],
            None,
            None,
            Vec::new(),
            BTreeMap::new(),
        );

        let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
            metadata.try_into().unwrap();
        let decoded = ComponentMetadata::try_from(proto).unwrap();

        assert!(decoded.memories()[0].shared);
    }

    fn sample_tool() -> Tool {
        Tool {
            version: "1.2.3".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "grep".to_string(),
                    aliases: vec!["search".to_string()],
                    doc: Doc {
                        summary: "Search files".to_string(),
                        description: "Searches files recursively".to_string(),
                        examples: Vec::new(),
                    },
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        }
    }

    fn metadata_with_tool() -> ComponentMetadata {
        let tool_name = ToolName::try_from("grep").unwrap();
        let binding = ToolBindingInput {
            version: Some("1.2.3".to_string()),
            parameters: NormalizedJsonValue::new(serde_json::json!({ "root": "/workspace" })),
            account: Some(crate::model::account::AccountEmail::new(
                "owner@example.com",
            )),
            secret_keys_readable: SecretKeyScope::Keys(BTreeSet::from([CanonicalAgentSecretPath(
                vec!["credentials".to_string(), "github".to_string()],
            )])),
            secret_keys_revealable: SecretKeyScope::Keys(BTreeSet::new()),
        };

        ComponentMetadata::from_parts_with_tools(
            KnownExports {
                tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
                ..KnownExports::default()
            },
            Vec::new(),
            None,
            None,
            Vec::new(),
            BTreeMap::new(),
            BTreeMap::from([(
                tool_name,
                ToolDeploymentMetadata {
                    definition: sample_tool(),
                    provision: ToolProvisionConfig {
                        config: NormalizedJsonValue::new(serde_json::json!({
                            "logLevel": "debug"
                        })),
                        env: BTreeMap::from([("RUST_LOG".to_string(), "debug".to_string())]),
                        plugins: Vec::new(),
                        files: Vec::new(),
                    },
                    environment_binding: Some(binding.clone()),
                    agent_bindings: BTreeMap::from([(
                        AgentTypeName("CoderAgent".to_string()),
                        binding,
                    )]),
                },
            )]),
        )
    }

    #[test]
    fn component_metadata_grpc_roundtrip_preserves_tool_envelope() {
        let metadata = metadata_with_tool();
        let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
            metadata.clone().into();
        let decoded = ComponentMetadata::try_from(proto).unwrap();

        assert_eq!(decoded, metadata);
    }

    #[test]
    fn component_metadata_serializes_tools_under_tools_field() {
        let json = serde_json::to_value(metadata_with_tool()).unwrap();

        assert!(json.get("tools").is_some());
        assert!(json.get("toolDeploymentMetadata").is_none());
    }

    #[test]
    fn component_metadata_binary_roundtrip_preserves_tool_envelope() {
        let metadata = metadata_with_tool();
        let bytes = crate::serialization::serialize(&metadata).unwrap();
        let decoded: ComponentMetadata = crate::serialization::deserialize(&bytes).unwrap();

        assert_eq!(decoded, metadata);
    }

    #[test]
    fn tool_deployment_state_proto_rejects_registered_tool_from_another_revision() {
        let registered_tool = RegisteredTool {
            deployment_revision: DeploymentRevision::try_from(1_u64).unwrap(),
            definition: sample_tool(),
            provision: ToolProvisionConfig::default(),
            source: ToolSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::try_from(1_u64).unwrap(),
                component_name: ComponentName("tools:grep".to_string()),
            },
            owner_account_id: AccountId(uuid::Uuid::new_v4()),
            owner_account_email: AccountEmail::new("owner@example.com"),
            metadata_version: "0.1.0".to_string(),
        };
        let proto = golem_api_grpc::proto::golem::registry::ToolDeploymentState {
            deployment_revision: 2,
            registered_tools: vec![registered_tool.into()],
            agent_tool_bindings: Vec::new(),
        };

        let decoded = ToolDeploymentState::try_from(proto);

        assert!(
            decoded.is_err(),
            "a coherent deployment snapshot must reject entries from another revision"
        );
    }

    #[test]
    fn tool_deployment_state_proto_rejects_revealable_secrets_outside_readable_scope() {
        let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
        let tool_name = ToolName::try_from("grep").unwrap();
        let agent_type_name = AgentTypeName("Agent".to_string());
        let source = ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::try_from(1_u64).unwrap(),
            component_name: ComponentName("tools:grep".to_string()),
        };
        let owner_account_id = AccountId(uuid::Uuid::new_v4());
        let owner_account_email = AccountEmail::new("owner@example.com");
        let registered = RegisteredTool {
            deployment_revision,
            definition: sample_tool(),
            provision: ToolProvisionConfig::default(),
            source: source.clone(),
            owner_account_id,
            owner_account_email: owner_account_email.clone(),
            metadata_version: "0.1.0".to_string(),
        };
        let binding = CompiledToolBinding {
            deployment_revision,
            agent_type_name: agent_type_name.clone(),
            tool_name: tool_name.clone(),
            version: registered.definition.version.clone(),
            metadata_version: registered.metadata_version.clone(),
            account_id: owner_account_id,
            account_email: owner_account_email,
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            secret_keys_readable: SecretKeyScope::Keys(BTreeSet::new()),
            secret_keys_revealable: SecretKeyScope::All,
            source,
        };
        let state = ToolDeploymentState {
            deployment_revision,
            registered_tools: BTreeMap::from([(tool_name.clone(), registered)]),
            agent_tool_bindings: BTreeMap::from([(
                agent_type_name,
                BTreeMap::from([(tool_name, binding)]),
            )]),
        };
        let proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState = state.into();

        let decoded = ToolDeploymentState::try_from(proto);

        assert!(
            decoded.is_err(),
            "compiled revealable secret keys must remain a subset of readable keys"
        );
    }

    #[test]
    fn tool_deployment_state_proto_roundtrip_preserves_coherent_snapshot() {
        let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
        let tool_name = ToolName::try_from("grep").unwrap();
        let agent_type_name = AgentTypeName("Agent".to_string());
        let source = ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::try_from(1_u64).unwrap(),
            component_name: ComponentName("tools:grep".to_string()),
        };
        let owner_account_id = AccountId(uuid::Uuid::new_v4());
        let owner_account_email = AccountEmail::new("owner@example.com");
        let registered = RegisteredTool {
            deployment_revision,
            definition: sample_tool(),
            provision: ToolProvisionConfig::default(),
            source: source.clone(),
            owner_account_id,
            owner_account_email: owner_account_email.clone(),
            metadata_version: "0.1.0".to_string(),
        };
        let binding = CompiledToolBinding {
            deployment_revision,
            agent_type_name: agent_type_name.clone(),
            tool_name: tool_name.clone(),
            version: registered.definition.version.clone(),
            metadata_version: registered.metadata_version.clone(),
            account_id: owner_account_id,
            account_email: owner_account_email,
            parameters: NormalizedJsonValue::new(serde_json::json!({ "root": "/workspace" })),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            source,
        };
        let state = ToolDeploymentState {
            deployment_revision,
            registered_tools: BTreeMap::from([(tool_name.clone(), registered)]),
            agent_tool_bindings: BTreeMap::from([(
                agent_type_name,
                BTreeMap::from([(tool_name, binding)]),
            )]),
        };

        let mut mismatched_binding_proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState =
            state.clone().into();
        mismatched_binding_proto.agent_tool_bindings[0].deployment_revision = 2;
        assert!(
            ToolDeploymentState::try_from(mismatched_binding_proto).is_err(),
            "a coherent deployment snapshot must reject bindings from another revision"
        );

        let proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState =
            state.clone().into();
        let decoded = ToolDeploymentState::try_from(proto).unwrap();

        assert_eq!(decoded, state);
    }
}
